//! xdg-desktop-portal ScreenCast session (via ashpd): VIRTUAL (or MONITOR) source, cursor as
//! metadata, restore token persistence.
//!
//! The async calls are driven from a runtime owned by [`PortalCapture`]; callers must be on a plain
//! thread (the session's video thread), never on a tokio worker.

use ashpd::desktop::PersistMode;
use ashpd::desktop::screencast::{
    CursorMode, Screencast, SelectSourcesOptions, SourceType, Stream,
};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Which kind of source to request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// A new KWin virtual output.
    Virtual,
    /// An existing monitor (the portal dialog lets the user pick it).
    Monitor,
}

impl SourceKind {
    fn token_file(self) -> &'static str {
        match self {
            SourceKind::Virtual => "kde-screencast-token",
            SourceKind::Monitor => "kde-screencast-token-monitor",
        }
    }
}

/// `$XDG_STATE_HOME/ezdisplay/<file>`, falling back to `~/.local/state/ezdisplay/<file>`.
pub fn token_path_from(
    xdg_state_home: Option<&str>,
    home: Option<&str>,
    file: &str,
) -> Option<PathBuf> {
    let base = match xdg_state_home.filter(|s| !s.is_empty()) {
        Some(s) => PathBuf::from(s),
        None => Path::new(home.filter(|s| !s.is_empty())?).join(".local/state"),
    };
    Some(base.join("ezdisplay").join(file))
}

/// Token file for `kind` using the process environment.
pub fn token_path(kind: SourceKind) -> Option<PathBuf> {
    token_path_from(
        std::env::var("XDG_STATE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
        kind.token_file(),
    )
}

fn load_token(path: &Path) -> Option<String> {
    let s = std::fs::read_to_string(path).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

fn save_token(path: &Path, token: &str) {
    let res = (|| -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, format!("{token}\n"))
    })();
    if let Err(e) = res {
        tracing::warn!("cannot save screencast restore token to {path:?}: {e}");
    }
}

/// An open ScreenCast session with its PipeWire remote.
pub struct PortalCapture {
    rt: tokio::runtime::Runtime,
    session: Option<ashpd::desktop::Session<Screencast>>,
    /// Connected PipeWire socket for the session's node.
    pub fd: Option<OwnedFd>,
    /// PipeWire node id of the stream.
    pub node_id: u32,
    /// Stream size as reported by the portal (logical pixels).
    pub size: Option<(i32, i32)>,
    /// Stream position in the compositor space.
    pub position: Option<(i32, i32)>,
    /// Portal stream id (compositor-specific output identifier).
    pub stream_id: Option<String>,
    /// True if the saved restore token was accepted without a dialog.
    pub restored: bool,
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("ezd-portal")
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start portal runtime: {e}"))
}

struct Started {
    session: ashpd::desktop::Session<Screencast>,
    stream: Stream,
    token: Option<String>,
    fd: OwnedFd,
}

async fn try_start(
    kind: SourceKind,
    restore_token: Option<&str>,
    persist: bool,
) -> Result<Started, String> {
    let e = |what: &str, err: ashpd::Error| format!("portal {what}: {err}");
    let proxy = Screencast::new().await.map_err(|x| e("connect", x))?;
    let session = proxy
        .create_session(Default::default())
        .await
        .map_err(|x| e("CreateSession", x))?;
    let sources = match kind {
        SourceKind::Virtual => SourceType::Virtual,
        SourceKind::Monitor => SourceType::Monitor,
    };
    let opts = SelectSourcesOptions::default()
        .set_cursor_mode(CursorMode::Metadata)
        .set_sources(ashpd::enumflags2::BitFlags::from_flag(sources))
        .set_multiple(false)
        .set_restore_token(restore_token)
        .set_persist_mode(if persist {
            PersistMode::ExplicitlyRevoked
        } else {
            PersistMode::DoNot
        });
    let req = proxy
        .select_sources(&session, opts)
        .await
        .map_err(|x| e("SelectSources", x))?;
    req.response().map_err(|x| e("SelectSources", x))?;
    let streams = proxy
        .start(&session, None, Default::default())
        .await
        .map_err(|x| e("Start", x))?
        .response()
        .map_err(|x| e("Start", x))?;
    let stream = streams
        .streams()
        .first()
        .cloned()
        .ok_or("portal returned no streams")?;
    let fd = proxy
        .open_pipe_wire_remote(&session, Default::default())
        .await
        .map_err(|x| e("OpenPipeWireRemote", x))?;
    Ok(Started {
        session,
        stream,
        token: streams.restore_token().map(str::to_string),
        fd,
    })
}

impl PortalCapture {
    /// Opens a session. This may show the portal dialog when there is no valid restore token;
    /// `timeout` bounds the whole interaction.
    pub fn open(kind: SourceKind, timeout: Duration) -> Result<PortalCapture, String> {
        let rt = runtime()?;
        let path = token_path(kind);
        let token = path.as_deref().and_then(load_token);
        let had_token = token.is_some();
        let t0 = std::time::Instant::now();
        let started = rt.block_on(async {
            tokio::time::timeout(timeout, async {
                match try_start(kind, token.as_deref(), true).await {
                    Ok(s) => Ok(s),
                    Err(err) if had_token => {
                        tracing::warn!("restore token rejected ({err}); asking via the dialog");
                        try_start(kind, None, true).await
                    }
                    Err(err) => Err(err),
                }
            })
            .await
            .map_err(|_| {
                format!(
                    "portal did not answer within {timeout:?} (is the selection dialog waiting for a click?)"
                )
            })?
        })?;
        // A new token is issued on every Start; the old one is single-use.
        match (&started.token, &path) {
            (Some(t), Some(p)) => save_token(p, t),
            (None, _) => tracing::warn!("portal gave no restore token; the dialog will reappear"),
            _ => {}
        }
        // Only for logging: a restored session answers without waiting for a click.
        let restored = had_token && t0.elapsed() < Duration::from_secs(3);
        Ok(PortalCapture {
            rt,
            node_id: started.stream.pipe_wire_node_id(),
            size: started.stream.size(),
            position: started.stream.position(),
            stream_id: started.stream.id().map(str::to_string),
            restored,
            fd: Some(started.fd),
            session: Some(started.session),
        })
    }

    /// Closes the portal session (destroys the virtual output).
    pub fn close(&mut self) {
        if let Some(s) = self.session.take() {
            let r = self
                .rt
                .block_on(async { tokio::time::timeout(Duration::from_secs(3), s.close()).await });
            if !matches!(r, Ok(Ok(()))) {
                tracing::debug!("closing portal session: {r:?}");
            }
        }
    }
}

impl Drop for PortalCapture {
    fn drop(&mut self) {
        self.close();
    }
}

/// Reads the ScreenCast portal properties; never opens a dialog. Safe to call from any thread.
pub fn query_properties() -> serde_json::Value {
    let work = || -> Result<serde_json::Value, String> {
        let rt = runtime()?;
        rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(3), async {
                let p = Screencast::new().await.map_err(|e| e.to_string())?;
                let types = p
                    .available_source_types()
                    .await
                    .map_err(|e| e.to_string())?;
                let cursors = p
                    .available_cursor_modes()
                    .await
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>(serde_json::json!({
                    "version": p.version(),
                    "available_source_types": types.bits(),
                    "source_types": {
                        "monitor": types.contains(SourceType::Monitor),
                        "window": types.contains(SourceType::Window),
                        "virtual": types.contains(SourceType::Virtual),
                    },
                    "available_cursor_modes": cursors.bits(),
                    "cursor_modes": {
                        "hidden": cursors.contains(CursorMode::Hidden),
                        "embedded": cursors.contains(CursorMode::Embedded),
                        "metadata": cursors.contains(CursorMode::Metadata),
                    },
                }))
            })
            .await
            .map_err(|_| "timeout talking to the portal".to_string())?
        })
    };
    // Own thread: callers may be inside a tokio runtime, where block_on would panic.
    match std::thread::spawn(work).join() {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => serde_json::json!({ "error": e }),
        Err(_) => serde_json::json!({ "error": "portal query panicked" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_path_prefers_xdg_state_home() {
        assert_eq!(
            token_path_from(Some("/s"), Some("/h"), "t").unwrap(),
            PathBuf::from("/s/ezdisplay/t")
        );
        assert_eq!(
            token_path_from(None, Some("/h"), "kde-screencast-token").unwrap(),
            PathBuf::from("/h/.local/state/ezdisplay/kde-screencast-token")
        );
        assert_eq!(
            token_path_from(Some(""), Some("/h"), "t").unwrap(),
            PathBuf::from("/h/.local/state/ezdisplay/t")
        );
        assert!(token_path_from(None, None, "t").is_none());
    }

    #[test]
    fn token_roundtrip() {
        let dir = std::env::temp_dir().join(format!("ezd-token-{}", std::process::id()));
        let p = dir.join("nested/ezdisplay/tok");
        assert_eq!(load_token(&p), None);
        save_token(&p, "abc-123");
        assert_eq!(load_token(&p).as_deref(), Some("abc-123"));
        std::fs::write(&p, "  \n").unwrap();
        assert_eq!(load_token(&p), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn kinds_use_different_token_files() {
        assert_ne!(
            SourceKind::Virtual.token_file(),
            SourceKind::Monitor.token_file()
        );
        assert_eq!(SourceKind::Virtual.token_file(), "kde-screencast-token");
    }
}

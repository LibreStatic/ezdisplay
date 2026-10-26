//! KDE Plasma provider: a KWin virtual output through the ScreenCast portal, sized with
//! `kscreen-doctor`.

use crate::kscreen;
use crate::portal::{PortalCapture, SourceKind};
use crate::provider::{FrameSource, OpenedOutput, OutputRequest, VirtualOutputProvider};
use crate::pw::Target;
use ezd_host_core::{DisplayMode, OutputInfo, SourceError};
use std::time::Duration;

fn portal_timeout() -> Duration {
    let secs = std::env::var("EZD_PORTAL_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(120);
    Duration::from_secs(secs)
}

/// Provider for KWin.
pub struct KdeProvider;

impl VirtualOutputProvider for KdeProvider {
    fn name(&self) -> &'static str {
        "kde"
    }

    fn open(&mut self, req: &OutputRequest) -> Result<OpenedOutput, SourceError> {
        let virtual_kind = req.kind == SourceKind::Virtual;
        let before = if virtual_kind && kscreen::available() {
            kscreen::list_outputs().ok()
        } else {
            None
        };
        let mut portal = PortalCapture::open(req.kind, portal_timeout()).map_err(|e| {
            let hint = virtual_kind
                .then(|| kscreen::list_outputs().ok())
                .flatten()
                .and_then(|outs| kscreen::disabled_virtual_hint(&outs));
            SourceError::msg(match hint {
                Some(h) => format!("{e}. {h}"),
                None => e,
            })
        })?;
        tracing::info!(
            node = portal.node_id,
            restored = portal.restored,
            size = ?portal.size,
            position = ?portal.position,
            stream = ?portal.stream_id,
            "screencast session open"
        );

        let mut output = None;
        if virtual_kind {
            match &before {
                Some(before) => output = switch_mode(before, req),
                None => tracing::warn!(
                    "kscreen-doctor unavailable: the virtual display keeps its default size"
                ),
            }
        } else if let Some(name) = &req.output_name
            && kscreen::available()
        {
            output = kscreen::list_outputs()
                .ok()
                .and_then(|outs| outs.into_iter().find(|o| &o.name == name));
        }
        let frames = FrameSource::PipeWire {
            fd: portal.fd.take(),
            target: Target::Node(portal.node_id),
        };
        Ok(OpenedOutput {
            frames,
            info: output.as_ref().map(output_info),
            guard: Box::new(portal),
        })
    }
}

/// The platform-neutral description of a kscreen output.
fn output_info(o: &kscreen::KOutput) -> OutputInfo {
    OutputInfo {
        name: o.name.clone(),
        mode: DisplayMode {
            width: o.size.0,
            height: o.size.1,
            refresh_mhz: o
                .modes
                .iter()
                .find(|m| m.id == o.current_mode_id)
                .map_or(0, |m| (m.refresh_hz * 1000.0).round() as u32),
        },
        origin: o.pos,
        adapter: None,
    }
}

/// Best-effort switch of the new virtual output to the negotiated mode.
/// Returns the output as KWin reports it after the switch.
fn switch_mode(before: &[kscreen::KOutput], req: &OutputRequest) -> Option<kscreen::KOutput> {
    let mut found = None;
    for _ in 0..20 {
        if let Ok(after) = kscreen::list_outputs() {
            found = kscreen::find_new_output(before, &after)
                .or_else(|| kscreen::find_virtual_output(&after))
                .map(|o| o.name.clone());
            if found.is_some() {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let Some(name) = found else {
        tracing::warn!("could not find the new virtual output; keeping its default mode");
        return None;
    };
    let (width, height) = (req.width, req.height);
    match kscreen::apply_mode(&name, width, height, req.refresh_mhz) {
        Ok(o) => {
            tracing::info!("{name} set to {width}x{height}");
            // Re-list: the position and scale can change with the mode.
            kscreen::list_outputs()
                .ok()
                .and_then(|outs| outs.into_iter().find(|x| x.name == name))
                .or(Some(o))
        }
        Err(e) => {
            tracing::warn!("could not set {name} to {width}x{height}: {e}");
            kscreen::list_outputs()
                .ok()
                .and_then(|outs| outs.into_iter().find(|x| x.name == name))
        }
    }
}

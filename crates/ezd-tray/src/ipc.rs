//! Single-instance protocol over a local socket (abstract socket on Linux, named pipe on
//! Windows). One request per connection: a line with the command, answered by a line when the
//! request has been carried out.

use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Stream, prelude::*};
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

/// What another instance can ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Open the settings window (also brings it to the front).
    Show,
    /// Stop the service and exit; the reply comes after that.
    Quit,
}

impl Command {
    fn wire(self) -> &'static str {
        match self {
            Command::Show => "show",
            Command::Quit => "quit",
        }
    }

    fn parse(s: &str) -> Option<Command> {
        match s.trim() {
            "show" => Some(Command::Show),
            "quit" => Some(Command::Quit),
            _ => None,
        }
    }
}

/// A request received by the primary instance. Send on `done` when it has been carried out; the
/// client waits for that.
pub struct Request {
    pub command: Command,
    pub done: Sender<()>,
}

/// Name of the socket for this user. `suffix` separates instances that use another config file
/// (tests, manual runs) from the real one.
pub fn socket_name(suffix: Option<&str>) -> String {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".into());
    let user: String = user
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    match suffix {
        Some(s) => format!("ezdisplay-{user}-{s}"),
        None => format!("ezdisplay-{user}"),
    }
}

/// Why [`Server::bind`] failed.
#[derive(Debug)]
pub enum BindError {
    /// Another instance owns the name.
    InUse,
    Other(std::io::Error),
}

/// The listening side of the primary instance. Requests arrive on [`Server::requests`].
pub struct Server {
    pub requests: Receiver<Request>,
}

impl Server {
    /// Claims `name` and serves it on a background thread. `on_request` runs after each request
    /// was queued (to wake the main loop).
    pub fn bind(
        name: &str,
        on_request: impl Fn() + Send + Sync + 'static,
    ) -> Result<Server, BindError> {
        let ns = name
            .to_ns_name::<GenericNamespaced>()
            .map_err(BindError::Other)?;
        let options = ListenerOptions::new().name(ns);
        #[cfg(windows)]
        let options = restrict_to_user(options);
        let listener = options.create_sync().map_err(|e| match e.kind() {
            std::io::ErrorKind::AddrInUse => BindError::InUse,
            // The first pipe instance is created with FILE_FLAG_FIRST_PIPE_INSTANCE,
            // which fails with ERROR_ACCESS_DENIED when the name is taken.
            #[cfg(windows)]
            std::io::ErrorKind::PermissionDenied => BindError::InUse,
            _ => BindError::Other(e),
        })?;
        let (tx, requests) = channel();
        let on_request = std::sync::Arc::new(on_request);
        std::thread::Builder::new()
            .name("ipc-accept".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    match conn {
                        Ok(conn) => {
                            let tx = tx.clone();
                            let on_request = on_request.clone();
                            let _ = std::thread::Builder::new()
                                .name("ipc-conn".into())
                                .spawn(move || serve(conn, &tx, &*on_request));
                        }
                        Err(e) => tracing::warn!("ipc accept failed: {e}"),
                    }
                }
            })
            .map_err(BindError::Other)?;
        Ok(Server { requests })
    }
}

/// An elevated process creates its pipe with a default DACL that only lets administrators in, which
/// would lock out the same user's normal processes (a Start menu launch asking to show the window).
/// So the pipe gets an explicit descriptor: SYSTEM, administrators and this user, plus a Medium
/// integrity label so that normal-integrity clients may write to it. Other users get nothing. The
/// user needs the generic file rights, which include creating further pipe instances; they run
/// as the same account anyway.
#[cfg(windows)]
fn restrict_to_user(options: ListenerOptions<'_>) -> ListenerOptions<'_> {
    use interprocess::os::windows::{
        local_socket::ListenerOptionsExt, security_descriptor::SecurityDescriptor,
    };
    let Some(sid) = ezd_host_windows::current_user_sid() else {
        tracing::warn!("ipc: cannot read the user SID; the pipe keeps its default access");
        return options;
    };
    let sddl = format!("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{sid})S:(ML;;NW;;;ME)");
    let sd = widestring::U16CString::from_str(&sddl)
        .map_err(std::io::Error::other)
        .and_then(|w| SecurityDescriptor::deserialize(&w));
    match sd {
        Ok(sd) => options.security_descriptor(sd),
        Err(e) => {
            tracing::warn!("ipc: cannot build the pipe access rules ({e}); using the default");
            options
        }
    }
}

/// Whether an instance already listens on `name` (connects and hangs up; nothing is requested).
#[cfg(any(windows, test))]
pub fn is_running(name: &str) -> bool {
    let Ok(ns) = name.to_ns_name::<GenericNamespaced>() else {
        return false;
    };
    match Stream::connect(ns) {
        Ok(_) => true,
        // An instance that exists but denies us is still an instance.
        Err(e) => e.kind() == std::io::ErrorKind::PermissionDenied,
    }
}

fn serve(conn: Stream, tx: &Sender<Request>, on_request: &(dyn Fn() + Send + Sync)) {
    let mut reader = BufReader::new(conn);
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let Some(command) = Command::parse(&line) else {
        tracing::warn!("ignoring unknown ipc request {line:?}");
        return;
    };
    let (done, finished) = channel();
    if tx.send(Request { command, done }).is_err() {
        return;
    }
    on_request();
    // Dropping `done` without sending (the app is exiting) also ends the wait.
    let _ = finished.recv();
    let _ = reader.get_mut().write_all(b"ok\n");
}

/// Why [`send`] failed.
#[derive(Debug)]
pub enum SendError {
    /// Nobody listens on the name.
    NoInstance,
    /// The instance did not answer in time.
    Timeout,
    Io(std::io::Error),
}

/// Sends `command` to the instance that owns `name` and waits until it has been carried out, or
/// `timeout` passed.
pub fn send(name: &str, command: Command, timeout: Duration) -> Result<(), SendError> {
    let ns = name
        .to_ns_name::<GenericNamespaced>()
        .map_err(SendError::Io)?;
    let conn = Stream::connect(ns).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
            SendError::NoInstance
        }
        _ => SendError::Io(e),
    })?;
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(conn);
        let r = reader
            .get_mut()
            .write_all(format!("{}\n", command.wire()).as_bytes())
            .and_then(|()| {
                let mut line = String::new();
                reader.read_line(&mut line).map(|_| ())
            });
        let _ = tx.send(r);
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(SendError::Io(e)),
        Err(_) => Err(SendError::Timeout),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_name() -> String {
        static N: AtomicU32 = AtomicU32::new(0);
        format!(
            "ezdisplay-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        )
    }

    #[test]
    fn no_instance() {
        let r = send(&unique_name(), Command::Show, Duration::from_secs(2));
        assert!(matches!(r, Err(SendError::NoInstance)), "{r:?}");
    }

    #[test]
    fn is_running_follows_the_listener() {
        let name = unique_name();
        assert!(!is_running(&name));
        let _server = Server::bind(&name, || {}).unwrap();
        assert!(is_running(&name));
    }

    #[test]
    fn second_bind_reports_in_use() {
        let name = unique_name();
        let _first = Server::bind(&name, || {}).unwrap();
        assert!(matches!(Server::bind(&name, || {}), Err(BindError::InUse)));
    }

    #[test]
    fn request_is_answered_after_it_was_carried_out() {
        let name = unique_name();
        let server = Server::bind(&name, || {}).unwrap();
        let client = {
            let name = name.clone();
            std::thread::spawn(move || send(&name, Command::Quit, Duration::from_secs(10)))
        };
        let req = server
            .requests
            .recv_timeout(Duration::from_secs(5))
            .expect("request");
        assert_eq!(req.command, Command::Quit);
        // The client keeps waiting until the request is done.
        std::thread::sleep(Duration::from_millis(200));
        assert!(!client.is_finished());
        req.done.send(()).unwrap();
        client.join().unwrap().unwrap();
    }

    #[test]
    fn show_round_trip() {
        let name = unique_name();
        let server = Server::bind(&name, || {}).unwrap();
        let client = {
            let name = name.clone();
            std::thread::spawn(move || send(&name, Command::Show, Duration::from_secs(10)))
        };
        let req = server
            .requests
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert_eq!(req.command, Command::Show);
        req.done.send(()).unwrap();
        client.join().unwrap().unwrap();
    }

    #[test]
    fn names_are_per_user_and_suffix() {
        assert!(socket_name(None).starts_with("ezdisplay-"));
        assert_ne!(socket_name(None), socket_name(Some("abc")));
    }
}

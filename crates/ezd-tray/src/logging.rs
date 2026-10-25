//! Daily rolling log files (7 kept) and a panic hook that writes to them.

use std::path::Path;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Keep the guard alive for the whole run: dropping it flushes and stops the writer thread.
pub fn init(dir: &Path) -> Option<WorkerGuard> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,ezd_tray=debug".into());
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("ezdisplay")
        .filename_suffix("log")
        .max_log_files(7)
        .build(dir);
    let (file_layer, guard) = match appender {
        Ok(a) => {
            let (writer, guard) = tracing_appender::non_blocking(a);
            let layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(writer);
            (Some(layer), Some(guard))
        }
        Err(e) => {
            eprintln!("cannot write logs to {}: {e}", dir.display());
            (None, None)
        }
    };
    // The Windows build has no console; elsewhere the log also goes to stderr.
    let stderr_layer =
        cfg!(not(windows)).then(|| tracing_subscriber::fmt::layer().with_writer(std::io::stderr));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init();

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(
            "panic: {info}\n{}",
            std::backtrace::Backtrace::force_capture()
        );
        previous(info);
    }));
    guard
}

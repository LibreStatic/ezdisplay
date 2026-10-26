//! Localized wording of the service state, shared by the tray menu and the window.

use crate::state::Lifecycle;
use ezd_host_service::{DeviceState, HostStatus, ServiceError};
use i18n_embed::fluent::FluentLanguageLoader;
use i18n_embed_fl::fl;

/// `number`, written with `.`, with the decimal separator of the loader's language: every
/// language of the app other than English writes "59,94".
pub fn decimal(l: &FluentLanguageLoader, number: String) -> String {
    let english = l
        .current_languages()
        .first()
        .is_none_or(|id| id.language.as_str() == "en");
    if english {
        number
    } else {
        number.replace('.', ",")
    }
}

/// Refresh rate in hertz without trailing zeros: 120000 -> "120", 59940 -> "59.94".
pub fn hz(refresh_mhz: u32) -> String {
    if refresh_mhz.is_multiple_of(1000) {
        (refresh_mhz / 1000).to_string()
    } else {
        let s = format!("{:.2}", f64::from(refresh_mhz) / 1000.0);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Seconds without trailing zeros: 2000 -> "2", 1500 -> "1.5".
pub fn seconds(ms: u32) -> String {
    hz(ms)
}

/// The one-line state: "Streaming to Tab S9 · 2960×1848 at 120 Hz", "Waiting for the tablet", ...
pub fn status_line(l: &FluentLanguageLoader, life: Lifecycle, status: &HostStatus) -> String {
    if matches!(life, Lifecycle::Stopping { .. }) {
        return fl!(l, "status-stopping");
    }
    match status {
        HostStatus::Stopped => fl!(l, "status-stopped"),
        HostStatus::Waiting { devices } => {
            let any_ready = devices.iter().any(|d| d.state == DeviceState::Ready);
            let any_unauthorized = devices.iter().any(|d| d.state == DeviceState::Unauthorized);
            if !any_ready && any_unauthorized {
                fl!(l, "status-unauthorized")
            } else {
                fl!(l, "status-waiting")
            }
        }
        HostStatus::Streaming(s) => fl!(
            l,
            "status-streaming",
            device = s.device_name.as_str(),
            width = s.stream.width,
            height = s.stream.height,
            hz = decimal(l, hz(s.stream.refresh_mhz))
        ),
        HostStatus::Error(e) => error_text(l, e),
    }
}

/// A [`ServiceError`] in the user's language.
pub fn error_text(l: &FluentLanguageLoader, e: &ServiceError) -> String {
    match e {
        ServiceError::Bind { addr, reason } => fl!(
            l,
            "error-bind",
            addr = addr.to_string(),
            reason = reason.as_str()
        ),
        ServiceError::AdbNotFound => fl!(l, "error-adb-not-found"),
        ServiceError::AdbReverse { serial, reason } => fl!(
            l,
            "error-adb-reverse",
            serial = serial.as_str(),
            reason = reason.as_str()
        ),
        ServiceError::UnsupportedPlatform => fl!(l, "error-unsupported"),
        ServiceError::Wifi(reason) => fl!(l, "error-wifi", reason = reason.as_str()),
        ServiceError::Source(reason) => fl!(l, "error-source", reason = reason.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hz_formatting() {
        assert_eq!(hz(120_000), "120");
        assert_eq!(hz(59_940), "59.94");
        assert_eq!(hz(59_900), "59.9");
        assert_eq!(hz(0), "0");
    }

    #[test]
    fn decimal_separator_follows_the_language() {
        let en = crate::i18n::loader_for(&["en".parse().unwrap()]);
        let es = crate::i18n::loader_for(&["es".parse().unwrap()]);
        assert_eq!(decimal(&en, hz(59_940)), "59.94");
        assert_eq!(decimal(&es, hz(59_940)), "59,94");
        assert_eq!(decimal(&es, hz(120_000)), "120");
    }
}

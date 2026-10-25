//! mDNS advertisement: `_ezdisplay._udp.local.` on the Wi-Fi port, so tablets can find the
//! host without typing an address.

use mdns_sd::{ServiceDaemon, ServiceInfo};

/// DNS-SD service type of the host.
pub const SERVICE_TYPE: &str = "_ezdisplay._udp.local.";

/// A registered advertisement; dropping it unregisters the service and stops the daemon.
pub struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

/// The TXT record: `fp` is the short fingerprint, `name` the host name, `pv` the protocol major.
pub fn txt_records(fingerprint_short: &str, host_name: &str) -> Vec<(String, String)> {
    vec![
        ("fp".into(), fingerprint_short.into()),
        ("name".into(), host_name.into()),
        ("pv".into(), ezd_proto::PROTOCOL_MAJOR.to_string()),
    ]
}

impl Advertiser {
    /// Starts advertising on `port`. The instance name is the host name; addresses follow the
    /// network interfaces automatically.
    pub fn start(
        port: u16,
        fingerprint_short: &str,
        host_name: &str,
    ) -> Result<Advertiser, mdns_sd::Error> {
        let daemon = ServiceDaemon::new()?;
        let dns_host = format!("{}.local.", dns_label(host_name));
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            host_name,
            &dns_host,
            "",
            port,
            &txt_records(fingerprint_short, host_name)[..],
        )?
        .enable_addr_auto();
        let fullname = info.get_fullname().to_string();
        if let Err(e) = daemon.register(info) {
            let _ = daemon.shutdown();
            return Err(e);
        }
        Ok(Advertiser { daemon, fullname })
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        if let Ok(rx) = self.daemon.unregister(&self.fullname) {
            // The goodbye packet goes out before the daemon stops.
            let _ = rx.recv_timeout(std::time::Duration::from_millis(500));
        }
        let _ = self.daemon.shutdown();
    }
}

/// A host name as one DNS label: letters, digits and hyphens.
fn dns_label(name: &str) -> String {
    let label: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let label = label.trim_matches('-');
    if label.is_empty() {
        "ezdisplay".into()
    } else {
        label.chars().take(63).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_labels_are_sanitized() {
        assert_eq!(dns_label("My PC (work)"), "My-PC--work");
        assert_eq!(dns_label("---"), "ezdisplay");
        assert_eq!(dns_label(&"a".repeat(100)).len(), 63);
    }

    #[test]
    fn txt_carries_fingerprint_name_and_protocol() {
        let t = txt_records("3f2a-91c0-77de-0b14", "desk");
        assert_eq!(t[0], ("fp".into(), "3f2a-91c0-77de-0b14".into()));
        assert_eq!(t[1], ("name".into(), "desk".into()));
        assert_eq!(t[2].0, "pv");
        assert_eq!(t[2].1, ezd_proto::PROTOCOL_MAJOR.to_string());
    }
}

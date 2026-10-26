//! Wi-Fi in the controller: the switch, the pairing window and the paired devices. Without the
//! `wifi` feature every method is a no-op and the card stays hidden.

#![cfg_attr(not(feature = "wifi"), allow(dead_code))]

use super::Controller;
use crate::ui::view::WifiState;
use ezd_host_service::HostEvent;
use tokio::sync::broadcast;

/// What the controller knows about Wi-Fi besides the configuration.
#[derive(Default)]
pub(super) struct WifiRuntime {
    /// Events of the running service.
    events: Option<broadcast::Receiver<HostEvent>>,
    pairing: Option<crate::ui::view::PairingOffer>,
    notice: Option<crate::ui::view::WifiNotice>,
    paired: Vec<crate::ui::view::PairedEntry>,
}

#[cfg(feature = "wifi")]
impl Controller {
    /// The service just started: follow its events and pick up an open pairing window.
    pub(super) fn wifi_started(&mut self) {
        self.wifi = WifiRuntime::default();
        if let Some(svc) = &self.service {
            self.wifi.events = Some(svc.events());
            // A window opened at start was announced before we subscribed.
            svc.refresh_pairing();
        }
        self.refresh_paired();
    }

    /// The service is going away.
    pub(super) fn wifi_stopped(&mut self) {
        self.wifi = WifiRuntime::default();
    }

    /// Applies the events that arrived since the last call.
    pub(super) fn poll_wifi(&mut self) {
        use crate::ui::view::{PairingOffer, WifiNotice};
        use ezd_host_service::PairingCloseReason;
        use tokio::sync::broadcast::error::TryRecvError;
        loop {
            let Some(rx) = &mut self.wifi.events else {
                return;
            };
            match rx.try_recv() {
                Ok(HostEvent::PairingOpen {
                    pin, uri, expires, ..
                }) => {
                    self.wifi.notice = None;
                    self.wifi.pairing = Some(PairingOffer { pin, uri, expires });
                }
                Ok(HostEvent::PairingClosed { reason }) => {
                    self.wifi.pairing = None;
                    match reason {
                        PairingCloseReason::TooManyAttempts => {
                            self.wifi.notice = Some(WifiNotice::TooManyAttempts);
                        }
                        PairingCloseReason::Expired => self.wifi.notice = Some(WifiNotice::Expired),
                        PairingCloseReason::Closed | PairingCloseReason::Paired => {}
                    }
                }
                Ok(HostEvent::Paired { device }) => {
                    self.wifi.notice = Some(WifiNotice::Paired(device));
                    self.refresh_paired();
                }
                Ok(_) => {}
                Err(TryRecvError::Lagged(_)) => {
                    // Events were lost: ask for the window again and reread the devices.
                    if let Some(svc) = &self.service {
                        svc.refresh_pairing();
                    }
                    self.refresh_paired();
                }
                Err(TryRecvError::Empty | TryRecvError::Closed) => return,
            }
        }
    }

    fn refresh_paired(&mut self) {
        use crate::ui::view::PairedEntry;
        let list = self.service.as_ref().and_then(|s| s.paired_devices());
        self.wifi.paired = match list {
            Some(Ok(devices)) => devices
                .into_iter()
                .map(|d| PairedEntry {
                    name: d.name,
                    fingerprint: d.fingerprint,
                })
                .collect(),
            Some(Err(e)) => {
                tracing::warn!("cannot read the paired devices: {e}");
                Vec::new()
            }
            None => Vec::new(),
        };
    }

    /// Switches the Wi-Fi listener on or off. The service reads it only when it starts, so a
    /// running service restarts (the tablet reconnects).
    pub(super) fn set_wifi(&mut self, on: bool) {
        if on == self.config.service.wifi.is_some() {
            return;
        }
        let mut c = self.config.clone();
        c.service.wifi = on.then(ezd_host_service::WifiConfig::default);
        self.set_config(c, false);
        if self.lifecycle.wants_running() {
            self.stop();
            self.start();
        }
    }

    pub(super) fn pair_device(&mut self) {
        if let Some(svc) = &self.service
            && svc.open_pairing().is_none()
        {
            tracing::warn!("cannot open pairing: Wi-Fi is off");
        }
    }

    pub(super) fn close_pairing(&mut self) {
        if let Some(svc) = &self.service {
            svc.close_pairing();
        }
    }

    pub(super) fn forget_device(&mut self, fingerprint: &str) {
        match self
            .service
            .as_ref()
            .and_then(|s| s.forget_paired_fingerprint(fingerprint))
        {
            Some(Ok(d)) => tracing::info!("forgot the paired device {}", d.name),
            Some(Err(e)) => tracing::warn!("cannot forget the device: {e}"),
            None => {}
        }
        self.refresh_paired();
    }

    /// What the Wi-Fi card shows.
    pub(super) fn wifi_state(&self) -> WifiState {
        WifiState {
            supported: true,
            enabled: self.config.service.wifi.is_some(),
            info: self
                .service
                .as_ref()
                .and_then(|s| s.wifi().borrow().clone()),
            pairing: self.wifi.pairing.clone(),
            paired: self.wifi.paired.clone(),
            notice: self.wifi.notice.clone(),
        }
    }
}

#[cfg(not(feature = "wifi"))]
impl Controller {
    pub(super) fn wifi_started(&mut self) {}
    pub(super) fn wifi_stopped(&mut self) {}
    pub(super) fn poll_wifi(&mut self) {}
    pub(super) fn set_wifi(&mut self, _on: bool) {}
    pub(super) fn pair_device(&mut self) {}
    pub(super) fn close_pairing(&mut self) {}
    pub(super) fn forget_device(&mut self, _fingerprint: &str) {}
    pub(super) fn wifi_state(&self) -> WifiState {
        WifiState::default()
    }
}

//! The adb watcher: keeps `adb reverse` set up for the devices the service handles and reports
//! their state.
//!
//! [`Watcher::poll`] is one blocking step (it runs `adb` processes); the service calls it every
//! couple of seconds from `spawn_blocking`.

use crate::config::AdbMode;
use crate::error::ServiceError;
use crate::status::{DeviceInfo, DeviceState};
use ezd_link::adb::{Adb, AdbDevice, AdbError, DeviceState as AdbState, Transport};
use ezd_proto::DEFAULT_PORT;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Package of the Android app.
pub const APP_PACKAGE: &str = "io.github.facum.ezdisplay";
/// Launcher activity of the Android app (see its manifest).
pub const APP_ACTIVITY: &str = ".MainActivity";

/// The adb operations the service needs; [`Adb`] implements them, tests use a fake.
pub trait AdbClient: Send + Sync {
    /// `adb devices -l`.
    fn devices(&self) -> Result<Vec<AdbDevice>, AdbError>;
    /// `adb -s SERIAL reverse tcp:DEVICE_PORT tcp:HOST_PORT`.
    fn reverse(&self, serial: &str, device_port: u16, host_port: u16) -> Result<(), AdbError>;
    /// `adb -s SERIAL reverse --remove tcp:DEVICE_PORT`.
    fn remove_reverse(&self, serial: &str, device_port: u16) -> Result<(), AdbError>;
    /// `adb -s SERIAL shell ARGS...`, returns stdout.
    fn shell(&self, serial: &str, args: &[&str]) -> Result<String, AdbError>;
}

impl AdbClient for Adb {
    fn devices(&self) -> Result<Vec<AdbDevice>, AdbError> {
        Adb::devices(self)
    }
    fn reverse(&self, serial: &str, device_port: u16, host_port: u16) -> Result<(), AdbError> {
        Adb::reverse(self, serial, device_port, host_port)
    }
    fn remove_reverse(&self, serial: &str, device_port: u16) -> Result<(), AdbError> {
        Adb::remove_reverse(self, serial, device_port)
    }
    fn shell(&self, serial: &str, args: &[&str]) -> Result<String, AdbError> {
        Adb::shell(self, serial, args)
    }
}

/// Finds adb on every poll while it is missing, so installing it takes effect without a restart.
pub type AdbLocator = Arc<dyn Fn() -> Option<Arc<dyn AdbClient>> + Send + Sync>;

/// Locator that looks for the real adb binary.
pub fn locate_system_adb() -> Option<Arc<dyn AdbClient>> {
    Adb::locate()
        .ok()
        .map(|a| Arc::new(a) as Arc<dyn AdbClient>)
}

/// True for wireless-debugging and `host:port` serials.
pub fn is_network_serial(serial: &str) -> bool {
    serial.contains(':')
        || serial.contains("._adb-tls-connect._tcp")
        || serial.contains("._adb._tcp")
}

fn handles(mode: &AdbMode, d: &AdbDevice) -> bool {
    match mode {
        AdbMode::Off => false,
        AdbMode::Serial(s) => d.serial == *s,
        AdbMode::Auto => d.transport != Transport::Tcp && !is_network_serial(&d.serial),
    }
}

/// Result of one [`Watcher::poll`].
#[derive(Debug, Clone, PartialEq)]
pub struct Poll {
    /// The devices handled in `mode`, in adb order.
    pub devices: Vec<DeviceInfo>,
    /// The first `adb reverse` that is still failing.
    pub error: Option<ServiceError>,
}

/// State carried between polls.
#[derive(Default)]
pub struct Watcher {
    /// Serial to the host port its reverse points at.
    reversed: HashMap<String, u16>,
    /// Serials the app was already started on during this connection.
    launched: HashSet<String>,
    models: HashMap<String, Option<String>>,
    /// Serials whose failing reverse was already logged.
    failed: HashSet<String>,
    list_failed: bool,
}

impl Watcher {
    /// Lists the devices, reverses new ready ones to `host_port` (`None`: nothing is listening,
    /// so nothing is set up), forgets the ones that left and, when `open_app` is set, starts the
    /// app on devices that just became usable.
    pub fn poll(
        &mut self,
        adb: &dyn AdbClient,
        mode: &AdbMode,
        host_port: Option<u16>,
        open_app: bool,
    ) -> Poll {
        let listed = match adb.devices() {
            Ok(v) => {
                self.list_failed = false;
                v
            }
            Err(e) => {
                if !self.list_failed {
                    tracing::warn!("cannot list adb devices: {e}");
                }
                self.list_failed = true;
                Vec::new()
            }
        };
        let handled: Vec<&AdbDevice> = listed.iter().filter(|d| handles(mode, d)).collect();
        let usable = |serial: &str| handled.iter().any(|d| d.serial == serial && d.is_ready());

        for serial in self.reversed.keys().cloned().collect::<Vec<_>>() {
            if usable(&serial) {
                continue;
            }
            // Gone devices lose their forwards by themselves; only a device we stopped handling
            // (the mode changed) still has one to remove.
            if listed.iter().any(|d| d.serial == serial && d.is_ready()) {
                self.remove(adb, &serial);
            }
            self.reversed.remove(&serial);
        }
        self.launched.retain(|s| usable(s));
        self.failed.retain(|s| usable(s));
        self.models.retain(|s, _| usable(s));

        let mut error = None;
        for d in handled.iter().filter(|d| d.is_ready()) {
            let Some(port) = host_port else { break };
            if self.reversed.get(&d.serial) == Some(&port) {
                continue;
            }
            match adb.reverse(&d.serial, DEFAULT_PORT, port) {
                Ok(()) => {
                    tracing::info!(serial = %d.serial, "adb reverse tcp:{DEFAULT_PORT} -> tcp:{port}");
                    self.reversed.insert(d.serial.clone(), port);
                    self.failed.remove(&d.serial);
                    if open_app && self.launched.insert(d.serial.clone()) {
                        open_app_on(adb, &d.serial);
                    }
                }
                Err(e) => {
                    if self.failed.insert(d.serial.clone()) {
                        tracing::warn!(serial = %d.serial, "adb reverse failed: {e}");
                    }
                    error.get_or_insert(ServiceError::AdbReverse {
                        serial: d.serial.clone(),
                        reason: e.to_string(),
                    });
                }
            }
        }

        let devices = handled
            .iter()
            .map(|d| DeviceInfo {
                serial: d.serial.clone(),
                state: match &d.state {
                    AdbState::Device => DeviceState::Ready,
                    AdbState::Unauthorized => DeviceState::Unauthorized,
                    AdbState::Offline => DeviceState::Offline,
                    AdbState::Other(s) => DeviceState::Other(s.clone()),
                },
                model: self.model_of(adb, d),
            })
            .collect();
        Poll { devices, error }
    }

    /// Removes every reverse this watcher set up.
    pub fn release_all(&mut self, adb: &dyn AdbClient) {
        for (serial, _) in std::mem::take(&mut self.reversed) {
            self.remove(adb, &serial);
        }
        self.launched.clear();
    }

    /// Whether any reverse is currently set up.
    pub fn has_reverses(&self) -> bool {
        !self.reversed.is_empty()
    }

    fn remove(&self, adb: &dyn AdbClient, serial: &str) {
        match adb.remove_reverse(serial, DEFAULT_PORT) {
            Ok(()) => tracing::info!(serial, "removed adb reverse"),
            Err(e) => tracing::warn!(serial, "could not remove adb reverse: {e}"),
        }
    }

    /// `ro.product.model`, asked once per connection; the `model:` of `adb devices -l` otherwise.
    fn model_of(&mut self, adb: &dyn AdbClient, d: &AdbDevice) -> Option<String> {
        if !d.is_ready() {
            return d.model.as_ref().map(|m| m.replace('_', " "));
        }
        self.models
            .entry(d.serial.clone())
            .or_insert_with(|| {
                let asked = adb
                    .shell(&d.serial, &["getprop", "ro.product.model"])
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                asked.or_else(|| d.model.as_ref().map(|m| m.replace('_', " ")))
            })
            .clone()
    }
}

/// Starts the app if it is installed. Failures are only logged.
fn open_app_on(adb: &dyn AdbClient, serial: &str) {
    let installed = adb
        .shell(serial, &["pm", "path", APP_PACKAGE])
        .is_ok_and(|out| out.contains("package:"));
    if !installed {
        tracing::info!(serial, "the ezdisplay app is not installed on the device");
        return;
    }
    let component = format!("{APP_PACKAGE}/{APP_ACTIVITY}");
    match adb.shell(serial, &["am", "start", "-n", &component]) {
        Ok(_) => tracing::info!(serial, "started the ezdisplay app"),
        Err(e) => tracing::warn!(serial, "could not start the ezdisplay app: {e}"),
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::sync::Mutex;

    pub fn device(serial: &str, state: AdbState) -> AdbDevice {
        let transport = if is_network_serial(serial) {
            Transport::Tcp
        } else {
            Transport::Usb
        };
        AdbDevice {
            serial: serial.into(),
            state,
            model: Some("SM_X110".into()),
            product: None,
            transport_id: None,
            transport,
        }
    }

    /// Scripted adb: `devices` returns the current list; calls are recorded.
    #[derive(Default)]
    pub struct FakeAdb {
        pub list: Mutex<Vec<AdbDevice>>,
        pub calls: Mutex<Vec<String>>,
        pub app_installed: bool,
        pub reverse_fails: bool,
    }

    impl FakeAdb {
        pub fn with_app(list: Vec<AdbDevice>) -> FakeAdb {
            FakeAdb {
                list: Mutex::new(list),
                app_installed: true,
                ..Default::default()
            }
        }
        pub fn set(&self, list: Vec<AdbDevice>) {
            *self.list.lock().unwrap() = list;
        }
        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
        pub fn take_calls(&self) -> Vec<String> {
            std::mem::take(&mut *self.calls.lock().unwrap())
        }
        fn record(&self, s: String) {
            self.calls.lock().unwrap().push(s);
        }
        fn failed(args: &str) -> AdbError {
            AdbError::Failed {
                args: args.into(),
                status: "exit status: 1".into(),
                stderr: "boom".into(),
            }
        }
    }

    impl AdbClient for FakeAdb {
        fn devices(&self) -> Result<Vec<AdbDevice>, AdbError> {
            Ok(self.list.lock().unwrap().clone())
        }
        fn reverse(&self, serial: &str, device_port: u16, host_port: u16) -> Result<(), AdbError> {
            self.record(format!("reverse {serial} {device_port} {host_port}"));
            if self.reverse_fails {
                return Err(Self::failed("reverse"));
            }
            Ok(())
        }
        fn remove_reverse(&self, serial: &str, device_port: u16) -> Result<(), AdbError> {
            self.record(format!("remove_reverse {serial} {device_port}"));
            Ok(())
        }
        fn shell(&self, serial: &str, args: &[&str]) -> Result<String, AdbError> {
            match args {
                ["getprop", _] => Ok("Galaxy Tab\n".into()),
                ["pm", "path", pkg] => {
                    self.record(format!("shell {serial} pm path {pkg}"));
                    if self.app_installed {
                        Ok(format!("package:/data/app/{pkg}/base.apk\n"))
                    } else {
                        Err(Self::failed("shell pm path"))
                    }
                }
                other => {
                    self.record(format!("shell {serial} {}", other.join(" ")));
                    Ok(String::new())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    const PORT: u16 = 27183;
    const AM_START: &str = "shell R1 am start -n io.github.facum.ezdisplay/.MainActivity";

    fn ready(s: &str) -> AdbDevice {
        device(s, AdbState::Device)
    }

    #[test]
    fn ready_device_is_reversed_and_the_app_started() {
        let adb = FakeAdb::with_app(vec![ready("R1")]);
        let mut w = Watcher::default();
        let p = w.poll(&adb, &AdbMode::Auto, Some(PORT), true);
        assert_eq!(
            adb.take_calls(),
            [
                "reverse R1 27183 27183",
                "shell R1 pm path io.github.facum.ezdisplay",
                AM_START
            ]
        );
        assert_eq!(
            p.devices,
            [DeviceInfo {
                serial: "R1".into(),
                state: DeviceState::Ready,
                model: Some("Galaxy Tab".into())
            }]
        );
        assert_eq!(p.error, None);
        // Nothing more to do while it stays connected.
        w.poll(&adb, &AdbMode::Auto, Some(PORT), true);
        assert!(adb.take_calls().is_empty());
    }

    #[test]
    fn unauthorized_and_offline_devices_are_only_reported() {
        let adb = FakeAdb::with_app(vec![
            device("U1", AdbState::Unauthorized),
            device("O1", AdbState::Offline),
            device("X1", AdbState::Other("recovery".into())),
        ]);
        let p = Watcher::default().poll(&adb, &AdbMode::Auto, Some(PORT), true);
        assert!(adb.calls().is_empty());
        let states: Vec<_> = p.devices.iter().map(|d| d.state.clone()).collect();
        assert_eq!(
            states,
            [
                DeviceState::Unauthorized,
                DeviceState::Offline,
                DeviceState::Other("recovery".into())
            ]
        );
        assert_eq!(p.devices[0].model.as_deref(), Some("SM X110"));
    }

    #[test]
    fn reconnect_reverses_again() {
        let adb = FakeAdb::with_app(vec![ready("R1")]);
        let mut w = Watcher::default();
        w.poll(&adb, &AdbMode::Auto, Some(PORT), true);
        adb.take_calls();
        adb.set(vec![]);
        assert!(
            w.poll(&adb, &AdbMode::Auto, Some(PORT), true)
                .devices
                .is_empty()
        );
        assert!(
            adb.take_calls().is_empty(),
            "a gone device needs no cleanup"
        );
        adb.set(vec![ready("R1")]);
        w.poll(&adb, &AdbMode::Auto, Some(PORT), true);
        let calls = adb.take_calls();
        assert_eq!(calls[0], "reverse R1 27183 27183");
        assert_eq!(calls.last().unwrap(), AM_START);
    }

    #[test]
    fn going_unauthorized_forgets_the_reverse() {
        let adb = FakeAdb::with_app(vec![ready("R1")]);
        let mut w = Watcher::default();
        w.poll(&adb, &AdbMode::Auto, Some(PORT), false);
        adb.set(vec![device("R1", AdbState::Offline)]);
        w.poll(&adb, &AdbMode::Auto, Some(PORT), false);
        adb.take_calls();
        adb.set(vec![ready("R1")]);
        w.poll(&adb, &AdbMode::Auto, Some(PORT), false);
        assert_eq!(adb.take_calls(), ["reverse R1 27183 27183"]);
    }

    #[test]
    fn auto_ignores_network_devices() {
        let adb = FakeAdb::with_app(vec![
            ready("192.168.0.5:44371"),
            ready("adb-RZ8N-abc._adb-tls-connect._tcp"),
            ready("adb-RZ8N-abc._adb._tcp"),
            ready("R1"),
        ]);
        let p = Watcher::default().poll(&adb, &AdbMode::Auto, Some(PORT), true);
        assert!(
            adb.calls()
                .iter()
                .all(|c| !c.contains("192.168") && !c.contains("_adb"))
        );
        assert_eq!(p.devices.len(), 1);
        assert_eq!(p.devices[0].serial, "R1");
    }

    #[test]
    fn serial_mode_handles_only_that_serial_on_any_transport() {
        let adb = FakeAdb::with_app(vec![ready("R1"), ready("10.0.0.2:5555")]);
        let mode = AdbMode::Serial("10.0.0.2:5555".into());
        let p = Watcher::default().poll(&adb, &mode, Some(PORT), false);
        assert_eq!(adb.calls(), ["reverse 10.0.0.2:5555 27183 27183"]);
        assert_eq!(p.devices.len(), 1);
    }

    #[test]
    fn missing_app_is_not_started() {
        let adb = FakeAdb {
            list: vec![ready("R1")].into(),
            app_installed: false,
            ..Default::default()
        };
        Watcher::default().poll(&adb, &AdbMode::Auto, Some(PORT), true);
        assert_eq!(
            adb.calls(),
            [
                "reverse R1 27183 27183",
                "shell R1 pm path io.github.facum.ezdisplay"
            ]
        );
    }

    #[test]
    fn app_is_left_alone_when_disabled() {
        let adb = FakeAdb::with_app(vec![ready("R1")]);
        Watcher::default().poll(&adb, &AdbMode::Auto, Some(PORT), false);
        assert_eq!(adb.calls(), ["reverse R1 27183 27183"]);
    }

    #[test]
    fn failing_reverse_is_reported_and_retried() {
        let adb = FakeAdb {
            list: vec![ready("R1")].into(),
            app_installed: true,
            reverse_fails: true,
            ..Default::default()
        };
        let mut w = Watcher::default();
        let p = w.poll(&adb, &AdbMode::Auto, Some(PORT), true);
        assert!(
            matches!(p.error, Some(ServiceError::AdbReverse { ref serial, .. }) if serial == "R1")
        );
        w.poll(&adb, &AdbMode::Auto, Some(PORT), true);
        let reverses = adb
            .calls()
            .iter()
            .filter(|c| c.starts_with("reverse"))
            .count();
        assert_eq!(reverses, 2);
        assert!(!adb.calls().iter().any(|c| c.contains("am start")));
    }

    #[test]
    fn nothing_is_reversed_without_a_listener() {
        let adb = FakeAdb::with_app(vec![ready("R1")]);
        let p = Watcher::default().poll(&adb, &AdbMode::Auto, None, true);
        assert!(adb.calls().is_empty());
        assert_eq!(p.devices.len(), 1);
    }

    #[test]
    fn a_new_host_port_is_reversed_again_without_restarting_the_app() {
        let adb = FakeAdb::with_app(vec![ready("R1")]);
        let mut w = Watcher::default();
        w.poll(&adb, &AdbMode::Auto, Some(PORT), true);
        adb.take_calls();
        w.poll(&adb, &AdbMode::Auto, Some(4000), true);
        assert_eq!(adb.take_calls(), ["reverse R1 27183 4000"]);
    }

    #[test]
    fn release_all_removes_what_was_set_up() {
        let adb = FakeAdb::with_app(vec![ready("R1"), ready("R2")]);
        let mut w = Watcher::default();
        w.poll(&adb, &AdbMode::Auto, Some(PORT), false);
        adb.take_calls();
        w.release_all(&adb);
        let mut calls = adb.take_calls();
        calls.sort();
        assert_eq!(
            calls,
            ["remove_reverse R1 27183", "remove_reverse R2 27183"]
        );
        assert!(!w.has_reverses());
    }

    #[test]
    fn switching_mode_removes_the_reverse_of_a_device_no_longer_handled() {
        let adb = FakeAdb::with_app(vec![ready("R1"), ready("R2")]);
        let mut w = Watcher::default();
        w.poll(&adb, &AdbMode::Auto, Some(PORT), false);
        adb.take_calls();
        w.poll(&adb, &AdbMode::Serial("R2".into()), Some(PORT), false);
        assert_eq!(adb.take_calls(), ["remove_reverse R1 27183"]);
        w.poll(&adb, &AdbMode::Off, Some(PORT), false);
        assert_eq!(adb.take_calls(), ["remove_reverse R2 27183"]);
    }

    #[test]
    fn network_serials() {
        assert!(is_network_serial("192.168.0.5:5555"));
        assert!(is_network_serial("adb-x._adb-tls-connect._tcp"));
        assert!(!is_network_serial("R52X204YJNW"));
        assert!(!is_network_serial("emulator-5554"));
    }
}

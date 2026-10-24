//! KWin's input-device D-Bus interface: maps the virtual touch and pen devices to the virtual
//! output (`outputName`), the only way KWin routes touch / tablet events to the right screen.

use std::time::{Duration, Instant};
use tokio::runtime::Handle;
use zbus::proxy::{Builder, CacheProperties};
use zbus::{Connection, Proxy};

const SERVICE: &str = "org.kde.KWin";
const MANAGER_PATH: &str = "/org/kde/KWin/InputDevice";
const MANAGER_IFACE: &str = "org.kde.KWin.InputDeviceManager";
const DEVICE_IFACE: &str = "org.kde.KWin.InputDevice";
const WAIT: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(50);
const CALL_TIMEOUT: Duration = Duration::from_secs(2);

/// A proxy that always asks KWin: its properties don't emit `PropertiesChanged`, so zbus's
/// property cache would keep the first `devicesSysNames` it read.
async fn proxy<'a>(conn: &Connection, path: &'a str, iface: &'a str) -> zbus::Result<Proxy<'a>> {
    Builder::new(conn)
        .destination(SERVICE)?
        .path(path)?
        .interface(iface)?
        .cache_properties(CacheProperties::No)
        .build()
        .await
}

/// D-Bus object path of the input device `sys_name` (`eventN`).
pub fn device_path(sys_name: &str) -> String {
    format!("{MANAGER_PATH}/{sys_name}")
}

async fn timed<T>(
    what: &str,
    f: impl std::future::Future<Output = Result<T, zbus::Error>>,
) -> Result<T, String> {
    match tokio::time::timeout(CALL_TIMEOUT, f).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(format!("{what}: {e}")),
        Err(_) => Err(format!("{what}: timed out")),
    }
}

async fn map_async(devices: &[(&str, &str)], output: &str) -> Vec<Result<(), String>> {
    let conn = match timed("session bus", Connection::session()).await {
        Ok(c) => c,
        Err(e) => return devices.iter().map(|_| Err(e.clone())).collect(),
    };
    let manager = match timed(
        "KWin input manager",
        proxy(&conn, MANAGER_PATH, MANAGER_IFACE),
    )
    .await
    {
        Ok(p) => p,
        Err(e) => return devices.iter().map(|_| Err(e.clone())).collect(),
    };
    let deadline = Instant::now() + WAIT;
    let mut results: Vec<Option<Result<(), String>>> = vec![None; devices.len()];
    let mut last_err = String::new();
    loop {
        let listed: Result<Vec<String>, String> =
            timed("devicesSysNames", manager.get_property("devicesSysNames")).await;
        match listed {
            Ok(names) => {
                for (i, (label, sys)) in devices.iter().enumerate() {
                    if results[i].is_none() && names.iter().any(|n| n == sys) {
                        results[i] = Some(set_output(&conn, label, sys, output).await);
                    }
                }
            }
            Err(e) => last_err = e,
        }
        if results.iter().all(Option::is_some) || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL).await;
    }
    devices
        .iter()
        .zip(results)
        .map(|((label, sys), r)| {
            r.unwrap_or_else(|| {
                if last_err.is_empty() {
                    Err(format!("KWin did not list the {label} device ({sys})"))
                } else {
                    Err(format!(
                        "KWin did not list the {label} device ({sys}): {last_err}"
                    ))
                }
            })
        })
        .collect()
}

async fn set_output(conn: &Connection, label: &str, sys: &str, output: &str) -> Result<(), String> {
    let path = device_path(sys);
    let dev = timed(
        "KWin input device",
        proxy(conn, path.as_str(), DEVICE_IFACE),
    )
    .await?;
    let r = async {
        dev.set_property("outputName", output.to_string())
            .await
            .map_err(zbus::Error::from)?;
        dev.get_property::<String>("outputName").await
    };
    let got = timed(&format!("mapping the {label} device"), r).await?;
    if got == output {
        Ok(())
    } else {
        Err(format!(
            "KWin kept outputName {got:?} for the {label} device instead of {output:?}"
        ))
    }
}

/// Maps each `(label, eventN)` device to `output`. Waits up to 2 s for KWin to list them.
/// One result per device, in order. Call from a thread outside the runtime.
pub fn map_devices(
    handle: &Handle,
    devices: &[(&str, &str)],
    output: &str,
) -> Vec<Result<(), String>> {
    handle.block_on(map_async(devices, output))
}

/// True if KWin's input device manager is on the session bus (for `ezd-host diag`). Uses its
/// own runtime on a helper thread, so it works inside and outside a tokio runtime.
pub fn manager_available() -> bool {
    std::thread::spawn(|| {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return false;
        };
        rt.block_on(async {
            let Ok(conn) = timed("session bus", Connection::session()).await else {
                return false;
            };
            let Ok(p) = timed("manager", proxy(&conn, MANAGER_PATH, MANAGER_IFACE)).await else {
                return false;
            };
            timed::<Vec<String>>("devicesSysNames", p.get_property("devicesSysNames"))
                .await
                .is_ok()
        })
    })
    .join()
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_paths() {
        assert_eq!(device_path("event7"), "/org/kde/KWin/InputDevice/event7");
    }
}

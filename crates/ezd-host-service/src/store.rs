//! The host's pairing data: the identity certificate and the list of paired devices.
//!
//! `paired.json` is a JSON array of [`PairedDevice`]. Times are Unix seconds (UTC). The file is
//! the source of truth and is read on every call, so `ezd-host paired forget` takes effect in a
//! running service. A file that does not parse is an error, never silently replaced: the
//! owner must fix or delete it.

use ezd_link::identity::{fingerprint_hex, parse_fingerprint, write_atomic};
use ezd_link::pair::PairedClient;
use ezd_proto::pair::Fingerprint;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Name of the paired-devices file inside the config directory.
pub const PAIRED_FILE: &str = "paired.json";

/// The default config directory: `ezdisplay` inside the user's config directory.
pub fn default_config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("ezdisplay"))
}

/// Seconds since the Unix epoch.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A device allowed to start sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedDevice {
    /// Name the device announced when it paired.
    pub name: String,
    /// Certificate fingerprint, lower-case hex.
    pub fingerprint: String,
    /// When it paired, Unix seconds.
    pub paired_at: u64,
    /// When it last started a session, Unix seconds.
    pub last_seen: Option<u64>,
}

/// Why the store cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The file could not be read or written.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// The OS error.
        source: io::Error,
    },
    /// The file is not a valid list of devices; it was left untouched.
    #[error("{} is corrupt: {reason}", path.display())]
    Corrupt {
        /// The file.
        path: PathBuf,
        /// What is wrong with it.
        reason: String,
    },
}

/// Why a device could not be forgotten.
#[derive(Debug, thiserror::Error)]
pub enum ForgetError {
    /// Nothing matches the name or fingerprint.
    #[error("no paired device matches")]
    NotFound,
    /// More than one device matches; use more of the fingerprint.
    #[error("{0} paired devices match")]
    Ambiguous(usize),
    /// The store is unusable.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// The paired devices of a config directory.
#[derive(Debug)]
pub struct PairedStore {
    path: PathBuf,
    /// Serializes read-modify-write cycles inside this process.
    lock: Mutex<()>,
}

impl PairedStore {
    /// The store in `dir`; the file is created by the first pairing.
    pub fn new(dir: &Path) -> PairedStore {
        PairedStore {
            path: dir.join(PAIRED_FILE),
            lock: Mutex::new(()),
        }
    }

    /// Where the list is kept.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn io_err(&self, source: io::Error) -> StoreError {
        StoreError::Io {
            path: self.path.clone(),
            source,
        }
    }

    fn read(&self) -> Result<Vec<PairedDevice>, StoreError> {
        let data = match std::fs::read(&self.path) {
            Ok(d) => d,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(self.io_err(e)),
        };
        let corrupt = |reason: String| StoreError::Corrupt {
            path: self.path.clone(),
            reason,
        };
        let devices: Vec<PairedDevice> =
            serde_json::from_slice(&data).map_err(|e| corrupt(e.to_string()))?;
        if let Some(d) = devices
            .iter()
            .find(|d| parse_fingerprint(&d.fingerprint).is_none())
        {
            return Err(corrupt(format!("invalid fingerprint for {:?}", d.name)));
        }
        Ok(devices)
    }

    fn write(&self, devices: &[PairedDevice]) -> Result<(), StoreError> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| self.io_err(e))?;
        }
        let json = serde_json::to_vec_pretty(devices).expect("devices serialize");
        write_atomic(&self.path, &json).map_err(|e| self.io_err(e))
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Every paired device, oldest first. A missing file is an empty list.
    pub fn list(&self) -> Result<Vec<PairedDevice>, StoreError> {
        let _g = self.guard();
        self.read()
    }

    /// True if a device with this fingerprint is paired.
    pub fn is_paired(&self, fp: &Fingerprint) -> Result<bool, StoreError> {
        let hex = fingerprint_hex(fp);
        Ok(self.list()?.iter().any(|d| d.fingerprint == hex))
    }

    /// Adds a device, or replaces the one with the same fingerprint (new name, new pairing
    /// time, `last_seen` cleared).
    pub fn add(&self, client: &PairedClient) -> Result<(), StoreError> {
        let _g = self.guard();
        let mut devices = self.read()?;
        let hex = fingerprint_hex(&client.fingerprint);
        devices.retain(|d| d.fingerprint != hex);
        devices.push(PairedDevice {
            name: client.name.clone(),
            fingerprint: hex,
            paired_at: now_secs(),
            last_seen: None,
        });
        self.write(&devices)
    }

    /// Records that the device started a session now. Unknown devices are ignored.
    pub fn touch(&self, fp: &Fingerprint) -> Result<(), StoreError> {
        let _g = self.guard();
        let mut devices = self.read()?;
        let hex = fingerprint_hex(fp);
        let Some(d) = devices.iter_mut().find(|d| d.fingerprint == hex) else {
            return Ok(());
        };
        d.last_seen = Some(now_secs());
        self.write(&devices)
    }

    /// Removes the device whose name (any case) equals `query`, or whose fingerprint is
    /// `query` in hex or in the short dashed form, in full or as a prefix of at least 4 digits.
    /// A name wins over a fingerprint prefix.
    pub fn forget(&self, query: &str) -> Result<PairedDevice, ForgetError> {
        let _g = self.guard();
        let mut devices = self.read()?;
        let query = query.trim();
        let mut hits: Vec<usize> = (0..devices.len())
            .filter(|&i| devices[i].name.eq_ignore_ascii_case(query))
            .collect();
        if hits.is_empty() {
            let prefix: String = query
                .chars()
                .filter(|c| *c != '-')
                .collect::<String>()
                .to_ascii_lowercase();
            if prefix.len() >= 4 && prefix.chars().all(|c| c.is_ascii_hexdigit()) {
                hits = (0..devices.len())
                    .filter(|&i| devices[i].fingerprint.starts_with(&prefix))
                    .collect();
            }
        }
        match hits.as_slice() {
            [] => Err(ForgetError::NotFound),
            [i] => {
                let gone = devices.remove(*i);
                self.write(&devices)?;
                Ok(gone)
            }
            more => Err(ForgetError::Ambiguous(more.len())),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A unique directory removed on drop.
    pub(crate) struct TempDir(pub PathBuf);

    impl TempDir {
        pub(crate) fn new(tag: &str) -> TempDir {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir()
                .join(format!("ezd-host-service-{tag}-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn client(name: &str, b: u8) -> PairedClient {
        PairedClient {
            fingerprint: [b; 32],
            name: name.into(),
        }
    }

    #[test]
    fn missing_file_is_empty_and_add_round_trips() {
        let dir = TempDir::new("store-rt");
        let store = PairedStore::new(&dir.0.join("nested"));
        assert!(store.list().unwrap().is_empty());
        assert!(!store.is_paired(&[1; 32]).unwrap());
        store.add(&client("Tab S9", 1)).unwrap();
        store.add(&client("Pixel", 2)).unwrap();
        let list = store.list().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "Tab S9");
        assert_eq!(list[0].fingerprint, "01".repeat(32));
        assert_eq!(list[0].last_seen, None);
        assert!(list[0].paired_at > 1_600_000_000);
        assert!(store.is_paired(&[2; 32]).unwrap());
        // A second handle (another process) sees the same data.
        assert_eq!(
            PairedStore::new(&dir.0.join("nested")).list().unwrap(),
            list
        );
    }

    #[test]
    fn adding_the_same_fingerprint_replaces() {
        let dir = TempDir::new("store-replace");
        let store = PairedStore::new(&dir.0);
        store.add(&client("old", 1)).unwrap();
        store.touch(&[1; 32]).unwrap();
        assert!(store.list().unwrap()[0].last_seen.is_some());
        store.add(&client("new", 1)).unwrap();
        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].name.as_str(), list[0].last_seen), ("new", None));
    }

    #[test]
    fn touch_sets_last_seen_and_ignores_unknown() {
        let dir = TempDir::new("store-touch");
        let store = PairedStore::new(&dir.0);
        store.touch(&[9; 32]).unwrap();
        assert!(!store.path().exists());
        store.add(&client("a", 1)).unwrap();
        store.touch(&[1; 32]).unwrap();
        assert!(store.list().unwrap()[0].last_seen.is_some());
    }

    #[test]
    fn forget_by_name_hex_and_short_prefix() {
        let dir = TempDir::new("store-forget");
        let store = PairedStore::new(&dir.0);
        store.add(&client("Tab S9", 0xab)).unwrap();
        store.add(&client("Pixel", 0xcd)).unwrap();
        store.add(&client("Other", 0xcf)).unwrap();
        assert_eq!(store.forget("tab s9").unwrap().name, "Tab S9");
        assert_eq!(store.forget(&"cd".repeat(32)).unwrap().name, "Pixel");
        assert!(matches!(store.forget("nobody"), Err(ForgetError::NotFound)));
        assert!(matches!(store.forget("ab"), Err(ForgetError::NotFound)));
        assert_eq!(store.forget("cfcf-cfcf").unwrap().name, "Other");
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn forget_refuses_an_ambiguous_prefix() {
        let dir = TempDir::new("store-ambiguous");
        let store = PairedStore::new(&dir.0);
        let mut a = client("a", 0);
        a.fingerprint[31] = 1;
        let mut b = client("b", 0);
        b.fingerprint[31] = 2;
        store.add(&a).unwrap();
        store.add(&b).unwrap();
        assert!(matches!(
            store.forget("0000"),
            Err(ForgetError::Ambiguous(2))
        ));
        assert_eq!(store.list().unwrap().len(), 2);
    }

    #[test]
    fn a_corrupt_file_is_an_error_and_is_never_overwritten() {
        let dir = TempDir::new("store-corrupt");
        let store = PairedStore::new(&dir.0);
        for bad in [
            "not json".to_string(),
            r#"[{"name":"x","fingerprint":"zz","paired_at":1,"last_seen":null}]"#.to_string(),
        ] {
            std::fs::write(store.path(), &bad).unwrap();
            assert!(matches!(store.list(), Err(StoreError::Corrupt { .. })));
            assert!(store.is_paired(&[1; 32]).is_err());
            assert!(store.add(&client("a", 1)).is_err());
            assert!(store.touch(&[1; 32]).is_err());
            assert!(matches!(
                store.forget("x"),
                Err(ForgetError::Store(StoreError::Corrupt { .. }))
            ));
            assert_eq!(std::fs::read_to_string(store.path()).unwrap(), bad);
        }
    }
}

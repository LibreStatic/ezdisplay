//! Device identity for Wi-Fi links: a self-signed certificate and its private key.
//!
//! Peers do not use a CA. Each side keeps one long-lived certificate and pins the other's
//! [`Fingerprint`] (SHA-256 of the certificate DER) when they pair.

use ezd_proto::pair::Fingerprint;
use std::fmt::Write as _;
use std::io;
use std::path::Path;

/// Certificate file name inside an identity directory.
pub const CERT_FILE: &str = "identity.crt";
/// Private key file name inside an identity directory.
pub const KEY_FILE: &str = "identity.key";

/// A certificate (DER) and its PKCS#8 private key (DER).
#[derive(Clone)]
pub struct Identity {
    cert_der: Vec<u8>,
    key_der: Vec<u8>,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("fingerprint", &fingerprint_hex(&self.fingerprint()))
            .finish_non_exhaustive()
    }
}

impl Identity {
    /// Generates a new ECDSA P-256 identity.
    pub fn generate() -> io::Result<Identity> {
        let key = rcgen::KeyPair::generate().map_err(io::Error::other)?;
        let params = rcgen::CertificateParams::new(vec!["ezdisplay".to_string()])
            .map_err(io::Error::other)?;
        let cert = params.self_signed(&key).map_err(io::Error::other)?;
        Ok(Identity {
            cert_der: cert.der().to_vec(),
            key_der: key.serialize_der(),
        })
    }

    /// Loads the identity stored in `dir`, or creates and stores a new one when neither file
    /// exists. Only one of the two files is an error rather than a silent new identity, which
    /// would drop every pairing. On Unix the directory is created owner-only and both files are
    /// readable by the owner only.
    pub fn load_or_create(dir: &Path) -> io::Result<Identity> {
        let (cert, key) = (dir.join(CERT_FILE), dir.join(KEY_FILE));
        match (std::fs::read(&cert), std::fs::read(&key)) {
            (Ok(cert_der), Ok(key_der)) => {
                let id = Identity { cert_der, key_der };
                // Fails early on a damaged file instead of at the first handshake.
                id.key_pair()?;
                return Ok(id);
            }
            (Err(c), Err(k))
                if c.kind() == io::ErrorKind::NotFound && k.kind() == io::ErrorKind::NotFound => {}
            (Err(e), _) | (_, Err(e)) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "incomplete identity in {}: one of {CERT_FILE} and {KEY_FILE} is missing",
                        dir.display()
                    ),
                ));
            }
        }
        create_private_dir(dir)?;
        let id = Identity::generate()?;
        // Key first: a crash in between leaves only the key, which the next start reports.
        write_private(&key, &id.key_der)?;
        write_private(&cert, &id.cert_der)?;
        Ok(id)
    }

    /// Certificate DER.
    pub fn cert_der(&self) -> &[u8] {
        &self.cert_der
    }

    /// PKCS#8 private key DER.
    pub fn key_der(&self) -> &[u8] {
        &self.key_der
    }

    /// SHA-256 of the certificate.
    pub fn fingerprint(&self) -> Fingerprint {
        fingerprint_of(&self.cert_der)
    }

    fn key_pair(&self) -> io::Result<rcgen::KeyPair> {
        rcgen::KeyPair::try_from(self.key_der.as_slice()).map_err(io::Error::other)
    }
}

/// SHA-256 of a certificate DER.
pub fn fingerprint_of(cert_der: &[u8]) -> Fingerprint {
    let d = ring::digest::digest(&ring::digest::SHA256, cert_der);
    let mut out = [0u8; 32];
    out.copy_from_slice(d.as_ref());
    out
}

/// Lower-case hex of a fingerprint (64 characters).
pub fn fingerprint_hex(fp: &Fingerprint) -> String {
    let mut s = String::with_capacity(64);
    for b in fp {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Parses [`fingerprint_hex`] output (either case).
pub fn parse_fingerprint(hex: &str) -> Option<Fingerprint> {
    let hex = hex.as_bytes();
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (o, pair) in out.iter_mut().zip(hex.chunks(2)) {
        let s = std::str::from_utf8(pair).ok()?;
        *o = u8::from_str_radix(s, 16).ok()?;
    }
    Some(out)
}

/// Short form for people to compare: the first 8 bytes as 4 dash-separated groups,
/// e.g. `3f2a-91c0-77de-0b14`.
pub fn fingerprint_short(fp: &Fingerprint) -> String {
    let hex = fingerprint_hex(fp);
    [&hex[0..4], &hex[4..8], &hex[8..12], &hex[12..16]].join("-")
}

/// Writes `data` to `path` through a new temporary file, `fsync` and a rename, so readers see
/// the old or the new contents and never a partial file. The file is readable by the owner
/// only (Unix).
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    write_private(path, data)
}

/// Unique temporary name next to `path` (`<name>.<pid>.<n>.tmp`).
fn temp_path(path: &Path) -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.{n}.tmp", std::process::id()));
    path.with_file_name(name)
}

fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    let tmp = temp_path(path);
    let mut opts = std::fs::OpenOptions::new();
    // `create_new` never reuses a leftover or planted file (or follows a symlink).
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    // %APPDATA% is already private to the user on Windows.
    let result = (|| {
        let mut f = opts.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)?;
        sync_dir(path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Makes the rename durable (Unix; a no-op elsewhere).
fn sync_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        let dir = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };
        std::fs::File::open(dir)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ezd-identity-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn load_or_create_is_stable() {
        let d = temp_dir("stable");
        let a = Identity::load_or_create(&d).unwrap();
        let b = Identity::load_or_create(&d).unwrap();
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_ne!(a.fingerprint(), Identity::generate().unwrap().fingerprint());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(d.join(KEY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
            let mode = std::fs::metadata(&d).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn missing_half_is_an_error() {
        let d = temp_dir("half");
        Identity::load_or_create(&d).unwrap();
        std::fs::remove_file(d.join(CERT_FILE)).unwrap();
        assert!(Identity::load_or_create(&d).is_err());
        assert!(!d.join(CERT_FILE).exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn atomic_write_ignores_leftover_temp_files() {
        let d = temp_dir("atomic");
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("data.json");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        let names: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["data.json"]);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn damaged_key_is_an_error() {
        let d = temp_dir("damaged");
        Identity::load_or_create(&d).unwrap();
        std::fs::write(d.join(KEY_FILE), b"garbage").unwrap();
        assert!(Identity::load_or_create(&d).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn fingerprint_text_forms() {
        let fp: Fingerprint = core::array::from_fn(|i| i as u8 * 7);
        let hex = fingerprint_hex(&fp);
        assert_eq!(parse_fingerprint(&hex), Some(fp));
        assert_eq!(parse_fingerprint(&hex.to_uppercase()), Some(fp));
        assert_eq!(parse_fingerprint(&hex[..62]), None);
        assert_eq!(fingerprint_short(&fp), "0007-0e15-1c23-2a31");
    }
}

//! Host side of the Android Open Accessory (AOA) USB transport (libusb through `rusb`).
//!
//! A tablet that shows up as a file-transfer (MTP) or debugging (adb) device is asked to switch to
//! accessory mode with a few vendor control requests. It then re-enumerates as a Google
//! accessory device whose two bulk endpoints carry the normal ezdisplay byte stream, so no
//! developer mode is needed on the tablet.
//!
//! The decisions that need no hardware (which devices may be switched, which one is the
//! accessory after the switch, how a write is split) are plain functions and types, tested with
//! fixtures. [`Aoa`] does the USB I/O and every call on it blocks: use `spawn_blocking` from async
//! code. [`Aoa::open`] must run inside a tokio runtime (it builds a [`Link`] with
//! [`Link::from_blocking`]).

use crate::BlockingOptions;
use crate::link::Link;
use rusb::{Context, Device, DeviceHandle, Direction, Recipient, RequestType, UsbContext};
use std::collections::HashMap;
use std::fmt;
use std::io::{self, ErrorKind, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// USB vendor id of every accessory-mode device.
pub const ACCESSORY_VID: u16 = 0x18D1;
/// Product ids of accessory-mode devices (`2D01`, `2D03` and `2D05` also expose adb).
pub const ACCESSORY_PIDS: std::ops::RangeInclusive<u16> = 0x2D00..=0x2D05;
/// The accessory data interface is always the first one.
pub const ACCESSORY_INTERFACE: u8 = 0;

/// Timeout of every control request of the handshake.
pub const CONTROL_TIMEOUT: Duration = Duration::from_secs(1);
/// How long a switched device may take to show up again as an accessory.
pub const REENUMERATE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a bulk read waits before it returns `TimedOut` (so the link can notice it was dropped).
pub const READ_TIMEOUT: Duration = Duration::from_millis(200);
/// How long a bulk write may take; a timeout is a hard error.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Bytes asked for in each bulk read.
pub const TRANSFER_LEN: usize = 16 * 1024;
/// Most bytes sent in one bulk transfer. Odd, so a full chunk is never a multiple of the
/// endpoint's `max_packet` and is not split by [`transfer_len`]: with 16 KiB chunks every one
/// went out as 16383 + 1 bytes, and each 1-byte transfer cost a full round trip to the tablet.
pub const WRITE_LEN: usize = 64 * 1024 - 1;

const REQ_GET_PROTOCOL: u8 = 51;
const REQ_SEND_STRING: u8 = 52;
const REQ_START: u8 = 53;

/// Why AOA cannot be used on this machine (no libusb, or no UsbDk on Windows).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Unavailable(pub String);

/// True for a device already in accessory mode.
pub fn is_accessory(vid: u16, pid: u16) -> bool {
    vid == ACCESSORY_VID && ACCESSORY_PIDS.contains(&pid)
}

/// True for an accessory-mode device that also exposes adb.
pub fn accessory_has_adb(pid: u16) -> bool {
    matches!(pid, 0x2D01 | 0x2D03 | 0x2D05)
}

/// Class, subclass and protocol of one USB interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceClass {
    /// Class code.
    pub class: u8,
    /// Subclass code.
    pub subclass: u8,
    /// Protocol code.
    pub protocol: u8,
}

impl InterfaceClass {
    /// Still Image / MTP (06/01/01).
    pub fn is_mtp(self) -> bool {
        (self.class, self.subclass, self.protocol) == (0x06, 0x01, 0x01)
    }

    /// Android Debug Bridge (ff/42/01).
    pub fn is_adb(self) -> bool {
        (self.class, self.subclass, self.protocol) == (0xFF, 0x42, 0x01)
    }
}

/// What a USB device the module cares about currently is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// File transfer (an MTP interface).
    Mtp,
    /// Debugging (an adb interface, no MTP).
    Adb,
    /// Already in accessory mode.
    Accessory,
}

impl Mode {
    /// Lower-case name for logs and diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Mtp => "mtp",
            Mode::Adb => "adb",
            Mode::Accessory => "accessory",
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The mode of a device, or `None` when it is none of our business: only accessory devices and
/// devices with an MTP or adb interface are ever listed, so vendor requests never reach anything
/// else (a keyboard, a storage stick, ...).
pub fn classify(vid: u16, pid: u16, interfaces: &[InterfaceClass]) -> Option<Mode> {
    if is_accessory(vid, pid) {
        Some(Mode::Accessory)
    } else if interfaces.iter().any(|i| i.is_mtp()) {
        Some(Mode::Mtp)
    } else if interfaces.iter().any(|i| i.is_adb()) {
        Some(Mode::Adb)
    } else {
        None
    }
}

/// One USB device from [`Aoa::list`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Vendor id.
    pub vid: u16,
    /// Product id.
    pub pid: u16,
    /// Bus number.
    pub bus: u8,
    /// Address on the bus.
    pub address: u8,
    /// Port path from the root hub.
    pub ports: Vec<u8>,
    /// Product string, when the device could be read.
    pub product: Option<String>,
    /// Serial number string, when the device has one and could be read. Never print it.
    pub serial: Option<String>,
    /// Current mode.
    pub mode: Mode,
}

impl DeviceInfo {
    /// Identity that survives a re-enumeration of the same device: the serial number when there
    /// is one, otherwise the place it is plugged in. Used to remember devices we gave up on.
    pub fn key(&self) -> String {
        match &self.serial {
            Some(s) => format!("{:04x}:{:04x}:{s}", self.vid, self.pid),
            None => {
                let ports: Vec<String> = self.ports.iter().map(|p| p.to_string()).collect();
                format!(
                    "{:04x}:{:04x}@{}-{}",
                    self.vid,
                    self.pid,
                    self.bus,
                    ports.join(".")
                )
            }
        }
    }

    /// Readable description without the serial number, for logs and diagnostics.
    pub fn describe(&self) -> String {
        let id = format!("{:04x}:{:04x}", self.vid, self.pid);
        match &self.product {
            Some(p) => format!("{id} {p}"),
            None => id,
        }
    }

    fn same_device(&self, other: &DeviceInfo) -> bool {
        (self.bus, self.address) == (other.bus, other.address)
    }
}

/// Picks the accessory that `original` turned into after the switch. `before` is the accessory
/// devices that were present before it, `now` the current device list.
///
/// A device with a serial number is matched by serial (an accessory that reports none is accepted
/// too, since some devices drop the string); without a serial, the first accessory that was not
/// there before is taken.
pub fn pick_accessory<'a>(
    original: &DeviceInfo,
    before: &[DeviceInfo],
    now: &'a [DeviceInfo],
) -> Option<&'a DeviceInfo> {
    let fresh =
        |d: &&DeviceInfo| d.mode == Mode::Accessory && !before.iter().any(|b| b.same_device(d));
    match &original.serial {
        Some(serial) => now
            .iter()
            .filter(fresh)
            .find(|d| d.serial.as_ref() == Some(serial))
            .or_else(|| now.iter().filter(fresh).find(|d| d.serial.is_none())),
        None => now.iter().find(fresh),
    }
}

/// The identification strings sent with request 52, as `(index, bytes)` with the trailing NUL.
pub fn accessory_strings() -> [(u16, Vec<u8>); 6] {
    let nul = |s: &str| {
        let mut v = s.as_bytes().to_vec();
        v.push(0);
        v
    };
    [
        (0, nul("LibreStatic")),
        (1, nul("ezdisplay")),
        (2, nul("ezdisplay extended display")),
        (3, nul(env!("CARGO_PKG_VERSION"))),
        (4, nul("https://github.com/LibreStatic/ezdisplay")),
        (5, nul("ezdisplay")),
    ]
}

/// The protocol version from the 2-byte answer to request 51; `None` when it is short or zero
/// (the device does not support accessory mode).
pub fn parse_protocol(answer: &[u8]) -> Option<u16> {
    let v = u16::from_le_bytes([*answer.first()?, *answer.get(1)?]);
    (v >= 1).then_some(v)
}

/// Length of the next bulk transfer for `remaining` bytes: at most `limit`, and never a multiple
/// of the OUT endpoint's `max_packet`. The accessory gadget completes a read early on a short
/// packet and reads a zero-length packet as end of stream, so a transfer that would end on a full
/// packet is sent one byte shorter and the rest goes in the next one.
pub fn transfer_len(remaining: usize, limit: usize, max_packet: usize) -> usize {
    let n = remaining.min(limit);
    if max_packet > 1 && n > 1 && n % max_packet == 0 {
        n - 1
    } else {
        n
    }
}

/// Bulk transfers of one claimed accessory interface; the seam that tests replace.
pub trait BulkIo: Send + Sync {
    /// One bulk IN transfer.
    fn read_bulk(&self, buf: &mut [u8], timeout: Duration) -> rusb::Result<usize>;
    /// One bulk OUT transfer.
    fn write_bulk(&self, buf: &[u8], timeout: Duration) -> rusb::Result<usize>;
}

fn io_error(e: rusb::Error) -> io::Error {
    let kind = match e {
        rusb::Error::Timeout => ErrorKind::TimedOut,
        rusb::Error::NoDevice => ErrorKind::NotConnected,
        rusb::Error::Pipe => ErrorKind::BrokenPipe,
        rusb::Error::Interrupted => ErrorKind::Interrupted,
        rusb::Error::Access => ErrorKind::PermissionDenied,
        rusb::Error::NotFound => ErrorKind::NotFound,
        _ => ErrorKind::Other,
    };
    io::Error::new(kind, format!("usb: {e}"))
}

/// `Read` over the bulk IN endpoint.
pub struct BulkReader<T: BulkIo> {
    io: Arc<T>,
}

impl<T: BulkIo> BulkReader<T> {
    /// Reads from `io`.
    pub fn new(io: Arc<T>) -> Self {
        BulkReader { io }
    }
}

impl<T: BulkIo> Read for BulkReader<T> {
    /// A timeout, and also a zero-length packet, is reported as `TimedOut`: neither is the end of
    /// the stream, the caller just reads again.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.io.read_bulk(buf, READ_TIMEOUT) {
            Ok(0) => Err(ErrorKind::TimedOut.into()),
            Ok(n) => Ok(n),
            Err(e) => Err(io_error(e)),
        }
    }
}

/// `Write` over the bulk OUT endpoint; never sends a transfer that ends on a full packet.
pub struct BulkWriter<T: BulkIo> {
    io: Arc<T>,
    max_packet: usize,
}

impl<T: BulkIo> BulkWriter<T> {
    /// Writes to `io`, whose OUT endpoint has `max_packet` as its maximum packet size.
    pub fn new(io: Arc<T>, max_packet: usize) -> Self {
        BulkWriter { io, max_packet }
    }
}

impl<T: BulkIo> Write for BulkWriter<T> {
    /// One bulk transfer of at most [`WRITE_LEN`] bytes, see [`transfer_len`]. A timeout is an
    /// error: the transfer may have been partial, so the stream cannot be trusted any more.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let n = transfer_len(buf.len(), WRITE_LEN, self.max_packet);
        self.io
            .write_bulk(&buf[..n], WRITE_TIMEOUT)
            .map_err(io_error)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A claimed accessory interface; releases it when the last user lets go.
struct Claimed {
    handle: DeviceHandle<Context>,
    iface: u8,
    ep_in: u8,
    ep_out: u8,
}

impl BulkIo for Claimed {
    fn read_bulk(&self, buf: &mut [u8], timeout: Duration) -> rusb::Result<usize> {
        self.handle.read_bulk(self.ep_in, buf, timeout)
    }

    fn write_bulk(&self, buf: &[u8], timeout: Duration) -> rusb::Result<usize> {
        self.handle.write_bulk(self.ep_out, buf, timeout)
    }
}

impl Drop for Claimed {
    fn drop(&mut self) {
        let _ = self.handle.release_interface(self.iface);
    }
}

type Strings = (Option<String>, Option<String>);

/// The USB subsystem: a libusb context plus a cache of the strings read from each device.
pub struct Aoa {
    ctx: Context,
    /// Product and serial strings per `(bus, address, vid, pid)`: reading them opens the device,
    /// which a poll every second must not do again and again.
    strings: Mutex<HashMap<(u8, u8, u16, u16), Strings>>,
}

impl Aoa {
    /// Initializes libusb. On Windows this needs UsbDk (libusb cannot reach a device that has no
    /// WinUSB driver otherwise), so a missing UsbDk makes AOA unavailable; USB then falls back to
    /// adb.
    pub fn new() -> Result<Aoa, Unavailable> {
        #[cfg(windows)]
        let ctx = Context::with_options(&[rusb::UsbOption::use_usbdk()])
            .map_err(|_| Unavailable("UsbDk is not installed".into()))?;
        #[cfg(not(windows))]
        let ctx = Context::new().map_err(|e| Unavailable(format!("libusb: {e}")))?;
        Ok(Aoa {
            ctx,
            strings: Mutex::new(HashMap::new()),
        })
    }

    /// Version of the libusb in use, like `1.0.29`.
    pub fn libusb_version() -> String {
        let v = rusb::version();
        format!("{}.{}.{}", v.major(), v.minor(), v.micro())
    }

    /// The accessory devices and the devices that may be switched, nothing else.
    pub fn list(&self) -> io::Result<Vec<DeviceInfo>> {
        let devices = self.ctx.devices().map_err(io_error)?;
        let mut out = Vec::new();
        let mut seen = Vec::new();
        for dev in devices.iter() {
            let Ok(desc) = dev.device_descriptor() else {
                continue;
            };
            let (vid, pid) = (desc.vendor_id(), desc.product_id());
            let mode = if is_accessory(vid, pid) {
                Mode::Accessory
            } else {
                match classify(vid, pid, &interfaces_of(&dev)) {
                    Some(m) => m,
                    None => continue,
                }
            };
            let key = (dev.bus_number(), dev.address(), vid, pid);
            seen.push(key);
            let (product, serial) = self.strings_of(&dev, key);
            out.push(DeviceInfo {
                vid,
                pid,
                bus: key.0,
                address: key.1,
                ports: dev.port_numbers().unwrap_or_default(),
                product,
                serial,
                mode,
            });
        }
        lock(&self.strings).retain(|k, _| seen.contains(k));
        Ok(out)
    }

    fn strings_of(&self, dev: &Device<Context>, key: (u8, u8, u16, u16)) -> Strings {
        if let Some(s) = lock(&self.strings).get(&key) {
            return s.clone();
        }
        let strings = read_strings(dev);
        lock(&self.strings).insert(key, strings.clone());
        strings
    }

    fn find(&self, info: &DeviceInfo) -> io::Result<Device<Context>> {
        let devices = self.ctx.devices().map_err(io_error)?;
        devices
            .iter()
            .find(|d| {
                (d.bus_number(), d.address()) == (info.bus, info.address)
                    && d.device_descriptor()
                        .is_ok_and(|x| (x.vendor_id(), x.product_id()) == (info.vid, info.pid))
            })
            .ok_or_else(|| io::Error::new(ErrorKind::NotFound, "usb: the device is gone"))
    }

    /// Switches `dev` (a device from [`list`](Self::list) that is not an accessory) to accessory
    /// mode and waits for it to come back as one, which is returned.
    ///
    /// Sends vendor requests to the device, so only call it for devices that [`list`](Self::list)
    /// returned.
    pub fn switch(&self, dev: &DeviceInfo) -> io::Result<DeviceInfo> {
        if dev.mode == Mode::Accessory {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "the device is already an accessory",
            ));
        }
        let before: Vec<DeviceInfo> = self
            .list()?
            .into_iter()
            .filter(|d| d.mode == Mode::Accessory)
            .collect();
        {
            let handle = self.find(dev)?.open().map_err(io_error)?;
            handshake(&handle)?;
        }
        let deadline = Instant::now() + REENUMERATE_TIMEOUT;
        loop {
            std::thread::sleep(Duration::from_millis(200));
            let now = self.list()?;
            if let Some(a) = pick_accessory(dev, &before, &now) {
                return Ok(a.clone());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    ErrorKind::TimedOut,
                    "the device did not come back as an accessory",
                ));
            }
        }
    }

    /// Claims the accessory interface of `dev` and returns a link over its bulk endpoints.
    /// Dropping the link ends its threads and releases the interface.
    ///
    /// Must be called inside a tokio runtime.
    pub fn open(&self, dev: &DeviceInfo) -> io::Result<Link> {
        let device = self.find(dev)?;
        let (ep_in, ep_out, max_packet) = bulk_endpoints(&device)?;
        let handle = device.open().map_err(io_error)?;
        // Not supported everywhere (Windows); the kernel driver is only an issue on Linux.
        let _ = handle.set_auto_detach_kernel_driver(true);
        handle
            .claim_interface(ACCESSORY_INTERFACE)
            .map_err(io_error)?;
        let io = Arc::new(Claimed {
            handle,
            iface: ACCESSORY_INTERFACE,
            ep_in,
            ep_out,
        });
        let peer = format!(
            "usb:{:04x}:{:04x} {}",
            dev.vid,
            dev.pid,
            dev.product.as_deref().unwrap_or("accessory")
        );
        Link::from_blocking(
            BulkReader::new(io.clone()),
            BulkWriter::new(io, max_packet),
            peer,
            BlockingOptions {
                read_buf: TRANSFER_LEN,
                write_buf: WRITE_LEN,
                thread_name: "ezd-aoa".into(),
                ..BlockingOptions::default()
            },
        )
    }

    /// Resets `dev`. A device that switched to accessory mode but never answered goes back to its
    /// normal USB mode this way (a phone gets its file transfer back).
    pub fn reset(&self, dev: &DeviceInfo) -> io::Result<()> {
        let handle = self.find(dev)?.open().map_err(io_error)?;
        handle.reset().map_err(io_error)
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn interfaces_of(dev: &Device<Context>) -> Vec<InterfaceClass> {
    let Ok(cfg) = dev.active_config_descriptor() else {
        return Vec::new();
    };
    cfg.interfaces()
        .flat_map(|i| i.descriptors().collect::<Vec<_>>())
        .map(|d| InterfaceClass {
            class: d.class_code(),
            subclass: d.sub_class_code(),
            protocol: d.protocol_code(),
        })
        .collect()
}

fn read_strings(dev: &Device<Context>) -> Strings {
    let (Ok(desc), Ok(handle)) = (dev.device_descriptor(), dev.open()) else {
        return (None, None);
    };
    let product = handle.read_product_string_ascii(&desc).ok();
    let serial = handle
        .read_serial_number_string_ascii(&desc)
        .ok()
        .filter(|s| !s.is_empty());
    (product, serial)
}

/// The bulk IN and OUT endpoint addresses of the accessory interface and the OUT endpoint's
/// maximum packet size.
fn bulk_endpoints(dev: &Device<Context>) -> io::Result<(u8, u8, usize)> {
    let cfg = dev.active_config_descriptor().map_err(io_error)?;
    let mut ep_in = None;
    let mut ep_out = None;
    for iface in cfg
        .interfaces()
        .filter(|i| i.number() == ACCESSORY_INTERFACE)
    {
        for d in iface.descriptors() {
            for ep in d.endpoint_descriptors() {
                if ep.transfer_type() != rusb::TransferType::Bulk {
                    continue;
                }
                match ep.direction() {
                    Direction::In => ep_in = ep_in.or(Some(ep.address())),
                    Direction::Out => {
                        ep_out = ep_out.or(Some((ep.address(), usize::from(ep.max_packet_size()))));
                    }
                }
            }
        }
    }
    match (ep_in, ep_out) {
        (Some(i), Some((o, mps))) => Ok((i, o, mps)),
        _ => Err(io::Error::new(
            ErrorKind::NotFound,
            "usb: the accessory interface has no bulk endpoints",
        )),
    }
}

/// GET_PROTOCOL, the identification strings and START.
fn handshake(handle: &DeviceHandle<Context>) -> io::Result<()> {
    let vendor_in = rusb::request_type(Direction::In, RequestType::Vendor, Recipient::Device);
    let vendor_out = rusb::request_type(Direction::Out, RequestType::Vendor, Recipient::Device);
    let mut answer = [0u8; 2];
    let n = handle
        .read_control(
            vendor_in,
            REQ_GET_PROTOCOL,
            0,
            0,
            &mut answer,
            CONTROL_TIMEOUT,
        )
        .map_err(io_error)?;
    let protocol = parse_protocol(&answer[..n]).ok_or_else(|| {
        io::Error::new(
            ErrorKind::Unsupported,
            "the device does not support accessory mode",
        )
    })?;
    tracing::debug!(protocol, "accessory protocol");
    for (index, bytes) in accessory_strings() {
        handle
            .write_control(
                vendor_out,
                REQ_SEND_STRING,
                0,
                index,
                &bytes,
                CONTROL_TIMEOUT,
            )
            .map_err(io_error)?;
    }
    match handle.write_control(vendor_out, REQ_START, 0, 0, &[], CONTROL_TIMEOUT) {
        // The device may drop off the bus before it completes the request.
        Ok(_) | Err(rusb::Error::NoDevice | rusb::Error::Io) => Ok(()),
        Err(e) => Err(io_error(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn ic(class: u8, subclass: u8, protocol: u8) -> InterfaceClass {
        InterfaceClass {
            class,
            subclass,
            protocol,
        }
    }

    const MTP: InterfaceClass = InterfaceClass {
        class: 6,
        subclass: 1,
        protocol: 1,
    };
    const ADB: InterfaceClass = InterfaceClass {
        class: 0xff,
        subclass: 0x42,
        protocol: 1,
    };

    fn dev(pid: u16, mode: Mode, addr: u8, serial: Option<&str>, ports: &[u8]) -> DeviceInfo {
        DeviceInfo {
            vid: if mode == Mode::Accessory {
                ACCESSORY_VID
            } else {
                0x04e8
            },
            pid,
            bus: 1,
            address: addr,
            ports: ports.to_vec(),
            product: Some("Tab".into()),
            serial: serial.map(String::from),
            mode,
        }
    }

    #[test]
    fn accessory_ids() {
        for pid in 0x2D00..=0x2D05 {
            assert!(is_accessory(0x18D1, pid));
        }
        assert!(!is_accessory(0x18D1, 0x2D06));
        assert!(!is_accessory(0x18D1, 0x4EE1));
        assert!(!is_accessory(0x04e8, 0x2D00));
        let adb: Vec<u16> = (0x2D00..=0x2D05)
            .filter(|p| accessory_has_adb(*p))
            .collect();
        assert_eq!(adb, [0x2D01, 0x2D03, 0x2D05]);
    }

    #[test]
    fn only_mtp_adb_and_accessory_devices_are_candidates() {
        assert_eq!(classify(0x04e8, 0x6860, &[MTP]), Some(Mode::Mtp));
        assert_eq!(classify(0x04e8, 0x6860, &[MTP, ADB]), Some(Mode::Mtp));
        assert_eq!(classify(0x18D1, 0x4EE2, &[ADB]), Some(Mode::Adb));
        assert_eq!(classify(0x18D1, 0x2D01, &[]), Some(Mode::Accessory));
        // Same class and subclass, other protocol; a keyboard; a mass storage stick; no interfaces.
        assert_eq!(classify(1, 2, &[ic(6, 1, 2)]), None);
        assert_eq!(classify(1, 2, &[ic(0xff, 0x42, 2)]), None);
        assert_eq!(classify(1, 2, &[ic(3, 1, 1)]), None);
        assert_eq!(classify(1, 2, &[ic(8, 6, 0x50)]), None);
        assert_eq!(classify(1, 2, &[]), None);
    }

    #[test]
    fn the_key_prefers_the_serial_and_falls_back_to_the_port() {
        let with = dev(0x6860, Mode::Mtp, 4, Some("R52"), &[2, 1]);
        assert_eq!(with.key(), "04e8:6860:R52");
        let without = dev(0x6860, Mode::Mtp, 4, None, &[2, 1]);
        assert_eq!(without.key(), "04e8:6860@1-2.1");
        // Same device after a re-enumeration at another address.
        let moved = DeviceInfo {
            address: 9,
            ..without.clone()
        };
        assert_eq!(moved.key(), without.key());
        assert!(!with.describe().contains("R52"));
        assert_eq!(with.describe(), "04e8:6860 Tab");
    }

    #[test]
    fn the_accessory_is_matched_by_serial() {
        let original = dev(0x6860, Mode::Mtp, 4, Some("A"), &[1]);
        let before = vec![dev(0x2D00, Mode::Accessory, 2, Some("OLD"), &[3])];
        let now = vec![
            before[0].clone(),
            dev(0x2D00, Mode::Accessory, 7, Some("B"), &[4]),
            dev(0x2D00, Mode::Accessory, 8, Some("A"), &[1]),
        ];
        assert_eq!(pick_accessory(&original, &before, &now), Some(&now[2]));
        // Not there yet.
        assert_eq!(pick_accessory(&original, &before, &now[..2]), None);
    }

    #[test]
    fn an_accessory_without_a_serial_is_accepted_for_a_device_with_one() {
        let original = dev(0x6860, Mode::Mtp, 4, Some("A"), &[1]);
        let now = vec![dev(0x2D00, Mode::Accessory, 8, None, &[1])];
        assert_eq!(pick_accessory(&original, &[], &now), Some(&now[0]));
    }

    #[test]
    fn without_a_serial_the_first_new_accessory_is_taken() {
        let original = dev(0x6860, Mode::Mtp, 4, None, &[1]);
        let before = vec![dev(0x2D00, Mode::Accessory, 2, None, &[3])];
        let now = vec![
            before[0].clone(),
            dev(0x04e8, Mode::Mtp, 5, None, &[5]),
            dev(0x2D01, Mode::Accessory, 8, None, &[1]),
        ];
        assert_eq!(pick_accessory(&original, &before, &now), Some(&now[2]));
        assert_eq!(pick_accessory(&original, &before, &now[..2]), None);
    }

    #[test]
    fn the_handshake_strings_are_nul_terminated_and_complete() {
        let s = accessory_strings();
        assert_eq!(
            s.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4, 5]
        );
        assert!(s.iter().all(|(_, b)| b.last() == Some(&0)));
        assert_eq!(s[0].1, b"LibreStatic\0");
        assert_eq!(s[1].1, b"ezdisplay\0");
        assert_eq!(s[2].1, b"ezdisplay extended display\0");
        assert_eq!(
            s[3].1,
            format!("{}\0", env!("CARGO_PKG_VERSION")).as_bytes()
        );
        assert_eq!(s[4].1, b"https://github.com/LibreStatic/ezdisplay\0");
        assert_eq!(s[5].1, b"ezdisplay\0");
    }

    #[test]
    fn the_protocol_answer_is_little_endian_and_at_least_one() {
        assert_eq!(parse_protocol(&[2, 0]), Some(2));
        assert_eq!(parse_protocol(&[0, 1]), Some(256));
        assert_eq!(parse_protocol(&[0, 0]), None);
        assert_eq!(parse_protocol(&[1]), None);
        assert_eq!(parse_protocol(&[]), None);
    }

    #[test]
    fn transfers_never_end_on_a_full_packet() {
        assert_eq!(transfer_len(100, 16384, 512), 100);
        assert_eq!(transfer_len(512, 16384, 512), 511);
        assert_eq!(transfer_len(1024, 16384, 512), 1023);
        assert_eq!(transfer_len(16384, 16384, 512), 16383);
        assert_eq!(transfer_len(100_000, 16384, 512), 16383);
        assert_eq!(transfer_len(100_000, 16384, 64), 16383);
        assert_eq!(transfer_len(1, 16384, 512), 1);
        assert_eq!(transfer_len(513, 16384, 512), 513);
        // A degenerate packet size cannot be avoided.
        assert_eq!(transfer_len(10, 16384, 1), 10);
    }

    struct FakeIo {
        reads: Mutex<VecDeque<rusb::Result<Vec<u8>>>>,
        writes: Mutex<Vec<usize>>,
        write_result: Mutex<Option<rusb::Error>>,
    }

    impl FakeIo {
        fn new(reads: Vec<rusb::Result<Vec<u8>>>) -> Arc<FakeIo> {
            Arc::new(FakeIo {
                reads: Mutex::new(reads.into()),
                writes: Mutex::new(Vec::new()),
                write_result: Mutex::new(None),
            })
        }
    }

    impl BulkIo for FakeIo {
        fn read_bulk(&self, buf: &mut [u8], _: Duration) -> rusb::Result<usize> {
            match self.reads.lock().unwrap().pop_front() {
                Some(Ok(d)) => {
                    buf[..d.len()].copy_from_slice(&d);
                    Ok(d.len())
                }
                Some(Err(e)) => Err(e),
                None => Err(rusb::Error::Timeout),
            }
        }

        fn write_bulk(&self, buf: &[u8], _: Duration) -> rusb::Result<usize> {
            if let Some(e) = *self.write_result.lock().unwrap() {
                return Err(e);
            }
            self.writes.lock().unwrap().push(buf.len());
            Ok(buf.len())
        }
    }

    #[test]
    fn a_read_maps_timeouts_and_zero_length_packets_to_timed_out() {
        let io = FakeIo::new(vec![
            Ok(vec![1, 2, 3]),
            Ok(vec![]),
            Err(rusb::Error::Timeout),
            Err(rusb::Error::NoDevice),
            Err(rusb::Error::Pipe),
            Err(rusb::Error::Io),
        ]);
        let mut r = BulkReader::new(io);
        let mut buf = [0u8; 16];
        assert_eq!(r.read(&mut buf).unwrap(), 3);
        assert_eq!(&buf[..3], [1, 2, 3]);
        assert_eq!(r.read(&mut buf).unwrap_err().kind(), ErrorKind::TimedOut);
        assert_eq!(r.read(&mut buf).unwrap_err().kind(), ErrorKind::TimedOut);
        assert_eq!(
            r.read(&mut buf).unwrap_err().kind(),
            ErrorKind::NotConnected
        );
        assert_eq!(r.read(&mut buf).unwrap_err().kind(), ErrorKind::BrokenPipe);
        assert_eq!(r.read(&mut buf).unwrap_err().kind(), ErrorKind::Other);
    }

    #[test]
    fn a_write_sends_one_transfer_and_avoids_full_packets() {
        let io = FakeIo::new(vec![]);
        let mut w = BulkWriter::new(io.clone(), 512);
        let data = vec![7u8; 40_000];
        w.write_all(&data).unwrap();
        let sent = io.writes.lock().unwrap().clone();
        assert_eq!(sent.iter().sum::<usize>(), 40_000);
        assert!(
            sent.iter().all(|n| *n <= WRITE_LEN && n % 512 != 0),
            "{sent:?}"
        );
        assert_eq!(sent, [40_000]);

        io.writes.lock().unwrap().clear();
        assert_eq!(w.write(&[1u8; 512]).unwrap(), 511);
        assert_eq!(w.write(&[]).unwrap(), 0);
        assert_eq!(*io.writes.lock().unwrap(), [511]);
    }

    #[test]
    fn full_write_chunks_are_never_split() {
        for max_packet in [64, 512, 1024] {
            assert_eq!(transfer_len(WRITE_LEN, WRITE_LEN, max_packet), WRITE_LEN);
            assert_eq!(transfer_len(1 << 20, WRITE_LEN, max_packet), WRITE_LEN);
        }
        // A large frame handed over in full chunks, as the link's writer thread does.
        let io = FakeIo::new(vec![]);
        let mut w = BulkWriter::new(io.clone(), 1024);
        for _ in 0..3 {
            w.write_all(&vec![7u8; WRITE_LEN]).unwrap();
        }
        w.write_all(&[7u8; 4096]).unwrap();
        let sent = io.writes.lock().unwrap().clone();
        assert_eq!(sent, [WRITE_LEN, WRITE_LEN, WRITE_LEN, 4095, 1]);
    }

    #[test]
    fn a_write_timeout_is_a_hard_error() {
        let io = FakeIo::new(vec![]);
        *io.write_result.lock().unwrap() = Some(rusb::Error::Timeout);
        let mut w = BulkWriter::new(io, 512);
        let e = w.write_all(&[0u8; 10]).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::TimedOut);
    }

    #[tokio::test]
    async fn a_link_over_the_fake_endpoints_moves_bytes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let io = FakeIo::new(vec![Ok(b"hello".to_vec())]);
        let mut link = Link::from_blocking(
            BulkReader::new(io.clone()),
            BulkWriter::new(io.clone(), 512),
            "usb:test",
            BlockingOptions::default(),
        )
        .unwrap();
        let mut got = [0u8; 5];
        link.reader.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"hello");
        link.writer.write_all(&[9u8; 512]).await.unwrap();
        link.writer.flush().await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while io.writes.lock().unwrap().iter().sum::<usize>() < 512 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(io.writes.lock().unwrap().iter().all(|n| n % 512 != 0));
    }
}

//! Env-gated tests against a real X server (`tools/x11-dummy-test.sh` starts one with the dummy
//! video driver, whose xf86 modes reproduce the pointer constraint that Xvfb lacks).

use crate::frame::{Pixels, spa_fmt};
use crate::portal::SourceKind;
use crate::provider::{FrameSource, OutputRequest, VirtualOutputProvider};
use crate::uinput_sink::DesktopInput;
use crate::x11_capture::X11Capture;
use crate::x11_input::{
    X11Desktop, get_matrix, init_xinput, matrix_atoms, set_matrix, transform_matrix,
};
use crate::x11_output::{MONITOR_NAME, Rect, X11Provider, atom, connect, monitors, snapshot};
use ezd_host_core::CursorSink;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::randr;
use x11rb::protocol::xinput;
use x11rb::protocol::xproto::{self, ConnectionExt as _, CreateWindowAux, WindowClass};
use x11rb::protocol::xtest;
use x11rb::rust_connection::RustConnection;

/// True when the test runs against the X server of the script.
fn enabled() -> bool {
    if std::env::var_os("EZD_X11_TEST").is_none_or(|v| v != "1") {
        eprintln!("skipped: set EZD_X11_TEST=1 and DISPLAY (tools/x11-dummy-test.sh does)");
        return false;
    }
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    true
}

fn sync(conn: &RustConnection) {
    conn.get_input_focus().unwrap().reply().unwrap();
}

/// Maps an override-redirect window of `rect` filled with `pixel`.
fn paint(conn: &RustConnection, root: u32, rect: Rect, pixel: u32) -> u32 {
    let win = conn.generate_id().unwrap();
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        win,
        root,
        rect.x as i16,
        rect.y as i16,
        rect.w as u16,
        rect.h as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new()
            .background_pixel(pixel)
            .override_redirect(1),
    )
    .unwrap();
    conn.map_window(win).unwrap();
    sync(conn);
    win
}

fn repaint(conn: &RustConnection, win: u32, pixel: u32) {
    conn.change_window_attributes(
        win,
        &xproto::ChangeWindowAttributesAux::new().background_pixel(pixel),
    )
    .unwrap();
    conn.clear_area(false, win, 0, 0, 0, 0).unwrap();
    sync(conn);
}

/// Waits for a frame whose pixel at (`x`, `y`) is `pixel` (`0xRRGGBB`); returns the frame.
fn frame_with_color(cap: &X11Capture, x: usize, y: usize, pixel: u32) -> crate::frame::Frame {
    let want = [pixel as u8, (pixel >> 8) as u8, (pixel >> 16) as u8];
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = None;
    while Instant::now() < deadline {
        if let Some(f) = cap.next(Duration::from_millis(200)).unwrap() {
            let Pixels::Shm(s) = &f.pixels else {
                panic!("not a shared-memory frame")
            };
            let at = y * s.stride + x * 4;
            last = Some(s.data[at..at + 3].to_vec());
            if s.data[at..at + 3] == want {
                return f;
            }
        }
    }
    panic!("no frame with colour {pixel:#08x} at ({x},{y}); last pixel {last:?}");
}

fn warp(conn: &RustConnection, root: u32, x: i16, y: i16) -> (i16, i16) {
    xtest::fake_input(conn, xproto::MOTION_NOTIFY_EVENT, 0, 0, root, x, y, 0).unwrap();
    sync(conn);
    let p = conn.query_pointer(root).unwrap().reply().unwrap();
    (p.root_x, p.root_y)
}

fn panning_of(conn: &RustConnection, root: u32) -> Vec<(u32, u16)> {
    let snap = snapshot(conn, root).unwrap();
    snap.crtcs
        .iter()
        .map(|c| {
            let p = randr::get_panning(conn, c.id).unwrap().reply().unwrap();
            (c.id, p.width)
        })
        .collect()
}

fn request(width: u32, height: u32) -> OutputRequest {
    OutputRequest {
        kind: SourceKind::Virtual,
        output_name: None,
        width,
        height,
        refresh_mhz: 60_000,
    }
}

#[test]
fn x11_virtual_monitor_capture_input_and_restore() {
    if !enabled() {
        return;
    }
    let (conn, n) = connect().unwrap();
    let root = conn.setup().roots[n].root;
    let before = snapshot(&conn, root).unwrap();
    let panning_before = panning_of(&conn, root);
    let monitors_before = monitors(&conn, root).unwrap().len();

    let opened = X11Provider.open(&request(1280, 720)).unwrap();
    let FrameSource::X11 { rect } = opened.frames else {
        panic!("not an X11 frame source")
    };
    let monitor = Rect {
        x: rect.0,
        y: rect.1,
        w: rect.2,
        h: rect.3,
    };
    // To the right of everything that was there, same height as asked.
    let right = before.crtcs.iter().map(|c| c.rect.right()).max().unwrap();
    assert_eq!(
        monitor,
        Rect {
            x: right,
            y: 0,
            w: 1280,
            h: 720
        }
    );
    let info = opened.info.as_ref().unwrap();
    assert_eq!(info.name, MONITOR_NAME);
    assert_eq!((info.mode.width, info.mode.height), (1280, 720));
    assert_eq!(info.origin, (monitor.x, 0));

    let after = snapshot(&conn, root).unwrap();
    assert_eq!(after.screen.0, right as u32 + 1280);
    assert!(after.screen.1 >= 720.max(before.screen.1));
    let listed = monitors(&conn, root).unwrap();
    assert_eq!(listed.len(), monitors_before + 1);
    let ours = listed.iter().find(|m| m.name == MONITOR_NAME).unwrap();
    assert_eq!(ours.rect, monitor);

    // The pointer reaches the new area (the server would clamp it to the CRTCs otherwise).
    let (tx, ty) = (monitor.x + 1000, 500);
    assert_eq!(
        warp(&conn, root, tx as i16, ty as i16),
        (tx as i16, ty as i16)
    );

    // Paint the new area and capture it.
    let win = paint(&conn, root, monitor, 0x00ff_8040);
    let mut cap = X11Capture::start(rect, 60_000, CursorSink::null()).unwrap();
    assert!(cap.uses_shm(), "MIT-SHM capture should be available");
    let fmt = cap
        .wait_format((1280, 720), Duration::from_secs(2))
        .expect("format");
    assert_eq!(
        (fmt.width, fmt.height, fmt.spa_format),
        (1280, 720, spa_fmt::BGRX)
    );
    let frame = frame_with_color(&cap, 10, 10, 0x00ff_8040);
    assert_eq!((frame.width, frame.height), (1280, 720));
    let Pixels::Shm(s) = &frame.pixels else {
        unreachable!()
    };
    assert_eq!(s.stride, 1280 * 4);
    assert!(s.data.len() >= s.stride * 719 + 1280 * 4);
    // Damage paces the capture: a change of colour produces a new frame.
    repaint(&conn, win, 0x0010_a0e0);
    frame_with_color(&cap, 1270, 710, 0x0010_a0e0);
    // The pointer is inside the area: the cursor tracker saw it.
    warp(&conn, root, (monitor.x + 100) as i16, 100);
    let deadline = Instant::now() + Duration::from_secs(2);
    while cap.cursor_updates() == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(cap.cursor_updates() > 0, "no cursor update");
    let (produced, _) = cap.counters();
    assert!(produced >= 2, "produced {produced}");
    cap.stop();

    // A desktop daemon deleting the monitor is undone by the watcher.
    let name = atom(&conn, MONITOR_NAME).unwrap();
    randr::delete_monitor(&conn, root, name)
        .unwrap()
        .check()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        if monitors(&conn, root)
            .unwrap()
            .iter()
            .any(|m| m.name == MONITOR_NAME)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the monitor was not applied again"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    // The same for what a daemon that resets the layout does: no monitor, no panning, the
    // original screen size.
    randr::delete_monitor(&conn, root, name)
        .unwrap()
        .check()
        .unwrap();
    for c in &before.crtcs {
        randr::set_panning(&conn, c.id, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
            .unwrap()
            .reply()
            .unwrap();
    }
    let (w, h) = before.screen;
    randr::set_screen_size(&conn, root, w as u16, h as u16, 508, 286)
        .unwrap()
        .check()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let now = snapshot(&conn, root).unwrap();
        let present = monitors(&conn, root)
            .unwrap()
            .iter()
            .any(|m| m.name == MONITOR_NAME);
        let panning = panning_of(&conn, root).iter().any(|(_, w)| *w > 0);
        if present && panning && now.screen.0 == right as u32 + 1280 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the layout was not applied again"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    conn.destroy_window(win).unwrap();
    sync(&conn);

    drop(opened.guard);
    let end = snapshot(&conn, root).unwrap();
    assert_eq!(end.screen, before.screen, "screen size restored");
    let listed = monitors(&conn, root).unwrap();
    assert!(
        listed.iter().all(|m| m.name != MONITOR_NAME),
        "monitor removed"
    );
    assert_eq!(listed.len(), monitors_before);
    assert_eq!(panning_of(&conn, root), panning_before, "panning restored");
}

#[test]
fn x11_capture_without_shared_memory() {
    if !enabled() {
        return;
    }
    let (conn, n) = connect().unwrap();
    let root = conn.setup().roots[n].root;
    let rect = Rect {
        x: 0,
        y: 0,
        w: 321,
        h: 201,
    };
    let win = paint(&conn, root, rect, 0x0020_4060);
    // An odd size, through plain GetImage.
    let cap = X11Capture::start_with((0, 0, 321, 201), 30_000, CursorSink::null(), false).unwrap();
    assert!(!cap.uses_shm());
    let frame = frame_with_color(&cap, 5, 5, 0x0020_4060);
    assert_eq!((frame.width, frame.height), (321, 201));
    conn.destroy_window(win).unwrap();
    sync(&conn);
}

#[test]
fn x11_matrix_property_round_trip() {
    if !enabled() {
        return;
    }
    let (conn, _) = connect().unwrap();
    init_xinput(&conn).unwrap();
    let (property, float) = matrix_atoms(&conn).unwrap();
    // Any slave pointer that carries the property: the XTEST pointer does.
    let all = xinput::xi_query_device(&conn, 0u16)
        .unwrap()
        .reply()
        .unwrap();
    let dev = all
        .infos
        .iter()
        .find(|d| String::from_utf8_lossy(&d.name).contains("XTEST pointer"))
        .expect("XTEST pointer");
    let m = transform_matrix(
        Rect {
            x: 1920,
            y: 0,
            w: 1920,
            h: 1080,
        },
        (3840, 1080),
    );
    set_matrix(&conn, dev.deviceid, m, property, float).unwrap();
    assert_eq!(get_matrix(&conn, dev.deviceid, property).unwrap(), Some(m));
    let identity = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    set_matrix(&conn, dev.deviceid, identity, property, float).unwrap();
    assert_eq!(
        get_matrix(&conn, dev.deviceid, property).unwrap(),
        Some(identity)
    );
}

/// With `/dev/uinput` access and a server that hot-plugs input devices: a uinput touchscreen gets
/// the matrix of the monitor. Skips (with a message) when either is missing.
#[test]
fn x11_maps_a_uinput_touchscreen() {
    if !enabled() {
        return;
    }
    let mut touch = match crate::uinput_sink::create_touch((268.5, 167.6)) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("skipped: no /dev/uinput access ({e})");
            return;
        }
    };
    let event = crate::uinput_sink::event_name(&mut touch).expect("event node");
    let opened = X11Provider.open(&request(1280, 720)).unwrap();
    let desktop = X11Desktop;
    let mapped = desktop.map_devices(&[("touch", event.as_str())], MONITOR_NAME);
    match &mapped.results[0] {
        Err(e) if e.contains("did not add") => {
            eprintln!("skipped: the X server does not hot-plug input devices ({e})");
            return;
        }
        r => r.clone().unwrap(),
    }
    let (conn, n) = connect().unwrap();
    init_xinput(&conn).unwrap();
    let root = conn.setup().roots[n].root;
    let snap = snapshot(&conn, root).unwrap();
    let mon = snap
        .monitors
        .iter()
        .find(|m| m.name == MONITOR_NAME)
        .unwrap();
    let (property, _) = matrix_atoms(&conn).unwrap();
    let node = atom(&conn, "Device Node").unwrap();
    let all = xinput::xi_query_device(&conn, 0u16)
        .unwrap()
        .reply()
        .unwrap();
    let id = all
        .infos
        .iter()
        .find(|d| {
            matches!(
                xinput::xi_get_property(&conn, d.deviceid, false, node, 0u32, 0, 64)
                    .unwrap()
                    .reply()
                    .unwrap()
                    .items,
                xinput::XIGetPropertyItems::Data8(ref v)
                    if String::from_utf8_lossy(v).trim_end_matches('\0') == format!("/dev/input/{event}")
            )
        })
        .map(|d| d.deviceid)
        .expect("device");
    let want = transform_matrix(mon.rect, snap.screen);
    assert_eq!(get_matrix(&conn, id, property).unwrap(), Some(want));
    drop(mapped.guard);
    assert_eq!(
        get_matrix(&conn, id, property).unwrap(),
        Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0])
    );
    // The outputs the pointer mapper sees: monitors plus the whole screen.
    let outs = desktop.outputs().unwrap();
    assert!(
        outs.iter()
            .any(|o| o.name == MONITOR_NAME && o.size == (1280, 720))
    );
    drop(opened.guard);
}

/// The matrix follows a change of the screen size (the output watcher re-applying, a monitor
/// plugged in). Uses the XTEST pointer, which every server has.
#[test]
fn x11_matrix_is_recomputed_when_the_screen_changes() {
    if !enabled() {
        return;
    }
    let opened = X11Provider.open(&request(1280, 720)).unwrap();
    let (conn, n) = connect().unwrap();
    init_xinput(&conn).unwrap();
    let root = conn.setup().roots[n].root;
    let (property, _) = matrix_atoms(&conn).unwrap();
    let all = xinput::xi_query_device(&conn, 0u16)
        .unwrap()
        .reply()
        .unwrap();
    let dev = all
        .infos
        .iter()
        .find(|d| String::from_utf8_lossy(&d.name).contains("XTEST pointer"))
        .expect("XTEST pointer")
        .deviceid;
    let guard = crate::x11_input::MapGuard::for_devices(MONITOR_NAME, vec![dev]).unwrap();
    // Let the watcher subscribe, so the change below arrives as an event.
    std::thread::sleep(Duration::from_secs(1));

    let snap = snapshot(&conn, root).unwrap();
    let mon = snap
        .monitors
        .iter()
        .find(|m| m.name == MONITOR_NAME)
        .unwrap()
        .rect;
    let (w, h) = snap.screen;
    randr::set_screen_size(&conn, root, (w + 200) as u16, h as u16, 1000, 300)
        .unwrap()
        .check()
        .unwrap();
    let want = transform_matrix(mon, (w + 200, h));
    let deadline = Instant::now() + Duration::from_secs(5);
    while get_matrix(&conn, dev, property).unwrap() != Some(want) {
        assert!(Instant::now() < deadline, "the matrix was not recomputed");
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(guard);
    assert_eq!(
        get_matrix(&conn, dev, property).unwrap(),
        Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0])
    );
    drop(opened.guard);
}

//! Helpers for the env-gated headless sway tests (`tools/wlroots-headless-test.sh`): a layer surface
//! that keeps the new output changing (the capture protocols only deliver frames on damage) and a
//! virtual pointer that moves across it (so the cursor session has something to report).

use std::os::fd::{AsFd, AsRawFd, FromRawFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, ZwlrLayerSurfaceV1},
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

/// True when the test runs inside the headless compositor of the script.
pub fn enabled() -> bool {
    if std::env::var_os("EZD_WLROOTS_TEST").is_none_or(|v| v != "1") {
        eprintln!("skipped: set EZD_WLROOTS_TEST=1 inside a headless sway session");
        return false;
    }
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    true
}

struct Pointer;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Pointer {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
delegate_noop!(Pointer: ignore wl_seat::WlSeat);
delegate_noop!(Pointer: ZwlrVirtualPointerManagerV1);
delegate_noop!(Pointer: ZwlrVirtualPointerV1);

struct Painter {
    outputs: Vec<(wl_output::WlOutput, Option<String>)>,
    size: Option<(u32, u32)>,
    closed: bool,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Painter {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, usize> for Painter {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            state.outputs[*index].1 = Some(name);
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for Painter {
    fn event(
        state: &mut Self,
        surface: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                surface.ack_configure(serial);
                state.size = Some((width, height));
            }
            zwlr_layer_surface_v1::Event::Closed => state.closed = true,
            _ => {}
        }
    }
}

delegate_noop!(Painter: ignore wl_surface::WlSurface);
delegate_noop!(Painter: wl_compositor::WlCompositor);
delegate_noop!(Painter: ignore wl_shm::WlShm);
delegate_noop!(Painter: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Painter: ignore wl_buffer::WlBuffer);
delegate_noop!(Painter: ZwlrLayerShellV1);

/// Covers `output` with a layer surface that alternates between red and blue, until `stop`.
fn paint(output: &str, stop: &AtomicBool) -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let (globals, mut queue) = registry_queue_init::<Painter>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let mut state = Painter {
        outputs: Vec::new(),
        size: None,
        closed: false,
    };
    for (i, g) in globals
        .contents()
        .clone_list()
        .iter()
        .filter(|g| g.interface == "wl_output")
        .enumerate()
    {
        let out =
            globals
                .registry()
                .bind::<wl_output::WlOutput, _, _>(g.name, g.version.min(4), &qh, i);
        state.outputs.push((out, None));
    }
    let compositor = globals
        .bind::<wl_compositor::WlCompositor, _, _>(&qh, 1..=4, ())
        .map_err(|e| e.to_string())?;
    let shm = globals
        .bind::<wl_shm::WlShm, _, _>(&qh, 1..=1, ())
        .map_err(|e| e.to_string())?;
    let shell = globals
        .bind::<ZwlrLayerShellV1, _, _>(&qh, 1..=1, ())
        .map_err(|e| e.to_string())?;
    queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
    let target = state
        .outputs
        .iter()
        .find(|(_, n)| n.as_deref() == Some(output))
        .map(|(o, _)| o.clone())
        .ok_or_else(|| format!("no output {output}"))?;
    let surface = compositor.create_surface(&qh, ());
    let layer = shell.get_layer_surface(
        &surface,
        Some(&target),
        zwlr_layer_shell_v1::Layer::Top,
        "ezd-test".into(),
        &qh,
        (),
    );
    layer.set_anchor(
        zwlr_layer_surface_v1::Anchor::Top
            | zwlr_layer_surface_v1::Anchor::Bottom
            | zwlr_layer_surface_v1::Anchor::Left
            | zwlr_layer_surface_v1::Anchor::Right,
    );
    layer.set_exclusive_zone(-1);
    surface.commit();
    while state.size.is_none() && !state.closed {
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
    }
    let (w, h) = state
        .size
        .filter(|&(w, h)| w > 0 && h > 0)
        .ok_or("no size")?;
    let len = (w * h * 4) as usize;
    // SAFETY: a private memfd, mapped for the lifetime of the loop below.
    let (fd, ptr) = unsafe {
        let raw = libc::memfd_create(c"ezd-test-paint".as_ptr(), libc::MFD_CLOEXEC);
        if raw < 0 {
            return Err("memfd_create failed".into());
        }
        let fd = std::os::fd::OwnedFd::from_raw_fd(raw);
        if libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) != 0 {
            return Err("ftruncate failed".into());
        }
        let p = libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        );
        if p == libc::MAP_FAILED {
            return Err("mmap failed".into());
        }
        (fd, p as *mut u32)
    };
    let pool = shm.create_pool(fd.as_fd(), len as i32, &qh, ());
    let buffer = pool.create_buffer(
        0,
        w as i32,
        h as i32,
        (w * 4) as i32,
        wl_shm::Format::Xrgb8888,
        &qh,
        (),
    );
    let mut n = 0u32;
    while !stop.load(Ordering::Relaxed) && !state.closed {
        let colour: u32 = if n % 2 == 0 { 0x00ff_0000 } else { 0x0000_00ff };
        // SAFETY: `ptr` maps `len` bytes, `w * h` pixels.
        unsafe { std::slice::from_raw_parts_mut(ptr, (w * h) as usize).fill(colour) };
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, w as i32, h as i32);
        surface.commit();
        conn.flush().map_err(|e| e.to_string())?;
        queue
            .dispatch_pending(&mut state)
            .map_err(|e| e.to_string())?;
        n += 1;
        std::thread::sleep(Duration::from_millis(40));
    }
    // SAFETY: unmapping the mapping created above.
    unsafe { libc::munmap(ptr as *mut libc::c_void, len) };
    layer.destroy();
    surface.destroy();
    let _ = conn.flush();
    Ok(())
}

/// Covers an output with alternating red and blue and wiggles a virtual pointer over its right
/// half, until dropped.
pub struct Driver {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Driver {
    /// Starts both helpers for the sway output `output`.
    pub fn start(output: &str) -> Driver {
        let stop = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();
        {
            let (stop, output) = (stop.clone(), output.to_string());
            threads.push(std::thread::spawn(move || {
                if let Err(e) = paint(&output, &stop) {
                    eprintln!("painter: {e}");
                }
            }));
        }
        {
            let stop = stop.clone();
            threads.push(std::thread::spawn(move || {
                if let Err(e) = wiggle(&stop) {
                    eprintln!("virtual pointer: {e}");
                }
            }));
        }
        Driver { stop, threads }
    }
}
fn wiggle(stop: &AtomicBool) -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let (globals, mut queue) = registry_queue_init::<Pointer>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let manager = globals
        .bind::<ZwlrVirtualPointerManagerV1, _, _>(&qh, 1..=2, ())
        .map_err(|e| e.to_string())?;
    let seat = globals.bind::<wl_seat::WlSeat, _, _>(&qh, 1..=5, ()).ok();
    let pointer = manager.create_virtual_pointer(seat.as_ref(), &qh, ());
    queue.roundtrip(&mut Pointer).map_err(|e| e.to_string())?;
    let mut t = 0u32;
    while !stop.load(Ordering::Relaxed) {
        // The extent covers the whole layout of two equal outputs: 600..900 of 1000 lies inside
        // the second one.
        let x = 600 + (t % 300);
        pointer.motion_absolute(t * 20, x, 500, 1000, 1000);
        pointer.frame();
        conn.flush().map_err(|e| e.to_string())?;
        t += 7;
        std::thread::sleep(Duration::from_millis(30));
    }
    pointer.destroy();
    let _ = conn.flush();
    Ok(())
}

impl Drop for Driver {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

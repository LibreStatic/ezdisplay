# Phase 0 findings

Measured facts that shape the design. Each section says where and how it was measured.

## Tablet: Samsung Galaxy Tab S9 Ultra (SM-X910)

Probed over ADB on 2026-10-23 (wireless debugging, then USB).

- Android 16 (API 36), Snapdragon 8 Gen 2 (SM8550, `kalama`).
- Panel 1848x2960 portrait, 280 dpi. The app runs landscape, so the stream is **2960x1848**.
- Display modes at that size: **120, 60 and 30 Hz**. The app selects 120 Hz.
- Video decoders (from `/vendor/etc/media_codecs*.xml`):
  - H.264, HEVC and VP9 each have a regular Qualcomm decoder and a dedicated
    low-latency instance: `c2.qti.avc.decoder.low_latency`, `c2.qti.hevc.decoder.low_latency`
    and `c2.qti.vp9.decoder.low_latency`. Both instances report the low-latency feature, so
    the client prefers the one named `low_latency`.
  - AV1 is software only (`c2.android.av1.decoder`), so AV1 is not worth negotiating here.
- S Pen (`sec_e-pen`, `/dev/input/event13`): `ABS_PRESSURE` 0-4095, `ABS_TILT_X/Y` -63..63,
  `ABS_DISTANCE` 0-255 (hover), `BTN_TOUCH`, `BTN_STYLUS`. Behaviour in the next section.
- Other input: `sec_touchscreen` (multitouch, 0-1847 on X) and the Book Cover Keyboard
  (EF-DX915) with its touchpad.
- USB: the tablet enumerated at **480 Mbit/s (USB 2.0 high speed)** on the dev machine,
  although the machine has a 10 Gbit/s controller and the tablet supports USB 3.2 Gen 1, so
  the cable (or the port) in use is USB 2 only. It is still far above the stream bitrate.

## S Pen behaviour

Recorded with `getevent -lt` on the pen device (about 100 s of scripted hover, strokes, button
presses in hover and in contact, tilt) and logcat at the same time, with the ezdisplay app in
the foreground (not streaming).

Digitizer (what Android turns into stylus `MotionEvent`s):

- About **480 reports/s** both in hover and in contact (hover has occasional longer gaps).
- `ABS_DISTANCE` is 0 in contact and 0 to about 100 in hover (median 48); the pen leaves the
  range (`BTN_TOOL_PEN` up) beyond that, so the declared 255 is never reached.
- `ABS_PRESSURE` is never above 0 without `BTN_TOUCH`. The first report after `BTN_TOUCH`
  down already has 90 to 1860, depending on how fast the tip lands.
- Tilt reached -36..39 of the declared -63..63 with deliberate tilting.
- `BTN_STYLUS` is reported cleanly in hover and in contact, including two quick presses
  during a stroke; there is no combined button state at this level. A button held while the
  pen enters or leaves the hover range goes down or up in the same report as
  `BTN_TOOL_PEN`.
- Attaching the pen to the back of the tablet sends `EV_SW` code 0x0d (named
  `SW_LINEIN_INSERT` by `getevent`) on the same device.

Samsung's Air Command service (logcat):

- The S Pen also sends its button over Bluetooth LE to the Air Command service, about 60 ms
  after the digitizer reports it. The service sorts those presses into single click, double
  click and long click (Air actions).
- A press that **starts while the pen hovers** is ignored ("Hover is entered. ignores the
  click event"), and so is its release, even if the tip touches the screen in between.
- Every other press is an Air action, **with the tip on the screen too**:
  - Single and double clicks look up the foreground app's Air actions. The app defines none
    (`isValidAction is false`, "invokeAction : no action present"). After a few such clicks
    Samsung shows a panel over the app saying it does not support Air actions; here it
    appeared on the second double click, once with the tip touching.
  - Holding the button away from the screen starts the long-click action, by default the
    **camera**, which took the focus away from the app twice during the test.
- The system still sees the digitizer press: `SpenGestureManagerService` logs
  `ACTION_BUTTON_PRESS` with `sideButtonPressed=true` for the presses in contact, before the
  Air action runs.

Consequences for input (phase 2): pen button gestures come from the digitizer (`MotionEvent`
button state while hovering or touching), but outside hover Samsung also turns the same
presses into Air actions, so a double click would map a gesture on the PC and raise the panel
on the tablet. The app cannot intercept the Bluetooth click. The user guide must say how to
turn off Air actions (Settings, Advanced features, S Pen, Air actions). On this tablet the
switch is `spen_air_action=1` in `Settings.System`; if a normal app may read it, the app should
warn while it is on. The app could instead define its own Air actions with
Samsung's S Pen Remote SDK (proprietary, so only behind a flag). Whether the app still
receives every digitizer button event while Air actions are on is still to be checked.

## End-to-end runs (synthetic source, Linux host)

`ezd-host run --source synthetic --adb <serial>` on the dev machine and the app on the tablet,
through `adb reverse`. Negotiated 2960x1848 at 120 Hz, H.264 at 46 Mbit/s, software `libx264`
(`ultrafast`, `zerolatency`; the synthetic source has no GPU encoder). Medians of 1 s windows
over about 27 s of steady state; no frame was dropped in any run and no ack timed out.
"E2E" is host capture to decoded on the tablet, so it leaves out the display latch.

| Link | Video window | Frames/s | Link transfer p50 / p95 | Decode p50 / p95 | E2E p50 / p95 |
|---|---|---|---|---|---|
| USB 2.0 | 1 | 52 | 1.6 / 4.0 ms | 9.4 / 12.7 ms | 26.0 / 32.7 ms |
| USB 2.0 | 2 | 65 | 1.5 / 19.2 ms | 9.8 / 13.3 ms | 26.1 / 44.4 ms |
| Wi-Fi | 1 | 29 | 7.8 / 12.9 ms | 10.7 / 12.9 ms | 31.0 / 41.2 ms |
| Wi-Fi | 2 | 54 | 9.3 / 25.1 ms | 9.7 / 13.3 ms | 36.0 / 53.0 ms |
| Wi-Fi | 3 | 63 | 11.9 / 28.3 ms | 9.7 / 13.5 ms | 35.9 / 55.1 ms |

The video window is the number of frames that may be sent but not yet acknowledged
(`ezd-host run --video-window N`, default 1). The client acks on reception, before decoding.

What sets the frame rate: the host only captures and encodes a frame once the link has a free
slot, so that each frame carries the newest content. With a window of 1 a frame therefore
costs capture + encode + transfer + the ack's way back, one after the other. libx264 alone
takes about 9.5 ms per frame at this size on the dev machine, so over USB the loop is about
19 ms (52 frames/s); on Wi-Fi the round trip adds about 15 ms more (29 frames/s). A larger
window overlaps the next encode with the frame in flight: it nearly doubles the rate on Wi-Fi
for about 5 ms more at p50 and 12 ms more at p95, and over USB it mostly adds queueing at p95.

Consequences:

- At 120 Hz with a window of 1, encode + transfer + ack must fit in 8.3 ms. That is plausible
  over USB with a GPU encoder; on Wi-Fi it is not, so the network mode will probably want a
  window of 2. The default is decided once the GPU encoders are measured on the tablet.
- Over local loopback, with no network, the same stream to the fake client gives 27 ms p50
  from capture to decoded (7 ms at 1080p). The real backends encode on the GPU (VAAPI on the
  Intel iGPU here: 7.4 ms p50 from submit to packet at 1080p), so these runs validate the
  client, the protocol and the links, not the latency target.

The rendered output was checked on a screenshot: full screen, landscape, synthetic pattern
intact, cursor drawn by the client.

## End-to-end runs (KDE virtual output, Linux host)

`ezd-host run --source virtual --adb <serial>` on the dev machine (Plasma 6.6 on Wayland,
Intel UHD 630 and GTX 1650 Mobile, KWin composing on the Intel GPU) and the app on the tablet,
over the same USB 2.0 link. The host resizes the KDE portal's virtual screen to 2960x1848 at
120 Hz. PipeWire delivered linear DMA-BUFs (modifier 0) in these runs; the host now prefers
Y-tiled ones (see "Real use" below). `h264_vaapi` encodes them on the Intel GPU without a copy
through the CPU. KWin only sends a frame when the output has damage,
so mpv played an animated pattern (`testsrc2` at 120 frames/s) full screen on the virtual
output. The figures are medians of 1 s windows over about 25 s. No frame was dropped in any
run. "Capture to encoded" starts at KWin's frame timestamp. "E2E" runs from that timestamp
to decoded on the tablet.

| Encoder | Video window | Frames/s | Capture to encoded p50 / p95 | Link transfer p50 / p95 | Decode p50 / p95 | E2E p50 / p95 |
|---|---|---|---|---|---|---|
| VAAPI regular mode, CBR (3 runs) | 1 | 22-29 | 43-50 ms (p50) | 3.1 / 7.8 ms | 10.8 / 12.4 ms | 56-66 / 65-75 ms |
| VAAPI regular mode, CBR | 2 | 35 | | 3.1 / 5.0 ms | 11.7 / 13.9 ms | 56.4 / 68.1 ms |
| NVENC (`--prefer-dgpu`, system memory) | 1 | 36 | | 3.3 / 9.6 ms | 13.0 / 15.0 ms | 69.0 / 92.8 ms |
| **VAAPI low-power, fixed QP 24** | 1 | 54 | 21.6 / 25.4 ms | 3.6 / 5.6 ms | 11.1 / 13.6 ms | **37.0 / 42.5 ms** |
| VAAPI low-power, fixed QP 24 | 2 | 73 | 23.2 / 29.7 ms | 3.2 / 4.8 ms | 11.0 / 14.2 ms | 37.7 / 45.7 ms |

In the low-power run with a window of 1, a frame waited 9.2 ms (p50) in the PipeWire queue
before the host took it. The encode then took 13.0 / 15.8 ms. Frames were about 70 KB, so
about 30 Mbit/s at 54 frames/s.

The encoder on the Intel GPU sets the pace. Raw throughput at 2960x1848, measured with
`ffmpeg` (600 frames uploaded from system memory, idle desktop):

| Encoder | Frames/s |
|---|---|
| VAAPI regular mode, CBR 46 Mbit/s | 48.7 |
| VAAPI low-power, fixed QP 24 | 95.4 |
| NVENC, `p1`, `ull`, CBR | 107.4 |

The regular mode runs on the GPU's shader cores. KWin and the apps on screen use the same
cores, so under load a frame took 34 ms to encode there, against 13 ms in low-power mode,
which uses the fixed-function encoder. NVENC on the GTX 1650 was slower end to end, because
KWin composes on the Intel GPU and every frame crosses to the NVIDIA GPU through system
memory.

What came up on this machine:

- **Saved virtual output disabled.** KWin restores the saved layout for the output name that
  the KDE portal reuses (`Virtual-virtual-xdp-kde-<app id>`). The layout here had that output
  disabled, so every stream failed with "Could not find output". The failed output also
  stayed behind until the portal restarted. The fix:
  1. Run `kscreen-doctor output.Virtual-virtual-xdp-kde-.enable`, or enable the output in
     Display settings, while it exists.
  2. Run `systemctl --user restart plasma-xdg-desktop-portal-kde`.

  The host now prints these steps when the portal fails this way.
- **Empty app id.** The host has no app id yet, so its output name is the one every other
  unregistered app gets from the portal, and they share the saved layout. A registered app
  id (portal registry or a `.desktop` file) will give it its own name.
- **Damage-driven frames.** A still desktop sends no frames, and the client keeps showing the
  last one. Latency figures need moving content.
- **PipeWire renegotiation.** KWin renegotiates the format when the host resizes the virtual
  output. After fixating a DMA-BUF format, the host must offer the fixated format followed by
  the full list again (as PipeWire's `video-play-fixate` example does). Otherwise the resize
  fails with "no more input formats". The parser for format choices also had to handle fixed
  (`None`) choices and repeated defaults, or the negotiation looped.
- **VAAPI on the UHD 630 (Gen 9.5, iHD driver).**
  - At 2960x1848 and 120 Hz, FFmpeg picks H.264 level 6.x, and every CBR frame then fails
    with "internal encoding error". Level 5.2 works, and the tablet decodes it.
  - Low-power mode only supports fixed QP. The host tries low-power CBR first, then
    low-power fixed QP (what this GPU uses), then the regular mode.
  - Fixed QP has no bitrate cap, so the network mode needs a rate limit of its own.

Screenshots of the tablet showed the KDE desktop full screen, in landscape.

## Real use on the KDE path (dragging windows)

Same machine and setup, VAAPI low-power with fixed QP. The user dragged windows onto the tablet
by hand. For repeatable runs, a KWin script (`loadDeclarativeScript` over D-Bus) moved a window
between the panel and the virtual output, or maximized and restored it every 700 ms.

**Dragging felt as laggy as the existing second-screen app at first. The cause was `adb reverse`, not the USB
bandwidth.**
- While the mouse moved, each video frame took 25-33 ms to reach the app, even with frames
  under 65 KB. A USB 3 cable (5 Gbit/s) did not change it.
- adbd accepts the device-side socket with Nagle's algorithm on. It holds each message until
  the app acknowledges the previous one. The app's kernel delays that acknowledgement by up to
  40 ms when the app has nothing to send back. Small cursor updates between frames triggered
  it.
- Fix: the app's read half sets `TCP_QUICKACK` again after every read (`ezd-link`).
- Result during a drag: link transfer 1-3 ms (from 25-33 ms), E2E p50 33-41 ms (from 60-70 ms).
  The user reported that dragging felt smooth after the fix.

**Scripted drags with a still window** ran at 63-73 frames/s, with about 10 ms per encode and
E2E p50 35-39 ms. That held both for KWrite and for Firefox with a busy page.

**Window animations still stall.** These include the maximize animation and the preview that
appears when a window is dragged against the top edge.
- Each one held a frame for 100-260 ms in the conversion step: `hwmap` plus `scale_vaapi`,
  BGRx to NV12.
- The encoder thread sleeps in `i915_request_wait_timeout` the whole time. The host's own GPU
  time stays low (DRM fdinfo: render about 10 %, video about 45 %, video-enhance 0 %).
- So the conversion waits on the render engine behind KWin's work. KWin uses a high-priority
  GPU context (it has `CAP_SYS_NICE`).
- Part of the wait is KWin finishing the frame itself: the buffer's write fence takes 10-50 ms
  to signal during the animation.
- It happens with KWrite (drawn on the CPU) too, so it is KWin's drawing, not the app's.

Maximize loop with KWrite, 11 s:

| Buffers | Virtual output | Frames held > 40 ms | Wait p95 (fence + conversion) | Worst |
|---|---|---|---|---|
| Linear | 120 Hz | 31 | 127 ms | 156 ms |
| Y-tiled | 120 Hz | 18-22 | 50-56 ms | 78-140 ms |
| Y-tiled | 60 Hz (`--max-refresh-mhz 60000`) | 8 | 29 ms | 75 ms |

- With Y-tiled buffers, KWin finishes each frame sooner (fence p90 12.5 ms, from 19.8 ms), and
  the conversion waits less (p90 29 ms, from 66 ms). The host now prefers Y-tiled, then
  X-tiled, then linear.
- KWin 6.6.4 only offers BGRA and BGRx for screencasts, so the conversion to NV12 can't be
  handed to KWin.

### Animation benchmark

A KWin script shows and hides the quick-tile outline 8 times, maximizes and restores a KWrite
window 8 times, and then moves it for 3 s. For each event, the host trace counts the frames KWin
delivered over PipeWire and the frames encoded in the next 500 ms. GPU busy time is one minus the
RC6 (idle) residency from `/sys/class/drm/card2/power/rc6_residency_ms`, sampled every 10 ms.
Tablet size 2960x1848, Y-tiled buffers. The tool is in
[`tools/kwin-anim-bench`](../tools/kwin-anim-bench/README.md).

Maximize, 500 ms after the event (KWin's animation lasts about 300 ms):

| Run | Frames from KWin | Encoded | GPU busy | E2E p50 / p95 |
|---|---|---|---|---|
| 120 Hz | 20.5 | 10.5 | 97 % | 61 / 206 ms |
| 60 Hz | 18 | 16 | 89 % | 43 / 94 ms |
| 120 Hz, frames dropped without encoding | 25 | - | - | - |
| 60 Hz, frames dropped without encoding | 16-17 | - | 82 % | - |
| 2220x1386, 120 Hz | 26 | 18.5 | 92 % | 32 / 68 ms |
| 2220x1386, 60 Hz | 18.5 | 16 | 87 % | 32 / 117 ms |

- The UHD 630 is the limit. With nothing encoded, KWin alone keeps it 82 % busy during the
  maximize animation at 60 Hz, and 52 % while a window moves. On the 1920x1080 panel, KWin needs
  40 % to move a window and 58 % to maximize one.
- KWin's color settings do not help the stream. The panel's ICC profile is configured but
  inactive (`kscreen-doctor` reports "Color profile source: sRGB"). Switching the panel's color
  power preference from "prefer accuracy" to "prefer efficiency" lowered its maximize cost from
  58 % to 52 % and its move cost from 41 % to 39 %. Virtual outputs do not expose the setting at
  all.
- At 120 Hz the host is the bottleneck. Each frame goes through three steps in series:
  - waiting for KWin's fence: 4-10 ms;
  - the conversion: 2-17 ms, waiting behind KWin on the render engine;
  - the encode: 7-9 ms.

  At 60 Hz the host encodes nearly every frame KWin delivers.
- The GPU clock is not the cause. With `gt_min_freq_mhz` raised to the 1050 MHz maximum, the
  results did not change; the 350 MHz readings were idle (RC6) time. The host could not raise the
  clock anyway: i915 on kernel 6.19 has no fence-deadline boost (`i915_fence_set_deadline` is
  absent), and its wait boost only applies to requests that have not started.
- Maximize and restore on the virtual output stall KWin's main thread for 34-51 ms per event (a
  10 ms script timer fires late), even when nothing is encoded. The same script on the panel
  shows no stall.
- The outline's 100-200 ms gaps also appear with nothing encoded and without main-thread stalls.
  They come from the outline animation itself, not from the host. Unloading the blur effect
  changed nothing.

### Lower resolution and converting ahead

Two changes from the options above, measured with the same benchmark. The client's 1 s
statistics windows give the frames the tablet received and the end-to-end latency (median of the
windows) during the maximize/restore cycles (about 11 s), and the latency during the window moves:

| Run | Frames received | E2E p50 / p95 | Moves p50 |
|---|---|---|---|
| 120 Hz, serial | 140-144 | 61-63 / 175-226 ms | 37 ms |
| 120 Hz, converting ahead | 161-176 | 80-82 / 213-220 ms | 37-38 ms |
| `--scale 0.75`, 120 Hz, serial | 254-260 | 34-37 / 105-217 ms | 32 ms |
| `--scale 0.75`, 120 Hz, converting ahead | 280 | 42-44 / 98-106 ms | 31 ms |
| 60 Hz, serial | 178 | 58 / 148 ms | 34 ms |
| 60 Hz, converting ahead | 194-195 | 63-64 / 145-180 ms | 33-34 ms |

- `--scale 0.75` (2220x1386) nearly doubles the frames during the animations and halves the
  latency. The tablet scales the picture up, so text is less sharp.
- Converting ahead (`EZD_CONVERT_AHEAD=1`) runs the fence wait and the conversion of the next
  frame on its own thread while the core thread encodes and sends the current one. It gives
  9-24 % more frames during the animations but adds 6-20 ms of latency, and the moves do not
  change. The GPU is 91-100 % busy, so the overlap cannot add capacity: while a conversion runs,
  the encoder's submit time goes from 12 to 23 ms (90th percentile). Starting the conversion late, so that it
  ends when the encoder asks for the frame, makes no difference here, because under this load
  the conversion takes longer than the encoder's cycle and the start is never delayed. It stays
  off by default; the NVIDIA path, where the GPU is not saturated, is the next place to try it.
- Waiting for the conversion with `vaSyncSurface` takes no time: the filter graph already
  returns after the GPU has finished.

Next options:
- A 60 Hz default when encoding on an integrated GPU.
- The NVIDIA GPU for both compositing and encoding, either in MUX dGPU mode or with KWin using
  the NVIDIA GPU as primary. This needs the zero-copy NVENC path.
- A higher GPU priority for the conversion needs `CAP_SYS_NICE`. It would also take render time
  from KWin, which is already the limit at 60 Hz.

### Zero-copy NVENC

A test program allocated buffers on the GTX 1650 with GBM (the `nvidia-drm` backend), linear and
block-linear (`0x0300000000606014`), and ran them through FFmpeg 8.1:

1. A `DRM_PRIME` frame on `/dev/dri/renderD128`.
2. `hwmap` to a Vulkan device derived from that DRM device with `disable_multiplane=1`.
3. `scale_vulkan=format=nv12:out_range=tv`.
4. `hwupload` to a CUDA device derived from the Vulkan one, which is a copy inside the GPU.
5. `h264_nvenc`.

Both layouts decoded to the exact colors with BT.709 limited range. At 2960x1848 the conversion
took 1.5-2 ms and the encode 1.7-2 ms per frame; on the UHD 630 they take 2-17 ms and 7-9 ms.

- `scale_vulkan` needs the colorspace set on the input frame. Without it, it fails with
  "Unsupported colorspace".
- Mapping Vulkan frames to CUDA with `hwmap` fails with `ENOSYS`, so the chain uploads instead.
- The `ffmpeg` command line cannot pass `disable_multiplane` to a Vulkan device derived from a
  DRM device, so the chain only works through the API.
- GBM on NVIDIA refuses the linear modifier through `gbm_bo_create_with_modifiers2` (plain
  `gbm_bo_create` with `GBM_BO_USE_LINEAR` works) and cannot map block-linear buffers.
- FFmpeg never frees a Vulkan frames context that a DRM frame was mapped into, with `hwmap` or
  `av_hwframe_map`. The mapping leaves the mapped frame in a command buffer of the context's own
  pool, and the frame references the context. The context's Vulkan device leaks with it: about
  43 fds and 65 MB per filter graph on NVIDIA, 13 fds and 14 MB on Intel, and vkCreateDevice
  crashed after about 16. The same code is in FFmpeg master. The host maps into one Vulkan
  frames context per buffer format and size, shared by the whole process. Opening a converter
  again no longer leaks, and the last 8 mapped buffers (one per compute queue) stay referenced
  until later mappings reuse their command buffers (only multi-plane buffers still go through
  FFmpeg's map, see below). The leaked devices also made the tests flaky:
  a buffer filled through Vulkan sometimes read back as zeros.
- Exiting the process while NVENC is being closed on another thread deadlocks: the exit handler
  of `libnvcuvid` and `avcodec_free_context` on the video thread block each other inside the
  driver. Ctrl-C used to drop the session and return from `main` with the video thread still
  stopping the source, so the host hung after "capture stopped" (also with `nvenc-sysmem`). Ctrl-C
  now ends the session through `SessionOptions::shutdown`, which sends `Bye` and waits for the
  source to stop.
- Some imports sampled as black. FFmpeg's DRM to Vulkan map (`av_hwframe_map`, the same in
  master) binds the imported memory with a dedicated allocation only when the driver prefers or
  requires one, and the NVIDIA driver (595) reports neither. Without one, some imports sample as
  zeros for the whole life of the image. Which ones depends only on the sequence of imports, not
  on timing or on the memory type FFmpeg picks: with 4 test buffers in turn, linear buffers came
  out black at frames 19, 52 and 85, block-linear ones at 7, 23, 40, 56 and 73, and with KWin's
  buffers about one frame in 16 was black, which showed as flicker while scrolling. With a
  dedicated allocation all 600 frames were right for both layouts; system memory (type 0) still
  failed. The host now imports single-plane buffers itself (`vkdma.rs`, through `ash` and
  FFmpeg's Vulkan device), preferring device-local memory, and hands FFmpeg the image as an
  `AVVkFrame`. The test `nvenc_dmabuf_many_frames_keep_their_content` fails with FFmpeg's map.
  `EZD_DIAG_DUMP=<file>` writes the stream as sent, which showed that the black frames were
  already in it.

### KWin on the NVIDIA GPU (hybrid mode)

With `export KWIN_DRM_DEVICES=/dev/dri/card1:/dev/dri/card2` in
`~/.config/plasma-workspace/env/kwin-nvidia.sh` (card1 is the GTX 1650), KWin composites on the
NVIDIA GPU and the Intel GPU only drives the panel. The host then encodes with `nvenc-dmabuf`.
Same benchmark, maximize and restore. NVIDIA busy is `utilization.gpu` from `nvidia-smi`, sampled
every 100 ms over the whole run (mean / max):

| Run | Frames from KWin | Encoded | NVIDIA busy | E2E p50 / p95 |
|---|---|---|---|---|
| 120 Hz | 26-28 | 22-23.5 | 19 / 38 % | 34 / 64 ms |
| 120 Hz, with the import fix | 27.5-28 | 23-25 | - | 32 / 50 ms |
| 60 Hz | 17 | 15-15.5 | 17 / 39 % | 36 / 88 ms |
| 120 Hz, frames dropped without encoding | 20.5-21 | - | 20 / 39 % | - |
| 120 Hz, converting ahead | 27.5-30 | 26-26.5 | 19 / 38 % | 31 / 66 ms |
| `--scale 0.75`, 120 Hz | 28-28.5 | 22-24 | 17 / 38 % | 28 / 58 ms |

Only the second row has the import fix above; that run also wrote the stream to disk and recorded
the tablet's screen.

- Against the UHD 630 at 120 Hz (20.5 frames from KWin, 10.5 encoded, 61 / 206 ms), KWin delivers
  about 30 % more frames, the host encodes twice as many and the latency halves. The GTX 1650
  stays under 40 % busy and its encoder under 42 %, so the GPU is no longer the limit.
- Window moves run at 64 frames/s from KWin and encoded in every run, with gaps of 17-18 ms at
  the 95th percentile.
- Dropping every frame without encoding gave fewer frames from KWin, not more, so the host does
  not hold KWin back. The main-thread stalls on maximize and restore remain (40-50 ms per event).
- Converting ahead encodes 2-3 more frames per event without lowering the latency, and
  `--scale 0.75` lowers it by a few ms. 60 Hz gives fewer frames and more latency than 120 Hz.
- In use, with the import fix, the tablet felt about the same as the built-in panel, scrolling
  included.
- This laptop (HP Pavilion Gaming 16-a0xxx, board 87AD, BIOS F.27) has no MUX, so there is no
  dGPU-only mode to compare with. The panel's eDP connector exists only on the Intel GPU,
  `nvidia-drm` lists only HDMI and DisplayPort, and the BIOS has no graphics mode setting (HP
  offers one on OMEN and Victus models). Hybrid mode is already the closest setup: the virtual
  output is rendered and encoded on the NVIDIA GPU, and only the panel's frames are copied to the
  Intel GPU.

## Still to measure

- The table runs above with a USB 3 cable. A drag over USB 3 did not change the link transfer.
- The stylus `MotionEvent`s the app receives (button state, hover, tool type) with Air
  actions on and off, and whether an app can read `spen_air_action`.
- The Windows path on the Nitro (see `phase0-nitro-checklist.md` and `windows-testing.md`).
- The KDE path with the cursor crossing between screens, over Wi-Fi, and glass to glass with
  `tools/latency`.

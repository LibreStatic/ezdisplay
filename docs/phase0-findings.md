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
  after the digitizer reports it. While the pen is in hover the service ignores that click
  ("Hover is entered. ignores the click event"), and pressing the button in hover did not open
  the Air Command menu over the full-screen app.
- With the pen **out of hover range**, holding the button starts an Air action. The default
  one opens the **camera**, which took the focus away from the app twice during the test. The
  app defines no Air actions (`isValidAction is false` for its package).

Consequences for input (phase 2): pen button gestures should come from the digitizer
(`MotionEvent` button state while hovering or touching), where Samsung does not act on them.
Holding the button away from the screen cannot be intercepted by the app; the user guide must
say how to turn off the pen-button Air actions, or the app could define its own Air actions
with Samsung's S Pen Remote SDK (proprietary, so only behind a flag). What the app actually
receives in `onGenericMotionEvent` while Air Command is enabled is still to be checked.

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

## Still to measure

- The same runs with a USB 3 cable.
- The stylus `MotionEvent`s the app receives (button state, hover, tool type) with Air
  Command enabled.
- The real Linux path (KDE portal, VAAPI) on the tablet, and the Windows path on the Nitro
  (see `phase0-nitro-checklist.md` and `windows-testing.md`).

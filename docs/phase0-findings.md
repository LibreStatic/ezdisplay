# Phase 0 findings

Measured facts that shape the design. Each section says where and how it was measured.

## Tablet: Samsung Galaxy Tab S9 Ultra (SM-X910)

Probed over ADB on 2026-10-23 (wireless debugging; USB not yet measured).

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
  `ABS_DISTANCE` 0-255 (hover), `BTN_TOUCH`, `BTN_STYLUS`. Button behaviour (hover vs contact,
  Air Command) is still to be recorded.
- Other input: `sec_touchscreen` (multitouch, 0-1847 on X) and the Book Cover Keyboard
  (EF-DX915) with its touchpad.

## First end-to-end run (synthetic source, Linux host, Wi-Fi ADB)

`ezd-host run --source synthetic --adb <serial>` on the dev machine, app on the tablet,
`adb reverse` over wireless debugging. Negotiated 2960x1848 at 120 Hz, H.264 at 46 Mbit/s,
software `libx264` (the synthetic source has no GPU encoder).

| Metric (1 s windows, steady state) | Value |
|---|---|
| Frames per second reaching the tablet | about 25 |
| Dropped frames | 0 |
| Link transfer (encoded -> received) p50 | 5-9 ms |
| Decode on the tablet p50 / p95 | about 11 ms / 14 ms |
| Host capture -> decoded p50 / p95 | about 31 ms / 40 ms |

The frame rate is bound by software x264 at 5.5 megapixels: the same stream to the fake
client over local loopback, with no network, gives 27 ms p50 from capture to decoded
(it is 7 ms at 1080p). The real backends encode on the GPU (VAAPI on the Intel iGPU here:
7.4 ms p50 from submit to packet at 1080p), so this run validates the client and the
protocol, not the latency target.

The rendered output was checked on a screenshot: full screen, landscape, synthetic pattern
intact, cursor drawn by the client.

## Still to measure

- USB: link speed and the same run over a cable instead of Wi-Fi.
- S Pen button events in hover and in contact, and whether Air Command interferes.
- The real Linux path (KDE portal, VAAPI) on the tablet, and the Windows path on the Nitro
  (see `phase0-nitro-checklist.md` and `windows-testing.md`).

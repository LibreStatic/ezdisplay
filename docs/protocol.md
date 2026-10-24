# ezdisplay wire protocol (version 0.3)

All integers are little endian. The protocol runs over any reliable ordered byte stream (TCP
through `adb reverse` today; AOA bulk endpoints later). It is implemented in the `ezd-proto`
crate (messages, framing) and `ezd-link` (prioritized mux).

## Framing

Every frame is an 8-byte header followed by `payload_len` bytes of payload.

```
 0       4       5       6       8
 +-------+-------+-------+-------+----------...
 | len   |  ch   | kind  | flags | payload
 | u32   |  u8   |  u8   | u16   |
```

| Field | Meaning |
| --- | --- |
| `payload_len` | Payload length in bytes, at most `MAX_FRAME_LEN` = 16 MiB. A larger value is a fatal protocol error. |
| `channel` | Logical channel, see below. |
| `kind` | Message type within the channel. |
| `flags` | Bit 0 `MORE`: this frame is a fragment and the message continues in a later frame with the same `(channel, kind)`. Other bits are reserved and must be ignored. |

### Channels

| Id | Name | Use |
| --- | --- | --- |
| 0 | Control | Handshake, capabilities, keepalive, statistics. |
| 1 | Input | Pen, touch, pointer buttons and scroll, client to host (0.3). |
| 2 | Cursor | Cursor shape and position. |
| 3 | Video | Encoded video frames. |
| 4 | Audio | Reserved. |

### Kinds

| Channel | Kind | Message | Direction | Payload |
| --- | --- | --- | --- | --- |
| Control | 1 | `Hello` | both | postcard |
| Control | 2 | `ClientCaps` | client to host | postcard |
| Control | 3 | `HostCaps` | host to client | postcard |
| Control | 4 | `StreamConfig` | host to client | postcard |
| Control | 5 | `ConfigAck` | client to host | empty |
| Control | 6 | `Ping` | either | postcard |
| Control | 7 | `Pong` | either | postcard |
| Control | 8 | `RequestKeyframe` | client to host | empty |
| Control | 9 | `ClientStats` | client to host | postcard |
| Control | 10 | `Reject` | either | postcard |
| Control | 11 | `Bye` | either | postcard |
| Control | 12 | `FrameAck` | client to host | postcard |
| Control | 13 | `InputStatus` | host to client | postcard (0.3) |
| Control | 14 | `ClientOptions` | host to client | postcard (0.3) |
| Input | 1 | `Pen` (`PenSample`) | client to host | postcard (0.3) |
| Input | 2 | `Touch` (`TouchFrame`) | client to host | postcard (0.3) |
| Input | 3 | `PointerButton` | client to host | postcard (0.3) |
| Input | 4 | `Scroll` | client to host | postcard (0.3) |
| Cursor | 1 | `CursorShape` | host to client | postcard |
| Cursor | 2 | `CursorState` | host to client | postcard |
| Video | 1 | `VideoFrame` | host to client | postcard |

The payload of a message is the [postcard](https://docs.rs/postcard) encoding (serde, varint
integers, no field names) of the struct of that name in `ezd-proto/src/msg.rs`. Byte vectors
(`VideoFrame.data`, `CursorShape.rgba`) use `serde_bytes`, i.e. a varint length followed by
raw bytes. The struct definitions are the normative field list and order.

### Fragmentation and reassembly

A message whose payload exceeds the writer's chunk size (default 64 KiB) is split into several
frames. Every fragment except the last carries `MORE`; the last one does not. The receiver
concatenates the payloads of all frames of one `(channel, kind)` until a frame without `MORE`.

Complete frames (and fragments of other `(channel, kind)` pairs) may be interleaved between
the fragments of a message. This is how a cursor update overtakes a large keyframe; reassembly
is therefore keyed by `(channel, kind)`. The fragments of one key stay in order. A reassembled
message must not exceed `MAX_MESSAGE_LEN` = 32 MiB.

The IO-free `FrameDecoder` accepts bytes in arbitrary pieces (one byte at a time or many frames
per push) and yields complete messages.

### Forward compatibility

* A `(channel, kind)` pair the receiver does not know is skipped silently (it is reassembled,
  counted and dropped), never an error. This includes the reserved Input and Audio channels.
* Unknown flag bits are ignored.
* A known pair with a malformed payload is a protocol error and the connection should be
  closed. Because postcard is not self-describing, existing structs are never extended in
  place within a major version: new data gets a new `kind`.

### Versioning

`Hello` carries `proto_major` and `proto_minor` (currently 0 and 3; 0.2 added `FrameAck`, 0.3
the input messages, `InputStatus` and `ClientOptions`). A different major version is fatal: the
receiver answers `Reject` and closes. Minor versions only add new `(channel,
kind)` pairs, so peers with different minors interoperate. `Hello::supports_input()` is true
when the peer's minor is at least `MINOR_INPUT` = 3; a host must not send `InputStatus` or
`ClientOptions` to an older client (it would skip them anyway), and a client must not send
Input messages to an older host.

## Handshake and session

```
client                                     host
  |  Hello(role=Client, version, name) ----->|
  |<----- Hello(role=Host, version, name)    |   either order; major mismatch -> Reject
  |  ClientCaps(display, decoders, ...) ---->|
  |<---------------------- HostCaps(encoders)|
  |            negotiate(ClientCaps, HostCaps, Prefs)  (host, pure function)
  |<-------------------------- StreamConfig |   or Reject{reason}
  |  open decoder                            |
  |  ConfigAck ----------------------------->|
  |                                          |   host starts capture + encode
  |<=== VideoFrame(CODEC_CONFIG) ============|
  |<=== VideoFrame(KEYFRAME) ================|
  |<=== VideoFrame..., CursorShape/State ====|   Ping/Pong both ways, ClientStats ~1 Hz
  |  FrameAck (per frame, before decode) --->|   releases the host's video credit
  |  RequestKeyframe (on decode error) ----->|   next frame is an IDR
  |  Bye ------------------------------------>|   or EOF; host closes its side
```

Cursor messages may arrive any time after `StreamConfig`; a client must tolerate cursor traffic
before its first video frame. Handshake messages are answered within a few seconds or the peer
gives up (the host uses 10 s per step).

### Negotiation (`ezd_proto::negotiate`)

* Codec: H.264 unless the preferences ask for another codec and both sides support it
  (fallback order H.264, HEVC, AV1 among codecs both sides share).
* Decoder: hardware and low-latency preferred; its limits constrain the stream.
* Resolution: the client's native size, first scaled by the host's `--scale` (default 1.0), then
  scaled down (aspect preserved) to the decoder limits, rounded down to even numbers.
* Refresh rate: the highest client rate not above `max_refresh_mhz` (default 120000) and the
  decoder `max_fps`. The encoder frame rate equals the refresh rate.
* Bitrate: `width * height * fps * 0.07` bit/s for H.264 (`0.05` for HEVC and AV1), clamped to
  the link ceiling (USB 80 Mbit/s, network 30 Mbit/s by default).
* Encoder: the first hardware encoder for the codec, otherwise the first software one.

## Video

`VideoFrame { frame_id, flags, capture_us, encoded_us, data }`. `data` is an Annex-B bitstream
for H.264 and HEVC. `flags`: bit 0 `KEYFRAME`, bit 1 `CODEC_CONFIG`.

### Codec-config rule

A `CODEC_CONFIG` frame contains only parameter sets (SPS and PPS; VPS as well for HEVC) in
Annex-B form. The host sends one before the first keyframe of a session and again whenever the
parameter sets change. The client hands it to its decoder as codec-specific data (on Android,
`MediaCodec` configuration or a buffer flagged `BUFFER_FLAG_CODEC_CONFIG`) before decoding the
keyframe that follows. `frame_id` of a config frame equals that of the frame sent right after it.
Keyframes may additionally carry the parameter sets in-band.

### Video credit and coalescing

Encoded frames are never dropped: dropping a P-frame corrupts every following picture. Instead
the host applies back-pressure before capture/encode:

1. the sender asks the mux for a video slot (`wait_video_slot`). It resolves only when no video
   frame is queued or being written **and** fewer than `window` frames are written but not yet
   acknowledged by the client (default window: 1);
2. only then it pulls the **newest** captured content, encodes it and sends it.

The credit is therefore released by the client, not by the socket write: the client sends
`FrameAck { frame_id, received_us }` (`received_us` on the client clock) as soon as a
non-config `VideoFrame` is fully reassembled, **before** decoding. An ack for id N releases N
and every outstanding id below N. `CODEC_CONFIG` frames are never acked and hold no credit.
If no ack arrives within the timeout (default 500 ms) the host releases the credit anyway,
counts an `ack_timeout` and logs a warning, so a client that never acks runs at about 2 fps
instead of stalling. Clients must therefore send acks.

When clock sync has an offset, the host maps `received_us` to its own clock and subtracts the
frame's `encoded_us` to get the link transfer time, reported as p50/p95.

If the link is slower than the display refresh rate, intermediate screen states are never
encoded, and `frame_id` skips numbers (it is strictly increasing but not contiguous). A
`CODEC_CONFIG` frame and the keyframe after it are queued back to back and share one credit.
The host may also be configured with write-based credit (freed when the write completes),
which is only meant for tests.

### Priority

The writer serves queues in the order control, input, cursor shapes, cursor state, video.
Video frames are written in chunks and the higher-priority queues are re-checked between
chunks, so a large keyframe delays a control, input or cursor message by at most one chunk
(64 KiB by default). `CursorState` is latest-wins: a state that has not been written yet is
replaced by a newer one. Everything else is reliable FIFO. Note that control messages overtake
video: send `Bye` after the last frame has been written if the peer should see all frames first.

Input is batched: when the writer takes input it drains every queued input message into one
buffer and writes and flushes it once. Under a backlog (128 or more queued input messages) a
new `Pen` replaces the last queued message if that is a `Pen` with identical `flags`, and a new
`Touch` whose contacts are all `MOVE` replaces the last queued message if that is also an
all-`MOVE` `Touch` with the same set of ids. Transitions (any pen flag change, `DOWN`, `UP`,
`CANCEL`, buttons, scrolls) are never dropped.

## Input (0.3)

All input messages travel client to host on the Input channel and carry `t_us`, the client's
monotonic clock (same epoch as the other client timestamps). Coordinates `x`, `y` are `u16`
normalized over the stream: 0 is the left/top edge and 65535 (`NORM_MAX`) the right/bottom edge
(`normalize(px, extent)` and `denormalize(v, extent)` convert). Codes and flag bits are plain
integers, not enums: receivers ignore unknown codes and bits.

`Pen(PenSample { t_us, x, y, pressure, tilt_x, tilt_y, flags })`, one message per sample.
`pressure` 65535 = 1.0. Tilt is in hundredths of a degree (-9000..=9000); positive `tilt_x` =
top of the pen tilted to the right, positive `tilt_y` = tilted toward the user. `flags`:
`IN_RANGE` = 1, `CONTACT` = 2, `BARREL` = 4, `ERASER` = 8. A sample without `IN_RANGE` means
the pen left proximity.

`Touch(TouchFrame { t_us, contacts })`, with `TouchContact { id: u8, x, y, phase: u8 }` and
phases `DOWN` = 0, `MOVE` = 1, `UP` = 2, `CANCEL` = 3. Every frame lists ALL active contacts;
a contact that ended in this frame appears once with `UP` or `CANCEL` and is absent afterwards.

`PointerButton { t_us, x, y, button: u8, pressed: bool }` with `LEFT` = 0, `RIGHT` = 1,
`MIDDLE` = 2.

`Scroll { t_us, x, y, dx: i32, dy: i32, modifiers: u8 }`. Deltas are in 1/120 of a notch with
the signs of Windows `WHEEL_DELTA` and evdev `REL_WHEEL`/`REL_HWHEEL`: `dy > 0` = wheel rotated
forward, away from the user (scroll up; zoom in with Ctrl), `dx > 0` = right. `modifiers`:
`CTRL` = 1, `SHIFT` = 2.

Host to client, on the Control channel: `InputStatus { caps: u32, note: String }` with `caps`
bits `TOUCH` = 1, `PEN` = 2, `ACTIONS` = 4 (what the host can inject) and a human-readable
`note` explaining what is unavailable (empty when all is fine); `ClientOptions { flags: u32 }`
with `TELEMETRY` = 1 (show the telemetry overlay).

## Cursor

`CursorShape { shape_id, width, height, hot_x, hot_y, rgba }` is sent once per `shape_id` per
session (straight RGBA8, row-major, no padding, `width * height * 4` bytes). `CursorState {
x, y, visible, shape_id, ts_us }` gives the position of the hotspot in virtual-display pixels
(may lie outside the display). A shape is always written before a state that references it.
The client draws the cursor locally on top of the video.

## Clock synchronization

Either side may send `Ping { seq, t0_us }`; the peer answers immediately with `Pong { seq, t0_us,
t1_us, t2_us }`, where `t1` is its clock when the ping arrived and `t2` its clock when the pong
was sent. All clocks are local monotonic microseconds with an arbitrary epoch. With `t3` the
pinger's clock when the pong arrived:

```
rtt    = (t3 - t0) - (t2 - t1)
offset = ((t1 - t0) + (t2 - t3)) / 2          (peer clock - local clock)
```

`ClockSync` keeps a sliding window (16 samples) and uses the minimum-RTT sample, the least
disturbed by queueing; the error is bounded by half the RTT asymmetry. The host pings every
500 ms; the Android client should ping as well, because it needs the offset to convert the host's
`capture_us` into its own clock for end-to-end latency (`e2e = decode_done - (capture_us - offset)`).

## Statistics

`ClientStats { window_ms, frames_received, frames_decoded, frames_dropped, decode_us_p50,
decode_us_p95, e2e_us_p50, e2e_us_p95 }` is sent about once per second; the host logs it.

//! PC audio playback: an AAudio output stream fed from a jitter buffer.
//!
//! PCM packets are converted and pushed straight into the buffer from the client thread. Opus
//! packets go to a small decode thread that owns a MediaCodec `audio/opus` decoder and pushes
//! its output. AAudio pulls from the buffer on its own real-time thread. If the device goes
//! away (headphones, route change) the stream is reopened once, on a helper thread.

use crate::jitter::{
    AudioStats, Jitter, SAMPLE_RATE, f32_to_i16, opus_csd1, opus_csd2, opus_pre_skip, pcm_to_f32,
};
use ezd_proto::{AudioCodec, AudioPacket, AudioStream as StreamInfo, SampleFormat};
use ndk::audio::{
    AudioCallbackResult, AudioDirection, AudioFormat, AudioPerformanceMode, AudioSharingMode,
    AudioStream, AudioStreamBuilder,
};
use ndk::media::media_codec::{
    DequeuedInputBufferResult, DequeuedOutputBufferInfoResult, MediaCodec, MediaCodecDirection,
};
use ndk::media::media_format::MediaFormat;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

const OPUS_MIME: &str = "audio/opus";
/// `AudioFormat.ENCODING_PCM_FLOAT`.
const ENCODING_PCM_FLOAT: i32 = 4;
const INPUT_WAIT: Duration = Duration::from_millis(5);
const IDLE_POLL: Duration = Duration::from_millis(5);
/// Frames per chunk when the stream wants 16-bit samples.
const CHUNK_SAMPLES: usize = 1024;

/// True when the device has an Opus decoder.
pub fn opus_available() -> bool {
    MediaCodec::from_decoder_type(OPUS_MIME).is_some()
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// AAudio streams can be used from any thread.
struct SendStream(AudioStream);
// SAFETY: AAudio stream handles are thread-safe; the owned callbacks are `Send`.
unsafe impl Send for SendStream {}

/// State shared by the player, the AAudio callbacks and the helper threads.
struct Core {
    jitter: Mutex<Jitter>,
    stats: AudioStats,
    channels: usize,
    i16_out: bool,
    stream: Mutex<Option<SendStream>>,
    closing: AtomicBool,
    reopened: AtomicBool,
}

/// Plays one negotiated audio stream until dropped.
pub struct AudioPlayer {
    core: Arc<Core>,
    format: SampleFormat,
    opus: Option<OpusDecoder>,
    reopen_tx: Sender<()>,
    reopen_thread: Option<JoinHandle<()>>,
    scratch: Vec<f32>,
}

impl AudioPlayer {
    /// Opens the output (and the Opus decoder when needed) for `info`.
    pub fn new(info: &StreamInfo) -> Result<AudioPlayer, String> {
        if info.sample_rate != SAMPLE_RATE {
            return Err(format!("unsupported sample rate {}", info.sample_rate));
        }
        if info.channels == 0 {
            return Err("audio stream without channels".into());
        }
        let channels = info.channels as usize;
        let core = Arc::new(Core {
            jitter: Mutex::new(Jitter::new(info.sample_rate, channels)),
            stats: AudioStats::default(),
            channels,
            i16_out: info.codec == AudioCodec::Pcm && info.format == SampleFormat::S16,
            stream: Mutex::new(None),
            closing: AtomicBool::new(false),
            reopened: AtomicBool::new(false),
        });
        let opus = match info.codec {
            AudioCodec::Pcm => None,
            AudioCodec::Opus => Some(OpusDecoder::spawn(core.clone(), info)?),
        };
        let (reopen_tx, reopen_rx) = channel();
        let stream = open_output(&core, &reopen_tx)?;
        tracing::info!(
            "audio output {} Hz, {} ch, {}, {} frames buffer (burst {})",
            stream.0.sample_rate(),
            stream.0.channel_count(),
            if core.i16_out { "i16" } else { "f32" },
            stream.0.buffer_size_in_frames(),
            stream.0.frames_per_burst(),
        );
        *lock(&core.stream) = Some(stream);
        let reopen_thread = {
            let (core, tx) = (core.clone(), reopen_tx.clone());
            std::thread::Builder::new()
                .name("ezd-audio-reopen".into())
                .spawn(move || reopen_loop(&core, &tx, reopen_rx))
                .ok()
        };
        Ok(AudioPlayer {
            core,
            format: info.format,
            opus,
            reopen_tx,
            reopen_thread,
            scratch: Vec::new(),
        })
    }

    /// Handles one packet from the host without blocking.
    pub fn on_packet(&mut self, pkt: AudioPacket) {
        use std::sync::atomic::Ordering::Relaxed;
        self.core.stats.packets.fetch_add(1, Relaxed);
        match &self.opus {
            Some(opus) => opus.send(pkt),
            None => {
                self.scratch.clear();
                pcm_to_f32(&pkt.data, self.format, &mut self.scratch);
                push(&self.core, &self.scratch);
            }
        }
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.core.closing.store(true, Ordering::Release);
        let _ = self.reopen_tx.send(());
        if let Some(j) = self.reopen_thread.take() {
            let _ = j.join();
        }
        // The decoder thread joins here, before the stream goes away.
        self.opus = None;
        // Dropping the stream stops and closes it.
        let stream = lock(&self.core.stream).take();
        drop(stream);
        let (packets, underruns, dropped) = self.core.stats.snapshot();
        tracing::info!(
            "audio stopped: {packets} packets, {underruns} underruns, {dropped} frames dropped"
        );
    }
}

/// Pushes decoded samples into the jitter buffer and counts what it dropped.
fn push(core: &Core, samples: &[f32]) {
    let dropped = lock(&core.jitter).push(samples);
    if dropped > 0 {
        core.stats
            .dropped_frames
            .fetch_add(dropped as u64, Ordering::Relaxed);
    }
}

fn open_output(core: &Arc<Core>, reopen_tx: &Sender<()>) -> Result<SendStream, String> {
    let data_core = core.clone();
    let err_tx = reopen_tx.clone();
    let channels = core.channels;
    let i16_out = core.i16_out;
    let stream = AudioStreamBuilder::new()
        .map_err(|e| format!("audio builder: {e}"))?
        .direction(AudioDirection::Output)
        .performance_mode(AudioPerformanceMode::LowLatency)
        .sharing_mode(AudioSharingMode::Shared)
        .sample_rate(SAMPLE_RATE as i32)
        .channel_count(channels as i32)
        .format(if i16_out {
            AudioFormat::PCM_I16
        } else {
            AudioFormat::PCM_Float
        })
        .data_callback(Box::new(move |_stream, data, frames| {
            let samples = frames.max(0) as usize * channels;
            let mut jitter = lock(&data_core.jitter);
            let mut underrun = false;
            if i16_out {
                // SAFETY: AAudio hands over `frames * channels` writable samples of the format
                // that was requested.
                let out = unsafe { std::slice::from_raw_parts_mut(data.cast::<i16>(), samples) };
                let mut tmp = [0f32; CHUNK_SAMPLES];
                for chunk in out.chunks_mut(CHUNK_SAMPLES) {
                    let t = &mut tmp[..chunk.len()];
                    underrun |= jitter.pull(t);
                    for (o, s) in chunk.iter_mut().zip(t.iter()) {
                        *o = f32_to_i16(*s);
                    }
                }
            } else {
                // SAFETY: as above, for f32.
                let out = unsafe { std::slice::from_raw_parts_mut(data.cast::<f32>(), samples) };
                underrun = jitter.pull(out);
            }
            drop(jitter);
            if underrun {
                data_core.stats.underruns.fetch_add(1, Ordering::Relaxed);
            }
            AudioCallbackResult::Continue
        }))
        .error_callback(Box::new(move |_stream, err| {
            tracing::warn!("audio stream error: {err}");
            let _ = err_tx.send(());
        }))
        .open_stream()
        .map_err(|e| format!("open audio stream: {e}"))?;
    // Two bursts keep the latency low while tolerating a late callback.
    let burst = stream.frames_per_burst();
    if burst > 0
        && let Err(e) = stream.set_buffer_size_in_frames(burst * 2)
    {
        tracing::debug!("audio buffer size: {e}");
    }
    stream
        .request_start()
        .map_err(|e| format!("start audio stream: {e}"))?;
    Ok(SendStream(stream))
}

/// Reopens the output once after an error; never runs inside an AAudio callback.
fn reopen_loop(core: &Arc<Core>, tx: &Sender<()>, rx: Receiver<()>) {
    while rx.recv().is_ok() {
        if core.closing.load(Ordering::Acquire) {
            return;
        }
        if core.reopened.swap(true, Ordering::AcqRel) {
            tracing::warn!("audio stream failed again, not reopening");
            continue;
        }
        let old = lock(&core.stream).take();
        drop(old);
        lock(&core.jitter).reset();
        match open_output(core, tx) {
            Ok(s) => {
                tracing::info!("audio stream reopened");
                *lock(&core.stream) = Some(s);
            }
            Err(e) => tracing::warn!("cannot reopen the audio stream: {e}"),
        }
    }
}

// ---------------------------------------------------------------- Opus

struct OpusDecoder {
    tx: Option<Sender<AudioPacket>>,
    join: Option<JoinHandle<()>>,
}

impl OpusDecoder {
    /// Starts the decode thread and waits for the codec to be configured.
    fn spawn(core: Arc<Core>, info: &StreamInfo) -> Result<OpusDecoder, String> {
        let (tx, rx) = channel();
        let (ready_tx, ready_rx) = channel();
        let info = info.clone();
        let join = std::thread::Builder::new()
            .name("ezd-audio-opus".into())
            .spawn(move || match create_codec(&info) {
                Ok((codec, float)) => {
                    let _ = ready_tx.send(Ok(()));
                    decode_loop(&codec, float, &core, rx);
                    let _ = codec.stop();
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| format!("spawn opus thread: {e}"))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(OpusDecoder {
                tx: Some(tx),
                join: Some(join),
            }),
            Ok(Err(e)) => {
                let _ = join.join();
                Err(e)
            }
            Err(_) => Err("opus thread died".into()),
        }
    }

    fn send(&self, pkt: AudioPacket) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(pkt);
        }
    }
}

impl Drop for OpusDecoder {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn opus_format(info: &StreamInfo, float: bool) -> Result<MediaFormat, String> {
    let pre_skip = opus_pre_skip(&info.codec_config).ok_or("invalid OpusHead")?;
    let mut f = MediaFormat::new();
    f.set_str("mime", OPUS_MIME);
    f.set_i32("sample-rate", SAMPLE_RATE as i32);
    f.set_i32("channel-count", info.channels as i32);
    f.set_buffer("csd-0", &info.codec_config);
    f.set_buffer("csd-1", &opus_csd1(pre_skip));
    f.set_buffer("csd-2", &opus_csd2());
    if float {
        f.set_i32("pcm-encoding", ENCODING_PCM_FLOAT);
    }
    Ok(f)
}

/// Configures and starts the decoder, asking for float output first. Returns the codec and
/// whether float output was requested.
fn create_codec(info: &StreamInfo) -> Result<(MediaCodec, bool), String> {
    let mut last = String::new();
    for float in [true, false] {
        let codec = MediaCodec::from_decoder_type(OPUS_MIME)
            .ok_or_else(|| format!("no decoder for {OPUS_MIME}"))?;
        let fmt = opus_format(info, float)?;
        match codec.configure(&fmt, None, MediaCodecDirection::Decoder) {
            Ok(()) => match codec.start() {
                Ok(()) => return Ok((codec, float)),
                Err(e) => last = format!("start: {e}"),
            },
            Err(e) => last = format!("configure (float={float}): {e}"),
        }
        tracing::warn!("opus decoder setup failed: {last}");
    }
    Err(last)
}

fn decode_loop(codec: &MediaCodec, requested_float: bool, core: &Core, rx: Receiver<AudioPacket>) {
    // Until the codec reports its output format, assume what was asked for.
    let mut float = requested_float;
    let mut samples: Vec<f32> = Vec::new();
    loop {
        match rx.recv_timeout(IDLE_POLL) {
            Ok(pkt) => match codec.dequeue_input_buffer(INPUT_WAIT) {
                Ok(DequeuedInputBufferResult::Buffer(mut ib)) => {
                    let buf = ib.buffer_mut();
                    let n = pkt.data.len().min(buf.len());
                    // SAFETY: `buf` has at least `n` bytes; u8 and MaybeUninit<u8> share a layout
                    // and the regions cannot overlap.
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            pkt.data.as_ptr(),
                            buf.as_mut_ptr().cast::<u8>(),
                            n,
                        );
                    }
                    if let Err(e) = codec.queue_input_buffer(ib, 0, n, pkt.pts_us, 0) {
                        tracing::warn!("opus queue_input_buffer: {e}");
                        return;
                    }
                }
                Ok(DequeuedInputBufferResult::TryAgainLater) => {
                    tracing::warn!("opus decoder input busy, dropped a packet");
                }
                Err(e) => {
                    tracing::warn!("opus dequeue_input_buffer: {e}");
                    return;
                }
            },
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
        loop {
            match codec.dequeue_output_buffer(Duration::ZERO) {
                Ok(DequeuedOutputBufferInfoResult::Buffer(ob)) => {
                    let (offset, size) = (ob.info().offset().max(0) as usize, ob.info().size());
                    samples.clear();
                    let data = ob.buffer();
                    let end = (offset + size.max(0) as usize).min(data.len());
                    let bytes = data.get(offset..end).unwrap_or(&[]);
                    if float {
                        pcm_to_f32(bytes, SampleFormat::F32, &mut samples);
                    } else {
                        pcm_to_f32(bytes, SampleFormat::S16, &mut samples);
                    }
                    if let Err(e) = codec.release_output_buffer(ob, false) {
                        tracing::warn!("opus release_output_buffer: {e}");
                        return;
                    }
                    if !samples.is_empty() {
                        push(core, &samples);
                    }
                }
                Ok(DequeuedOutputBufferInfoResult::OutputFormatChanged) => {
                    let format = codec.output_format();
                    float = format.i32("pcm-encoding") == Some(ENCODING_PCM_FLOAT);
                    tracing::info!("opus output format changed: {format}");
                }
                Ok(DequeuedOutputBufferInfoResult::OutputBuffersChanged) => {}
                Ok(DequeuedOutputBufferInfoResult::TryAgainLater) => break,
                Err(e) => {
                    tracing::warn!("opus dequeue_output_buffer: {e}");
                    return;
                }
            }
        }
    }
}

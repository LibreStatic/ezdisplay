//! Pure parts of the audio path: the jitter buffer, PCM decoding and the Opus codec-specific data.
//! No Android types, so they build and test on the host.

use ezd_proto::SampleFormat;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

/// Output sample rate; the host sends 48 kHz.
pub const SAMPLE_RATE: u32 = 48_000;
/// Playback starts once this much audio is buffered (after the start or an underrun).
pub const TARGET_MS: u32 = 20;
/// Above this much buffered audio the oldest samples are dropped down to [`TARGET_MS`].
pub const MAX_MS: u32 = 60;
/// Opus seek pre-roll that MediaCodec expects in `csd-2`.
pub const OPUS_SEEK_PREROLL_NS: u64 = 80_000_000;

/// Counters shared between the packet path, the audio callback and the final log line.
#[derive(Default)]
pub struct AudioStats {
    /// Packets received from the host.
    pub packets: AtomicU64,
    /// Times the output ran dry while playing.
    pub underruns: AtomicU64,
    /// Sample frames (one sample per channel) dropped because the buffer grew too long.
    pub dropped_frames: AtomicU64,
}

impl AudioStats {
    /// `(packets, underruns, dropped frames)`.
    pub fn snapshot(&self) -> (u64, u64, u64) {
        (
            self.packets.load(Ordering::Relaxed),
            self.underruns.load(Ordering::Relaxed),
            self.dropped_frames.load(Ordering::Relaxed),
        )
    }
}

/// Interleaved f32 FIFO with a prebuffer and a latency cap.
pub struct Jitter {
    channels: usize,
    buf: VecDeque<f32>,
    target: usize,
    max: usize,
    playing: bool,
}

impl Jitter {
    /// Buffer for `channels` interleaved channels at `rate` Hz.
    pub fn new(rate: u32, channels: usize) -> Self {
        let channels = channels.max(1);
        let samples = |ms: u32| (rate as usize * ms as usize / 1000) * channels;
        Jitter {
            channels,
            buf: VecDeque::with_capacity(samples(MAX_MS) * 2),
            target: samples(TARGET_MS),
            max: samples(MAX_MS),
            playing: false,
        }
    }

    /// Buffered samples (all channels).
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// True when nothing is buffered.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// True while audio is being handed out (not prebuffering).
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Appends `samples` (a whole number of frames). When more than [`MAX_MS`] is buffered the
    /// oldest audio is dropped down to [`TARGET_MS`]. Returns the sample frames dropped.
    pub fn push(&mut self, samples: &[f32]) -> usize {
        self.buf.extend(samples.iter().copied());
        if self.buf.len() <= self.max {
            return 0;
        }
        let drop = (self.buf.len() - self.target) / self.channels * self.channels;
        self.buf.drain(..drop);
        drop / self.channels
    }

    /// Fills `out` with buffered audio. While prebuffering, or for the part that is missing on an
    /// underrun, it writes silence. Returns true when an underrun happened (the buffer ran dry
    /// while playing); playback then waits for the prebuffer again.
    pub fn pull(&mut self, out: &mut [f32]) -> bool {
        if !self.playing {
            if self.buf.len() < self.target {
                out.fill(0.0);
                return false;
            }
            self.playing = true;
        }
        let n = out.len().min(self.buf.len());
        for (o, s) in out.iter_mut().zip(self.buf.drain(..n)) {
            *o = s;
        }
        if n < out.len() {
            out[n..].fill(0.0);
            self.playing = false;
            return true;
        }
        false
    }

    /// Forgets everything and prebuffers again.
    pub fn reset(&mut self) {
        self.buf.clear();
        self.playing = false;
    }
}

/// Decodes little-endian interleaved PCM into f32, appending to `out`. A trailing partial
/// sample is ignored.
pub fn pcm_to_f32(data: &[u8], format: SampleFormat, out: &mut Vec<f32>) {
    match format {
        SampleFormat::S16 => out.extend(
            data.chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0),
        ),
        SampleFormat::F32 => out.extend(
            data.chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        ),
    }
}

/// f32 to i16 with clipping.
pub fn f32_to_i16(v: f32) -> i16 {
    (v * 32768.0).round().clamp(-32768.0, 32767.0) as i16
}

/// Pre-skip (samples at 48 kHz) of an OpusHead header, or `None` when it is not one.
pub fn opus_pre_skip(head: &[u8]) -> Option<u16> {
    if head.len() < 19 || &head[..8] != b"OpusHead" {
        return None;
    }
    Some(u16::from_le_bytes([head[10], head[11]]))
}

/// Codec delay for `csd-1`: the pre-skip in nanoseconds, 8 bytes little-endian.
pub fn opus_csd1(pre_skip: u16) -> [u8; 8] {
    (pre_skip as u64 * 1_000_000_000 / SAMPLE_RATE as u64).to_le_bytes()
}

/// Seek pre-roll for `csd-2`: 80 ms in nanoseconds, 8 bytes little-endian.
pub fn opus_csd2() -> [u8; 8] {
    OPUS_SEEK_PREROLL_NS.to_le_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10 ms of stereo.
    fn ms(n: usize, v: f32) -> Vec<f32> {
        vec![v; 48 * n * 2]
    }

    #[test]
    fn waits_for_the_prebuffer() {
        let mut j = Jitter::new(48_000, 2);
        j.push(&ms(10, 0.5));
        let mut out = vec![1.0; 96];
        assert!(!j.pull(&mut out));
        assert!(out.iter().all(|&s| s == 0.0));
        assert!(!j.is_playing());
        // The silence consumed nothing.
        assert_eq!(j.len(), 960);
        j.push(&ms(10, 0.5));
        assert!(!j.pull(&mut out));
        assert!(out.iter().all(|&s| s == 0.5));
        assert!(j.is_playing());
        assert_eq!(j.len(), 1920 - 96);
    }

    #[test]
    fn drops_the_oldest_above_the_cap() {
        let mut j = Jitter::new(48_000, 2);
        j.push(&ms(30, 0.1));
        j.push(&ms(20, 0.2));
        assert_eq!(j.len(), 48 * 50 * 2);
        // 70 ms in total: down to 20 ms, keeping the newest.
        let dropped = j.push(&ms(20, 0.3));
        assert_eq!(dropped, 48 * 50);
        assert_eq!(j.len(), 48 * 20 * 2);
        let mut out = vec![0.0; 48 * 20 * 2];
        j.pull(&mut out);
        assert!(out.iter().all(|&s| s == 0.3));
    }

    #[test]
    fn exactly_the_cap_is_kept() {
        let mut j = Jitter::new(48_000, 2);
        assert_eq!(j.push(&ms(60, 0.1)), 0);
        assert_eq!(j.len(), 48 * 60 * 2);
    }

    #[test]
    fn underrun_gives_silence_and_prebuffers_again() {
        let mut j = Jitter::new(48_000, 2);
        j.push(&ms(20, 0.5));
        let mut out = vec![0.0; 48 * 30 * 2];
        assert!(j.pull(&mut out), "ran dry while playing");
        assert!(out[..48 * 20 * 2].iter().all(|&s| s == 0.5));
        assert!(out[48 * 20 * 2..].iter().all(|&s| s == 0.0));
        assert!(!j.is_playing());
        // Less than the target arrives: still silent.
        j.push(&ms(10, 0.7));
        let mut out = vec![1.0; 96];
        assert!(!j.pull(&mut out));
        assert!(out.iter().all(|&s| s == 0.0));
        j.push(&ms(10, 0.7));
        assert!(!j.pull(&mut out));
        assert!(out.iter().all(|&s| s == 0.7));
    }

    #[test]
    fn drops_whole_frames_for_odd_channel_counts() {
        let mut j = Jitter::new(48_000, 3);
        j.push(&vec![0.0; 48 * 59 * 3]);
        let dropped = j.push(&vec![0.0; 48 * 5 * 3]);
        assert_eq!(j.len() % 3, 0);
        assert_eq!(dropped * 3 + j.len(), 48 * 64 * 3);
    }

    #[test]
    fn decodes_s16() {
        let mut out = Vec::new();
        let data: Vec<u8> = [0i16, 16384, -32768, 32767]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        pcm_to_f32(&data, SampleFormat::S16, &mut out);
        assert_eq!(out, vec![0.0, 0.5, -1.0, 32767.0 / 32768.0]);
    }

    #[test]
    fn decodes_f32_and_ignores_a_partial_tail() {
        let mut out = Vec::new();
        let mut data: Vec<u8> = [0.25f32, -0.75]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        data.push(0xAA);
        pcm_to_f32(&data, SampleFormat::F32, &mut out);
        assert_eq!(out, vec![0.25, -0.75]);
    }

    #[test]
    fn converts_f32_to_i16_with_clipping() {
        assert_eq!(f32_to_i16(0.0), 0);
        assert_eq!(f32_to_i16(0.5), 16384);
        assert_eq!(f32_to_i16(-1.0), -32768);
        assert_eq!(f32_to_i16(2.0), 32767);
        assert_eq!(f32_to_i16(-2.0), -32768);
    }

    #[test]
    fn builds_the_opus_csd() {
        let mut head = b"OpusHead".to_vec();
        head.extend([1, 2]); // version, channels
        head.extend(312u16.to_le_bytes()); // pre-skip
        head.extend(48_000u32.to_le_bytes());
        head.extend([0, 0, 0]); // gain, mapping family
        assert_eq!(opus_pre_skip(&head), Some(312));
        // 312 samples at 48 kHz = 6.5 ms.
        assert_eq!(u64::from_le_bytes(opus_csd1(312)), 6_500_000);
        assert_eq!(u64::from_le_bytes(opus_csd2()), 80_000_000);
        assert_eq!(opus_pre_skip(b"OpusTags"), None);
        assert_eq!(opus_pre_skip(&head[..10]), None);
    }
}

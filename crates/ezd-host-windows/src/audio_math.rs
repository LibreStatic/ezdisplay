//! Pure logic of the WASAPI loopback capture (`win/audio.rs`): the shared-mode format request,
//! frame/sample math, the silent-packet flag and the packet timestamp. Platform independent and
//! unit-tested everywhere.

/// Sample rate of the audio stream (fixed by the protocol).
pub const SAMPLE_RATE: u32 = 48_000;
/// Requested WASAPI buffer length (about two 10 ms Opus frames).
pub const BUFFER_MS: u32 = 20;
/// `AUDCLNT_BUFFERFLAGS_SILENT`: the packet content must be treated as zeros.
pub const BUFFERFLAG_SILENT: u32 = 0x2;
/// `WAVE_FORMAT_EXTENSIBLE`.
pub const FORMAT_TAG_EXTENSIBLE: u16 = 0xFFFE;
/// `WAVE_FORMAT_IEEE_FLOAT` sub-format tag (the first field of `KSDATAFORMAT_SUBTYPE_IEEE_FLOAT`).
pub const SUBFORMAT_IEEE_FLOAT: u32 = 0x0000_0003;

/// The fields of the `WAVEFORMATEXTENSIBLE` the capture asks WASAPI for: 32-bit IEEE float,
/// 48 kHz, `channels` interleaved channels. The Windows audio engine converts and resamples the
/// mix to it (`AUTOCONVERTPCM`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaveFormat {
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    pub block_align: u16,
    pub avg_bytes_per_sec: u32,
    pub channel_mask: u32,
}

/// Format for `channels` interleaved float channels at 48 kHz.
pub fn wave_format(channels: u8) -> WaveFormat {
    let channels = u16::from(channels.max(1));
    let block_align = channels * 4;
    WaveFormat {
        channels,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 32,
        block_align,
        avg_bytes_per_sec: SAMPLE_RATE * u32::from(block_align),
        channel_mask: channel_mask(channels),
    }
}

/// Speaker positions (`SPEAKER_*` bits) for the usual layouts; 0 (unspecified) for the others.
pub fn channel_mask(channels: u16) -> u32 {
    match channels {
        1 => 0x4,   // front center
        2 => 0x3,   // front left + right
        4 => 0x33,  // quad
        6 => 0x3F,  // 5.1
        8 => 0x63F, // 7.1
        _ => 0,
    }
}

/// Requested buffer duration in 100 ns units.
pub fn buffer_duration_hns() -> i64 {
    i64::from(BUFFER_MS) * 10_000
}

/// Duration of `frames` frames in microseconds.
pub fn frames_to_us(frames: u32) -> u64 {
    u64::from(frames) * 1_000_000 / u64::from(SAMPLE_RATE)
}

/// Timestamp of the first frame of a packet of `frames` frames that was read at `read_us`
/// (`now_us()` units). Packets are read right after the engine produced them, so the last frame
/// is about "now" and the first one is one packet duration earlier. (The packet's QPC position
/// is not used: it is on another clock than `now_us()` and loopback reports it unreliably.)
pub fn packet_pts_us(read_us: u64, frames: u32) -> u64 {
    read_us.saturating_sub(frames_to_us(frames))
}

/// Interleaved samples of one packet: zeros when it is flagged silent (its buffer content is
/// undefined), else the little-endian `f32` values of `raw` (a trailing partial sample is ignored).
pub fn packet_samples(raw: &[u8], frames: u32, channels: u16, flags: u32) -> Vec<f32> {
    let count = frames as usize * usize::from(channels);
    if flags & BUFFERFLAG_SILENT != 0 {
        return vec![0.0; count];
    }
    raw.chunks_exact(4)
        .take(count)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_format() {
        let f = wave_format(2);
        assert_eq!(f.channels, 2);
        assert_eq!(f.sample_rate, 48_000);
        assert_eq!(f.bits_per_sample, 32);
        assert_eq!(f.block_align, 8);
        assert_eq!(f.avg_bytes_per_sec, 384_000);
        assert_eq!(f.channel_mask, 0x3);
    }

    #[test]
    fn mono_and_zero_channels() {
        assert_eq!(wave_format(1).channel_mask, 0x4);
        assert_eq!(wave_format(0).channels, 1);
        assert_eq!(wave_format(3).channel_mask, 0);
    }

    #[test]
    fn buffer_is_20_ms() {
        assert_eq!(buffer_duration_hns(), 200_000);
    }

    #[test]
    fn frame_durations() {
        assert_eq!(frames_to_us(480), 10_000);
        assert_eq!(frames_to_us(0), 0);
    }

    #[test]
    fn pts_is_one_packet_before_the_read() {
        assert_eq!(packet_pts_us(50_000, 480), 40_000);
        assert_eq!(packet_pts_us(5_000, 480), 0);
    }

    #[test]
    fn silent_packets_are_zeros_even_with_garbage() {
        let raw = [0xFFu8; 16];
        let s = packet_samples(&raw, 2, 2, BUFFERFLAG_SILENT | 0x1);
        assert_eq!(s, vec![0.0; 4]);
    }

    #[test]
    fn data_packets_decode_floats() {
        let mut raw = Vec::new();
        for v in [0.5f32, -0.25, 1.0, 0.0] {
            raw.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(packet_samples(&raw, 2, 2, 0), vec![0.5, -0.25, 1.0, 0.0]);
        assert_eq!(packet_samples(&raw, 1, 2, 0), vec![0.5, -0.25]);
        assert_eq!(packet_samples(&raw[..6], 2, 2, 0), vec![0.5]);
    }
}

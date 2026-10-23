//! Synthetic raw frames for tests: every frame encodes its own number.
//!
//! The top-left `256x128` luma area holds a grid of `8x4` blocks of `32x32`
//! pixels, one per bit of the (32-bit) frame number, black (16) for 0 and white
//! (235) for 1. Large flat blocks survive lossy compression, so the number can be
//! read back from a decoded frame with [`read_frame_number`]. The rest of the
//! frame is a static diagonal gradient with a moving vertical bar so that
//! inter-frame prediction has something to do.

use std::fmt;

const BLOCK: usize = 32;
const COLS: usize = 8;
const ROWS: usize = 4;
/// Minimum width of a synthetic frame.
pub const MIN_WIDTH: u32 = (BLOCK * COLS) as u32;
/// Minimum height of a synthetic frame.
pub const MIN_HEIGHT: u32 = (BLOCK * ROWS) as u32;
const LOW: u8 = 16;
const HIGH: u8 = 235;
const BAR_W: usize = 64;

/// Raw pixel layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// Planar Y, U, V with 2x2 chroma subsampling.
    I420,
    /// Planar Y followed by interleaved UV.
    Nv12,
}

/// Invalid frame geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadSize;

impl fmt::Display for BadSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "synthetic frames need even dimensions of at least {MIN_WIDTH}x{MIN_HEIGHT}"
        )
    }
}
impl std::error::Error for BadSize {}

/// Generator of tightly packed raw frames.
#[derive(Debug, Clone)]
pub struct SyntheticFrames {
    width: u32,
    height: u32,
    format: PixelFormat,
    background: Vec<u8>,
}

impl SyntheticFrames {
    /// Creates a generator. Dimensions must be even and at least [`MIN_WIDTH`]x[`MIN_HEIGHT`].
    pub fn new(width: u32, height: u32, format: PixelFormat) -> Result<Self, BadSize> {
        if width < MIN_WIDTH || height < MIN_HEIGHT || width % 2 != 0 || height % 2 != 0 {
            return Err(BadSize);
        }
        let (w, h) = (width as usize, height as usize);
        let mut background = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                background[y * w + x] = 64 + (((x + y) >> 3) & 0x3f) as u8;
            }
        }
        Ok(SyntheticFrames {
            width,
            height,
            format,
            background,
        })
    }

    /// Frame width.
    pub fn width(&self) -> u32 {
        self.width
    }
    /// Frame height.
    pub fn height(&self) -> u32 {
        self.height
    }
    /// Pixel format.
    pub fn format(&self) -> PixelFormat {
        self.format
    }
    /// Size in bytes of one frame.
    pub fn frame_len(&self) -> usize {
        self.width as usize * self.height as usize * 3 / 2
    }

    /// Renders frame number `n` (only the low 32 bits are encoded) into `out`,
    /// resizing it to [`frame_len`](Self::frame_len). Planes are tightly packed
    /// (stride == width, chroma stride == width/2 for I420, width for NV12).
    pub fn render(&self, n: u64, out: &mut Vec<u8>) {
        let (w, h) = (self.width as usize, self.height as usize);
        out.clear();
        out.extend_from_slice(&self.background);
        let y_plane = &mut out[..w * h];
        // Moving bar below the number grid.
        let bar_x = (n as usize * 12) % (w + BAR_W);
        for y in (BLOCK * ROWS)..h {
            let row = &mut y_plane[y * w..(y + 1) * w];
            let start = bar_x.saturating_sub(BAR_W);
            let end = bar_x.min(w);
            if start < end {
                row[start..end].fill(HIGH);
            }
        }
        // Number grid.
        for bit in 0..32 {
            let v = if (n >> bit) & 1 == 1 { HIGH } else { LOW };
            let (bx, by) = ((bit % COLS) * BLOCK, (bit / COLS) * BLOCK);
            for y in by..by + BLOCK {
                y_plane[y * w + bx..y * w + bx + BLOCK].fill(v);
            }
        }
        out.resize(self.frame_len(), 128);
    }

    /// Convenience wrapper around [`render`](Self::render).
    pub fn frame(&self, n: u64) -> Vec<u8> {
        let mut v = Vec::new();
        self.render(n, &mut v);
        v
    }
}

/// Reads the (32-bit) frame number back from a luma plane with row `stride`.
///
/// Returns `None` if the plane is too small. Robust against lossy
/// compression: every block is classified by the mean of its central pixels.
pub fn read_frame_number(luma: &[u8], stride: usize, width: u32, height: u32) -> Option<u64> {
    if width < MIN_WIDTH || height < MIN_HEIGHT || stride < width as usize {
        return None;
    }
    if luma.len() < stride * (height as usize - 1) + width as usize {
        return None;
    }
    let margin = BLOCK / 4;
    let mut n = 0u64;
    for bit in 0..32 {
        let (bx, by) = ((bit % COLS) * BLOCK, (bit / COLS) * BLOCK);
        let mut sum = 0u64;
        let mut cnt = 0u64;
        for y in by + margin..by + BLOCK - margin {
            for x in bx + margin..bx + BLOCK - margin {
                sum += luma[y * stride + x] as u64;
                cnt += 1;
            }
        }
        if sum / cnt >= (LOW as u64 + HIGH as u64) / 2 {
            n |= 1 << bit;
        }
    }
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_sizes() {
        assert!(SyntheticFrames::new(255, 128, PixelFormat::I420).is_err());
        assert!(SyntheticFrames::new(256, 127, PixelFormat::I420).is_err());
        assert!(SyntheticFrames::new(257, 130, PixelFormat::I420).is_err());
        assert!(SyntheticFrames::new(256, 128, PixelFormat::Nv12).is_ok());
    }

    #[test]
    fn number_roundtrip() {
        for fmt in [PixelFormat::I420, PixelFormat::Nv12] {
            let g = SyntheticFrames::new(640, 360, fmt).unwrap();
            assert_eq!(g.frame(0).len(), 640 * 360 * 3 / 2);
            for n in [
                0u64,
                1,
                2,
                255,
                256,
                1000,
                123_456,
                0xdead_beef,
                u32::MAX as u64,
            ] {
                let f = g.frame(n);
                assert_eq!(read_frame_number(&f, 640, 640, 360), Some(n), "{fmt:?} {n}");
            }
            // Only the low 32 bits are encoded.
            assert_eq!(
                read_frame_number(&g.frame((1 << 32) + 5), 640, 640, 360),
                Some(5)
            );
        }
    }

    #[test]
    fn robust_to_noise_and_blur() {
        let g = SyntheticFrames::new(1280, 720, PixelFormat::I420).unwrap();
        let mut seed = 12345u32;
        for n in [7u64, 0x5555_5555, 0xaaaa_aaaa, 99_999] {
            let mut f = g.frame(n);
            for p in f[..1280 * 720].iter_mut() {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = ((seed >> 24) as i32 % 61) - 30; // +-30 luma
                *p = (*p as i32 + noise).clamp(0, 255) as u8;
            }
            assert_eq!(read_frame_number(&f, 1280, 1280, 720), Some(n));
        }
    }

    #[test]
    fn frames_differ_and_bar_moves() {
        let g = SyntheticFrames::new(640, 360, PixelFormat::I420).unwrap();
        assert_ne!(g.frame(1), g.frame(2));
        // Bar content differs even when the number bits are masked out.
        let (a, b) = (g.frame(0), g.frame(1));
        let below = 640 * 128;
        assert_ne!(a[below..640 * 360], b[below..640 * 360]);
    }

    #[test]
    fn stride_and_short_buffer() {
        let g = SyntheticFrames::new(256, 128, PixelFormat::I420).unwrap();
        let f = g.frame(42);
        // Re-pack luma with a larger stride.
        let mut padded = vec![0u8; 300 * 128];
        for y in 0..128 {
            padded[y * 300..y * 300 + 256].copy_from_slice(&f[y * 256..(y + 1) * 256]);
        }
        assert_eq!(read_frame_number(&padded, 300, 256, 128), Some(42));
        assert_eq!(read_frame_number(&f[..1000], 256, 256, 128), None);
        assert_eq!(read_frame_number(&f, 256, 100, 100), None);
    }
}

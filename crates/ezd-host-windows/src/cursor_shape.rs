//! Conversion of DXGI Desktop Duplication pointer shapes to straight RGBA8.
//!
//! `DXGI_OUTDUPL_POINTER_SHAPE_INFO` describes three layouts:
//!
//! * `MONOCHROME` (1): 1 bpp AND mask followed by a 1 bpp XOR mask; the reported
//!   `Height` covers both masks, the cursor is `Height / 2` tall. Per pixel
//!   `(AND, XOR)`: `(0,0)` black, `(0,1)` white, `(1,0)` transparent, `(1,1)`
//!   "invert the screen".
//! * `COLOR` (2): 32 bpp BGRA with straight alpha.
//! * `MASKED_COLOR` (4): 32 bpp BGRA where alpha is a mask: `0` means the pixel
//!   replaces the screen, `0xFF` means it is XORed with the screen.
//!
//! The tablet draws the shape over the video, so "invert" cannot be reproduced
//! exactly; those pixels become mid-gray (visible on light and dark content),
//! and XOR pixels of a masked-colour cursor keep their colour (a zero colour
//! XORed with anything is a no-op, so it becomes transparent).

/// `DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME`.
pub const SHAPE_MONOCHROME: u32 = 1;
/// `DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR`.
pub const SHAPE_COLOR: u32 = 2;
/// `DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR`.
pub const SHAPE_MASKED_COLOR: u32 = 4;

/// A converted shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Straight RGBA8, `width * height * 4` bytes.
    pub data: Vec<u8>,
}

/// Mid-gray used for "invert the screen" pixels.
const INVERT_GRAY: [u8; 4] = [128, 128, 128, 255];

/// Converts the buffer returned by `GetFramePointerShape`.
///
/// `width`/`height`/`pitch` are the values of `DXGI_OUTDUPL_POINTER_SHAPE_INFO`.
pub fn to_rgba(kind: u32, width: u32, height: u32, pitch: u32, buf: &[u8]) -> Result<Rgba, String> {
    if width == 0 || height == 0 || width > 512 || height > 1024 {
        return Err(format!("implausible pointer size {width}x{height}"));
    }
    match kind {
        SHAPE_MONOCHROME => {
            if height % 2 != 0 {
                return Err(format!("monochrome pointer height {height} is odd"));
            }
            let h = height / 2;
            let (pitch, w) = (pitch as usize, width as usize);
            if pitch < w.div_ceil(8) || buf.len() < pitch * height as usize {
                return Err(format!(
                    "monochrome pointer buffer too small: {} bytes for pitch {pitch}, height {height}",
                    buf.len()
                ));
            }
            let (and_mask, xor_mask) = buf.split_at(pitch * h as usize);
            let mut out = Vec::with_capacity(w * h as usize * 4);
            for y in 0..h as usize {
                for x in 0..w {
                    let bit = 0x80u8 >> (x % 8);
                    let a = and_mask[y * pitch + x / 8] & bit != 0;
                    let o = xor_mask[y * pitch + x / 8] & bit != 0;
                    out.extend_from_slice(&match (a, o) {
                        (false, false) => [0, 0, 0, 255],
                        (false, true) => [255, 255, 255, 255],
                        (true, false) => [0, 0, 0, 0],
                        (true, true) => INVERT_GRAY,
                    });
                }
            }
            Ok(Rgba {
                width,
                height: h,
                data: out,
            })
        }
        SHAPE_COLOR | SHAPE_MASKED_COLOR => {
            let (pitch, w, h) = (pitch as usize, width as usize, height as usize);
            if pitch < w * 4 || buf.len() < pitch * h {
                return Err(format!(
                    "color pointer buffer too small: {} bytes for pitch {pitch}, height {h}",
                    buf.len()
                ));
            }
            let mut out = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                for px in buf[y * pitch..y * pitch + w * 4].chunks_exact(4) {
                    let (b, g, r, a) = (px[0], px[1], px[2], px[3]);
                    out.extend_from_slice(&if kind == SHAPE_COLOR {
                        [r, g, b, a]
                    } else if a == 0 {
                        [r, g, b, 255]
                    } else if (r, g, b) == (0, 0, 0) {
                        [0, 0, 0, 0]
                    } else {
                        [r, g, b, 255]
                    });
                }
            }
            Ok(Rgba {
                width,
                height,
                data: out,
            })
        }
        other => Err(format!("unknown pointer shape type {other}")),
    }
}

/// Stable, non-zero shape id: FNV-1a over geometry and pixels, so the same
/// bitmap keeps its id across sessions and shape updates.
pub fn shape_id(shape: &Rgba, hot_x: u32, hot_y: u32) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    let mut feed = |bytes: &[u8]| {
        for &b in bytes {
            h ^= b as u32;
            h = h.wrapping_mul(0x0100_0193);
        }
    };
    feed(&shape.width.to_le_bytes());
    feed(&shape.height.to_le_bytes());
    feed(&hot_x.to_le_bytes());
    feed(&hot_y.to_le_bytes());
    feed(&shape.data);
    h.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monochrome_combinations() {
        // 4 px wide, cursor 1 row tall (reported height 2). AND row then XOR row.
        // pixels: (and,xor) = (0,0) (0,1) (1,0) (1,1)  -> AND bits 0011, XOR bits 0101
        let buf = [0b0011_0000, 0b0101_0000];
        let c = to_rgba(SHAPE_MONOCHROME, 4, 2, 1, &buf).unwrap();
        assert_eq!((c.width, c.height), (4, 1));
        assert_eq!(
            c.data,
            [
                0, 0, 0, 255, // black
                255, 255, 255, 255, // white
                0, 0, 0, 0, // transparent
                128, 128, 128, 255, // invert
            ]
        );
    }

    #[test]
    fn monochrome_respects_pitch_and_wide_rows() {
        // 10 px wide needs 2 bytes per row; pitch 4 (padding). Cursor 2 rows (height 4).
        let mut buf = vec![0xFFu8; 4 * 2]; // AND all 1 -> transparent unless XOR set
        buf.extend(vec![0u8; 4 * 2]); // XOR all 0
        buf[4] = 0x7F; // row 1, pixel 0: AND=0 XOR=0 -> black
        let c = to_rgba(SHAPE_MONOCHROME, 10, 4, 4, &buf).unwrap();
        assert_eq!((c.width, c.height, c.data.len()), (10, 2, 80));
        assert_eq!(&c.data[0..4], [0, 0, 0, 0]);
        assert_eq!(&c.data[40..44], [0, 0, 0, 255]);
        assert_eq!(&c.data[44..48], [0, 0, 0, 0]);
    }

    #[test]
    fn color_swaps_bgra_to_rgba_and_skips_row_padding() {
        // 2x2, pitch 12 (4 bytes padding per row).
        let mut buf = Vec::new();
        for y in 0..2u8 {
            buf.extend([10 + y, 20, 30, 200, 1, 2, 3, 4]); // B G R A, B G R A
            buf.extend([9, 9, 9, 9]);
        }
        let c = to_rgba(SHAPE_COLOR, 2, 2, 12, &buf).unwrap();
        assert_eq!(&c.data[0..8], [30, 20, 10, 200, 3, 2, 1, 4]);
        assert_eq!(&c.data[8..12], [30, 20, 11, 200]);
    }

    #[test]
    fn masked_color_maps_alpha_mask() {
        let buf = [
            1, 2, 3, 0x00, // replace: opaque (r=3,g=2,b=1)
            255, 255, 255, 0xFF, // xor white: invert-ish, keep white
            0, 0, 0, 0xFF, // xor black: no-op -> transparent
            9, 8, 7, 0x00, //
        ];
        let c = to_rgba(SHAPE_MASKED_COLOR, 4, 1, 16, &buf).unwrap();
        assert_eq!(
            c.data,
            [
                3, 2, 1, 255, //
                255, 255, 255, 255, //
                0, 0, 0, 0, //
                7, 8, 9, 255
            ]
        );
    }

    #[test]
    fn rejects_bad_input() {
        assert!(to_rgba(SHAPE_COLOR, 2, 2, 8, &[0; 4]).is_err());
        assert!(
            to_rgba(SHAPE_COLOR, 2, 2, 4, &[0; 16]).is_err(),
            "pitch too small"
        );
        assert!(
            to_rgba(SHAPE_MONOCHROME, 8, 3, 1, &[0; 3]).is_err(),
            "odd height"
        );
        assert!(to_rgba(3, 2, 2, 8, &[0; 16]).is_err());
        assert!(to_rgba(SHAPE_COLOR, 0, 2, 8, &[0; 16]).is_err());
        assert!(to_rgba(SHAPE_COLOR, 100_000, 2, 8, &[0; 16]).is_err());
    }

    #[test]
    fn shape_id_is_stable_nonzero_and_content_sensitive() {
        let a = Rgba {
            width: 1,
            height: 1,
            data: vec![1, 2, 3, 4],
        };
        let mut b = a.clone();
        b.data[0] = 9;
        assert_eq!(shape_id(&a, 0, 0), shape_id(&a.clone(), 0, 0));
        assert_ne!(shape_id(&a, 0, 0), shape_id(&b, 0, 0));
        assert_ne!(shape_id(&a, 0, 0), shape_id(&a, 1, 0));
        assert_ne!(shape_id(&a, 0, 0), 0);
    }
}

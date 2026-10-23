//! Cursor bitmap handling for `SPA_META_Cursor`: pixel format conversion to straight RGBA and a
//! stable shape id.

use ezd_proto::CursorShape;

/// Largest cursor edge we forward.
pub const MAX_CURSOR_EDGE: u32 = 512;

/// Byte order of a 32-bit cursor pixel in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelOrder {
    /// `R G B A`
    Rgba,
    /// `B G R A` (little-endian ARGB32)
    Bgra,
    /// `A R G B`
    Argb,
    /// `A B G R`
    Abgr,
    /// `R G B x` (alpha ignored: opaque)
    Rgbx,
    /// `B G R x` (alpha ignored: opaque)
    Bgrx,
}

impl PixelOrder {
    /// Index of (r, g, b, a) inside one pixel; `None` for alpha means opaque.
    fn indices(self) -> (usize, usize, usize, Option<usize>) {
        match self {
            PixelOrder::Rgba => (0, 1, 2, Some(3)),
            PixelOrder::Bgra => (2, 1, 0, Some(3)),
            PixelOrder::Argb => (1, 2, 3, Some(0)),
            PixelOrder::Abgr => (3, 2, 1, Some(0)),
            PixelOrder::Rgbx => (0, 1, 2, None),
            PixelOrder::Bgrx => (2, 1, 0, None),
        }
    }
}

/// Converts `size` pixels of `src` (rows `stride` bytes apart) to tightly packed straight RGBA.
///
/// Compositors disagree on whether the cursor bitmap is premultiplied. Pixels are treated as
/// premultiplied when no color channel exceeds its alpha anywhere and at least one pixel is
/// semi-transparent (a straight bitmap with such colors only loses a little brightness).
///
/// Returns `None` for empty/oversized cursors or when `src` is too short.
pub fn to_straight_rgba(
    src: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    order: PixelOrder,
) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || width > MAX_CURSOR_EDGE || height > MAX_CURSOR_EDGE {
        return None;
    }
    let (w, h) = (width as usize, height as usize);
    if stride < w * 4 || src.len() < stride * (h - 1) + w * 4 {
        return None;
    }
    let (ri, gi, bi, ai) = order.indices();
    let mut out = vec![0u8; w * h * 4];
    let mut premultiplied_possible = true;
    let mut semi_transparent = false;
    for y in 0..h {
        let row = &src[y * stride..y * stride + w * 4];
        let dst = &mut out[y * w * 4..(y + 1) * w * 4];
        for (p, d) in row.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
            let a = ai.map_or(255, |i| p[i]);
            let (r, g, b) = (p[ri], p[gi], p[bi]);
            if r > a || g > a || b > a {
                premultiplied_possible = false;
            }
            if a != 0 && a != 255 {
                semi_transparent = true;
            }
            d.copy_from_slice(&[r, g, b, a]);
        }
    }
    if premultiplied_possible && semi_transparent {
        for d in out.chunks_exact_mut(4) {
            let a = d[3] as u32;
            if a != 0 && a != 255 {
                for c in &mut d[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
    }
    Some(out)
}

/// Stable id of a cursor image: FNV-1a over size, hotspot and pixels, folded to 32 bits.
/// Never 0, so 0 can mean "no shape yet".
pub fn shape_id(width: u16, height: u16, hot_x: u16, hot_y: u16, rgba: &[u8]) -> u32 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    feed(&width.to_le_bytes());
    feed(&height.to_le_bytes());
    feed(&hot_x.to_le_bytes());
    feed(&hot_y.to_le_bytes());
    feed(rgba);
    let id = (h ^ (h >> 32)) as u32;
    if id == 0 { 1 } else { id }
}

/// Builds the wire shape from a source bitmap. Hotspot coordinates are clamped to the bitmap.
pub fn build_shape(
    src: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    order: PixelOrder,
    hot_x: i32,
    hot_y: i32,
) -> Option<CursorShape> {
    let rgba = to_straight_rgba(src, width, height, stride, order)?;
    let (w, h) = (width as u16, height as u16);
    let hx = hot_x.clamp(0, w as i32 - 1) as u16;
    let hy = hot_y.clamp(0, h as i32 - 1) as u16;
    Some(CursorShape {
        shape_id: shape_id(w, h, hx, hy, &rgba),
        width: w,
        height: h,
        hot_x: hx,
        hot_y: hy,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swizzles_bgra_and_respects_stride() {
        // 2x1 pixels, stride 12 (4 bytes padding).
        let src = [
            10, 20, 30, 255, 40, 50, 60, 255, 0xAA, 0xAA, 0xAA, 0xAA, //
        ];
        let out = to_straight_rgba(&src, 2, 1, 12, PixelOrder::Bgra).unwrap();
        assert_eq!(out, [30, 20, 10, 255, 60, 50, 40, 255]);
        let out = to_straight_rgba(&src[..8], 2, 1, 8, PixelOrder::Argb).unwrap();
        assert_eq!(out, [20, 30, 255, 10, 50, 60, 255, 40]);
    }

    #[test]
    fn multi_row_with_stride() {
        let mut src = vec![0u8; 16 * 2];
        src[0..4].copy_from_slice(&[1, 2, 3, 255]);
        src[16..20].copy_from_slice(&[5, 6, 7, 255]);
        let out = to_straight_rgba(&src, 1, 2, 16, PixelOrder::Rgba).unwrap();
        assert_eq!(out, [1, 2, 3, 255, 5, 6, 7, 255]);
    }

    #[test]
    fn x_formats_are_opaque() {
        let out = to_straight_rgba(&[1, 2, 3, 0], 1, 1, 4, PixelOrder::Bgrx).unwrap();
        assert_eq!(out, [3, 2, 1, 255]);
    }

    #[test]
    fn unpremultiplies_when_consistent() {
        // White at 50% alpha, premultiplied: (128,128,128,128) -> (255,255,255,128)
        let out = to_straight_rgba(&[128, 128, 128, 128], 1, 1, 4, PixelOrder::Rgba).unwrap();
        assert_eq!(out, [255, 255, 255, 128]);
        // Straight bitmap (color above alpha): untouched.
        let out = to_straight_rgba(&[255, 255, 255, 128], 1, 1, 4, PixelOrder::Rgba).unwrap();
        assert_eq!(out, [255, 255, 255, 128]);
        // Fully opaque / transparent only: untouched.
        let out = to_straight_rgba(&[9, 8, 7, 255, 0, 0, 0, 0], 2, 1, 8, PixelOrder::Rgba).unwrap();
        assert_eq!(out, [9, 8, 7, 255, 0, 0, 0, 0]);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(to_straight_rgba(&[0; 4], 0, 1, 4, PixelOrder::Rgba).is_none());
        assert!(to_straight_rgba(&[0; 4], 1, 1, 3, PixelOrder::Rgba).is_none());
        assert!(to_straight_rgba(&[0; 7], 2, 1, 8, PixelOrder::Rgba).is_none());
        assert!(
            to_straight_rgba(&vec![0; 4 * 600 * 600], 600, 600, 2400, PixelOrder::Rgba).is_none()
        );
    }

    #[test]
    fn shape_id_is_stable_and_discriminating() {
        let a = vec![1u8; 16];
        let id = shape_id(2, 2, 0, 0, &a);
        assert_eq!(id, shape_id(2, 2, 0, 0, &a));
        assert_ne!(id, shape_id(2, 2, 1, 0, &a), "hotspot");
        assert_ne!(id, shape_id(4, 1, 0, 0, &a), "size");
        let mut b = a.clone();
        b[7] = 2;
        assert_ne!(id, shape_id(2, 2, 0, 0, &b), "pixels");
        assert_ne!(id, 0);
        // Pin the hash so ids stay stable across releases.
        assert_eq!(shape_id(1, 1, 0, 0, &[0, 0, 0, 0]), 0x325b_0010);
    }

    #[test]
    fn build_shape_clamps_hotspot() {
        let src = vec![255u8; 4 * 4 * 4];
        let s = build_shape(&src, 4, 4, 16, PixelOrder::Rgba, 99, -3).unwrap();
        assert_eq!((s.width, s.height, s.hot_x, s.hot_y), (4, 4, 3, 0));
        assert_eq!(s.rgba.len(), 64);
    }
}

//! `SPA_META_Cursor` decoding and the state machine that turns cursor metadata into
//! [`CursorShape`] / [`CursorState`] updates.

use crate::cursor::build_shape;
use crate::frame::pixel_order;
use ezd_proto::CursorShape;
use std::collections::HashMap;

/// Size of `struct spa_meta_cursor`.
pub const META_CURSOR_SIZE: usize = 28;
/// Size of `struct spa_meta_bitmap`.
pub const META_BITMAP_SIZE: usize = 20;

/// Decoded `spa_meta_bitmap` with its pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap<'a> {
    /// `SPA_VIDEO_FORMAT_*`.
    pub format: u32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Row stride in bytes.
    pub stride: i32,
    /// Bytes from the first pixel to the end of the meta block.
    pub pixels: &'a [u8],
}

/// Decoded `spa_meta_cursor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorMeta<'a> {
    /// Cursor id; 0 means "no cursor".
    pub id: u32,
    /// Position of the hotspot in stream pixels.
    pub x: i32,
    /// Position of the hotspot in stream pixels.
    pub y: i32,
    /// Hotspot offset inside the bitmap.
    pub hot_x: i32,
    /// Hotspot offset inside the bitmap.
    pub hot_y: i32,
    /// Bitmap, when the compositor attached one (shape change).
    pub bitmap: Option<Bitmap<'a>>,
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

fn i32_at(b: &[u8], off: usize) -> Option<i32> {
    u32_at(b, off).map(|v| v as i32)
}

/// Parses the bytes of one cursor meta block.
pub fn parse_cursor_meta(meta: &[u8]) -> Option<CursorMeta<'_>> {
    if meta.len() < META_CURSOR_SIZE {
        return None;
    }
    let bitmap_offset = u32_at(meta, 24)? as usize;
    let bitmap =
        if bitmap_offset >= META_CURSOR_SIZE && bitmap_offset + META_BITMAP_SIZE <= meta.len() {
            let b = &meta[bitmap_offset..];
            let data_off = u32_at(b, 16)? as usize;
            let pixels = b.get(data_off..).unwrap_or(&[]);
            Some(Bitmap {
                format: u32_at(b, 0)?,
                width: u32_at(b, 4)?,
                height: u32_at(b, 8)?,
                stride: i32_at(b, 12)?,
                pixels,
            })
        } else {
            None
        };
    Some(CursorMeta {
        id: u32_at(meta, 0)?,
        x: i32_at(meta, 8)?,
        y: i32_at(meta, 12)?,
        hot_x: i32_at(meta, 16)?,
        hot_y: i32_at(meta, 20)?,
        bitmap,
    })
}

/// Cursor position and visibility without a timestamp (the sender adds `ts_us`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateUpdate {
    /// X in stream pixels.
    pub x: i32,
    /// Y in stream pixels.
    pub y: i32,
    /// Visibility.
    pub visible: bool,
    /// Shape id.
    pub shape_id: u32,
}

/// What to publish after one metadata block.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CursorEvents {
    /// New shape to send before the state.
    pub shape: Option<CursorShape>,
    /// New state, only when it differs from the last one published.
    pub state: Option<StateUpdate>,
}

/// Tracks the cursor between metadata blocks (bitmaps are only attached on shape changes).
#[derive(Debug, Default)]
pub struct CursorTracker {
    /// compositor cursor id -> shape id (0 = empty bitmap, i.e. hidden).
    shapes: HashMap<u32, u32>,
    current_shape: u32,
    last: Option<StateUpdate>,
}

impl CursorTracker {
    /// Feeds one metadata block.
    pub fn update(&mut self, meta: &CursorMeta<'_>) -> CursorEvents {
        let mut ev = CursorEvents::default();
        if meta.id != 0 {
            if let Some(bm) = &meta.bitmap {
                let shape = if bm.width == 0 || bm.height == 0 {
                    None
                } else {
                    pixel_order(bm.format).and_then(|order| {
                        build_shape(
                            bm.pixels,
                            bm.width,
                            bm.height,
                            bm.stride.max(0) as usize,
                            order,
                            meta.hot_x,
                            meta.hot_y,
                        )
                    })
                };
                match shape {
                    Some(s) => {
                        self.shapes.insert(meta.id, s.shape_id);
                        self.current_shape = s.shape_id;
                        ev.shape = Some(s);
                    }
                    None => {
                        self.shapes.insert(meta.id, 0);
                        self.current_shape = 0;
                    }
                }
            } else if let Some(&s) = self.shapes.get(&meta.id) {
                self.current_shape = s;
            }
        }
        let st = StateUpdate {
            x: meta.x,
            y: meta.y,
            visible: meta.id != 0 && self.current_shape != 0,
            shape_id: self.current_shape,
        };
        if self.last != Some(st) {
            self.last = Some(st);
            ev.state = Some(st);
        }
        ev
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::spa_fmt;

    /// Builds a meta block: cursor header, optional bitmap with `pixels`.
    fn block(
        id: u32,
        pos: (i32, i32),
        hot: (i32, i32),
        bitmap: Option<(u32, u32, u32, &[u8])>,
    ) -> Vec<u8> {
        let mut m = Vec::new();
        for v in [
            id,
            0,
            pos.0 as u32,
            pos.1 as u32,
            hot.0 as u32,
            hot.1 as u32,
        ] {
            m.extend_from_slice(&v.to_ne_bytes());
        }
        let off = if bitmap.is_some() {
            META_CURSOR_SIZE as u32
        } else {
            0
        };
        m.extend_from_slice(&off.to_ne_bytes());
        if let Some((fmt, w, h, px)) = bitmap {
            for v in [fmt, w, h, (w * 4), META_BITMAP_SIZE as u32] {
                m.extend_from_slice(&v.to_ne_bytes());
            }
            m.extend_from_slice(px);
        }
        m
    }

    #[test]
    fn layout_matches_spa() {
        assert_eq!(
            META_CURSOR_SIZE,
            std::mem::size_of::<pipewire::spa::sys::spa_meta_cursor>()
        );
        assert_eq!(
            META_BITMAP_SIZE,
            std::mem::size_of::<pipewire::spa::sys::spa_meta_bitmap>()
        );
    }

    #[test]
    fn parses_blocks() {
        let px = [1u8, 2, 3, 255, 4, 5, 6, 255];
        let b = block(7, (10, -4), (1, 2), Some((spa_fmt::BGRA, 2, 1, &px)));
        let m = parse_cursor_meta(&b).unwrap();
        assert_eq!((m.id, m.x, m.y, m.hot_x, m.hot_y), (7, 10, -4, 1, 2));
        let bm = m.bitmap.unwrap();
        assert_eq!(
            (bm.format, bm.width, bm.height, bm.stride),
            (spa_fmt::BGRA, 2, 1, 8)
        );
        assert_eq!(bm.pixels, px);

        let b = block(7, (1, 1), (0, 0), None);
        assert!(parse_cursor_meta(&b).unwrap().bitmap.is_none());
        assert!(parse_cursor_meta(&b[..10]).is_none());
        // Bitmap offset pointing outside the block is ignored.
        let mut bad = block(7, (1, 1), (0, 0), None);
        bad[24..28].copy_from_slice(&1000u32.to_ne_bytes());
        assert!(parse_cursor_meta(&bad).unwrap().bitmap.is_none());
    }

    #[test]
    fn tracker_shape_then_move_then_reuse() {
        let px = vec![200u8; 4 * 4 * 4];
        let mut t = CursorTracker::default();

        let b = block(3, (5, 6), (0, 0), Some((spa_fmt::BGRA, 4, 4, &px)));
        let ev = t.update(&parse_cursor_meta(&b).unwrap());
        let shape = ev.shape.expect("new shape");
        let st = ev.state.unwrap();
        assert!(st.visible && st.shape_id == shape.shape_id);
        assert_eq!((st.x, st.y), (5, 6));

        // Pure move, no bitmap: no shape, new state.
        let b = block(3, (9, 6), (0, 0), None);
        let ev = t.update(&parse_cursor_meta(&b).unwrap());
        assert!(ev.shape.is_none());
        assert_eq!(ev.state.unwrap().x, 9);

        // Identical state is not republished.
        assert_eq!(
            t.update(&parse_cursor_meta(&b).unwrap()),
            CursorEvents::default()
        );
    }

    #[test]
    fn tracker_hides_and_restores() {
        let px = vec![200u8; 2 * 2 * 4];
        let mut t = CursorTracker::default();
        let b = block(1, (0, 0), (0, 0), Some((spa_fmt::RGBA, 2, 2, &px)));
        t.update(&parse_cursor_meta(&b).unwrap());
        // id 0: no cursor.
        let b = block(0, (0, 0), (0, 0), None);
        assert!(
            !t.update(&parse_cursor_meta(&b).unwrap())
                .state
                .unwrap()
                .visible
        );
        // Back with the old id and no bitmap: visible again with the old shape, no resend.
        let b = block(1, (3, 3), (0, 0), None);
        let ev = t.update(&parse_cursor_meta(&b).unwrap());
        assert!(ev.shape.is_none() && ev.state.unwrap().visible);
        // Empty bitmap hides.
        let b = block(2, (3, 3), (0, 0), Some((spa_fmt::RGBA, 0, 0, &[])));
        assert!(
            !t.update(&parse_cursor_meta(&b).unwrap())
                .state
                .unwrap()
                .visible
        );
    }

    #[test]
    fn unknown_id_without_bitmap_is_invisible_until_shape_arrives() {
        let mut t = CursorTracker::default();
        let b = block(9, (1, 2), (0, 0), None);
        let ev = t.update(&parse_cursor_meta(&b).unwrap());
        assert!(!ev.state.unwrap().visible);
    }

    #[test]
    fn same_pixels_give_same_shape_id() {
        let px = vec![10u8; 3 * 3 * 4];
        let mut a = CursorTracker::default();
        let mut b = CursorTracker::default();
        let m1 = block(1, (0, 0), (1, 1), Some((spa_fmt::BGRA, 3, 3, &px)));
        let m2 = block(55, (4, 4), (1, 1), Some((spa_fmt::BGRA, 3, 3, &px)));
        let s1 = a.update(&parse_cursor_meta(&m1).unwrap()).shape.unwrap();
        let s2 = b.update(&parse_cursor_meta(&m2).unwrap()).shape.unwrap();
        assert_eq!(s1.shape_id, s2.shape_id);
    }
}

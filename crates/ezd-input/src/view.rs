//! Local zoom view transform.

use ezd_proto::normalize;

/// Local zoom of the view: `screen = scale * content + t`, pivot at (0, 0).
///
/// Kotlin applies `scaleX = scaleY = scale`, `translationX = tx`,
/// `translationY = ty` to the video surface and the cursor layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewTransform {
    /// Zoom factor, 1..=4.
    pub scale: f32,
    /// Horizontal translation in view pixels.
    pub tx: f32,
    /// Vertical translation in view pixels.
    pub ty: f32,
}

impl ViewTransform {
    /// No zoom.
    pub const IDENTITY: ViewTransform = ViewTransform {
        scale: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    /// True if zoomed in.
    pub fn is_zoomed(&self) -> bool {
        self.scale > 1.0 + 1e-3
    }

    /// Inverse transform: view pixel to content pixel.
    pub fn to_content(&self, x: f32, y: f32) -> (f32, f32) {
        let s = if self.scale >= 1.0 { self.scale } else { 1.0 };
        ((x - self.tx) / s, (y - self.ty) / s)
    }

    /// Clamps scale to 1..=4 and the translation so the content covers the view.
    pub fn clamped(self, w: f32, h: f32) -> ViewTransform {
        let scale = if self.scale.is_finite() {
            self.scale.clamp(1.0, 4.0)
        } else {
            1.0
        };
        let lim = |t: f32, extent: f32| {
            let lo = (extent * (1.0 - scale)).min(0.0);
            if t.is_finite() {
                t.max(lo).min(0.0)
            } else {
                0.0
            }
        };
        ViewTransform {
            scale,
            tx: lim(self.tx, w),
            ty: lim(self.ty, h),
        }
    }
}

/// View transform plus the view size, mapping view pixels to normalized stream
/// coordinates.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Mapper {
    pub view: ViewTransform,
    pub w: f32,
    pub h: f32,
}

impl Mapper {
    /// Maps a raw view pixel position to normalized stream coordinates.
    pub fn map(&self, x: f32, y: f32) -> (u16, u16) {
        let (cx, cy) = self.view.to_content(x, y);
        (
            normalize(f64::from(cx), f64::from(self.w)),
            normalize(f64::from(cy), f64::from(self.h)),
        )
    }
}

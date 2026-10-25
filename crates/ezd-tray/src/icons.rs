//! Status icons, embedded in the binary (regenerate with `assets/gen-icons.sh`).

use ezd_host_service::HostStatus;
use std::sync::Arc;

/// Which glyph the tray shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    Stopped,
    Waiting,
    Streaming,
    Error,
}

impl IconKind {
    pub fn of(status: &HostStatus) -> IconKind {
        match status {
            HostStatus::Stopped => IconKind::Stopped,
            HostStatus::Waiting { .. } => IconKind::Waiting,
            HostStatus::Streaming(_) => IconKind::Streaming,
            HostStatus::Error(_) => IconKind::Error,
        }
    }

    fn png(self) -> &'static [u8] {
        // Windows shows 16-32 px; the StatusNotifier hosts of Linux scale a larger bitmap better.
        macro_rules! pick {
            ($name:literal) => {
                if cfg!(windows) {
                    include_bytes!(concat!("../assets/status-", $name, "-32.png")).as_slice()
                } else {
                    include_bytes!(concat!("../assets/status-", $name, "-64.png")).as_slice()
                }
            };
        }
        match self {
            IconKind::Stopped => pick!("stopped"),
            IconKind::Waiting => pick!("waiting"),
            IconKind::Streaming => pick!("streaming"),
            IconKind::Error => pick!("error"),
        }
    }
}

/// A decoded RGBA8 image.
pub struct Rgba {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Decodes an embedded PNG to RGBA8.
pub fn decode(bytes: &[u8]) -> Result<Rgba, png::DecodingError> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
    let data = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buf
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => unreachable!("EXPAND turns indexed into RGB(A)"),
    };
    Ok(Rgba {
        data,
        width: info.width,
        height: info.height,
    })
}

/// The tray icon for `kind`.
pub fn tray_icon(kind: IconKind) -> Result<tray_icon::Icon, String> {
    let img = decode(kind.png()).map_err(|e| e.to_string())?;
    tray_icon::Icon::from_rgba(img.data, img.width, img.height).map_err(|e| e.to_string())
}

/// The window icon.
pub fn window_icon() -> Option<Arc<eframe::egui::IconData>> {
    let img = decode(include_bytes!("../assets/status-stopped-64.png")).ok()?;
    Some(Arc::new(eframe::egui::IconData {
        rgba: img.data,
        width: img.width,
        height: img.height,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_decodes_and_has_pixels() {
        for kind in [
            IconKind::Stopped,
            IconKind::Waiting,
            IconKind::Streaming,
            IconKind::Error,
        ] {
            let img = decode(kind.png()).unwrap();
            assert_eq!(img.width, img.height);
            assert_eq!(img.data.len(), (img.width * img.height * 4) as usize);
            assert!(
                img.data.as_chunks::<4>().0.iter().any(|p| p[3] > 0),
                "{kind:?} is empty"
            );
        }
    }
}

//! Parsing of the Windows accent colour registry values (platform independent, unit-tested).

/// Registry key (under HKCU) holding the accent palette and, below `DWM`, the accent colour.
pub const EXPLORER_ACCENT_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent";
pub const ACCENT_PALETTE_VALUE: &str = "AccentPalette";
pub const DWM_KEY: &str = r"Software\Microsoft\Windows\DWM";
pub const DWM_ACCENT_VALUE: &str = "AccentColor";

/// The base accent of `AccentPalette`: 8 RGBA entries of 4 bytes, the base accent is entry 3.
pub fn parse_accent_palette(bytes: &[u8]) -> Option<[u8; 3]> {
    let e = bytes.get(12..16)?;
    Some([e[0], e[1], e[2]])
}

/// `DWM\AccentColor`: a DWORD laid out as 0xAABBGGRR.
pub fn parse_dwm_accent(v: u32) -> [u8; 3] {
    [v as u8, (v >> 8) as u8, (v >> 16) as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_entry_three_is_the_base_accent() {
        let mut b = vec![0u8; 32];
        b[12..16].copy_from_slice(&[0x00, 0x78, 0xD4, 0xFF]);
        assert_eq!(parse_accent_palette(&b), Some([0x00, 0x78, 0xD4]));
    }

    #[test]
    fn short_palette_is_rejected() {
        assert_eq!(parse_accent_palette(&[0; 15]), None);
        assert_eq!(parse_accent_palette(&[]), None);
    }

    #[test]
    fn dwm_accent_is_abgr() {
        assert_eq!(parse_dwm_accent(0xFFD47800), [0x00, 0x78, 0xD4]);
    }
}

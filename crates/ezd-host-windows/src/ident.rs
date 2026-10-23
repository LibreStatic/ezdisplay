//! Recognising the Virtual-Display-Driver in Windows device strings.

/// Lower-case fragments that identify the driver: the EDID PnP id of its
/// monitors (`MTT1337`), its name, and the vendor.
const MARKERS: &[&str] = &[
    "mtt1337",
    "mttvdd",
    "virtual display driver",
    "virtualdisplaydriver",
    "vdd by mtt",
    "mikethetech",
];

/// True if a monitor/adapter name, device id or device path belongs to the VDD.
pub fn looks_like_vdd(s: &str) -> bool {
    let l = s.to_lowercase();
    MARKERS.iter().any(|m| l.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_vdd_strings() {
        assert!(looks_like_vdd(
            r"MONITOR\MTT1337\{4d36e96e-e325-11ce-bfc1-08002be10318}\0004"
        ));
        assert!(looks_like_vdd(
            r"\\?\DISPLAY#mtt1337#5&2b1bd5d5&0&UID0#{e6f07b5f}"
        ));
        assert!(looks_like_vdd("Virtual Display Driver by MTT"));
        assert!(!looks_like_vdd("Generic PnP Monitor"));
        assert!(!looks_like_vdd(r"MONITOR\ACR0A5B\{x}\0001"));
        assert!(!looks_like_vdd(""));
    }
}

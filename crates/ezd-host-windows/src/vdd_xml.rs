//! Reading and editing the Virtual-Display-Driver `vdd_settings.xml`.
//!
//! Edits are done as byte-range splices of the original text (ranges come from
//! `roxmltree`), so comments, whitespace and every setting we do not touch stay
//! exactly as they were.
//!
//! Layout the driver reads (see `loadSettings` in the driver source):
//!
//! ```xml
//! <vdd_settings>
//!   <monitors><count>1</count></monitors>
//!   <gpu><friendlyname>default</friendlyname></gpu>
//!   <global><g_refresh_rate>60</g_refresh_rate></global>
//!   <resolutions>
//!     <resolution><width>1920</width><height>1080</height><refresh_rate>60</refresh_rate></resolution>
//!   </resolutions>
//!   <cursor><HardwareCursor>true</HardwareCursor></cursor>
//! </vdd_settings>
//! ```
//!
//! The driver offers every explicit `resolution` entry with its own refresh
//! rate, plus every distinct `width x height` combined with every
//! `g_refresh_rate` (integers).

use roxmltree::{Document, Node};
use std::fmt;

/// A display mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Refresh rate in millihertz.
    pub refresh_mhz: u32,
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}x{}@{}Hz",
            self.width,
            self.height,
            format_refresh(self.refresh_mhz)
        )
    }
}

impl Mode {
    /// Parses `WxH@Hz` (`1920x1080@120`, `2560x1600@59.94`).
    pub fn parse(spec: &str) -> Result<Mode, XmlError> {
        let bad = || XmlError::Msg(format!("invalid mode {spec:?}, expected WxH@Hz"));
        let (res, hz) = spec.trim().split_once('@').ok_or_else(bad)?;
        let (w, h) = res.split_once(['x', 'X']).ok_or_else(bad)?;
        let hz: f64 = hz
            .trim()
            .trim_end_matches("Hz")
            .parse()
            .map_err(|_| bad())?;
        let mode = Mode {
            width: w.trim().parse().map_err(|_| bad())?,
            height: h.trim().parse().map_err(|_| bad())?,
            refresh_mhz: (hz * 1000.0).round() as u32,
        };
        if mode.width == 0 || mode.height == 0 || mode.refresh_mhz == 0 {
            return Err(bad());
        }
        Ok(mode)
    }
}

/// `120000` -> `"120"`, `59940` -> `"59.94"`.
pub fn format_refresh(mhz: u32) -> String {
    if mhz % 1000 == 0 {
        (mhz / 1000).to_string()
    } else {
        let s = format!("{:.3}", mhz as f64 / 1000.0);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Errors from parsing or editing the settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XmlError {
    /// Anything else.
    Msg(String),
}

impl fmt::Display for XmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let XmlError::Msg(m) = self;
        f.write_str(m)
    }
}

impl std::error::Error for XmlError {}

fn err<T>(m: impl Into<String>) -> Result<T, XmlError> {
    Err(XmlError::Msg(m.into()))
}

/// What the driver will read from the file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    /// `<monitors><count>`.
    pub monitor_count: Option<u32>,
    /// `<gpu><friendlyname>`: `default`, a name, or `name,bus`.
    pub gpu: Option<String>,
    /// `<global><g_refresh_rate>` values.
    pub global_refresh_hz: Vec<u32>,
    /// Explicit `<resolution>` entries.
    pub resolutions: Vec<Mode>,
    /// `<cursor><HardwareCursor>`.
    pub hardware_cursor: Option<bool>,
}

fn find<'a, 'i>(parent: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    parent
        .children()
        .find(|n| n.is_element() && n.tag_name().name().eq_ignore_ascii_case(name))
}

fn text_of(n: Node<'_, '_>) -> String {
    n.text().unwrap_or("").trim().to_string()
}

fn child_text(parent: Node<'_, '_>, name: &str) -> Option<String> {
    find(parent, name).map(text_of)
}

fn parse_hz(s: &str) -> Option<u32> {
    let v: f64 = s.trim().parse().ok()?;
    (v > 0.0).then(|| (v * 1000.0).round() as u32)
}

/// Parses the settings (tolerates missing sections, like the driver does).
pub fn parse(xml: &str) -> Result<Summary, XmlError> {
    let doc = Document::parse(xml).map_err(|e| XmlError::Msg(format!("XML parse error: {e}")))?;
    let root = doc.root_element();
    let mut s = Summary::default();
    if let Some(m) = find(root, "monitors") {
        s.monitor_count = child_text(m, "count").and_then(|t| t.parse().ok());
    }
    if let Some(g) = find(root, "gpu") {
        s.gpu = child_text(g, "friendlyname");
    }
    if let Some(g) = find(root, "global") {
        s.global_refresh_hz = g
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "g_refresh_rate")
            .filter_map(|n| text_of(n).parse().ok())
            .collect();
    }
    if let Some(rs) = find(root, "resolutions") {
        for r in rs
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "resolution")
        {
            let w = child_text(r, "width").and_then(|t| t.parse().ok());
            let h = child_text(r, "height").and_then(|t| t.parse().ok());
            // The driver defaults a missing refresh rate to 30.
            let hz = child_text(r, "refresh_rate")
                .filter(|t| !t.is_empty())
                .map_or(Some(30_000), |t| parse_hz(&t));
            if let (Some(width), Some(height), Some(refresh_mhz)) = (w, h, hz) {
                s.resolutions.push(Mode {
                    width,
                    height,
                    refresh_mhz,
                });
            }
        }
    }
    if let Some(c) = find(root, "cursor") {
        s.hardware_cursor = child_text(c, "HardwareCursor").map(|t| t.eq_ignore_ascii_case("true"));
    }
    Ok(s)
}

impl Summary {
    /// Whether the driver will offer `mode` (explicit entry, or a known
    /// resolution combined with a global refresh rate).
    pub fn supports(&self, mode: Mode) -> bool {
        if self.resolutions.contains(&mode) {
            return true;
        }
        mode.refresh_mhz % 1000 == 0
            && self.global_refresh_hz.contains(&(mode.refresh_mhz / 1000))
            && self
                .resolutions
                .iter()
                .any(|r| r.width == mode.width && r.height == mode.height)
    }

    /// JSON for reports.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "monitor_count": self.monitor_count,
            "gpu": self.gpu,
            "global_refresh_hz": self.global_refresh_hz,
            "resolutions": self.resolutions.iter().map(|m| m.to_string()).collect::<Vec<_>>(),
            "hardware_cursor": self.hardware_cursor,
        })
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Indentation (spaces/tabs after the last newline) in front of `pos`, if the
/// element starts its own line.
fn indent_before(xml: &str, pos: usize) -> Option<String> {
    let line_start = xml[..pos].rfind('\n')? + 1;
    let ind = &xml[line_start..pos];
    ind.chars()
        .all(|c| c == ' ' || c == '\t')
        .then(|| ind.to_string())
}

fn unit_indent(xml: &str, node: Node<'_, '_>) -> String {
    let own = indent_before(xml, node.range().start).unwrap_or_default();
    node.parent_element()
        .and_then(|p| indent_before(xml, p.range().start))
        .and_then(|pi| own.strip_prefix(pi.as_str()).map(str::to_string))
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "  ".to_string())
}

/// Inserts `child_xml` as the last child of `parent` (an element that may be
/// empty or self-closing), at `child_indent` indentation.
fn append_child(xml: &str, parent: Node<'_, '_>, child_xml: &str, child_indent: &str) -> String {
    let range = parent.range();
    let name = parent.tag_name().name();
    let parent_indent = indent_before(xml, range.start).unwrap_or_default();
    let last_child = parent.children().rfind(|n| n.is_element());
    let close = format!("</{name}>");
    if let Some(last) = last_child {
        let at = last.range().end;
        format!("{}\n{child_indent}{child_xml}{}", &xml[..at], &xml[at..])
    } else if xml[range.clone()].ends_with("/>") {
        let open = &xml[range.start..range.end - 2];
        format!(
            "{}{}>\n{child_indent}{child_xml}\n{parent_indent}{close}{}",
            &xml[..range.start],
            open.trim_end(),
            &xml[range.end..]
        )
    } else {
        // `<name>  </name>` or `<name></name>`: replace the content.
        let content_start = xml[range.clone()].find('>').map(|i| range.start + i + 1);
        let content_end = range.end.saturating_sub(close.len());
        match content_start {
            Some(cs) if cs <= content_end => format!(
                "{}\n{child_indent}{child_xml}\n{parent_indent}{}",
                &xml[..cs],
                &xml[content_end..]
            ),
            _ => xml.to_string(),
        }
    }
}

fn resolution_block(mode: Mode, ind: &str, unit: &str) -> String {
    format!(
        "<resolution>\n{ind}{unit}<width>{}</width>\n{ind}{unit}<height>{}</height>\n{ind}{unit}<refresh_rate>{}</refresh_rate>\n{ind}</resolution>",
        mode.width,
        mode.height,
        format_refresh(mode.refresh_mhz)
    )
}

/// Adds `mode` as an explicit `<resolution>`. Returns `None` if the driver
/// already offers it (no change needed).
pub fn add_mode(xml: &str, mode: Mode) -> Result<Option<String>, XmlError> {
    if parse(xml)?.supports(mode) {
        return Ok(None);
    }
    let doc = Document::parse(xml).map_err(|e| XmlError::Msg(format!("XML parse error: {e}")))?;
    let root = doc.root_element();
    let out = if let Some(rs) = find(root, "resolutions") {
        let ind_unit = unit_indent(xml, rs);
        let (child_ind, block_ind) = match rs
            .children()
            .find(|n| n.is_element())
            .and_then(|f| indent_before(xml, f.range().start))
        {
            Some(i) => (i.clone(), i),
            None => {
                let pi = indent_before(xml, rs.range().start).unwrap_or_default();
                (format!("{pi}{ind_unit}"), format!("{pi}{ind_unit}"))
            }
        };
        append_child(
            xml,
            rs,
            &resolution_block(mode, &block_ind, &ind_unit),
            &child_ind,
        )
    } else {
        let unit = unit_indent_of_root(xml, root);
        let block = format!(
            "<resolutions>\n{unit}{unit}{}\n{unit}</resolutions>",
            resolution_block(mode, &format!("{unit}{unit}"), &unit)
        );
        append_child(xml, root, &block, &unit)
    };
    // Safety net: never hand back something the driver could not parse.
    let check = parse(&out)?;
    if !check.supports(mode) {
        return err("internal error: edited settings do not contain the new mode");
    }
    Ok(Some(out))
}

fn unit_indent_of_root(xml: &str, root: Node<'_, '_>) -> String {
    root.children()
        .find(|n| n.is_element())
        .and_then(|c| indent_before(xml, c.range().start))
        .filter(|i| !i.is_empty())
        .unwrap_or_else(|| "  ".to_string())
}

/// Sets `<gpu><friendlyname>` (creating the elements when missing).
pub fn set_gpu(xml: &str, name: &str) -> Result<String, XmlError> {
    let doc = Document::parse(xml).map_err(|e| XmlError::Msg(format!("XML parse error: {e}")))?;
    let root = doc.root_element();
    let element = format!("<friendlyname>{}</friendlyname>", escape(name));
    let out = if let Some(g) = find(root, "gpu") {
        if let Some(f) = find(g, "friendlyname") {
            let r = f.range();
            format!("{}{element}{}", &xml[..r.start], &xml[r.end..])
        } else {
            let unit = unit_indent(xml, g);
            let ind = indent_before(xml, g.range().start).unwrap_or_default();
            append_child(xml, g, &element, &format!("{ind}{unit}"))
        }
    } else {
        let unit = unit_indent_of_root(xml, root);
        let block = format!("<gpu>\n{unit}{unit}{element}\n{unit}</gpu>");
        append_child(xml, root, &block, &unit)
    };
    let check = parse(&out)?;
    if check.gpu.as_deref() != Some(name) {
        return err("internal error: edited settings do not contain the new GPU");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version='1.0' encoding='utf-8'?>
<vdd_settings>
    <!-- keep me -->
    <monitors>
        <count>1</count>
    </monitors>
    <gpu>
        <friendlyname>default</friendlyname>
    </gpu>
    <global>
        <g_refresh_rate>60</g_refresh_rate>
        <g_refresh_rate>144</g_refresh_rate>
    </global>
    <resolutions>
        <resolution>
            <width>1920</width>
            <height>1080</height>
            <refresh_rate>60</refresh_rate>
        </resolution>
    </resolutions>
    <logging>
        <logging>false</logging>
    </logging>
    <cursor>
        <HardwareCursor>true</HardwareCursor>
    </cursor>
</vdd_settings>
"#;

    fn m(w: u32, h: u32, hz: u32) -> Mode {
        Mode {
            width: w,
            height: h,
            refresh_mhz: hz * 1000,
        }
    }

    #[test]
    fn parses_the_sections() {
        let s = parse(SAMPLE).unwrap();
        assert_eq!(s.monitor_count, Some(1));
        assert_eq!(s.gpu.as_deref(), Some("default"));
        assert_eq!(s.global_refresh_hz, [60, 144]);
        assert_eq!(s.resolutions, [m(1920, 1080, 60)]);
        assert_eq!(s.hardware_cursor, Some(true));
    }

    #[test]
    fn supports_explicit_and_global_combinations() {
        let s = parse(SAMPLE).unwrap();
        assert!(s.supports(m(1920, 1080, 60)));
        assert!(s.supports(m(1920, 1080, 144)), "known size x global rate");
        assert!(!s.supports(m(1920, 1080, 120)));
        assert!(!s.supports(m(2560, 1600, 60)));
        assert!(!s.supports(Mode {
            width: 1920,
            height: 1080,
            refresh_mhz: 59_940
        }));
    }

    #[test]
    fn add_mode_preserves_everything_else() {
        let out = add_mode(SAMPLE, m(2560, 1600, 120)).unwrap().unwrap();
        assert!(out.contains("<!-- keep me -->"));
        assert!(out.contains("<HardwareCursor>true</HardwareCursor>"));
        assert!(out.starts_with("<?xml version='1.0' encoding='utf-8'?>"));
        let s = parse(&out).unwrap();
        assert_eq!(s.resolutions, [m(1920, 1080, 60), m(2560, 1600, 120)]);
        assert!(s.supports(m(2560, 1600, 120)));
        // Only the resolutions block changed.
        let before_end = SAMPLE.find("</resolutions>").unwrap();
        let tail = &SAMPLE[before_end..];
        assert!(out.ends_with(tail));
        let head_end = SAMPLE.find("        </resolution>\n").unwrap();
        assert!(out.starts_with(&SAMPLE[..head_end]));
        // Indentation follows the file.
        assert!(out.contains("\n        <resolution>\n            <width>2560</width>"));
    }

    #[test]
    fn add_mode_is_idempotent() {
        assert_eq!(add_mode(SAMPLE, m(1920, 1080, 60)).unwrap(), None);
        let once = add_mode(SAMPLE, m(2000, 1200, 90)).unwrap().unwrap();
        assert_eq!(add_mode(&once, m(2000, 1200, 90)).unwrap(), None);
    }

    #[test]
    fn add_mode_fractional_refresh() {
        let mode = Mode {
            width: 1280,
            height: 800,
            refresh_mhz: 59_940,
        };
        let out = add_mode(SAMPLE, mode).unwrap().unwrap();
        assert!(out.contains("<refresh_rate>59.94</refresh_rate>"));
        assert!(parse(&out).unwrap().supports(mode));
    }

    #[test]
    fn add_mode_to_empty_and_missing_resolutions() {
        let empty = "<vdd_settings>\n  <resolutions>\n  </resolutions>\n</vdd_settings>\n";
        let out = add_mode(empty, m(800, 600, 60)).unwrap().unwrap();
        assert_eq!(parse(&out).unwrap().resolutions, [m(800, 600, 60)]);
        let selfclosed = "<vdd_settings>\n  <resolutions/>\n</vdd_settings>\n";
        let out = add_mode(selfclosed, m(800, 600, 60)).unwrap().unwrap();
        assert_eq!(parse(&out).unwrap().resolutions, [m(800, 600, 60)]);
        let missing = "<vdd_settings>\n  <monitors><count>1</count></monitors>\n</vdd_settings>\n";
        let out = add_mode(missing, m(800, 600, 60)).unwrap().unwrap();
        assert_eq!(parse(&out).unwrap().resolutions, [m(800, 600, 60)]);
        assert!(out.contains("<monitors><count>1</count></monitors>"));
    }

    #[test]
    fn set_gpu_replaces_or_creates() {
        let out = set_gpu(SAMPLE, "NVIDIA GeForce RTX 3050 Laptop GPU").unwrap();
        assert_eq!(
            parse(&out).unwrap().gpu.as_deref(),
            Some("NVIDIA GeForce RTX 3050 Laptop GPU")
        );
        assert!(out.contains("<!-- keep me -->"));
        assert_eq!(
            out.replace("NVIDIA GeForce RTX 3050 Laptop GPU", "default"),
            SAMPLE
        );
        let none = "<vdd_settings>\n  <monitors><count>1</count></monitors>\n</vdd_settings>\n";
        let out = set_gpu(none, "A & B").unwrap();
        assert!(out.contains("A &amp; B"));
        assert_eq!(parse(&out).unwrap().gpu.as_deref(), Some("A & B"));
        let nofriendly = "<vdd_settings>\n  <gpu>\n  </gpu>\n</vdd_settings>\n";
        let out = set_gpu(nofriendly, "X").unwrap();
        assert_eq!(parse(&out).unwrap().gpu.as_deref(), Some("X"));
    }

    #[test]
    fn invalid_xml_is_an_error() {
        assert!(parse("<a>").is_err());
        assert!(add_mode("not xml", m(1, 1, 60)).is_err());
        assert!(set_gpu("<a><b></a>", "x").is_err());
    }

    #[test]
    fn mode_spec_parsing() {
        assert_eq!(Mode::parse("1920x1080@120").unwrap(), m(1920, 1080, 120));
        assert_eq!(
            Mode::parse("2560X1600@59.94Hz").unwrap().refresh_mhz,
            59_940
        );
        assert!(Mode::parse("1920x1080").is_err());
        assert!(Mode::parse("0x0@60").is_err());
        assert_eq!(m(2560, 1600, 120).to_string(), "2560x1600@120Hz");
        assert_eq!(format_refresh(59_940), "59.94");
    }
}

//! Minimal SPA pod writer/reader for the handful of parameters the screencast consumer needs
//! (`EnumFormat`/`Format` with DMA-BUF modifiers, `Buffers`, `Meta`).
//!
//! The libspa Rust bindings cannot express property flags such as `DONT_FIXATE` conveniently, and the
//! layout is small and stable, so it is written out here and checked against `spa_sys` in tests.

#![allow(dead_code)]

// Pod types.
const T_ID: u32 = 3;
const T_INT: u32 = 4;
const T_LONG: u32 = 5;
const T_RECT: u32 = 10;
const T_FRAC: u32 = 11;
const T_OBJECT: u32 = 15;
const T_CHOICE: u32 = 19;

/// `SPA_CHOICE_None`: the choice is fixed to its first value.
const CHOICE_NONE: u32 = 0;

/// Object type of format params.
pub const OBJ_FORMAT: u32 = 0x40003;
/// Object type of buffers params.
pub const OBJ_PARAM_BUFFERS: u32 = 0x40004;
/// Object type of meta params.
pub const OBJ_PARAM_META: u32 = 0x40005;

/// Param ids.
pub const PARAM_ENUM_FORMAT: u32 = 3;
/// `SPA_PARAM_Format`.
pub const PARAM_FORMAT: u32 = 4;
/// `SPA_PARAM_Buffers`.
pub const PARAM_BUFFERS: u32 = 5;
/// `SPA_PARAM_Meta`.
pub const PARAM_META: u32 = 6;

/// Format keys.
pub const FORMAT_MEDIA_TYPE: u32 = 1;
/// `SPA_FORMAT_mediaSubtype`.
pub const FORMAT_MEDIA_SUBTYPE: u32 = 2;
/// `SPA_FORMAT_VIDEO_format`.
pub const FORMAT_VIDEO_FORMAT: u32 = 0x20001;
/// `SPA_FORMAT_VIDEO_modifier`.
pub const FORMAT_VIDEO_MODIFIER: u32 = 0x20002;
/// `SPA_FORMAT_VIDEO_size`.
pub const FORMAT_VIDEO_SIZE: u32 = 0x20003;
/// `SPA_FORMAT_VIDEO_framerate`.
pub const FORMAT_VIDEO_FRAMERATE: u32 = 0x20004;
/// `SPA_FORMAT_VIDEO_maxFramerate`.
pub const FORMAT_VIDEO_MAX_FRAMERATE: u32 = 0x20005;
/// `SPA_MEDIA_TYPE_video`.
pub const MEDIA_TYPE_VIDEO: u32 = 2;
/// `SPA_MEDIA_SUBTYPE_raw`.
pub const MEDIA_SUBTYPE_RAW: u32 = 1;

/// Buffers keys.
pub const BUFFERS_BUFFERS: u32 = 1;
/// `SPA_PARAM_BUFFERS_blocks`.
pub const BUFFERS_BLOCKS: u32 = 2;
/// `SPA_PARAM_BUFFERS_dataType`.
pub const BUFFERS_DATA_TYPE: u32 = 6;
/// Meta keys.
pub const META_TYPE: u32 = 1;
/// `SPA_PARAM_META_size`.
pub const META_SIZE: u32 = 2;
/// `SPA_META_Header`.
pub const META_HEADER: u32 = 1;
/// `SPA_META_Cursor`.
pub const META_CURSOR: u32 = 5;

/// Data types.
pub const DATA_MEM_PTR: u32 = 1;
/// `SPA_DATA_MemFd`.
pub const DATA_MEM_FD: u32 = 2;
/// `SPA_DATA_DmaBuf`.
pub const DATA_DMA_BUF: u32 = 3;

/// Property flags.
pub const PROP_MANDATORY: u32 = 8;
/// `SPA_POD_PROP_FLAG_DONT_FIXATE`.
pub const PROP_DONT_FIXATE: u32 = 16;

/// A value that can be written as (part of) a pod.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    /// `Id`.
    Id(u32),
    /// `Int`.
    Int(i32),
    /// `Long`.
    Long(i64),
    /// `Rectangle(width, height)`.
    Rect(u32, u32),
    /// `Fraction(num, denom)`.
    Frac(u32, u32),
    /// Enum choice of ids: `[default, alternatives...]`.
    EnumId(Vec<u32>),
    /// Enum choice of longs: `[default, alternatives...]`.
    EnumLong(Vec<i64>),
    /// Range choice of ints: `(default, min, max)`.
    RangeInt(i32, i32, i32),
    /// Range choice of rectangles.
    RangeRect((u32, u32), (u32, u32), (u32, u32)),
    /// Range choice of fractions.
    RangeFrac((u32, u32), (u32, u32), (u32, u32)),
    /// Flags choice of ints with a single value.
    FlagsInt(i32),
}

fn pad(out: &mut Vec<u8>) {
    while out.len() % 8 != 0 {
        out.push(0);
    }
}

fn put(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_ne_bytes());
}

/// Writes `size,type,body` and pads. `body` writes the body bytes.
fn pod(out: &mut Vec<u8>, ty: u32, body: impl FnOnce(&mut Vec<u8>)) {
    let at = out.len();
    put(out, 0);
    put(out, ty);
    body(out);
    let size = (out.len() - at - 8) as u32;
    out[at..at + 4].copy_from_slice(&size.to_ne_bytes());
    pad(out);
}

fn choice(out: &mut Vec<u8>, kind: u32, child_ty: u32, child_size: u32, values: &[&[u8]]) {
    pod(out, T_CHOICE, |o| {
        put(o, kind);
        put(o, 0);
        put(o, child_size);
        put(o, child_ty);
        for v in values {
            o.extend_from_slice(v);
        }
    });
}

impl Val {
    fn write(&self, out: &mut Vec<u8>) {
        let b = |v: u32| v.to_ne_bytes();
        let pair = |a: (u32, u32)| -> Vec<u8> { [b(a.0), b(a.1)].concat() };
        match self {
            Val::Id(v) => pod(out, T_ID, |o| put(o, *v)),
            Val::Int(v) => pod(out, T_INT, |o| o.extend_from_slice(&v.to_ne_bytes())),
            Val::Long(v) => pod(out, T_LONG, |o| o.extend_from_slice(&v.to_ne_bytes())),
            Val::Rect(w, h) => pod(out, T_RECT, |o| {
                put(o, *w);
                put(o, *h)
            }),
            Val::Frac(n, d) => pod(out, T_FRAC, |o| {
                put(o, *n);
                put(o, *d)
            }),
            // Enum choices list the default first and then every alternative, default included.
            Val::EnumId(v) => {
                let vals: Vec<[u8; 4]> = v.first().into_iter().chain(v).map(|x| b(*x)).collect();
                let refs: Vec<&[u8]> = vals.iter().map(|x| x.as_slice()).collect();
                choice(
                    out,
                    if v.len() > 1 { 3 } else { CHOICE_NONE },
                    T_ID,
                    4,
                    &refs,
                );
            }
            Val::EnumLong(v) if v.len() == 1 => Val::Long(v[0]).write(out),
            Val::EnumLong(v) => {
                let vals: Vec<[u8; 8]> = v
                    .first()
                    .into_iter()
                    .chain(v)
                    .map(|x| x.to_ne_bytes())
                    .collect();
                let refs: Vec<&[u8]> = vals.iter().map(|x| x.as_slice()).collect();
                choice(
                    out,
                    if v.len() > 1 { 3 } else { CHOICE_NONE },
                    T_LONG,
                    8,
                    &refs,
                );
            }
            Val::RangeInt(d, lo, hi) => {
                let vals = [d.to_ne_bytes(), lo.to_ne_bytes(), hi.to_ne_bytes()];
                let refs: Vec<&[u8]> = vals.iter().map(|x| x.as_slice()).collect();
                choice(out, 1, T_INT, 4, &refs);
            }
            Val::RangeRect(d, lo, hi) => {
                let vals = [pair(*d), pair(*lo), pair(*hi)];
                let refs: Vec<&[u8]> = vals.iter().map(|x| x.as_slice()).collect();
                choice(out, 1, T_RECT, 8, &refs);
            }
            Val::RangeFrac(d, lo, hi) => {
                let vals = [pair(*d), pair(*lo), pair(*hi)];
                let refs: Vec<&[u8]> = vals.iter().map(|x| x.as_slice()).collect();
                choice(out, 1, T_FRAC, 8, &refs);
            }
            Val::FlagsInt(v) => {
                let val = v.to_ne_bytes();
                choice(out, 4, T_INT, 4, &[&val]);
            }
        }
    }
}

/// Serializes an object pod: `props` are `(key, flags, value)`.
pub fn object(obj_type: u32, obj_id: u32, props: &[(u32, u32, Val)]) -> Vec<u8> {
    let mut out = Vec::new();
    pod(&mut out, T_OBJECT, |o| {
        put(o, obj_type);
        put(o, obj_id);
        for (key, flags, val) in props {
            put(o, *key);
            put(o, *flags);
            val.write(o);
        }
    });
    out
}

/// A video format advertised to or parsed from the stream.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoFormatProps {
    /// `SPA_VIDEO_FORMAT_*`.
    pub format: u32,
    /// Modifiers; `None` = shared-memory format without modifier.
    pub modifiers: Option<Vec<u64>>,
    /// Resolution range or fixed size.
    pub size: Val,
    /// Frame rate.
    pub framerate: Val,
    /// Highest frame rate we want (the compositor caps its delivery to it).
    pub max_fps: u32,
}

/// One `EnumFormat` for `fmt`. For DMA-BUF formats the modifier property is mandatory and
/// `DONT_FIXATE` is set unless there is exactly one modifier.
pub fn enum_format(param_id: u32, fmt: &VideoFormatProps) -> Vec<u8> {
    let mut props = vec![
        (FORMAT_MEDIA_TYPE, 0, Val::Id(MEDIA_TYPE_VIDEO)),
        (FORMAT_MEDIA_SUBTYPE, 0, Val::Id(MEDIA_SUBTYPE_RAW)),
        (FORMAT_VIDEO_FORMAT, 0, Val::Id(fmt.format)),
    ];
    if let Some(mods) = &fmt.modifiers {
        let flags = if mods.len() > 1 {
            PROP_MANDATORY | PROP_DONT_FIXATE
        } else {
            PROP_MANDATORY
        };
        props.push((
            FORMAT_VIDEO_MODIFIER,
            flags,
            Val::EnumLong(mods.iter().map(|m| *m as i64).collect()),
        ));
    }
    props.push((FORMAT_VIDEO_SIZE, 0, fmt.size.clone()));
    props.push((FORMAT_VIDEO_FRAMERATE, 0, fmt.framerate.clone()));
    props.push((
        FORMAT_VIDEO_MAX_FRAMERATE,
        0,
        Val::RangeFrac((fmt.max_fps, 1), (1, 1), (fmt.max_fps.max(240), 1)),
    ));
    object(OBJ_FORMAT, param_id, &props)
}

/// A fully fixed format with exactly one modifier (answer to a `DONT_FIXATE` format).
pub fn fixated_format(f: &ParsedFormat, modifier: u64) -> Option<Vec<u8>> {
    let (w, h) = f.size?;
    let (n, d) = f.framerate.unwrap_or((0, 1));
    Some(object(
        OBJ_FORMAT,
        PARAM_ENUM_FORMAT,
        &[
            (FORMAT_MEDIA_TYPE, 0, Val::Id(MEDIA_TYPE_VIDEO)),
            (FORMAT_MEDIA_SUBTYPE, 0, Val::Id(MEDIA_SUBTYPE_RAW)),
            (FORMAT_VIDEO_FORMAT, 0, Val::Id(f.format?)),
            (
                FORMAT_VIDEO_MODIFIER,
                PROP_MANDATORY,
                Val::Long(modifier as i64),
            ),
            (FORMAT_VIDEO_SIZE, 0, Val::Rect(w, h)),
            (FORMAT_VIDEO_FRAMERATE, 0, Val::Frac(n, d)),
        ],
    ))
}

/// A scalar read from a parsed pod (choices yield their default/first value).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scalar {
    /// Id.
    Id(u32),
    /// Int.
    Int(i32),
    /// Long.
    Long(i64),
    /// Two u32 (rectangle or fraction).
    Pair(u32, u32),
}

/// Parsed view of a negotiated `Format` object.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParsedFormat {
    /// Pixel format id.
    pub format: Option<u32>,
    /// Size.
    pub size: Option<(u32, u32)>,
    /// Frame rate (num, denom).
    pub framerate: Option<(u32, u32)>,
    /// Modifier values present (the default/fixed value first).
    pub modifiers: Option<Vec<u64>>,
    /// True if the modifier property carries `DONT_FIXATE` (the format still has to be fixated).
    pub modifier_dont_fixate: bool,
}

fn rd(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

fn rd64(b: &[u8], off: usize) -> Option<i64> {
    Some(i64::from_ne_bytes(b.get(off..off + 8)?.try_into().ok()?))
}

/// Reads a scalar value `ty` at `off`.
fn scalar(b: &[u8], ty: u32, off: usize) -> Option<Scalar> {
    Some(match ty {
        T_ID => Scalar::Id(rd(b, off)?),
        T_INT => Scalar::Int(rd(b, off)? as i32),
        T_LONG => Scalar::Long(rd64(b, off)?),
        T_RECT | T_FRAC => Scalar::Pair(rd(b, off)?, rd(b, off + 4)?),
        _ => return None,
    })
}

/// Values of a pod at `off` (header position): one for plain pods and fixed (`None`) choices,
/// the distinct values in order for other choices.
fn values(b: &[u8], off: usize) -> Option<Vec<Scalar>> {
    let size = rd(b, off)? as usize;
    let ty = rd(b, off + 4)?;
    let body = off + 8;
    if ty != T_CHOICE {
        return Some(vec![scalar(b, ty, body)?]);
    }
    let kind = rd(b, body)?;
    let child_size = rd(b, body + 8)? as usize;
    let child_ty = rd(b, body + 12)?;
    if child_size == 0 {
        return None;
    }
    let mut v = Vec::new();
    let mut p = body + 16;
    while p + child_size <= body + size {
        let x = scalar(b, child_ty, p)?;
        // A fixated choice keeps its old alternatives after the value; only the first counts.
        if kind == CHOICE_NONE {
            return Some(vec![x]);
        }
        if !v.contains(&x) {
            v.push(x);
        }
        p += child_size;
    }
    Some(v)
}

/// Parses the bytes of a `Format` object pod.
pub fn parse_format(b: &[u8]) -> Option<ParsedFormat> {
    if rd(b, 4)? != T_OBJECT {
        return None;
    }
    let total = 8 + rd(b, 0)? as usize;
    let mut p = 8 + 8;
    let mut out = ParsedFormat::default();
    while p + 16 <= total.min(b.len()) {
        let key = rd(b, p)?;
        let flags = rd(b, p + 4)?;
        let vsize = rd(b, p + 8)? as usize;
        let vals = values(b, p + 8);
        if let Some(vals) = vals {
            match (key, vals.first().copied()) {
                (FORMAT_VIDEO_FORMAT, Some(Scalar::Id(f))) => out.format = Some(f),
                (FORMAT_VIDEO_SIZE, Some(Scalar::Pair(w, h))) => out.size = Some((w, h)),
                (FORMAT_VIDEO_FRAMERATE, Some(Scalar::Pair(n, d))) => out.framerate = Some((n, d)),
                (FORMAT_VIDEO_MODIFIER, Some(Scalar::Long(_))) => {
                    out.modifiers = Some(
                        vals.iter()
                            .filter_map(|s| match s {
                                Scalar::Long(l) => Some(*l as u64),
                                _ => None,
                            })
                            .collect(),
                    );
                    out.modifier_dont_fixate = flags & PROP_DONT_FIXATE != 0;
                }
                _ => {}
            }
        }
        p += 8 + 8 + vsize.div_ceil(8) * 8;
    }
    Some(out)
}

/// `Buffers` param asking for the given data types (bit mask of `1 << DATA_*`).
pub fn buffers_param(data_types: u32) -> Vec<u8> {
    object(
        OBJ_PARAM_BUFFERS,
        PARAM_BUFFERS,
        &[
            (BUFFERS_BUFFERS, 0, Val::RangeInt(4, 2, 16)),
            (BUFFERS_BLOCKS, 0, Val::Int(1)),
            (BUFFERS_DATA_TYPE, 0, Val::FlagsInt(data_types as i32)),
        ],
    )
}

/// `Meta` param requesting a metadata block of `size` bytes (at least).
pub fn meta_param(meta_type: u32, size: u32) -> Vec<u8> {
    object(
        OBJ_PARAM_META,
        PARAM_META,
        &[
            (META_TYPE, 0, Val::Id(meta_type)),
            (
                META_SIZE,
                0,
                Val::RangeInt(size as i32, size as i32, size as i32 * 4),
            ),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipewire::spa::sys as s;

    #[test]
    fn constants_match_spa() {
        assert_eq!(OBJ_FORMAT, s::SPA_TYPE_OBJECT_Format);
        assert_eq!(OBJ_PARAM_BUFFERS, s::SPA_TYPE_OBJECT_ParamBuffers);
        assert_eq!(OBJ_PARAM_META, s::SPA_TYPE_OBJECT_ParamMeta);
        assert_eq!(PARAM_ENUM_FORMAT, s::SPA_PARAM_EnumFormat);
        assert_eq!(PARAM_FORMAT, s::SPA_PARAM_Format);
        assert_eq!(PARAM_BUFFERS, s::SPA_PARAM_Buffers);
        assert_eq!(PARAM_META, s::SPA_PARAM_Meta);
        assert_eq!(FORMAT_MEDIA_TYPE, s::SPA_FORMAT_mediaType);
        assert_eq!(FORMAT_MEDIA_SUBTYPE, s::SPA_FORMAT_mediaSubtype);
        assert_eq!(FORMAT_VIDEO_FORMAT, s::SPA_FORMAT_VIDEO_format);
        assert_eq!(FORMAT_VIDEO_MODIFIER, s::SPA_FORMAT_VIDEO_modifier);
        assert_eq!(FORMAT_VIDEO_SIZE, s::SPA_FORMAT_VIDEO_size);
        assert_eq!(FORMAT_VIDEO_FRAMERATE, s::SPA_FORMAT_VIDEO_framerate);
        assert_eq!(FORMAT_VIDEO_MAX_FRAMERATE, s::SPA_FORMAT_VIDEO_maxFramerate);
        assert_eq!(MEDIA_TYPE_VIDEO, s::SPA_MEDIA_TYPE_video);
        assert_eq!(MEDIA_SUBTYPE_RAW, s::SPA_MEDIA_SUBTYPE_raw);
        assert_eq!(BUFFERS_BUFFERS, s::SPA_PARAM_BUFFERS_buffers);
        assert_eq!(BUFFERS_BLOCKS, s::SPA_PARAM_BUFFERS_blocks);
        assert_eq!(BUFFERS_DATA_TYPE, s::SPA_PARAM_BUFFERS_dataType);
        assert_eq!(META_TYPE, s::SPA_PARAM_META_type);
        assert_eq!(META_SIZE, s::SPA_PARAM_META_size);
        assert_eq!(META_HEADER, s::SPA_META_Header);
        assert_eq!(META_CURSOR, s::SPA_META_Cursor);
        assert_eq!(DATA_MEM_PTR, s::SPA_DATA_MemPtr);
        assert_eq!(DATA_MEM_FD, s::SPA_DATA_MemFd);
        assert_eq!(DATA_DMA_BUF, s::SPA_DATA_DmaBuf);
        assert_eq!(PROP_MANDATORY, s::SPA_POD_PROP_FLAG_MANDATORY);
        assert_eq!(PROP_DONT_FIXATE, s::SPA_POD_PROP_FLAG_DONT_FIXATE);
        assert_eq!(T_OBJECT, s::SPA_TYPE_Object);
        assert_eq!(T_CHOICE, s::SPA_TYPE_Choice);
        assert_eq!(T_LONG, s::SPA_TYPE_Long);
        assert_eq!(T_RECT, s::SPA_TYPE_Rectangle);
        assert_eq!(T_FRAC, s::SPA_TYPE_Fraction);
    }

    fn sample(mods: Option<Vec<u64>>) -> VideoFormatProps {
        VideoFormatProps {
            format: 8,
            modifiers: mods,
            size: Val::RangeRect((1920, 1080), (1, 1), (8192, 8192)),
            framerate: Val::RangeFrac((0, 1), (0, 1), (240, 1)),
            max_fps: 120,
        }
    }

    #[test]
    fn pods_are_valid_to_libspa() {
        for bytes in [
            enum_format(
                PARAM_ENUM_FORMAT,
                &sample(Some(vec![0, 0x0100_0000_0000_0002])),
            ),
            enum_format(PARAM_ENUM_FORMAT, &sample(None)),
            buffers_param(1 << DATA_DMA_BUF),
            meta_param(META_CURSOR, 4096),
        ] {
            assert_eq!(bytes.len() % 8, 0);
            let p = pipewire::spa::pod::Pod::from_bytes(&bytes).expect("valid pod");
            assert!(p.is_object());
        }
    }

    #[test]
    fn modifier_flags_and_roundtrip() {
        let bytes = enum_format(PARAM_FORMAT, &sample(Some(vec![0, 0x0100_0000_0000_0002])));
        let f = parse_format(&bytes).unwrap();
        assert_eq!(f.format, Some(8));
        assert_eq!(f.size, Some((1920, 1080)));
        assert_eq!(f.framerate, Some((0, 1)));
        assert_eq!(f.modifiers, Some(vec![0, 0x0100_0000_0000_0002]));
        assert!(f.modifier_dont_fixate);

        let one = parse_format(&enum_format(PARAM_FORMAT, &sample(Some(vec![0])))).unwrap();
        assert_eq!(one.modifiers, Some(vec![0]));
        assert!(!one.modifier_dont_fixate);

        let shm = parse_format(&enum_format(PARAM_FORMAT, &sample(None))).unwrap();
        assert_eq!(shm.modifiers, None);
    }

    #[test]
    fn libspa_agrees_on_modifier_prop() {
        let bytes = enum_format(PARAM_ENUM_FORMAT, &sample(Some(vec![0, 7])));
        let pod = pipewire::spa::pod::Pod::from_bytes(&bytes).unwrap();
        let obj = pod.as_object().unwrap();
        let prop = obj
            .find_prop(pipewire::spa::utils::Id(FORMAT_VIDEO_MODIFIER))
            .unwrap();
        assert!(
            prop.flags()
                .contains(pipewire::spa::pod::PodPropFlags::DONT_FIXATE)
        );
        assert!(prop.value().is_choice());
    }

    #[test]
    fn fixated_format_has_single_modifier() {
        let parsed = ParsedFormat {
            format: Some(8),
            size: Some((2560, 1600)),
            framerate: Some((0, 1)),
            modifiers: Some(vec![0, 2]),
            modifier_dont_fixate: true,
        };
        let bytes = fixated_format(&parsed, 2).unwrap();
        assert!(pipewire::spa::pod::Pod::from_bytes(&bytes).is_some());
        let back = parse_format(&bytes).unwrap();
        assert_eq!(back.modifiers, Some(vec![2]));
        assert!(!back.modifier_dont_fixate);
        assert_eq!(back.size, Some((2560, 1600)));
        assert!(fixated_format(&ParsedFormat::default(), 0).is_none());
    }

    /// A `Format` object whose modifier is a choice of `kind` with exactly these children.
    fn format_with_modifier_choice(kind: u32, children: &[i64]) -> Vec<u8> {
        let mut out = Vec::new();
        pod(&mut out, T_OBJECT, |o| {
            put(o, OBJ_FORMAT);
            put(o, PARAM_FORMAT);
            put(o, FORMAT_VIDEO_FORMAT);
            put(o, 0);
            Val::Id(8).write(o);
            put(o, FORMAT_VIDEO_MODIFIER);
            put(o, PROP_MANDATORY);
            let vals: Vec<[u8; 8]> = children.iter().map(|m| m.to_ne_bytes()).collect();
            let refs: Vec<&[u8]> = vals.iter().map(|x| x.as_slice()).collect();
            choice(o, kind, T_LONG, 8, &refs);
        });
        out
    }

    #[test]
    fn fixed_choice_yields_only_its_value() {
        // What KWin sends back after the consumer fixates the modifier: a `None` choice that
        // still carries the old alternatives.
        let bytes = format_with_modifier_choice(CHOICE_NONE, &[0, 0, 0]);
        assert!(pipewire::spa::pod::Pod::from_bytes(&bytes).is_some());
        let f = parse_format(&bytes).unwrap();
        assert_eq!(f.modifiers, Some(vec![0]));
        assert!(!f.modifier_dont_fixate);

        let f = parse_format(&format_with_modifier_choice(CHOICE_NONE, &[7, 0, 9])).unwrap();
        assert_eq!(f.modifiers, Some(vec![7]));
    }

    #[test]
    fn enum_choice_lists_each_modifier_once() {
        let f = parse_format(&format_with_modifier_choice(3, &[0, 0, 0, 7, 8])).unwrap();
        assert_eq!(f.modifiers, Some(vec![0, 7, 8]));
    }

    #[test]
    fn enum_choice_repeats_the_default_as_an_alternative() {
        let bytes = enum_format(PARAM_ENUM_FORMAT, &sample(Some(vec![0, 7])));
        let pod = pipewire::spa::pod::Pod::from_bytes(&bytes).unwrap();
        let prop = pod
            .as_object()
            .unwrap()
            .find_prop(pipewire::spa::utils::Id(FORMAT_VIDEO_MODIFIER))
            .unwrap();
        // Choice header (16 bytes) + default + both alternatives.
        assert_eq!(prop.value().size() as usize, 16 + 3 * 8);
    }

    #[test]
    fn parse_rejects_truncated() {
        let bytes = enum_format(PARAM_FORMAT, &sample(None));
        let _ = parse_format(&bytes[..12]);
        let _ = parse_format(&bytes[..bytes.len() - 5]);
        assert!(parse_format(&[]).is_none());
    }
}

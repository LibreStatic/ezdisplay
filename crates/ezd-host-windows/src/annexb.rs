//! Annex-B helpers: parameter-set extraction for `codec_config`.
//!
//! Pure byte manipulation, tested on every platform.

/// Iterates over NAL units of an Annex-B stream. Yields `(start_of_start_code, nal_start, nal_end)`
/// where `nal_start` is the offset of the NAL header byte.
fn nal_ranges(data: &[u8]) -> Vec<(usize, usize, usize)> {
    let mut starts = Vec::new(); // (start-code offset, payload offset)
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            let sc = if i > 0 && data[i - 1] == 0 { i - 1 } else { i };
            starts.push((sc, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (n, &(sc, ps)) in starts.iter().enumerate() {
        let end = starts.get(n + 1).map_or(data.len(), |s| s.0);
        out.push((sc, ps, end));
    }
    out
}

/// H.264 NAL unit types present in `data`, in order.
pub fn h264_nal_types(data: &[u8]) -> Vec<u8> {
    nal_ranges(data)
        .into_iter()
        .filter(|&(_, ps, end)| ps < end)
        .map(|(_, ps, _)| data[ps] & 0x1f)
        .collect()
}

/// Concatenation (with their start codes) of all SPS (7) and PPS (8) NAL units.
/// `None` if there is no SPS or no PPS.
pub fn extract_h264_param_sets(data: &[u8]) -> Option<Vec<u8>> {
    let (mut out, mut sps, mut pps) = (Vec::new(), false, false);
    for (sc, ps, end) in nal_ranges(data) {
        if ps >= end {
            continue;
        }
        match data[ps] & 0x1f {
            7 => sps = true,
            8 => pps = true,
            _ => continue,
        }
        out.extend_from_slice(&data[sc..end]);
    }
    (sps && pps).then_some(out)
}

/// Converts an `avcC` record (extradata of MP4-style H.264) to Annex-B SPS/PPS.
pub fn avcc_to_annexb(avcc: &[u8]) -> Option<Vec<u8>> {
    if avcc.len() < 7 || avcc[0] != 1 {
        return None;
    }
    let mut out = Vec::new();
    let mut p = 5;
    let nsps = (avcc[p] & 0x1f) as usize;
    p += 1;
    let take = |p: &mut usize, out: &mut Vec<u8>| -> Option<()> {
        let len = u16::from_be_bytes([*avcc.get(*p)?, *avcc.get(*p + 1)?]) as usize;
        *p += 2;
        let nal = avcc.get(*p..*p + len)?;
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
        *p += len;
        Some(())
    };
    for _ in 0..nsps {
        take(&mut p, &mut out)?;
    }
    let npps = *avcc.get(p)? as usize;
    p += 1;
    for _ in 0..npps {
        take(&mut p, &mut out)?;
    }
    Some(out)
}

/// Parameter sets from encoder extradata (Annex-B or avcC), if usable.
pub fn param_sets_from_extradata(extradata: &[u8]) -> Option<Vec<u8>> {
    extract_h264_param_sets(extradata).or_else(|| avcc_to_annexb(extradata))
}

/// Returns `packet` with `params` prepended unless it already carries an SPS.
pub fn with_param_sets(packet: Vec<u8>, params: &[u8]) -> Vec<u8> {
    if params.is_empty() || h264_nal_types(&packet).contains(&7) {
        return packet;
    }
    let mut out = Vec::with_capacity(params.len() + packet.len());
    out.extend_from_slice(params);
    out.extend_from_slice(&packet);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPS: &[u8] = &[0x67, 0x42, 0xc0, 0x1f];
    const PPS: &[u8] = &[0x68, 0xce, 0x3c, 0x80];
    const IDR: &[u8] = &[0x65, 0x88, 0x84, 0x00];

    fn annexb(nals: &[&[u8]], long: bool) -> Vec<u8> {
        let mut v = Vec::new();
        for n in nals {
            if long {
                v.push(0);
            }
            v.extend_from_slice(&[0, 0, 1]);
            v.extend_from_slice(n);
        }
        v
    }

    #[test]
    fn types_and_extraction() {
        for long in [true, false] {
            let s = annexb(&[SPS, PPS, IDR], long);
            assert_eq!(h264_nal_types(&s), vec![7, 8, 5]);
            let p = extract_h264_param_sets(&s).unwrap();
            assert_eq!(p, annexb(&[SPS, PPS], long));
        }
        assert!(extract_h264_param_sets(&annexb(&[SPS, IDR], true)).is_none());
        assert!(extract_h264_param_sets(&[]).is_none());
        assert!(extract_h264_param_sets(&[1, 2, 3]).is_none());
    }

    #[test]
    fn avcc_conversion() {
        let mut avcc = vec![1, 0x42, 0xc0, 0x1f, 0xff, 0xe1];
        avcc.extend_from_slice(&(SPS.len() as u16).to_be_bytes());
        avcc.extend_from_slice(SPS);
        avcc.push(1);
        avcc.extend_from_slice(&(PPS.len() as u16).to_be_bytes());
        avcc.extend_from_slice(PPS);
        let a = avcc_to_annexb(&avcc).unwrap();
        assert_eq!(a, annexb(&[SPS, PPS], true));
        assert_eq!(param_sets_from_extradata(&avcc).unwrap(), a);
        // Truncated records do not panic.
        assert!(avcc_to_annexb(&avcc[..avcc.len() - 2]).is_none());
        assert!(avcc_to_annexb(&[1, 2]).is_none());
    }

    #[test]
    fn param_sets_prepended_only_when_missing() {
        let params = annexb(&[SPS, PPS], true);
        let bare = annexb(&[IDR], true);
        let out = with_param_sets(bare.clone(), &params);
        assert_eq!(h264_nal_types(&out), vec![7, 8, 5]);
        let full = annexb(&[SPS, PPS, IDR], true);
        assert_eq!(with_param_sets(full.clone(), &params), full);
        assert_eq!(with_param_sets(bare.clone(), &[]), bare);
    }
}

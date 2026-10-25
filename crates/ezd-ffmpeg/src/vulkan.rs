//! Version-independent access to `AVVulkanDeviceContext` (its queue family fields changed in
//! FFmpeg 7.1).

use ffmpeg_sys_next as ffi;

/// Appends `idx` unless it is negative or already present.
fn push_unique(out: &mut Vec<u32>, idx: i64) {
    if let Ok(idx) = u32::try_from(idx)
        && !out.contains(&idx)
    {
        out.push(idx);
    }
}

/// Distinct non-negative indices of `indices`, in order of first appearance.
fn distinct_queue_families(indices: impl IntoIterator<Item = i32>) -> Vec<u32> {
    let mut out = Vec::new();
    for idx in indices {
        push_unique(&mut out, i64::from(idx));
    }
    out
}

/// Distinct queue family indices the Vulkan device was created with, in order of first
/// appearance.
#[cfg(ffmpeg_7_1)]
pub fn vulkan_queue_families(ctx: &ffi::AVVulkanDeviceContext) -> Vec<u32> {
    let n = ctx.nb_qf.clamp(0, 64) as usize;
    distinct_queue_families(ctx.qf[..n].iter().map(|q| q.idx))
}

/// Distinct queue family indices the Vulkan device was created with, in order of first
/// appearance (FFmpeg before 7.1: one field per queue kind, -1 when absent).
#[cfg(not(ffmpeg_7_1))]
#[allow(deprecated)]
pub fn vulkan_queue_families(ctx: &ffi::AVVulkanDeviceContext) -> Vec<u32> {
    distinct_queue_families([
        ctx.queue_family_index,
        ctx.queue_family_tx_index,
        ctx.queue_family_comp_index,
        ctx.queue_family_encode_index,
        ctx.queue_family_decode_index,
    ])
}

#[cfg(test)]
mod tests {
    use super::distinct_queue_families;

    #[test]
    fn keeps_order_and_drops_duplicates_and_negatives() {
        assert_eq!(distinct_queue_families([0, 2, 0, -1, 2, 1]), vec![0, 2, 1]);
        assert!(distinct_queue_families([-1, -1]).is_empty());
    }
}

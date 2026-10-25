//! Display-topology decisions that do not need Windows: whether the VDD shares its GDI source with
//! another monitor (Duplicate mode) and which CCD path to switch it to so it is extended instead.

/// What matters of one CCD path. Sources and targets are `(adapter LUID, id)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathKey {
    pub source: (u64, u32),
    pub target: (u64, u32),
    pub active: bool,
    pub target_is_vdd: bool,
}

/// Which paths to swap to take the VDD out of a clone group.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExtendPlan {
    /// `(active path to replace, inactive replacement path)`.
    pub replacements: Vec<(usize, usize)>,
    /// A cloned VDD path had no free source to move to.
    pub unresolved: bool,
}

/// True when one source drives the VDD and at least one other monitor.
/// `is_vdd` has one entry per monitor on the source.
pub fn source_is_cloned(is_vdd: &[bool]) -> bool {
    is_vdd.iter().any(|&v| v) && is_vdd.iter().any(|&v| !v)
}

/// For every active VDD path whose source is shared with an active non-VDD path, picks an inactive
/// path to the same target on a source no active path uses, preferring a source on the target's
/// own adapter. Paths of monitors that are not the VDD are never touched.
pub fn plan_extend(paths: &[PathKey]) -> ExtendPlan {
    let mut plan = ExtendPlan::default();
    let mut used: Vec<(u64, u32)> = paths
        .iter()
        .filter(|p| p.active)
        .map(|p| p.source)
        .collect();
    for (i, p) in paths.iter().enumerate() {
        let cloned = p.active
            && p.target_is_vdd
            && paths
                .iter()
                .any(|o| o.active && !o.target_is_vdd && o.source == p.source);
        if !cloned {
            continue;
        }
        let free = |j: &usize| {
            let c = &paths[*j];
            !c.active && c.target == p.target && !used.contains(&c.source)
        };
        let pick = (0..paths.len())
            .filter(free)
            .find(|&j| paths[j].source.0 == p.target.0)
            .or_else(|| (0..paths.len()).find(free));
        match pick {
            Some(j) => {
                used.push(paths[j].source);
                plan.replacements.push((i, j));
            }
            None => plan.unresolved = true,
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: u64 = 1;
    const B: u64 = 2;

    fn path(source: (u64, u32), target: (u64, u32), active: bool, vdd: bool) -> PathKey {
        PathKey {
            source,
            target,
            active,
            target_is_vdd: vdd,
        }
    }

    #[test]
    fn classifies_sources() {
        assert!(source_is_cloned(&[false, true]));
        assert!(!source_is_cloned(&[true]));
        assert!(!source_is_cloned(&[false, false]));
        assert!(!source_is_cloned(&[]));
    }

    #[test]
    fn cloned_vdd_moves_to_free_own_source() {
        let paths = [
            path((A, 0), (A, 10), true, false),
            path((A, 0), (A, 20), true, true),
            path((A, 1), (A, 20), false, true),
            path((A, 0), (A, 20), false, true),
        ];
        let plan = plan_extend(&paths);
        assert_eq!(plan.replacements, vec![(1, 2)]);
        assert!(!plan.unresolved);
    }

    #[test]
    fn other_adapter_source_only_when_no_own_one() {
        let paths = [
            path((A, 0), (A, 10), true, false),
            path((A, 0), (A, 20), true, true),
            path((B, 0), (A, 20), false, true),
        ];
        assert_eq!(plan_extend(&paths).replacements, vec![(1, 2)]);
        let both = [
            paths[0],
            paths[1],
            paths[2],
            path((A, 1), (A, 20), false, true),
        ];
        assert_eq!(plan_extend(&both).replacements, vec![(1, 3)]);
    }

    #[test]
    fn no_free_source_is_unresolved() {
        let paths = [
            path((A, 0), (A, 10), true, false),
            path((A, 0), (A, 20), true, true),
            path((A, 0), (A, 20), false, true),
        ];
        let plan = plan_extend(&paths);
        assert!(plan.replacements.is_empty());
        assert!(plan.unresolved);
    }

    #[test]
    fn extended_vdd_is_left_alone() {
        let paths = [
            path((A, 0), (A, 10), true, false),
            path((A, 1), (A, 20), true, true),
            path((A, 2), (A, 20), false, true),
        ];
        assert_eq!(plan_extend(&paths), ExtendPlan::default());
    }

    #[test]
    fn cloned_regular_monitors_are_untouched() {
        let paths = [
            path((A, 0), (A, 10), true, false),
            path((A, 0), (A, 11), true, false),
            path((A, 1), (A, 11), false, false),
        ];
        assert_eq!(plan_extend(&paths), ExtendPlan::default());
    }

    #[test]
    fn source_used_by_an_active_path_is_never_chosen() {
        let paths = [
            path((A, 0), (A, 10), true, false),
            path((A, 0), (A, 20), true, true),
            path((A, 1), (A, 30), true, false),
            path((A, 1), (A, 20), false, true),
        ];
        let plan = plan_extend(&paths);
        assert!(plan.replacements.is_empty());
        assert!(plan.unresolved);
    }
}

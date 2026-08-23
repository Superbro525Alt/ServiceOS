use core::fmt;
use core::fmt::Write;
use core::str;

use serviceos_abi::ServiceId;
use serviceos_bundle::{ServiceManifest, BOOT_STORE_MAX_DEPENDENCIES};

use crate::state::{MAX_SERVICE_SLOTS, ServiceSlot};
use crate::util::{fallback_logf, service_name};

pub(crate) const MAX_REFS: usize =
    BOOT_STORE_MAX_DEPENDENCIES + 4 + 16;

pub(crate) const SAFE_CORE: [ServiceId; 5] = [
    ServiceId::Storage,
    ServiceId::Console,
    ServiceId::Config,
    ServiceId::Log,
    ServiceId::Status,
];

pub(crate) const REDUCED_CORE: [ServiceId; 9] = [
    ServiceId::Storage,
    ServiceId::Console,
    ServiceId::Config,
    ServiceId::Log,
    ServiceId::Status,
    ServiceId::Shell,
    ServiceId::Package,
    ServiceId::Network,
    ServiceId::Security,
];

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(crate) enum BootMode {
    Full,
    Reduced,
    Safe,
}

impl BootMode {
    pub(crate) fn from_word(word: u64) -> Self {
        match word {
            1 => BootMode::Reduced,
            2 => BootMode::Safe,
            _ => BootMode::Full,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            BootMode::Full => "full",
            BootMode::Reduced => "reduced",
            BootMode::Safe => "safe",
        }
    }

    pub(crate) fn core_set(self) -> &'static [ServiceId] {
        match self {
            BootMode::Full => &[],
            BootMode::Reduced => &REDUCED_CORE,
            BootMode::Safe => &SAFE_CORE,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct RefTables {
    pub(crate) ids: [ServiceId; MAX_SERVICE_SLOTS],
    pub(crate) occupied: [bool; MAX_SERVICE_SLOTS],
    pub(crate) refs: [[ServiceId; MAX_REFS]; MAX_SERVICE_SLOTS],
    pub(crate) ref_counts: [usize; MAX_SERVICE_SLOTS],
}

impl RefTables {
    pub(crate) fn empty() -> Self {
        Self {
            ids: [ServiceId::RootManager; MAX_SERVICE_SLOTS],
            occupied: [false; MAX_SERVICE_SLOTS],
            refs: [[ServiceId::RootManager; MAX_REFS]; MAX_SERVICE_SLOTS],
            ref_counts: [0; MAX_SERVICE_SLOTS],
        }
    }

    pub(crate) fn capture(slots: &[ServiceSlot; MAX_SERVICE_SLOTS], service_count: usize) -> Self {
        let mut tables = RefTables::empty();
        for index in 0..service_count.min(MAX_SERVICE_SLOTS) {
            tables.ids[index] = slots[index].manifest.service_id;
            tables.occupied[index] = slots[index].occupied;
            tables.ref_counts[index] = manifest_refs(
                &slots[index].manifest,
                &mut tables.refs[index],
            );
        }
        tables
    }
}

pub(crate) fn manifest_refs(
    manifest: &ServiceManifest,
    out: &mut [ServiceId; MAX_REFS],
) -> usize {
    let mut count = 0usize;
    for dependency in manifest.dependencies[..manifest.dependency_count]
        .iter()
        .copied()
    {
        push_ref(out, &mut count, dependency);
    }
    for grant in manifest.grants[..manifest.grant_count].iter().copied() {
        push_ref(out, &mut count, grant.target);
    }
    for lookup in manifest.lookups[..manifest.lookup_count].iter().copied() {
        push_ref(out, &mut count, lookup.target);
    }
    count
}

fn push_ref(out: &mut [ServiceId; MAX_REFS], count: &mut usize, target: ServiceId) {
    if *count < out.len() {
        out[*count] = target;
        *count += 1;
    }
}

pub(crate) fn depends_on(
    refs: &[ServiceId; MAX_REFS],
    ref_count: usize,
    target: ServiceId,
) -> bool {
    if target == ServiceId::RootManager {
        return false;
    }
    refs[..ref_count].contains(&target)
}

/// Fixed-point transitive closure over service ids: seed indices plus every
/// occupied slot any kept slot references. Returns a bitmask of kept slots.
pub(crate) fn seed_closure_by_id(
    occupied: &[bool; MAX_SERVICE_SLOTS],
    ids: &[ServiceId; MAX_SERVICE_SLOTS],
    service_count: usize,
    seeds_mask: u32,
    refs: &[[ServiceId; MAX_REFS]; MAX_SERVICE_SLOTS],
    ref_counts: &[usize; MAX_SERVICE_SLOTS],
) -> u32 {
    let mut mask = seeds_mask;
    loop {
        let mut changed = false;
        for candidate in 0..service_count.min(MAX_SERVICE_SLOTS) {
            let bit = 1u32 << candidate;
            if !occupied[candidate] || mask & bit != 0 {
                continue;
            }
            let target = ids[candidate];
            let mut keep = false;
            for kept in 0..service_count.min(MAX_SERVICE_SLOTS) {
                if mask & (1u32 << kept) == 0 || kept == candidate {
                    continue;
                }
                if depends_on(&refs[kept], ref_counts[kept], target) {
                    keep = true;
                    break;
                }
            }
            if keep {
                mask |= bit;
                changed = true;
            }
        }
        if !changed {
            return mask;
        }
    }
}

pub(crate) fn apply_boot_mode(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    mode: BootMode,
) -> usize {
    let seeds = match mode.core_set() {
        [] => return occupied_count(slots, service_count),
        core => core,
    };
    let tables = RefTables::capture(slots, service_count);

    let mut seeds_mask = 0u32;
    for seed in seeds.iter().copied() {
        if let Some(index) = find_table_index(&tables, service_count, seed) {
            seeds_mask |= 1u32 << index;
        }
    }

    let keep = seed_closure_by_id(
        &tables.occupied,
        &tables.ids,
        service_count,
        seeds_mask,
        &tables.refs,
        &tables.ref_counts,
    );

    let mut dropped = 0usize;
    for index in 0..service_count.min(MAX_SERVICE_SLOTS) {
        if slots[index].occupied && keep & (1u32 << index) == 0 {
            slots[index].occupied = false;
            dropped = dropped.saturating_add(1);
        }
    }

    let kept = occupied_count(slots, service_count);
    let _ = fallback_logf(format_args!(
        "boot mode={} kept {} of {} services, skipped {}",
        mode.name(),
        kept,
        kept + dropped,
        dropped
    ));
    kept
}

fn find_table_index(tables: &RefTables, service_count: usize, id: ServiceId) -> Option<usize> {
    (0..service_count.min(MAX_SERVICE_SLOTS))
        .find(|index| tables.occupied[*index] && tables.ids[*index] == id)
}

fn occupied_count(slots: &[ServiceSlot; MAX_SERVICE_SLOTS], service_count: usize) -> usize {
    slots[..service_count].iter().filter(|slot| slot.occupied).count()
}

pub(crate) struct CyclePath {
    pub(crate) nodes: [ServiceId; MAX_SERVICE_SLOTS + 1],
    pub(crate) len: usize,
}

impl CyclePath {
    fn push(&mut self, id: ServiceId) {
        if self.len < self.nodes.len() {
            self.nodes[self.len] = id;
        }
        self.len = self.len.saturating_add(1);
    }
}

/// Walks the first-unready-dependency chain of every waiting slot and returns
/// the first cycle found as `A->B->...->A` node list.
pub(crate) fn find_blocked_cycle(
    waiting: &[bool; MAX_SERVICE_SLOTS],
    ids: &[ServiceId; MAX_SERVICE_SLOTS],
    blocked_on: &[ServiceId; MAX_SERVICE_SLOTS],
    service_count: usize,
) -> Option<CyclePath> {
    for start in 0..service_count.min(MAX_SERVICE_SLOTS) {
        if !waiting[start] {
            continue;
        }
        let mut visited = 0u32;
        let mut path = CyclePath {
            nodes: [ServiceId::RootManager; MAX_SERVICE_SLOTS + 1],
            len: 0,
        };
        let mut current = start;
        loop {
            if visited & (1u32 << current) != 0 {
                let head = ids[current];
                let cycle_start = (0..path.len)
                    .find(|position| path.nodes[(*position).min(MAX_SERVICE_SLOTS)] == head);
                if let Some(cycle_start) = cycle_start {
                    let mut cycle = CyclePath {
                        nodes: [ServiceId::RootManager; MAX_SERVICE_SLOTS + 1],
                        len: 0,
                    };
                    for position in cycle_start..path.len.min(MAX_SERVICE_SLOTS) {
                        cycle.push(path.nodes[position]);
                    }
                    cycle.push(head);
                    return Some(cycle);
                }
                break;
            }
            visited |= 1u32 << current;
            path.push(ids[current]);

            let next_id = blocked_on[current];
            if next_id == ServiceId::RootManager {
                break;
            }
            let Some(next) = (0..service_count.min(MAX_SERVICE_SLOTS)).find(|index| {
                ids[*index] == next_id && (waiting[*index] || visited & (1u32 << *index) != 0)
            }) else {
                break;
            };
            current = next;
        }
    }
    None
}

struct LogBuffer {
    bytes: [u8; 224],
    len: usize,
}

impl LogBuffer {
    fn new() -> Self {
        Self {
            bytes: [0u8; 224],
            len: 0,
        }
    }
}

impl Write for LogBuffer {
    fn write_str(&mut self, fragment: &str) -> fmt::Result {
        let remaining = self.bytes.len() - self.len;
        let amount = fragment.len().min(remaining);
        self.bytes[self.len..self.len + amount].copy_from_slice(fragment[..amount].as_bytes());
        self.len += amount;
        Ok(())
    }
}

pub(crate) fn log_cycle_path(cycle: &CyclePath) {
    let mut buffer = LogBuffer::new();
    let _ = write!(buffer, "dependency cycle detected:");
    let segments = cycle.len.min(cycle.nodes.len());
    for position in 0..segments {
        let _ = write!(buffer, " {}", service_name(cycle.nodes[position]));
        if position + 1 < segments {
            let _ = write!(buffer, " ->");
        }
    }
    if let Ok(text) = str::from_utf8(&buffer.bytes[..buffer.len]) {
        let _ = fallback_logf(format_args!("{}", text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables(ids: &[ServiceId], edges: &[(usize, ServiceId)]) -> (RefTables, usize) {
        let mut tables = RefTables::empty();
        let count = ids.len();
        for (index, id) in ids.iter().enumerate() {
            tables.ids[index] = *id;
            tables.occupied[index] = true;
        }
        for (from, target) in edges {
            let slot = tables.ref_counts[*from];
            tables.refs[*from][slot] = *target;
            tables.ref_counts[*from] = slot + 1;
        }
        (tables, count)
    }

    fn mask_of(tables: &RefTables, count: usize, seeds: &[ServiceId]) -> u32 {
        let mut seeds_mask = 0u32;
        for seed in seeds {
            for index in 0..count {
                if tables.ids[index] == *seed {
                    seeds_mask |= 1u32 << index;
                }
            }
        }
        seed_closure_by_id(
            &tables.occupied,
            &tables.ids,
            count,
            seeds_mask,
            &tables.refs,
            &tables.ref_counts,
        )
    }

    #[test]
    fn boot_mode_parses_words() {
        assert_eq!(BootMode::from_word(0), BootMode::Full);
        assert_eq!(BootMode::from_word(1), BootMode::Reduced);
        assert_eq!(BootMode::from_word(2), BootMode::Safe);
        assert_eq!(BootMode::from_word(99), BootMode::Full);
    }

    #[test]
    fn closure_keeps_direct_and_transitive_dependencies() {
        let (tables, count) = tables(
            &[ServiceId::Session, ServiceId::Graphics, ServiceId::Storage],
            &[(0, ServiceId::Graphics), (1, ServiceId::Storage)],
        );
        // Seed the top of the chain (session): graphics and storage must be
        // pulled back in transitively since session references them.
        let mask = mask_of(&tables, count, &[ServiceId::Session]);
        assert_eq!(mask, 0b111);
    }

    #[test]
    fn closure_leaves_unreferenced_services_dropped() {
        let (tables, count) = tables(
            &[ServiceId::Storage, ServiceId::Audio],
            &[],
        );
        let mask = mask_of(&tables, count, &[ServiceId::Storage]);
        assert_eq!(mask, 0b01);
    }

    #[test]
    fn closure_pulls_lookup_targets_back_in() {
        // Session looks up Clipboard even though there is no dependency edge.
        let mut tables = RefTables::empty();
        let ids = [ServiceId::Shell, ServiceId::Clipboard];
        for (index, id) in ids.iter().enumerate() {
            tables.ids[index] = *id;
            tables.occupied[index] = true;
        }
        tables.refs[0][0] = ServiceId::Clipboard;
        tables.ref_counts[0] = 1;
        let mask = mask_of(&tables, 2, &[ServiceId::Shell]);
        assert_eq!(mask, 0b11);
    }

    #[test]
    fn cycle_path_reports_full_loop() {
        let mut waiting = [false; MAX_SERVICE_SLOTS];
        let mut ids = [ServiceId::RootManager; MAX_SERVICE_SLOTS];
        let mut blocked_on = [ServiceId::RootManager; MAX_SERVICE_SLOTS];
        ids[0] = ServiceId::Graphics;
        ids[1] = ServiceId::Session;
        ids[2] = ServiceId::Storage;
        waiting[0] = true;
        waiting[1] = true;
        blocked_on[0] = ServiceId::Session;
        blocked_on[1] = ServiceId::Graphics;

        let cycle = find_blocked_cycle(&waiting, &ids, &blocked_on, 4)
            .expect("cycle between graphics and session");
        assert_eq!(cycle.len, 3);
        assert_eq!(cycle.nodes[0], ServiceId::Graphics);
        assert_eq!(cycle.nodes[1], ServiceId::Session);
        assert_eq!(cycle.nodes[2], ServiceId::Graphics);
    }

    #[test]
    fn open_wait_chain_is_not_a_cycle() {
        let mut waiting = [false; MAX_SERVICE_SLOTS];
        let mut ids = [ServiceId::RootManager; MAX_SERVICE_SLOTS];
        let mut blocked_on = [ServiceId::RootManager; MAX_SERVICE_SLOTS];
        ids[0] = ServiceId::Shell;
        ids[1] = ServiceId::Storage;
        waiting[0] = true;
        blocked_on[0] = ServiceId::Storage;
        // storage is present but never waiting -> chain terminates, no cycle
        assert!(find_blocked_cycle(&waiting, &ids, &blocked_on, 2).is_none());
    }
}

//! Per-source staged-rollout cohorts and upgrade rules.
//!
//! Update eligibility becomes a pure policy decision combining the existing
//! channel/ring gates with (a) a deterministic per-source cohort assignment
//! and (b) per-source rules (held packages, minimum ring floor, version-step
//! cap). Everything here is host-testable: the gate function takes the
//! policy row explicitly and the codec is plain text, following the
//! `feed-keys.cfg` / `feed-journal.cfg` persistence pattern.
//!
//! DEFAULTS: a source with no configured row (or an empty table) admits
//! every target, so update behavior is byte-identical to the pre-rollout
//! service. Tests below pin that passthrough.

use crate::signing::FixedText;
use crate::sysupdate_model::parse_version_triplet;
use rt::PackageRing;

use core::fmt::Write;
use serviceos_userspace_runtime as rt;

pub const ROLLOUT_SOURCE_MAX: usize = crate::signing::SOURCE_NAME_MAX;
pub const COHORT_NAME_MAX: usize = 24;
pub const HOLD_NAME_MAX: usize = 24;
pub const MAX_HOLD: usize = 6;
pub const POLICY_SOURCES_MAX: usize = 8;

/// Weighted dotted-version step distance (major*10000 + minor*100 + patch),
/// reusing the existing triplet parse. Saturating; target must already be
/// Greater per `compare_versions` when consulted.
pub const STEP_MAJOR_WEIGHT: u64 = 10_000;
pub const STEP_MINOR_WEIGHT: u64 = 100;

/// RolloutSetRequest operation words.
pub const ROLLOUT_OP_COHORT: u64 = 1;
pub const ROLLOUT_OP_HOLD_ADD: u64 = 2;
pub const ROLLOUT_OP_HOLD_REMOVE: u64 = 3;
pub const ROLLOUT_OP_HOLD_CLEAR: u64 = 4;
pub const ROLLOUT_OP_MIN_RING: u64 = 5;
pub const ROLLOUT_OP_MAX_STEP: u64 = 6;
pub const ROLLOUT_OP_CLEAR: u64 = 7;

/// Gate outcome words shared on the wire (RolloutStatusReply) and in tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RolloutReason {
    NoUpdate = 0,
    Admit = 1,
    Held = 2,
    CohortOut = 3,
    RingFloor = 4,
    StepCap = 5,
}

/// Staging cohort for one source: optional name (`name:percent` grammar)
/// plus the eligible percentage of installs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CohortSpec {
    pub name: FixedText<COHORT_NAME_MAX>,
    pub percent: u32,
}

impl CohortSpec {
    pub const fn open() -> Self {
        Self {
            name: FixedText::empty(),
            percent: 100,
        }
    }
}

/// Full per-source policy row. Unset fields fall back to permissive
/// defaults (open cohort, Production floor, unlimited step, no holds).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceRollout {
    pub source: FixedText<ROLLOUT_SOURCE_MAX>,
    pub cohort: CohortSpec,
    pub min_ring: PackageRing,
    pub max_step: u32,
    pub hold_count: usize,
    pub hold: [FixedText<HOLD_NAME_MAX>; MAX_HOLD],
}

impl SourceRollout {
    pub const fn empty() -> Self {
        Self {
            source: FixedText::empty(),
            cohort: CohortSpec::open(),
            min_ring: PackageRing::Production,
            max_step: 0,
            hold_count: 0,
            hold: [FixedText::empty(); MAX_HOLD],
        }
    }

    pub fn is_held(&self, package: &str) -> bool {
        (0..self.hold_count).any(|index| self.hold[index].as_str() == package)
    }

    pub fn hold_add(&mut self, package: &str) -> bool {
        if self.is_held(package) {
            return true;
        }
        if self.hold_count >= MAX_HOLD || package.is_empty() || package.len() > HOLD_NAME_MAX {
            return false;
        }
        let _ = self.hold[self.hold_count].set(package);
        self.hold_count += 1;
        true
    }

    pub fn hold_remove(&mut self, package: &str) -> bool {
        for index in 0..self.hold_count {
            if self.hold[index].as_str() == package {
                let last = self.hold_count - 1;
                self.hold[index] = self.hold[last];
                self.hold[last] = FixedText::empty();
                self.hold_count = last;
                return true;
            }
        }
        false
    }

    pub fn hold_clear(&mut self) {
        self.hold = [FixedText::empty(); MAX_HOLD];
        self.hold_count = 0;
    }
}

/// Whole policy table persisted under `state/packages/policy.cfg`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RolloutPolicy {
    pub sources: [SourceRollout; POLICY_SOURCES_MAX],
    pub count: usize,
}

impl RolloutPolicy {
    pub const fn empty() -> Self {
        Self {
            sources: [SourceRollout::empty(); POLICY_SOURCES_MAX],
            count: 0,
        }
    }

    pub fn source_rollout(&self, source: &str) -> Option<&SourceRollout> {
        (0..self.count)
            .map(|index| &self.sources[index])
            .find(|row| row.source.as_str() == source)
    }

    /// Mutable lookup of an EXISTING row; no insertion. Use
    /// `source_or_insert` when the caller may create the row.
    pub fn source_rollout_mut(&mut self, source: &str) -> Option<&mut SourceRollout> {
        let index = (0..self.count).find(|index| self.sources[*index].source.as_str() == source)?;
        self.sources.get_mut(index)
    }

    /// Row for `source`, creating an empty permissive one when missing.
    /// Returns None when the table is full or the name is not usable.
    pub fn source_or_insert(&mut self, source: &str) -> Option<&mut SourceRollout> {
        if let Some(index) =
            (0..self.count).find(|index| self.sources[*index].source.as_str() == source)
        {
            return self.sources.get_mut(index);
        }
        if self.count >= POLICY_SOURCES_MAX
            || source.is_empty()
            || source.len() > ROLLOUT_SOURCE_MAX
        {
            return None;
        }
        let slot = self.count;
        self.sources[slot] = SourceRollout::empty();
        let _ = self.sources[slot].source.set(source);
        self.count += 1;
        self.sources.get_mut(slot)
    }

    pub fn remove_source(&mut self, source: &str) -> bool {
        for index in 0..self.count {
            if self.sources[index].source.as_str() == source {
                let last = self.count - 1;
                self.sources[index] = self.sources[last];
                self.sources[last] = SourceRollout::empty();
                self.count = last;
                return true;
            }
        }
        false
    }
}

/// Deterministic cohort bucket for one source+package(+cohort name).
/// FNV-1a64 over the NUL-free fields joined by 0x1f; the same inputs always
/// land in the same bucket, so membership is stable across boots.
pub fn cohort_bucket(source: &str, package: &str, cohort_name: &str) -> u64 {
    let mut seed = [0u8; ROLLOUT_SOURCE_MAX + HOLD_NAME_MAX + COHORT_NAME_MAX + 2];
    let mut len = 0usize;
    for field in [source, package, cohort_name] {
        if len > 0 {
            seed[len] = 0x1f;
            len += 1;
        }
        let bytes = field.as_bytes();
        let room = seed.len() - len;
        let take = bytes.len().min(room);
        seed[len..len + take].copy_from_slice(&bytes[..take]);
        len += take;
    }
    crate::signing::fnv1a64(&seed[..len])
}

/// Cohort membership: eligible iff the stable bucket falls below the
/// configured percent. percent=100 always admits; percent=0 never does.
pub fn cohort_member(cohort: &CohortSpec, source: &str, package: &str) -> bool {
    if cohort.percent >= 100 {
        return true;
    }
    let name = cohort.name.as_str();
    cohort_bucket(source, package, name) % 100 < u64::from(cohort.percent)
}

/// Weighted dotted-version jump distance, saturating. Non-numeric components
/// parse as zero (existing triplet behavior).
pub fn version_step_distance(from: &str, to: &str) -> u64 {
    let (fmaj, fmin, fpat) = parse_version_triplet(from);
    let (tmaj, tmin, tpat) = parse_version_triplet(to);
    (u64::from(tmaj.saturating_sub(fmaj)) * STEP_MAJOR_WEIGHT)
        + (u64::from(tmin.saturating_sub(fmin)) * STEP_MINOR_WEIGHT)
        + u64::from(tpat.saturating_sub(fpat))
}

pub fn ring_rank(ring: PackageRing) -> u32 {
    match ring {
        PackageRing::Production => 0,
        PackageRing::Preview => 1,
        PackageRing::Testing => 2,
    }
}

/// The pure update-gate decision for one target. `policy` of None (source
/// without configured rules) admits everything. Check order — held, cohort,
/// ring floor, step cap — is part of the contract and pinned by tests.
pub fn evaluate_update_gate(
    policy: Option<&SourceRollout>,
    source: &str,
    package: &str,
    target_ring: PackageRing,
    target_version: &str,
    installed_version: &str,
) -> RolloutReason {
    let Some(policy) = policy else {
        return RolloutReason::Admit;
    };
    if policy.is_held(package) {
        return RolloutReason::Held;
    }
    if !cohort_member(&policy.cohort, source, package) {
        return RolloutReason::CohortOut;
    }
    if ring_rank(target_ring) < ring_rank(policy.min_ring) {
        return RolloutReason::RingFloor;
    }
    if policy.max_step > 0
        && version_step_distance(installed_version, target_version) > u64::from(policy.max_step)
    {
        return RolloutReason::StepCap;
    }
    RolloutReason::Admit
}

// ---------------------------------------------------------------------------
// policy.cfg codec: one `rollout=` row per source
//
// rollout=<source>|<percent>|<cohort_name>|<min_ring>|<max_step>|<hold_csv>
// ---------------------------------------------------------------------------

pub fn parse_cohort_argument(value: &str) -> Option<CohortSpec> {
    if value == "none" {
        return Some(CohortSpec::open());
    }
    let (name, percent_text) = match value.split_once(':') {
        Some((name, percent)) => (name, percent),
        None => ("", value),
    };
    if name.len() > COHORT_NAME_MAX || name.contains('|') || name.contains(',') {
        return None;
    }
    let percent = percent_text.parse::<u32>().ok()?;
    if percent > 100 {
        return None;
    }
    let mut spec = CohortSpec::open();
    let _ = spec.name.set(name);
    spec.percent = percent;
    Some(spec)
}

pub fn parse_hold_csv(text: &str) -> ([FixedText<HOLD_NAME_MAX>; MAX_HOLD], usize) {
    let mut hold = [FixedText::<HOLD_NAME_MAX>::empty(); MAX_HOLD];
    let mut count = 0usize;
    for name in text.split(',').filter(|name| !name.is_empty()) {
        if count >= MAX_HOLD {
            break;
        }
        let _ = hold[count].set(name);
        count += 1;
    }
    (hold, count)
}

/// Parse a persisted policy file. Unknown lines are skipped so older or
/// newer writers stay compatible (additive-file convention).
pub fn parse_policy(text: &str) -> RolloutPolicy {
    let mut policy = RolloutPolicy::empty();
    for line in text.lines() {
        let Some(row) = line.strip_prefix("rollout=") else {
            continue;
        };
        let mut parts = row.split('|');
        let source = parts.next().unwrap_or("");
        let percent = parts.next().and_then(|v| v.parse::<u32>().ok());
        let cohort_name = parts.next().unwrap_or("");
        let min_ring = parts
            .next()
            .and_then(|v| v.parse::<u32>().ok())
            .and_then(ring_from_word);
        let max_step = parts.next().and_then(|v| v.parse::<u32>().ok());
        let hold_csv = parts.next().unwrap_or("");
        if source.is_empty() || source.len() > ROLLOUT_SOURCE_MAX {
            continue;
        }
        if policy.count >= POLICY_SOURCES_MAX {
            break;
        }
        let slot = policy.count;
        let entry = &mut policy.sources[slot];
        let _ = entry.source.set(source);
        if let Some(percent) = percent.filter(|p| *p <= 100) {
            entry.cohort.percent = percent;
        }
        if cohort_name.len() <= COHORT_NAME_MAX {
            let _ = entry.cohort.name.set(cohort_name);
        }
        if let Some(ring) = min_ring {
            entry.min_ring = ring;
        }
        if let Some(step) = max_step {
            entry.max_step = step;
        }
        let (hold, count) = parse_hold_csv(hold_csv);
        entry.hold = hold;
        entry.hold_count = count;
        policy.count += 1;
    }
    policy
}

pub fn ring_from_word(word: u32) -> Option<PackageRing> {
    match word {
        1 => Some(PackageRing::Production),
        2 => Some(PackageRing::Preview),
        3 => Some(PackageRing::Testing),
        _ => None,
    }
}

pub fn ring_word(ring: PackageRing) -> u32 {
    ring as u32
}

pub fn serialize_policy<const N: usize>(policy: &RolloutPolicy, out: &mut rt::FixedLogBuffer<N>) {
    let _ = write!(out, "version=1\n");
    for row in policy.sources[..policy.count].iter() {
        let _ = write!(out, "rollout={}", row.source.as_str());
        let _ = write!(out, "|{}", row.cohort.percent);
        let _ = write!(out, "|{}", row.cohort.name.as_str());
        let _ = write!(out, "|{}", row.min_ring as u32);
        let _ = write!(out, "|{}", row.max_step);
        let _ = write!(out, "|");
        for index in 0..row.hold_count {
            if index > 0 {
                let _ = write!(out, ",");
            }
            let _ = write!(out, "{}", row.hold[index].as_str());
        }
        let _ = write!(out, "\n");
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    fn row(source: &str) -> SourceRollout {
        let mut policy = RolloutPolicy::empty();
        policy.source_or_insert(source).unwrap().clone()
    }

    #[test]
    fn empty_policy_admits_everything() {
        for source in ["", "boot", "edge"] {
            for ring in [
                PackageRing::Production,
                PackageRing::Preview,
                PackageRing::Testing,
            ] {
                assert_eq!(
                    evaluate_update_gate(None, source, "pkg", ring, "9.9.9", "1.0.0"),
                    RolloutReason::Admit
                );
            }
        }
    }

    #[test]
    fn default_row_admits_everything() {
        let policy = row("edge");
        assert_eq!(
            evaluate_update_gate(
                Some(&policy),
                "edge",
                "netd",
                PackageRing::Testing,
                "9.9.9",
                "0.0.1"
            ),
            RolloutReason::Admit
        );
    }

    #[test]
    fn cohort_percent_bounds_are_absolute() {
        let mut policy = row("edge");
        policy.cohort.percent = 100;
        assert!(cohort_member(&policy.cohort, "edge", "anything"));
        policy.cohort.percent = 0;
        assert!(!cohort_member(&policy.cohort, "edge", "anything"));
    }

    #[test]
    fn cohort_membership_is_deterministic_and_matches_bucket() {
        let bucket = cohort_bucket("edge", "netd", "");
        for percent in [0u32, 1, 37, 50, 99, 100] {
            let mut policy = row("edge");
            policy.cohort.percent = percent;
            assert_eq!(
                cohort_member(&policy.cohort, "edge", "netd"),
                bucket % 100 < u64::from(percent)
            );
        }
        assert_eq!(cohort_bucket("edge", "netd", ""), bucket);
    }

    #[test]
    fn cohort_membership_varies_by_package_and_name_but_stays_stable() {
        let mut policy = row("edge");
        policy.cohort.percent = 50;
        let same = cohort_member(&policy.cohort, "edge", "netd");
        assert_eq!(same, cohort_member(&policy.cohort, "edge", "netd"));
        let mut named = policy;
        let _ = named.cohort.name.set("wave");
        assert_eq!(named.cohort.percent, 50, "clone keeps percent until edited");
        // Anonymous vs named buckets differ for at least one package sample.
        let samples = ["a", "b", "c", "d", "e", "f", "g", "h"];
        let differs = samples.iter().any(|pkg| {
            cohort_member(&policy.cohort, "edge", pkg) != cohort_member(&named.cohort, "edge", pkg)
        });
        assert!(differs, "named cohort must re-randomize membership");
        // Changing the package, not the boot, changes membership odds but the
        // per-package decision itself is stable across calls.
        for pkg in samples {
            let first = cohort_member(&policy.cohort, "edge", pkg);
            assert_eq!(first, cohort_member(&policy.cohort, "edge", pkg));
        }
    }

    #[test]
    fn held_packages_are_rejected_before_any_other_rule() {
        let mut policy = row("edge");
        assert!(policy.hold_add("netd"));
        policy.cohort.percent = 0;
        policy.min_ring = PackageRing::Testing;
        policy.max_step = 1;
        assert_eq!(
            evaluate_update_gate(
                Some(&policy),
                "edge",
                "netd",
                PackageRing::Production,
                "9.9.9",
                "1.0.0"
            ),
            RolloutReason::Held
        );
    }

    #[test]
    fn cohort_out_blocks_ungated_target() {
        let mut policy = row("edge");
        // Percent 0 pins every package out of the cohort deterministically.
        policy.cohort.percent = 0;
        assert_eq!(
            evaluate_update_gate(
                Some(&policy),
                "edge",
                "netd",
                PackageRing::Production,
                "1.2.0",
                "1.0.0"
            ),
            RolloutReason::CohortOut
        );
    }

    #[test]
    fn min_ring_floor_blocks_lower_rings_only() {
        let mut policy = row("edge");
        policy.min_ring = PackageRing::Preview;
        let gated =
            |ring| evaluate_update_gate(Some(&policy), "edge", "netd", ring, "1.2.0", "1.0.0");
        assert_eq!(gated(PackageRing::Production), RolloutReason::RingFloor);
        assert_eq!(gated(PackageRing::Preview), RolloutReason::Admit);
        assert_eq!(gated(PackageRing::Testing), RolloutReason::Admit);
    }

    #[test]
    fn max_step_caps_version_jump_distance() {
        let mut policy = row("edge");
        policy.max_step = 5;
        let gated = |from, to| {
            evaluate_update_gate(
                Some(&policy),
                "edge",
                "netd",
                PackageRing::Production,
                to,
                from,
            )
        };
        assert_eq!(gated("1.2.3", "1.2.8"), RolloutReason::Admit);
        assert_eq!(gated("1.2.3", "1.2.9"), RolloutReason::StepCap);
        assert_eq!(gated("1.2.3", "1.3.0"), RolloutReason::StepCap);
        assert_eq!(gated("1.2.3", "2.0.0"), RolloutReason::StepCap);
        policy.max_step = 0;
        assert_eq!(
            evaluate_update_gate(
                Some(&policy),
                "edge",
                "netd",
                PackageRing::Production,
                "9.0.0",
                "1.0.0"
            ),
            RolloutReason::Admit
        );
    }

    #[test]
    fn rule_precedence_is_hold_then_cohort_then_ring_then_step() {
        let mut policy = row("edge");
        policy.cohort.percent = 0;
        policy.min_ring = PackageRing::Testing;
        policy.max_step = 1;
        // Held wins over everything.
        assert!(policy.hold_add("netd"));
        assert_eq!(
            evaluate_update_gate(
                Some(&policy),
                "edge",
                "netd",
                PackageRing::Production,
                "9.0.0",
                "1.0.0"
            ),
            RolloutReason::Held
        );
        // Cohort beats ring floor and step cap.
        assert!(policy.hold_remove("netd"));
        assert_eq!(
            evaluate_update_gate(
                Some(&policy),
                "edge",
                "netd",
                PackageRing::Production,
                "9.0.0",
                "1.0.0"
            ),
            RolloutReason::CohortOut
        );
        // Ring floor beats step cap.
        policy.cohort.percent = 100;
        assert_eq!(
            evaluate_update_gate(
                Some(&policy),
                "edge",
                "netd",
                PackageRing::Production,
                "9.0.0",
                "1.0.0"
            ),
            RolloutReason::RingFloor
        );
        // Step cap fires last.
        policy.min_ring = PackageRing::Production;
        assert_eq!(
            evaluate_update_gate(
                Some(&policy),
                "edge",
                "netd",
                PackageRing::Production,
                "9.0.0",
                "1.0.0"
            ),
            RolloutReason::StepCap
        );
    }

    #[test]
    fn hold_add_remove_roundtrip_with_capacity_cap() {
        let mut policy = row("edge");
        for name in ["a", "b", "c", "d", "e"] {
            assert!(policy.hold_add(name));
        }
        assert!(policy.hold_add("f"), "MAX_HOLD entries must all fit");
        assert!(!policy.hold_add("g"), "cap MAX_HOLD must reject the 7th");
        assert!(!policy.hold_add(""));
        assert!(policy.is_held("a"));
        assert!(policy.hold_remove("c"));
        assert!(!policy.is_held("c"));
        assert!(!policy.hold_remove("c"), "double remove reports false");
        policy.hold_clear();
        assert!(!policy.is_held("a"));
        assert_eq!(policy.hold_count, 0);
        assert!(policy.hold_add("z"));
        assert_eq!(policy.hold_count, 1);
    }

    #[test]
    fn policy_table_upsert_remove_and_caps() {
        let mut policy = RolloutPolicy::empty();
        assert!(policy.source_rollout("edge").is_none());
        policy.source_or_insert("edge").unwrap().max_step = 5;
        policy.source_or_insert("edge").unwrap().max_step = 7;
        assert_eq!(policy.count, 1, "upsert must not duplicate rows");
        assert_eq!(policy.source_rollout("edge").unwrap().max_step, 7);
        for index in 0..POLICY_SOURCES_MAX - 1 {
            let name = std::format!("s{index}");
            assert!(policy.source_or_insert(&name).is_some());
        }
        assert_eq!(policy.count, POLICY_SOURCES_MAX);
        assert!(policy.source_or_insert("overflow").is_none());
        assert!(policy.source_or_insert("").is_none());
        assert!(policy.remove_source("s1"));
        assert_eq!(policy.count, POLICY_SOURCES_MAX - 1);
        assert!(policy.source_rollout("s1").is_none());
        assert!(policy.source_rollout("edge").is_some());
    }

    #[test]
    fn cohort_argument_grammar() {
        let open = parse_cohort_argument("none").unwrap();
        assert_eq!(open.percent, 100);
        assert!(open.name.is_empty());
        let anon = parse_cohort_argument("25").unwrap();
        assert_eq!(anon.percent, 25);
        assert!(anon.name.is_empty());
        let named = parse_cohort_argument("wave:40").unwrap();
        assert_eq!(named.percent, 40);
        assert_eq!(named.name.as_str(), "wave");
        assert!(parse_cohort_argument("101").is_none());
        assert!(parse_cohort_argument("wave:101").is_none());
        assert!(parse_cohort_argument("wave:").is_none());
        assert!(parse_cohort_argument("-1").is_none());
        let max_ok = "x".repeat(COHORT_NAME_MAX);
        assert!(parse_cohort_argument(&std::format!("{max_ok}:10")).is_some());
        let too_long = "x".repeat(COHORT_NAME_MAX + 1);
        assert!(parse_cohort_argument(&too_long).is_none());
        assert!(parse_cohort_argument(&std::format!("{too_long}:10")).is_none());
    }

    #[test]
    fn policy_codec_roundtrip_preserves_all_rules() {
        let mut policy = RolloutPolicy::empty();
        {
            let edge = policy.source_or_insert("edge").unwrap();
            edge.cohort.percent = 25;
            let _ = edge.cohort.name.set("wave");
            edge.min_ring = PackageRing::Preview;
            edge.max_step = 300;
            assert!(edge.hold_add("netd"));
            assert!(edge.hold_add("logger"));
        }
        {
            let boot = policy.source_or_insert("boot").unwrap();
            boot.cohort.percent = 0;
        }
        let mut text = rt::FixedLogBuffer::<2048>::new();
        serialize_policy(&policy, &mut text);
        let parsed = parse_policy(text.as_str());
        assert_eq!(parsed.count, 2);
        let edge = parsed.source_rollout("edge").unwrap();
        assert_eq!(edge.cohort.percent, 25);
        assert_eq!(edge.cohort.name.as_str(), "wave");
        assert_eq!(edge.min_ring, PackageRing::Preview);
        assert_eq!(edge.max_step, 300);
        assert!(edge.is_held("netd") && edge.is_held("logger"));
        assert_eq!(edge.hold_count, 2);
        let boot = parsed.source_rollout("boot").unwrap();
        assert_eq!(boot.cohort.percent, 0);
        assert!(boot.cohort.name.is_empty());
        assert_eq!(boot.hold_count, 0);
    }

    #[test]
    fn source_rollout_mut_edits_existing_row_without_inserting() {
        let mut policy = RolloutPolicy::empty();
        assert!(policy.source_rollout_mut("edge").is_none());
        policy.source_or_insert("edge").unwrap();
        policy.source_rollout_mut("edge").unwrap().hold_add("netd");
        assert_eq!(policy.count, 1);
        assert!(policy.source_rollout("edge").unwrap().is_held("netd"));
        assert!(policy.source_rollout_mut("ghost").is_none());
    }

    #[test]
    fn policy_parser_skips_malformed_and_unknown_lines() {
        let parsed = parse_policy(
            "version=1\nrollout=|50|\nrollout=ok|101|\nrollout=ok2|50|||7|\ngarbage\nrollout=ok3|50||1|0|h1,h2,h3",
        );
        assert_eq!(parsed.count, 3);
        // Out-of-range percent is ignored (open default), never trusted.
        assert_eq!(parsed.source_rollout("ok").unwrap().cohort.percent, 100);
        let ok2 = parsed.source_rollout("ok2").unwrap();
        assert_eq!(ok2.cohort.percent, 50);
        assert_eq!(ok2.max_step, 7);
        assert_eq!(ok2.hold_count, 0);
        let ok3 = parsed.source_rollout("ok3").unwrap();
        assert_eq!(ok3.hold_count, 3);
        assert_eq!(ok3.min_ring, PackageRing::Production);
    }

    #[test]
    fn empty_policy_file_parses_to_default_table() {
        let parsed = parse_policy("version=1\n");
        assert_eq!(parsed.count, 0);
        assert!(parsed.source_rollout("boot").is_none());
    }

    #[test]
    fn step_distance_is_weighted_and_saturating() {
        assert_eq!(version_step_distance("1.2.3", "1.2.4"), 1);
        assert_eq!(version_step_distance("1.2.3", "1.2.9"), 6);
        assert_eq!(version_step_distance("1.2.3", "1.3.0"), 100);
        assert_eq!(version_step_distance("1.2.3", "2.0.0"), 10_000);
        assert_eq!(version_step_distance("1.2.3", "1.2.3"), 0);
        assert_eq!(version_step_distance("9.9.9", "99.0.0"), 900_000);
    }
}

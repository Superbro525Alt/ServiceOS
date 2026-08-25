//! Static catalog metadata side-table plus search/filter/compatibility helpers.
//!
//! The package manifest parser (`shared/bundle`) rejects unknown keys, so rich
//! per-package metadata lives here instead of in `package.pkg` descriptors.
//! Values mirror facts already present under `userspace/bundles/`.

use serviceos_userspace_runtime as rt;

use rt::ServiceId;

pub(crate) const MAX_QUERY_BYTES: usize = 32;

#[derive(Clone, Copy)]
pub(crate) struct PackageMeta {
    pub(crate) category: &'static str,
    pub(crate) description: &'static str,
    pub(crate) keywords: &'static [&'static str],
    pub(crate) targets: &'static [&'static str],
    pub(crate) screenshot_ref: &'static str,
    /// Editorial placeholder rating in half-star tenths out of
    /// [`MAX_RATING_TENTHS`] (50 = 5 stars). No telemetry source feeds this;
    /// values are static curation until a real signal exists.
    pub(crate) rating_tenths: u16,
}

pub(crate) const HOST_TARGET: &str = if cfg!(target_arch = "aarch64") {
    "aarch64"
} else {
    "x86_64"
};

pub(crate) const TARGET_X86_64: &str = "x86_64";
pub(crate) const TARGET_AARCH64: &str = "aarch64";

const UNIVERSAL_TARGETS: &[&str] = &[TARGET_X86_64, TARGET_AARCH64];

pub(crate) fn meta_for(service_id: ServiceId) -> Option<&'static PackageMeta> {
    match service_id {
        ServiceId::Announce => Some(&PackageMeta {
            category: "Messaging",
            description: "Broadcast announcement messages to subscribed desktop listeners.",
            keywords: &["announce", "notification", "broadcast", "message"],
            targets: UNIVERSAL_TARGETS,
            screenshot_ref: "announce-desktop-broadcast",
            rating_tenths: 42,
        }),
        ServiceId::Runtime => Some(&PackageMeta {
            category: "Runtime",
            description: "POSIX-style runtime root with shell tools for hosted workloads.",
            keywords: &["runtime", "posix", "workload", "sandbox", "shell"],
            targets: UNIVERSAL_TARGETS,
            screenshot_ref: "",
            rating_tenths: 46,
        }),
        ServiceId::Developer => Some(&PackageMeta {
            category: "Development",
            description: "Cross-platform SDK with toolchains and sample workspaces.",
            keywords: &["sdk", "toolchain", "development", "compiler", "workspace"],
            targets: UNIVERSAL_TARGETS,
            screenshot_ref: "developer-sdk-workspace",
            rating_tenths: 38,
        }),
        _ => None,
    }
}

/// Metadata used for search ranking. Built from the static side-table when one
/// exists, otherwise derived from the feed-provided category bytes.
#[derive(Clone, Copy)]
pub(crate) struct SearchDoc {
    pub(crate) name: &'static str,
    pub(crate) category: &'static str,
    pub(crate) description: &'static str,
    pub(crate) keywords: &'static [&'static str],
}

pub(crate) fn doc_for(service_id: ServiceId, feed_category: &[u8]) -> SearchDoc {
    match meta_for(service_id) {
        Some(meta) => SearchDoc {
            name: crate::state::service_label(service_id),
            category: meta.category,
            description: meta.description,
            keywords: meta.keywords,
        },
        None => SearchDoc {
            name: crate::state::service_label(service_id),
            category: feed_category_text(feed_category),
            description: "",
            keywords: &[],
        },
    }
}

fn feed_category_text(bytes: &[u8]) -> &'static str {
    // Feed categories for boot packages mirror the package name rather than a
    // real taxonomy; anything without a side-table entry is a system package.
    let _ = bytes;
    "System"
}

pub(crate) fn category_for(service_id: ServiceId, feed_category: &[u8]) -> &'static str {
    doc_for(service_id, feed_category).category
}

pub(crate) fn description_for(service_id: ServiceId) -> &'static str {
    meta_for(service_id)
        .map(|meta| meta.description)
        .unwrap_or("")
}

pub(crate) fn targets_for(service_id: ServiceId) -> &'static [&'static str] {
    meta_for(service_id)
        .map(|meta| meta.targets)
        .unwrap_or(UNIVERSAL_TARGETS)
}

pub(crate) fn screenshot_ref_for(service_id: ServiceId) -> &'static str {
    meta_for(service_id)
        .map(|meta| meta.screenshot_ref)
        .unwrap_or("")
}

pub(crate) const MAX_RATING_TENTHS: u16 = 50;

/// Clamp a raw rating to the displayable 1..=50 tenths-of-star range.
/// Zero means "no rating yet" and maps to `None`.
pub(crate) fn clamp_rating_tenths(raw: u16) -> Option<u8> {
    if raw == 0 {
        return None;
    }
    Some(raw.min(MAX_RATING_TENTHS) as u8)
}

pub(crate) fn rating_tenths_for(service_id: ServiceId) -> Option<u8> {
    meta_for(service_id)
        .and_then(|meta| clamp_rating_tenths(meta.rating_tenths))
}

/// Map tenths of a star to five ASCII star slots, rounding half-up
/// (25 tenths -> three filled). Filled slots are `*`, empty ones `-`.
pub(crate) fn star_bar(tenths: u8) -> [u8; 5] {
    let clamped = u16::from(clamp_rating_tenths(u16::from(tenths)).unwrap_or(0));
    let filled = ((clamped + 5) / 10) as usize;
    let mut bar = [b'-'; 5];
    for slot in bar.iter_mut().take(filled.min(5)) {
        *slot = b'*';
    }
    bar
}

/// Stylized placeholder headlines for screenshot cards. The framebuffer text
/// stack cannot decode images, so a reference renders as an honestly labeled
/// card; the headline is picked deterministically from the reference bytes.
pub(crate) const SCREENSHOT_HEADLINES: [&str; 3] =
    ["PACKAGE SCREENSHOT", "APP PREVIEW", "SEE IT IN ACTION"];

fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

pub(crate) fn screenshot_placeholder_headline(screenshot_ref: &str) -> Option<&'static str> {
    if screenshot_ref.is_empty() {
        return None;
    }
    let index = fnv1a(screenshot_ref.as_bytes()) as usize % SCREENSHOT_HEADLINES.len();
    Some(SCREENSHOT_HEADLINES[index])
}

/// Per-catalog-entry facts recommendation scoring needs. Built from search
/// docs so entries without side-table metadata still participate.
#[derive(Clone, Copy)]
pub(crate) struct RecommendInput {
    pub(crate) category: &'static str,
    pub(crate) keywords: &'static [&'static str],
    pub(crate) installed: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct Recommendation {
    pub(crate) index: usize,
    pub(crate) category_hits: u16,
    pub(crate) keyword_hits: u16,
}

impl Recommendation {
    pub(crate) const EMPTY: Self = Self {
        index: 0,
        category_hits: 0,
        keyword_hits: 0,
    };

    pub(crate) fn score(self) -> u32 {
        u32::from(self.category_hits) * 4 + u32::from(self.keyword_hits) * 3
    }

    /// Human-facing reason derived from which signal fired.
    pub(crate) fn reason(self) -> &'static str {
        match (self.category_hits > 0, self.keyword_hits > 0) {
            (true, true) => "popular in its category and matches apps you have",
            (true, false) => "popular with installs in this category",
            (false, true) => "similar to apps you have installed",
            (false, false) => "suggested for you",
        }
    }
}

/// Cap on how many entries the "recommended for you" row shows.
pub(crate) const MAX_RECOMMENDATIONS: usize = 3;

/// Deterministic, offline recommendations: score every not-installed entry by
/// same-category install popularity plus keyword overlap with installed apps,
/// keep the best [`MAX_RECOMMENDATIONS`], ties resolved by catalog order.
/// Entries scoring zero are dropped entirely.
pub(crate) fn rank_recommendations(
    count: usize,
    mut input_at: impl FnMut(usize) -> RecommendInput,
    out: &mut [Recommendation],
) -> usize {
    let count = count.min(crate::state::MAX_ENTRIES);
    let mut categories: [&str; crate::state::MAX_ENTRIES] = [""; crate::state::MAX_ENTRIES];
    let mut keywords: [&[&str]; crate::state::MAX_ENTRIES] = [&[]; crate::state::MAX_ENTRIES];
    let mut installed_flags = [false; crate::state::MAX_ENTRIES];
    let mut installed_count = 0usize;
    for index in 0..count {
        let input = input_at(index);
        categories[index] = input.category;
        keywords[index] = input.keywords;
        installed_flags[index] = input.installed;
        if input.installed {
            installed_count += 1;
        }
    }
    if installed_count == 0 {
        return 0;
    }

    let mut picks: [Recommendation; crate::state::MAX_ENTRIES] =
        [Recommendation::EMPTY; crate::state::MAX_ENTRIES];
    let mut pick_count = 0usize;
    for index in 0..count {
        if installed_flags[index] {
            continue;
        }
        let mut category_hits = 0u16;
        let mut keyword_hits = 0u16;
        for other in 0..count {
            if other == index || !installed_flags[other] {
                continue;
            }
            if eq_ci(categories[index], categories[other]) {
                category_hits += 1;
            }
            for keyword in keywords[index] {
                if keywords[other].iter().any(|other_kw| eq_ci(keyword, other_kw)) {
                    keyword_hits += 1;
                    break;
                }
            }
        }
        let candidate = Recommendation {
            index,
            category_hits,
            keyword_hits,
        };
        if candidate.score() == 0 {
            continue;
        }
        picks[pick_count] = candidate;
        pick_count += 1;
    }

    // Stable insertion sort by descending score keeps catalog order on ties.
    for slot in 1..pick_count {
        let item = picks[slot];
        let mut cursor = slot;
        while cursor > 0 && picks[cursor - 1].score() < item.score() {
            picks[cursor] = picks[cursor - 1];
            cursor -= 1;
        }
        picks[cursor] = item;
    }

    let written = pick_count.min(out.len()).min(MAX_RECOMMENDATIONS);
    out[..written].copy_from_slice(&picks[..written]);
    written
}

fn ascii_lower(byte: u8) -> u8 {
    byte.to_ascii_lowercase()
}

pub(crate) fn field_eq_ci(a: &str, b: &str) -> bool {
    eq_ci(a, b)
}

fn eq_ci(haystack: &str, needle: &str) -> bool {
    haystack.len() == needle.len()
        && haystack
            .bytes()
            .zip(needle.bytes())
            .all(|(a, b)| ascii_lower(a) == ascii_lower(b))
}

fn prefix_ci(haystack: &str, needle: &str) -> bool {
    haystack.len() >= needle.len()
        && !needle.is_empty()
        && haystack
            .bytes()
            .take(needle.len())
            .zip(needle.bytes())
            .all(|(a, b)| ascii_lower(a) == ascii_lower(b))
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let hay = haystack.as_bytes();
    let need = needle.as_bytes();
    let lower_need: [u8; MAX_QUERY_BYTES] = {
        let mut buf = [0u8; MAX_QUERY_BYTES];
        for (index, byte) in need.iter().enumerate().take(MAX_QUERY_BYTES) {
            buf[index] = ascii_lower(*byte);
        }
        buf
    };
    let need_len = need.len().min(MAX_QUERY_BYTES);
    if hay.len() < need_len {
        return false;
    }
    'outer: for start in 0..=(hay.len() - need_len) {
        for offset in 0..need_len {
            if ascii_lower(hay[start + offset]) != lower_need[offset] {
                continue 'outer;
            }
        }
        return true;
    }
    false
}

/// Tiered relevance: exact field match > field prefix > substring.
/// Higher is better; `None` means the query does not match the doc at all.
pub(crate) fn match_score(doc: &SearchDoc, query: &str) -> Option<u8> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    const EXACT_NAME: u8 = 9;
    const EXACT_OTHER: u8 = 6;
    const PREFIX_NAME: u8 = 5;
    const PREFIX_OTHER: u8 = 4;
    const SUBSTRING_ANY: u8 = 1;

    let mut best: Option<u8> = None;
    let consider = |best: &mut Option<u8>, score: u8| {
        if best.is_none_or(|current| score > current) {
            *best = Some(score);
        }
    };

    if eq_ci(doc.name, query) {
        consider(&mut best, EXACT_NAME);
    } else if prefix_ci(doc.name, query) {
        consider(&mut best, PREFIX_NAME);
    } else if contains_ci(doc.name, query) {
        consider(&mut best, SUBSTRING_ANY);
    }

    if eq_ci(doc.category, query) {
        consider(&mut best, EXACT_OTHER);
    } else if prefix_ci(doc.category, query) {
        consider(&mut best, PREFIX_OTHER);
    } else if contains_ci(doc.category, query) {
        consider(&mut best, SUBSTRING_ANY);
    }

    if eq_ci(doc.description, query) {
        consider(&mut best, EXACT_OTHER);
    } else if prefix_ci(doc.description, query) {
        consider(&mut best, PREFIX_OTHER);
    } else if contains_ci(doc.description, query) {
        consider(&mut best, SUBSTRING_ANY);
    }

    for keyword in doc.keywords {
        if eq_ci(keyword, query) {
            consider(&mut best, EXACT_OTHER);
        } else if prefix_ci(keyword, query) {
            consider(&mut best, PREFIX_OTHER);
        } else if contains_ci(keyword, query) || contains_ci(query, keyword) {
            consider(&mut best, SUBSTRING_ANY);
        }
    }

    best
}

/// Rank docs against a query. Writes matching indices into `out` ordered by
/// descending score then original position, returns how many were written.
/// Ties keep catalog order so results are deterministic.
pub(crate) fn rank_docs(
    count: usize,
    mut doc_at: impl FnMut(usize) -> SearchDoc,
    query: &str,
    out: &mut [usize],
) -> usize {
    let mut scores = [0u8; crate::state::MAX_ENTRIES];
    let mut hits = 0usize;
    for index in 0..count.min(out.len()).min(scores.len()) {
        if let Some(score) = match_score(&doc_at(index), query) {
            scores[hits] = score;
            out[hits] = index;
            hits += 1;
        }
    }
    for slot in 1..hits {
        let index = out[slot];
        let score = scores[slot];
        let mut cursor = slot;
        while cursor > 0 && scores[cursor - 1] < score {
            out[cursor] = out[cursor - 1];
            scores[cursor] = scores[cursor - 1];
            cursor -= 1;
        }
        out[cursor] = index;
        scores[cursor] = score;
    }
    hits
}

pub(crate) fn host_supported(targets: &[&str]) -> bool {
    targets.iter().any(|target| eq_ci(target, HOST_TARGET))
}

pub(crate) fn compat_label(targets: &[&str]) -> &'static str {
    if host_supported(targets) {
        "supported"
    } else {
        "unsupported on this host"
    }
}

pub(crate) fn keycode_to_char(key: u32) -> Option<u8> {
    const QWERTY_TOP: [u8; 10] = *b"qwertyuiop";
    const HOME_ROW: [u8; 9] = *b"asdfghjkl";
    const BOTTOM_ROW: [u8; 7] = *b"zxcvbnm";
    match key {
        2..=10 => Some(b'1' + (key - 2) as u8),
        11 => Some(b'0'),
        12 => Some(b'-'),
        13 => Some(b'='),
        16..=25 => Some(QWERTY_TOP[(key - 16) as usize]),
        30..=38 => Some(HOME_ROW[(key - 30) as usize]),
        39 => Some(b';'),
        41 => Some(b'\''),
        43 => Some(b'\\'),
        44..=50 => Some(BOTTOM_ROW[(key - 44) as usize]),
        51 => Some(b','),
        52 => Some(b'.'),
        53 => Some(b'/'),
        _ => None,
    }
}

/// Display state for an installed package relative to the newest catalog
/// version. `Unknown` covers missing data instead of guessing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UpdateDecision {
    UpToDate,
    UpdateAvailable,
    Unknown,
}

impl UpdateDecision {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            UpdateDecision::UpToDate => "up-to-date",
            UpdateDecision::UpdateAvailable => "update-available",
            UpdateDecision::Unknown => "unknown",
        }
    }
}

/// Compare two dotted versions numerically per component; missing trailing
/// components count as zero and non-numeric tails fall back to byte order.
pub(crate) fn compare_versions(left: &str, right: &str) -> core::cmp::Ordering {
    let mut left_parts = left.split('.');
    let mut right_parts = right.split('.');
    loop {
        match (left_parts.next(), right_parts.next()) {
            (None, None) => return core::cmp::Ordering::Equal,
            (Some(left_part), Some(right_part)) => {
                let ordering = compare_component(left_part, right_part);
                if ordering != core::cmp::Ordering::Equal {
                    return ordering;
                }
            }
            (Some(_), None) => {
                return if left_parts.all(is_zero_component) {
                    core::cmp::Ordering::Equal
                } else {
                    core::cmp::Ordering::Greater
                };
            }
            (None, Some(_)) => {
                return if right_parts.all(is_zero_component) {
                    core::cmp::Ordering::Equal
                } else {
                    core::cmp::Ordering::Less
                };
            }
        }
    }
}

fn is_zero_component(part: &str) -> bool {
    part.is_empty() || part.bytes().all(|byte| byte == b'0')
}

fn compare_component(left: &str, right: &str) -> core::cmp::Ordering {
    match (fully_numeric(left), fully_numeric(right)) {
        (Some(left_number), Some(right_number)) => left_number.cmp(&right_number),
        // Mixed or non-numeric components keep a deterministic byte order so
        // tagged versions never silently compare equal.
        _ => left.cmp(right),
    }
}

fn fully_numeric(part: &str) -> Option<u64> {
    if !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()) {
        part.parse::<u64>().ok()
    } else {
        None
    }
}

/// Decide the installed-vs-latest badge state for the detail panel.
pub(crate) fn decide_update(installed: Option<&str>, latest: Option<&str>) -> UpdateDecision {
    let (Some(installed), Some(latest)) = (installed, latest) else {
        return UpdateDecision::Unknown;
    };
    if installed.is_empty() || latest.is_empty() {
        return UpdateDecision::Unknown;
    }
    if compare_versions(installed, latest) == core::cmp::Ordering::Less {
        UpdateDecision::UpdateAvailable
    } else {
        UpdateDecision::UpToDate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(name: &'static str) -> SearchDoc {
        SearchDoc {
            name,
            category: "System",
            description: "",
            keywords: &[],
        }
    }

    #[test]
    fn exact_beats_prefix_beats_substring() {
        let exact = SearchDoc {
            keywords: &[],
            ..doc("announce")
        };
        let prefix = SearchDoc {
            keywords: &[],
            ..doc("announce-daemon")
        };
        let substring = desc_doc("service that announces things", "other");
        let query = "announce";
        let exact_score = match_score(&exact, query).unwrap();
        let prefix_score = match_score(&prefix, query).unwrap();
        let substring_score = match_score(&substring, query).unwrap();
        assert!(exact_score > prefix_score);
        assert!(prefix_score > substring_score);
    }

    fn desc_doc(description: &'static str, name: &'static str) -> SearchDoc {
        SearchDoc {
            name,
            category: "System",
            description,
            keywords: &[],
        }
    }

    #[test]
    fn matching_is_case_insensitive() {
        let subject = SearchDoc {
            keywords: &["WorkSpace"],
            ..doc("Runtime-Service")
        };
        assert_eq!(match_score(&subject, "RUNTIME-SERVICE"), Some(9));
        assert_eq!(match_score(&subject, "run"), Some(5));
        assert_eq!(match_score(&subject, "workspace"), Some(6));
    }

    #[test]
    fn description_and_keywords_participate() {
        let sdk = SearchDoc {
            name: "developer-service",
            category: "Development",
            description: "Cross-platform SDK with toolchains and sample workspaces.",
            keywords: &["sdk", "toolchain"],
        };
        assert!(match_score(&sdk, "toolch").is_some());
        assert!(match_score(&sdk, "sample").is_some());
        assert_eq!(match_score(&sdk, "gpu"), None);
    }

    #[test]
    fn empty_query_matches_nothing() {
        assert_eq!(match_score(&doc("anything"), ""), None);
        assert_eq!(match_score(&doc("anything"), "   "), None);
    }

    #[test]
    fn ranking_orders_by_score_then_catalog_order() {
        let docs = [
            SearchDoc {
                keywords: &[],
                ..doc("announce-daemon")
            },
            desc_doc("network daemon guide", "unrelated"),
            SearchDoc {
                keywords: &["announce"],
                ..doc("messenger")
            },
            SearchDoc {
                keywords: &[],
                ..doc("announce-service")
            },
        ];
        let mut out = [0usize; crate::state::MAX_ENTRIES];
        let hits = rank_docs(docs.len(), |index| docs[index], "announce", &mut out);
        assert_eq!(hits, 3);
        assert_eq!(out[0], 2);
        assert_eq!(out[1], 0);
        assert_eq!(out[2], 3);
    }

    #[test]
    fn compatibility_reflects_host_target() {
        assert!(host_supported(&[TARGET_X86_64]));
        assert!(host_supported(UNIVERSAL_TARGETS));
        let only_other = if HOST_TARGET == TARGET_X86_64 {
            [TARGET_AARCH64]
        } else {
            [TARGET_X86_64]
        };
        assert!(!host_supported(&only_other));
        assert_eq!(compat_label(&only_other), "unsupported on this host");
        assert_eq!(compat_label(UNIVERSAL_TARGETS), "supported");
    }

    #[test]
    fn keycodes_map_to_expected_characters() {
        assert_eq!(keycode_to_char(2), Some(b'1'));
        assert_eq!(keycode_to_char(11), Some(b'0'));
        assert_eq!(keycode_to_char(17), Some(b'w'));
        assert_eq!(keycode_to_char(19), Some(b'r'));
        assert_eq!(keycode_to_char(46), Some(b'c'));
        assert_eq!(keycode_to_char(50), Some(b'm'));
        assert_eq!(keycode_to_char(14), None);
        assert_eq!(keycode_to_char(999), None);
    }

    #[test]
    fn version_components_compare_numerically() {
        use core::cmp::Ordering;
        assert_eq!(compare_versions("1.9.0", "1.10.0"), Ordering::Less);
        assert_eq!(compare_versions("1.2", "1.2.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.0.0-beta", "1.0.0"), Ordering::Greater);
    }

    #[test]
    fn update_decision_maps_missing_or_equal_data() {
        assert_eq!(
            decide_update(Some("1.0.0"), Some("1.1.0")),
            UpdateDecision::UpdateAvailable
        );
        assert_eq!(
            decide_update(Some("2.0.0"), Some("2.0.0")),
            UpdateDecision::UpToDate
        );
        assert_eq!(decide_update(None, Some("1.0.0")), UpdateDecision::Unknown);
        assert_eq!(UpdateDecision::Unknown.label(), "unknown");
    }

    fn rec_input(
        category: &'static str,
        keywords: &'static [&'static str],
        installed: bool,
    ) -> RecommendInput {
        RecommendInput {
            category,
            keywords,
            installed,
        }
    }

    #[test]
    fn rating_clamps_to_displayable_range() {
        assert_eq!(clamp_rating_tenths(0), None);
        assert_eq!(clamp_rating_tenths(42), Some(42));
        assert_eq!(clamp_rating_tenths(51), Some(MAX_RATING_TENTHS as u8));
        assert_eq!(
            clamp_rating_tenths(u16::MAX),
            Some(MAX_RATING_TENTHS as u8)
        );
        assert_eq!(rating_tenths_for(ServiceId::Runtime), Some(46));
    }

    #[test]
    fn star_bar_rounds_half_up_and_caps_at_five() {
        let bar = |tenths: u8| -> [u8; 5] { star_bar(tenths) };
        assert_eq!(&bar(50), b"*****");
        assert_eq!(&bar(45), b"*****");
        assert_eq!(&bar(44), b"****-");
        assert_eq!(&bar(25), b"***--");
        assert_eq!(&bar(24), b"**---");
        // One tenth of a star rounds to zero filled slots.
        assert_eq!(&bar(1), b"-----");
        assert_eq!(&bar(5), b"*----");
        assert_eq!(&bar(0), b"-----");
        // Out-of-range input saturates instead of panicking or underflowing.
        assert_eq!(&bar(255), b"*****");
    }

    #[test]
    fn screenshot_placeholder_selection_is_stable_and_labeled() {
        assert_eq!(screenshot_placeholder_headline(""), None);
        let chosen = screenshot_placeholder_headline("announce-desktop-broadcast").unwrap();
        assert!(SCREENSHOT_HEADLINES.contains(&chosen));
        // Same reference always selects the same headline (deterministic).
        assert_eq!(
            screenshot_placeholder_headline("announce-desktop-broadcast"),
            Some(chosen)
        );
        let other = screenshot_placeholder_headline("zz-unseen-ref").unwrap();
        assert!(SCREENSHOT_HEADLINES.contains(&other));
    }

    #[test]
    fn recommendations_rank_by_overlap_then_catalog_order() {
        let categories = ["Messaging", "Runtime", "Development", "Messaging", "Development"];
        let keyword_sets: [&[&str]; 5] = [
            &["notify"],
            &["posix"],
            &["sdk", "workspace"],
            &["chat"],
            &["sdk", "tools"],
        ];
        let installed = [false, false, false, true, true];
        let inputs = |index: usize| {
            rec_input(categories[index], keyword_sets[index], installed[index])
        };
        let mut out = [Recommendation::EMPTY; MAX_RECOMMENDATIONS];
        let written = rank_recommendations(5, inputs, &mut out);
        // Development SDK scores 4 (category) + 3 (keyword) = 7; Messaging
        // scores 4 (category only); Runtime scores zero and is dropped.
        assert_eq!(written, 2);
        assert_eq!(out[0].index, 2);
        assert_eq!(out[0].score(), 7);
        assert_eq!(
            out[0].reason(),
            "popular in its category and matches apps you have"
        );
        assert_eq!(out[1].index, 0);
        assert_eq!(out[1].category_hits, 1);
        assert_eq!(out[1].keyword_hits, 0);
        assert_eq!(out[1].reason(), "popular with installs in this category");
    }

    #[test]
    fn recommendations_break_ties_by_catalog_order_and_cap_output() {
        let categories = ["A", "A", "A", "A"];
        let keyword_sets: [&[&str]; 4] = [&[]; 4];
        let installed = [true, false, false, false];
        let inputs = |index: usize| {
            rec_input(categories[index], keyword_sets[index], installed[index])
        };
        let mut out = [Recommendation::EMPTY; MAX_RECOMMENDATIONS];
        let written = rank_recommendations(4, inputs, &mut out);
        assert_eq!(written, MAX_RECOMMENDATIONS);
        assert_eq!(out[0].index, 1);
        assert_eq!(out[1].index, 2);
        assert_eq!(out[2].index, 3);
    }

    #[test]
    fn recommendations_need_an_installed_base_and_skip_zero_scores() {
        let categories = ["A", "B"];
        let keyword_sets: [&[&str]; 2] = [&["x"], &["y"]];
        let installed = [false, false];
        let inputs = |index: usize| {
            rec_input(categories[index], keyword_sets[index], installed[index])
        };
        let mut out = [Recommendation::EMPTY; MAX_RECOMMENDATIONS];
        assert_eq!(rank_recommendations(2, inputs, &mut out), 0);

        let installed = [true, false];
        let inputs = |index: usize| {
            rec_input(categories[index], keyword_sets[index], installed[index])
        };
        assert_eq!(rank_recommendations(2, inputs, &mut out), 0);
    }
}

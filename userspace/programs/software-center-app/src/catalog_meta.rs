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
            screenshot_ref: "",
        }),
        ServiceId::Runtime => Some(&PackageMeta {
            category: "Runtime",
            description: "POSIX-style runtime root with shell tools for hosted workloads.",
            keywords: &["runtime", "posix", "workload", "sandbox", "shell"],
            targets: UNIVERSAL_TARGETS,
            screenshot_ref: "",
        }),
        ServiceId::Developer => Some(&PackageMeta {
            category: "Development",
            description: "Cross-platform SDK with toolchains and sample workspaces.",
            keywords: &["sdk", "toolchain", "development", "compiler", "workspace"],
            targets: UNIVERSAL_TARGETS,
            screenshot_ref: "",
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
}

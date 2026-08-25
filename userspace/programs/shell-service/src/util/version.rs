//! Version comparison and update-decision display helpers shared by the
//! package operator views. Pure functions so host tests cover the mapping.

use core::cmp::Ordering;

/// Outcome of comparing an installed version against the newest catalog
/// version. `Unknown` covers missing/unparsable data instead of guessing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateDecision {
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

    pub(crate) const fn flag(self) -> &'static str {
        match self {
            UpdateDecision::UpToDate => "no",
            UpdateDecision::UpdateAvailable => "yes",
            UpdateDecision::Unknown => "?",
        }
    }
}

/// Compare two dotted versions numerically per component. Missing components
/// count as zero (1.2 == 1.2.0); non-numeric tails fall back to byte order so
/// unusual versions still sort deterministically.
pub fn compare_versions(left: &str, right: &str) -> Ordering {
    let mut left_parts = left.split('.');
    let mut right_parts = right.split('.');
    loop {
        match (left_parts.next(), right_parts.next()) {
            (None, None) => return Ordering::Equal,
            (Some(left_part), Some(right_part)) => {
                let ordering = compare_component(left_part, right_part);
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (Some(_), None) => {
                // Remaining left components must be zero to stay equal.
                return if left_parts.all(is_zero_component) {
                    Ordering::Equal
                } else {
                    Ordering::Greater
                };
            }
            (None, Some(_)) => {
                return if right_parts.all(is_zero_component) {
                    Ordering::Equal
                } else {
                    Ordering::Less
                };
            }
        }
    }
}

fn is_zero_component(part: &str) -> bool {
    part.is_empty() || part.bytes().all(|byte| byte == b'0')
}

fn compare_component(left: &str, right: &str) -> Ordering {
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

/// Decide the installed-vs-latest display state. Either side missing or
/// non-utf8 yields `Unknown` rather than a false "up-to-date".
pub fn decide_update(installed: Option<&str>, latest: Option<&str>) -> UpdateDecision {
    let (Some(installed), Some(latest)) = (installed, latest) else {
        return UpdateDecision::Unknown;
    };
    if installed.is_empty() || latest.is_empty() {
        return UpdateDecision::Unknown;
    }
    if compare_versions(installed, latest) == Ordering::Less {
        UpdateDecision::UpdateAvailable
    } else {
        UpdateDecision::UpToDate
    }
}

#[cfg(test)]
mod tests {
    use super::{UpdateDecision, compare_versions, decide_update};
    use core::cmp::Ordering;

    #[test]
    fn numeric_components_compare_numerically() {
        assert_eq!(compare_versions("1.9.0", "1.10.0"), Ordering::Less);
        assert_eq!(compare_versions("1.10.0", "1.9.0"), Ordering::Greater);
        assert_eq!(compare_versions("2.0.0", "1.99.99"), Ordering::Greater);
    }

    #[test]
    fn equal_versions_with_different_depths_match() {
        assert_eq!(compare_versions("1.2", "1.2.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.2.0.0", "1.2"), Ordering::Equal);
        assert_eq!(compare_versions("3.4.1", "3.4.1"), Ordering::Equal);
    }

    #[test]
    fn non_numeric_tails_fall_back_to_byte_order() {
        assert_eq!(compare_versions("1.0.0-beta", "1.0.0"), Ordering::Greater);
        assert_eq!(compare_versions("abc", "abd"), Ordering::Less);
    }

    #[test]
    fn update_decision_maps_missing_data_to_unknown() {
        assert_eq!(decide_update(Some("1.0.0"), None), UpdateDecision::Unknown);
        assert_eq!(decide_update(None, Some("1.0.0")), UpdateDecision::Unknown);
        assert_eq!(
            decide_update(Some(""), Some("1.0.0")),
            UpdateDecision::Unknown
        );
    }

    #[test]
    fn update_decision_flags_only_older_installed_versions() {
        assert_eq!(
            decide_update(Some("1.0.0"), Some("1.1.0")),
            UpdateDecision::UpdateAvailable
        );
        assert_eq!(
            decide_update(Some("1.1.0"), Some("1.1.0")),
            UpdateDecision::UpToDate
        );
        assert_eq!(
            decide_update(Some("1.2.0"), Some("1.1.0")),
            UpdateDecision::UpToDate
        );
    }

    #[test]
    fn labels_and_flags_stay_operator_readable() {
        assert_eq!(UpdateDecision::UpToDate.label(), "up-to-date");
        assert_eq!(UpdateDecision::UpdateAvailable.flag(), "yes");
        assert_eq!(UpdateDecision::Unknown.flag(), "?");
    }
}

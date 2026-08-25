use core::cmp::Ordering;
use core::str;

use serviceos_userspace_runtime as rt;

use crate::{
    consts::MAX_TOOLCHAINS,
    types::{FixedBytes, ToolchainSlot},
};

pub(crate) const MAX_VERSION_PARTS: usize = 8;

/// Build-failure detail code: toolchain SDK install root missing in storage.
pub(crate) const TOOLCHAIN_ROOT_MISSING: u64 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolchainFamily {
    Rust,
    Gcc,
    Llvm,
    Native,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ToolchainVersion {
    pub(crate) len: usize,
    pub(crate) parts: [u16; MAX_VERSION_PARTS],
}

impl ToolchainVersion {
    fn empty() -> Self {
        Self {
            len: 0,
            parts: [0; MAX_VERSION_PARTS],
        }
    }
}

/// One registry record per packaged toolchain descriptor: derived family and
/// version plus a live presence probe result for the SDK install root.
#[derive(Clone, Copy)]
pub(crate) struct RegistryRecord {
    pub(crate) occupied: bool,
    pub(crate) family: ToolchainFamily,
    pub(crate) version: Option<ToolchainVersion>,
    pub(crate) present: bool,
    /// Newest-first position among same-family versioned entries: 0 is the
    /// latest. Version-less entries rank after all versioned ones.
    pub(crate) rank: u8,
}

impl RegistryRecord {
    pub(crate) const fn empty() -> Self {
        Self {
            occupied: false,
            family: ToolchainFamily::Other,
            version: None,
            present: false,
            rank: 0,
        }
    }
}

/// Classify the toolchain family from the descriptor name first ("rust",
/// "gcc", "llvm" prefixes) with the SDK root path as a fallback signal.
pub(crate) fn family_of(name: &[u8], sdk_root: &[u8]) -> ToolchainFamily {
    let lower = |bytes: &[u8]| -> FixedBytes<{ crate::consts::MAX_PATH }> {
        let mut out = FixedBytes::<{ crate::consts::MAX_PATH }>::empty();
        let mut buffer = [0u8; crate::consts::MAX_PATH];
        for (index, byte) in bytes.iter().take(crate::consts::MAX_PATH).enumerate() {
            buffer[index] = byte.to_ascii_lowercase();
        }
        let _ = out.set(&buffer[..bytes.len().min(crate::consts::MAX_PATH)]);
        out
    };
    let name = lower(name);
    let root = lower(sdk_root);
    for family in [&b"rust"[..], &b"gcc"[..], &b"llvm"[..]] {
        if name.as_bytes().starts_with(family) || contains_subslice(root.as_bytes(), family) {
            return match family[0] {
                b'r' => ToolchainFamily::Rust,
                b'g' => ToolchainFamily::Gcc,
                _ => ToolchainFamily::Llvm,
            };
        }
    }
    if name.as_bytes().windows(6).any(|w| w == b"native")
        || root.as_bytes().windows(6).any(|w| w == b"native")
    {
        return ToolchainFamily::Native;
    }
    ToolchainFamily::Other
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Extract a dotted numeric version from the trailing "-" separated segments
/// of the descriptor name, then from the "/" separated segments of the SDK
/// install root (versioned install roots such as ".../sdk/llvm-17.1").
pub(crate) fn version_of(name: &[u8], sdk_root: &[u8]) -> Option<ToolchainVersion> {
    for segment in name.rsplit(|byte| *byte == b'-') {
        if let Some(version) = parse_version(segment) {
            return Some(version);
        }
    }
    // Only the leaf of a versioned install root counts: intermediate numeric
    // directories (package versions such as ".../1.0.0/") are not toolchain
    // versions.
    if let Some(leaf) = sdk_root.rsplit(|byte| *byte == b'/').next() {
        let candidate = match leaf.iter().rposition(|byte| *byte == b'-') {
            Some(hyphen) => &leaf[hyphen + 1..],
            None => leaf,
        };
        if let Some(version) = parse_version(candidate) {
            return Some(version);
        }
    }
    None
}

/// Parse "\d+(\.\d+)*" with an optional leading 'v'; at least one part.
pub(crate) fn parse_version(text: &[u8]) -> Option<ToolchainVersion> {
    let text = text.strip_prefix(b"v").unwrap_or(text);
    if text.is_empty() || text.len() > 32 {
        return None;
    }
    let mut version = ToolchainVersion::empty();
    for part in text.split(|byte| *byte == b'.') {
        if part.is_empty() || part.len() > 5 || !part.iter().all(u8::is_ascii_digit) {
            return None;
        }
        if version.len >= MAX_VERSION_PARTS {
            return None;
        }
        let mut value = 0u16;
        for byte in part {
            value = value
                .saturating_mul(10)
                .saturating_add((*byte - b'0') as u16);
        }
        version.parts[version.len] = value;
        version.len += 1;
    }
    Some(version)
}

/// Numeric component-wise ordering; shorter operand pads with zeroes, so
/// "17" == "17.0". Version-less entries order before any versioned entry.
pub(crate) fn compare_versions(
    left: Option<&ToolchainVersion>,
    right: Option<&ToolchainVersion>,
) -> Ordering {
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(left), Some(right)) => {
            let width = left.len.max(right.len);
            for index in 0..width {
                let l = left.parts.get(index).copied().unwrap_or(0);
                let r = right.parts.get(index).copied().unwrap_or(0);
                if l != r {
                    return l.cmp(&r);
                }
            }
            Ordering::Equal
        }
    }
}

pub(crate) fn build_registry(
    toolchains: &[ToolchainSlot],
    count: usize,
) -> [RegistryRecord; MAX_TOOLCHAINS] {
    let mut records = [RegistryRecord::empty(); MAX_TOOLCHAINS];
    for (index, slot) in toolchains[..count.min(MAX_TOOLCHAINS)].iter().enumerate() {
        if !slot.occupied {
            continue;
        }
        records[index] = RegistryRecord {
            occupied: true,
            family: family_of(slot.name.as_bytes(), slot.sdk_root.as_bytes()),
            version: version_of(slot.name.as_bytes(), slot.sdk_root.as_bytes()),
            present: false,
            rank: 0,
        };
    }
    for index in 0..MAX_TOOLCHAINS {
        let record = &records[index];
        if !record.occupied {
            continue;
        }
        let Some(version) = record.version else {
            continue;
        };
        let family = record.family;
        let newer = records
            .iter()
            .enumerate()
            .filter(|(other_index, other)| {
                other.occupied
                    && *other_index != index
                    && other.family == family
                    && other.version.is_some_and(|other_version| {
                        compare_versions(Some(&other_version), Some(&version)) == Ordering::Greater
                    })
            })
            .count();
        records[index].rank = newer.min(u8::MAX as usize) as u8;
    }
    records
}

/// Render a parsed version back to dotted text for control-contract replies.
pub(crate) fn format_version_text<'a>(version: &ToolchainVersion, out: &'a mut [u8]) -> &'a [u8] {
    let mut len = 0usize;
    for (index, part) in version.parts[..version.len].iter().enumerate() {
        if index > 0 {
            if len < out.len() {
                out[len] = b'.';
                len += 1;
            }
        }
        let mut digits = [0u8; 5];
        let mut count = 0usize;
        let mut value = *part;
        loop {
            digits[count] = b'0' + (value % 10) as u8;
            count += 1;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        for byte in digits[..count].iter().rev() {
            if len < out.len() {
                out[len] = *byte;
                len += 1;
            }
        }
    }
    &out[..len]
}

/// Live verify-present operation against storage: the SDK install root must
/// open as a readable blob for the toolchain to count as present.
pub(crate) fn verify_present(storage_handle: rt::Handle, toolchain: &ToolchainSlot) -> bool {
    let Ok(path) = str::from_utf8(toolchain.sdk_root.as_bytes()) else {
        return false;
    };
    if path.is_empty() {
        return false;
    }
    match rt::storage_open(storage_handle, path) {
        Ok((blob, _)) => {
            let _ = rt::storage_blob_close(blob);
            true
        }
        Err(_) => false,
    }
}

/// Bitmask over registry families for the catalog-loaded telemetry word:
/// Rust=bit0, Gcc=bit1, Llvm=bit2, Native=bit3, Other=bit4.
pub(crate) fn family_mask(records: &[RegistryRecord; MAX_TOOLCHAINS]) -> u64 {
    let mut mask = 0u64;
    for record in records.iter().filter(|record| record.occupied) {
        let bit = match record.family {
            ToolchainFamily::Rust => 0,
            ToolchainFamily::Gcc => 1,
            ToolchainFamily::Llvm => 2,
            ToolchainFamily::Native => 3,
            ToolchainFamily::Other => 4,
        };
        mask |= 1u64 << bit;
    }
    mask
}

/// Number of registry entries that carry a parsed version tag.
pub(crate) fn versioned_count(records: &[RegistryRecord; MAX_TOOLCHAINS]) -> u64 {
    records
        .iter()
        .filter(|record| record.occupied && record.version.is_some())
        .count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(name: &[u8], sdk_root: &[u8]) -> ToolchainSlot {
        let mut slot = ToolchainSlot::empty();
        slot.occupied = true;
        let _ = slot.name.set(name);
        let _ = slot.sdk_root.set(sdk_root);
        slot
    }

    #[test]
    fn family_from_name_prefix() {
        assert_eq!(
            family_of(b"rust-std-1.78", b"sdk/rust"),
            ToolchainFamily::Rust
        );
        assert_eq!(family_of(b"gcc-cross", b"sdk/gcc"), ToolchainFamily::Gcc);
        assert_eq!(
            family_of(b"llvm-17.1", b"sdk/llvm-17.1"),
            ToolchainFamily::Llvm
        );
    }

    #[test]
    fn family_from_sdk_root_fallback() {
        assert_eq!(
            family_of(b"rustc-host", b"tools/rust/1.78"),
            ToolchainFamily::Rust
        );
    }

    #[test]
    fn family_native_and_other() {
        assert_eq!(
            family_of(b"serviceos-native", b"packages/d/1.0.0/sdk/native"),
            ToolchainFamily::Native
        );
        assert_eq!(
            family_of(b"linux-x64", b"packages/d/1.0.0/sdk/linux"),
            ToolchainFamily::Other
        );
    }

    #[test]
    fn family_case_insensitive() {
        assert_eq!(family_of(b"LLVM-18", b"sdk/x"), ToolchainFamily::Llvm);
    }

    #[test]
    fn version_from_name_segment() {
        let version = version_of(b"llvm-17.1.2", b"sdk/llvm").unwrap();
        assert_eq!(version.len, 3);
        assert_eq!(&version.parts[..3], &[17, 1, 2]);
    }

    #[test]
    fn version_from_install_root_segment() {
        let version = version_of(b"gcc-cross", b"sdk/gcc-13.2").unwrap();
        assert_eq!(version.len, 2);
        assert_eq!(&version.parts[..2], &[13, 2]);
    }

    #[test]
    fn version_absent_for_plain_names() {
        assert_eq!(
            version_of(b"linux-x64", b"packages/d/1.0.0/sdk/linux"),
            None
        );
    }

    #[test]
    fn version_parse_rejects_garbage() {
        assert_eq!(parse_version(b"x64"), None);
        assert_eq!(parse_version(b"1..2"), None);
        assert_eq!(parse_version(b""), None);
        assert_eq!(parse_version(b"1a"), None);
    }

    #[test]
    fn version_parse_accepts_v_prefix_and_single_part() {
        assert_eq!(parse_version(b"v4").unwrap().parts[0], 4);
        assert_eq!(parse_version(b"2024").unwrap().len, 1);
    }

    #[test]
    fn version_ordering_numeric_not_lexicographic() {
        let a = parse_version(b"9").unwrap();
        let b = parse_version(b"17.1").unwrap();
        assert_eq!(compare_versions(Some(&a), Some(&b)), Ordering::Less);
    }

    #[test]
    fn version_ordering_pads_missing_parts() {
        let a = parse_version(b"17").unwrap();
        let b = parse_version(b"17.0").unwrap();
        assert_eq!(compare_versions(Some(&a), Some(&b)), Ordering::Equal);
    }

    #[test]
    fn version_ordering_deep_components() {
        let a = parse_version(b"17.1.9").unwrap();
        let b = parse_version(b"17.2").unwrap();
        assert_eq!(compare_versions(Some(&a), Some(&b)), Ordering::Less);
    }

    #[test]
    fn versionless_sorts_before_versioned() {
        assert_eq!(
            compare_versions(None, Some(&parse_version(b"1").unwrap())),
            Ordering::Less
        );
        assert_eq!(compare_versions(None, None), Ordering::Equal);
    }

    #[test]
    fn registry_records_derive_from_slots() {
        let toolchains = [
            slot(b"llvm-17.1", b"sdk/llvm-17.1"),
            slot(b"linux-x64", b"sdk/linux"),
            ToolchainSlot::empty(),
        ];
        let records = build_registry(&toolchains, 3);
        assert!(records[0].occupied);
        assert_eq!(records[0].family, ToolchainFamily::Llvm);
        assert_eq!(records[0].version.unwrap().parts[0], 17);
        assert!(records[1].occupied);
        assert_eq!(records[1].family, ToolchainFamily::Other);
        assert!(!records[2].occupied);
    }
}

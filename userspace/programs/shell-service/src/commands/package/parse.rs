use rt::{PackageChannel, PackageMaintenanceAction, PackageRepositoryTrustMode, PackageRing};
use serviceos_userspace_runtime as rt;

pub(super) const MAX_PACKAGE_TEXT: usize = 96;

pub(super) fn parse_channel(value: &str) -> Option<PackageChannel> {
    match value {
        "stable" => Some(PackageChannel::Stable),
        "beta" => Some(PackageChannel::Beta),
        "canary" => Some(PackageChannel::Canary),
        _ => None,
    }
}

pub(super) fn parse_ring(value: &str) -> Option<PackageRing> {
    match value {
        "production" => Some(PackageRing::Production),
        "preview" => Some(PackageRing::Preview),
        "testing" => Some(PackageRing::Testing),
        _ => None,
    }
}

pub(super) fn parse_repo_trust(value: &str) -> Option<(PackageRepositoryTrustMode, u64)> {
    if value == "unsigned" {
        Some((PackageRepositoryTrustMode::Unsigned, 0))
    } else if let Some(hex) = value.strip_prefix("pinned:") {
        parse_hex_u64(hex).map(|digest| (PackageRepositoryTrustMode::PinnedDigest, digest))
    } else if value == "signed-key" {
        Some((PackageRepositoryTrustMode::SignedKey, 0))
    } else {
        None
    }
}

pub(super) fn parse_usize(value: &str) -> Option<usize> {
    value.parse::<usize>().ok()
}

fn parse_hex_u64(value: &str) -> Option<u64> {
    let trimmed = value.strip_prefix("0x").unwrap_or(value);
    u64::from_str_radix(trimmed, 16).ok()
}

pub(super) fn trust_mode_name(value: PackageRepositoryTrustMode) -> &'static str {
    match value {
        PackageRepositoryTrustMode::Boot => "boot",
        PackageRepositoryTrustMode::Unsigned => "unsigned",
        PackageRepositoryTrustMode::PinnedDigest => "pinned",
        PackageRepositoryTrustMode::SignedKey => "signed-key",
    }
}

pub(super) fn repo_sync_state_name(value: rt::PackageRepositorySyncState) -> &'static str {
    match value {
        rt::PackageRepositorySyncState::Idle => "idle",
        rt::PackageRepositorySyncState::Ready => "ready",
        rt::PackageRepositorySyncState::Offline => "offline",
        rt::PackageRepositorySyncState::Failed => "failed",
    }
}

pub(super) fn trust_state_name(value: rt::PackageTrustState) -> &'static str {
    match value {
        rt::PackageTrustState::BootTrusted => "boot-trusted",
        rt::PackageTrustState::Unverified => "unverified",
        rt::PackageTrustState::DigestPinned => "digest-pinned",
        rt::PackageTrustState::SignedKeyTrusted => "signed-key-trusted",
        rt::PackageTrustState::VerificationFailed => "verification-failed",
    }
}

/// Operator-facing signing state derived from the package trust contract:
/// boot-trusted packages are anchored in the boot trust root, digest-pinned
/// packages carry a pinned digest verification, and unverified/failed states
/// mean no accepted signature evidence exists.
pub(super) fn signing_state_name(value: rt::PackageTrustState) -> &'static str {
    match value {
        rt::PackageTrustState::BootTrusted => "trust-root",
        rt::PackageTrustState::Unverified => "unsigned",
        rt::PackageTrustState::DigestPinned => "digest-signed",
        rt::PackageTrustState::SignedKeyTrusted => "ed25519-signed",
        rt::PackageTrustState::VerificationFailed => "verification-failed",
    }
}

pub(super) fn channel_name(value: PackageChannel) -> &'static str {
    match value {
        PackageChannel::Stable => "stable",
        PackageChannel::Beta => "beta",
        PackageChannel::Canary => "canary",
    }
}

pub(super) fn ring_name(value: PackageRing) -> &'static str {
    match value {
        PackageRing::Production => "production",
        PackageRing::Preview => "preview",
        PackageRing::Testing => "testing",
    }
}

pub(super) fn maintenance_action_name(value: PackageMaintenanceAction) -> &'static str {
    match value {
        PackageMaintenanceAction::Validate => "validated",
        PackageMaintenanceAction::Repair => "repaired",
        PackageMaintenanceAction::GarbageCollect => "garbage-collected",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_state_tracks_trust_contract() {
        assert_eq!(
            signing_state_name(rt::PackageTrustState::BootTrusted),
            "trust-root"
        );
        assert_eq!(
            signing_state_name(rt::PackageTrustState::DigestPinned),
            "digest-signed"
        );
        assert_eq!(
            signing_state_name(rt::PackageTrustState::SignedKeyTrusted),
            "ed25519-signed"
        );
        assert_eq!(
            signing_state_name(rt::PackageTrustState::Unverified),
            "unsigned"
        );
        assert_eq!(
            signing_state_name(rt::PackageTrustState::VerificationFailed),
            "verification-failed"
        );
    }

    #[test]
    fn signed_key_trust_mode_parses_and_names_roundtrip() {
        assert_eq!(
            parse_repo_trust("signed-key"),
            Some((PackageRepositoryTrustMode::SignedKey, 0))
        );
        assert_eq!(PackageRepositoryTrustMode::SignedKey as u64, 4);
        assert_eq!(
            trust_mode_name(PackageRepositoryTrustMode::SignedKey),
            "signed-key"
        );
        assert_eq!(
            trust_state_name(rt::PackageTrustState::SignedKeyTrusted),
            "signed-key-trusted"
        );
        assert_eq!(parse_repo_trust("bogus"), None);
    }
}

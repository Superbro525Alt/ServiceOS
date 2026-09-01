#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageTag {
    ListRequest = 0x700,
    ListReply = 0x701,
    InfoRequest = 0x702,
    InfoReply = 0x703,
    InstallRequest = 0x704,
    InstallReply = 0x705,
    RemoveRequest = 0x706,
    RemoveReply = 0x707,
    UpdateRequest = 0x708,
    UpdateReply = 0x709,
    RollbackRequest = 0x70a,
    RollbackReply = 0x70b,
    HistoryRequest = 0x70c,
    HistoryReply = 0x70d,
    CatalogRequest = 0x70e,
    CatalogReply = 0x70f,
    MetadataRequest = 0x710,
    MetadataReply = 0x711,
    RepositoryListRequest = 0x712,
    RepositoryListReply = 0x713,
    RepositoryAddRequest = 0x714,
    RepositoryAddReply = 0x715,
    RepositorySyncRequest = 0x716,
    RepositorySyncReply = 0x717,
    ProvenanceRequest = 0x718,
    ProvenanceReply = 0x719,
    PolicyRequest = 0x71a,
    PolicyReply = 0x71b,
    PolicySetRequest = 0x71c,
    PolicySetReply = 0x71d,
    MaintenanceRequest = 0x71e,
    MaintenanceReply = 0x71f,
    /// Feed-keystore key management (additive, shell-driven):
    /// list / enroll / activate-by-id / rotate-source / generate keypair.
    KeysListRequest = 0x720,
    KeysListReply = 0x721,
    KeysEnrollRequest = 0x722,
    KeysEnrollReply = 0x723,
    KeysActivateRequest = 0x724,
    KeysActivateReply = 0x725,
    KeysRotateRequest = 0x726,
    KeysRotateReply = 0x727,
    KeysGenRequest = 0x728,
    KeysGenReply = 0x729,
    /// Per-source staged-rollout cohorts and upgrade rules (additive,
    /// shell-driven): list configured policies / page hold names / mutate
    /// one rule / report the gated update decision for one package.
    RolloutListRequest = 0x72a,
    RolloutListReply = 0x72b,
    RolloutGetRequest = 0x72c,
    RolloutGetReply = 0x72d,
    RolloutSetRequest = 0x72e,
    RolloutSetReply = 0x72f,
    RolloutStatusRequest = 0x730,
    RolloutStatusReply = 0x731,
    /// Trust-root enrollment layer (additive, shell-driven): the
    /// operator-managed ROOT list from which enrolled feed-signing keys
    /// derive their standing. List/get one root row, add/remove roots.
    RootListRequest = 0x732,
    RootListReply = 0x733,
    RootGetRequest = 0x734,
    RootGetReply = 0x735,
    RootAddRequest = 0x736,
    RootAddReply = 0x737,
    RootRemoveRequest = 0x738,
    RootRemoveReply = 0x739,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageStatus {
    Ok = 0,
    NotFound = 1,
    AlreadyInstalled = 2,
    NotInstalled = 3,
    Busy = 4,
    Denied = 5,
    IntegrityFailed = 6,
    End = 7,
    NoChange = 8,
    NoRollback = 9,
    Unsupported = 10,
    Offline = 11,
    Interrupted = 12,
    VerificationFailed = 13,
    InvalidParameter = 14,
    AlreadyExists = 15,
    /// Trust-root enrollment refused: the chosen root key id has no private
    /// keypair in the keystore, so it cannot sign attestations. Additive at
    /// the END of the status space (legacy readers see an unknown word).
    NoKeyPair = 16,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageTrustState {
    BootTrusted = 1,
    DigestPinned = 2,
    Unverified = 3,
    VerificationFailed = 4,
    SignedKeyTrusted = 5,
}

/// Provenance standing of an enrolled feed-signing key relative to the
/// operator-managed trust-root list. ROOT is membership on the root list;
/// DIRECT now requires a verifiable cryptographic chain: the keystore
/// record carries an ed25519 attestation over the key, signed by the root
/// key that was authoritative at enrollment, and the chain verifies against
/// the CURRENT root list. A broken chain (tampered signature, root removed,
/// root rotated away) honestly drops the standing to Unattested while the
/// record itself stays intact.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageKeyStanding {
    /// No valid trust chain: legacy pre-root records, attested records
    /// whose signature is missing/invalid, or records whose attesting root
    /// is no longer on the root list. Displayed honestly as such.
    Unattested = 0,
    /// Directly trusted by operator enrollment: the key id is on the ROOT list.
    Root = 1,
    /// Enrolled under a root regime with a verifiable attestation: the
    /// keystore record carries the enrolled-at tick, the attesting root id,
    /// and a root-signed ed25519 signature over the canonical attestation.
    Direct = 2,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageRepositorySyncState {
    Idle = 1,
    Ready = 2,
    Offline = 3,
    Failed = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageRepositoryTrustMode {
    Boot = 1,
    Unsigned = 2,
    PinnedDigest = 3,
    SignedKey = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageChannel {
    Stable = 1,
    Beta = 2,
    Canary = 3,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageRing {
    Production = 1,
    Preview = 2,
    Testing = 3,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageMaintenanceAction {
    Validate = 1,
    Repair = 2,
    GarbageCollect = 3,
}

#[cfg(test)]
mod tests {
    use super::PackageTag as T;

    /// Wire contract: the trust-root block must stay pinned at the END of the
    /// package tag space; renumbering any package tag breaks the protocol.
    #[test]
    fn package_trust_root_tag_wire_values() {
        assert_eq!(T::RolloutStatusReply as u32, 0x731);
        assert_eq!(T::RootListRequest as u32, 0x732);
        assert_eq!(T::RootListReply as u32, 0x733);
        assert_eq!(T::RootGetRequest as u32, 0x734);
        assert_eq!(T::RootGetReply as u32, 0x735);
        assert_eq!(T::RootAddRequest as u32, 0x736);
        assert_eq!(T::RootAddReply as u32, 0x737);
        assert_eq!(T::RootRemoveRequest as u32, 0x738);
        assert_eq!(T::RootRemoveReply as u32, 0x739);
    }

    /// NoKeyPair extends the status space at the END; pinned so the shell
    /// mapping (status_from_word) stays in lockstep with the service.
    #[test]
    fn package_no_key_pair_status_is_additive_tail() {
        assert_eq!(super::PackageStatus::AlreadyExists as u32, 15);
        assert_eq!(super::PackageStatus::NoKeyPair as u32, 16);
    }
}

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
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageTrustState {
    BootTrusted = 1,
    DigestPinned = 2,
    Unverified = 3,
    VerificationFailed = 4,
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

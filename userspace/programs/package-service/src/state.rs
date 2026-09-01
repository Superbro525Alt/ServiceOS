use rt::{
    IPC_MAX_WORDS, PackageChannel, PackageRepositorySyncState, PackageRepositoryTrustMode,
    PackageRing, PackageTrustState, ServiceId,
};
use serviceos_bundle::{BOOT_STORE_PATH_MAX, InlinePath, PackageManifest};
use serviceos_userspace_runtime as rt;

pub(crate) const MAX_INDEX_BYTES: usize = 512;
pub(crate) const MAX_PACKAGE_BYTES: usize = 2048;
pub(crate) const MAX_FEED_BYTES: usize = 4096;
pub(crate) const MAX_HTTP_BYTES: usize = 4096;
pub(crate) const MAX_STATE_BYTES: usize = 2048;
pub(crate) const MAX_PACKAGE_SLOTS: usize = 12;
pub(crate) const MAX_PACKAGE_VERSIONS: usize = 8;
pub(crate) const MAX_REPOSITORIES: usize = 4;
pub(crate) const BUILTIN_REPOSITORY_INDEX: usize = 0;
pub(crate) const REPO_NAME_MAX: usize = 24;
pub(crate) const REPO_URL_MAX: usize = 88;
pub(crate) const INSTALL_PATH_MAX: usize = BOOT_STORE_PATH_MAX;
pub(crate) const HTTP_TIMEOUT_TICKS: u64 = 600;
pub(crate) const HTTP_CHUNK_BYTES: usize = (IPC_MAX_WORDS - 2) * 8;
pub(crate) use crate::ops_model::{
    JOURNAL_INSTALL, JOURNAL_NONE, JOURNAL_REMOVE, JOURNAL_ROLLBACK, JOURNAL_SYSUPDATE,
    JOURNAL_UPDATE,
};

#[derive(Clone, Copy)]
pub(crate) struct PackageVersionSlot {
    pub(crate) manifest: PackageManifest,
    pub(crate) manifest_loaded: bool,
    pub(crate) repo_index: usize,
    pub(crate) repo_manifest_path: InlinePath,
    pub(crate) local_manifest_path: InlinePath,
    pub(crate) version: InlinePath,
    pub(crate) compatibility: InlinePath,
    pub(crate) category: InlinePath,
    pub(crate) summary: InlinePath,
    pub(crate) trust_state: PackageTrustState,
    pub(crate) occupied: bool,
}

impl PackageVersionSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            manifest: PackageManifest::empty(),
            manifest_loaded: false,
            repo_index: 0,
            repo_manifest_path: InlinePath::empty(),
            local_manifest_path: InlinePath::empty(),
            version: InlinePath::empty(),
            compatibility: InlinePath::empty(),
            category: InlinePath::empty(),
            summary: InlinePath::empty(),
            trust_state: PackageTrustState::BootTrusted,
            occupied: false,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct PackageSlot {
    pub(crate) service_id: ServiceId,
    pub(crate) package_name: InlinePath,
    pub(crate) versions: [PackageVersionSlot; MAX_PACKAGE_VERSIONS],
    pub(crate) version_count: usize,
    pub(crate) installed: Option<usize>,
    pub(crate) active: Option<usize>,
    pub(crate) rollback: Option<usize>,
    pub(crate) pin_version: InlinePath,
    pub(crate) channel: PackageChannel,
    pub(crate) ring: PackageRing,
    pub(crate) occupied: bool,
}

impl PackageSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            service_id: ServiceId::RootManager,
            package_name: InlinePath::empty(),
            versions: [PackageVersionSlot::empty(); MAX_PACKAGE_VERSIONS],
            version_count: 0,
            installed: None,
            active: None,
            rollback: None,
            pin_version: InlinePath::empty(),
            channel: PackageChannel::Stable,
            ring: PackageRing::Production,
            occupied: false,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct RepositorySlot {
    pub(crate) name: InlinePath,
    pub(crate) url: InlinePath,
    pub(crate) trust_mode: PackageRepositoryTrustMode,
    pub(crate) bound_key_id: crate::signing::FixedText<{ crate::signing::KEY_ID_MAX }>,
    pub(crate) bound_key_fingerprint: u64,
    pub(crate) sync_state: PackageRepositorySyncState,
    pub(crate) channel: PackageChannel,
    pub(crate) ring: PackageRing,
    pub(crate) enabled: bool,
    pub(crate) builtin: bool,
    pub(crate) pinned_digest: u64,
    pub(crate) last_digest: u64,
    pub(crate) package_count: u32,
    pub(crate) occupied: bool,
}

impl RepositorySlot {
    pub(crate) const fn empty() -> Self {
        Self {
            name: InlinePath::empty(),
            url: InlinePath::empty(),
            trust_mode: PackageRepositoryTrustMode::Unsigned,
            bound_key_id: crate::signing::FixedText::empty(),
            bound_key_fingerprint: 0,
            sync_state: PackageRepositorySyncState::Idle,
            channel: PackageChannel::Stable,
            ring: PackageRing::Production,
            enabled: false,
            builtin: false,
            pinned_digest: 0,
            last_digest: 0,
            package_count: 0,
            occupied: false,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct JournalState {
    pub(crate) pending_action: u32,
    pub(crate) service_id: ServiceId,
    pub(crate) version: InlinePath,
    pub(crate) manifest_path: InlinePath,
}

impl JournalState {
    pub(crate) const fn empty() -> Self {
        Self {
            pending_action: JOURNAL_NONE,
            service_id: ServiceId::RootManager,
            version: InlinePath::empty(),
            manifest_path: InlinePath::empty(),
        }
    }
}

pub(crate) static mut REPOSITORY_SLOTS: [RepositorySlot; MAX_REPOSITORIES] =
    [RepositorySlot::empty(); MAX_REPOSITORIES];
pub(crate) static mut PACKAGE_SLOTS: [PackageSlot; MAX_PACKAGE_SLOTS] =
    [PackageSlot::empty(); MAX_PACKAGE_SLOTS];
pub(crate) static mut JOURNAL_SLOT: JournalState = JournalState::empty();
static mut RECOVERY_STATE: Option<JournalState> = None;
pub(crate) static mut FEED_KEYSTORE: crate::signing::Keystore = crate::signing::Keystore::empty();
pub(crate) static mut REJECT_JOURNAL: crate::signing::RejectJournal =
    crate::signing::RejectJournal::empty();
pub(crate) static mut ROLLOUT_POLICY: crate::rollout::RolloutPolicy =
    crate::rollout::RolloutPolicy::empty();
pub(crate) static mut TRUST_ROOTS: crate::signing::TrustRoots = crate::signing::TrustRoots::empty();

/// Operator-managed trust-root list (service-local v0; no crypto chaining).
pub(crate) fn trust_roots() -> &'static crate::signing::TrustRoots {
    unsafe { &*core::ptr::addr_of!(TRUST_ROOTS) }
}

/// Keys pinned for a feed source, if any.
pub(crate) fn feed_keys_for(source: &str) -> Option<&'static crate::signing::SourceKeys> {
    let keystore = unsafe { &*core::ptr::addr_of!(FEED_KEYSTORE) };
    keystore.source_keys(source)
}

/// Staged-rollout/upgrade-rules row for a feed source, if configured.
/// Absence means the source is unstaged: every target admits.
pub(crate) fn rollout_policy_for(source: &str) -> Option<&'static crate::rollout::SourceRollout> {
    let policy = unsafe { &*core::ptr::addr_of!(ROLLOUT_POLICY) };
    policy.source_rollout(source)
}

/// Journal entry observed as stale during startup (interrupted operation),
/// kept for maintenance/recovery reporting until resumed or discarded.
pub(crate) fn set_recovery_state(recovery: Option<JournalState>) {
    unsafe {
        RECOVERY_STATE = recovery;
    }
}

pub(crate) fn recovery_state() -> Option<JournalState> {
    unsafe { RECOVERY_STATE }
}

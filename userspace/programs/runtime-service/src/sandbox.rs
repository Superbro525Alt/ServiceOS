//! S11 sandboxing groundwork: per-environment capability grant matrix over
//! device classes (network / graphics / input / audio).
//!
//! Matrix semantics (default-deny):
//! - A class is *requested* when the environment profile's `caps` line lists
//!   the matching capability bit.
//! - A class becomes *granted* only through an operator decision on the
//!   existing pending-approval flow (`EnvDecisionRequest`, audited as
//!   `RuntimeApprovalRequested` / `RuntimeApprovalChanged`).
//! - A class is usable by a compatibility workload only when requested AND
//!   granted; every other matrix cell denies.
//!
//! Enforcement points:
//! - Run launch (`handle_run_launch_request`) refuses workloads on
//!   environments whose matrix still has requested-but-ungranted classes
//!   (pending) or whose approval was denied; this is the contract-level gate
//!   available inside runtime-service today.
//! - Grant/revoke transitions stay inside the audited decision handler, so
//!   security-service retains the full approval trail.
//!
//! The input class is reserved groundwork: shared ABI `runtime_capability`
//! has no INPUT bit yet, so it cannot be requested through a profile and
//! stays denied until the wire contract grows one.
//!
//! The profile is persisted with the environment record (`EnvSlot::sandbox`)
//! for the lifetime of the environment and mirrored into `EnvStatusReply`
//! trailing words (requested/granted class masks) for inspection.

use serviceos_userspace_runtime as rt;

use crate::types::EnvSlot;

pub(crate) const CLASS_COUNT: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DeviceClass {
    Network,
    Graphics,
    Input,
    Audio,
}

pub(crate) const DEVICE_CLASSES: [DeviceClass; CLASS_COUNT] = [
    DeviceClass::Network,
    DeviceClass::Graphics,
    DeviceClass::Input,
    DeviceClass::Audio,
];

impl DeviceClass {
    pub(crate) fn index(self) -> usize {
        self as usize
    }

    /// Human-readable class label; inspection surfaces consume this later.
    #[cfg(test)]
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Graphics => "graphics",
            Self::Input => "input",
            Self::Audio => "audio",
        }
    }

    /// Wire bit backing the class, if one exists in the shared ABI yet.
    pub(crate) fn capability_bit(self) -> Option<u32> {
        match self {
            Self::Network => Some(rt::runtime_capability::NETWORK),
            Self::Graphics => Some(rt::runtime_capability::GRAPHICS),
            Self::Audio => Some(rt::runtime_capability::AUDIO),
            Self::Input => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_capability_bit(bit: u32) -> Option<Self> {
        DEVICE_CLASSES
            .iter()
            .copied()
            .find(|class| class.capability_bit() == Some(bit))
    }
}

/// Per-environment sandbox profile stored alongside the environment record.
#[derive(Clone, Copy)]
pub(crate) struct SandboxProfile {
    pub(crate) requested: [bool; CLASS_COUNT],
    pub(crate) granted: [bool; CLASS_COUNT],
}

impl SandboxProfile {
    pub(crate) const fn empty() -> Self {
        Self {
            requested: [false; CLASS_COUNT],
            granted: [false; CLASS_COUNT],
        }
    }

    /// Derive the matrix from capability bitmasks (requested from the env's
    /// declared capabilities, granted from its approved subset).
    pub(crate) fn from_masks(capabilities: u32, granted_caps: u32) -> Self {
        let mut profile = Self::empty();
        for class in DEVICE_CLASSES {
            if let Some(bit) = class.capability_bit()
                && capabilities & bit != 0
            {
                profile.request(class);
            }
        }
        profile.apply_granted_mask(granted_caps);
        profile
    }

    pub(crate) fn class_requested(&self, class: DeviceClass) -> bool {
        self.requested[class.index()]
    }

    pub(crate) fn class_granted(&self, class: DeviceClass) -> bool {
        self.granted[class.index()]
    }

    /// Matrix decision cell: default-deny; usable only when the workload
    /// declares the class AND an operator granted it.
    pub(crate) fn class_allowed(&self, class: DeviceClass) -> bool {
        self.requested[class.index()] && self.granted[class.index()]
    }

    /// Declare the class as part of the environment profile (`caps` line).
    pub(crate) fn request(&mut self, class: DeviceClass) {
        self.requested[class.index()] = true;
    }

    pub(crate) fn grant(&mut self, class: DeviceClass) {
        self.granted[class.index()] = true;
    }

    pub(crate) fn revoke_all(&mut self) {
        self.granted = [false; CLASS_COUNT];
    }

    /// Sync the granted column with an approval bitmask returned by the
    /// env-decision flow (revoking everything not present).
    pub(crate) fn apply_granted_mask(&mut self, granted_caps: u32) {
        self.revoke_all();
        for class in DEVICE_CLASSES {
            if let Some(bit) = class.capability_bit()
                && granted_caps & bit != 0
            {
                self.grant(class);
            }
        }
    }

    pub(crate) fn has_pending_classes(&self) -> bool {
        DEVICE_CLASSES
            .iter()
            .any(|class| self.class_requested(*class) && !self.class_allowed(*class))
    }

    pub(crate) fn requested_mask(&self) -> u32 {
        mask_of(&self.requested)
    }

    pub(crate) fn granted_mask(&self) -> u32 {
        mask_of(&self.granted)
    }
}

fn mask_of(fields: &[bool; CLASS_COUNT]) -> u32 {
    fields
        .iter()
        .enumerate()
        .fold(0u32, |mask, (index, set)| mask | (u32::from(*set) << index))
}

/// Launch-gate verdict derived from the environment record plus its matrix.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LaunchDecision {
    Allowed,
    PendingApproval,
    Denied,
}

pub(crate) fn launch_decision(env: &EnvSlot) -> LaunchDecision {
    if !env.occupied || matches!(env.state, rt::RuntimeEnvState::Denied) {
        return LaunchDecision::Denied;
    }
    if env.sandbox.has_pending_classes() {
        return LaunchDecision::PendingApproval;
    }
    LaunchDecision::Allowed
}

/// Wire version of the per-workload sandbox manifest blob grammar. Bumping
/// this value changes the blob layout; the launch envelope header word
/// carries it so old services refuse new manifests loudly (and vice versa).
pub(crate) const SANDBOX_MANIFEST_VERSION: u8 = 1;

/// Fixed v1 blob size: exactly one packed envelope word.
pub(crate) const SANDBOX_MANIFEST_BLOB_LEN: usize = 8;

/// Flag bit in blob byte 1: bytes 4..8 carry a capability allow-mask.
const MANIFEST_FLAG_CAPS_ALLOW: u8 = 0b0000_0001;

/// Class-mask width in blob byte 2: one bit per known device class.
const MANIFEST_GRANT_MASK: u8 = (1 << CLASS_COUNT) - 1;

/// Why a manifest was refused. Wire mapping: every variant refuses the
/// launch with `RuntimeStatus::Unsupported` — there is no silent fallback
/// onto the environment-level profile.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ManifestError {
    /// Bad length, unknown version, unknown flag bits, nonzero reserved
    /// byte, or a launch envelope that does not exactly fit the manifest.
    Malformed,
    /// A grant bit outside the known device-class vocabulary.
    UnknownClass,
    /// The manifest tries to widen beyond the environment-level profile
    /// (granting a class the operator never granted, or a capability the
    /// environment never declared).
    Widening,
}

/// Per-workload sandbox manifest: the workload's own declared least-privilege
/// subset of its environment's sandbox profile. Same device-class vocabulary
/// as the environment matrix (class-index bits, see `SandboxProfile`), plus
/// an optional narrower capability allow-mask. The manifest may only NARROW:
/// the effective profile is the intersection computed by `effective_profile`.
///
/// Honest enforcement scope: the environment profile constrains device
/// classes at the runtime-service level only (launch gate), and so does the
/// manifest — it is a runtime-service-level document. It cannot widen any
/// environment grant, cannot cure pending approvals, and cannot un-deny a
/// denied environment; kernel-visible task attributes are unchanged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct SandboxManifest {
    pub(crate) version: u8,
    /// Device classes this workload may use: granted by the operator AND
    /// present here. Absent bits narrow; bits never widen.
    pub(crate) grants: [bool; CLASS_COUNT],
    /// When present, the workload's capability set is the environment's
    /// capabilities intersected with this mask.
    pub(crate) caps_allow: Option<u32>,
}

impl SandboxManifest {
    /// The manifest carried by workloads that declare no restrictions of
    /// their own... does not exist: absence of a manifest is identity, and
    /// an explicit all-empty manifest narrows every device class away.
    pub(crate) fn decode(blob: &[u8]) -> Result<Self, ManifestError> {
        if blob.len() != SANDBOX_MANIFEST_BLOB_LEN {
            return Err(ManifestError::Malformed);
        }
        if blob[0] != SANDBOX_MANIFEST_VERSION {
            return Err(ManifestError::Malformed);
        }
        if blob[1] & !MANIFEST_FLAG_CAPS_ALLOW != 0 {
            return Err(ManifestError::Malformed);
        }
        if blob[2] & !MANIFEST_GRANT_MASK != 0 {
            return Err(ManifestError::UnknownClass);
        }
        if blob[3] != 0 {
            return Err(ManifestError::Malformed);
        }
        let caps_raw = u32::from_le_bytes([blob[4], blob[5], blob[6], blob[7]]);
        let caps_allow = if blob[1] & MANIFEST_FLAG_CAPS_ALLOW != 0 {
            Some(caps_raw)
        } else {
            if caps_raw != 0 {
                return Err(ManifestError::Malformed);
            }
            None
        };
        let mut grants = [false; CLASS_COUNT];
        for (index, slot) in grants.iter_mut().enumerate() {
            *slot = blob[2] & (1 << index) != 0;
        }
        Ok(Self {
            version: blob[0],
            grants,
            caps_allow,
        })
    }

    /// Inverse of `decode`; the pair is the codec under test.
    pub(crate) fn encode(&self) -> [u8; SANDBOX_MANIFEST_BLOB_LEN] {
        let mut blob = [0u8; SANDBOX_MANIFEST_BLOB_LEN];
        blob[0] = self.version;
        if self.caps_allow.is_some() {
            blob[1] = MANIFEST_FLAG_CAPS_ALLOW;
        }
        for (index, set) in self.grants.iter().enumerate() {
            if *set {
                blob[2] |= 1 << index;
            }
        }
        if let Some(mask) = self.caps_allow {
            blob[4..8].copy_from_slice(&mask.to_le_bytes());
        }
        blob
    }

    pub(crate) fn allows_class(&self, class: DeviceClass) -> bool {
        self.grants[class.index()]
    }

    pub(crate) fn grants_mask(&self) -> u32 {
        mask_of(&self.grants)
    }

    /// Narrow-only validation against the environment record: every granted
    /// class must be requested AND granted by the environment matrix, and a
    /// capability allow-mask must stay inside the environment's declared
    /// capabilities. Anything else is a widening attempt.
    pub(crate) fn validate_against_env(&self, env: &EnvSlot) -> Result<(), ManifestError> {
        for class in DEVICE_CLASSES {
            if self.allows_class(class)
                && (!env.sandbox.class_requested(class) || !env.sandbox.class_granted(class))
            {
                return Err(ManifestError::Widening);
            }
        }
        if let Some(mask) = self.caps_allow
            && mask & !env.capabilities != 0
        {
            return Err(ManifestError::Widening);
        }
        Ok(())
    }

    /// Decode the optional manifest from a `RunLaunchRequest` envelope.
    /// Layout: words[0..3] header, `ceil(arg_len/8)` packed arg words, then
    /// optionally one header word (`blob_len | version << 56`) plus one
    /// packed blob word. Presence is detected by word_count only, so legacy
    /// messages decode as `None` and legacy receivers ignore the trailing
    /// words (`unpack_bytes` bounds on arg_len). Trailing junk, partial
    /// manifests, and unknown versions are refused — never ignored.
    pub(crate) fn from_launch_words(
        words: &[u64; rt::IPC_MAX_WORDS],
        arg_len: usize,
        word_count: usize,
    ) -> Result<Option<Self>, ManifestError> {
        let header_index = 3 + arg_len.div_ceil(8);
        if word_count <= header_index {
            return Ok(None);
        }
        // Exact fit: one header word + one packed blob word, nothing else.
        if word_count != header_index + 2 {
            return Err(ManifestError::Malformed);
        }
        let expected_header =
            u64::from(SANDBOX_MANIFEST_VERSION) << 56 | SANDBOX_MANIFEST_BLOB_LEN as u64;
        if words[header_index] != expected_header {
            return Err(ManifestError::Malformed);
        }
        Self::decode(&words[header_index + 1].to_le_bytes()).map(Some)
    }

    /// The launch-envelope header word matching `from_launch_words`.
    pub(crate) fn launch_header_word() -> u64 {
        u64::from(SANDBOX_MANIFEST_VERSION) << 56 | SANDBOX_MANIFEST_BLOB_LEN as u64
    }
}

/// Environment profile intersected with an optional workload manifest —
/// the effective profile one pure function computes. Verdict semantics:
/// pending stays an environment-level notion (a manifest never cures an
/// ungranted request and never un-denies a denied environment); the
/// manifest narrows the certified class set and the capability set.
pub(crate) struct EffectiveProfile {
    pub(crate) requested: [bool; CLASS_COUNT],
    pub(crate) granted: [bool; CLASS_COUNT],
    /// Matrix decision cells for this workload: environment-allowed AND
    /// manifest-granted.
    pub(crate) certified: [bool; CLASS_COUNT],
    pub(crate) capabilities: u32,
}

impl EffectiveProfile {
    pub(crate) fn has_pending_classes(&self) -> bool {
        (0..CLASS_COUNT).any(|index| self.requested[index] && !self.granted[index])
    }

    pub(crate) fn certified_mask(&self) -> u32 {
        mask_of(&self.certified)
    }
}

pub(crate) fn effective_profile(
    env: &EnvSlot,
    manifest: Option<&SandboxManifest>,
) -> EffectiveProfile {
    let mut effective = EffectiveProfile {
        requested: env.sandbox.requested,
        granted: env.sandbox.granted,
        certified: [false; CLASS_COUNT],
        capabilities: env.capabilities,
    };
    for class in DEVICE_CLASSES {
        let index = class.index();
        effective.certified[index] = env.sandbox.class_allowed(class)
            && manifest.is_none_or(|declared| declared.allows_class(class));
    }
    if let Some(mask) = manifest.and_then(|declared| declared.caps_allow) {
        effective.capabilities &= mask;
    }
    effective
}

/// Launch verdict for a workload manifest that has already been decoded and
/// (for a first presentation) validated against the environment.
pub(crate) fn launch_decision_with_manifest(
    env: &EnvSlot,
    manifest: Option<&SandboxManifest>,
) -> LaunchDecision {
    let effective = effective_profile(env, manifest);
    if !env.occupied || matches!(env.state, rt::RuntimeEnvState::Denied) {
        return LaunchDecision::Denied;
    }
    if effective.has_pending_classes() {
        return LaunchDecision::PendingApproval;
    }
    LaunchDecision::Allowed
}

/// Handler-level launch gate: decode outcome plus latching rules, kept pure
/// so the whole manifest contract is host-testable. Refusals map to one
/// distinct status (`Unsupported`); there is no silent fallback.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum GateOutcome {
    Proceed(LaunchDecision),
    Refuse(rt::RuntimeStatus),
}

/// Gate one launch against the environment and its optional manifest.
///
/// Latching rules (the workload→manifest association rides the env record):
/// - the first manifest-carrying launch validates and latches the manifest;
/// - later launches must present the exact same manifest (a different one
///   refuses — no per-launch permission shopping);
/// - once latched, guest-image launches must present the manifest (a bare
///   launch would silently shed the workload's own declaration); hosted
///   housekeeping workloads are unaffected.
pub(crate) fn gate_launch(
    env: &mut EnvSlot,
    manifest: Option<SandboxManifest>,
    guest_exec: bool,
) -> GateOutcome {
    match manifest {
        Some(requested) => match env.manifest {
            Some(latched) if latched == requested => {}
            Some(_) => return GateOutcome::Refuse(rt::RuntimeStatus::Unsupported),
            None => {
                if requested.validate_against_env(env).is_err() {
                    return GateOutcome::Refuse(rt::RuntimeStatus::Unsupported);
                }
                env.manifest = Some(requested);
            }
        },
        None => {
            if env.manifest.is_some() && guest_exec {
                return GateOutcome::Refuse(rt::RuntimeStatus::Unsupported);
            }
        }
    }
    GateOutcome::Proceed(launch_decision_with_manifest(env, env.manifest.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NET: u32 = rt::runtime_capability::NETWORK;
    const GFX: u32 = rt::runtime_capability::GRAPHICS;
    const AUD: u32 = rt::runtime_capability::AUDIO;
    const ALL_WIRE_BITS: u32 = NET | GFX | AUD;

    fn ready_env(capabilities: u32, granted_caps: u32) -> EnvSlot {
        let mut env = EnvSlot::empty();
        env.occupied = true;
        env.state = rt::RuntimeEnvState::Ready;
        env.capabilities = capabilities;
        env.granted_caps = granted_caps;
        env.sandbox = SandboxProfile::from_masks(capabilities, granted_caps);
        env
    }

    #[test]
    fn manifest_codec_roundtrips() {
        let manifest = SandboxManifest {
            version: SANDBOX_MANIFEST_VERSION,
            grants: [true, false, false, true],
            caps_allow: Some(rt::runtime_capability::FILE_READ | NET),
        };
        let blob = manifest.encode();
        assert_eq!(blob.len(), SANDBOX_MANIFEST_BLOB_LEN);
        assert_eq!(SandboxManifest::decode(&blob), Ok(manifest));

        let bare = SandboxManifest {
            version: SANDBOX_MANIFEST_VERSION,
            grants: [false; CLASS_COUNT],
            caps_allow: None,
        };
        assert_eq!(SandboxManifest::decode(&bare.encode()), Ok(bare));
        // Fixed grammar: version in byte 0, flags in byte 1, grants in
        // byte 2, reserved byte 3 stays zero, allow-mask little-endian.
        assert_eq!(bare.encode()[0], SANDBOX_MANIFEST_VERSION);
        assert_eq!(bare.encode()[3], 0);
    }

    #[test]
    fn manifest_decode_rejects_malformed_blobs() {
        let good = SandboxManifest {
            version: SANDBOX_MANIFEST_VERSION,
            grants: [true, false, false, false],
            caps_allow: None,
        }
        .encode();
        assert_eq!(
            SandboxManifest::decode(&good[..7]),
            Err(ManifestError::Malformed)
        );
        assert_eq!(SandboxManifest::decode(&[]), Err(ManifestError::Malformed));

        let mut bad_version = good;
        bad_version[0] = 2;
        assert_eq!(
            SandboxManifest::decode(&bad_version),
            Err(ManifestError::Malformed)
        );

        let mut bad_flags = good;
        bad_flags[1] = 0b0000_0010;
        assert_eq!(
            SandboxManifest::decode(&bad_flags),
            Err(ManifestError::Malformed)
        );

        let mut bad_reserved = good;
        bad_reserved[3] = 1;
        assert_eq!(
            SandboxManifest::decode(&bad_reserved),
            Err(ManifestError::Malformed)
        );

        // Allow-mask bytes without the flag bit are malformed, not ignored.
        let mut ghost_caps = good;
        ghost_caps[4] = 1;
        assert_eq!(
            SandboxManifest::decode(&ghost_caps),
            Err(ManifestError::Malformed)
        );
    }

    #[test]
    fn manifest_decode_rejects_unknown_class_bits() {
        let mut unknown = SandboxManifest {
            version: SANDBOX_MANIFEST_VERSION,
            grants: [false; CLASS_COUNT],
            caps_allow: None,
        }
        .encode();
        unknown[2] |= 1 << CLASS_COUNT;
        assert_eq!(
            SandboxManifest::decode(&unknown),
            Err(ManifestError::UnknownClass)
        );
        unknown[2] = 0xff;
        assert_eq!(
            SandboxManifest::decode(&unknown),
            Err(ManifestError::UnknownClass)
        );
    }

    #[test]
    fn manifest_validation_rejects_widening() {
        // Granted by nobody: granting network widens past the operator.
        let ungranted = ready_env(NET | GFX, 0);
        let manifest = SandboxManifest {
            version: SANDBOX_MANIFEST_VERSION,
            grants: [true, false, false, false],
            caps_allow: None,
        };
        assert_eq!(
            manifest.validate_against_env(&ungranted),
            Err(ManifestError::Widening)
        );

        // Granted but never requested: structurally impossible through the
        // decision flow, yet the validator refuses it defensively.
        let odd = ready_env(NET, NET | GFX);
        let gfx_manifest = SandboxManifest {
            version: SANDBOX_MANIFEST_VERSION,
            grants: [false, true, false, false],
            caps_allow: None,
        };
        assert_eq!(
            gfx_manifest.validate_against_env(&odd),
            Err(ManifestError::Widening)
        );

        // Capability allow-mask reaching outside the environment's caps.
        let env = ready_env(rt::runtime_capability::FILE_READ | NET, NET);
        let widening_caps = SandboxManifest {
            version: SANDBOX_MANIFEST_VERSION,
            grants: [true, false, false, false],
            caps_allow: Some(rt::runtime_capability::FILE_READ | GFX),
        };
        assert_eq!(
            widening_caps.validate_against_env(&env),
            Err(ManifestError::Widening)
        );
        assert!(
            widening_caps
                .validate_against_env(&ready_env(0, 0))
                .is_err()
        );
    }

    #[test]
    fn effective_profile_intersects_narrow_only() {
        let env = ready_env(rt::runtime_capability::FILE_READ | NET | GFX, NET | GFX);

        // No manifest: effective profile is the environment profile itself.
        let identity = effective_profile(&env, None);
        for class in DEVICE_CLASSES {
            assert_eq!(
                identity.certified[class.index()],
                env.sandbox.class_allowed(class)
            );
        }
        assert_eq!(identity.capabilities, env.capabilities);

        // Manifest narrows to network only and clips capabilities.
        let manifest = SandboxManifest {
            version: SANDBOX_MANIFEST_VERSION,
            grants: [true, false, false, false],
            caps_allow: Some(rt::runtime_capability::FILE_READ),
        };
        let narrowed = effective_profile(&env, Some(&manifest));
        assert!(narrowed.certified[DeviceClass::Network.index()]);
        assert!(!narrowed.certified[DeviceClass::Graphics.index()]);
        // Narrowing never touches the operator's request/grant columns.
        assert_eq!(narrowed.requested, env.sandbox.requested);
        assert_eq!(narrowed.granted, env.sandbox.granted);
        assert_eq!(narrowed.capabilities, rt::runtime_capability::FILE_READ);
        assert_eq!(narrowed.certified_mask(), 1 << DeviceClass::Network.index());
    }

    #[test]
    fn manifest_launch_words_decode_absent_present_and_malformed() {
        let manifest = SandboxManifest {
            version: SANDBOX_MANIFEST_VERSION,
            grants: [true, false, false, false],
            caps_allow: None,
        };
        let arg = b"/bin/demo";

        // Legacy envelope: word_count stops after the packed arg bytes.
        let arg_words = arg.len().div_ceil(8);
        let mut words = [0u64; rt::IPC_MAX_WORDS];
        let header_word = SandboxManifest::launch_header_word();
        let blob_word = u64::from_le_bytes(manifest.encode());
        words[3..3 + arg_words].copy_from_slice(&[0xdead_beef, 0]);
        assert_eq!(
            SandboxManifest::from_launch_words(&words, arg.len(), 3 + arg_words),
            Ok(None)
        );

        // Manifest-carrying envelope: header + one packed blob word.
        words[3 + arg_words] = header_word;
        words[3 + arg_words + 1] = blob_word;
        assert_eq!(
            SandboxManifest::from_launch_words(&words, arg.len(), 3 + arg_words + 2),
            Ok(Some(manifest))
        );

        // Trailing junk (partial manifest) is malformed, never ignored.
        assert_eq!(
            SandboxManifest::from_launch_words(&words, arg.len(), 3 + arg_words + 1),
            Err(ManifestError::Malformed)
        );
        assert_eq!(
            SandboxManifest::from_launch_words(&words, arg.len(), 3 + arg_words + 3),
            Err(ManifestError::Malformed)
        );

        // Wrong header word or version refuses loudly.
        words[3 + arg_words] = 0;
        assert_eq!(
            SandboxManifest::from_launch_words(&words, arg.len(), 3 + arg_words + 2),
            Err(ManifestError::Malformed)
        );
        words[3 + arg_words] =
            (u64::from(SANDBOX_MANIFEST_VERSION) + 1) << 56 | SANDBOX_MANIFEST_BLOB_LEN as u64;
        assert_eq!(
            SandboxManifest::from_launch_words(&words, arg.len(), 3 + arg_words + 2),
            Err(ManifestError::Malformed)
        );
    }

    #[test]
    fn manifest_envelope_budget_fits_sixteen_words() {
        // Envelope budget: 3 header words + arg words + 2 manifest words
        // must fit IPC_MAX_WORDS, so a manifest-carrying launch caps its
        // argument at 11 words (88 bytes).
        assert_eq!(3 + 11 + 2, rt::IPC_MAX_WORDS);

        // An argument that fills the whole envelope cannot carry a
        // manifest: the sender-side budget contract keeps such launches
        // manifest-less instead of truncating silently.
        let full_arg_len = (rt::IPC_MAX_WORDS - 3) * 8;
        let words = [0u64; rt::IPC_MAX_WORDS];
        assert_eq!(
            SandboxManifest::from_launch_words(&words, full_arg_len, rt::IPC_MAX_WORDS),
            Ok(None)
        );
    }

    #[test]
    fn gate_launch_latches_and_requires_exact_manifest() {
        let mut env = ready_env(NET | GFX, NET | GFX);
        let manifest = SandboxManifest {
            version: SANDBOX_MANIFEST_VERSION,
            grants: [true, false, false, false],
            caps_allow: None,
        };

        // First manifest-carrying launch validates and latches.
        assert!(matches!(
            gate_launch(&mut env, Some(manifest), true),
            GateOutcome::Proceed(LaunchDecision::Allowed)
        ));
        assert_eq!(env.manifest, Some(manifest));

        // The exact same declaration relaunches fine.
        assert!(matches!(
            gate_launch(&mut env, Some(manifest), true),
            GateOutcome::Proceed(LaunchDecision::Allowed)
        ));

        // A different manifest is a permission-shopping attempt.
        let other = SandboxManifest {
            version: SANDBOX_MANIFEST_VERSION,
            grants: [false, false, false, false],
            caps_allow: None,
        };
        assert_eq!(
            gate_launch(&mut env, Some(other), true),
            GateOutcome::Refuse(rt::RuntimeStatus::Unsupported)
        );
        assert_eq!(env.manifest, Some(manifest));

        // Guest-image launches must present the declared manifest; hosted
        // housekeeping workloads stay unaffected.
        assert_eq!(
            gate_launch(&mut env, None, true),
            GateOutcome::Refuse(rt::RuntimeStatus::Unsupported)
        );
        assert!(matches!(
            gate_launch(&mut env, None, false),
            GateOutcome::Proceed(LaunchDecision::Allowed)
        ));
    }

    #[test]
    fn gate_launch_refuses_widening_without_latching() {
        let mut env = ready_env(NET | GFX, NET);
        let manifest = SandboxManifest {
            version: SANDBOX_MANIFEST_VERSION,
            grants: [true, true, false, false],
            caps_allow: None,
        };
        assert_eq!(
            gate_launch(&mut env, Some(manifest), true),
            GateOutcome::Refuse(rt::RuntimeStatus::Unsupported)
        );
        // Refused manifests never associate with the environment.
        assert_eq!(env.manifest, None);
    }

    #[test]
    fn gate_launch_without_manifest_is_byte_identical() {
        // Pending env.
        let mut pending = ready_env(NET, 0);
        assert!(matches!(
            gate_launch(&mut pending, None, true),
            GateOutcome::Proceed(LaunchDecision::PendingApproval)
        ));
        assert_eq!(pending.manifest, None);

        // Denied env.
        let mut denied = ready_env(NET, NET);
        denied.state = rt::RuntimeEnvState::Denied;
        assert!(matches!(
            gate_launch(&mut denied, None, true),
            GateOutcome::Proceed(LaunchDecision::Denied)
        ));

        // Clean env.
        let mut clean = ready_env(0, 0);
        assert!(matches!(
            gate_launch(&mut clean, None, true),
            GateOutcome::Proceed(LaunchDecision::Allowed)
        ));
        // Verdicts match the legacy decision function in every case.
        assert_eq!(
            gate_launch(&mut pending, None, true),
            GateOutcome::Proceed(launch_decision(&pending))
        );
        assert_eq!(
            gate_launch(&mut denied, None, true),
            GateOutcome::Proceed(launch_decision(&denied))
        );
        assert_eq!(
            gate_launch(&mut clean, None, true),
            GateOutcome::Proceed(launch_decision(&clean))
        );
    }

    #[test]
    fn launch_decision_with_manifest_narrows_but_never_cures() {
        // A manifest narrowing the certified set still launches cleanly...
        let mut env = ready_env(NET | GFX, NET | GFX);
        let manifest = SandboxManifest {
            version: SANDBOX_MANIFEST_VERSION,
            grants: [true, false, false, false],
            caps_allow: None,
        };
        assert!(matches!(
            gate_launch(&mut env, Some(manifest), true),
            GateOutcome::Proceed(LaunchDecision::Allowed)
        ));

        // ...but it cannot cure an env-level pending approval, and an
        // operator revocation shrinks the certified set under the same
        // manifest instead of refusing it.
        let pending = ready_env(NET | GFX, NET);
        assert!(matches!(
            launch_decision_with_manifest(&pending, Some(&manifest)),
            LaunchDecision::PendingApproval
        ));
    }

    #[test]
    fn default_matrix_denies_every_class() {
        let profile = SandboxProfile::empty();
        for class in DEVICE_CLASSES {
            assert!(!profile.class_requested(class));
            assert!(!profile.class_granted(class));
            assert!(!profile.class_allowed(class), "{} must deny", class.name());
        }
        assert!(!profile.has_pending_classes());
        assert_eq!(profile.requested_mask(), 0);
        assert_eq!(profile.granted_mask(), 0);
    }

    #[test]
    fn from_masks_marks_requested_without_granting() {
        let profile = SandboxProfile::from_masks(ALL_WIRE_BITS, 0);
        assert!(profile.class_requested(DeviceClass::Network));
        assert!(profile.class_requested(DeviceClass::Graphics));
        assert!(profile.class_requested(DeviceClass::Audio));
        assert!(!profile.class_requested(DeviceClass::Input));
        for class in DEVICE_CLASSES {
            assert!(!profile.class_granted(class));
            assert!(!profile.class_allowed(class));
        }
        assert!(profile.has_pending_classes());
    }

    #[test]
    fn grant_and_revoke_transitions_track_matrix_cells() {
        let mut profile = SandboxProfile::from_masks(ALL_WIRE_BITS, 0);
        assert!(matches!(
            launch_decision_for(&profile),
            LaunchDecision::PendingApproval
        ));

        profile.grant(DeviceClass::Network);
        assert!(profile.class_allowed(DeviceClass::Network));
        assert!(!profile.class_allowed(DeviceClass::Graphics));
        assert!(!profile.class_allowed(DeviceClass::Audio));
        assert!(matches!(
            launch_decision_for(&profile),
            LaunchDecision::PendingApproval
        ));

        profile.grant(DeviceClass::Graphics);
        profile.grant(DeviceClass::Audio);
        assert!(!profile.has_pending_classes());

        profile.revoke_all();
        for class in DEVICE_CLASSES {
            if class == DeviceClass::Input {
                assert!(!profile.class_requested(class));
            } else {
                assert!(profile.class_requested(class));
            }
            assert!(!profile.class_granted(class));
            assert!(!profile.class_allowed(class));
        }
        assert!(matches!(
            launch_decision_for(&profile),
            LaunchDecision::PendingApproval
        ));
    }

    #[test]
    fn input_class_has_no_wire_bit_and_stays_denied() {
        assert_eq!(DeviceClass::Input.capability_bit(), None);
        assert_eq!(DeviceClass::from_capability_bit(1 << 5), None);
        assert_eq!(
            DeviceClass::from_capability_bit(NET),
            Some(DeviceClass::Network)
        );

        let profile = SandboxProfile::from_masks(u32::MAX, u32::MAX);
        assert!(!profile.class_requested(DeviceClass::Input));
        assert!(!profile.class_granted(DeviceClass::Input));
        assert!(!profile.class_allowed(DeviceClass::Input));
    }

    #[test]
    fn launch_decision_reflects_state_and_matrix() {
        let mut env = crate::types::EnvSlot::empty();
        assert!(matches!(launch_decision(&env), LaunchDecision::Denied));

        env.occupied = true;
        env.state = rt::RuntimeEnvState::Ready;
        env.sandbox = SandboxProfile::empty();
        assert!(matches!(launch_decision(&env), LaunchDecision::Allowed));

        env.state = rt::RuntimeEnvState::PendingApproval;
        env.capabilities = ALL_WIRE_BITS;
        env.sandbox = SandboxProfile::from_masks(ALL_WIRE_BITS, NET);
        assert!(matches!(
            launch_decision(&env),
            LaunchDecision::PendingApproval
        ));

        env.sandbox.grant(DeviceClass::Graphics);
        env.sandbox.grant(DeviceClass::Audio);
        assert!(matches!(launch_decision(&env), LaunchDecision::Allowed));

        env.state = rt::RuntimeEnvState::Denied;
        assert!(matches!(launch_decision(&env), LaunchDecision::Denied));
    }

    #[test]
    fn instantiate_then_decide_keeps_sandbox_synced() {
        let mut profile = crate::types::Profile::empty();
        profile.kind = rt::RuntimeKind::Posix;
        profile.capabilities = rt::runtime_capability::FILE_READ | NET | AUD;
        let env = crate::util::instantiate_env(profile);
        assert!(env.sandbox.class_requested(DeviceClass::Network));
        assert!(!env.sandbox.class_requested(DeviceClass::Graphics));
        assert!(matches!(
            launch_decision(&env),
            LaunchDecision::PendingApproval
        ));

        let (_, granted) = crate::protocol::apply_decision(
            profile.capabilities,
            0,
            rt::PermissionPolicyState::Allowed,
            Some(NET),
        );
        let synced = SandboxProfile::from_masks(profile.capabilities, granted);
        assert!(synced.class_allowed(DeviceClass::Network));
        assert!(!synced.class_allowed(DeviceClass::Audio));
        assert!(synced.has_pending_classes());
    }

    #[test]
    fn status_masks_pack_class_index_bits() {
        let mut profile = SandboxProfile::from_masks(NET | AUD, 0);
        assert_eq!(
            profile.requested_mask(),
            (1 << DeviceClass::Network.index()) | (1 << DeviceClass::Audio.index())
        );
        assert_eq!(profile.granted_mask(), 0);
        profile.grant(DeviceClass::Audio);
        assert_eq!(profile.granted_mask(), 1 << DeviceClass::Audio.index());
    }

    fn launch_decision_for(profile: &SandboxProfile) -> LaunchDecision {
        let mut env = EnvSlot::empty();
        env.occupied = true;
        env.state = if profile.has_pending_classes() {
            rt::RuntimeEnvState::PendingApproval
        } else {
            rt::RuntimeEnvState::Ready
        };
        env.sandbox = *profile;
        launch_decision(&env)
    }
}

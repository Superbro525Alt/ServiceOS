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

    #[cfg(test)]
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

#[cfg(test)]
mod tests {
    use super::*;

    const NET: u32 = rt::runtime_capability::NETWORK;
    const GFX: u32 = rt::runtime_capability::GRAPHICS;
    const AUD: u32 = rt::runtime_capability::AUDIO;
    const ALL_WIRE_BITS: u32 = NET | GFX | AUD;

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

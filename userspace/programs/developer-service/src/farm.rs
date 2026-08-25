use rt::{DeveloperTarget, DeveloperToolchainState, ServiceId};
use serviceos_userspace_runtime as rt;

use crate::{
    consts::{MAX_PATH, MAX_TOOLCHAINS},
    types::{FixedBytes, ToolchainSlot},
};

/// Farm dispatch status codes carried in reply words and log details:
/// registered (queued at the endpoint), not-configured (descriptor carries
/// no remote_endpoint), unreachable (no network transport to dispatch).
pub(crate) const FARM_STATUS_REGISTERED: u64 = 0;
pub(crate) const FARM_STATUS_NOT_CONFIGURED: u64 = 1;
pub(crate) const FARM_STATUS_UNREACHABLE: u64 = 2;

/// One registry record per remote-only toolchain descriptor, keyed by its
/// target; the descriptor name stays on the toolchain table entry at the
/// same index, so a farm record is always named through its toolchain.
#[derive(Clone, Copy)]
pub(crate) struct FarmEndpoint {
    pub(crate) occupied: bool,
    pub(crate) target: DeveloperTarget,
    pub(crate) endpoint: FixedBytes<MAX_PATH>,
}

impl FarmEndpoint {
    pub(crate) const fn empty() -> Self {
        Self {
            occupied: false,
            target: DeveloperTarget::NativeX64,
            endpoint: FixedBytes::empty(),
        }
    }
}

/// Derive farm records from the loaded toolchain catalog: every remote-only
/// descriptor registers one endpoint record keyed by its target.
pub(crate) fn build_farm(
    toolchains: &[ToolchainSlot],
    count: usize,
) -> [FarmEndpoint; MAX_TOOLCHAINS] {
    let mut records = [FarmEndpoint::empty(); MAX_TOOLCHAINS];
    for (index, slot) in toolchains[..count.min(MAX_TOOLCHAINS)].iter().enumerate() {
        if !slot.occupied || slot.state != DeveloperToolchainState::RemoteOnly {
            continue;
        }
        records[index] = FarmEndpoint {
            occupied: true,
            target: slot.target,
            endpoint: slot.remote_endpoint,
        };
    }
    records
}

/// First registered endpoint serving `target`, with its registry index.
pub(crate) fn endpoint_for_target(
    records: &[FarmEndpoint; MAX_TOOLCHAINS],
    target: DeveloperTarget,
) -> Option<(usize, &FarmEndpoint)> {
    records
        .iter()
        .enumerate()
        .find(|(_, record)| record.occupied && record.target == target)
}

pub(crate) enum DispatchOutcome {
    NotConfigured,
    Unreachable,
    Registered,
}

/// Dispatch decision for a remote-target build: an unconfigured endpoint is
/// an explicit refusal; a configured endpoint without transport is reported
/// unreachable rather than failing obscurely later.
pub(crate) fn dispatch_outcome(endpoint_text: &[u8], transport_up: bool) -> DispatchOutcome {
    if endpoint_text.is_empty() {
        return DispatchOutcome::NotConfigured;
    }
    if !transport_up {
        return DispatchOutcome::Unreachable;
    }
    DispatchOutcome::Registered
}

/// Transport probe: remote dispatch requires the network service; when it
/// does not answer, every configured endpoint counts as unreachable.
pub(crate) fn probe_transport(bootstrap: rt::Handle) -> bool {
    match rt::lookup_service(bootstrap, ServiceId::Network) {
        Ok(handle) => {
            let _ = rt::handle_close(handle);
            true
        }
        Err(_) => false,
    }
}

/// Bitmask over workspace target slots whose toolchain descriptor registers
/// a configured remote endpoint: bit 0 native, bit 1 linux, bit 2 windows,
/// bit 3 macos.
pub(crate) fn configured_mask(toolchains: &[ToolchainSlot], count: usize) -> u32 {
    let mut mask = 0u32;
    for (index, slot) in toolchains[..count.min(MAX_TOOLCHAINS)].iter().enumerate() {
        if slot.occupied && slot.state == DeveloperToolchainState::RemoteOnly && slot.configured() {
            mask |= 1u32 << index.min(31);
        }
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(name: &[u8], state: DeveloperToolchainState) -> ToolchainSlot {
        let mut slot = ToolchainSlot::empty();
        slot.occupied = true;
        let _ = slot.name.set(name);
        slot.state = state;
        slot
    }

    #[test]
    fn build_farm_keeps_only_remote_only_descriptors() {
        let mut remote = slot(b"macos-x64", DeveloperToolchainState::RemoteOnly);
        remote.target = DeveloperTarget::MacosX64;
        let local = slot(b"linux-x64", DeveloperToolchainState::Installed);
        let records = build_farm(&[remote, local], 2);
        assert!(records[0].occupied);
        assert_eq!(records[0].target, DeveloperTarget::MacosX64);
        assert!(!records[1].occupied);
    }

    #[test]
    fn endpoint_record_carries_config_text() {
        let mut remote = slot(b"macos-x64", DeveloperToolchainState::RemoteOnly);
        remote.target = DeveloperTarget::MacosX64;
        let _ = remote.remote_endpoint.set(b"farm@10.0.0.9:7900");
        let records = build_farm(&[remote], 1);
        assert_eq!(records[0].endpoint.as_bytes(), b"farm@10.0.0.9:7900");
    }

    #[test]
    fn endpoint_missing_until_descriptor_provides_one() {
        let remote = slot(b"macos-x64", DeveloperToolchainState::RemoteOnly);
        let records = build_farm(&[remote], 1);
        assert!(records[0].endpoint.as_bytes().is_empty());
    }

    #[test]
    fn endpoint_lookup_matches_target_only() {
        let mut macos = slot(b"macos-x64", DeveloperToolchainState::RemoteOnly);
        macos.target = DeveloperTarget::MacosX64;
        let mut linux = slot(b"linux-remote", DeveloperToolchainState::RemoteOnly);
        linux.target = DeveloperTarget::LinuxX64;
        let records = build_farm(&[linux, macos], 2);
        let (index, _) = endpoint_for_target(&records, DeveloperTarget::MacosX64).unwrap();
        assert_eq!(index, 1);
        assert!(endpoint_for_target(&records, DeveloperTarget::WindowsX64).is_none());
    }

    #[test]
    fn dispatch_outcome_reports_not_configured() {
        assert!(matches!(
            dispatch_outcome(&[], true),
            DispatchOutcome::NotConfigured
        ));
        assert!(matches!(
            dispatch_outcome(&[], false),
            DispatchOutcome::NotConfigured
        ));
    }

    #[test]
    fn dispatch_outcome_reports_unreachable_without_transport() {
        assert!(matches!(
            dispatch_outcome(b"farm@host", false),
            DispatchOutcome::Unreachable
        ));
    }

    #[test]
    fn dispatch_outcome_registers_with_transport() {
        assert!(matches!(
            dispatch_outcome(b"farm@host", true),
            DispatchOutcome::Registered
        ));
    }

    #[test]
    fn configured_mask_marks_slots_with_endpoints() {
        let mut macos = slot(b"macos-x64", DeveloperToolchainState::RemoteOnly);
        macos.target = DeveloperTarget::MacosX64;
        let _ = macos.remote_endpoint.set(b"farm@host");
        let bare = slot(b"win-r", DeveloperToolchainState::RemoteOnly);
        let installed = slot(b"native", DeveloperToolchainState::Installed);
        let slots = [installed, bare, macos];
        assert_eq!(configured_mask(&slots, 3), 0b100);
    }
}

use rt::PermissionPolicyState;
use serviceos_userspace_runtime as rt;

use crate::state::PendingRuntime;

pub(crate) fn security_policy_count(security_handle: rt::Handle) -> rt::Result<usize> {
    let mut index = 0usize;
    while rt::security_policy_list(security_handle, index)?.is_some() {
        index += 1;
    }
    Ok(index)
}

pub(crate) fn update_policy(
    security_handle: rt::Handle,
    selected_policy_index: usize,
    policy: PermissionPolicyState,
) -> rt::Result<()> {
    if let Some(info) = rt::security_policy_list(security_handle, selected_policy_index)? {
        rt::security_policy_set(security_handle, info.image_id, policy)?;
    }
    Ok(())
}

pub(crate) fn first_actionable_runtime(
    runtime_handle: rt::Handle,
) -> rt::Result<Option<PendingRuntime>> {
    if runtime_handle == rt::INVALID_HANDLE {
        return Ok(None);
    }
    let mut envs = [rt::RuntimeEnvInfo {
        env_id: 0,
        kind: rt::RuntimeKind::Posix,
        state: rt::RuntimeEnvState::Destroyed,
        capabilities: 0,
        mount_count: 0,
        var_count: 0,
        active_runs: 0,
    }; 8];
    let count = rt::runtime_env_list(runtime_handle, &mut envs)?;
    for env in envs.into_iter().take(count) {
        if matches!(
            env.state,
            rt::RuntimeEnvState::PendingApproval | rt::RuntimeEnvState::Denied
        ) {
            return Ok(Some(PendingRuntime {
                env_id: env.env_id,
                state: env.state,
                capabilities: env.capabilities,
            }));
        }
    }
    Ok(None)
}

pub(crate) fn policy_name(policy: PermissionPolicyState) -> &'static str {
    match policy {
        PermissionPolicyState::DefaultAllow => "default-allow",
        PermissionPolicyState::Allowed => "allowed",
        PermissionPolicyState::Blocked => "blocked",
    }
}

pub(crate) fn runtime_env_state_name(state: rt::RuntimeEnvState) -> &'static str {
    match state {
        rt::RuntimeEnvState::Ready => "ready",
        rt::RuntimeEnvState::Busy => "busy",
        rt::RuntimeEnvState::Destroyed => "destroyed",
        rt::RuntimeEnvState::PendingApproval => "pending-approval",
        rt::RuntimeEnvState::Denied => "denied",
    }
}

pub(crate) fn audit_kind_name(kind: rt::SecurityAuditKind) -> &'static str {
    match kind {
        rt::SecurityAuditKind::PolicyChanged => "policy-changed",
        rt::SecurityAuditKind::LaunchDenied => "launch-denied",
        rt::SecurityAuditKind::RuntimeApprovalRequested => "approval-requested",
        rt::SecurityAuditKind::RuntimeApprovalChanged => "approval-changed",
    }
}

pub(crate) fn image_name(image_id: rt::ServiceImageId) -> &'static str {
    match image_id {
        rt::ServiceImageId::SettingsApp => "settings",
        rt::ServiceImageId::FilesApp => "files",
        rt::ServiceImageId::MonitorApp => "monitor",
        rt::ServiceImageId::TerminalApp => "terminal",
        rt::ServiceImageId::SoftwareCenterApp => "software",
        rt::ServiceImageId::SysinfoTool => "sysinfo",
        rt::ServiceImageId::PosixHostTool => "runtime-host",
        rt::ServiceImageId::CrossBuilderTool => "cross-builder",
        _ => "unknown",
    }
}

pub(crate) struct PermissionSummary(pub(crate) u32);

impl core::fmt::Display for PermissionSummary {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut first = true;
        for (name, mask) in [
            ("config", rt::app_permission::CONFIG),
            ("storage", rt::app_permission::STORAGE),
            ("status", rt::app_permission::STATUS),
            ("package", rt::app_permission::PACKAGE),
            ("network", rt::app_permission::NETWORK),
            ("audio", rt::app_permission::AUDIO),
            ("terminal", rt::app_permission::TERMINAL),
            ("clipboard", rt::app_permission::CLIPBOARD),
        ] {
            if self.0 & mask == 0 {
                continue;
            }
            if !first {
                let _ = f.write_str(",");
            }
            first = false;
            let _ = f.write_str(name);
        }
        if first {
            let _ = f.write_str("-");
        }
        Ok(())
    }
}

pub(crate) struct RuntimeCapSummary(pub(crate) u32);

impl core::fmt::Display for RuntimeCapSummary {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut first = true;
        for (name, mask) in [
            ("file-read", rt::runtime_capability::FILE_READ),
            ("terminal-io", rt::runtime_capability::TERMINAL_IO),
            ("network", rt::runtime_capability::NETWORK),
            ("graphics", rt::runtime_capability::GRAPHICS),
            ("audio", rt::runtime_capability::AUDIO),
        ] {
            if self.0 & mask == 0 {
                continue;
            }
            if !first {
                let _ = f.write_str(",");
            }
            first = false;
            let _ = f.write_str(name);
        }
        if first {
            let _ = f.write_str("-");
        }
        Ok(())
    }
}

/// Sensitive capability classes the runtime approval flow gates on — the same
/// set runtime-service's sensitive_capabilities() intersects decisions with.
pub(crate) fn sensitive_runtime_caps(capabilities: u32) -> u32 {
    capabilities
        & (rt::runtime_capability::NETWORK
            | rt::runtime_capability::GRAPHICS
            | rt::runtime_capability::AUDIO)
}

/// Decoded granted-mask marker for a runtime approval audit record:
/// runtime-service packs the granted capability mask into the audit detail
/// word for every RuntimeApprovalChanged record. A partial grant (subset of
/// the sensitive classes) is flagged so history shows granted=subset.
pub(crate) struct RuntimeGrantSummary {
    pub(crate) granted: u32,
    pub(crate) partial: bool,
}

pub(crate) fn runtime_grant_summary(audit: &rt::RuntimeAuditInfo) -> Option<RuntimeGrantSummary> {
    if audit.kind != rt::SecurityAuditKind::RuntimeApprovalChanged {
        return None;
    }
    let granted = audit.detail as u32;
    if granted == 0 {
        return None;
    }
    Some(RuntimeGrantSummary {
        granted,
        partial: granted != sensitive_runtime_caps(audit.capabilities),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approval_audit(capabilities: u32, granted: u32) -> rt::RuntimeAuditInfo {
        rt::RuntimeAuditInfo {
            sequence: 7,
            kind: rt::SecurityAuditKind::RuntimeApprovalChanged,
            env_id: 3,
            capabilities,
            detail: granted as u64,
        }
    }

    #[test]
    fn sensitive_caps_cover_only_gated_classes() {
        let caps = rt::runtime_capability::FILE_READ
            | rt::runtime_capability::TERMINAL_IO
            | rt::runtime_capability::NETWORK
            | rt::runtime_capability::GRAPHICS
            | rt::runtime_capability::AUDIO;
        assert_eq!(
            sensitive_runtime_caps(caps),
            rt::runtime_capability::NETWORK
                | rt::runtime_capability::GRAPHICS
                | rt::runtime_capability::AUDIO
        );
        assert_eq!(sensitive_runtime_caps(rt::runtime_capability::FILE_READ), 0);
    }

    #[test]
    fn grant_summary_flags_subset_as_partial() {
        let caps = rt::runtime_capability::FILE_READ
            | rt::runtime_capability::NETWORK
            | rt::runtime_capability::GRAPHICS
            | rt::runtime_capability::AUDIO;
        let partial = runtime_grant_summary(&approval_audit(caps, rt::runtime_capability::NETWORK))
            .expect("granted record decodes");
        assert_eq!(partial.granted, rt::runtime_capability::NETWORK);
        assert!(partial.partial);

        let full = runtime_grant_summary(&approval_audit(
            caps,
            rt::runtime_capability::NETWORK
                | rt::runtime_capability::GRAPHICS
                | rt::runtime_capability::AUDIO,
        ))
        .expect("full grant decodes");
        assert!(!full.partial);
    }

    #[test]
    fn grant_summary_ignores_non_approval_and_empty_grants() {
        let mut audit = approval_audit(0, 0);
        audit.kind = rt::SecurityAuditKind::LaunchDenied;
        assert!(runtime_grant_summary(&audit).is_none());
        assert!(runtime_grant_summary(&approval_audit(0, 0)).is_none());
    }
}

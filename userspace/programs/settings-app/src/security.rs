use rt::PermissionPolicyState;
use serviceos_userspace_runtime as rt;

use crate::state::PendingRuntime;

// ---- per-env-kind policy defaults (roadmap row 204 residual) ------------

/// Service-local runtime tag pair, mirrored from runtime-service
/// `consts.rs` exactly as the backup module mirrors backup-service tags:
/// get = 0xc1e/0xc1f, set = 0xc20/0xc21 (additive continuation of the
/// shared RuntimeTag range, appended after AuditListReply = 0xc1d).
pub(crate) const ENV_POLICY_GET_REQUEST_TAG: u32 = 0xc1e;
pub(crate) const ENV_POLICY_GET_REPLY_TAG: u32 = 0xc1f;
pub(crate) const ENV_POLICY_SET_REQUEST_TAG: u32 = 0xc20;
pub(crate) const ENV_POLICY_SET_REPLY_TAG: u32 = 0xc21;

/// Additive audit detail marker (bit 31): runtime-service sets it on
/// policy-derived verdicts so history can distinguish them from operator
/// decisions. The AuditListReply packs the detail word as
/// `policy | detail << 32`, so only the detail word's low 32 bits survive
/// the reply — bit 31 (top of that window) is the marker, and the granted
/// mask (bits 0..4) is untouched.
pub(crate) const AUDIT_DETAIL_POLICY_DERIVED: u64 = 1 << 31;

/// Per-env-kind default decision, decoded from the policy contract's wire
/// words (0 = ask, 1 = allow-all, 2 = deny-all).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EnvPolicyDefault {
    Ask,
    AllowAll,
    DenyAll,
}

impl EnvPolicyDefault {
    pub(crate) const fn word(self) -> u64 {
        match self {
            EnvPolicyDefault::Ask => 0,
            EnvPolicyDefault::AllowAll => 1,
            EnvPolicyDefault::DenyAll => 2,
        }
    }

    pub(crate) fn from_word(word: u64) -> Option<Self> {
        match word {
            0 => Some(EnvPolicyDefault::Ask),
            1 => Some(EnvPolicyDefault::AllowAll),
            2 => Some(EnvPolicyDefault::DenyAll),
            _ => None,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            EnvPolicyDefault::Ask => "ask",
            EnvPolicyDefault::AllowAll => "allow-all",
            EnvPolicyDefault::DenyAll => "deny-all",
        }
    }
}

/// The env kinds the policy section lists, in wire order.
pub(crate) const POLICY_KINDS: [rt::RuntimeKind; 2] =
    [rt::RuntimeKind::Posix, rt::RuntimeKind::Windows];

pub(crate) fn policy_kind_name(kind: rt::RuntimeKind) -> &'static str {
    match kind {
        rt::RuntimeKind::Posix => "posix",
        rt::RuntimeKind::Windows => "windows",
    }
}

/// Row label for the policy section (render-vocabulary, pre-uppercased for
/// the no_std surface which has no case-folding helpers).
pub(crate) fn policy_kind_label(kind: rt::RuntimeKind) -> &'static str {
    match kind {
        rt::RuntimeKind::Posix => "POSIX",
        rt::RuntimeKind::Windows => "WINDOWS",
    }
}

/// Honest classification of a failed policy operation: bad reply shape,
/// service-reported Unsupported, or a transport failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PolicyOpError {
    Unsupported,
    Malformed,
    Transport,
}

/// GET request shape: words[0] = kind word (mirrors runtime-service
/// `handle_env_policy_get`).
pub(crate) fn encode_policy_get_request(kind: rt::RuntimeKind) -> rt::RawMessage {
    let mut request = rt::RawMessage::empty(ENV_POLICY_GET_REQUEST_TAG);
    request.word_count = 1;
    request.words[0] = kind as u32 as u64;
    request
}

pub(crate) fn decode_policy_get_reply(
    reply: &rt::RawMessage,
) -> Result<EnvPolicyDefault, PolicyOpError> {
    if reply.tag != ENV_POLICY_GET_REPLY_TAG || reply.word_count < 3 {
        return Err(PolicyOpError::Malformed);
    }
    match reply.words[0] as u32 {
        x if x == rt::RuntimeStatus::Ok as u32 => {}
        x if x == rt::RuntimeStatus::Unsupported as u32 => {
            return Err(PolicyOpError::Unsupported);
        }
        _ => return Err(PolicyOpError::Unsupported),
    }
    EnvPolicyDefault::from_word(reply.words[2]).ok_or(PolicyOpError::Malformed)
}

/// SET request shape: words[0] = kind word, words[1] = default word.
pub(crate) fn encode_policy_set_request(
    kind: rt::RuntimeKind,
    default: EnvPolicyDefault,
) -> rt::RawMessage {
    let mut request = rt::RawMessage::empty(ENV_POLICY_SET_REQUEST_TAG);
    request.word_count = 2;
    request.words[0] = kind as u32 as u64;
    request.words[1] = default.word();
    request
}

pub(crate) fn decode_policy_set_reply(reply: &rt::RawMessage) -> Result<(), PolicyOpError> {
    if reply.tag != ENV_POLICY_SET_REPLY_TAG || reply.word_count < 1 {
        return Err(PolicyOpError::Malformed);
    }
    match reply.words[0] as u32 {
        x if x == rt::RuntimeStatus::Ok as u32 => Ok(()),
        _ => Err(PolicyOpError::Unsupported),
    }
}

/// Reads the current default for `kind` over the runtime channel the
/// security page already holds. Blocking round trip, like every other query
/// the page drives.
pub(crate) fn policy_get(
    runtime_handle: rt::Handle,
    kind: rt::RuntimeKind,
) -> Result<EnvPolicyDefault, PolicyOpError> {
    if runtime_handle == rt::INVALID_HANDLE {
        return Err(PolicyOpError::Transport);
    }
    let mut request = encode_policy_get_request(kind);
    match rt::channel_call(runtime_handle, &mut request) {
        Ok(reply) => decode_policy_get_reply(&reply),
        Err(_) => Err(PolicyOpError::Transport),
    }
}

/// Persists the default for `kind` through the policy set contract.
pub(crate) fn policy_set(
    runtime_handle: rt::Handle,
    kind: rt::RuntimeKind,
    default: EnvPolicyDefault,
) -> Result<(), PolicyOpError> {
    if runtime_handle == rt::INVALID_HANDLE {
        return Err(PolicyOpError::Transport);
    }
    let mut request = encode_policy_set_request(kind, default);
    match rt::channel_call(runtime_handle, &mut request) {
        Ok(reply) => decode_policy_set_reply(&reply),
        Err(_) => Err(PolicyOpError::Transport),
    }
}

/// Three-state selector vocabulary: `<`/`>` cycle ask → allow-all →
/// deny-all → ask, wrapping at the edges.
pub(crate) fn next_policy_default(current: EnvPolicyDefault) -> EnvPolicyDefault {
    match current {
        EnvPolicyDefault::Ask => EnvPolicyDefault::AllowAll,
        EnvPolicyDefault::AllowAll => EnvPolicyDefault::DenyAll,
        EnvPolicyDefault::DenyAll => EnvPolicyDefault::Ask,
    }
}

pub(crate) fn prev_policy_default(current: EnvPolicyDefault) -> EnvPolicyDefault {
    match current {
        EnvPolicyDefault::Ask => EnvPolicyDefault::DenyAll,
        EnvPolicyDefault::AllowAll => EnvPolicyDefault::Ask,
        EnvPolicyDefault::DenyAll => EnvPolicyDefault::AllowAll,
    }
}

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
/// Additive policy tail: detail bit 63 marks a POLICY-DERIVED verdict
/// (per-kind default applied by runtime-service instead of an operator
/// decision); such records surface even when nothing was granted, because
/// an auto-deny is exactly the event an operator must see.
pub(crate) struct RuntimeGrantSummary {
    pub(crate) granted: u32,
    pub(crate) partial: bool,
    pub(crate) policy_derived: bool,
}

pub(crate) fn runtime_grant_summary(audit: &rt::RuntimeAuditInfo) -> Option<RuntimeGrantSummary> {
    if audit.kind != rt::SecurityAuditKind::RuntimeApprovalChanged {
        return None;
    }
    let policy_derived = audit.detail & AUDIT_DETAIL_POLICY_DERIVED != 0;
    let granted = (audit.detail & !AUDIT_DETAIL_POLICY_DERIVED) as u32;
    if granted == 0 && !policy_derived {
        return None;
    }
    Some(RuntimeGrantSummary {
        granted,
        partial: granted != sensitive_runtime_caps(audit.capabilities),
        policy_derived,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approval_audit(capabilities: u32, granted: u64) -> rt::RuntimeAuditInfo {
        rt::RuntimeAuditInfo {
            sequence: 7,
            kind: rt::SecurityAuditKind::RuntimeApprovalChanged,
            env_id: 3,
            capabilities,
            detail: granted,
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
        let partial = runtime_grant_summary(&approval_audit(
            caps,
            rt::runtime_capability::NETWORK as u64,
        ))
        .expect("granted record decodes");
        assert_eq!(partial.granted, rt::runtime_capability::NETWORK);
        assert!(partial.partial);
        assert!(!partial.policy_derived);

        let full = runtime_grant_summary(&approval_audit(
            caps,
            (rt::runtime_capability::NETWORK
                | rt::runtime_capability::GRAPHICS
                | rt::runtime_capability::AUDIO) as u64,
        ))
        .expect("full grant decodes");
        assert!(!full.partial);
        assert!(!full.policy_derived);
    }

    #[test]
    fn grant_summary_ignores_non_approval_and_empty_grants() {
        let mut audit = approval_audit(0, 0);
        audit.kind = rt::SecurityAuditKind::LaunchDenied;
        assert!(runtime_grant_summary(&audit).is_none());
        assert!(runtime_grant_summary(&approval_audit(0, 0)).is_none());
    }

    #[test]
    fn grant_summary_flags_policy_derived_verdicts_including_auto_deny() {
        let caps = rt::runtime_capability::NETWORK | rt::runtime_capability::GRAPHICS;

        // Policy-derived full grant carries the marker next to the mask.
        let auto = runtime_grant_summary(&approval_audit(
            caps,
            rt::runtime_capability::NETWORK as u64 | AUDIT_DETAIL_POLICY_DERIVED,
        ))
        .expect("policy-derived grant decodes");
        assert_eq!(auto.granted, rt::runtime_capability::NETWORK);
        assert!(auto.policy_derived);
        assert!(auto.partial);

        // Policy-derived DENY: granted window empty, marker only — must
        // surface (an auto-deny is exactly what history must disclose).
        let denied = runtime_grant_summary(&approval_audit(caps, AUDIT_DETAIL_POLICY_DERIVED))
            .expect("policy-derived deny decodes");
        assert_eq!(denied.granted, 0);
        assert!(denied.policy_derived);
        assert!(denied.partial);

        // Operator deny (no marker, granted 0) still decodes to None: the
        // pre-policy behavior is unchanged.
        assert!(runtime_grant_summary(&approval_audit(caps, 0)).is_none());
    }

    #[test]
    fn policy_words_roundtrip_and_reject_unknowns() {
        for default in [
            EnvPolicyDefault::Ask,
            EnvPolicyDefault::AllowAll,
            EnvPolicyDefault::DenyAll,
        ] {
            assert_eq!(EnvPolicyDefault::from_word(default.word()), Some(default));
        }
        assert_eq!(EnvPolicyDefault::from_word(3), None);
        assert_eq!(EnvPolicyDefault::from_word(u64::MAX), None);
        assert_eq!(EnvPolicyDefault::Ask.name(), "ask");
        assert_eq!(EnvPolicyDefault::AllowAll.name(), "allow-all");
        assert_eq!(EnvPolicyDefault::DenyAll.name(), "deny-all");
    }

    #[test]
    fn policy_selector_cycles_both_directions() {
        let mut current = EnvPolicyDefault::Ask;
        for expected in [
            EnvPolicyDefault::AllowAll,
            EnvPolicyDefault::DenyAll,
            EnvPolicyDefault::Ask,
        ] {
            current = next_policy_default(current);
            assert_eq!(current, expected);
        }
        for expected in [
            EnvPolicyDefault::DenyAll,
            EnvPolicyDefault::AllowAll,
            EnvPolicyDefault::Ask,
        ] {
            current = prev_policy_default(current);
            assert_eq!(current, expected);
        }
    }

    #[test]
    fn policy_wire_roundtrip_get_and_set() {
        // GET request shape: single kind word.
        let request = encode_policy_get_request(rt::RuntimeKind::Posix);
        assert_eq!(request.tag, ENV_POLICY_GET_REQUEST_TAG);
        assert_eq!(request.word_count, 1);
        assert_eq!(request.words[0], rt::RuntimeKind::Posix as u32 as u64);

        // GET reply decode: [status, kind echo, default word].
        let mut reply = rt::RawMessage::empty(ENV_POLICY_GET_REPLY_TAG);
        reply.word_count = 3;
        reply.words[0] = rt::RuntimeStatus::Ok as u32 as u64;
        reply.words[1] = rt::RuntimeKind::Posix as u32 as u64;
        reply.words[2] = EnvPolicyDefault::DenyAll.word();
        assert_eq!(
            decode_policy_get_reply(&reply),
            Ok(EnvPolicyDefault::DenyAll)
        );

        // SET request shape: kind + default words.
        let request = encode_policy_set_request(rt::RuntimeKind::Windows, EnvPolicyDefault::Ask);
        assert_eq!(request.tag, ENV_POLICY_SET_REQUEST_TAG);
        assert_eq!(request.word_count, 2);
        assert_eq!(request.words[0], rt::RuntimeKind::Windows as u32 as u64);
        assert_eq!(request.words[1], 0);

        // SET reply decode.
        let mut reply = rt::RawMessage::empty(ENV_POLICY_SET_REPLY_TAG);
        reply.word_count = 1;
        reply.words[0] = rt::RuntimeStatus::Ok as u32 as u64;
        assert_eq!(decode_policy_set_reply(&reply), Ok(()));
        reply.words[0] = rt::RuntimeStatus::Unsupported as u32 as u64;
        assert_eq!(
            decode_policy_set_reply(&reply),
            Err(PolicyOpError::Unsupported)
        );
    }

    #[test]
    fn policy_wire_rejects_malformed_and_unsupported_replies() {
        let mut reply = rt::RawMessage::empty(ENV_POLICY_GET_REPLY_TAG);
        reply.word_count = 1;
        assert_eq!(
            decode_policy_get_reply(&reply),
            Err(PolicyOpError::Malformed)
        );
        let mut reply = rt::RawMessage::empty(ENV_POLICY_SET_REPLY_TAG);
        reply.word_count = 1;
        reply.words[0] = rt::RuntimeStatus::Ok as u32 as u64;
        // Foreign tag is malformed even with a well-formed body.
        assert_eq!(
            decode_policy_get_reply(&reply),
            Err(PolicyOpError::Malformed)
        );
        let mut reply = rt::RawMessage::empty(ENV_POLICY_GET_REPLY_TAG);
        reply.word_count = 3;
        reply.words[0] = rt::RuntimeStatus::Unsupported as u32 as u64;
        assert_eq!(
            decode_policy_get_reply(&reply),
            Err(PolicyOpError::Unsupported)
        );
        // Truncated/absent transport never reaches decode; the INVALID
        // handle guard reports Transport without touching the channel.
        assert_eq!(
            policy_get(rt::INVALID_HANDLE, rt::RuntimeKind::Posix),
            Err(PolicyOpError::Transport)
        );
        assert_eq!(
            policy_set(
                rt::INVALID_HANDLE,
                rt::RuntimeKind::Posix,
                EnvPolicyDefault::Ask
            ),
            Err(PolicyOpError::Transport)
        );
    }

    #[test]
    fn policy_kind_vocabulary_matches_wire_order() {
        assert_eq!(POLICY_KINDS.len(), 2);
        assert_eq!(policy_kind_name(POLICY_KINDS[0]), "posix");
        assert_eq!(policy_kind_name(POLICY_KINDS[1]), "windows");
    }
}

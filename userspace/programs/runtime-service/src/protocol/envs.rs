use rt::{
    LogEvent, LogSeverity, PermissionPolicyState, RawMessage, RuntimeStatus, RuntimeTag,
    SecurityAuditKind,
};
use serviceos_userspace_runtime as rt;

use crate::{
    consts::{MAX_AUDIT, MAX_ENVS},
    types::{AuditSlot, EnvSlot, Profile},
    util::{emit_log, instantiate_env, pack_pair, sensitive_capabilities},
};

pub(crate) fn allocate_env(
    envs: &mut [EnvSlot; MAX_ENVS],
    kind_word: u64,
    profile: Profile,
) -> rt::Result<(u32, u32)> {
    let requested_kind = match kind_word as u32 {
        x if x == rt::RuntimeKind::Posix as u32 => rt::RuntimeKind::Posix,
        x if x == rt::RuntimeKind::Windows as u32 => rt::RuntimeKind::Windows,
        _ => return Err(rt::Error::Unsupported),
    };
    if requested_kind != profile.kind {
        return Err(rt::Error::Unsupported);
    }
    let Some(index) = (0..envs.len()).find(|index| !envs[*index].occupied) else {
        return Err(rt::Error::CapacityExceeded);
    };
    envs[index] = instantiate_env(profile);
    let pending_caps = sensitive_capabilities(envs[index].capabilities);
    if pending_caps != 0 {
        envs[index].state = rt::RuntimeEnvState::PendingApproval;
    }
    Ok((index as u32, pending_caps))
}

pub(crate) fn encode_env_status(reply: &mut RawMessage, env_id: u32, env: EnvSlot) {
    reply.words[0] = RuntimeStatus::Ok as u32 as u64;
    reply.words[1] = env_id as u64;
    reply.words[2] = env.kind as u32 as u64;
    reply.words[3] = env.state as u32 as u64;
    reply.words[4] = env.capabilities as u64;
    reply.words[5] = env.mount_count as u64;
    reply.words[6] = env.var_count as u64;
    reply.words[7] = env.active_runs as u64;
    reply.words[8] = env.granted_caps as u64;
    reply.words[9] = pending_capabilities(env.capabilities, env.granted_caps) as u64;
    reply.words[10] = env.sandbox.requested_mask() as u64;
    reply.words[11] = env.sandbox.granted_mask() as u64;
    // Additive word 12: guest syscall-ABI mode of the environment
    // (1 = linux-syscall translation, 0 = native numbering).
    reply.words[12] = env.linux_syscall as u64;
    // Additive word 13: guest syscall table rows the kernel-side mapping
    // carries for this build's architecture (13 on x86_64, 12 on aarch64,
    // 0 = no guest table); mode-independent availability signal.
    reply.words[13] = crate::linux_abi::guest_table_rows_for_build();
    // Additive words 14/15: the per-workload sandbox manifest associated
    // with this environment (S11 manifests). Word 14 doubles as the
    // presence + version marker: 0 = no manifest associated, else
    // `version | manifest_grant_mask << 8`. Word 15 carries the effective
    // profile for this workload — certified class mask in the low byte,
    // effective capabilities (environment capabilities intersected with the
    // manifest's allow-mask, narrow-only) in bits 8..40.
    let effective = crate::sandbox::effective_profile(&env, env.manifest.as_ref());
    reply.words[14] = env.manifest.as_ref().map_or(0, |declared| {
        u64::from(declared.version) | (u64::from(declared.grants_mask()) << 8)
    });
    reply.words[15] =
        u64::from(effective.certified_mask()) | ((effective.capabilities as u64) << 8);
    reply.word_count = 16;
}

pub(crate) fn decision_policy(decision: u64) -> PermissionPolicyState {
    match decision as u32 {
        x if x == PermissionPolicyState::Blocked as u32 => PermissionPolicyState::Blocked,
        x if x == PermissionPolicyState::DefaultAllow as u32 => PermissionPolicyState::DefaultAllow,
        _ => PermissionPolicyState::Allowed,
    }
}

pub(crate) fn pending_capabilities(capabilities: u32, granted_caps: u32) -> u32 {
    sensitive_capabilities(capabilities) & !granted_caps
}

pub(crate) fn apply_decision(
    capabilities: u32,
    granted_caps: u32,
    policy: PermissionPolicyState,
    mask: Option<u32>,
) -> (rt::RuntimeEnvState, u32) {
    let sensitive = sensitive_capabilities(capabilities);
    match policy {
        PermissionPolicyState::Allowed => {
            let grant = match mask {
                Some(requested) => requested & sensitive,
                None => sensitive,
            };
            let granted = granted_caps | grant;
            let state = if pending_capabilities(capabilities, granted) == 0 {
                rt::RuntimeEnvState::Ready
            } else {
                rt::RuntimeEnvState::PendingApproval
            };
            (state, granted)
        }
        PermissionPolicyState::Blocked => (rt::RuntimeEnvState::Denied, 0),
        PermissionPolicyState::DefaultAllow => {
            let state = if sensitive == 0 {
                rt::RuntimeEnvState::Ready
            } else {
                rt::RuntimeEnvState::PendingApproval
            };
            (state, granted_caps)
        }
    }
}

pub(crate) fn handle_env_decision_request(
    log_handle: rt::Handle,
    envs: &mut [EnvSlot; MAX_ENVS],
    audits: &mut [AuditSlot; MAX_AUDIT],
    next_audit_sequence: &mut u32,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.handle_count < 1 || message.word_count < 2 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let env_id = message.words[0] as usize;
    let decision = message.words[1];
    let mask = if message.word_count >= 3 {
        Some(message.words[2] as u32)
    } else {
        None
    };
    let mut reply = RawMessage::empty(RuntimeTag::EnvDecisionReply as u32);
    reply.word_count = 1;
    match envs.get_mut(env_id).filter(|env| env.occupied) {
        Some(env) => {
            let policy = decision_policy(decision);
            let (state, granted) = apply_decision(env.capabilities, env.granted_caps, policy, mask);
            env.granted_caps = granted;
            env.state = state;
            env.sandbox.apply_granted_mask(granted);
            record_audit(
                audits,
                next_audit_sequence,
                SecurityAuditKind::RuntimeApprovalChanged,
                env_id as u32,
                env.capabilities,
                policy,
                granted as u64,
            );
            let _ = emit_log(
                log_handle,
                LogSeverity::Info,
                LogEvent::RuntimeApprovalChanged,
                env_id as u64,
                (state as u32 as u64) | ((granted as u64) << 32),
            );
            reply.words[0] = RuntimeStatus::Ok as u32 as u64;
        }
        None => reply.words[0] = RuntimeStatus::NotFound as u32 as u64,
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn audit_kind_from_word(value: u64) -> Option<SecurityAuditKind> {
    match value as u32 {
        x if x == SecurityAuditKind::PolicyChanged as u32 => Some(SecurityAuditKind::PolicyChanged),
        x if x == SecurityAuditKind::LaunchDenied as u32 => Some(SecurityAuditKind::LaunchDenied),
        x if x == SecurityAuditKind::RuntimeApprovalRequested as u32 => {
            Some(SecurityAuditKind::RuntimeApprovalRequested)
        }
        x if x == SecurityAuditKind::RuntimeApprovalChanged as u32 => {
            Some(SecurityAuditKind::RuntimeApprovalChanged)
        }
        _ => None,
    }
}

fn audit_entry_matches(kind: SecurityAuditKind, filter: Option<u64>) -> bool {
    match filter {
        None => true,
        Some(word) => audit_kind_from_word(word).is_some_and(|expected| expected == kind),
    }
}

pub(crate) fn select_audit(
    audits: &[AuditSlot; MAX_AUDIT],
    index: usize,
    filter: Option<u64>,
) -> Option<AuditSlot> {
    audits
        .iter()
        .filter(|entry| entry.occupied && audit_entry_matches(entry.kind, filter))
        .nth(index)
        .copied()
}

pub(crate) fn handle_audit_list_request(
    audits: &[AuditSlot; MAX_AUDIT],
    message: &RawMessage,
) -> rt::Result<()> {
    if message.handle_count < 1 || message.word_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let index = message.words[0] as usize;
    let filter = if message.word_count >= 2 {
        Some(message.words[1])
    } else {
        None
    };
    let mut reply = RawMessage::empty(RuntimeTag::AuditListReply as u32);
    reply.word_count = 6;
    if let Some(entry) = select_audit(audits, index, filter) {
        reply.words[0] = RuntimeStatus::Ok as u32 as u64;
        reply.words[1] = entry.sequence as u64;
        reply.words[2] = entry.kind as u32 as u64;
        reply.words[3] = entry.env_id as u64;
        reply.words[4] = entry.capabilities as u64;
        reply.words[5] = entry.policy as u32 as u64 | (entry.detail << 32);
    } else {
        reply.words[0] = RuntimeStatus::NotFound as u32 as u64;
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn env_state_from_word(value: u64) -> Option<rt::RuntimeEnvState> {
    match value as u32 {
        x if x == rt::RuntimeEnvState::Ready as u32 => Some(rt::RuntimeEnvState::Ready),
        x if x == rt::RuntimeEnvState::Busy as u32 => Some(rt::RuntimeEnvState::Busy),
        x if x == rt::RuntimeEnvState::Destroyed as u32 => Some(rt::RuntimeEnvState::Destroyed),
        x if x == rt::RuntimeEnvState::PendingApproval as u32 => {
            Some(rt::RuntimeEnvState::PendingApproval)
        }
        x if x == rt::RuntimeEnvState::Denied as u32 => Some(rt::RuntimeEnvState::Denied),
        _ => None,
    }
}

pub(crate) fn env_matches_state(state: rt::RuntimeEnvState, filter: Option<u64>) -> bool {
    match filter {
        None => true,
        Some(word) => env_state_from_word(word).is_some_and(|expected| expected == state),
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) fn handle_env_list_request(
    envs: &[EnvSlot; MAX_ENVS],
    message: &RawMessage,
) -> rt::Result<()> {
    if message.handle_count < 1 {
        return Ok(());
    }
    let start = if message.word_count > 0 {
        message.words[0] as usize
    } else {
        0
    };
    let filter = if message.word_count >= 2 {
        Some(message.words[1])
    } else {
        None
    };
    let reply_handle = message.handles[0];
    let mut reply = RawMessage::empty(RuntimeTag::EnvListReply as u32);
    reply.word_count = 3;
    reply.words[0] = RuntimeStatus::Ok as u32 as u64;
    reply.words[1] = 0;
    reply.words[2] = usize::MAX as u64;
    let mut emitted = 0usize;
    let mut visible = 0usize;
    for (index, env) in envs.iter().enumerate() {
        if !env.occupied || !env_matches_state(env.state, filter) {
            continue;
        }
        if visible < start {
            visible += 1;
            continue;
        }
        if reply.word_count as usize + 6 > rt::IPC_MAX_WORDS {
            reply.words[2] = visible as u64;
            break;
        }
        let base = reply.word_count as usize;
        reply.words[base] = index as u64;
        reply.words[base + 1] = env.kind as u32 as u64;
        reply.words[base + 2] = env.state as u32 as u64;
        reply.words[base + 3] = env.capabilities as u64;
        reply.words[base + 4] = env.mount_count as u64;
        reply.words[base + 5] = env.active_runs as u64;
        reply.word_count += 6;
        emitted += 1;
        visible += 1;
    }
    reply.words[1] = emitted as u64;
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

pub(crate) fn record_audit(
    audits: &mut [AuditSlot; MAX_AUDIT],
    next_audit_sequence: &mut u32,
    kind: SecurityAuditKind,
    env_id: u32,
    capabilities: u32,
    policy: PermissionPolicyState,
    detail: u64,
) {
    let index = audits.iter().position(|entry| !entry.occupied).unwrap_or(0);
    audits[index] = AuditSlot {
        occupied: true,
        sequence: *next_audit_sequence,
        kind,
        env_id,
        capabilities,
        detail,
        policy,
    };
    *next_audit_sequence = next_audit_sequence.saturating_add(1);
}

pub(crate) fn handle_env_mount_request(
    envs: &[EnvSlot; MAX_ENVS],
    message: &RawMessage,
) -> rt::Result<()> {
    if message.handle_count < 1 || message.word_count < 2 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let env_id = message.words[0] as usize;
    let index = message.words[1] as usize;
    let mut reply = RawMessage::empty(RuntimeTag::EnvMountListReply as u32);
    reply.word_count = 3;
    match envs.get(env_id).filter(|env| env.occupied) {
        Some(env) if index < env.mount_count => {
            reply.words[0] = RuntimeStatus::Ok as u32 as u64;
            reply.words[1] = env.mounts[index].guest.len as u64;
            reply.words[2] = env.mounts[index].source.len as u64;
            reply.word_count += pack_pair(
                env.mounts[index].guest.as_bytes(),
                env.mounts[index].source.as_bytes(),
                &mut reply.words[3..],
            )?;
        }
        _ => {
            reply.words[0] = RuntimeStatus::NotFound as u32 as u64;
        }
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

pub(crate) fn handle_env_var_request(
    envs: &[EnvSlot; MAX_ENVS],
    message: &RawMessage,
) -> rt::Result<()> {
    if message.handle_count < 1 || message.word_count < 2 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let env_id = message.words[0] as usize;
    let index = message.words[1] as usize;
    let mut reply = RawMessage::empty(RuntimeTag::EnvVarListReply as u32);
    reply.word_count = 3;
    match envs.get(env_id).filter(|env| env.occupied) {
        Some(env) if index < env.var_count => {
            reply.words[0] = RuntimeStatus::Ok as u32 as u64;
            reply.words[1] = env.vars[index].key.len as u64;
            reply.words[2] = env.vars[index].value.len as u64;
            reply.word_count += pack_pair(
                env.vars[index].key.as_bytes(),
                env.vars[index].value.as_bytes(),
                &mut reply.words[3..],
            )?;
        }
        _ => {
            reply.words[0] = RuntimeStatus::NotFound as u32 as u64;
        }
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENSITIVE: u32 = rt::runtime_capability::NETWORK
        | rt::runtime_capability::GRAPHICS
        | rt::runtime_capability::AUDIO;

    fn sensitive_env() -> EnvSlot {
        let mut env = EnvSlot::empty();
        env.occupied = true;
        env.capabilities = rt::runtime_capability::FILE_READ | SENSITIVE;
        env.state = rt::RuntimeEnvState::PendingApproval;
        env
    }

    #[test]
    fn decision_policy_maps_enum_discriminants() {
        assert!(matches!(
            decision_policy(PermissionPolicyState::Allowed as u64),
            PermissionPolicyState::Allowed
        ));
        assert!(matches!(
            decision_policy(PermissionPolicyState::Blocked as u64),
            PermissionPolicyState::Blocked
        ));
        assert!(matches!(
            decision_policy(PermissionPolicyState::DefaultAllow as u64),
            PermissionPolicyState::DefaultAllow
        ));
        assert!(matches!(
            decision_policy(u64::MAX),
            PermissionPolicyState::Allowed
        ));
    }

    #[test]
    fn env_status_surfaces_linux_syscall_mode_in_additive_word() {
        // Native env: word 12 = 0, word count 16 (14 before the S11
        // manifest words; the trailing words are additive-only).
        let mut native = EnvSlot::empty();
        native.occupied = true;
        let mut reply = RawMessage::empty(0);
        encode_env_status(&mut reply, 3, native);
        assert_eq!(reply.word_count, 16);
        assert_eq!(reply.words[12], 0);

        // linux-syscall env: word 12 = 1; word 13 is the same
        // mode-independent per-arch table count in both replies.
        native.linux_syscall = true;
        let mut flagged = RawMessage::empty(0);
        encode_env_status(&mut flagged, 3, native);
        assert_eq!(flagged.word_count, 16);
        assert_eq!(flagged.words[12], 1);
        assert_eq!(flagged.words[13], reply.words[13]);

        // Word 13 reports this build's guest table (host arch decides).
        #[cfg(target_arch = "x86_64")]
        assert_eq!(flagged.words[13], 13);
        #[cfg(target_arch = "aarch64")]
        assert_eq!(flagged.words[13], 12);
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        assert_eq!(flagged.words[13], 0);

        // Words 0..13 are unchanged from the legacy+mode layout.
        let mut legacy = EnvSlot::empty();
        legacy.occupied = true;
        let mut untouched = RawMessage::empty(0);
        encode_env_status(&mut untouched, 3, legacy);
        assert_eq!(untouched.words[0..13], reply.words[0..13]);
    }

    #[test]
    fn env_status_surfaces_manifest_words_additively() {
        use crate::sandbox::{DeviceClass, SANDBOX_MANIFEST_VERSION, SandboxManifest};

        // No manifest: presence word 0; effective word carries the plain
        // environment profile (certified mask low byte, caps above).
        let mut plain = EnvSlot::empty();
        plain.occupied = true;
        plain.capabilities = rt::runtime_capability::FILE_READ | rt::runtime_capability::NETWORK;
        plain.sandbox = crate::sandbox::SandboxProfile::from_masks(plain.capabilities, 0);
        let mut reply = RawMessage::empty(0);
        encode_env_status(&mut reply, 0, plain);
        assert_eq!(reply.words[14], 0);
        assert_eq!(reply.words[15], (plain.capabilities as u64) << 8);

        // Manifest associated: version marker + manifest grant mask, and
        // the effective word is the narrow-only intersection.
        plain.state = rt::RuntimeEnvState::Ready;
        plain.sandbox.grant(DeviceClass::Network);
        let manifest = SandboxManifest {
            version: SANDBOX_MANIFEST_VERSION,
            grants: [true, false, false, false],
            caps_allow: Some(rt::runtime_capability::FILE_READ),
        };
        plain.manifest = Some(manifest);
        let mut flagged = RawMessage::empty(0);
        encode_env_status(&mut flagged, 0, plain);
        assert_eq!(
            flagged.words[14],
            u64::from(SANDBOX_MANIFEST_VERSION) | (u64::from(manifest.grants_mask()) << 8)
        );
        assert_eq!(
            flagged.words[15],
            (1u32 << DeviceClass::Network.index()) as u64
                | ((rt::runtime_capability::FILE_READ as u64) << 8)
        );
    }

    #[test]
    fn approve_subset_grants_only_requested_sensitive_bits() {
        let (state, granted) = apply_decision(
            sensitive_env().capabilities,
            0,
            PermissionPolicyState::Allowed,
            Some(rt::runtime_capability::NETWORK),
        );
        assert!(matches!(state, rt::RuntimeEnvState::PendingApproval));
        assert_eq!(granted, rt::runtime_capability::NETWORK);
        assert_eq!(
            pending_capabilities(sensitive_env().capabilities, granted),
            rt::runtime_capability::GRAPHICS | rt::runtime_capability::AUDIO
        );
    }

    #[test]
    fn approve_subset_ignores_non_sensitive_mask_bits() {
        let requested = u32::MAX;
        let (_, granted) = apply_decision(
            sensitive_env().capabilities,
            0,
            PermissionPolicyState::Allowed,
            Some(requested),
        );
        assert_eq!(granted, SENSITIVE);
    }

    #[test]
    fn repeated_subset_approves_reach_ready() {
        let capabilities = sensitive_env().capabilities;
        let (_, first) = apply_decision(
            capabilities,
            0,
            PermissionPolicyState::Allowed,
            Some(rt::runtime_capability::NETWORK | rt::runtime_capability::TERMINAL_IO),
        );
        let (state, second) = apply_decision(
            capabilities,
            first,
            PermissionPolicyState::Allowed,
            Some(rt::runtime_capability::GRAPHICS | rt::runtime_capability::AUDIO),
        );
        assert!(matches!(state, rt::RuntimeEnvState::Ready));
        assert_eq!(second, SENSITIVE);
        assert_eq!(pending_capabilities(capabilities, second), 0);
    }

    #[test]
    fn approve_without_mask_grants_everything() {
        let (state, granted) = apply_decision(
            sensitive_env().capabilities,
            0,
            PermissionPolicyState::Allowed,
            None,
        );
        assert!(matches!(state, rt::RuntimeEnvState::Ready));
        assert_eq!(granted, SENSITIVE);
    }

    #[test]
    fn deny_revokes_all_grants() {
        let (_, partial) = apply_decision(
            sensitive_env().capabilities,
            0,
            PermissionPolicyState::Allowed,
            Some(rt::runtime_capability::NETWORK),
        );
        let (state, granted) = apply_decision(
            sensitive_env().capabilities,
            partial,
            PermissionPolicyState::Blocked,
            None,
        );
        assert!(matches!(state, rt::RuntimeEnvState::Denied));
        assert_eq!(granted, 0);
    }

    #[test]
    fn default_allow_keeps_existing_grants_pending() {
        let (_, partial) = apply_decision(
            sensitive_env().capabilities,
            0,
            PermissionPolicyState::Allowed,
            Some(rt::runtime_capability::NETWORK),
        );
        let (state, granted) = apply_decision(
            sensitive_env().capabilities,
            partial,
            PermissionPolicyState::DefaultAllow,
            None,
        );
        assert!(matches!(state, rt::RuntimeEnvState::PendingApproval));
        assert_eq!(granted, rt::runtime_capability::NETWORK);
    }

    #[test]
    fn default_allow_readies_when_nothing_sensitive() {
        let (state, granted) = apply_decision(
            rt::runtime_capability::FILE_READ,
            0,
            PermissionPolicyState::DefaultAllow,
            None,
        );
        assert!(matches!(state, rt::RuntimeEnvState::Ready));
        assert_eq!(granted, 0);
    }

    #[test]
    fn audit_roundtrip_preserves_entries() {
        let mut audits = [AuditSlot::empty(); MAX_AUDIT];
        let mut next = 1u32;
        record_audit(
            &mut audits,
            &mut next,
            SecurityAuditKind::RuntimeApprovalRequested,
            1,
            SENSITIVE,
            PermissionPolicyState::DefaultAllow,
            SENSITIVE as u64,
        );
        record_audit(
            &mut audits,
            &mut next,
            SecurityAuditKind::RuntimeApprovalChanged,
            1,
            SENSITIVE,
            PermissionPolicyState::Allowed,
            rt::runtime_capability::NETWORK as u64,
        );
        let first = select_audit(&audits, 0, None).expect("first entry");
        assert!(matches!(
            first.kind,
            SecurityAuditKind::RuntimeApprovalRequested
        ));
        assert_eq!(first.sequence, 1);
        assert_eq!(first.detail, SENSITIVE as u64);
        let second = select_audit(&audits, 1, None).expect("second entry");
        assert!(matches!(
            second.kind,
            SecurityAuditKind::RuntimeApprovalChanged
        ));
        assert_eq!(second.sequence, 2);
        assert_eq!(second.detail, rt::runtime_capability::NETWORK as u64);
        assert_eq!(next, 3);
    }

    #[test]
    fn audit_kind_filter_skips_non_matching_entries() {
        let mut audits = [AuditSlot::empty(); MAX_AUDIT];
        let mut next = 1u32;
        record_audit(
            &mut audits,
            &mut next,
            SecurityAuditKind::RuntimeApprovalRequested,
            0,
            0,
            PermissionPolicyState::DefaultAllow,
            0,
        );
        record_audit(
            &mut audits,
            &mut next,
            SecurityAuditKind::RuntimeApprovalChanged,
            0,
            0,
            PermissionPolicyState::Allowed,
            7,
        );
        record_audit(
            &mut audits,
            &mut next,
            SecurityAuditKind::RuntimeApprovalChanged,
            1,
            0,
            PermissionPolicyState::Blocked,
            0,
        );
        let changed_word = SecurityAuditKind::RuntimeApprovalChanged as u32 as u64;
        let filtered = select_audit(&audits, 0, Some(changed_word)).expect("filtered entry");
        assert_eq!(filtered.sequence, 2);
        assert_eq!(filtered.detail, 7);
        let later = select_audit(&audits, 1, Some(changed_word)).expect("second filtered");
        assert_eq!(later.sequence, 3);
        assert!(select_audit(&audits, 2, Some(changed_word)).is_none());
        assert!(select_audit(&audits, 0, Some(9999)).is_none());
    }

    #[test]
    fn audit_wraparound_overwrites_oldest_slot() {
        let mut audits = [AuditSlot::empty(); MAX_AUDIT];
        let mut next = 1u32;
        for _ in 0..=MAX_AUDIT {
            record_audit(
                &mut audits,
                &mut next,
                SecurityAuditKind::RuntimeApprovalRequested,
                0,
                0,
                PermissionPolicyState::DefaultAllow,
                0,
            );
        }
        assert!(audits.iter().all(|entry| entry.occupied));
        assert_eq!(audits[0].sequence, MAX_AUDIT as u32 + 1);
        assert_eq!(next, MAX_AUDIT as u32 + 2);
    }

    #[test]
    fn pending_queue_state_filter_matches() {
        let pending_word = rt::RuntimeEnvState::PendingApproval as u32 as u64;
        assert!(env_matches_state(
            rt::RuntimeEnvState::PendingApproval,
            Some(pending_word)
        ));
        assert!(!env_matches_state(
            rt::RuntimeEnvState::Ready,
            Some(pending_word)
        ));
        assert!(!env_matches_state(
            rt::RuntimeEnvState::PendingApproval,
            Some(9999)
        ));
        assert!(env_matches_state(rt::RuntimeEnvState::Denied, None));
    }
}

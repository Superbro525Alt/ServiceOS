use serviceos_userspace_runtime as rt;
use rt::{
    LogEvent, LogSeverity, PermissionPolicyState, RawMessage, RuntimeStatus, RuntimeTag,
    SecurityAuditKind,
};

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
    let mut reply = RawMessage::empty(RuntimeTag::EnvDecisionReply as u32);
    reply.word_count = 1;
    match envs.get_mut(env_id).filter(|env| env.occupied) {
        Some(env) => {
            let policy = match decision as u32 {
                2 => PermissionPolicyState::Blocked,
                3 => PermissionPolicyState::DefaultAllow,
                _ => PermissionPolicyState::Allowed,
            };
            env.state = match policy {
                PermissionPolicyState::Allowed => rt::RuntimeEnvState::Ready,
                PermissionPolicyState::Blocked => rt::RuntimeEnvState::Denied,
                PermissionPolicyState::DefaultAllow => {
                    if sensitive_capabilities(env.capabilities) != 0 {
                        rt::RuntimeEnvState::PendingApproval
                    } else {
                        rt::RuntimeEnvState::Ready
                    }
                }
            };
            record_audit(
                audits,
                next_audit_sequence,
                SecurityAuditKind::RuntimeApprovalChanged,
                env_id as u32,
                env.capabilities,
                policy,
                decision,
            );
            let _ = emit_log(
                log_handle,
                LogSeverity::Info,
                LogEvent::RuntimeApprovalChanged,
                env_id as u64,
                (env.state as u32 as u64) | ((env.capabilities as u64) << 32),
            );
            reply.words[0] = RuntimeStatus::Ok as u32 as u64;
        }
        None => reply.words[0] = RuntimeStatus::NotFound as u32 as u64,
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
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
    let mut reply = RawMessage::empty(RuntimeTag::AuditListReply as u32);
    reply.word_count = 6;
    if let Some(entry) = audits.iter().filter(|entry| entry.occupied).nth(index).copied() {
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

use rt::{
    LogEvent, LogSeverity, PermissionPolicyState, RawMessage, RuntimeStatus, RuntimeTag,
    SecurityAuditKind,
};
use serviceos_userspace_runtime as rt;

use crate::{
    consts::{MAX_AUDIT, MAX_ENVS, MAX_RUNS},
    types::{AuditSlot, EnvSlot, Profile, RunSlot},
    util::emit_log,
};

pub(crate) mod envs;
mod runs;
mod sessions;

#[cfg(test)]
pub(crate) use self::envs::apply_decision;
use self::envs::{
    allocate_env, encode_env_status, handle_audit_list_request, handle_env_decision_request,
    handle_env_list_request, handle_env_mount_request, handle_env_var_request, record_audit,
};
use self::runs::handle_run_launch_request;
pub(crate) use self::{runs::poll_run_exits, sessions::handle_run_session_request};

use crate::envstore;

/// Tags whose handling may mutate the durable parts of an env record
/// (create / destroy / approval decision / launch-time manifest latch).
/// Read-only list and status tags are excluded from the write-through
/// snapshot entirely.
fn is_durable_mutation(tag: u32) -> bool {
    tag == RuntimeTag::EnvCreateRequest as u32
        || tag == RuntimeTag::EnvDestroyRequest as u32
        || tag == RuntimeTag::RunLaunchRequest as u32
        || tag == RuntimeTag::EnvDecisionRequest as u32
}

pub(crate) fn handle_public_request(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    log_handle: rt::Handle,
    profile: Profile,
    envstore_writable: bool,
    envs: &mut [EnvSlot; MAX_ENVS],
    runs: &mut [RunSlot; MAX_RUNS],
    audits: &mut [AuditSlot; MAX_AUDIT],
    next_audit_sequence: &mut u32,
    message: &RawMessage,
) -> rt::Result<()> {
    // Write-through baseline: mutating tags snapshot the table first; any
    // difference after dispatch rewrites the store in full. Read-only tags
    // never persist.
    let durable_before = if is_durable_mutation(message.tag) {
        Some(*envs)
    } else {
        None
    };
    match message.tag {
        x if x == RuntimeTag::EnvCreateRequest as u32 => {
            if message.handle_count < 1 || message.word_count < 1 {
                return Ok(());
            }
            let reply_handle = message.handles[0];
            let mut reply = RawMessage::empty(RuntimeTag::EnvCreateReply as u32);
            reply.word_count = 2;
            match allocate_env(envs, message.words[0], profile) {
                Ok((env_id, pending_caps)) => {
                    reply.words[0] = RuntimeStatus::Ok as u32 as u64;
                    reply.words[1] = env_id as u64;
                    if pending_caps != 0 {
                        record_audit(
                            audits,
                            next_audit_sequence,
                            SecurityAuditKind::RuntimeApprovalRequested,
                            env_id,
                            pending_caps,
                            PermissionPolicyState::DefaultAllow,
                            message.words[0],
                        );
                        let _ = emit_log(
                            log_handle,
                            LogSeverity::Warn,
                            LogEvent::RuntimeApprovalPending,
                            env_id as u64,
                            pending_caps as u64,
                        );
                    } else {
                        let _ = emit_log(
                            log_handle,
                            LogSeverity::Info,
                            LogEvent::RuntimeEnvironmentCreated,
                            env_id as u64,
                            message.words[0],
                        );
                    }
                }
                Err(error) => {
                    reply.words[0] = match error {
                        rt::Error::Unsupported => RuntimeStatus::Unsupported as u32 as u64,
                        rt::Error::CapacityExceeded => RuntimeStatus::Busy as u32 as u64,
                        _ => RuntimeStatus::Busy as u32 as u64,
                    };
                }
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == RuntimeTag::EnvDestroyRequest as u32 => {
            if message.handle_count < 1 || message.word_count < 1 {
                return Ok(());
            }
            let reply_handle = message.handles[0];
            let env_id = message.words[0] as usize;
            let mut reply = RawMessage::empty(RuntimeTag::EnvDestroyReply as u32);
            reply.word_count = 1;
            if let Some(env) = envs.get_mut(env_id).filter(|env| env.occupied) {
                if env.active_runs != 0 {
                    reply.words[0] = RuntimeStatus::Busy as u32 as u64;
                } else {
                    *env = EnvSlot::empty();
                    reply.words[0] = RuntimeStatus::Ok as u32 as u64;
                    let _ = emit_log(
                        log_handle,
                        LogSeverity::Info,
                        LogEvent::RuntimeEnvironmentDestroyed,
                        env_id as u64,
                        0,
                    );
                }
            } else {
                reply.words[0] = RuntimeStatus::NotFound as u32 as u64;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == RuntimeTag::EnvListRequest as u32 => {
            handle_env_list_request(envs, message)?;
        }
        x if x == RuntimeTag::EnvStatusRequest as u32 => {
            if message.handle_count < 1 || message.word_count < 1 {
                return Ok(());
            }
            let reply_handle = message.handles[0];
            let env_id = message.words[0] as usize;
            let mut reply = RawMessage::empty(RuntimeTag::EnvStatusReply as u32);
            reply.word_count = 8;
            if let Some(env) = envs.get(env_id).filter(|env| env.occupied) {
                encode_env_status(&mut reply, env_id as u32, *env);
            } else {
                reply.words[0] = RuntimeStatus::NotFound as u32 as u64;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == RuntimeTag::EnvMountListRequest as u32 => {
            handle_env_mount_request(envs, message)?;
        }
        x if x == RuntimeTag::EnvVarListRequest as u32 => {
            handle_env_var_request(envs, message)?;
        }
        x if x == RuntimeTag::RunLaunchRequest as u32 => {
            handle_run_launch_request(bootstrap, storage_handle, log_handle, envs, runs, message)?;
        }
        x if x == RuntimeTag::EnvDecisionRequest as u32 => {
            handle_env_decision_request(log_handle, envs, audits, next_audit_sequence, message)?;
        }
        x if x == RuntimeTag::AuditListRequest as u32 => {
            handle_audit_list_request(audits, message)?;
        }
        x if x == RuntimeTag::RunListRequest as u32 => {
            if message.handle_count < 1 {
                return Ok(());
            }
            let start = if message.word_count > 0 {
                message.words[0] as usize
            } else {
                0
            };
            let reply_handle = message.handles[0];
            let mut reply = RawMessage::empty(RuntimeTag::RunListReply as u32);
            reply.word_count = 3;
            reply.words[0] = RuntimeStatus::Ok as u32 as u64;
            reply.words[1] = 0;
            reply.words[2] = usize::MAX as u64;
            let mut emitted = 0usize;
            let mut visible = 0usize;
            for (index, run) in runs.iter().enumerate() {
                if !run.occupied {
                    continue;
                }
                if visible < start {
                    visible += 1;
                    continue;
                }
                if reply.word_count as usize + 5 > rt::IPC_MAX_WORDS {
                    reply.words[2] = visible as u64;
                    break;
                }
                let base = reply.word_count as usize;
                reply.words[base] = index as u64;
                reply.words[base + 1] = run.env_id as u64;
                reply.words[base + 2] = run.workload_word();
                reply.words[base + 3] = run.state as u32 as u64;
                reply.words[base + 4] = run.exit_code;
                reply.word_count += 5;
                emitted += 1;
                visible += 1;
            }
            reply.words[1] = emitted as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == RuntimeTag::RunStatusRequest as u32 => {
            if message.handle_count < 1 || message.word_count < 1 {
                return Ok(());
            }
            let reply_handle = message.handles[0];
            let run_id = message.words[0] as usize;
            let mut reply = RawMessage::empty(RuntimeTag::RunStatusReply as u32);
            reply.word_count = 6;
            if let Some(run) = runs.get(run_id).filter(|run| run.occupied) {
                reply.words[0] = RuntimeStatus::Ok as u32 as u64;
                reply.words[1] = run_id as u64;
                reply.words[2] = run.env_id as u64;
                reply.words[3] = run.workload_word();
                reply.words[4] = run.state as u32 as u64;
                reply.words[5] = run.exit_code;
            } else {
                reply.words[0] = RuntimeStatus::NotFound as u32 as u64;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        _ => {}
    }

    poll_run_exits(log_handle, envs, runs);
    if let Some(before) = durable_before
        && *envs != before
        && envstore_writable
    {
        envstore::persist_envs(storage_handle, envs);
    }
    Ok(())
}

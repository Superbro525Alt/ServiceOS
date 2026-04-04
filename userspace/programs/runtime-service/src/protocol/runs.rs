use serviceos_userspace_runtime as rt;
use rt::{LogEvent, LogSeverity, RawMessage, RuntimeRunState, RuntimeStatus, RuntimeTag, RuntimeWorkloadKind, TaskStateCode};

use crate::{
    consts::{MAX_ENVS, MAX_RUNS},
    types::{EnvSlot, RunSlot},
    util::{emit_log, release_run_slot},
};

pub(crate) fn poll_run_exits(
    log_handle: rt::Handle,
    envs: &mut [EnvSlot; MAX_ENVS],
    runs: &mut [RunSlot; MAX_RUNS],
) {
    for (run_id, run) in runs.iter_mut().enumerate() {
        if !run.occupied || run.state == RuntimeRunState::Exited || run.state == RuntimeRunState::Failed {
            continue;
        }
        let Ok(status) = rt::task_status(run.task_handle) else {
            continue;
        };
        if !matches!(status.state, TaskStateCode::Exited | TaskStateCode::Faulted) {
            continue;
        }
        run.exit_code = status.exit_code;
        run.state = if status.exit_code == 0 {
            RuntimeRunState::Exited
        } else {
            RuntimeRunState::Failed
        };
        if let Some(env) = envs.get_mut(run.env_id as usize)
            && env.occupied
            && env.active_runs > 0
        {
            env.active_runs -= 1;
        }
        if run.session_handle != rt::INVALID_HANDLE {
            let _ = rt::handle_close(run.session_handle);
            run.session_handle = rt::INVALID_HANDLE;
        }
        let _ = emit_log(
            log_handle,
            LogSeverity::Info,
            LogEvent::RuntimeLaunchExited,
            run_id as u64,
            status.exit_code,
        );
    }
}

fn allocate_run_slot(runs: &mut [RunSlot; MAX_RUNS]) -> rt::Result<usize> {
    if let Some(index) = (0..runs.len()).find(|index| {
        !runs[*index].occupied
            || matches!(runs[*index].state, RuntimeRunState::Exited | RuntimeRunState::Failed)
    }) {
        if runs[index].occupied {
            release_run_slot(&mut runs[index]);
        }
        return Ok(index);
    }
    Err(rt::Error::CapacityExceeded)
}

pub(crate) fn handle_run_launch_request(
    bootstrap: rt::Handle,
    log_handle: rt::Handle,
    envs: &mut [EnvSlot; MAX_ENVS],
    runs: &mut [RunSlot; MAX_RUNS],
    message: &RawMessage,
) -> rt::Result<()> {
    if message.handle_count < 2 || message.word_count < 3 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let output_handle = message.handles[1];
    let env_id = message.words[0] as usize;
    let workload = match message.words[1] as u32 {
        x if x == RuntimeWorkloadKind::Inspect as u32 => RuntimeWorkloadKind::Inspect,
        x if x == RuntimeWorkloadKind::Env as u32 => RuntimeWorkloadKind::Env,
        x if x == RuntimeWorkloadKind::Mounts as u32 => RuntimeWorkloadKind::Mounts,
        x if x == RuntimeWorkloadKind::Cat as u32 => RuntimeWorkloadKind::Cat,
        _ => RuntimeWorkloadKind::Inspect,
    };
    let arg_len = message.words[2] as usize;
    let mut arg_bytes = [0u8; (rt::IPC_MAX_WORDS - 3) * 8];
    let _ = rt::unpack_bytes(
        &message.words[3..message.word_count as usize],
        arg_len,
        &mut arg_bytes,
    );

    let mut reply = RawMessage::empty(RuntimeTag::RunLaunchReply as u32);
    reply.word_count = 2;

    let Some(env) = envs.get_mut(env_id).filter(|env| env.occupied) else {
        reply.words[0] = RuntimeStatus::NotFound as u32 as u64;
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(reply_handle);
        let _ = rt::handle_close(output_handle);
        return Ok(());
    };
    if env.state == rt::RuntimeEnvState::PendingApproval {
        reply.words[0] = RuntimeStatus::PendingApproval as u32 as u64;
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(reply_handle);
        let _ = rt::handle_close(output_handle);
        return Ok(());
    }
    if env.state == rt::RuntimeEnvState::Denied {
        reply.words[0] = RuntimeStatus::Denied as u32 as u64;
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(reply_handle);
        let _ = rt::handle_close(output_handle);
        return Ok(());
    }

    let slot_index = match allocate_run_slot(runs) {
        Ok(index) => index,
        Err(_) => {
            reply.words[0] = RuntimeStatus::Busy as u32 as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
            let _ = rt::handle_close(output_handle);
            return Ok(());
        }
    };

    let pair = rt::channel_create()?;
    let startup_handles = [
        rt::StartupHandle {
            handle: output_handle,
            rights: rt::rights::SEND | rt::rights::DUPLICATE | rt::rights::TRANSFER,
        },
        rt::StartupHandle {
            handle: pair.second,
            rights: rt::rights::SEND
                | rt::rights::RECEIVE
                | rt::rights::DUPLICATE
                | rt::rights::TRANSFER,
        },
    ];
    let mut startup_words = [0u64; rt::IPC_MAX_WORDS];
    startup_words[0] = workload as u32 as u64;
    startup_words[1] = arg_len as u64;
    let packed = rt::pack_bytes(&arg_bytes[..arg_len], &mut startup_words[2..])?;
    let task_handle = rt::manager_launch_program_with_payload(
        bootstrap,
        rt::ServiceImageId::PosixHostTool,
        &startup_words[..2 + packed as usize],
        &startup_handles,
    );
    let _ = rt::handle_close(output_handle);
    let _ = rt::handle_close(pair.second);

    match task_handle {
        Ok(task_handle) => {
            runs[slot_index] = RunSlot {
                occupied: true,
                env_id: env_id as u32,
                workload,
                state: RuntimeRunState::Running,
                task_handle,
                session_handle: pair.first,
                exit_code: 0,
            };
            env.active_runs = env.active_runs.saturating_add(1);
            reply.words[0] = RuntimeStatus::Ok as u32 as u64;
            reply.words[1] = slot_index as u64;
            let _ = emit_log(
                log_handle,
                LogSeverity::Info,
                LogEvent::RuntimeLaunchStarted,
                slot_index as u64,
                ((env_id as u64) << 32) | workload as u32 as u64,
            );
        }
        Err(error) => {
            let _ = rt::handle_close(pair.first);
            reply.words[0] = match error {
                rt::Error::PermissionDenied => RuntimeStatus::Denied as u32 as u64,
                rt::Error::NotFound => RuntimeStatus::NotFound as u32 as u64,
                _ => RuntimeStatus::Busy as u32 as u64,
            };
        }
    }

    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

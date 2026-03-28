use serviceos_userspace_runtime as rt;
use rt::{
    LogEvent, LogSeverity, RawMessage, RuntimeRunState, RuntimeStatus, RuntimeTag,
    RuntimeWorkloadKind, TaskStateCode,
};

use crate::{
    consts::{MAX_ENVS, MAX_RUNS, MAX_STORAGE_PATH},
    types::{EnvSlot, FixedBytes, Profile, RunSlot},
    util::{emit_log, instantiate_env, pack_pair, release_run_slot, resolve_guest_path},
};

pub(crate) fn handle_public_request(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    log_handle: rt::Handle,
    profile: Profile,
    envs: &mut [EnvSlot; MAX_ENVS],
    runs: &mut [RunSlot; MAX_RUNS],
    message: &RawMessage,
) -> rt::Result<()> {
    match message.tag {
        x if x == RuntimeTag::EnvCreateRequest as u32 => {
            if message.handle_count < 1 || message.word_count < 1 {
                return Ok(());
            }
            let reply_handle = message.handles[0];
            let mut reply = RawMessage::empty(RuntimeTag::EnvCreateReply as u32);
            reply.word_count = 2;
            match allocate_env(envs, message.words[0], profile) {
                Ok(env_id) => {
                    reply.words[0] = RuntimeStatus::Ok as u32 as u64;
                    reply.words[1] = env_id as u64;
                    let _ = emit_log(
                        log_handle,
                        LogSeverity::Info,
                        LogEvent::RuntimeEnvironmentCreated,
                        env_id as u64,
                        message.words[0],
                    );
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
            if message.handle_count < 1 {
                return Ok(());
            }
            let start = if message.word_count > 0 {
                message.words[0] as usize
            } else {
                0
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
                if !env.occupied {
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
            handle_run_launch_request(
                bootstrap,
                log_handle,
                envs,
                runs,
                message,
            )?;
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
                reply.words[base + 2] = run.workload as u32 as u64;
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
                reply.words[3] = run.workload as u32 as u64;
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
    let _ = storage_handle;
    Ok(())
}

pub(crate) fn handle_run_session_request(
    storage_handle: rt::Handle,
    log_handle: rt::Handle,
    envs: &[EnvSlot; MAX_ENVS],
    run: &RunSlot,
    message: &RawMessage,
) -> rt::Result<()> {
    match message.tag {
        x if x == RuntimeTag::SessionInfoRequest as u32 => {
            if message.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = message.handles[0];
            let mut reply = RawMessage::empty(RuntimeTag::SessionInfoReply as u32);
            reply.word_count = 8;
            if let Some(env) = envs.get(run.env_id as usize).filter(|env| env.occupied) {
                encode_env_status(&mut reply, run.env_id, *env);
            } else {
                reply.words[0] = RuntimeStatus::NotFound as u32 as u64;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == RuntimeTag::SessionMountListRequest as u32 => {
            handle_session_mount_request(envs, run.env_id, message)?;
        }
        x if x == RuntimeTag::SessionVarListRequest as u32 => {
            handle_session_var_request(envs, run.env_id, message)?;
        }
        x if x == RuntimeTag::SessionReadFileRequest as u32 => {
            if message.handle_count < 1 || message.word_count < 3 {
                return Ok(());
            }
            let reply_handle = message.handles[0];
            let mut reply = RawMessage::empty(RuntimeTag::SessionReadFileReply as u32);
            reply.word_count = 2;
            let offset = message.words[0] as usize;
            let path_len = message.words[1] as usize;
            let requested = message.words[2] as usize;
            let mut guest_path = [0u8; MAX_STORAGE_PATH];
            let mut resolved = FixedBytes::<MAX_STORAGE_PATH>::empty();
            let env = envs
                .get(run.env_id as usize)
                .copied()
                .filter(|env| env.occupied)
                .ok_or(rt::Error::NotFound)?;
            if rt::unpack_bytes(
                &message.words[3..message.word_count as usize],
                path_len,
                &mut guest_path,
            )
            .is_err()
            {
                reply.words[0] = RuntimeStatus::InvalidPath as u32 as u64;
            } else if resolve_guest_path(&env, &guest_path[..path_len], &mut resolved).is_err() {
                reply.words[0] = RuntimeStatus::InvalidPath as u32 as u64;
            } else {
                match rt::storage_open(
                    storage_handle,
                    core::str::from_utf8(resolved.as_bytes())
                        .map_err(|_| rt::Error::InvalidArgument)?,
                ) {
                    Ok((blob_handle, _)) => {
                        let mut buffer = [0u8; (rt::IPC_MAX_WORDS - 2) * 8];
                        let read_len = requested.min(buffer.len());
                        let read = rt::storage_read(blob_handle, offset, &mut buffer[..read_len])?;
                        let _ = rt::storage_blob_close(blob_handle);
                        reply.words[0] = RuntimeStatus::Ok as u32 as u64;
                        reply.words[1] = read as u64;
                        reply.word_count = 2 + rt::pack_bytes(&buffer[..read], &mut reply.words[2..])?;
                        let _ = emit_log(
                            log_handle,
                            LogSeverity::Debug,
                            LogEvent::RuntimeMappedRead,
                            run.env_id as u64,
                            read as u64,
                        );
                    }
                    Err(rt::Error::NotFound) => {
                        reply.words[0] = RuntimeStatus::NotFound as u32 as u64;
                    }
                    Err(_) => {
                        reply.words[0] = RuntimeStatus::Busy as u32 as u64;
                    }
                }
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        _ => {}
    }
    Ok(())
}

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

fn allocate_env(
    envs: &mut [EnvSlot; MAX_ENVS],
    kind_word: u64,
    profile: Profile,
) -> rt::Result<u32> {
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
    Ok(index as u32)
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

fn encode_env_status(reply: &mut RawMessage, env_id: u32, env: EnvSlot) {
    reply.words[0] = RuntimeStatus::Ok as u32 as u64;
    reply.words[1] = env_id as u64;
    reply.words[2] = env.kind as u32 as u64;
    reply.words[3] = env.state as u32 as u64;
    reply.words[4] = env.capabilities as u64;
    reply.words[5] = env.mount_count as u64;
    reply.words[6] = env.var_count as u64;
    reply.words[7] = env.active_runs as u64;
}

fn handle_env_mount_request(envs: &[EnvSlot; MAX_ENVS], message: &RawMessage) -> rt::Result<()> {
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

fn handle_env_var_request(envs: &[EnvSlot; MAX_ENVS], message: &RawMessage) -> rt::Result<()> {
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

fn handle_session_mount_request(
    envs: &[EnvSlot; MAX_ENVS],
    env_id: u32,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.handle_count < 1 || message.word_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let index = message.words[0] as usize;
    let mut reply = RawMessage::empty(RuntimeTag::SessionMountListReply as u32);
    reply.word_count = 3;
    match envs.get(env_id as usize).filter(|env| env.occupied) {
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

fn handle_session_var_request(
    envs: &[EnvSlot; MAX_ENVS],
    env_id: u32,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.handle_count < 1 || message.word_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let index = message.words[0] as usize;
    let mut reply = RawMessage::empty(RuntimeTag::SessionVarListReply as u32);
    reply.word_count = 3;
    match envs.get(env_id as usize).filter(|env| env.occupied) {
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

fn handle_run_launch_request(
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

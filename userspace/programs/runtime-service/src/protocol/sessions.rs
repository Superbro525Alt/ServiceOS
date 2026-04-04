use serviceos_userspace_runtime as rt;
use rt::{LogEvent, LogSeverity, RawMessage, RuntimeStatus, RuntimeTag};

use crate::{
    consts::{MAX_ENVS, MAX_STORAGE_PATH},
    types::{EnvSlot, FixedBytes, RunSlot},
    util::{emit_log, pack_pair, resolve_guest_path},
};

use super::envs::encode_env_status;

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
            handle_session_read_file_request(storage_handle, log_handle, envs, run, message)?;
        }
        _ => {}
    }
    Ok(())
}

fn handle_session_read_file_request(
    storage_handle: rt::Handle,
    log_handle: rt::Handle,
    envs: &[EnvSlot; MAX_ENVS],
    run: &RunSlot,
    message: &RawMessage,
) -> rt::Result<()> {
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
            core::str::from_utf8(resolved.as_bytes()).map_err(|_| rt::Error::InvalidArgument)?,
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

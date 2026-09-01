use crate::{
    Error, Handle, IPC_MAX_WORDS, PermissionPolicyState, RawMessage, Result, RuntimeAuditInfo,
    RuntimeEnvInfo, RuntimeKind, RuntimeRunInfo, RuntimeStatus, RuntimeTag, RuntimeWorkloadKind,
    channel_create, channel_receive_blocking, channel_send, handle_close, handle_duplicate,
    pack_bytes, rights, runtime_env_state_from_word, runtime_kind_from_word,
    runtime_run_state_from_word, runtime_status_error, runtime_status_from_word,
    runtime_workload_kind_from_word, security_audit_kind_from_word, unpack_bytes,
};

pub fn runtime_env_create(runtime_handle: Handle, kind: RuntimeKind) -> Result<u32> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(RuntimeTag::EnvCreateRequest as u32);
    request.word_count = 1;
    request.words[0] = kind as u32 as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(runtime_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != RuntimeTag::EnvCreateReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match runtime_status_from_word(response.words[0]) {
        RuntimeStatus::Ok => Ok(response.words[1] as u32),
        status => Err(runtime_status_error(status)),
    }
}

pub fn runtime_env_destroy(runtime_handle: Handle, env_id: u32) -> Result<()> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(RuntimeTag::EnvDestroyRequest as u32);
    request.word_count = 1;
    request.words[0] = env_id as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(runtime_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != RuntimeTag::EnvDestroyReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match runtime_status_from_word(response.words[0]) {
        RuntimeStatus::Ok => Ok(()),
        status => Err(runtime_status_error(status)),
    }
}

pub fn runtime_env_list(runtime_handle: Handle, envs: &mut [RuntimeEnvInfo]) -> Result<usize> {
    let mut filled = 0usize;
    let mut start = 0usize;

    loop {
        let reply = channel_create()?;
        let mut request = RawMessage::empty(RuntimeTag::EnvListRequest as u32);
        request.word_count = 1;
        request.words[0] = start as u64;
        request.handle_count = 1;
        request.handles[0] = reply.second;
        request.handle_rights[0] = rights::SEND;
        channel_send(runtime_handle, &request)?;
        let _ = handle_close(reply.second);

        let mut response = RawMessage::empty(0);
        channel_receive_blocking(reply.first, &mut response)?;
        let _ = handle_close(reply.first);
        if response.tag != RuntimeTag::EnvListReply as u32 || response.word_count < 3 {
            return Err(Error::InvalidArgument);
        }
        match runtime_status_from_word(response.words[0]) {
            RuntimeStatus::Ok => {}
            status => return Err(runtime_status_error(status)),
        }

        let count = response.words[1] as usize;
        let next = response.words[2] as usize;
        if filled + count > envs.len() || response.word_count as usize != 3 + count * 6 {
            return Err(Error::BufferTooSmall);
        }
        for page_index in 0..count {
            let base = 3 + page_index * 6;
            envs[filled + page_index] = RuntimeEnvInfo {
                env_id: response.words[base] as u32,
                kind: runtime_kind_from_word(response.words[base + 1]),
                state: runtime_env_state_from_word(response.words[base + 2]),
                capabilities: response.words[base + 3] as u32,
                mount_count: response.words[base + 4] as u32,
                var_count: 0,
                active_runs: response.words[base + 5] as u32,
            };
        }
        filled += count;
        if count == 0 || next <= start {
            return Ok(filled);
        }
        start = next;
    }
}

pub fn runtime_env_status(runtime_handle: Handle, env_id: u32) -> Result<RuntimeEnvInfo> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(RuntimeTag::EnvStatusRequest as u32);
    request.word_count = 1;
    request.words[0] = env_id as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(runtime_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != RuntimeTag::EnvStatusReply as u32 || response.word_count < 8 {
        return Err(Error::InvalidArgument);
    }
    match runtime_status_from_word(response.words[0]) {
        RuntimeStatus::Ok => Ok(RuntimeEnvInfo {
            env_id: response.words[1] as u32,
            kind: runtime_kind_from_word(response.words[2]),
            state: runtime_env_state_from_word(response.words[3]),
            capabilities: response.words[4] as u32,
            mount_count: response.words[5] as u32,
            var_count: response.words[6] as u32,
            active_runs: response.words[7] as u32,
        }),
        status => Err(runtime_status_error(status)),
    }
}

pub fn runtime_env_mount(
    runtime_handle: Handle,
    env_id: u32,
    index: usize,
    guest: &mut [u8],
    source: &mut [u8],
) -> Result<Option<(usize, usize)>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(RuntimeTag::EnvMountListRequest as u32);
    request.word_count = 2;
    request.words[0] = env_id as u64;
    request.words[1] = index as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(runtime_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != RuntimeTag::EnvMountListReply as u32 || response.word_count < 3 {
        return Err(Error::InvalidArgument);
    }
    match runtime_status_from_word(response.words[0]) {
        RuntimeStatus::Ok => {
            let guest_len = response.words[1] as usize;
            let source_len = response.words[2] as usize;
            let total = guest_len + source_len;
            let total_words = total.div_ceil(8);
            if guest_len > guest.len()
                || source_len > source.len()
                || response.word_count as usize != 3 + total_words
            {
                return Err(Error::BufferTooSmall);
            }
            let mut combined = [0u8; IPC_MAX_WORDS * 8];
            unpack_bytes(
                &response.words[3..response.word_count as usize],
                total,
                &mut combined,
            )?;
            guest[..guest_len].copy_from_slice(&combined[..guest_len]);
            source[..source_len].copy_from_slice(&combined[guest_len..guest_len + source_len]);
            Ok(Some((guest_len, source_len)))
        }
        RuntimeStatus::NotFound => Ok(None),
        status => Err(runtime_status_error(status)),
    }
}

pub fn runtime_env_var(
    runtime_handle: Handle,
    env_id: u32,
    index: usize,
    key: &mut [u8],
    value: &mut [u8],
) -> Result<Option<(usize, usize)>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(RuntimeTag::EnvVarListRequest as u32);
    request.word_count = 2;
    request.words[0] = env_id as u64;
    request.words[1] = index as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(runtime_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != RuntimeTag::EnvVarListReply as u32 || response.word_count < 3 {
        return Err(Error::InvalidArgument);
    }
    match runtime_status_from_word(response.words[0]) {
        RuntimeStatus::Ok => {
            let key_len = response.words[1] as usize;
            let value_len = response.words[2] as usize;
            let total = key_len + value_len;
            let total_words = total.div_ceil(8);
            if key_len > key.len()
                || value_len > value.len()
                || response.word_count as usize != 3 + total_words
            {
                return Err(Error::BufferTooSmall);
            }
            let mut combined = [0u8; IPC_MAX_WORDS * 8];
            unpack_bytes(
                &response.words[3..response.word_count as usize],
                total,
                &mut combined,
            )?;
            key[..key_len].copy_from_slice(&combined[..key_len]);
            value[..value_len].copy_from_slice(&combined[key_len..key_len + value_len]);
            Ok(Some((key_len, value_len)))
        }
        RuntimeStatus::NotFound => Ok(None),
        status => Err(runtime_status_error(status)),
    }
}

/// Sandbox-manifest launch-envelope constants. Must mirror the receiving
/// side in `runtime-service/src/sandbox.rs` (`SANDBOX_MANIFEST_VERSION`,
/// `SANDBOX_MANIFEST_BLOB_LEN`, and the header word layout
/// `blob_len | version << 56`).
const SANDBOX_MANIFEST_VERSION: u64 = 1;
const SANDBOX_MANIFEST_BLOB_LEN: usize = 8;

/// Pure packing core of the launch envelope, shared by the manifest-less
/// and manifest-carrying senders. Layout: words[0..3] header, packed
/// argument words, then optionally one manifest header word plus one packed
/// 8-byte manifest blob word. The argument budget shrinks by two words when
/// a manifest rides along (16-word IPC envelope).
fn pack_launch_words(
    request: &mut RawMessage,
    env_id: u32,
    workload: RuntimeWorkloadKind,
    arg_bytes: &[u8],
    manifest: Option<&[u8; SANDBOX_MANIFEST_BLOB_LEN]>,
) -> Result<()> {
    let manifest_words = usize::from(manifest.is_some()) * 2;
    let max_inline_bytes = IPC_MAX_WORDS.saturating_sub(3 + manifest_words) * 8;
    if arg_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }
    let packed = pack_bytes(arg_bytes, &mut request.words[3..])?;
    request.word_count = (3 + packed as usize + manifest_words) as u32;
    if let Some(blob) = manifest {
        let header_index = 3 + packed as usize;
        request.words[header_index] =
            (SANDBOX_MANIFEST_VERSION << 56) | SANDBOX_MANIFEST_BLOB_LEN as u64;
        request.words[header_index + 1] = u64::from_le_bytes(*blob);
    }
    request.words[0] = env_id as u64;
    request.words[1] = workload as u32 as u64;
    request.words[2] = arg_bytes.len() as u64;
    Ok(())
}

pub fn runtime_run_launch(
    runtime_handle: Handle,
    env_id: u32,
    workload: RuntimeWorkloadKind,
    argument: &str,
    output_handle: Handle,
) -> Result<u32> {
    runtime_run_launch_with_manifest(
        runtime_handle,
        env_id,
        workload,
        argument,
        output_handle,
        None,
    )
}

/// Launch a workload, optionally carrying its per-workload sandbox manifest
/// as additive trailing envelope words. Workloads launched without a
/// manifest produce byte-identical messages to `runtime_run_launch`.
pub fn runtime_run_launch_with_manifest(
    runtime_handle: Handle,
    env_id: u32,
    workload: RuntimeWorkloadKind,
    argument: &str,
    output_handle: Handle,
    manifest: Option<&[u8; SANDBOX_MANIFEST_BLOB_LEN]>,
) -> Result<u32> {
    let arg_bytes = argument.as_bytes();
    let manifest_words = usize::from(manifest.is_some()) * 2;
    let max_inline_bytes = IPC_MAX_WORDS.saturating_sub(3 + manifest_words) * 8;
    if arg_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let transferred_output = handle_duplicate(
        output_handle,
        rights::SEND | rights::DUPLICATE | rights::TRANSFER,
    )?;
    let mut request = RawMessage::empty(RuntimeTag::RunLaunchRequest as u32);
    pack_launch_words(&mut request, env_id, workload, arg_bytes, manifest)?;
    request.handle_count = 2;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    request.handles[1] = transferred_output;
    request.handle_rights[1] = rights::SEND | rights::DUPLICATE | rights::TRANSFER;
    channel_send(runtime_handle, &request)?;
    let _ = handle_close(transferred_output);
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != RuntimeTag::RunLaunchReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match runtime_status_from_word(response.words[0]) {
        RuntimeStatus::Ok => Ok(response.words[1] as u32),
        status => Err(runtime_status_error(status)),
    }
}

pub fn runtime_run_list(runtime_handle: Handle, runs: &mut [RuntimeRunInfo]) -> Result<usize> {
    let mut filled = 0usize;
    let mut start = 0usize;

    loop {
        let reply = channel_create()?;
        let mut request = RawMessage::empty(RuntimeTag::RunListRequest as u32);
        request.word_count = 1;
        request.words[0] = start as u64;
        request.handle_count = 1;
        request.handles[0] = reply.second;
        request.handle_rights[0] = rights::SEND;
        channel_send(runtime_handle, &request)?;
        let _ = handle_close(reply.second);

        let mut response = RawMessage::empty(0);
        channel_receive_blocking(reply.first, &mut response)?;
        let _ = handle_close(reply.first);
        if response.tag != RuntimeTag::RunListReply as u32 || response.word_count < 3 {
            return Err(Error::InvalidArgument);
        }
        match runtime_status_from_word(response.words[0]) {
            RuntimeStatus::Ok => {}
            status => return Err(runtime_status_error(status)),
        }

        let count = response.words[1] as usize;
        let next = response.words[2] as usize;
        if filled + count > runs.len() || response.word_count as usize != 3 + count * 5 {
            return Err(Error::BufferTooSmall);
        }
        for page_index in 0..count {
            let base = 3 + page_index * 5;
            runs[filled + page_index] = RuntimeRunInfo {
                run_id: response.words[base] as u32,
                env_id: response.words[base + 1] as u32,
                workload: runtime_workload_kind_from_word(response.words[base + 2]),
                state: runtime_run_state_from_word(response.words[base + 3]),
                exit_code: response.words[base + 4],
            };
        }
        filled += count;
        if count == 0 || next <= start {
            return Ok(filled);
        }
        start = next;
    }
}

pub fn runtime_run_status(runtime_handle: Handle, run_id: u32) -> Result<RuntimeRunInfo> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(RuntimeTag::RunStatusRequest as u32);
    request.word_count = 1;
    request.words[0] = run_id as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(runtime_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != RuntimeTag::RunStatusReply as u32 || response.word_count < 6 {
        return Err(Error::InvalidArgument);
    }
    match runtime_status_from_word(response.words[0]) {
        RuntimeStatus::Ok => Ok(RuntimeRunInfo {
            run_id: response.words[1] as u32,
            env_id: response.words[2] as u32,
            workload: runtime_workload_kind_from_word(response.words[3]),
            state: runtime_run_state_from_word(response.words[4]),
            exit_code: response.words[5],
        }),
        status => Err(runtime_status_error(status)),
    }
}

pub fn runtime_session_info(session_handle: Handle) -> Result<RuntimeEnvInfo> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(RuntimeTag::SessionInfoRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(session_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != RuntimeTag::SessionInfoReply as u32 || response.word_count < 8 {
        return Err(Error::InvalidArgument);
    }
    match runtime_status_from_word(response.words[0]) {
        RuntimeStatus::Ok => Ok(RuntimeEnvInfo {
            env_id: response.words[1] as u32,
            kind: runtime_kind_from_word(response.words[2]),
            state: runtime_env_state_from_word(response.words[3]),
            capabilities: response.words[4] as u32,
            mount_count: response.words[5] as u32,
            var_count: response.words[6] as u32,
            active_runs: response.words[7] as u32,
        }),
        status => Err(runtime_status_error(status)),
    }
}

pub fn runtime_session_mount(
    session_handle: Handle,
    index: usize,
    guest: &mut [u8],
    source: &mut [u8],
) -> Result<Option<(usize, usize)>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(RuntimeTag::SessionMountListRequest as u32);
    request.word_count = 1;
    request.words[0] = index as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(session_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != RuntimeTag::SessionMountListReply as u32 || response.word_count < 3 {
        return Err(Error::InvalidArgument);
    }
    match runtime_status_from_word(response.words[0]) {
        RuntimeStatus::Ok => {
            let guest_len = response.words[1] as usize;
            let source_len = response.words[2] as usize;
            let total = guest_len + source_len;
            let total_words = total.div_ceil(8);
            if guest_len > guest.len()
                || source_len > source.len()
                || response.word_count as usize != 3 + total_words
            {
                return Err(Error::BufferTooSmall);
            }
            let mut combined = [0u8; IPC_MAX_WORDS * 8];
            unpack_bytes(
                &response.words[3..response.word_count as usize],
                total,
                &mut combined,
            )?;
            guest[..guest_len].copy_from_slice(&combined[..guest_len]);
            source[..source_len].copy_from_slice(&combined[guest_len..guest_len + source_len]);
            Ok(Some((guest_len, source_len)))
        }
        RuntimeStatus::NotFound => Ok(None),
        status => Err(runtime_status_error(status)),
    }
}

pub fn runtime_session_var(
    session_handle: Handle,
    index: usize,
    key: &mut [u8],
    value: &mut [u8],
) -> Result<Option<(usize, usize)>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(RuntimeTag::SessionVarListRequest as u32);
    request.word_count = 1;
    request.words[0] = index as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(session_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != RuntimeTag::SessionVarListReply as u32 || response.word_count < 3 {
        return Err(Error::InvalidArgument);
    }
    match runtime_status_from_word(response.words[0]) {
        RuntimeStatus::Ok => {
            let key_len = response.words[1] as usize;
            let value_len = response.words[2] as usize;
            let total = key_len + value_len;
            let total_words = total.div_ceil(8);
            if key_len > key.len()
                || value_len > value.len()
                || response.word_count as usize != 3 + total_words
            {
                return Err(Error::BufferTooSmall);
            }
            let mut combined = [0u8; IPC_MAX_WORDS * 8];
            unpack_bytes(
                &response.words[3..response.word_count as usize],
                total,
                &mut combined,
            )?;
            key[..key_len].copy_from_slice(&combined[..key_len]);
            value[..value_len].copy_from_slice(&combined[key_len..key_len + value_len]);
            Ok(Some((key_len, value_len)))
        }
        RuntimeStatus::NotFound => Ok(None),
        status => Err(runtime_status_error(status)),
    }
}

pub fn runtime_session_read_file(
    session_handle: Handle,
    guest_path: &str,
    offset: usize,
    buffer: &mut [u8],
) -> Result<usize> {
    let path_bytes = guest_path.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(3)) * 8;
    let requested = buffer.len().min(max_inline_bytes);
    if path_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(RuntimeTag::SessionReadFileRequest as u32);
    request.word_count = 3 + pack_bytes(path_bytes, &mut request.words[3..])?;
    request.words[0] = offset as u64;
    request.words[1] = path_bytes.len() as u64;
    request.words[2] = requested as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(session_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != RuntimeTag::SessionReadFileReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match runtime_status_from_word(response.words[0]) {
        RuntimeStatus::Ok => {
            let byte_len = response.words[1] as usize;
            if byte_len > buffer.len() {
                return Err(Error::BufferTooSmall);
            }
            unpack_bytes(
                &response.words[2..response.word_count as usize],
                byte_len,
                buffer,
            )?;
            Ok(byte_len)
        }
        status => Err(runtime_status_error(status)),
    }
}

/// Packs the EnvDecisionRequest additively: words 0..2 are the legacy shape
/// (env id, policy word) that every existing caller sends; a mask request
/// appends word 2 = allowed capability mask, which runtime-service applies as
/// granted = requested ∩ sensitive.
pub(crate) fn runtime_env_decide_request(
    env_id: u32,
    policy: PermissionPolicyState,
    mask: Option<u32>,
) -> RawMessage {
    let mut request = RawMessage::empty(RuntimeTag::EnvDecisionRequest as u32);
    request.word_count = match mask {
        Some(_) => 3,
        None => 2,
    };
    request.words[0] = env_id as u64;
    request.words[1] = policy as u32 as u64;
    if let Some(allowed) = mask {
        request.words[2] = allowed as u64;
    }
    request
}

pub fn runtime_env_decide(
    runtime_handle: Handle,
    env_id: u32,
    policy: PermissionPolicyState,
) -> Result<()> {
    runtime_env_decide_with_mask(runtime_handle, env_id, policy, None)
}

/// Decision variant with a per-capability allow-mask: `Some(mask)` narrows the
/// grant to the requested subset (runtime-service keeps the env
/// PendingApproval until every sensitive bit is granted); `None` keeps the
/// legacy approve-all shape.
pub fn runtime_env_decide_with_mask(
    runtime_handle: Handle,
    env_id: u32,
    policy: PermissionPolicyState,
    allowed_mask: Option<u32>,
) -> Result<()> {
    let reply = channel_create()?;
    let mut request = runtime_env_decide_request(env_id, policy, allowed_mask);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(runtime_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != RuntimeTag::EnvDecisionReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match runtime_status_from_word(response.words[0]) {
        RuntimeStatus::Ok => Ok(()),
        status => Err(runtime_status_error(status)),
    }
}

pub fn runtime_audit_list(
    runtime_handle: Handle,
    index: usize,
) -> Result<Option<RuntimeAuditInfo>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(RuntimeTag::AuditListRequest as u32);
    request.word_count = 1;
    request.words[0] = index as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(runtime_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != RuntimeTag::AuditListReply as u32 || response.word_count < 6 {
        return Err(Error::InvalidArgument);
    }
    match runtime_status_from_word(response.words[0]) {
        RuntimeStatus::Ok => Ok(Some(RuntimeAuditInfo {
            sequence: response.words[1] as u32,
            kind: security_audit_kind_from_word(response.words[2]),
            env_id: response.words[3] as u32,
            capabilities: response.words[4] as u32,
            detail: response.words[5] >> 32,
        })),
        RuntimeStatus::NotFound => Ok(None),
        status => Err(runtime_status_error(status)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_capability;

    #[test]
    fn decide_request_legacy_shape_stays_two_words() {
        let request = runtime_env_decide_request(7, PermissionPolicyState::Allowed, None);
        assert_eq!(request.tag, RuntimeTag::EnvDecisionRequest as u32);
        assert_eq!(request.word_count, 2);
        assert_eq!(request.words[0], 7);
        assert_eq!(
            request.words[1],
            PermissionPolicyState::Allowed as u32 as u64
        );
    }

    #[test]
    fn decide_request_mask_shape_appends_allowed_mask_word() {
        let mask = runtime_capability::NETWORK | runtime_capability::AUDIO;
        let request = runtime_env_decide_request(3, PermissionPolicyState::Allowed, Some(mask));
        assert_eq!(request.tag, RuntimeTag::EnvDecisionRequest as u32);
        assert_eq!(request.word_count, 3);
        assert_eq!(request.words[0], 3);
        assert_eq!(
            request.words[1],
            PermissionPolicyState::Allowed as u32 as u64
        );
        assert_eq!(request.words[2], mask as u64);

        // Legacy prefix words are byte-identical in both shapes.
        let legacy = runtime_env_decide_request(3, PermissionPolicyState::Allowed, None);
        assert_eq!(request.words[0..2], legacy.words[0..2]);
    }

    #[test]
    fn launch_packing_without_manifest_matches_legacy_layout() {
        let mut request = RawMessage::empty(RuntimeTag::RunLaunchRequest as u32);
        pack_launch_words(&mut request, 2, RuntimeWorkloadKind::Cat, b"/data/a", None)
            .expect("pack");
        assert_eq!(request.word_count, 3 + 1);
        assert_eq!(request.words[0], 2);
        assert_eq!(request.words[1], RuntimeWorkloadKind::Cat as u32 as u64);
        assert_eq!(request.words[2], 7);
        // Legacy envelopes never carry manifest words.
        assert_eq!(request.words[4], 0);
    }

    #[test]
    fn launch_packing_appends_manifest_trailing_words() {
        let blob: [u8; SANDBOX_MANIFEST_BLOB_LEN] = [1, 0, 0b0001, 0, 0, 0, 0, 0];
        let mut request = RawMessage::empty(RuntimeTag::RunLaunchRequest as u32);
        pack_launch_words(
            &mut request,
            0,
            RuntimeWorkloadKind::Inspect,
            b"/bin/demo",
            Some(&blob),
        )
        .expect("pack");
        // 3 header words + ceil(9/8)=2 arg words + 2 manifest words.
        assert_eq!(request.word_count, 3 + 2 + 2);
        let header_index = 3 + 2;
        assert_eq!(
            request.words[header_index],
            (SANDBOX_MANIFEST_VERSION << 56) | SANDBOX_MANIFEST_BLOB_LEN as u64
        );
        assert_eq!(request.words[header_index + 1], u64::from_le_bytes(blob));
        // Header + arg prefix are byte-identical to the manifest-less shape.
        let mut legacy = RawMessage::empty(RuntimeTag::RunLaunchRequest as u32);
        pack_launch_words(
            &mut legacy,
            0,
            RuntimeWorkloadKind::Inspect,
            b"/bin/demo",
            None,
        )
        .expect("legacy pack");
        assert_eq!(request.words[0..3 + 2], legacy.words[0..3 + 2]);
    }

    #[test]
    fn launch_packing_bounds_argument_by_manifest_budget() {
        let blob: [u8; SANDBOX_MANIFEST_BLOB_LEN] = [0; SANDBOX_MANIFEST_BLOB_LEN];
        let long = [b'a'; (IPC_MAX_WORDS - 3) * 8];
        let mut request = RawMessage::empty(0);
        // Full-legacy-budget argument fits without a manifest…
        assert!(
            pack_launch_words(&mut request, 0, RuntimeWorkloadKind::Inspect, &long, None).is_ok()
        );
        // …but cannot carry one: two trailing words would overflow the
        // 16-word envelope, and truncating silently is not an option.
        assert!(
            pack_launch_words(
                &mut request,
                0,
                RuntimeWorkloadKind::Inspect,
                &long,
                Some(&blob)
            )
            .is_err()
        );
        let max_manifest_arg = [b'a'; (IPC_MAX_WORDS - 3 - 2) * 8];
        assert!(
            pack_launch_words(
                &mut request,
                0,
                RuntimeWorkloadKind::Inspect,
                &max_manifest_arg,
                Some(&blob)
            )
            .is_ok()
        );
    }
}

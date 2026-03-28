use crate::{
    channel_create, channel_receive_blocking, channel_send, handle_close, handle_duplicate, pack_bytes,
    runtime_env_state_from_word, runtime_kind_from_word, runtime_run_state_from_word,
    runtime_status_error, runtime_status_from_word, runtime_workload_kind_from_word, rights,
    unpack_bytes, Error, Handle, RawMessage, Result, RuntimeEnvInfo, RuntimeKind, RuntimeRunInfo,
    RuntimeStatus, RuntimeTag, RuntimeWorkloadKind, IPC_MAX_WORDS,
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
            unpack_bytes(&response.words[3..response.word_count as usize], total, &mut combined)?;
            guest[..guest_len].copy_from_slice(&combined[..guest_len]);
            source[..source_len]
                .copy_from_slice(&combined[guest_len..guest_len + source_len]);
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
            unpack_bytes(&response.words[3..response.word_count as usize], total, &mut combined)?;
            key[..key_len].copy_from_slice(&combined[..key_len]);
            value[..value_len].copy_from_slice(&combined[key_len..key_len + value_len]);
            Ok(Some((key_len, value_len)))
        }
        RuntimeStatus::NotFound => Ok(None),
        status => Err(runtime_status_error(status)),
    }
}

pub fn runtime_run_launch(
    runtime_handle: Handle,
    env_id: u32,
    workload: RuntimeWorkloadKind,
    argument: &str,
    output_handle: Handle,
) -> Result<u32> {
    let arg_bytes = argument.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(3)) * 8;
    if arg_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let transferred_output = handle_duplicate(
        output_handle,
        rights::SEND | rights::DUPLICATE | rights::TRANSFER,
    )?;
    let mut request = RawMessage::empty(RuntimeTag::RunLaunchRequest as u32);
    request.word_count = 3 + pack_bytes(arg_bytes, &mut request.words[3..])?;
    request.words[0] = env_id as u64;
    request.words[1] = workload as u32 as u64;
    request.words[2] = arg_bytes.len() as u64;
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
            unpack_bytes(&response.words[3..response.word_count as usize], total, &mut combined)?;
            guest[..guest_len].copy_from_slice(&combined[..guest_len]);
            source[..source_len]
                .copy_from_slice(&combined[guest_len..guest_len + source_len]);
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
            unpack_bytes(&response.words[3..response.word_count as usize], total, &mut combined)?;
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
            unpack_bytes(&response.words[2..response.word_count as usize], byte_len, buffer)?;
            Ok(byte_len)
        }
        status => Err(runtime_status_error(status)),
    }
}

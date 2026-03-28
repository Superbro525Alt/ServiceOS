use crate::{
    channel_create, channel_receive_blocking, channel_send, handle_close, handle_duplicate,
    unpack_bytes, developer_artifact_format_from_word, developer_job_state_from_word,
    developer_status_error, developer_status_from_word, developer_target_from_word,
    developer_toolchain_state_from_word, rights, DeveloperArtifactFormat, DeveloperJobInfo,
    DeveloperStatus, DeveloperTag, DeveloperTarget, DeveloperToolchainInfo, DeveloperWorkspaceInfo,
    Error, Handle, RawMessage, Result, IPC_MAX_WORDS,
};

pub fn developer_toolchain_list(
    developer_handle: Handle,
    toolchains: &mut [DeveloperToolchainInfo],
) -> Result<usize> {
    let mut index = 0usize;
    while index < toolchains.len() {
        let reply = channel_create()?;
        let mut request = RawMessage::empty(DeveloperTag::ToolchainListRequest as u32);
        request.word_count = 1;
        request.words[0] = index as u64;
        request.handle_count = 1;
        request.handles[0] = reply.second;
        request.handle_rights[0] = rights::SEND;
        channel_send(developer_handle, &request)?;
        let _ = handle_close(reply.second);

        let mut response = RawMessage::empty(0);
        channel_receive_blocking(reply.first, &mut response)?;
        let _ = handle_close(reply.first);
        if response.tag != DeveloperTag::ToolchainListReply as u32 || response.word_count < 6 {
            return Err(Error::InvalidArgument);
        }
        match developer_status_from_word(response.words[0]) {
            DeveloperStatus::Ok => {
                let name_len = response.words[5] as usize;
                if name_len > 64 {
                    return Err(Error::BufferTooSmall);
                }
                let mut name = [0u8; 64];
                unpack_bytes(
                    &response.words[6..response.word_count as usize],
                    name_len,
                    &mut name,
                )?;
                toolchains[index] = DeveloperToolchainInfo {
                    toolchain_id: response.words[1] as u32,
                    target: developer_target_from_word(response.words[2]),
                    state: developer_toolchain_state_from_word(response.words[3]),
                    format: developer_artifact_format_from_word(response.words[4]),
                    name_len: name_len as u32,
                    name,
                };
                index += 1;
            }
            DeveloperStatus::NotFound => return Ok(index),
            status => return Err(developer_status_error(status)),
        }
    }
    Ok(index)
}

pub fn developer_toolchain_status(
    developer_handle: Handle,
    toolchain_id: u32,
    name: &mut [u8],
    sdk_root: &mut [u8],
) -> Result<(DeveloperToolchainInfo, usize, usize)> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(DeveloperTag::ToolchainInfoRequest as u32);
    request.word_count = 1;
    request.words[0] = toolchain_id as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(developer_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != DeveloperTag::ToolchainInfoReply as u32 || response.word_count < 7 {
        return Err(Error::InvalidArgument);
    }
    match developer_status_from_word(response.words[0]) {
        DeveloperStatus::Ok => {
            let name_len = response.words[5] as usize;
            let sdk_len = response.words[6] as usize;
            let total = name_len + sdk_len;
            if name_len > name.len() || sdk_len > sdk_root.len() {
                return Err(Error::BufferTooSmall);
            }
            let mut combined = [0u8; IPC_MAX_WORDS * 8];
            unpack_bytes(
                &response.words[7..response.word_count as usize],
                total,
                &mut combined,
            )?;
            name[..name_len].copy_from_slice(&combined[..name_len]);
            sdk_root[..sdk_len].copy_from_slice(&combined[name_len..name_len + sdk_len]);
            Ok((
                DeveloperToolchainInfo {
                    toolchain_id: response.words[1] as u32,
                    target: developer_target_from_word(response.words[2]),
                    state: developer_toolchain_state_from_word(response.words[3]),
                    format: developer_artifact_format_from_word(response.words[4]),
                    name_len: name_len as u32,
                    name: [0; 64],
                },
                name_len,
                sdk_len,
            ))
        }
        status => Err(developer_status_error(status)),
    }
}

pub fn developer_workspace_list(
    developer_handle: Handle,
    workspaces: &mut [DeveloperWorkspaceInfo],
) -> Result<usize> {
    let mut index = 0usize;
    while index < workspaces.len() {
        let reply = channel_create()?;
        let mut request = RawMessage::empty(DeveloperTag::WorkspaceListRequest as u32);
        request.word_count = 1;
        request.words[0] = index as u64;
        request.handle_count = 1;
        request.handles[0] = reply.second;
        request.handle_rights[0] = rights::SEND;
        channel_send(developer_handle, &request)?;
        let _ = handle_close(reply.second);

        let mut response = RawMessage::empty(0);
        channel_receive_blocking(reply.first, &mut response)?;
        let _ = handle_close(reply.first);
        if response.tag != DeveloperTag::WorkspaceListReply as u32 || response.word_count < 4 {
            return Err(Error::InvalidArgument);
        }
        match developer_status_from_word(response.words[0]) {
            DeveloperStatus::Ok => {
                let name_len = response.words[3] as usize;
                if name_len > 64 {
                    return Err(Error::BufferTooSmall);
                }
                let mut name = [0u8; 64];
                unpack_bytes(
                    &response.words[4..response.word_count as usize],
                    name_len,
                    &mut name,
                )?;
                workspaces[index] = DeveloperWorkspaceInfo {
                    workspace_id: response.words[1] as u32,
                    target_mask: response.words[2] as u32,
                    name_len: name_len as u32,
                    name,
                    source_path_len: 0,
                    source_path: [0; 96],
                    toolchains: [u32::MAX; 4],
                };
                index += 1;
            }
            DeveloperStatus::NotFound => return Ok(index),
            status => return Err(developer_status_error(status)),
        }
    }
    Ok(index)
}

pub fn developer_workspace_status(
    developer_handle: Handle,
    workspace_id: u32,
    name: &mut [u8],
    source_path: &mut [u8],
) -> Result<DeveloperWorkspaceInfo> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(DeveloperTag::WorkspaceInfoRequest as u32);
    request.word_count = 1;
    request.words[0] = workspace_id as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(developer_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != DeveloperTag::WorkspaceInfoReply as u32 || response.word_count < 5 {
        return Err(Error::InvalidArgument);
    }
    match developer_status_from_word(response.words[0]) {
        DeveloperStatus::Ok => {
            let workspace_id = response.words[1] as u32;
            let target_mask = (response.words[1] >> 32) as u32;
            let name_len = response.words[2] as u32 as usize;
            let source_len = (response.words[2] >> 32) as u32 as usize;
            let total = name_len + source_len;
            if name_len > name.len() || source_len > source_path.len() {
                return Err(Error::BufferTooSmall);
            }
            let mut combined = [0u8; IPC_MAX_WORDS * 8];
            unpack_bytes(
                &response.words[5..response.word_count as usize],
                total,
                &mut combined,
            )?;
            name[..name_len].copy_from_slice(&combined[..name_len]);
            source_path[..source_len]
                .copy_from_slice(&combined[name_len..name_len + source_len]);
            Ok(DeveloperWorkspaceInfo {
                workspace_id,
                target_mask,
                name_len: name_len as u32,
                name: [0; 64],
                source_path_len: source_len as u32,
                source_path: [0; 96],
                toolchains: [
                    response.words[3] as u32,
                    (response.words[3] >> 32) as u32,
                    response.words[4] as u32,
                    (response.words[4] >> 32) as u32,
                ],
            })
        }
        status => Err(developer_status_error(status)),
    }
}

pub fn developer_build_submit(
    developer_handle: Handle,
    workspace_id: u32,
    target: DeveloperTarget,
    output_handle: Handle,
) -> Result<u32> {
    let reply = channel_create()?;
    let transferred_output = handle_duplicate(
        output_handle,
        rights::SEND | rights::DUPLICATE | rights::TRANSFER,
    )?;
    let mut request = RawMessage::empty(DeveloperTag::BuildRequest as u32);
    request.word_count = 2;
    request.words[0] = workspace_id as u64;
    request.words[1] = target as u32 as u64;
    request.handle_count = 2;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    request.handles[1] = transferred_output;
    request.handle_rights[1] = rights::SEND | rights::DUPLICATE | rights::TRANSFER;
    channel_send(developer_handle, &request)?;
    let _ = handle_close(transferred_output);
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != DeveloperTag::BuildReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match developer_status_from_word(response.words[0]) {
        DeveloperStatus::Ok => Ok(response.words[1] as u32),
        status => Err(developer_status_error(status)),
    }
}

pub fn developer_job_list(
    developer_handle: Handle,
    jobs: &mut [DeveloperJobInfo],
) -> Result<usize> {
    let mut index = 0usize;
    while index < jobs.len() {
        let reply = channel_create()?;
        let mut request = RawMessage::empty(DeveloperTag::JobListRequest as u32);
        request.word_count = 1;
        request.words[0] = index as u64;
        request.handle_count = 1;
        request.handles[0] = reply.second;
        request.handle_rights[0] = rights::SEND;
        channel_send(developer_handle, &request)?;
        let _ = handle_close(reply.second);

        let mut response = RawMessage::empty(0);
        channel_receive_blocking(reply.first, &mut response)?;
        let _ = handle_close(reply.first);
        if response.tag != DeveloperTag::JobListReply as u32 || response.word_count < 7 {
            return Err(Error::InvalidArgument);
        }
        match developer_status_from_word(response.words[0]) {
            DeveloperStatus::Ok => {
                jobs[index] = DeveloperJobInfo {
                    job_id: response.words[1] as u32,
                    workspace_id: response.words[2] as u32,
                    target: developer_target_from_word(response.words[3]),
                    state: developer_job_state_from_word(response.words[4]),
                    format: developer_artifact_format_from_word(response.words[5]),
                    artifact_size: response.words[6] as usize,
                    artifact_name_len: 0,
                    artifact_name: [0; 64],
                };
                index += 1;
            }
            DeveloperStatus::NotFound => return Ok(index),
            status => return Err(developer_status_error(status)),
        }
    }
    Ok(index)
}

pub fn developer_job_status(
    developer_handle: Handle,
    job_id: u32,
    artifact_name: &mut [u8],
) -> Result<DeveloperJobInfo> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(DeveloperTag::JobInfoRequest as u32);
    request.word_count = 1;
    request.words[0] = job_id as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(developer_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != DeveloperTag::JobInfoReply as u32 || response.word_count < 8 {
        return Err(Error::InvalidArgument);
    }
    match developer_status_from_word(response.words[0]) {
        DeveloperStatus::Ok => {
            let name_len = response.words[7] as usize;
            if name_len > artifact_name.len() {
                return Err(Error::BufferTooSmall);
            }
            unpack_bytes(
                &response.words[8..response.word_count as usize],
                name_len,
                artifact_name,
            )?;
            Ok(DeveloperJobInfo {
                job_id: response.words[1] as u32,
                workspace_id: response.words[2] as u32,
                target: developer_target_from_word(response.words[3]),
                state: developer_job_state_from_word(response.words[4]),
                format: developer_artifact_format_from_word(response.words[5]),
                artifact_size: response.words[6] as usize,
                artifact_name_len: name_len as u32,
                artifact_name: [0; 64],
            })
        }
        status => Err(developer_status_error(status)),
    }
}

pub fn developer_artifact_open(
    developer_handle: Handle,
    job_id: u32,
    artifact_name: &mut [u8],
) -> Result<(Handle, usize, DeveloperArtifactFormat, usize)> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(DeveloperTag::ArtifactOpenRequest as u32);
    request.word_count = 1;
    request.words[0] = job_id as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(developer_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != DeveloperTag::ArtifactOpenReply as u32 || response.word_count < 4 {
        return Err(Error::InvalidArgument);
    }
    match developer_status_from_word(response.words[0]) {
        DeveloperStatus::Ok if response.handle_count > 0 => {
            let name_len = response.words[3] as usize;
            if name_len > artifact_name.len() {
                return Err(Error::BufferTooSmall);
            }
            unpack_bytes(
                &response.words[4..response.word_count as usize],
                name_len,
                artifact_name,
            )?;
            Ok((
                response.handles[0],
                response.words[1] as usize,
                developer_artifact_format_from_word(response.words[2]),
                name_len,
            ))
        }
        DeveloperStatus::Ok => Err(Error::InvalidArgument),
        status => Err(developer_status_error(status)),
    }
}

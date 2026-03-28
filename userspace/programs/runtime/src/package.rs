use crate::{
    channel_create, channel_receive_blocking, channel_send, handle_close, pack_bytes,
    package_status_error, package_status_from_word, rights, unpack_bytes, Error, Handle,
    PackageInfo, PackageListEntry, PackageStatus, PackageTag, RawMessage, Result, ServiceId,
    IPC_MAX_WORDS,
};

pub fn package_list(
    package_handle: Handle,
    index: usize,
    installed_version: &mut [u8],
    active_version: &mut [u8],
) -> Result<Option<PackageListEntry>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(PackageTag::ListRequest as u32);
    request.word_count = 1;
    request.words[0] = index as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(package_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != PackageTag::ListReply as u32 || response.word_count < 7 {
        return Err(Error::InvalidArgument);
    }

    let status = package_status_from_word(response.words[0]);
    if status == PackageStatus::End {
        return Ok(None);
    }
    if status != PackageStatus::Ok {
        return Err(package_status_error(status));
    }

    let installed_len = response.words[4] as usize;
    let active_len = response.words[5] as usize;
    let total_bytes = installed_len + active_len;
    let total_words = total_bytes.div_ceil(8);
    if response.word_count as usize != 7 + total_words {
        return Err(Error::InvalidArgument);
    }

    let mut combined = [0u8; IPC_MAX_WORDS * 8];
    unpack_bytes(
        &response.words[7..response.word_count as usize],
        total_bytes,
        &mut combined,
    )?;
    installed_version[..installed_len].copy_from_slice(&combined[..installed_len]);
    active_version[..active_len]
        .copy_from_slice(&combined[installed_len..installed_len + active_len]);

    Ok(Some(PackageListEntry {
        service_id: crate::service_id_from_word(response.words[1]),
        installed: response.words[2] & 1 != 0,
        active: response.words[2] & 2 != 0,
        rollback_available: response.words[2] & 4 != 0,
        repository_versions: response.words[3] as u32,
        installed_version_len: installed_len,
        active_version_len: active_len,
    }))
}

pub fn package_info(
    package_handle: Handle,
    service_id: ServiceId,
    installed_version: &mut [u8],
    active_version: &mut [u8],
    rollback_version: &mut [u8],
    latest_version: &mut [u8],
) -> Result<PackageInfo> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(PackageTag::InfoRequest as u32);
    request.word_count = 1;
    request.words[0] = service_id as u32 as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(package_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != PackageTag::InfoReply as u32 || response.word_count < 8 {
        return Err(Error::InvalidArgument);
    }

    let status = package_status_from_word(response.words[0]);
    if status != PackageStatus::Ok {
        return Err(package_status_error(status));
    }

    let installed_len = response.words[3] as usize;
    let active_len = response.words[4] as usize;
    let rollback_len = response.words[5] as usize;
    let latest_len = response.words[6] as usize;
    let total_bytes = installed_len + active_len + rollback_len + latest_len;
    let total_words = total_bytes.div_ceil(8);
    if response.word_count as usize != 8 + total_words {
        return Err(Error::InvalidArgument);
    }

    let mut combined = [0u8; IPC_MAX_WORDS * 8];
    unpack_bytes(
        &response.words[8..response.word_count as usize],
        total_bytes,
        &mut combined,
    )?;

    let mut offset = 0usize;
    installed_version[..installed_len].copy_from_slice(&combined[offset..offset + installed_len]);
    offset += installed_len;
    active_version[..active_len].copy_from_slice(&combined[offset..offset + active_len]);
    offset += active_len;
    rollback_version[..rollback_len].copy_from_slice(&combined[offset..offset + rollback_len]);
    offset += rollback_len;
    latest_version[..latest_len].copy_from_slice(&combined[offset..offset + latest_len]);

    Ok(PackageInfo {
        installed: response.words[1] & 1 != 0,
        active: response.words[1] & 2 != 0,
        rollback_available: response.words[1] & 4 != 0,
        repository_versions: response.words[2] as u32,
        installed_version_len: installed_len,
        active_version_len: active_len,
        rollback_version_len: rollback_len,
        latest_version_len: latest_len,
    })
}

pub fn package_install(
    package_handle: Handle,
    service_id: ServiceId,
    version: Option<&str>,
) -> Result<()> {
    package_mutation(
        package_handle,
        PackageTag::InstallRequest,
        PackageTag::InstallReply,
        service_id,
        version,
    )
}

pub fn package_update(
    package_handle: Handle,
    service_id: ServiceId,
    version: Option<&str>,
) -> Result<()> {
    package_mutation(
        package_handle,
        PackageTag::UpdateRequest,
        PackageTag::UpdateReply,
        service_id,
        version,
    )
}

pub fn package_remove(package_handle: Handle, service_id: ServiceId) -> Result<()> {
    package_mutation(
        package_handle,
        PackageTag::RemoveRequest,
        PackageTag::RemoveReply,
        service_id,
        None,
    )
}

pub fn package_rollback(package_handle: Handle, service_id: ServiceId) -> Result<()> {
    package_mutation(
        package_handle,
        PackageTag::RollbackRequest,
        PackageTag::RollbackReply,
        service_id,
        None,
    )
}

pub fn package_history(
    package_handle: Handle,
    service_id: ServiceId,
    current_version: &mut [u8],
    previous_version: &mut [u8],
) -> Result<(usize, usize)> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(PackageTag::HistoryRequest as u32);
    request.word_count = 1;
    request.words[0] = service_id as u32 as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(package_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != PackageTag::HistoryReply as u32 || response.word_count < 4 {
        return Err(Error::InvalidArgument);
    }

    let status = package_status_from_word(response.words[0]);
    if status != PackageStatus::Ok {
        return Err(package_status_error(status));
    }

    let current_len = response.words[1] as usize;
    let previous_len = response.words[2] as usize;
    let total_bytes = current_len + previous_len;
    let total_words = total_bytes.div_ceil(8);
    if response.word_count as usize != 4 + total_words {
        return Err(Error::InvalidArgument);
    }

    let mut combined = [0u8; IPC_MAX_WORDS * 8];
    unpack_bytes(
        &response.words[4..response.word_count as usize],
        total_bytes,
        &mut combined,
    )?;
    current_version[..current_len].copy_from_slice(&combined[..current_len]);
    previous_version[..previous_len]
        .copy_from_slice(&combined[current_len..current_len + previous_len]);
    Ok((current_len, previous_len))
}

fn package_mutation(
    package_handle: Handle,
    request_tag: PackageTag,
    reply_tag: PackageTag,
    service_id: ServiceId,
    version: Option<&str>,
) -> Result<()> {
    let version_bytes = version.unwrap_or("").as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(2)) * 8;
    if version_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(request_tag as u32);
    request.word_count = 2 + pack_bytes(version_bytes, &mut request.words[2..])?;
    request.words[0] = service_id as u32 as u64;
    request.words[1] = version_bytes.len() as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(package_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != reply_tag as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }

    match package_status_from_word(response.words[0]) {
        PackageStatus::Ok => Ok(()),
        status => Err(package_status_error(status)),
    }
}

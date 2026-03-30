use crate::{
    channel_create, channel_receive_blocking, channel_send, handle_close, pack_bytes, rights,
    unpack_bytes, Error, Handle, RawMessage, Result, StorageEntryKind, StorageStatus, StorageTag, IPC_MAX_WORDS,
};

pub fn storage_open(storage_handle: Handle, path: &str) -> Result<(Handle, usize)> {
    let path_bytes = path.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(1)) * 8;
    if path_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(StorageTag::OpenRequest as u32);
    request.word_count = 1 + pack_bytes(path_bytes, &mut request.words[1..])?;
    request.words[0] = path_bytes.len() as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(storage_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != StorageTag::OpenReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }

    match response.words[0] as u32 {
        x if x == StorageStatus::Ok as u32 && response.handle_count > 0 => {
            Ok((response.handles[0], response.words[1] as usize))
        }
        x if x == StorageStatus::NotFound as u32 => Err(Error::NotFound),
        x if x == StorageStatus::Busy as u32 => Err(Error::Busy),
        x if x == StorageStatus::Denied as u32 => Err(Error::PermissionDenied),
        x if x == StorageStatus::InvalidPath as u32 => Err(Error::InvalidArgument),
        _ => Err(Error::InvalidArgument),
    }
}

pub fn storage_read(blob_handle: Handle, offset: usize, buffer: &mut [u8]) -> Result<usize> {
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(3)) * 8;
    let requested = buffer.len().min(max_inline_bytes);
    let reply = channel_create()?;
    let mut request = RawMessage::empty(StorageTag::ReadRequest as u32);
    request.word_count = 2;
    request.words[0] = offset as u64;
    request.words[1] = requested as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(blob_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != StorageTag::ReadReply as u32 || response.word_count < 3 {
        return Err(Error::InvalidArgument);
    }

    match response.words[0] as u32 {
        x if x == StorageStatus::Ok as u32 => {
            let byte_len = response.words[2] as usize;
            if byte_len > requested || byte_len > buffer.len() {
                return Err(Error::BufferTooSmall);
            }
            unpack_bytes(&response.words[3..response.word_count as usize], byte_len, buffer)?;
            Ok(byte_len)
        }
        x if x == StorageStatus::InvalidOffset as u32 => Err(Error::InvalidArgument),
        x if x == StorageStatus::NotFound as u32 => Err(Error::NotFound),
        _ => Err(Error::InvalidArgument),
    }
}

pub fn storage_blob_close(blob_handle: Handle) -> Result<()> {
    let request = RawMessage::empty(StorageTag::CloseRequest as u32);
    let _ = channel_send(blob_handle, &request);
    handle_close(blob_handle)
}

pub fn storage_list(
    storage_handle: Handle,
    prefix: &str,
    index: usize,
    path_buffer: &mut [u8],
) -> Result<Option<(StorageStatus, usize)>> {
    let prefix_bytes = prefix.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(2)) * 8;
    if prefix_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(StorageTag::ListRequest as u32);
    request.word_count = 2 + pack_bytes(prefix_bytes, &mut request.words[2..])?;
    request.words[0] = index as u64;
    request.words[1] = prefix_bytes.len() as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(storage_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != StorageTag::ListReply as u32 || response.word_count < 3 {
        return Err(Error::InvalidArgument);
    }

    let status = match response.words[0] as u32 {
        x if x == StorageStatus::Ok as u32 => StorageStatus::Ok,
        x if x == StorageStatus::End as u32 => StorageStatus::End,
        x if x == StorageStatus::Busy as u32 => StorageStatus::Busy,
        x if x == StorageStatus::InvalidPath as u32 => StorageStatus::InvalidPath,
        _ => return Err(Error::InvalidArgument),
    };
    if status == StorageStatus::End {
        return Ok(None);
    }

    let path_len = response.words[2] as usize;
    unpack_bytes(&response.words[3..response.word_count as usize], path_len, path_buffer)?;
    Ok(Some((status, path_len)))
}

pub fn storage_read_all(
    blob_handle: Handle,
    buffer: &mut [u8],
    expected_len: usize,
) -> Result<usize> {
    if expected_len > buffer.len() {
        return Err(Error::BufferTooSmall);
    }

    let mut offset = 0usize;
    while offset < expected_len {
        let read = storage_read(blob_handle, offset, &mut buffer[offset..expected_len])?;
        if read == 0 {
            break;
        }
        offset += read;
    }
    Ok(offset)
}

pub fn storage_list_directory(
    storage_handle: Handle,
    prefix: &str,
    cursor: usize,
    path_buffer: &mut [u8],
) -> Result<Option<(usize, StorageEntryKind, usize)>> {
    let prefix_bytes = prefix.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(2)) * 8;
    if prefix_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(StorageTag::DirectoryListRequest as u32);
    request.word_count = 2 + pack_bytes(prefix_bytes, &mut request.words[2..])?;
    request.words[0] = cursor as u64;
    request.words[1] = prefix_bytes.len() as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(storage_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != StorageTag::DirectoryListReply as u32 || response.word_count < 4 {
        return Err(Error::InvalidArgument);
    }

    let status = match response.words[0] as u32 {
        x if x == StorageStatus::Ok as u32 => StorageStatus::Ok,
        x if x == StorageStatus::End as u32 => StorageStatus::End,
        x if x == StorageStatus::Busy as u32 => StorageStatus::Busy,
        x if x == StorageStatus::InvalidPath as u32 => StorageStatus::InvalidPath,
        _ => return Err(Error::InvalidArgument),
    };
    if status == StorageStatus::End {
        return Ok(None);
    }

    let next_cursor = response.words[1] as usize;
    let entry_kind = match response.words[2] as u32 {
        x if x == StorageEntryKind::File as u32 => StorageEntryKind::File,
        x if x == StorageEntryKind::Directory as u32 => StorageEntryKind::Directory,
        _ => return Err(Error::InvalidArgument),
    };
    let path_len = response.words[3] as usize;
    unpack_bytes(&response.words[4..response.word_count as usize], path_len, path_buffer)?;
    Ok(Some((next_cursor, entry_kind, path_len)))
}

pub fn storage_open_directory(
    storage_handle: Handle,
    path: &str,
    writable: bool,
) -> Result<Handle> {
    let path_bytes = path.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(2)) * 8;
    if path_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(StorageTag::DirectoryOpenRequest as u32);
    request.word_count = 2 + pack_bytes(path_bytes, &mut request.words[2..])?;
    request.words[0] = path_bytes.len() as u64;
    request.words[1] = u64::from(writable);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(storage_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != StorageTag::DirectoryOpenReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }

    match response.words[0] as u32 {
        x if x == StorageStatus::Ok as u32 && response.handle_count > 0 => Ok(response.handles[0]),
        x if x == StorageStatus::NotFound as u32 => Err(Error::NotFound),
        x if x == StorageStatus::Denied as u32 => Err(Error::PermissionDenied),
        x if x == StorageStatus::Busy as u32 => Err(Error::Busy),
        x if x == StorageStatus::InvalidPath as u32 => Err(Error::InvalidArgument),
        x if x == StorageStatus::NotDirectory as u32 => Err(Error::InvalidArgument),
        _ => Err(Error::InvalidArgument),
    }
}

pub fn storage_directory_create(
    directory_handle: Handle,
    name: &str,
    kind: StorageEntryKind,
) -> Result<()> {
    let name_bytes = name.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(2)) * 8;
    if name_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(StorageTag::DirectoryCreateRequest as u32);
    request.word_count = 2 + pack_bytes(name_bytes, &mut request.words[2..])?;
    request.words[0] = kind as u32 as u64;
    request.words[1] = name_bytes.len() as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(directory_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != StorageTag::DirectoryCreateReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }

    match response.words[0] as u32 {
        x if x == StorageStatus::Ok as u32 => Ok(()),
        x if x == StorageStatus::Denied as u32 => Err(Error::PermissionDenied),
        x if x == StorageStatus::AlreadyExists as u32 => Err(Error::Busy),
        x if x == StorageStatus::InvalidPath as u32 => Err(Error::InvalidArgument),
        x if x == StorageStatus::NotDirectory as u32 => Err(Error::InvalidArgument),
        _ => Err(Error::InvalidArgument),
    }
}

pub fn storage_directory_remove(directory_handle: Handle, name: &str) -> Result<()> {
    let name_bytes = name.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(1)) * 8;
    if name_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(StorageTag::DirectoryRemoveRequest as u32);
    request.word_count = 1 + pack_bytes(name_bytes, &mut request.words[1..])?;
    request.words[0] = name_bytes.len() as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(directory_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != StorageTag::DirectoryRemoveReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }

    match response.words[0] as u32 {
        x if x == StorageStatus::Ok as u32 => Ok(()),
        x if x == StorageStatus::NotFound as u32 => Err(Error::NotFound),
        x if x == StorageStatus::Denied as u32 => Err(Error::PermissionDenied),
        x if x == StorageStatus::Busy as u32 => Err(Error::Busy),
        x if x == StorageStatus::InvalidPath as u32 => Err(Error::InvalidArgument),
        _ => Err(Error::InvalidArgument),
    }
}

pub fn storage_directory_open_file(
    directory_handle: Handle,
    name: &str,
    create: bool,
    writable: bool,
) -> Result<(Handle, usize)> {
    let name_bytes = name.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(3)) * 8;
    if name_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(StorageTag::DirectoryOpenFileRequest as u32);
    request.word_count = 3 + pack_bytes(name_bytes, &mut request.words[3..])?;
    request.words[0] = name_bytes.len() as u64;
    request.words[1] = u64::from(create);
    request.words[2] = u64::from(writable);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(directory_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != StorageTag::DirectoryOpenFileReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }

    match response.words[0] as u32 {
        x if x == StorageStatus::Ok as u32 && response.handle_count > 0 => {
            Ok((response.handles[0], response.words[1] as usize))
        }
        x if x == StorageStatus::NotFound as u32 => Err(Error::NotFound),
        x if x == StorageStatus::Denied as u32 => Err(Error::PermissionDenied),
        x if x == StorageStatus::Busy as u32 => Err(Error::Busy),
        x if x == StorageStatus::InvalidPath as u32 => Err(Error::InvalidArgument),
        x if x == StorageStatus::NotDirectory as u32 => Err(Error::InvalidArgument),
        _ => Err(Error::InvalidArgument),
    }
}

pub fn storage_write(blob_handle: Handle, offset: usize, total_len: usize, bytes: &[u8]) -> Result<usize> {
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(3)) * 8;
    if bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(StorageTag::WriteRequest as u32);
    request.word_count = 3 + pack_bytes(bytes, &mut request.words[3..])?;
    request.words[0] = offset as u64;
    request.words[1] = total_len as u64;
    request.words[2] = bytes.len() as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(blob_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != StorageTag::WriteReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }

    match response.words[0] as u32 {
        x if x == StorageStatus::Ok as u32 => Ok(response.words[1] as usize),
        x if x == StorageStatus::Denied as u32 => Err(Error::PermissionDenied),
        x if x == StorageStatus::Busy as u32 => Err(Error::Busy),
        x if x == StorageStatus::InvalidOffset as u32 => Err(Error::InvalidArgument),
        _ => Err(Error::InvalidArgument),
    }
}

pub fn storage_directory_read(
    directory_handle: Handle,
    cursor: usize,
    path_buffer: &mut [u8],
) -> Result<Option<(usize, StorageEntryKind, usize)>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(StorageTag::DirectoryReadRequest as u32);
    request.word_count = 1;
    request.words[0] = cursor as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(directory_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != StorageTag::DirectoryReadReply as u32 || response.word_count < 4 {
        return Err(Error::InvalidArgument);
    }

    let status = match response.words[0] as u32 {
        x if x == StorageStatus::Ok as u32 => StorageStatus::Ok,
        x if x == StorageStatus::End as u32 => StorageStatus::End,
        x if x == StorageStatus::Busy as u32 => StorageStatus::Busy,
        _ => return Err(Error::InvalidArgument),
    };
    if status == StorageStatus::End {
        return Ok(None);
    }

    let next_cursor = response.words[1] as usize;
    let entry_kind = match response.words[2] as u32 {
        x if x == StorageEntryKind::File as u32 => StorageEntryKind::File,
        x if x == StorageEntryKind::Directory as u32 => StorageEntryKind::Directory,
        _ => return Err(Error::InvalidArgument),
    };
    let path_len = response.words[3] as usize;
    unpack_bytes(
        &response.words[4..response.word_count as usize],
        path_len,
        path_buffer,
    )?;
    Ok(Some((next_cursor, entry_kind, path_len)))
}

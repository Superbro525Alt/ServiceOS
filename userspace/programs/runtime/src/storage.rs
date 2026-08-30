use crate::{
    Error, Handle, IPC_MAX_WORDS, RawMessage, Result, StorageEntryKind, StorageMountKind,
    StorageStatus, StorageTag, channel_call, channel_send, handle_close, pack_bytes, unpack_bytes,
};

#[derive(Clone, Copy)]
pub struct StorageMountInfo {
    pub next_cursor: usize,
    pub kind: StorageMountKind,
    pub writable: bool,
    pub persistent: bool,
    pub path_len: usize,
}

pub const STORAGE_SEARCH_REQUEST_TAG: u32 = 0x521;
pub const STORAGE_SEARCH_REPLY_TAG: u32 = 0x522;
pub const STORAGE_SEARCH_QUERY_TOKEN_MAX: usize = 3;
pub const STORAGE_SEARCH_QUERY_TOKEN_LEN_MAX: usize = 24;
pub const STORAGE_SEARCH_QUERY_BYTES_MAX: usize =
    STORAGE_SEARCH_QUERY_TOKEN_MAX * (STORAGE_SEARCH_QUERY_TOKEN_LEN_MAX + 1) - 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StorageSearchQuery {
    bytes: [u8; STORAGE_SEARCH_QUERY_BYTES_MAX],
    len: usize,
}

impl StorageSearchQuery {
    pub fn new(query: &str) -> Option<Self> {
        Self::from_bytes(query.as_bytes())
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let mut query = Self {
            bytes: [0; STORAGE_SEARCH_QUERY_BYTES_MAX],
            len: 0,
        };
        let mut token_count = 0usize;
        let mut token_len = 0usize;
        let mut in_token = false;

        for byte in bytes.iter().copied() {
            if byte == b' ' || byte == 0 {
                if in_token {
                    token_count += 1;
                    token_len = 0;
                    in_token = false;
                    if token_count >= STORAGE_SEARCH_QUERY_TOKEN_MAX {
                        break;
                    }
                }
                continue;
            }
            if token_count >= STORAGE_SEARCH_QUERY_TOKEN_MAX {
                break;
            }
            if token_len == 0 && query.len > 0 {
                query.bytes[query.len] = b' ';
                query.len += 1;
            }
            if token_len < STORAGE_SEARCH_QUERY_TOKEN_LEN_MAX {
                query.bytes[query.len] = byte;
                query.len += 1;
                token_len += 1;
            }
            in_token = true;
        }

        if query.len == 0 { None } else { Some(query) }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    pub fn len(&self) -> usize {
        self.len
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StorageSearchHit<const PATH_MAX: usize> {
    pub next_cursor: usize,
    pub kind: StorageEntryKind,
    pub path: [u8; PATH_MAX],
    pub path_len: usize,
}

impl<const PATH_MAX: usize> StorageSearchHit<PATH_MAX> {
    pub fn path_as_bytes(&self) -> &[u8] {
        &self.path[..self.path_len.min(self.path.len())]
    }

    pub fn path_as_str(&self) -> &str {
        core::str::from_utf8(self.path_as_bytes()).unwrap_or("")
    }
}

pub fn storage_search_request(
    cursor: usize,
    scope: &[u8],
    query: &StorageSearchQuery,
) -> Result<RawMessage> {
    let scope_words = scope.len().div_ceil(8);
    let query_words = query.len().div_ceil(8);
    if 6 + scope_words + query_words > IPC_MAX_WORDS {
        return Err(Error::BufferTooSmall);
    }

    let mut request = RawMessage::empty(STORAGE_SEARCH_REQUEST_TAG);
    request.word_count = 6;
    request.words[0] = cursor as u64;
    request.words[1] = scope.len() as u64;
    request.words[2] = 0;
    request.words[3] = 0;
    request.words[4] = 0;
    request.words[5] = query.len() as u64;
    request.word_count += pack_bytes(scope, &mut request.words[6..])?;
    let query_offset = request.word_count as usize;
    request.word_count += pack_bytes(query.as_bytes(), &mut request.words[query_offset..])?;
    Ok(request)
}

pub fn storage_search_parse_reply<const PATH_MAX: usize>(
    reply: &RawMessage,
) -> Result<Option<StorageSearchHit<PATH_MAX>>> {
    if reply.tag != STORAGE_SEARCH_REPLY_TAG || reply.word_count < 4 {
        return Err(Error::InvalidArgument);
    }

    match reply.words[0] as u32 {
        x if x == StorageStatus::End as u32 => return Ok(None),
        x if x == StorageStatus::Ok as u32 => {}
        _ => return Err(Error::InvalidArgument),
    }

    let kind = match reply.words[2] as u32 {
        x if x == StorageEntryKind::File as u32 => StorageEntryKind::File,
        x if x == StorageEntryKind::Directory as u32 => StorageEntryKind::Directory,
        _ => return Err(Error::InvalidArgument),
    };
    let path_len = reply.words[3] as usize;
    if path_len == 0 || path_len > PATH_MAX {
        return Err(Error::InvalidArgument);
    }

    let mut hit = StorageSearchHit {
        next_cursor: reply.words[1] as usize,
        kind,
        path: [0; PATH_MAX],
        path_len,
    };
    unpack_bytes(
        &reply.words[4..reply.word_count as usize],
        path_len,
        &mut hit.path,
    )?;
    Ok(Some(hit))
}

pub fn storage_search<const PATH_MAX: usize>(
    storage_handle: Handle,
    cursor: usize,
    scope: &[u8],
    query: &StorageSearchQuery,
) -> Result<Option<StorageSearchHit<PATH_MAX>>> {
    let mut request = storage_search_request(cursor, scope, query)?;
    let response = channel_call(storage_handle, &mut request)?;
    storage_search_parse_reply(&response)
}

pub fn storage_open(storage_handle: Handle, path: &str) -> Result<(Handle, usize)> {
    let path_bytes = path.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(1)) * 8;
    if path_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let mut request = RawMessage::empty(StorageTag::OpenRequest as u32);
    request.word_count = 1 + pack_bytes(path_bytes, &mut request.words[1..])?;
    request.words[0] = path_bytes.len() as u64;
    let response = channel_call(storage_handle, &mut request)?;
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
    let mut request = RawMessage::empty(StorageTag::ReadRequest as u32);
    request.word_count = 2;
    request.words[0] = offset as u64;
    request.words[1] = requested as u64;
    let response = channel_call(blob_handle, &mut request)?;
    if response.tag != StorageTag::ReadReply as u32 || response.word_count < 3 {
        return Err(Error::InvalidArgument);
    }

    match response.words[0] as u32 {
        x if x == StorageStatus::Ok as u32 => {
            let byte_len = response.words[2] as usize;
            if byte_len > requested || byte_len > buffer.len() {
                return Err(Error::BufferTooSmall);
            }
            unpack_bytes(
                &response.words[3..response.word_count as usize],
                byte_len,
                buffer,
            )?;
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

    let mut request = RawMessage::empty(StorageTag::ListRequest as u32);
    request.word_count = 2 + pack_bytes(prefix_bytes, &mut request.words[2..])?;
    request.words[0] = index as u64;
    request.words[1] = prefix_bytes.len() as u64;
    let response = channel_call(storage_handle, &mut request)?;
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
    unpack_bytes(
        &response.words[3..response.word_count as usize],
        path_len,
        path_buffer,
    )?;
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

    let mut request = RawMessage::empty(StorageTag::DirectoryListRequest as u32);
    request.word_count = 2 + pack_bytes(prefix_bytes, &mut request.words[2..])?;
    request.words[0] = cursor as u64;
    request.words[1] = prefix_bytes.len() as u64;
    let response = channel_call(storage_handle, &mut request)?;
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
    unpack_bytes(
        &response.words[4..response.word_count as usize],
        path_len,
        path_buffer,
    )?;
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

    let mut request = RawMessage::empty(StorageTag::DirectoryOpenRequest as u32);
    request.word_count = 2 + pack_bytes(path_bytes, &mut request.words[2..])?;
    request.words[0] = path_bytes.len() as u64;
    request.words[1] = u64::from(writable);
    let response = channel_call(storage_handle, &mut request)?;
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

    let mut request = RawMessage::empty(StorageTag::DirectoryCreateRequest as u32);
    request.word_count = 2 + pack_bytes(name_bytes, &mut request.words[2..])?;
    request.words[0] = kind as u32 as u64;
    request.words[1] = name_bytes.len() as u64;
    let response = channel_call(directory_handle, &mut request)?;
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

    let mut request = RawMessage::empty(StorageTag::DirectoryRemoveRequest as u32);
    request.word_count = 1 + pack_bytes(name_bytes, &mut request.words[1..])?;
    request.words[0] = name_bytes.len() as u64;
    let response = channel_call(directory_handle, &mut request)?;
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

    let mut request = RawMessage::empty(StorageTag::DirectoryOpenFileRequest as u32);
    request.word_count = 3 + pack_bytes(name_bytes, &mut request.words[3..])?;
    request.words[0] = name_bytes.len() as u64;
    request.words[1] = u64::from(create);
    request.words[2] = u64::from(writable);
    let response = channel_call(directory_handle, &mut request)?;
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

pub fn storage_write(
    blob_handle: Handle,
    offset: usize,
    total_len: usize,
    bytes: &[u8],
) -> Result<usize> {
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(3)) * 8;
    if bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let mut request = RawMessage::empty(StorageTag::WriteRequest as u32);
    request.word_count = 3 + pack_bytes(bytes, &mut request.words[3..])?;
    request.words[0] = offset as u64;
    request.words[1] = total_len as u64;
    request.words[2] = bytes.len() as u64;
    let response = channel_call(blob_handle, &mut request)?;
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
    let mut request = RawMessage::empty(StorageTag::DirectoryReadRequest as u32);
    request.word_count = 1;
    request.words[0] = cursor as u64;
    let response = channel_call(directory_handle, &mut request)?;
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

pub fn storage_mount_list(
    storage_handle: Handle,
    cursor: usize,
    path_buffer: &mut [u8],
) -> Result<Option<StorageMountInfo>> {
    let mut request = RawMessage::empty(StorageTag::MountListRequest as u32);
    request.word_count = 1;
    request.words[0] = cursor as u64;
    let response = channel_call(storage_handle, &mut request)?;
    if response.tag != StorageTag::MountListReply as u32 || response.word_count < 5 {
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

    let kind = match response.words[2] as u32 {
        x if x == StorageMountKind::Boot as u32 => StorageMountKind::Boot,
        x if x == StorageMountKind::Persistent as u32 => StorageMountKind::Persistent,
        x if x == StorageMountKind::Ephemeral as u32 => StorageMountKind::Ephemeral,
        _ => return Err(Error::InvalidArgument),
    };
    let path_len = response.words[4] as usize;
    unpack_bytes(
        &response.words[5..response.word_count as usize],
        path_len,
        path_buffer,
    )?;
    Ok(Some(StorageMountInfo {
        next_cursor: response.words[1] as usize,
        kind,
        writable: response.words[3] & 1 != 0,
        persistent: response.words[3] & 2 != 0,
        path_len,
    }))
}

fn storage_directory_traverse(
    directory_handle: Handle,
    path: &str,
    directory: bool,
    writable: bool,
) -> Result<(Handle, usize)> {
    let path_bytes = path.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(3)) * 8;
    if path_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let mut request = RawMessage::empty(StorageTag::DirectoryTraverseRequest as u32);
    request.word_count = 3 + pack_bytes(path_bytes, &mut request.words[3..])?;
    request.words[0] = path_bytes.len() as u64;
    request.words[1] = u64::from(directory);
    request.words[2] = u64::from(writable);
    let response = channel_call(directory_handle, &mut request)?;
    if response.tag != StorageTag::DirectoryTraverseReply as u32 || response.word_count < 3 {
        return Err(Error::InvalidArgument);
    }

    match response.words[0] as u32 {
        x if x == StorageStatus::Ok as u32 && response.handle_count > 0 => {
            Ok((response.handles[0], response.words[2] as usize))
        }
        x if x == StorageStatus::NotFound as u32 => Err(Error::NotFound),
        x if x == StorageStatus::Denied as u32 => Err(Error::PermissionDenied),
        x if x == StorageStatus::Busy as u32 => Err(Error::Busy),
        x if x == StorageStatus::InvalidPath as u32 => Err(Error::InvalidArgument),
        x if x == StorageStatus::NotDirectory as u32 => Err(Error::InvalidArgument),
        _ => Err(Error::InvalidArgument),
    }
}

pub fn storage_directory_open_path(
    directory_handle: Handle,
    path: &str,
    writable: bool,
) -> Result<Handle> {
    let (handle, _) = storage_directory_traverse(directory_handle, path, true, writable)?;
    Ok(handle)
}

pub fn storage_directory_open_path_file(
    directory_handle: Handle,
    path: &str,
    writable: bool,
) -> Result<(Handle, usize)> {
    storage_directory_traverse(directory_handle, path, false, writable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_search_query_compacts_and_caps_tokens() {
        let query = StorageSearchQuery::from_bytes(b"  alpha   beta gamma delta  ")
            .expect("query should keep first three tokens");
        assert_eq!(query.as_bytes(), b"alpha beta gamma");

        let query = StorageSearchQuery::from_bytes(b"abcdefghijklmnopqrstuvwxYZ123456  second")
            .expect("query should keep truncated first token");
        assert_eq!(query.as_bytes(), b"abcdefghijklmnopqrstuvwx second");

        assert!(StorageSearchQuery::from_bytes(b"   ").is_none());
        assert!(StorageSearchQuery::from_bytes(&[0, b' ', 0]).is_none());
    }

    #[test]
    fn storage_search_request_packs_scope_and_query() {
        let query = StorageSearchQuery::new("boot notes").expect("query should build");
        let request = storage_search_request(3, b"state/docs/", &query).expect("request builds");
        assert_eq!(request.tag, STORAGE_SEARCH_REQUEST_TAG);
        assert_eq!(request.word_count, 10);
        assert_eq!(request.words[0], 3);
        assert_eq!(request.words[1], 11);
        assert_eq!(request.words[2], 0);
        assert_eq!(request.words[3], 0);
        assert_eq!(request.words[4], 0);
        assert_eq!(request.words[5], 10);

        let mut scope = [0u8; 16];
        unpack_bytes(&request.words[6..8], 11, &mut scope).expect("scope decodes");
        assert_eq!(&scope[..11], b"state/docs/");

        let mut query_buf = [0u8; STORAGE_SEARCH_QUERY_BYTES_MAX];
        unpack_bytes(&request.words[8..10], 10, &mut query_buf).expect("query decodes");
        assert_eq!(&query_buf[..10], b"boot notes");
    }

    #[test]
    fn storage_search_parse_reply_handles_ok_and_end() {
        let mut payload = [0u64; 4];
        pack_bytes(b"boot/notes.txt", &mut payload).expect("payload packs");

        let mut reply = RawMessage::empty(STORAGE_SEARCH_REPLY_TAG);
        reply.word_count = 6;
        reply.words[0] = StorageStatus::Ok as u32 as u64;
        reply.words[1] = 4;
        reply.words[2] = StorageEntryKind::File as u32 as u64;
        reply.words[3] = 14;
        reply.words[4..8].copy_from_slice(&payload);

        let parsed = storage_search_parse_reply::<32>(&reply).expect("reply should parse cleanly");
        let parsed = parsed.expect("reply should produce a hit");
        assert_eq!(parsed.next_cursor, 4);
        assert_eq!(parsed.kind, StorageEntryKind::File);
        assert_eq!(parsed.path_as_str(), "boot/notes.txt");

        reply.words[0] = StorageStatus::End as u32 as u64;
        assert_eq!(
            storage_search_parse_reply::<32>(&reply).expect("end should parse"),
            None
        );
    }

    #[test]
    fn storage_search_parse_reply_rejects_malformed_shapes() {
        let mut reply = RawMessage::empty(STORAGE_SEARCH_REPLY_TAG);
        reply.word_count = 4;
        reply.words[0] = StorageStatus::Busy as u32 as u64;
        assert_eq!(
            storage_search_parse_reply::<16>(&reply),
            Err(Error::InvalidArgument)
        );

        reply.words[0] = StorageStatus::Ok as u32 as u64;
        reply.words[2] = 99;
        reply.words[3] = 4;
        assert_eq!(
            storage_search_parse_reply::<16>(&reply),
            Err(Error::InvalidArgument)
        );

        reply.tag = 0x999;
        assert_eq!(
            storage_search_parse_reply::<16>(&reply),
            Err(Error::InvalidArgument)
        );

        reply.tag = STORAGE_SEARCH_REPLY_TAG;
        reply.word_count = 3;
        assert_eq!(
            storage_search_parse_reply::<16>(&reply),
            Err(Error::InvalidArgument)
        );
    }
}

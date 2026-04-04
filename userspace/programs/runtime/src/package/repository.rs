use crate::{
    package::common::{
        package_channel_from_word, package_repo_sync_state_from_word, package_ring_from_word,
        package_trust_mode_from_word, send_request,
    },
    pack_bytes, package_status_error, package_status_from_word, unpack_bytes, Error, Handle,
    PackageChannel, PackageRepositoryInfo,
    PackageRepositorySyncInfo, PackageRepositoryTrustMode, PackageRing, PackageStatus, PackageTag,
    RawMessage, Result, IPC_MAX_WORDS,
};

pub fn package_repository_list(
    package_handle: Handle,
    index: usize,
    name: &mut [u8],
    url: &mut [u8],
) -> Result<Option<PackageRepositoryInfo>> {
    let mut request = RawMessage::empty(PackageTag::RepositoryListRequest as u32);
    request.word_count = 1;
    request.words[0] = index as u64;
    let response = send_request(package_handle, request)?;
    if response.tag != PackageTag::RepositoryListReply as u32 || response.word_count < 8 {
        return Err(Error::InvalidArgument);
    }

    let status = package_status_from_word(response.words[0]);
    if status == PackageStatus::End {
        return Ok(None);
    }
    if status != PackageStatus::Ok {
        return Err(package_status_error(status));
    }

    let name_len = response.words[4] as usize;
    let url_len = response.words[5] as usize;
    let total_bytes = name_len + url_len;
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
    name[..name_len].copy_from_slice(&combined[..name_len]);
    url[..url_len].copy_from_slice(&combined[name_len..name_len + url_len]);

    let flags = response.words[3] as u32;
    Ok(Some(PackageRepositoryInfo {
        repo_index: response.words[1] as u32,
        package_count: response.words[2] as u32,
        trust_mode: package_trust_mode_from_word((flags & 0xff) as u64),
        sync_state: package_repo_sync_state_from_word(((flags >> 8) & 0xff) as u64),
        channel: package_channel_from_word(((flags >> 16) & 0xff) as u64),
        ring: package_ring_from_word(((flags >> 24) & 0x3f) as u64),
        enabled: flags & (1 << 30) != 0,
        pinned_digest: response.words[6],
        last_digest: response.words[7],
        name_len,
        url_len,
    }))
}

#[allow(clippy::too_many_arguments)]
pub fn package_repository_add(
    package_handle: Handle,
    name: &str,
    url: &str,
    trust_mode: PackageRepositoryTrustMode,
    channel: PackageChannel,
    ring: PackageRing,
    enabled: bool,
    pinned_digest: u64,
) -> Result<()> {
    let name_bytes = name.as_bytes();
    let url_bytes = url.as_bytes();
    let total_bytes = name_bytes.len() + url_bytes.len();
    if total_bytes > (IPC_MAX_WORDS.saturating_sub(4)) * 8 {
        return Err(Error::BufferTooSmall);
    }

    let mut request = RawMessage::empty(PackageTag::RepositoryAddRequest as u32);
    request.words[0] = (trust_mode as u64)
        | ((channel as u64) << 16)
        | ((ring as u64) << 32)
        | ((u64::from(enabled)) << 48);
    request.words[1] = pinned_digest;
    request.words[2] = name_bytes.len() as u64;
    request.words[3] = url_bytes.len() as u64;
    let mut combined = [0u8; (IPC_MAX_WORDS - 4) * 8];
    combined[..name_bytes.len()].copy_from_slice(name_bytes);
    combined[name_bytes.len()..name_bytes.len() + url_bytes.len()].copy_from_slice(url_bytes);
    request.word_count = 4 + pack_bytes(&combined[..total_bytes], &mut request.words[4..])?;

    let response = send_request(package_handle, request)?;
    if response.tag != PackageTag::RepositoryAddReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }

    match package_status_from_word(response.words[0]) {
        PackageStatus::Ok => Ok(()),
        status => Err(package_status_error(status)),
    }
}

pub fn package_repository_sync(
    package_handle: Handle,
    repo_index: Option<usize>,
) -> Result<PackageRepositorySyncInfo> {
    let mut request = RawMessage::empty(PackageTag::RepositorySyncRequest as u32);
    request.word_count = 1;
    request.words[0] = repo_index.unwrap_or(usize::MAX) as u64;

    let response = send_request(package_handle, request)?;
    if response.tag != PackageTag::RepositorySyncReply as u32 || response.word_count < 3 {
        return Err(Error::InvalidArgument);
    }

    let status = package_status_from_word(response.words[0]);
    if !matches!(status, PackageStatus::Ok | PackageStatus::Busy | PackageStatus::Offline) {
        return Err(package_status_error(status));
    }

    Ok(PackageRepositorySyncInfo {
        synced: response.words[1] as u32,
        failed: response.words[2] as u32,
    })
}

use crate::{
    channel_create, channel_receive_blocking, channel_send, handle_close, pack_bytes,
    package_status_error, package_status_from_word, rights, Error, Handle, PackageChannel,
    PackageRepositorySyncState, PackageRepositoryTrustMode, PackageRing, PackageStatus, PackageTag,
    PackageTrustState, RawMessage, Result, ServiceId, IPC_MAX_WORDS,
};

pub(crate) fn send_request(
    package_handle: Handle,
    mut request: RawMessage,
) -> Result<RawMessage> {
    let reply = channel_create()?;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(package_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    Ok(response)
}

pub(crate) fn package_mutation(
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

    let mut request = RawMessage::empty(request_tag as u32);
    request.word_count = 2 + pack_bytes(version_bytes, &mut request.words[2..])?;
    request.words[0] = service_id as u32 as u64;
    request.words[1] = version_bytes.len() as u64;

    let response = send_request(package_handle, request)?;
    if response.tag != reply_tag as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }

    match package_status_from_word(response.words[0]) {
        PackageStatus::Ok => Ok(()),
        status => Err(package_status_error(status)),
    }
}

pub(crate) fn package_trust_mode_from_word(value: u64) -> PackageRepositoryTrustMode {
    match value as u32 {
        x if x == PackageRepositoryTrustMode::Boot as u32 => PackageRepositoryTrustMode::Boot,
        x if x == PackageRepositoryTrustMode::PinnedDigest as u32 => {
            PackageRepositoryTrustMode::PinnedDigest
        }
        _ => PackageRepositoryTrustMode::Unsigned,
    }
}

pub(crate) fn package_repo_sync_state_from_word(value: u64) -> PackageRepositorySyncState {
    match value as u32 {
        x if x == PackageRepositorySyncState::Ready as u32 => PackageRepositorySyncState::Ready,
        x if x == PackageRepositorySyncState::Offline as u32 => {
            PackageRepositorySyncState::Offline
        }
        x if x == PackageRepositorySyncState::Failed as u32 => {
            PackageRepositorySyncState::Failed
        }
        _ => PackageRepositorySyncState::Idle,
    }
}

pub(crate) fn package_trust_state_from_word(value: u64) -> PackageTrustState {
    match value as u32 {
        x if x == PackageTrustState::BootTrusted as u32 => PackageTrustState::BootTrusted,
        x if x == PackageTrustState::DigestPinned as u32 => PackageTrustState::DigestPinned,
        x if x == PackageTrustState::VerificationFailed as u32 => {
            PackageTrustState::VerificationFailed
        }
        _ => PackageTrustState::Unverified,
    }
}

pub(crate) fn package_channel_from_word(value: u64) -> PackageChannel {
    match value as u32 {
        x if x == PackageChannel::Beta as u32 => PackageChannel::Beta,
        x if x == PackageChannel::Canary as u32 => PackageChannel::Canary,
        _ => PackageChannel::Stable,
    }
}

pub(crate) fn package_ring_from_word(value: u64) -> PackageRing {
    match value as u32 {
        x if x == PackageRing::Preview as u32 => PackageRing::Preview,
        x if x == PackageRing::Testing as u32 => PackageRing::Testing,
        _ => PackageRing::Production,
    }
}

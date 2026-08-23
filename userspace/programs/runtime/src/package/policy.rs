use crate::{
    Error, Handle, IPC_MAX_WORDS, PackageMaintenanceAction, PackageMaintenanceInfo,
    PackagePolicyInfo, PackageStatus, PackageTag, RawMessage, Result, ServiceId, pack_bytes,
    package::common::{
        package_channel_from_word, package_mutation, package_ring_from_word, send_request,
    },
    package_status_error, package_status_from_word, unpack_bytes,
};

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

pub fn package_policy(
    package_handle: Handle,
    service_id: ServiceId,
    pinned_version: &mut [u8],
) -> Result<PackagePolicyInfo> {
    let mut request = RawMessage::empty(PackageTag::PolicyRequest as u32);
    request.word_count = 1;
    request.words[0] = service_id as u32 as u64;
    let response = send_request(package_handle, request)?;
    if response.tag != PackageTag::PolicyReply as u32 || response.word_count < 4 {
        return Err(Error::InvalidArgument);
    }

    let status = package_status_from_word(response.words[0]);
    if status != PackageStatus::Ok {
        return Err(package_status_error(status));
    }

    let pin_len = response.words[3] as usize;
    let total_words = pin_len.div_ceil(8);
    if response.word_count as usize != 4 + total_words {
        return Err(Error::InvalidArgument);
    }
    if pin_len > 0 {
        let mut combined = [0u8; IPC_MAX_WORDS * 8];
        unpack_bytes(
            &response.words[4..response.word_count as usize],
            pin_len,
            &mut combined,
        )?;
        pinned_version[..pin_len].copy_from_slice(&combined[..pin_len]);
    }

    Ok(PackagePolicyInfo {
        channel: package_channel_from_word(response.words[1]),
        ring: package_ring_from_word(response.words[2]),
        pinned_version_len: pin_len,
    })
}

pub fn package_policy_set(
    package_handle: Handle,
    service_id: ServiceId,
    channel: crate::PackageChannel,
    ring: crate::PackageRing,
    pinned_version: Option<&str>,
) -> Result<()> {
    let pin_bytes = pinned_version.unwrap_or("").as_bytes();
    if pin_bytes.len() > (IPC_MAX_WORDS.saturating_sub(4)) * 8 {
        return Err(Error::BufferTooSmall);
    }

    let mut request = RawMessage::empty(PackageTag::PolicySetRequest as u32);
    request.word_count = 4 + pack_bytes(pin_bytes, &mut request.words[4..])?;
    request.words[0] = service_id as u32 as u64;
    request.words[1] = channel as u32 as u64;
    request.words[2] = ring as u32 as u64;
    request.words[3] = pin_bytes.len() as u64;

    let response = send_request(package_handle, request)?;
    if response.tag != PackageTag::PolicySetReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }

    match package_status_from_word(response.words[0]) {
        PackageStatus::Ok => Ok(()),
        status => Err(package_status_error(status)),
    }
}

pub fn package_maintenance(
    package_handle: Handle,
    action: PackageMaintenanceAction,
) -> Result<PackageMaintenanceInfo> {
    let mut request = RawMessage::empty(PackageTag::MaintenanceRequest as u32);
    request.word_count = 1;
    request.words[0] = action as u32 as u64;
    let response = send_request(package_handle, request)?;
    if response.tag != PackageTag::MaintenanceReply as u32 || response.word_count < 3 {
        return Err(Error::InvalidArgument);
    }

    let status = package_status_from_word(response.words[0]);
    if status != PackageStatus::Ok {
        return Err(package_status_error(status));
    }

    Ok(PackageMaintenanceInfo {
        action,
        repaired_entries: response.words[1] as u32,
        garbage_collected_entries: response.words[2] as u32,
    })
}

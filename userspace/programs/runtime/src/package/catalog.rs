use crate::{
    Error, Handle, IPC_MAX_WORDS, PackageCatalogEntry, PackageInfo, PackageListEntry,
    PackageProvenanceInfo, PackageStatus, PackageTag, RawMessage, Result, ServiceId,
    package::common::send_request, package_status_error, package_status_from_word, unpack_bytes,
};

pub fn package_list(
    package_handle: Handle,
    index: usize,
    installed_version: &mut [u8],
    active_version: &mut [u8],
) -> Result<Option<PackageListEntry>> {
    let mut request = RawMessage::empty(PackageTag::ListRequest as u32);
    request.word_count = 1;
    request.words[0] = index as u64;
    let response = send_request(package_handle, request)?;
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
    let mut request = RawMessage::empty(PackageTag::InfoRequest as u32);
    request.word_count = 1;
    request.words[0] = service_id as u32 as u64;
    let response = send_request(package_handle, request)?;
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

pub fn package_history(
    package_handle: Handle,
    service_id: ServiceId,
    current_version: &mut [u8],
    previous_version: &mut [u8],
) -> Result<(usize, usize)> {
    let mut request = RawMessage::empty(PackageTag::HistoryRequest as u32);
    request.word_count = 1;
    request.words[0] = service_id as u32 as u64;
    let response = send_request(package_handle, request)?;
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

pub fn package_catalog(
    package_handle: Handle,
    index: usize,
    latest_version: &mut [u8],
    category: &mut [u8],
    summary: &mut [u8],
) -> Result<Option<PackageCatalogEntry>> {
    let mut request = RawMessage::empty(PackageTag::CatalogRequest as u32);
    request.word_count = 1;
    request.words[0] = index as u64;
    let response = send_request(package_handle, request)?;
    if response.tag != PackageTag::CatalogReply as u32 || response.word_count < 7 {
        return Err(Error::InvalidArgument);
    }

    let status = package_status_from_word(response.words[0]);
    if status == PackageStatus::End {
        return Ok(None);
    }
    if status != PackageStatus::Ok {
        return Err(package_status_error(status));
    }

    let latest_len = response.words[4] as usize;
    let category_len = response.words[5] as usize;
    let summary_len = response.words[6] as usize;
    let total_bytes = latest_len + category_len + summary_len;
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
    latest_version[..latest_len].copy_from_slice(&combined[..latest_len]);
    category[..category_len].copy_from_slice(&combined[latest_len..latest_len + category_len]);
    summary[..summary_len].copy_from_slice(
        &combined[latest_len + category_len..latest_len + category_len + summary_len],
    );

    Ok(Some(PackageCatalogEntry {
        service_id: crate::service_id_from_word(response.words[1]),
        repo_index: response.words[3] as u32,
        installed: response.words[2] & 1 != 0,
        active: response.words[2] & 2 != 0,
        rollback_available: response.words[2] & 4 != 0,
        category_len,
        summary_len,
        latest_version_len: latest_len,
    }))
}

pub fn package_provenance(
    package_handle: Handle,
    service_id: ServiceId,
    installed_version: &mut [u8],
    active_version: &mut [u8],
    rollback_version: &mut [u8],
    latest_version: &mut [u8],
    source: &mut [u8],
) -> Result<PackageProvenanceInfo> {
    let mut request = RawMessage::empty(PackageTag::ProvenanceRequest as u32);
    request.word_count = 1;
    request.words[0] = service_id as u32 as u64;
    let response = send_request(package_handle, request)?;
    if response.tag != PackageTag::ProvenanceReply as u32 || response.word_count < 8 {
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
    let source_len = response.words[7] as usize;
    let total_bytes = installed_len + active_len + rollback_len + latest_len + source_len;
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
    offset += latest_len;
    source[..source_len].copy_from_slice(&combined[offset..offset + source_len]);

    let flags = response.words[2] as u32;
    let package_flags = (flags >> 24) & 0xff;
    Ok(PackageProvenanceInfo {
        repo_index: response.words[1] as u32,
        trust_state: super::common::package_trust_state_from_word((flags & 0xff) as u64),
        channel: super::common::package_channel_from_word(((flags >> 8) & 0xff) as u64),
        ring: super::common::package_ring_from_word(((flags >> 16) & 0xff) as u64),
        installed: package_flags & 1 != 0,
        active: package_flags & 2 != 0,
        rollback_available: package_flags & 4 != 0,
        installed_version_len: installed_len,
        active_version_len: active_len,
        rollback_version_len: rollback_len,
        latest_version_len: latest_len,
        source_len,
    })
}

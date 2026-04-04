use crate::{
    channel_receive_blocking, channel_send, manager_availability_from_word,
    manager_lookup_policy_from_word, manager_phase_from_word, manager_startup_from_word,
    manager_status_from_word, pack_bytes, Error, Handle, IPC_MAX_HANDLES, IPC_MAX_WORDS,
    ManagerAction, ManagerGraphStatusInfo, ManagerLookupPolicy, ManagerServiceInfo,
    ManagerServiceLookupInfo, ManagerServiceStatusInfo, ManagerServiceTemplateInfo, ManagerStatus,
    ManagerTag, RawMessage, Result, ServiceId, ServiceImageId, rights, service_id_from_word,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StartupHandle {
    pub handle: Handle,
    pub rights: u64,
}

pub fn manager_list_services(
    bootstrap: Handle,
    services: &mut [ManagerServiceInfo],
) -> Result<usize> {
    let mut loaded = 0usize;
    let mut page = 0usize;

    loop {
        let mut request = RawMessage::empty(ManagerTag::ListServicesRequest as u32);
        request.word_count = 1;
        request.words[0] = page as u64;
        channel_send(bootstrap, &request)?;

        let mut response = RawMessage::empty(0);
        channel_receive_blocking(bootstrap, &mut response)?;
        if response.tag != ManagerTag::ListServicesReply as u32 || response.word_count < 2 {
            return Err(Error::InvalidArgument);
        }

        let count = response.words[0] as usize;
        let next_page = response.words[1] as usize;
        if loaded + count > services.len() || response.word_count < (2 + count * 2) as u32 {
            return Err(Error::BufferTooSmall);
        }

        for index in 0..count {
            services[loaded + index] = ManagerServiceInfo {
                service_id: service_id_from_word(response.words[2 + index * 2]),
                phase: manager_phase_from_word(response.words[3 + index * 2]),
                attempts: (response.words[3 + index * 2] >> 32) as u32,
            };
        }
        loaded += count;

        if next_page == usize::MAX {
            break;
        }
        page = next_page;
    }

    Ok(loaded)
}

pub fn manager_service_status(
    bootstrap: Handle,
    service_id: ServiceId,
) -> Result<ManagerServiceStatusInfo> {
    let mut request = RawMessage::empty(ManagerTag::ServiceStatusRequest as u32);
    request.word_count = 1;
    request.words[0] = service_id as u32 as u64;
    channel_send(bootstrap, &request)?;

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(bootstrap, &mut response)?;
    if response.tag != ManagerTag::ServiceStatusReply as u32 || response.word_count < 10 {
        return Err(Error::InvalidArgument);
    }

    Ok(ManagerServiceStatusInfo {
        status: manager_status_from_word(response.words[0]),
        phase: manager_phase_from_word(response.words[1]),
        attempts: response.words[2] as u32,
        last_exit: response.words[3],
        startup: manager_startup_from_word(response.words[4]),
        availability: manager_availability_from_word(response.words[5]),
        blocked_dependency: service_id_from_word(response.words[6]),
        last_start_tick: response.words[7],
        last_ready_tick: response.words[8],
        next_restart_tick: response.words[9],
    })
}

pub fn manager_service_template(
    bootstrap: Handle,
    service_id: ServiceId,
) -> Result<ManagerServiceTemplateInfo> {
    let mut request = RawMessage::empty(ManagerTag::ServiceTemplateRequest as u32);
    request.word_count = 1;
    request.words[0] = service_id as u32 as u64;
    channel_send(bootstrap, &request)?;

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(bootstrap, &mut response)?;
    if response.tag != ManagerTag::ServiceTemplateReply as u32 || response.word_count < 8 {
        return Err(Error::InvalidArgument);
    }
    if manager_status_from_word(response.words[0]) != ManagerStatus::Ok {
        return Err(Error::NotFound);
    }

    Ok(ManagerServiceTemplateInfo {
        startup: manager_startup_from_word(response.words[1]),
        availability: manager_availability_from_word(response.words[2]),
        ready_timeout_ticks: response.words[3] as u32,
        restart_limit: response.words[4] as u32,
        restart_backoff_ticks: response.words[5] as u32,
        grant_count: response.words[6] as u32,
        lookup_count: response.words[7] as u32,
    })
}

pub fn manager_graph_status(bootstrap: Handle) -> Result<ManagerGraphStatusInfo> {
    let request = RawMessage::empty(ManagerTag::ServiceGraphStatusRequest as u32);
    channel_send(bootstrap, &request)?;

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(bootstrap, &mut response)?;
    if response.tag != ManagerTag::ServiceGraphStatusReply as u32 || response.word_count < 4 {
        return Err(Error::InvalidArgument);
    }
    Ok(ManagerGraphStatusInfo {
        degraded_boot: response.words[0] != 0,
        blocked_services: response.words[1] as u32,
        degraded_services: response.words[2] as u32,
        service_count: response.words[3] as u32,
    })
}

pub fn manager_service_lookups(
    bootstrap: Handle,
    service_id: ServiceId,
    entries: &mut [ManagerServiceLookupInfo],
) -> Result<usize> {
    let mut loaded = 0usize;
    let mut page = 0usize;

    loop {
        let mut request = RawMessage::empty(ManagerTag::ServiceLookupListRequest as u32);
        request.word_count = 2;
        request.words[0] = service_id as u32 as u64;
        request.words[1] = page as u64;
        channel_send(bootstrap, &request)?;

        let mut response = RawMessage::empty(0);
        channel_receive_blocking(bootstrap, &mut response)?;
        if response.tag != ManagerTag::ServiceLookupListReply as u32 || response.word_count < 3 {
            return Err(Error::InvalidArgument);
        }
        match manager_status_from_word(response.words[0]) {
            ManagerStatus::Ok => {}
            ManagerStatus::NotFound => return Err(Error::NotFound),
            ManagerStatus::Denied => return Err(Error::PermissionDenied),
            ManagerStatus::Busy | ManagerStatus::Failed => return Err(Error::Busy),
        }

        let count = response.words[1] as usize;
        let next_page = response.words[2] as usize;
        if loaded + count > entries.len() || response.word_count < (3 + count * 3) as u32 {
            return Err(Error::BufferTooSmall);
        }
        for index in 0..count {
            entries[loaded + index] = ManagerServiceLookupInfo {
                target: service_id_from_word(response.words[3 + index * 3]),
                rights: response.words[4 + index * 3],
                policy: manager_lookup_policy_from_word(response.words[5 + index * 3]),
            };
        }
        loaded += count;
        if next_page == usize::MAX {
            break;
        }
        page = next_page;
    }

    Ok(loaded)
}

pub fn manager_set_service_lookup_policy(
    bootstrap: Handle,
    service_id: ServiceId,
    target: ServiceId,
    policy: ManagerLookupPolicy,
) -> Result<()> {
    let mut request = RawMessage::empty(ManagerTag::ServiceLookupPolicySetRequest as u32);
    request.word_count = 3;
    request.words[0] = service_id as u32 as u64;
    request.words[1] = target as u32 as u64;
    request.words[2] = policy as u32 as u64;
    channel_send(bootstrap, &request)?;

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(bootstrap, &mut response)?;
    if response.tag != ManagerTag::ServiceLookupPolicySetReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match manager_status_from_word(response.words[0]) {
        ManagerStatus::Ok => Ok(()),
        ManagerStatus::Denied => Err(Error::PermissionDenied),
        ManagerStatus::NotFound => Err(Error::NotFound),
        ManagerStatus::Busy | ManagerStatus::Failed => Err(Error::Busy),
    }
}

pub fn manager_restart_service(bootstrap: Handle, service_id: ServiceId) -> Result<()> {
    let mut request = RawMessage::empty(ManagerTag::ServiceActionRequest as u32);
    request.word_count = 2;
    request.words[0] = service_id as u32 as u64;
    request.words[1] = ManagerAction::Restart as u32 as u64;
    channel_send(bootstrap, &request)?;

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(bootstrap, &mut response)?;
    if response.tag != ManagerTag::ServiceActionReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match manager_status_from_word(response.words[0]) {
        ManagerStatus::Ok => Ok(()),
        ManagerStatus::Busy | ManagerStatus::Failed => Err(Error::Busy),
        ManagerStatus::NotFound => Err(Error::NotFound),
        ManagerStatus::Denied => Err(Error::PermissionDenied),
    }
}

pub fn manager_launch_program(
    bootstrap: Handle,
    image_id: ServiceImageId,
    io_handle: Option<Handle>,
) -> Result<Handle> {
    match io_handle {
        Some(handle) => manager_launch_program_with_payload(
            bootstrap,
            image_id,
            &[1],
            &[StartupHandle {
                handle,
                rights: rights::SEND | rights::RECEIVE | rights::DUPLICATE | rights::TRANSFER,
            }],
        ),
        None => manager_launch_program_with_payload(bootstrap, image_id, &[0], &[]),
    }
}

pub fn manager_launch_program_with_payload(
    bootstrap: Handle,
    image_id: ServiceImageId,
    startup_words: &[u64],
    startup_handles: &[StartupHandle],
) -> Result<Handle> {
    if startup_words.len() + 2 > IPC_MAX_WORDS || startup_handles.len() > IPC_MAX_HANDLES {
        return Err(Error::BufferTooSmall);
    }
    let mut request = RawMessage::empty(ManagerTag::LaunchRequest as u32);
    request.word_count = 2 + startup_words.len() as u32;
    request.words[0] = image_id as u32 as u64;
    request.words[1] = startup_words.len() as u64;
    for (index, word) in startup_words.iter().copied().enumerate() {
        request.words[2 + index] = word;
    }
    for (index, startup_handle) in startup_handles.iter().copied().enumerate() {
        request.handles[index] = startup_handle.handle;
        request.handle_rights[index] = startup_handle.rights;
    }
    request.handle_count = startup_handles.len() as u32;
    channel_send(bootstrap, &request)?;

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(bootstrap, &mut response)?;
    if response.tag != ManagerTag::LaunchReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match manager_status_from_word(response.words[0]) {
        ManagerStatus::Ok if response.handle_count > 0 => Ok(response.handles[0]),
        ManagerStatus::Busy | ManagerStatus::Failed => Err(Error::Busy),
        ManagerStatus::NotFound => Err(Error::NotFound),
        ManagerStatus::Denied => Err(Error::PermissionDenied),
        _ => Err(Error::InvalidArgument),
    }
}

pub fn manager_launch_image_with_payload(
    bootstrap: Handle,
    image_handle: Handle,
    startup_words: &[u64],
    startup_handles: &[StartupHandle],
) -> Result<Handle> {
    if startup_words.len() + 1 > IPC_MAX_WORDS || startup_handles.len() + 1 > IPC_MAX_HANDLES {
        return Err(Error::BufferTooSmall);
    }
    let mut request = RawMessage::empty(ManagerTag::LaunchImageRequest as u32);
    request.word_count = 1 + startup_words.len() as u32;
    request.words[0] = startup_words.len() as u64;
    for (index, word) in startup_words.iter().copied().enumerate() {
        request.words[1 + index] = word;
    }
    request.handle_count = 1 + startup_handles.len() as u32;
    request.handles[0] = image_handle;
    request.handle_rights[0] = rights::READ | rights::MAP | rights::TRANSFER | rights::DUPLICATE;
    for (index, startup_handle) in startup_handles.iter().copied().enumerate() {
        request.handles[index + 1] = startup_handle.handle;
        request.handle_rights[index + 1] = startup_handle.rights;
    }
    channel_send(bootstrap, &request)?;

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(bootstrap, &mut response)?;
    if response.tag != ManagerTag::LaunchImageReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match manager_status_from_word(response.words[0]) {
        ManagerStatus::Ok if response.handle_count > 0 => Ok(response.handles[0]),
        ManagerStatus::Busy | ManagerStatus::Failed => Err(Error::Busy),
        ManagerStatus::NotFound => Err(Error::NotFound),
        ManagerStatus::Denied => Err(Error::PermissionDenied),
        _ => Err(Error::InvalidArgument),
    }
}

pub fn manager_launch_stored_program_with_payload(
    bootstrap: Handle,
    path: &str,
    startup_words: &[u64],
    startup_handles: &[StartupHandle],
) -> Result<Handle> {
    let path_bytes = path.as_bytes();
    let packed_len = path_bytes.len().div_ceil(8);
    if startup_words.len() + 2 + packed_len > IPC_MAX_WORDS || startup_handles.len() > IPC_MAX_HANDLES {
        return Err(Error::BufferTooSmall);
    }

    let mut request = RawMessage::empty(ManagerTag::LaunchStoredImageRequest as u32);
    request.word_count = 2 + startup_words.len() as u32 + pack_bytes(path_bytes, &mut request.words[2 + startup_words.len()..])?;
    request.words[0] = startup_words.len() as u64;
    request.words[1] = path_bytes.len() as u64;
    for (index, word) in startup_words.iter().copied().enumerate() {
        request.words[2 + index] = word;
    }
    for (index, startup_handle) in startup_handles.iter().copied().enumerate() {
        request.handles[index] = startup_handle.handle;
        request.handle_rights[index] = startup_handle.rights;
    }
    request.handle_count = startup_handles.len() as u32;
    channel_send(bootstrap, &request)?;

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(bootstrap, &mut response)?;
    if response.tag != ManagerTag::LaunchStoredImageReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match manager_status_from_word(response.words[0]) {
        ManagerStatus::Ok if response.handle_count > 0 => Ok(response.handles[0]),
        ManagerStatus::Busy | ManagerStatus::Failed => Err(Error::Busy),
        ManagerStatus::NotFound => Err(Error::NotFound),
        ManagerStatus::Denied => Err(Error::PermissionDenied),
        _ => Err(Error::InvalidArgument),
    }
}

pub fn manager_activate_service(bootstrap: Handle, manifest_path: &str) -> Result<ServiceId> {
    let path_bytes = manifest_path.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(1)) * 8;
    if path_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let mut request = RawMessage::empty(ManagerTag::ActivateRequest as u32);
    request.word_count = 1 + pack_bytes(path_bytes, &mut request.words[1..])?;
    request.words[0] = path_bytes.len() as u64;
    channel_send(bootstrap, &request)?;

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(bootstrap, &mut response)?;
    if response.tag != ManagerTag::ActivateReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }

    match manager_status_from_word(response.words[0]) {
        ManagerStatus::Ok => Ok(service_id_from_word(response.words[1])),
        ManagerStatus::Busy | ManagerStatus::Failed => Err(Error::Busy),
        ManagerStatus::NotFound => Err(Error::NotFound),
        ManagerStatus::Denied => Err(Error::PermissionDenied),
    }
}

pub fn manager_deactivate_service(bootstrap: Handle, service_id: ServiceId) -> Result<()> {
    let mut request = RawMessage::empty(ManagerTag::DeactivateRequest as u32);
    request.word_count = 1;
    request.words[0] = service_id as u32 as u64;
    channel_send(bootstrap, &request)?;

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(bootstrap, &mut response)?;
    if response.tag != ManagerTag::DeactivateReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }

    match manager_status_from_word(response.words[0]) {
        ManagerStatus::Ok => Ok(()),
        ManagerStatus::Busy | ManagerStatus::Failed => Err(Error::Busy),
        ManagerStatus::NotFound => Err(Error::NotFound),
        ManagerStatus::Denied => Err(Error::PermissionDenied),
    }
}

use crate::{
    Error, Handle, RawMessage, Result, StatusHealth, StatusResult, StatusServiceInfo, StatusTag,
    channel_call, channel_create, channel_receive_blocking, channel_send, handle_close,
    manager_phase_from_word, rights, service_id_from_word,
};

pub fn status_snapshot(status_handle: Handle) -> Result<(u64, u64, u64)> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(StatusTag::SnapshotRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(status_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != StatusTag::SnapshotReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    Ok((
        response.words[0],
        response.words[1],
        response.words.get(2).copied().unwrap_or(0),
    ))
}

pub fn status_report_service(
    status_handle: Handle,
    service_id: crate::ServiceId,
    phase: crate::ManagerServicePhase,
    health: StatusHealth,
    detail_kind: u32,
    detail0: u64,
    detail1: u64,
    updated_tick: u64,
) -> Result<()> {
    let mut request = RawMessage::empty(StatusTag::ServiceReport as u32);
    request.word_count = 7;
    request.words[0] = service_id as u32 as u64;
    request.words[1] = phase as u32 as u64;
    request.words[2] = health as u32 as u64;
    request.words[3] = detail_kind as u64;
    request.words[4] = detail0;
    request.words[5] = detail1;
    request.words[6] = updated_tick;
    channel_send(status_handle, &request)
}

pub fn status_query_service(
    status_handle: Handle,
    service_id: crate::ServiceId,
) -> Result<Option<StatusServiceInfo>> {
    let mut request = RawMessage::empty(StatusTag::ServiceQueryRequest as u32);
    request.word_count = 1;
    request.words[0] = service_id as u32 as u64;
    let response = channel_call(status_handle, &mut request)?;
    if response.tag != StatusTag::ServiceQueryReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match response.words[0] as u32 {
        x if x == StatusResult::NotFound as u32 => Ok(None),
        x if x == StatusResult::Ok as u32 => {
            if response.word_count < 8 {
                return Err(Error::InvalidArgument);
            }
            Ok(Some(StatusServiceInfo {
                service_id: service_id_from_word(response.words[1]),
                phase: manager_phase_from_word(response.words[2]),
                health: status_health_from_word(response.words[3]),
                detail_kind: response.words[4] as u32,
                detail0: response.words[5],
                detail1: response.words[6],
                updated_tick: response.words[7],
            }))
        }
        _ => Err(Error::InvalidArgument),
    }
}

pub fn status_list_services(
    status_handle: Handle,
    entries: &mut [StatusServiceInfo],
) -> Result<usize> {
    let mut loaded = 0usize;
    let mut page = 0usize;
    loop {
        let mut request = RawMessage::empty(StatusTag::ServiceListRequest as u32);
        request.word_count = 1;
        request.words[0] = page as u64;
        let response = channel_call(status_handle, &mut request)?;
        if response.tag != StatusTag::ServiceListReply as u32 || response.word_count < 2 {
            return Err(Error::InvalidArgument);
        }
        let count = response.words[0] as usize;
        let next_page = response.words[1] as usize;
        if loaded + count > entries.len() || response.word_count < (2 + count * 7) as u32 {
            return Err(Error::BufferTooSmall);
        }
        for index in 0..count {
            let base = 2 + index * 7;
            entries[loaded + index] = StatusServiceInfo {
                service_id: service_id_from_word(response.words[base]),
                phase: manager_phase_from_word(response.words[base + 1]),
                health: status_health_from_word(response.words[base + 2]),
                detail_kind: response.words[base + 3] as u32,
                detail0: response.words[base + 4],
                detail1: response.words[base + 5],
                updated_tick: response.words[base + 6],
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

pub fn status_subscribe(status_handle: Handle, filter: Option<crate::ServiceId>) -> Result<Handle> {
    let subscription = channel_create()?;
    let reply = channel_create()?;
    let mut request = RawMessage::empty(StatusTag::SubscribeRequest as u32);
    request.word_count = 1;
    request.words[0] = filter.map_or(0, |service| service as u32 as u64);
    request.handle_count = 2;
    request.handles[0] = subscription.second;
    request.handle_rights[0] = rights::SEND;
    request.handles[1] = reply.second;
    request.handle_rights[1] = rights::SEND;
    channel_send(status_handle, &request)?;
    let _ = handle_close(subscription.second);
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != StatusTag::SubscribeReply as u32 || response.word_count < 1 {
        let _ = handle_close(subscription.first);
        return Err(Error::InvalidArgument);
    }
    match response.words[0] as u32 {
        x if x == StatusResult::Ok as u32 => Ok(subscription.first),
        x if x == StatusResult::Busy as u32 => {
            let _ = handle_close(subscription.first);
            Err(Error::Busy)
        }
        x if x == StatusResult::Denied as u32 => {
            let _ = handle_close(subscription.first);
            Err(Error::PermissionDenied)
        }
        _ => {
            let _ = handle_close(subscription.first);
            Err(Error::InvalidArgument)
        }
    }
}

pub fn status_receive_event(subscription_handle: Handle) -> Result<StatusServiceInfo> {
    let mut response = RawMessage::empty(0);
    channel_receive_blocking(subscription_handle, &mut response)?;
    if response.tag != StatusTag::StreamEvent as u32 || response.word_count < 7 {
        return Err(Error::InvalidArgument);
    }
    Ok(StatusServiceInfo {
        service_id: service_id_from_word(response.words[0]),
        phase: manager_phase_from_word(response.words[1]),
        health: status_health_from_word(response.words[2]),
        detail_kind: response.words[3] as u32,
        detail0: response.words[4],
        detail1: response.words[5],
        updated_tick: response.words[6],
    })
}

fn status_health_from_word(value: u64) -> StatusHealth {
    match value as u32 {
        x if x == StatusHealth::Healthy as u32 => StatusHealth::Healthy,
        x if x == StatusHealth::Degraded as u32 => StatusHealth::Degraded,
        x if x == StatusHealth::Failing as u32 => StatusHealth::Failing,
        x if x == StatusHealth::Recovering as u32 => StatusHealth::Recovering,
        x if x == StatusHealth::Dormant as u32 => StatusHealth::Dormant,
        _ => StatusHealth::Unknown,
    }
}

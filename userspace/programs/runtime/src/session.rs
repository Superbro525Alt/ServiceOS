use crate::{
    Error, Handle, RawMessage, Result, SessionStatus, SessionStatusInfo, SessionTag,
    channel_create, channel_receive_blocking, channel_send, handle_close, rights,
    session_input_source_from_word, session_status_error, session_status_from_word,
};

pub fn session_list(session_handle: Handle, ids: &mut [u32]) -> Result<usize> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SessionTag::ListRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(session_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SessionTag::ListReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    let status = session_status_from_word(response.words[0]);
    if status != SessionStatus::Ok {
        return Err(session_status_error(status));
    }
    let count = response.words[1] as usize;
    if count > ids.len() || (response.word_count as usize) < 2 + count {
        return Err(Error::BufferTooSmall);
    }
    for (index, id) in ids.iter_mut().enumerate().take(count) {
        *id = response.words[2 + index] as u32;
    }
    Ok(count)
}

pub fn session_status(
    service_handle: Handle,
    session_id: u32,
) -> Result<Option<SessionStatusInfo>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SessionTag::StatusRequest as u32);
    request.word_count = 1;
    request.words[0] = session_id as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(service_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SessionTag::StatusReply as u32 || response.word_count < 5 {
        return Err(Error::InvalidArgument);
    }
    let status = session_status_from_word(response.words[0]);
    if status == SessionStatus::NotFound {
        return Ok(None);
    }
    if status != SessionStatus::Ok {
        return Err(session_status_error(status));
    }

    Ok(Some(SessionStatusInfo {
        session_id: response.words[1] as u32,
        input_source: session_input_source_from_word(response.words[2]),
        focused_surface: response.words[3] as u32,
        surface_count: response.words[4] as u32,
    }))
}

pub fn session_focus(service_handle: Handle, session_id: u32, surface_id: u32) -> Result<u32> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SessionTag::FocusRequest as u32);
    request.word_count = 2;
    request.words[0] = session_id as u64;
    request.words[1] = surface_id as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(service_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SessionTag::FocusReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match session_status_from_word(response.words[0]) {
        SessionStatus::Ok => Ok(response.words[1] as u32),
        status => Err(session_status_error(status)),
    }
}

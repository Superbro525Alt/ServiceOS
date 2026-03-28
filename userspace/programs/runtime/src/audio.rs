use crate::{
    AudioEndpointStatusInfo, AudioStatus, AudioStreamDirection, AudioStreamInfo, AudioTag,
    AudioToneRequest, Error, Handle, RawMessage, Result, audio_endpoint_backend_from_word,
    audio_endpoint_direction_from_word, audio_endpoint_info, audio_endpoint_play_tone,
    audio_endpoint_state_from_word, audio_status_error, audio_status_from_word,
    audio_stream_direction_from_word, audio_stream_state_from_word, audio_endpoint_stop, channel_create,
    channel_receive_blocking, channel_send, handle_close, rights,
};

pub fn audio_service_endpoint_count(audio_handle: Handle) -> Result<usize> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(AudioTag::EndpointListRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(audio_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != AudioTag::EndpointListReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match audio_status_from_word(response.words[0]) {
        AudioStatus::Ok => Ok(response.words[1] as usize),
        status => Err(audio_status_error(status)),
    }
}

pub fn audio_service_endpoint_status(
    audio_handle: Handle,
    index: usize,
) -> Result<Option<AudioEndpointStatusInfo>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(AudioTag::EndpointStatusRequest as u32);
    request.word_count = 1;
    request.words[0] = index as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(audio_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != AudioTag::EndpointStatusReply as u32 || response.word_count < 12 {
        return Err(Error::InvalidArgument);
    }
    let status = audio_status_from_word(response.words[0]);
    if status == AudioStatus::NotFound {
        return Ok(None);
    }
    if status != AudioStatus::Ok {
        return Err(audio_status_error(status));
    }

    Ok(Some(AudioEndpointStatusInfo {
        index: response.words[1] as u32,
        backend: audio_endpoint_backend_from_word(response.words[2]),
        direction: audio_endpoint_direction_from_word(response.words[3]),
        state: audio_endpoint_state_from_word(response.words[4]),
        capabilities: response.words[5] as u32,
        nominal_rate_hz: response.words[6] as u32,
        channels: response.words[7] as u32,
        min_frequency_hz: response.words[8] as u32,
        max_frequency_hz: response.words[9] as u32,
        current_frequency_hz: response.words[10] as u32,
        play_count: response.words[11],
    }))
}

pub fn audio_stream_open(
    audio_handle: Handle,
    direction: AudioStreamDirection,
    session_id: u32,
) -> Result<Handle> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(AudioTag::StreamOpenRequest as u32);
    request.word_count = 2;
    request.words[0] = direction as u32 as u64;
    request.words[1] = session_id as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(audio_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != AudioTag::StreamOpenReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    let status = audio_status_from_word(response.words[0]);
    if status != AudioStatus::Ok {
        return Err(audio_status_error(status));
    }
    if response.handle_count < 1 {
        return Err(Error::InvalidArgument);
    }
    Ok(response.handles[0])
}

pub fn audio_stream_list(audio_handle: Handle, streams: &mut [AudioStreamInfo]) -> Result<usize> {
    let mut filled = 0usize;
    let mut start = 0usize;

    loop {
        let reply = channel_create()?;
        let mut request = RawMessage::empty(AudioTag::StreamListRequest as u32);
        request.word_count = 1;
        request.words[0] = start as u64;
        request.handle_count = 1;
        request.handles[0] = reply.second;
        request.handle_rights[0] = rights::SEND;
        channel_send(audio_handle, &request)?;
        let _ = handle_close(reply.second);

        let mut response = RawMessage::empty(0);
        channel_receive_blocking(reply.first, &mut response)?;
        let _ = handle_close(reply.first);
        if response.tag != AudioTag::StreamListReply as u32 || response.word_count < 3 {
            return Err(Error::InvalidArgument);
        }
        let status = audio_status_from_word(response.words[0]);
        if status != AudioStatus::Ok {
            return Err(audio_status_error(status));
        }
        let count = response.words[1] as usize;
        let next = response.words[2] as usize;
        if filled + count > streams.len() || response.word_count as usize != 3 + count * 6 {
            return Err(Error::BufferTooSmall);
        }
        for page_index in 0..count {
            let base = 3 + page_index * 6;
            streams[filled + page_index] = AudioStreamInfo {
                slot: response.words[base] as u32,
                direction: audio_stream_direction_from_word(response.words[base + 1]),
                state: audio_stream_state_from_word(response.words[base + 2]),
                session_id: response.words[base + 3] as u32,
                endpoint_index: response.words[base + 4] as u32,
                frequency_hz: response.words[base + 5] as u32,
                remaining_ticks: 0,
            };
        }
        filled += count;
        if count == 0 || next <= start {
            return Ok(filled);
        }
        start = next;
    }
}

pub fn audio_stream_status(stream_handle: Handle) -> Result<AudioStreamInfo> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(AudioTag::StreamStatusRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(stream_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != AudioTag::StreamStatusReply as u32 || response.word_count < 7 {
        return Err(Error::InvalidArgument);
    }
    let status = audio_status_from_word(response.words[0]);
    if status != AudioStatus::Ok {
        return Err(audio_status_error(status));
    }
    Ok(AudioStreamInfo {
        slot: response.words[1] as u32,
        direction: audio_stream_direction_from_word(response.words[2]),
        state: audio_stream_state_from_word(response.words[3]),
        session_id: response.words[4] as u32,
        endpoint_index: response.words[5] as u32,
        frequency_hz: response.words[6] as u32,
        remaining_ticks: response.words.get(7).copied().unwrap_or(0),
    })
}

pub fn audio_stream_play_tone(
    stream_handle: Handle,
    frequency_hz: u32,
    duration_ms: u32,
) -> Result<()> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(AudioTag::StreamPlayToneRequest as u32);
    request.word_count = 2;
    request.words[0] = frequency_hz as u64;
    request.words[1] = duration_ms as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(stream_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != AudioTag::StreamPlayToneReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    let status = audio_status_from_word(response.words[0]);
    if status != AudioStatus::Ok {
        return Err(audio_status_error(status));
    }
    Ok(())
}

pub fn audio_stream_close(stream_handle: Handle) -> Result<()> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(AudioTag::StreamCloseRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(stream_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != AudioTag::StreamCloseReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    let status = audio_status_from_word(response.words[0]);
    if status != AudioStatus::Ok && status != AudioStatus::Closed {
        return Err(audio_status_error(status));
    }
    Ok(())
}

pub fn kernel_audio_endpoint_status(handle: Handle) -> Result<AudioEndpointStatusInfo> {
    let info = audio_endpoint_info(handle)?;
    Ok(AudioEndpointStatusInfo {
        index: 0,
        backend: audio_endpoint_backend_from_word(info.backend as u64),
        direction: audio_endpoint_direction_from_word(info.direction as u64),
        state: audio_endpoint_state_from_word(info.state as u64),
        capabilities: info.capabilities,
        nominal_rate_hz: info.nominal_rate_hz,
        channels: info.channels,
        min_frequency_hz: info.min_frequency_hz,
        max_frequency_hz: info.max_frequency_hz,
        current_frequency_hz: info.current_frequency_hz,
        play_count: info.play_count,
    })
}

pub fn kernel_audio_endpoint_play_tone(
    handle: Handle,
    frequency_hz: u32,
    duration_ticks: u32,
) -> Result<()> {
    audio_endpoint_play_tone(
        handle,
        AudioToneRequest {
            frequency_hz,
            duration_ticks,
            volume: u16::MAX,
            flags: 0,
        },
    )
}

pub fn kernel_audio_endpoint_stop(handle: Handle) -> Result<()> {
    audio_endpoint_stop(handle)
}

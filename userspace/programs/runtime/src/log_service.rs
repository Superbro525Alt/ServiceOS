use crate::{
    channel_create, channel_receive_blocking, channel_send, handle_close, rights, service_id_from_word,
    severity_from_word, domain_from_word, event_from_word, Error, Handle, LogDomain, LogEvent,
    LogQueryStatus, LogRecord, LogSeverity, LogTag, RawMessage, Result, ServiceId,
};

pub fn send_log_record(
    log_handle: Handle,
    source: ServiceId,
    severity: LogSeverity,
    domain: LogDomain,
    event: LogEvent,
    arg0: u64,
    arg1: u64,
) -> Result<()> {
    let mut message = RawMessage::empty(LogTag::Record as u32);
    message.word_count = 6;
    message.words[0] = source as u32 as u64;
    message.words[1] = severity as u32 as u64;
    message.words[2] = domain as u32 as u64;
    message.words[3] = event as u32 as u64;
    message.words[4] = arg0;
    message.words[5] = arg1;
    channel_send(log_handle, &message)
}

pub fn log_query_info(log_handle: Handle) -> Result<(u64, u64)> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(LogTag::QueryInfoRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(log_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != LogTag::QueryInfoReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    Ok((response.words[0], response.words[1]))
}

pub fn log_query_record(log_handle: Handle, sequence: u64) -> Result<Option<LogRecord>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(LogTag::QueryRecordRequest as u32);
    request.word_count = 1;
    request.words[0] = sequence;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(log_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != LogTag::QueryRecordReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    if response.words[0] as u32 == LogQueryStatus::NotFound as u32 {
        return Ok(None);
    }
    if response.word_count < 8 {
        return Err(Error::InvalidArgument);
    }

    Ok(Some(LogRecord {
        sequence: response.words[1],
        source: service_id_from_word(response.words[2]),
        severity: severity_from_word(response.words[3]),
        domain: domain_from_word(response.words[4]),
        event: event_from_word(response.words[5]),
        arg0: response.words[6],
        arg1: response.words[7],
    }))
}

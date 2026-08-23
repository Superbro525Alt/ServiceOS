use crate::{
    Error, Handle, LOG_FILTER_ANY, LogDomain, LogEvent, LogQueryStatus, LogRecord, LogSeverity,
    LogStatus, LogTag, RawMessage, Result, ServiceId, channel_call, channel_create,
    channel_receive_blocking, channel_send, domain_from_word, event_from_word, handle_close,
    rights, service_id_from_word, severity_from_word,
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
    send_log_record_ex(log_handle, source, severity, domain, event, arg0, arg1, 0)
}

pub fn send_log_record_ex(
    log_handle: Handle,
    source: ServiceId,
    severity: LogSeverity,
    domain: LogDomain,
    event: LogEvent,
    arg0: u64,
    arg1: u64,
    arg2: u64,
) -> Result<()> {
    let mut message = RawMessage::empty(LogTag::Record as u32);
    message.word_count = 7;
    message.words[0] = source as u32 as u64;
    message.words[1] = severity as u32 as u64;
    message.words[2] = domain as u32 as u64;
    message.words[3] = event as u32 as u64;
    message.words[4] = arg0;
    message.words[5] = arg1;
    message.words[6] = arg2;
    channel_send(log_handle, &message)
}

pub fn log_query_info(log_handle: Handle) -> Result<(u64, u64)> {
    let mut request = RawMessage::empty(LogTag::QueryInfoRequest as u32);
    let response = channel_call(log_handle, &mut request)?;
    if response.tag != LogTag::QueryInfoReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    Ok((response.words[0], response.words[1]))
}

pub fn log_query_record(log_handle: Handle, sequence: u64) -> Result<Option<LogRecord>> {
    let mut request = RawMessage::empty(LogTag::QueryRecordRequest as u32);
    request.word_count = 1;
    request.words[0] = sequence;
    let response = channel_call(log_handle, &mut request)?;
    if response.tag != LogTag::QueryRecordReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    if response.words[0] as u32 == LogQueryStatus::NotFound as u32 {
        return Ok(None);
    }
    if response.word_count < 10 {
        return Err(Error::InvalidArgument);
    }

    Ok(Some(LogRecord {
        sequence: response.words[1],
        tick: response.words[2],
        source: service_id_from_word(response.words[3]),
        severity: severity_from_word(response.words[4]),
        domain: domain_from_word(response.words[5]),
        event: event_from_word(response.words[6]),
        arg0: response.words[7],
        arg1: response.words[8],
        arg2: response.words[9],
    }))
}

pub fn log_subscribe(
    log_handle: Handle,
    minimum_severity: LogSeverity,
    source_filter: Option<ServiceId>,
    domain_filter: Option<LogDomain>,
) -> Result<Handle> {
    let subscription = channel_create()?;
    let reply = channel_create()?;
    let mut request = RawMessage::empty(LogTag::SubscribeRequest as u32);
    request.word_count = 3;
    request.words[0] = minimum_severity as u32 as u64;
    request.words[1] = source_filter.map_or(LOG_FILTER_ANY, |source| source as u32 as u64);
    request.words[2] = domain_filter.map_or(LOG_FILTER_ANY, |domain| domain as u32 as u64);
    request.handle_count = 2;
    request.handles[0] = subscription.second;
    request.handle_rights[0] = rights::SEND;
    request.handles[1] = reply.second;
    request.handle_rights[1] = rights::SEND;
    channel_send(log_handle, &request)?;
    let _ = handle_close(subscription.second);
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != LogTag::SubscribeReply as u32 || response.word_count < 1 {
        let _ = handle_close(subscription.first);
        return Err(Error::InvalidArgument);
    }
    match response.words[0] as u32 {
        x if x == LogStatus::Ok as u32 => Ok(subscription.first),
        x if x == LogStatus::Busy as u32 => {
            let _ = handle_close(subscription.first);
            Err(Error::Busy)
        }
        _ => {
            let _ = handle_close(subscription.first);
            Err(Error::InvalidArgument)
        }
    }
}

pub fn log_receive_record(subscription_handle: Handle) -> Result<LogRecord> {
    let mut response = RawMessage::empty(0);
    channel_receive_blocking(subscription_handle, &mut response)?;
    if response.tag != LogTag::StreamRecord as u32 || response.word_count < 9 {
        return Err(Error::InvalidArgument);
    }
    Ok(LogRecord {
        sequence: response.words[0],
        tick: response.words[1],
        source: service_id_from_word(response.words[2]),
        severity: severity_from_word(response.words[3]),
        domain: domain_from_word(response.words[4]),
        event: event_from_word(response.words[5]),
        arg0: response.words[6],
        arg1: response.words[7],
        arg2: response.words[8],
    })
}

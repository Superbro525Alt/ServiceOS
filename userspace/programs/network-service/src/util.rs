use smoltcp::time::Instant;
use smoltcp::wire::Ipv4Address;

use serviceos_userspace_runtime as rt;
use rt::{ControlTag, LifecycleEvent, LogEvent, LogSeverity, RawMessage, ServiceId};

pub(crate) fn decode_inline_text<'a>(
    words: &[u64],
    length: usize,
    buffer: &'a mut [u8],
) -> rt::Result<&'a str> {
    decode_inline_bytes(words, length, buffer).and_then(|bytes| {
        core::str::from_utf8(bytes).map_err(|_| rt::Error::InvalidArgument)
    })
}

pub(crate) fn decode_inline_bytes<'a>(
    words: &[u64],
    length: usize,
    buffer: &'a mut [u8],
) -> rt::Result<&'a [u8]> {
    if length > buffer.len() || length > words.len() * 8 {
        return Err(rt::Error::BufferTooSmall);
    }
    unpack_inline_bytes(words, length, buffer)?;
    Ok(&buffer[..length])
}

pub(crate) fn pack_inline_bytes(source: &[u8], words: &mut [u64]) -> rt::Result<u32> {
    let required_words = source.len().div_ceil(8);
    if required_words > words.len() {
        return Err(rt::Error::BufferTooSmall);
    }
    for (index, chunk) in source.chunks(8).enumerate() {
        let mut bytes = [0u8; 8];
        bytes[..chunk.len()].copy_from_slice(chunk);
        words[index] = u64::from_le_bytes(bytes);
    }
    Ok(required_words as u32)
}

pub(crate) fn unpack_inline_bytes(
    words: &[u64],
    length: usize,
    destination: &mut [u8],
) -> rt::Result<()> {
    if length > destination.len() || length > words.len() * 8 {
        return Err(rt::Error::BufferTooSmall);
    }

    let mut copied = 0usize;
    for word in words.iter().copied() {
        if copied >= length {
            break;
        }
        let bytes = word.to_le_bytes();
        let chunk = (length - copied).min(bytes.len());
        destination[copied..copied + chunk].copy_from_slice(&bytes[..chunk]);
        copied += chunk;
    }
    Ok(())
}

pub(crate) fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
    let mut message = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut message) {
        Ok(()) if message.tag == ControlTag::Lifecycle as u32 && message.word_count > 0 => {
            Ok(matches!(
                lifecycle_event_from_word(message.words[0]),
                LifecycleEvent::Restarting | LifecycleEvent::Stopped
            ))
        }
        Ok(()) => Ok(false),
        Err(rt::Error::QueueEmpty) => Ok(false),
        Err(error) => Err(error),
    }
}

fn lifecycle_event_from_word(value: u64) -> LifecycleEvent {
    match value as u32 {
        x if x == LifecycleEvent::Starting as u32 => LifecycleEvent::Starting,
        x if x == LifecycleEvent::Ready as u32 => LifecycleEvent::Ready,
        x if x == LifecycleEvent::Failed as u32 => LifecycleEvent::Failed,
        x if x == LifecycleEvent::Stopped as u32 => LifecycleEvent::Stopped,
        _ => LifecycleEvent::Restarting,
    }
}

pub(crate) fn emit_log(
    log_handle: rt::Handle,
    severity: LogSeverity,
    event: LogEvent,
    arg0: u64,
    arg1: u64,
) -> rt::Result<()> {
    rt::send_log_record(
        log_handle,
        ServiceId::Network,
        severity,
        rt::LogDomain::Network,
        event,
        arg0,
        arg1,
    )
}

pub(crate) fn pack_mac(mac: [u8; 6]) -> u64 {
    (mac[0] as u64)
        | ((mac[1] as u64) << 8)
        | ((mac[2] as u64) << 16)
        | ((mac[3] as u64) << 24)
        | ((mac[4] as u64) << 32)
        | ((mac[5] as u64) << 40)
}

pub(crate) fn ipv4_to_u32(address: Ipv4Address) -> u32 {
    let [a, b, c, d] = address.octets();
    u32::from_be_bytes([a, b, c, d])
}

pub(crate) fn u32_to_ipv4(value: u32) -> Ipv4Address {
    let [a, b, c, d] = value.to_be_bytes();
    Ipv4Address::new(a, b, c, d)
}

pub(crate) fn now_instant() -> Instant {
    Instant::from_millis(ticks_to_millis(rt::monotonic_now().unwrap_or(0)) as i64)
}

pub(crate) fn ticks_to_millis(ticks: u64) -> u64 {
    ticks.saturating_mul(10)
}

use smoltcp::time::Instant;
use smoltcp::wire::Ipv4Address;

use rt::{ControlTag, LifecycleEvent, LogEvent, LogSeverity, RawMessage, ServiceId};
use serviceos_userspace_runtime as rt;

pub(crate) fn decode_inline_text<'a>(
    words: &[u64],
    length: usize,
    buffer: &'a mut [u8],
) -> rt::Result<&'a str> {
    decode_inline_bytes(words, length, buffer)
        .and_then(|bytes| core::str::from_utf8(bytes).map_err(|_| rt::Error::InvalidArgument))
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

/// Derive the interface's IPv6 link-local address from its 48-bit MAC via
/// modified EUI-64 (RFC 4291 section 2.5.1): insert ff:fe in the middle of
/// the identifier and flip the universal/local bit of the first octet.
pub(crate) fn eui64_link_local(mac: [u8; 6]) -> smoltcp::wire::Ipv6Address {
    smoltcp::wire::Ipv6Address::new(
        0xfe80,
        0,
        0,
        0,
        ((mac[0] ^ 0x02) as u16) << 8 | mac[1] as u16,
        ((mac[2] as u16) << 8) | 0xff,
        (0xfe << 8) | mac[3] as u16,
        ((mac[4] as u16) << 8) | mac[5] as u16,
    )
}

/// The solicited-node multicast address (RFC 4291 section 2.7.1) for a
/// unicast address: ff02::1:ffXX:XXXX where XX:XXXX are the low 24 bits.
/// (smoltcp keeps its own helper crate-private.)
pub(crate) fn solicited_node_multicast(
    address: smoltcp::wire::Ipv6Address,
) -> smoltcp::wire::Ipv6Address {
    let octets = address.octets();
    smoltcp::wire::Ipv6Address::new(
        0xff02,
        0,
        0,
        0,
        0,
        1,
        0xff00 | octets[13] as u16,
        ((octets[14] as u16) << 8) | octets[15] as u16,
    )
}

/// Render an IPv6 address as eight lowercase hex groups for log lines.
pub(crate) struct Ipv6LogAddress(pub(crate) smoltcp::wire::Ipv6Address);

impl core::fmt::Display for Ipv6LogAddress {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let octets = self.0.octets();
        for group in 0..8 {
            let value = ((octets[group * 2] as u16) << 8) | octets[group * 2 + 1] as u16;
            if group > 0 {
                f.write_str(":")?;
            }
            write!(f, "{value:x}")?;
        }
        Ok(())
    }
}

pub(crate) fn now_instant() -> Instant {
    // Host tests must not reach rt::monotonic_now: the raw `int 0x80` stub
    // terminates the calling thread under a host kernel (the ServiceOS
    // MonotonicNow number is ia32 sys_exit there), which deadlocks the test
    // harness on thread join. Test builds advance a synthetic millisecond
    // clock instead; the serviceos build keeps the real syscall.
    #[cfg(test)]
    {
        use core::sync::atomic::{AtomicU64, Ordering};
        static TEST_TICKS: AtomicU64 = AtomicU64::new(0);
        return Instant::from_millis(TEST_TICKS.fetch_add(1, Ordering::Relaxed) as i64);
    }
    #[cfg(not(test))]
    {
        Instant::from_millis(ticks_to_millis(rt::monotonic_now().unwrap_or(0)) as i64)
    }
}

pub(crate) fn ticks_to_millis(ticks: u64) -> u64 {
    ticks.saturating_mul(10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eui64_link_local_follows_rfc4291() {
        // Modified EUI-64: ff:fe inserted mid-identifier, U/L bit flipped.
        let addr = eui64_link_local([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
        assert_eq!(
            addr.octets(),
            [
                0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0x50, 0x54, 0x00, 0xff, 0xfe, 0x12, 0x34, 0x56
            ]
        );
        // Universal/local bit set in the interface identifier.
        let addr = eui64_link_local([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
        assert_eq!(addr.octets()[8], 0x00);
        // Prefix is always fe80::/64.
        assert_eq!(&addr.octets()[..8], &[0xfe, 0x80, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn ipv6_log_address_renders_eight_groups() {
        use std::fmt::Write as _;
        let addr = eui64_link_local([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
        let mut rendered = String::new();
        write!(&mut rendered, "{}", Ipv6LogAddress(addr)).unwrap();
        assert_eq!(rendered, "fe80:0:0:0:5054:ff:fe12:3456");
    }
}

use core::str;
use serviceos_userspace_runtime as rt;

/// Wire parity with desktop-shell-service windows/drag.rs: notify-channel
/// text starting with this marker is a content intent, never a notification.
pub(crate) const CONTENT_INTENT_MARKER: u8 = 0x02;
pub(crate) const CONTENT_PAYLOAD_MAX: usize = 96;

/// Builds `[marker][hint][path]` into `out`, returning the encoded length.
pub(crate) fn encode_intent_bytes(
    hint: u8,
    path: &[u8],
    out: &mut [u8; CONTENT_PAYLOAD_MAX],
) -> rt::Result<usize> {
    let path_len = path.len();
    if path_len < 1 || 2 + path_len > out.len() {
        return Err(rt::Error::BufferTooSmall);
    }
    if !path.iter().all(|byte| byte.is_ascii_graphic()) {
        return Err(rt::Error::InvalidArgument);
    }
    out[0] = CONTENT_INTENT_MARKER;
    out[1] = hint;
    out[2..2 + path_len].copy_from_slice(path);
    Ok(2 + path_len)
}

/// Sends an open-with handoff (hint = target app digit) or arms a live drag
/// (hint = b'0') via the shell's existing notify channel.
pub(crate) fn send_content_intent(desktop: rt::Handle, hint: u8, path: &[u8]) -> rt::Result<()> {
    if desktop == rt::INVALID_HANDLE {
        return Err(rt::Error::NotFound);
    }
    let mut buffer = [0u8; CONTENT_PAYLOAD_MAX];
    let len = encode_intent_bytes(hint, path, &mut buffer)?;
    let text = str::from_utf8(&buffer[..len]).map_err(|_| rt::Error::InvalidArgument)?;
    rt::desktop_notify(desktop, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_marker_hint_and_path() {
        let mut out = [0u8; CONTENT_PAYLOAD_MAX];
        let len = encode_intent_bytes(b'4', b"home/build.log", &mut out).expect("valid");
        assert_eq!(len, 2 + b"home/build.log".len());
        assert_eq!(out[0], CONTENT_INTENT_MARKER);
        assert_eq!(out[1], b'4');
        assert_eq!(&out[2..len], b"home/build.log");
        let mut drag_out = [0u8; CONTENT_PAYLOAD_MAX];
        let drag_len = encode_intent_bytes(b'0', b"home/a.txt", &mut drag_out).expect("valid");
        assert_eq!(drag_out[..drag_len][1], b'0');
    }

    #[test]
    fn rejects_empty_oversize_and_non_graphic_paths() {
        let mut out = [0u8; CONTENT_PAYLOAD_MAX];
        assert_eq!(
            encode_intent_bytes(b'0', b"", &mut out),
            Err(rt::Error::BufferTooSmall)
        );
        let max_path = [b'a'; CONTENT_PAYLOAD_MAX - 2];
        assert!(encode_intent_bytes(b'0', &max_path, &mut out).is_ok());
        let oversized = [b'a'; CONTENT_PAYLOAD_MAX - 1];
        assert_eq!(
            encode_intent_bytes(b'0', &oversized, &mut out),
            Err(rt::Error::BufferTooSmall)
        );
        assert_eq!(
            encode_intent_bytes(b'0', b"has space.txt", &mut out),
            Err(rt::Error::InvalidArgument)
        );
    }
}

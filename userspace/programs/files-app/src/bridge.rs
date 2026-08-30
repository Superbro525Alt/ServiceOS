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

/// Longest single path inside a multi-file drag payload.
pub(crate) const MULTI_PATH_MAX: usize = 40;
/// Fan-out cap (wire parity with desktop-shell drag.rs budget).
pub(crate) const MULTI_COUNT_MAX: usize = 4;

/// Encodes a multi-file undecided drag into `out` as
/// `[marker][b'0'][count '2'..='4'][2-digit len][path]...` — exactly
/// `paths.len()` segments, each ASCII-graphic, total within the 96-byte
/// notify budget. Wire parity with desktop-shell drag.rs parse_multi_paths.
pub(crate) fn encode_multi_intent_bytes(
    paths: &[&[u8]],
    out: &mut [u8; CONTENT_PAYLOAD_MAX],
) -> rt::Result<usize> {
    if paths.len() < 2 || paths.len() > MULTI_COUNT_MAX {
        return Err(rt::Error::InvalidArgument);
    }
    out[0] = CONTENT_INTENT_MARKER;
    out[1] = b'0';
    out[2] = b'0' + paths.len() as u8;
    let mut len = 3usize;
    for path in paths {
        if path.is_empty()
            || path.len() > MULTI_PATH_MAX
            || len + 2 + path.len() > out.len()
            || !path.iter().all(|byte| byte.is_ascii_graphic())
        {
            return Err(rt::Error::BufferTooSmall);
        }
        out[len] = b'0' + (path.len() / 10) as u8;
        out[len + 1] = b'0' + (path.len() % 10) as u8;
        out[len + 2..len + 2 + path.len()].copy_from_slice(path);
        len += 2 + path.len();
    }
    Ok(len)
}

/// Sends a multi-file live drag (hint = b'0') over the notify channel.
pub(crate) fn send_multi_content_intent(desktop: rt::Handle, paths: &[&[u8]]) -> rt::Result<()> {
    if desktop == rt::INVALID_HANDLE {
        return Err(rt::Error::NotFound);
    }
    let mut buffer = [0u8; CONTENT_PAYLOAD_MAX];
    let len = encode_multi_intent_bytes(paths, &mut buffer)?;
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

    #[test]
    fn multi_encodes_count_len_prefixed_segments() {
        let mut out = [0u8; CONTENT_PAYLOAD_MAX];
        let len = encode_multi_intent_bytes(&[b"home/a.txt", b"home/bb.txt"], &mut out)
            .expect("valid multi");
        assert_eq!(out[0], CONTENT_INTENT_MARKER);
        assert_eq!(out[1], b'0');
        assert_eq!(out[2], b'2');
        assert_eq!(&out[3..5], b"10");
        assert_eq!(&out[5..15], b"home/a.txt");
        assert_eq!(&out[15..17], b"11");
        assert_eq!(&out[17..len], b"home/bb.txt");
    }

    #[test]
    fn multi_rejects_bad_counts_lengths_and_non_graphic() {
        let mut out = [0u8; CONTENT_PAYLOAD_MAX];
        assert_eq!(
            encode_multi_intent_bytes(&[b"only"], &mut out),
            Err(rt::Error::InvalidArgument)
        );
        let five: [&[u8]; 5] = [&b"x"[..]; 5];
        assert_eq!(
            encode_multi_intent_bytes(&five, &mut out),
            Err(rt::Error::InvalidArgument)
        );
        assert_eq!(
            encode_multi_intent_bytes(&[b"a", b""], &mut out),
            Err(rt::Error::BufferTooSmall)
        );
        let long_path = [b'a'; MULTI_PATH_MAX + 1];
        assert_eq!(
            encode_multi_intent_bytes(&[b"a", &long_path], &mut out),
            Err(rt::Error::BufferTooSmall)
        );
        assert_eq!(
            encode_multi_intent_bytes(&[b"a", b"sp ace"], &mut out),
            Err(rt::Error::BufferTooSmall)
        );
    }

    #[test]
    fn multi_accepts_full_budget_of_four_paths() {
        let mut out = [0u8; CONTENT_PAYLOAD_MAX];
        let paths: [&[u8]; 4] = [&b"home/aaaaaaaaa"[..]; 4];
        let len = encode_multi_intent_bytes(&paths, &mut out).expect("fits budget");
        assert_eq!(len, 3 + 4 * (2 + paths[0].len()));
        assert_eq!(out[2], b'4');
    }
}

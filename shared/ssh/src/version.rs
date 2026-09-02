//! Version exchange (RFC 4253 §4.2): identification string parse and emit.
//!
//! Rules implemented: the identification is a single line
//! `SSH-protoversion-softwareversion[ SP comments] CRLF`, at most 255 octets
//! including CRLF, printable US-ASCII only. Both `SSH-2.0-` and the
//! transitional `SSH-1.99-` prefix are accepted (the latter means
//! "v2-capable", which is all we speak). The server role rejects client
//! pre-banner lines (the RFC forbids them for clients); the client role
//! tolerates a bounded number of server pre-banner lines.

/// Maximum identification line length including CRLF (RFC 4253 §4.2).
pub const IDENT_MAX: usize = 255;

/// Our server identification string, complete with CRLF.
pub const SERVER_BANNER: &[u8] = b"SSH-2.0-ServiceOS_0.1.0\r\n";

/// Identification text without CRLF, i.e. the V_C / V_S string used in the
/// exchange hash.
pub const SERVER_BANNER_TEXT: &[u8] = b"SSH-2.0-ServiceOS_0.1.0";

/// Identification parse failures (all map to protocol disconnects).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentErr {
    /// Line text exceeded 253 octets (255 incl. CRLF).
    TooLong,
    /// Line did not start with SSH-2.0- or SSH-1.99-.
    BadPrefix,
    /// Non-printable or whitespace-bearing octet outside the comment rules.
    NotPrintable,
    /// Empty line.
    Empty,
}

/// Parse an identification line with the CRLF already stripped.
/// Returns the V_x text (full line) on success.
pub fn parse_identification(line: &[u8]) -> Result<&[u8], IdentErr> {
    if line.is_empty() {
        return Err(IdentErr::Empty);
    }
    if line.len() > IDENT_MAX - 2 {
        return Err(IdentErr::TooLong);
    }
    if !(line.starts_with(b"SSH-2.0-") || line.starts_with(b"SSH-1.99-")) {
        return Err(IdentErr::BadPrefix);
    }
    for &b in line {
        // Printable US-ASCII, no CR/LF/NUL (they are already stripped from
        // the line, but defend against embedded control bytes).
        if !(0x20..0x7f).contains(&b) {
            return Err(IdentErr::NotPrintable);
        }
    }
    Ok(line)
}

/// Parse the server's identification from the client role, tolerating up to
/// `max_pre_lines` leading non-identification lines (RFC 4253 §4.2 allows
/// the server to send them). `buf` holds raw received bytes. Returns
/// `Ok(None)` when no complete line is available yet, otherwise
/// `Ok(Some((consumed, ident_line)))` where `consumed` bytes may be dropped
/// from the front of the stream and `ident_line` is the identified line with
/// CRLF stripped. Pre-lines longer than `IDENT_MAX` abort with `TooLong`.
pub fn parse_server_identification(
    buf: &[u8],
    max_pre_lines: usize,
) -> Result<Option<(usize, &[u8])>, IdentErr> {
    let mut pre_lines = 0;
    let mut scan = 0;
    loop {
        let Some(lf) = buf[scan..].iter().position(|&b| b == b'\n') else {
            // Incomplete line; bound the total pending length.
            if buf.len() > IDENT_MAX {
                return Err(IdentErr::TooLong);
            }
            return Ok(None);
        };
        let line_end = scan + lf;
        let mut line = &buf[scan..line_end];
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        let consumed = line_end + 1;
        if line.starts_with(b"SSH-2.0-") || line.starts_with(b"SSH-1.99-") {
            return parse_identification(line).map(|l| Some((consumed, l)));
        }
        pre_lines += 1;
        if pre_lines > max_pre_lines {
            return Err(IdentErr::BadPrefix);
        }
        scan = consumed;
        if scan >= buf.len() {
            return Ok(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v2_and_v199() {
        assert_eq!(
            parse_identification(b"SSH-2.0-OpenSSH_9.7").unwrap(),
            b"SSH-2.0-OpenSSH_9.7"
        );
        assert_eq!(
            parse_identification(b"SSH-2.0-OpenSSH_9.7 comment here").unwrap(),
            b"SSH-2.0-OpenSSH_9.7 comment here"
        );
        assert_eq!(
            parse_identification(b"SSH-1.99-x").unwrap(),
            &b"SSH-1.99-x"[..]
        );
    }

    #[test]
    fn rejects_bad_lines() {
        assert_eq!(parse_identification(b""), Err(IdentErr::Empty));
        assert_eq!(
            parse_identification(b"HTTP/1.1 x"),
            Err(IdentErr::BadPrefix)
        );
        assert_eq!(
            parse_identification(b"SSH-1.5-old"),
            Err(IdentErr::BadPrefix)
        );
        let long = [b'a'; 254];
        assert_eq!(parse_identification(&long), Err(IdentErr::TooLong));
        let mut ctrl = b"SSH-2.0-x".to_vec();
        ctrl.push(0x01);
        assert_eq!(parse_identification(&ctrl), Err(IdentErr::NotPrintable));
    }

    #[test]
    fn banner_shape() {
        assert!(SERVER_BANNER.ends_with(b"\r\n"));
        assert!(SERVER_BANNER.len() <= IDENT_MAX);
        let text = &SERVER_BANNER[..SERVER_BANNER.len() - 2];
        assert_eq!(text, SERVER_BANNER_TEXT);
        parse_identification(SERVER_BANNER_TEXT).unwrap();
    }

    #[test]
    fn server_ident_skips_pre_lines() {
        let raw = b"hello\nanother\r\nSSH-2.0-srv\r\ntrailing";
        let (consumed, ident) = parse_server_identification(raw, 4).unwrap().unwrap();
        assert_eq!(ident, &b"SSH-2.0-srv"[..]);
        assert_eq!(consumed, "hello\nanother\r\nSSH-2.0-srv\r\n".len());
    }

    #[test]
    fn server_ident_no_pre_line_needed() {
        let (consumed, ident) = parse_server_identification(b"SSH-2.0-x\r\nmore", 0)
            .unwrap()
            .unwrap();
        assert_eq!(ident, &b"SSH-2.0-x"[..]);
        assert_eq!(consumed, "SSH-2.0-x\r\n".len());
    }

    #[test]
    fn server_ident_incomplete_is_none() {
        assert!(
            parse_server_identification(b"SSH-2.0-x", 2)
                .unwrap()
                .is_none()
        );
        assert!(parse_server_identification(b"", 2).unwrap().is_none());
        let long = [b'a'; 300];
        assert_eq!(
            parse_server_identification(&long, 2),
            Err(IdentErr::TooLong)
        );
    }

    #[test]
    fn server_ident_pre_line_cap() {
        assert_eq!(
            parse_server_identification(b"a\nb\nSSH-2.0-x\r\n", 1),
            Err(IdentErr::BadPrefix)
        );
    }

    #[test]
    fn crlf_only_line_ends() {
        // LF-terminated without CR is tolerated on read; CR is stripped.
        let (consumed, ident) = parse_server_identification(b"SSH-2.0-y\n", 0)
            .unwrap()
            .unwrap();
        assert_eq!(ident, &b"SSH-2.0-y"[..]);
        assert_eq!(consumed, "SSH-2.0-y\n".len());
    }
}

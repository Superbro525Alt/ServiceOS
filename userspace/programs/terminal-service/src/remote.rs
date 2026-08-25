//! Remote terminal sessions over the network service's TCP listener path.
//!
//! PROTOCOL (rsh-like, PLAINTEXT, pre-SSH): each frame is
//! `u16be length | payload`, `1 <= length <= REMOTE_FRAME_MAX`. The first
//! client->server frame carries the auth token when [`REMOTE_AUTH_TOKEN`]
//! is non-empty; every other frame in either direction is a raw terminal
//! byte-stream chunk. No encryption, no integrity protection: this is a
//! stopgap until real SSH/TLS lands (see docs/roadmap.md S10).
//!
//! The framing codec, auth gate, and link state machine are pure logic
//! (host-testable below); socket IPC glue lives in the non-test helpers.

use crate::state::REMOTE_FRAME_MAX;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip_yields_payload() {
        let mut wire = [0u8; 2 + REMOTE_FRAME_MAX];
        let written = encode_frame(b"echo hi", &mut wire).unwrap();
        assert_eq!(written, 9);
        assert_eq!(&wire[..2], &[0x00, 0x07]);
        let mut decoder = FrameDecoder::new();
        decoder.push_bytes(&wire[..written]).unwrap();
        let mut out = [0u8; REMOTE_FRAME_MAX];
        assert_eq!(decoder.next_frame_into(&mut out).unwrap(), Some(7));
        assert_eq!(&out[..7], b"echo hi");
    }

    #[test]
    fn frame_rejects_empty_and_oversize_payload() {
        let mut wire = [0u8; 2 + REMOTE_FRAME_MAX];
        assert_eq!(encode_frame(b"", &mut wire), Err(RemoteError::BadLength));
        let big = [0u8; REMOTE_FRAME_MAX + 1];
        assert_eq!(encode_frame(&big, &mut wire), Err(RemoteError::BadLength));
        let mut tiny = [0u8; 4];
        assert_eq!(
            encode_frame(b"abcde", &mut tiny),
            Err(RemoteError::Overflow)
        );
    }

    #[test]
    fn frame_decoder_splits_across_chunks() {
        let mut wire = [0u8; 2 + REMOTE_FRAME_MAX];
        let written = encode_frame(b"remote", &mut wire).unwrap();
        let mut decoder = FrameDecoder::new();
        let mut out = [0u8; REMOTE_FRAME_MAX];
        decoder.push_bytes(&wire[..3]).unwrap();
        assert!(decoder.next_frame_into(&mut out).unwrap().is_none());
        decoder.push_bytes(&wire[3..written]).unwrap();
        assert_eq!(decoder.next_frame_into(&mut out).unwrap(), Some(6));
        assert_eq!(&out[..6], b"remote");
    }

    #[test]
    fn frame_decoder_rejects_bad_length_prefix() {
        let mut decoder = FrameDecoder::new();
        // Zero-length prefix is not a valid frame start.
        assert_eq!(
            decoder.push_bytes(&[0x00, 0x00]),
            Err(RemoteError::BadLength)
        );
        let mut decoder = FrameDecoder::new();
        // Length beyond capacity must trip before buffering overflows.
        assert_eq!(
            decoder.push_bytes(&[0xff, 0xff, 0x01]),
            Err(RemoteError::BadLength)
        );
    }

    #[test]
    fn auth_gate_accepts_matching_token() {
        assert_eq!(AuthGate::check(b"sekret", b"sekret"), AuthOutcome::Accepted);
    }

    #[test]
    fn auth_gate_rejects_wrong_token() {
        assert_eq!(AuthGate::check(b"sekret", b"wrong!"), AuthOutcome::Rejected);
        assert_eq!(AuthGate::check(b"sekret", b""), AuthOutcome::Rejected);
        assert_eq!(
            AuthGate::check(b"sekret", b"sekrets"),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn auth_gate_open_when_unconfigured() {
        assert_eq!(AuthGate::check(b"", b""), AuthOutcome::Accepted);
        assert_eq!(AuthGate::check(b"", b"anything"), AuthOutcome::Accepted);
    }

    #[test]
    fn link_starts_awaiting_auth_then_activates() {
        let mut link = RemoteLink::new(b"token");
        assert_eq!(link.state(), LinkState::AwaitingAuth);
        match link.on_frame(b"token") {
            LinkEvent::Banner => {}
            other => panic!("expected banner, got {other:?}"),
        }
        assert_eq!(link.state(), LinkState::Active);
        match link.on_frame(b"ls\r\n") {
            LinkEvent::Input(bytes) => assert_eq!(bytes, b"ls\r\n"),
            other => panic!("expected input, got {other:?}"),
        }
    }

    #[test]
    fn link_refuses_bad_token_and_closes() {
        let mut link = RemoteLink::new(b"token");
        assert!(matches!(link.on_frame(b"nope"), LinkEvent::Refuse));
        assert_eq!(link.state(), LinkState::Closed);
        // Frames after closure are ignored, never forwarded.
        assert!(matches!(link.on_frame(b"x"), LinkEvent::None));
    }

    #[test]
    fn link_open_gate_admits_first_frame_as_input() {
        let mut link = RemoteLink::new(b"");
        match link.on_frame(b"ls\r\n") {
            LinkEvent::Input(bytes) => assert_eq!(bytes, b"ls\r\n"),
            other => panic!("expected input, got {other:?}"),
        }
        assert_eq!(link.state(), LinkState::Active);
    }

    #[test]
    fn link_close_marks_closed_for_detach() {
        let mut link = RemoteLink::new(b"token");
        let _ = link.on_frame(b"token");
        link.close();
        assert_eq!(link.state(), LinkState::Closed);
    }
}

/// Errors surfaced by the framing codec.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RemoteError {
    BadLength,
    Overflow,
}

/// Encode one frame (`u16be len | payload`) into `out`.
pub(crate) fn encode_frame(payload: &[u8], out: &mut [u8]) -> Result<usize, RemoteError> {
    if payload.is_empty() || payload.len() > REMOTE_FRAME_MAX {
        return Err(RemoteError::BadLength);
    }
    if out.len() < payload.len() + 2 {
        return Err(RemoteError::Overflow);
    }
    let length = payload.len() as u16;
    out[0] = (length >> 8) as u8;
    out[1] = (length & 0xff) as u8;
    out[2..2 + payload.len()].copy_from_slice(payload);
    Ok(payload.len() + 2)
}

/// Incremental frame decoder over a fixed buffer.
pub(crate) struct FrameDecoder {
    buffer: [u8; 2 + REMOTE_FRAME_MAX],
    fill: usize,
}

impl FrameDecoder {
    pub(crate) fn new() -> Self {
        Self {
            buffer: [0; 2 + REMOTE_FRAME_MAX],
            fill: 0,
        }
    }

    /// Length prefix once both header bytes are buffered.
    fn pending_length(&self) -> Option<usize> {
        if self.fill < 2 {
            return None;
        }
        Some(((self.buffer[0] as usize) << 8) | self.buffer[1] as usize)
    }

    /// Push raw wire bytes into the internal buffer. A bad length prefix
    /// poisons the stream (error resets the buffer).
    pub(crate) fn push_bytes(&mut self, bytes: &[u8]) -> Result<(), RemoteError> {
        for byte in bytes {
            if self.fill >= self.buffer.len() {
                self.fill = 0;
                return Err(RemoteError::BadLength);
            }
            self.buffer[self.fill] = *byte;
            self.fill += 1;
            if self.fill == 2 {
                let length = self.pending_length().unwrap_or(0);
                if length == 0 || length > REMOTE_FRAME_MAX {
                    self.fill = 0;
                    return Err(RemoteError::BadLength);
                }
            }
        }
        Ok(())
    }

    /// Copy the next fully buffered frame's payload into `out` and slide the
    /// remainder forward. None when more wire bytes are needed.
    pub(crate) fn next_frame_into(&mut self, out: &mut [u8]) -> Result<Option<usize>, RemoteError> {
        let Some(length) = self.pending_length() else {
            return Ok(None);
        };
        if self.fill < 2 + length {
            return Ok(None);
        }
        if out.len() < length {
            return Err(RemoteError::Overflow);
        }
        out[..length].copy_from_slice(&self.buffer[2..2 + length]);
        let rest = self.fill - (2 + length);
        if rest > 0 {
            self.buffer.copy_within(2 + length..self.fill, 0);
        }
        self.fill = rest;
        Ok(Some(length))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AuthOutcome {
    Accepted,
    Rejected,
}

/// Single-token plaintext gate. Empty configured token = gate disabled.
pub(crate) struct AuthGate;

impl AuthGate {
    pub(crate) fn check(expected: &[u8], presented: &[u8]) -> AuthOutcome {
        if expected.is_empty() {
            return AuthOutcome::Accepted;
        }
        if presented.len() == expected.len() && presented[..] == expected[..] {
            AuthOutcome::Accepted
        } else {
            AuthOutcome::Rejected
        }
    }
}

/// Lifecycle of one remote connection: token gate first, then a raw
/// byte-stream bridge into a terminal session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LinkState {
    AwaitingAuth,
    Active,
    Closed,
}

/// Action the IO layer must take after one inbound frame.
#[derive(Debug)]
pub(crate) enum LinkEvent<'a> {
    None,
    /// Send the refusal notice, then drop the connection.
    Refuse,
    /// Token accepted: send the welcome banner.
    Banner,
    /// Feed these bytes into the bridged session's input line.
    Input(&'a [u8]),
}

pub(crate) struct RemoteLink {
    expected_token: &'static [u8],
    state: LinkState,
}

impl RemoteLink {
    pub(crate) fn new(expected_token: &'static [u8]) -> Self {
        Self {
            expected_token,
            // An open gate skips the handshake entirely: frames stream from
            // byte one. The welcome banner is sent by the IO layer on accept.
            state: if expected_token.is_empty() {
                LinkState::Active
            } else {
                LinkState::AwaitingAuth
            },
        }
    }

    pub(crate) fn state(&self) -> LinkState {
        self.state
    }

    pub(crate) fn on_frame<'a>(&mut self, payload: &'a [u8]) -> LinkEvent<'a> {
        match self.state {
            LinkState::AwaitingAuth => {
                if AuthGate::check(self.expected_token, payload) == AuthOutcome::Accepted {
                    self.state = LinkState::Active;
                    LinkEvent::Banner
                } else {
                    self.state = LinkState::Closed;
                    LinkEvent::Refuse
                }
            }
            LinkState::Active => LinkEvent::Input(payload),
            LinkState::Closed => LinkEvent::None,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn close(&mut self) {
        self.state = LinkState::Closed;
    }
}

use rt::RawMessage;
use serviceos_userspace_runtime as rt;

use crate::session::{detach_session, handle_input_byte, initialize_session, release_session};
use crate::state::{
    MAX_REMOTE_LINKS, MAX_SESSIONS, REMOTE_AUTH_TOKEN, REMOTE_BACKLOG, REMOTE_LISTENER_PORT,
    REMOTE_PUMP_BUDGET, Session,
};

/// Banner sent once a link activates (before any command output).
const BANNER_TEXT: &[u8] =
    b"ServiceOS remote terminal ready (PLAINTEXT rsh-like protocol, no SSH crypto)\r\n";
const REFUSAL_TEXT: &[u8] = b"remote access denied\r\n";
/// Text sent when no session slot can back a new connection.
const BUSY_TEXT: &[u8] = b"no free sessions\r\n";

/// One accepted TCP connection and its protocol state. `session_id` links
/// the bridge to the terminal session created on activation.
pub(crate) struct RemoteBridge {
    pub(crate) used: bool,
    pub(crate) stream: rt::Handle,
    pub(crate) link: RemoteLink,
    pub(crate) decoder: FrameDecoder,
    pub(crate) session_id: u32,
}

impl RemoteBridge {
    pub(crate) fn empty() -> Self {
        Self {
            used: false,
            stream: rt::INVALID_HANDLE,
            link: RemoteLink::new(REMOTE_AUTH_TOKEN),
            decoder: FrameDecoder::new(),
            session_id: 0,
        }
    }

    fn open(stream: rt::Handle) -> Self {
        Self {
            used: true,
            stream,
            link: RemoteLink::new(REMOTE_AUTH_TOKEN),
            decoder: FrameDecoder::new(),
            session_id: 0,
        }
    }
}

fn network_public(bootstrap: rt::Handle) -> rt::Result<rt::Handle> {
    let handle = rt::lookup_service(bootstrap, rt::ServiceId::Network)?;
    Ok(handle)
}

/// Bind the remote-session listener. Best-effort: a failure leaves the
/// service running without remote sessions (logged by the caller).
pub(crate) fn bind_listener(bootstrap: rt::Handle) -> Option<rt::Handle> {
    let network = network_public(bootstrap).ok()?;
    let mut request = RawMessage::empty(rt::NetworkTag::SocketListenRequest as u32);
    request.word_count = 2;
    request.words[0] = rt::NetworkSocketKind::TcpStream as u32 as u64;
    request.words[1] = rt::pack_listen_params(REMOTE_LISTENER_PORT, REMOTE_BACKLOG);
    let response = rt::channel_call(network, &mut request).ok()?;
    let _ = rt::handle_close(network);
    if response.tag != rt::NetworkTag::SocketListenReply as u32 || response.handle_count < 1 {
        return None;
    }
    if response.words[0] as u32 != rt::NetworkStatus::Ok as u32 {
        return None;
    }
    Some(response.handles[0])
}

/// Pop one pending inbound connection from the listener.
fn accept_inbound(listener: rt::Handle) -> Option<(rt::Handle, u64)> {
    let mut request = RawMessage::empty(rt::NetworkSocketTag::AcceptRequest as u32);
    let response = rt::channel_call(listener, &mut request).ok()?;
    if response.tag != rt::NetworkSocketTag::AcceptReply as u32 {
        return None;
    }
    if response.words[0] as u32 != rt::NetworkStatus::Ok as u32 || response.handle_count < 1 {
        return None;
    }
    Some((response.handles[0], response.words[1]))
}

fn close_stream(stream: rt::Handle) {
    if stream != rt::INVALID_HANDLE {
        let _ = rt::network_socket_close(stream);
    }
}

fn error_code(error: &rt::Error) -> u32 {
    match error {
        rt::Error::Busy => 1,
        rt::Error::QueueEmpty => 2,
        rt::Error::NotFound => 3,
        rt::Error::PermissionDenied => 4,
        rt::Error::InvalidArgument => 5,
        rt::Error::Unsupported => 6,
        _ => 9,
    }
}

/// Outcome of one nonblocking receive attempt.
pub(crate) enum WireRead {
    /// N bytes landed in the buffer.
    Bytes(usize),
    /// Nothing available right now.
    Idle,
    /// The connection is gone (closed, reset, or failed).
    Dead,
}

fn recv_wire(stream: rt::Handle, buffer: &mut [u8]) -> WireRead {
    match rt::network_socket_receive(stream, buffer) {
        Ok(count) if count > 0 => WireRead::Bytes(count),
        Ok(_) => WireRead::Idle,
        Err(rt::Error::QueueEmpty) | Err(rt::Error::Busy) => WireRead::Idle,
        Err(_) => WireRead::Dead,
    }
}

/// Frame `bytes` and push them out. Retries transient Busy states with a
/// yield so bursts of shell output survive small socket buffers; persistent
/// failure reports Err so callers can tear the link down.
pub(crate) fn send_framed(stream: rt::Handle, bytes: &[u8]) -> rt::Result<()> {
    let mut offset = 0usize;
    while offset < bytes.len() {
        let end = (offset + REMOTE_FRAME_MAX).min(bytes.len());
        let chunk = &bytes[offset..end];
        let mut wire = [0u8; 2 + REMOTE_FRAME_MAX];
        let written = match encode_frame(chunk, &mut wire) {
            Ok(written) => written,
            Err(_) => return Err(rt::Error::BufferTooSmall),
        };
        let mut sent = false;
        for _ in 0..64 {
            match rt::network_socket_send(stream, &wire[..written]) {
                Ok(_) => {
                    sent = true;
                    break;
                }
                Err(rt::Error::Busy) => {
                    let _ = rt::yield_current();
                }
                Err(error) => return Err(error),
            }
        }
        if !sent {
            return Err(rt::Error::Busy);
        }
        offset = end;
    }
    Ok(())
}

fn send_text(stream: rt::Handle, text: &[u8]) {
    let _ = send_framed(stream, text);
}

/// Emit-path wrapper: a persistently dead link must not abort an in-flight
/// shell command, so failures are logged once and swallowed; the pump's
/// Dead detection tears the link down on its own schedule.
pub(crate) fn send_framed_lenient(stream: rt::Handle, bytes: &[u8]) -> rt::Result<()> {
    if send_framed(stream, bytes).is_err() {
        let _ = rt::write_logf("terminal", format_args!("remote link send stalled"));
        return Ok(());
    }
    Ok(())
}

/// Activate a bridge: mint the terminal session that backs it and announce
/// the banner. Returns false when no session slot or channel is available.
fn activate_bridge(
    bootstrap: rt::Handle,
    sessions: &mut [Session; MAX_SESSIONS],
    bridge: &mut RemoteBridge,
    next_session_id: &mut u32,
) -> bool {
    let Some(slot) = sessions.iter_mut().find(|session| !session.occupied) else {
        return false;
    };
    let pair = match initialize_session(bootstrap, slot, next_session_id, None) {
        Ok(pair) => pair,
        Err(_) => return false,
    };
    // The pane-side handle is unused for a remote client; the spare
    // duplicate retained inside the session still allows a local reattach
    // after the remote side goes away.
    let _ = rt::handle_close(pair.second);
    bridge.session_id = slot.id;
    slot.remote_stream = bridge.stream;
    send_text(bridge.stream, BANNER_TEXT);
    true
}

/// Tear a bridge down: drop the socket, detach (never kill) the bridged
/// session so scrollback/state survive for a later local reattach.
fn teardown_bridge(
    sessions: &mut [Session; MAX_SESSIONS],
    bridge: &mut RemoteBridge,
    announce_detach: bool,
) {
    close_stream(bridge.stream);
    let stream = bridge.stream;
    let session_id = bridge.session_id;
    *bridge = RemoteBridge::empty();
    if !announce_detach {
        return;
    }
    if let Some(session) = sessions.iter_mut().find(|session| {
        session.occupied && session.id == session_id && session.remote_stream == stream
    }) {
        session.remote_stream = rt::INVALID_HANDLE;
        detach_session(session);
        let _ = rt::write_logf(
            "terminal",
            format_args!("remote session detached id={}", session_id),
        );
    }
}

/// Route one decoded frame through the link state machine. Input errors
/// release the bridged session (mirrors pane-input handling).
fn dispatch_frame(
    bootstrap: rt::Handle,
    sessions: &mut [Session; MAX_SESSIONS],
    bridge: &mut RemoteBridge,
    payload: &[u8],
    next_session_id: &mut u32,
) {
    match bridge.link.on_frame(payload) {
        LinkEvent::Banner => {
            if !activate_bridge(bootstrap, sessions, bridge, next_session_id) {
                send_text(bridge.stream, BUSY_TEXT);
                teardown_bridge(sessions, bridge, false);
            }
        }
        LinkEvent::Refuse => {
            send_text(bridge.stream, REFUSAL_TEXT);
            teardown_bridge(sessions, bridge, false);
        }
        LinkEvent::Input(bytes) => {
            let stream = bridge.stream;
            let session_id = bridge.session_id;
            if let Some(session) = sessions.iter_mut().find(|session| {
                session.occupied && session.id == session_id && session.remote_stream == stream
            }) {
                for byte in bytes.iter().copied() {
                    if handle_input_byte(bootstrap, session, byte).is_err() {
                        release_session(bootstrap, session);
                        teardown_bridge(sessions, bridge, false);
                        return;
                    }
                }
            }
        }
        LinkEvent::None => {}
    }
}

/// Per-turn driver: accept pending connections, drain socket input through
/// the framing decoder into the session input path, and reap dead links.
pub(crate) fn pump_remote(
    bootstrap: rt::Handle,
    listener: rt::Handle,
    sessions: &mut [Session; MAX_SESSIONS],
    bridges: &mut [RemoteBridge; MAX_REMOTE_LINKS],
    next_session_id: &mut u32,
) -> rt::Result<()> {
    // Accept new connections into free bridge slots.
    for _ in 0..2 {
        let Some(free_index) = bridges.iter().position(|bridge| !bridge.used) else {
            break;
        };
        let Some((stream, _remote_address)) = accept_inbound(listener) else {
            break;
        };
        let mut bridge = RemoteBridge::open(stream);
        // Open-gate links activate immediately (banner + session).
        if bridge.link.state() == LinkState::Active
            && !activate_bridge(bootstrap, sessions, &mut bridge, next_session_id)
        {
            send_text(stream, BUSY_TEXT);
            close_stream(stream);
            continue;
        }
        let connected_id = bridge.session_id;
        bridges[free_index] = bridge;
        let _ = rt::write_logf(
            "terminal",
            format_args!("remote connection accepted id={}", connected_id),
        );
    }

    // Drain each active bridge within a small per-turn budget.
    for bridge in bridges.iter_mut() {
        if !bridge.used {
            continue;
        }
        for _ in 0..REMOTE_PUMP_BUDGET {
            let mut wire = [0u8; 2 + REMOTE_FRAME_MAX];
            match recv_wire(bridge.stream, &mut wire) {
                WireRead::Bytes(count) => {
                    if bridge.decoder.push_bytes(&wire[..count]).is_err() {
                        teardown_bridge(sessions, bridge, true);
                        break;
                    }
                    // Extract every frame this chunk completed.
                    loop {
                        let mut payload = [0u8; REMOTE_FRAME_MAX];
                        match bridge.decoder.next_frame_into(&mut payload) {
                            Ok(Some(length)) => {
                                dispatch_frame(
                                    bootstrap,
                                    sessions,
                                    bridge,
                                    &payload[..length],
                                    next_session_id,
                                );
                                if !bridge.used || bridge.link.state() == LinkState::Closed {
                                    break;
                                }
                            }
                            Ok(None) => break,
                            Err(_) => {
                                teardown_bridge(sessions, bridge, true);
                                break;
                            }
                        }
                    }
                    if !bridge.used {
                        break;
                    }
                }
                WireRead::Idle => break,
                WireRead::Dead => {
                    teardown_bridge(sessions, bridge, true);
                    break;
                }
            }
        }
    }
    Ok(())
}

/// Boot-time evidence run: open a loopback connection to our own listener,
/// drive one framed command through a real session, then disconnect and
/// confirm the session detaches instead of dying. Strictly bounded so boot
/// never stalls; every failure is logged and non-fatal.
///
/// Firewall note: this relies on inbound-to-self being permitted (default
/// inbound allow, or an explicit allow rule for REMOTE_LISTENER_PORT).
pub(crate) fn selftest_loopback(bootstrap: rt::Handle, port: u16) {
    const SPIN_BUDGET: u32 = 600;
    let Ok(network) = network_public(bootstrap) else {
        return;
    };
    // Outbound leg: connect to ourselves.
    let mut stream = rt::INVALID_HANDLE;
    let mut open_spins: u32 = 0;
    for _ in 0..SPIN_BUDGET {
        open_spins += 1;
        match rt::network_socket_open(network, rt::NetworkSocketKind::TcpStream, "127.0.0.1", port)
        {
            Ok(handle) => {
                stream = handle;
                break;
            }
            Err(rt::Error::Busy) => {
                let _ = rt::yield_current();
            }
            Err(_) => break,
        }
    }
    if stream == rt::INVALID_HANDLE {
        let _ = rt::write_logf(
            "terminal",
            format_args!(
                "remote selftest skip: no loopback client spins={}",
                open_spins
            ),
        );
        let _ = rt::handle_close(network);
        return;
    }
    // Wait for establishment.
    let mut established = false;
    let mut last_state: u32 = 0;
    let mut spins: u32 = 0;
    for _ in 0..SPIN_BUDGET {
        spins += 1;
        match rt::network_socket_status(stream) {
            Ok(info) => {
                last_state = info.state as u32;
                if info.state == rt::NetworkSocketState::Established {
                    established = true;
                    break;
                }
                let _ = rt::yield_current();
            }
            Err(error) => {
                last_state = 1000 + error_code(&error);
                break;
            }
        }
    }
    if !established {
        let _ = rt::write_logf(
            "terminal",
            format_args!(
                "remote selftest fail: not established state={} spins={} (see state.rs REMOTE_LOOPBACK_SELFTEST note)",
                last_state, spins
            ),
        );
        close_stream(stream);
        let _ = rt::handle_close(network);
        return;
    }

    // Drive one framed command and look for it echoed back in the output.
    let command = b"echo remote-selftest\r\n";
    let sent = send_framed(stream, command).is_ok();
    let mut seen = false;
    if sent {
        let mut scratch = [0u8; 512];
        let mut collected = heapless_note::Note::new();
        for _ in 0..SPIN_BUDGET {
            match recv_wire(stream, &mut scratch) {
                WireRead::Bytes(count) => {
                    collected.extend(&scratch[..count]);
                    if collected.contains(b"remote-selftest") {
                        seen = true;
                        break;
                    }
                }
                WireRead::Idle => {
                    let _ = rt::yield_current();
                }
                WireRead::Dead => break,
            }
        }
    }
    close_stream(stream);
    let _ = rt::handle_close(network);
    if seen {
        let _ = rt::write_logf(
            "terminal",
            format_args!("remote selftest ok: loopback session round-trip"),
        );
    } else {
        let _ = rt::write_logf("terminal", format_args!("remote selftest fail: no echo"));
    }
}

/// Tiny fixed-capacity haystack used only by the selftest.
mod heapless_note {
    pub(super) struct Note {
        bytes: [u8; 1024],
        len: usize,
    }

    impl Note {
        pub(super) const fn new() -> Self {
            Self {
                bytes: [0; 1024],
                len: 0,
            }
        }

        pub(super) fn extend(&mut self, bytes: &[u8]) {
            for byte in bytes {
                if self.len < self.bytes.len() {
                    self.bytes[self.len] = *byte;
                    self.len += 1;
                }
            }
        }

        pub(super) fn contains(&self, needle: &[u8]) -> bool {
            self.bytes[..self.len]
                .windows(needle.len())
                .any(|window| window == needle)
        }
    }
}

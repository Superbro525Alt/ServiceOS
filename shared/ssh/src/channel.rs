//! RFC 4254 channel layer — one interactive "session" channel (v0).
//!
//! The server accepts a peer-initiated CHANNEL_OPEN of type "session",
//! answers pty-req with CHANNEL_SUCCESS (dimensions recorded), "shell" with
//! CHANNEL_SUCCESS, and shuttles CHANNEL_DATA both directions with honest
//! window accounting:
//!
//! - Outgoing (server → peer): the peer's initial window from the open
//!   confirmation is the budget; sends decrement it, peer WINDOW_ADJUST
//!   refills it. Output that does not fit waits in a small fixed buffer
//!   (`PENDING_OUT_CAP`) and is flushed in `CHUNK`-sized packets by later
//!   sends/adjusts; overflow past the buffer truncates (interactive
//!   line-mode output stays far below the cap — the bound is documented,
//!   not hidden).
//! - Incoming (peer → server): we advertise [`LOCAL_RECV_WINDOW`]; every
//!   delivered batch is acknowledged with WINDOW_ADJUST once the host
//!   consumes it via [`SshTransport::ack_channel_data`]. v0 delivers
//!   whatever arrives (line-mode traffic is tiny) and never gates on the
//!   advertised window, so a pump that acks promptly cannot wedge.
//!
//! Everything else is refused honestly: non-session channel opens get
//! OPEN_FAILURE (administratively prohibited), exec/subsystem/env requests
//! get CHANNEL_FAILURE (this surface is a line-mode shell only), global
//! requests with want_reply get REQUEST_FAILURE (keeps client keepalives
//! honest). EOF and CHANNEL_CLOSE propagate in both directions.
//!
//! Handler style: message fields are parsed out of the transport's staging
//! buffer by free functions into owned small values, so the `&mut self`
//! actions never fight the parse borrow.

use crate::error::{DisconnectReason, Fail};
use crate::transport::SshTransport;
use crate::wire::{Reader, Writer};

pub const SSH_MSG_GLOBAL_REQUEST: u8 = 80;
pub const SSH_MSG_REQUEST_FAILURE: u8 = 82;
pub const SSH_MSG_CHANNEL_OPEN: u8 = 90;
pub const SSH_MSG_CHANNEL_OPEN_CONFIRMATION: u8 = 91;
pub const SSH_MSG_CHANNEL_OPEN_FAILURE: u8 = 92;
pub const SSH_MSG_CHANNEL_WINDOW_ADJUST: u8 = 93;
pub const SSH_MSG_CHANNEL_DATA: u8 = 94;
pub const SSH_MSG_CHANNEL_EOF: u8 = 96;
pub const SSH_MSG_CHANNEL_CLOSE: u8 = 97;
pub const SSH_MSG_CHANNEL_REQUEST: u8 = 98;
pub const SSH_MSG_CHANNEL_SUCCESS: u8 = 99;
pub const SSH_MSG_CHANNEL_FAILURE: u8 = 100;

/// Receive window advertised for peer → server data. The pump acknowledges
/// every delivered batch immediately, so this never exhausts in practice.
pub const LOCAL_RECV_WINDOW: u32 = 16384;
/// Buffer for outgoing data held back by the peer's window.
const PENDING_OUT_CAP: usize = 4096;
/// Largest outbound data chunk per packet (stack scratch stays small).
const CHUNK: usize = 1024;
/// Server-side channel id (single channel; fixed for v0).
const LOCAL_ID: u32 = 0;
/// Open-failure reason: administrative prohibition (RFC 4254 §5.1).
const OPEN_ADMIN_PROHIBITED: u32 = 1;
/// Header of a CHANNEL_DATA payload: type + recipient + string length.
const DATA_HEADER: usize = 9;
/// Inbound batches above this are refused: no line-mode client legitimately
/// sends a 4 KiB keystroke burst, and the pump's line buffer is bounded.
const MAX_INBOUND_BATCH: usize = 4096;

/// Parsed CHANNEL_OPEN: sender id + initial window, session type only.
struct ParsedOpen {
    sender: u32,
    initial_window: u32,
}

/// Parsed CHANNEL_REQUEST action.
enum ParsedRequest {
    Pty { width: u32, height: u32 },
    Shell,
    Refused,
}

fn parse_open(payload: &[u8]) -> Option<ParsedOpen> {
    let mut r = Reader::new(&payload[1..]);
    let kind = r.string().ok()?;
    let sender = r.u32().ok()?;
    let initial_window = r.u32().ok()?;
    if kind != b"session" {
        return None;
    }
    Some(ParsedOpen {
        sender,
        initial_window,
    })
}

/// Non-session open: still need the sender id to answer OPEN_FAILURE.
fn parse_open_sender(payload: &[u8]) -> Option<u32> {
    let mut r = Reader::new(&payload[1..]);
    let _kind = r.string().ok()?;
    r.u32().ok()
}

fn parse_request(payload: &[u8]) -> Option<ParsedRequest> {
    let mut r = Reader::new(&payload[1..]);
    let _recipient = r.u32().ok()?;
    let name = r.string().ok()?;
    let _want_reply = r.u8().ok()?;
    match name {
        b"pty-req" => {
            let _term = r.string().ok()?;
            let width = r.u32().ok()?;
            let height = r.u32().ok()?;
            let _xpixels = r.u32().ok()?;
            let _ypixels = r.u32().ok()?;
            let _modes = r.string().ok()?;
            Some(ParsedRequest::Pty { width, height })
        }
        b"shell" => Some(ParsedRequest::Shell),
        // exec / env / subsystem / x11 / agent / signals: refused.
        _ => Some(ParsedRequest::Refused),
    }
}

fn parse_recipient_u32(payload: &[u8]) -> Option<(u32, u32)> {
    // (recipient, extra u32) shared by WINDOW_ADJUST; EOF/CLOSE only need
    // the first word and ignore the rest.
    let mut r = Reader::new(&payload[1..]);
    let recipient = r.u32().ok()?;
    let extra = r.u32().unwrap_or(0);
    Some((recipient, extra))
}

fn parse_data_header(payload: &[u8]) -> Option<(u32, usize)> {
    let mut r = Reader::new(&payload[1..]);
    let recipient = r.u32().ok()?;
    let data = r.string().ok()?;
    Some((recipient, data.len()))
}

fn parse_global_request(payload: &[u8]) -> Option<bool> {
    let mut r = Reader::new(&payload[1..]);
    let _name = r.string().ok()?;
    let want_reply = r.u8().ok()?;
    Some(want_reply != 0)
}

/// Per-message outcome the transport dispatcher acts on.
/// Outcome for the simple (no-payload-out) channel messages.
pub(crate) enum ChanSimple {
    Handled,
    Passthrough,
}

pub(crate) enum ChannelOutcome {
    /// Fully handled inside the channel layer.
    Handled,
    /// A CHANNEL_DATA batch is ready for the host; the dispatcher hands out
    /// the slice (still resident in the transport's staging buffer).
    Data(usize),
    /// Not addressed to us and not ours to swallow (client test-helper
    /// observing peer traffic) — treat like an unknown message type.
    Passthrough,
}

#[derive(Debug)]
pub(crate) struct ChannelState {
    open: bool,
    remote_id: u32,
    remote_window: u32,
    pty: bool,
    pty_width: u32,
    pty_height: u32,
    shell: bool,
    eof_in: bool,
    eof_out: bool,
    close_in: bool,
    close_out: bool,
    pending_out: [u8; PENDING_OUT_CAP],
    pending_len: usize,
}

impl ChannelState {
    pub(crate) const fn new() -> ChannelState {
        ChannelState {
            open: false,
            remote_id: 0,
            remote_window: 0,
            pty: false,
            pty_width: 0,
            pty_height: 0,
            shell: false,
            eof_in: false,
            eof_out: false,
            close_in: false,
            close_out: false,
            pending_out: [0; PENDING_OUT_CAP],
            pending_len: 0,
        }
    }
}

impl SshTransport {
    // ------------------------------------------------------------------
    // Inbound message handlers (payload slice into the staging buffer)
    // ------------------------------------------------------------------

    pub(crate) fn handle_channel_open(&mut self, start: usize, len: usize) -> Result<(), Fail> {
        let body = &self.proc[start..start + len];
        match parse_open(body) {
            Some(open) => {
                if self.chan.open {
                    return Err(self.fail_disconnect(
                        DisconnectReason::ProtocolError,
                        "duplicate session channel (single-session v0)",
                    ));
                }
                self.chan.open = true;
                self.chan.remote_id = open.sender;
                self.chan.remote_window = open.initial_window;
                let mut reply = [0u8; 17];
                reply[0] = SSH_MSG_CHANNEL_OPEN_CONFIRMATION;
                reply[1..5].copy_from_slice(&open.sender.to_be_bytes());
                reply[5..9].copy_from_slice(&LOCAL_ID.to_be_bytes());
                reply[9..13].copy_from_slice(&LOCAL_RECV_WINDOW.to_be_bytes());
                reply[13..17]
                    .copy_from_slice(&(crate::packet::MAX_PAYLOAD_LEN as u32).to_be_bytes());
                self.emit_packet(&reply)
            }
            None => {
                let sender = parse_open_sender(body).ok_or_else(|| {
                    self.fail_disconnect(DisconnectReason::ProtocolError, "malformed CHANNEL_OPEN")
                })?;
                let mut reply = [0u8; 60];
                reply[0] = SSH_MSG_CHANNEL_OPEN_FAILURE;
                reply[1..5].copy_from_slice(&sender.to_be_bytes());
                reply[5..9].copy_from_slice(&OPEN_ADMIN_PROHIBITED.to_be_bytes());
                let n = {
                    let mut w = Writer::new(&mut reply[9..]);
                    let _ = w.string(b"only session channels are supported");
                    let _ = w.string(b"");
                    9 + w.into_written()
                };
                self.emit_packet(&reply[..n])
            }
        }
    }

    pub(crate) fn handle_channel_request(&mut self, start: usize, len: usize) -> Result<(), Fail> {
        let recipient = Reader::new(&self.proc[start + 1..start + len]).u32();
        let recipient = match recipient {
            Ok(v) => v,
            Err(_) => {
                return Err(self.fail_disconnect(
                    DisconnectReason::ProtocolError,
                    "malformed CHANNEL_REQUEST",
                ));
            }
        };
        if recipient != LOCAL_ID {
            return Err(self.fail_disconnect(
                DisconnectReason::ProtocolError,
                "channel request for unknown channel",
            ));
        }
        // want_reply byte (sits after recipient+name) decides whether we
        // answer; parse the full shape first.
        let parsed = {
            let body = &self.proc[start..start + len];
            match parse_request(body) {
                Some(p) => p,
                None => {
                    return Err(self.fail_disconnect(
                        DisconnectReason::ProtocolError,
                        "malformed CHANNEL_REQUEST",
                    ));
                }
            }
        };
        let want_reply = {
            let body = &self.proc[start..start + len];
            request_want_reply(body).unwrap_or(0)
        };
        let success = match parsed {
            ParsedRequest::Pty { width, height } => {
                self.chan.pty = true;
                self.chan.pty_width = width;
                self.chan.pty_height = height;
                true
            }
            ParsedRequest::Shell => {
                self.chan.shell = true;
                true
            }
            ParsedRequest::Refused => false,
        };
        if want_reply == 0 {
            return Ok(());
        }
        let mut reply = [0u8; 5];
        reply[0] = if success {
            SSH_MSG_CHANNEL_SUCCESS
        } else {
            SSH_MSG_CHANNEL_FAILURE
        };
        reply[1..5].copy_from_slice(&self.chan.remote_id.to_be_bytes());
        self.emit_packet(&reply)
    }

    /// CHANNEL_DATA: validate, then report the batch length for delivery.
    pub(crate) fn try_channel_data(
        &mut self,
        start: usize,
        len: usize,
    ) -> Result<ChannelOutcome, Fail> {
        let body = &self.proc[start..start + len];
        let (recipient, data_len) = match parse_data_header(body) {
            Some(v) => v,
            None => {
                return Err(
                    self.fail_disconnect(DisconnectReason::ProtocolError, "malformed CHANNEL_DATA")
                );
            }
        };
        if recipient != LOCAL_ID {
            if self.role() == crate::transport::Role::Client {
                // The client role is a test helper: hand peer traffic
                // through instead of swallowing it.
                return Ok(ChannelOutcome::Passthrough);
            }
            return Ok(ChannelOutcome::Handled);
        }
        if !self.chan.open || self.chan.close_in {
            return Ok(ChannelOutcome::Handled);
        }
        if data_len > MAX_INBOUND_BATCH {
            return Err(
                self.fail_disconnect(DisconnectReason::ProtocolError, "oversized CHANNEL_DATA")
            );
        }
        Ok(ChannelOutcome::Data(data_len))
    }

    pub(crate) fn handle_window_adjust(
        &mut self,
        start: usize,
        len: usize,
    ) -> Result<ChanSimple, Fail> {
        let body = &self.proc[start..start + len];
        let Some((recipient, bytes)) = parse_recipient_u32(body) else {
            return Err(
                self.fail_disconnect(DisconnectReason::ProtocolError, "malformed WINDOW_ADJUST")
            );
        };
        if recipient != LOCAL_ID {
            return Ok(self.client_passthrough());
        }
        self.chan.remote_window = self.chan.remote_window.saturating_add(bytes);
        self.flush_pending_out()?;
        Ok(ChanSimple::Handled)
    }

    pub(crate) fn handle_channel_eof(
        &mut self,
        start: usize,
        len: usize,
    ) -> Result<ChanSimple, Fail> {
        let body = &self.proc[start..start + len];
        let Some((recipient, _)) = parse_recipient_u32(body) else {
            return Err(
                self.fail_disconnect(DisconnectReason::ProtocolError, "malformed CHANNEL_EOF")
            );
        };
        if recipient == LOCAL_ID && self.chan.open {
            self.chan.eof_in = true;
            return Ok(ChanSimple::Handled);
        }
        Ok(self.client_passthrough())
    }

    pub(crate) fn handle_channel_close(
        &mut self,
        start: usize,
        len: usize,
    ) -> Result<ChanSimple, Fail> {
        let body = &self.proc[start..start + len];
        let Some((recipient, _)) = parse_recipient_u32(body) else {
            return Err(
                self.fail_disconnect(DisconnectReason::ProtocolError, "malformed CHANNEL_CLOSE")
            );
        };
        if recipient != LOCAL_ID {
            return Ok(self.client_passthrough());
        }
        self.chan.close_in = true;
        if !self.chan.close_out {
            self.chan.close_out = true;
            let mut reply = [0u8; 5];
            reply[0] = SSH_MSG_CHANNEL_CLOSE;
            reply[1..5].copy_from_slice(&self.chan.remote_id.to_be_bytes());
            self.emit_packet(&reply)?;
        }
        Ok(ChanSimple::Handled)
    }

    pub(crate) fn handle_global_request(&mut self, start: usize, len: usize) -> Result<(), Fail> {
        let body = &self.proc[start..start + len];
        let Some(want_reply) = parse_global_request(body) else {
            return Err(
                self.fail_disconnect(DisconnectReason::ProtocolError, "malformed GLOBAL_REQUEST")
            );
        };
        // v0 supports no global requests; all fail honestly.
        if want_reply {
            self.emit_packet(&[SSH_MSG_REQUEST_FAILURE])?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Host-facing API (the sshd pump)
    // ------------------------------------------------------------------

    /// The session channel is ready for the shell bridge (open + shell
    /// request confirmed; authentication is checked by the caller's
    /// `auth_phase`).
    pub fn channel_ready(&self) -> bool {
        self.chan.open && self.chan.shell
    }

    /// Peer half-closed its side (EOF) — the host should wind the shell
    /// session down.
    pub fn channel_eof_in(&self) -> bool {
        self.chan.eof_in
    }

    /// The peer closed the channel.
    pub fn channel_closed(&self) -> bool {
        !self.chan.open || self.chan.close_in
    }

    /// Acknowledge `n` delivered inbound bytes (WINDOW_ADJUST back to the
    /// peer). RFC 4254 §5.2 shape: recipient channel + bytes to add — no
    /// sender channel field on this message.
    pub fn ack_channel_data(&mut self, n: usize) -> Result<(), Fail> {
        if n == 0 || !self.chan.open || self.chan.close_out {
            return Ok(());
        }
        let mut payload = [0u8; 9];
        payload[0] = SSH_MSG_CHANNEL_WINDOW_ADJUST;
        payload[1..5].copy_from_slice(&self.chan.remote_id.to_be_bytes());
        payload[5..9].copy_from_slice(&(n as u32).to_be_bytes());
        self.emit_packet(&payload)
    }

    /// Queue server → peer channel data. Returns the number of bytes
    /// accepted (sent or held in the pending buffer); overflow past
    /// `PENDING_OUT_CAP` truncates (documented v0 bound).
    pub fn send_channel_data(&mut self, data: &[u8]) -> Result<usize, Fail> {
        if !self.chan.open || self.chan.close_out {
            return Err(Fail::NotReady);
        }
        let room = PENDING_OUT_CAP - self.chan.pending_len;
        let accepted = data.len().min(room);
        let start = self.chan.pending_len;
        self.chan.pending_out[start..start + accepted].copy_from_slice(&data[..accepted]);
        self.chan.pending_len += accepted;
        self.flush_pending_out()?;
        Ok(accepted)
    }

    /// Send EOF for the channel (idempotent).
    pub fn send_channel_eof(&mut self) -> Result<(), Fail> {
        if !self.chan.open || self.chan.eof_out {
            return Ok(());
        }
        self.chan.eof_out = true;
        let mut payload = [0u8; 5];
        payload[0] = SSH_MSG_CHANNEL_EOF;
        payload[1..5].copy_from_slice(&self.chan.remote_id.to_be_bytes());
        self.emit_packet(&payload)
    }

    /// Close the channel from our side (idempotent); the peer's reply (or
    /// the session teardown) completes the exchange.
    pub fn send_channel_close(&mut self) -> Result<(), Fail> {
        if !self.chan.open || self.chan.close_out {
            return Ok(());
        }
        self.chan.close_out = true;
        let mut payload = [0u8; 5];
        payload[0] = SSH_MSG_CHANNEL_CLOSE;
        payload[1..5].copy_from_slice(&self.chan.remote_id.to_be_bytes());
        self.emit_packet(&payload)
    }

    /// Recorded PTY dimensions (0 when no pty-req arrived).
    pub fn pty_size(&self) -> (u32, u32) {
        (self.chan.pty_width, self.chan.pty_height)
    }

    // ------------------------------------------------------------------
    // Internals
    // ------------------------------------------------------------------

    /// Flush held output: one bounded packet per call while the peer's
    /// window allows; the remainder flushes on later pump passes.
    /// Client test-helper observation: messages not addressed to the
    /// client's own channel id pass through instead of being swallowed.
    fn client_passthrough(&self) -> ChanSimple {
        if self.role() == crate::transport::Role::Client {
            ChanSimple::Passthrough
        } else {
            ChanSimple::Handled
        }
    }

    fn flush_pending_out(&mut self) -> Result<(), Fail> {
        if self.chan.pending_len == 0 || self.chan.remote_window == 0 {
            return Ok(());
        }
        let chunk = self
            .chan
            .pending_len
            .min(CHUNK)
            .min(self.chan.remote_window as usize);
        let mut payload = [0u8; DATA_HEADER + CHUNK];
        payload[0] = SSH_MSG_CHANNEL_DATA;
        payload[1..5].copy_from_slice(&self.chan.remote_id.to_be_bytes());
        payload[5..9].copy_from_slice(&(chunk as u32).to_be_bytes());
        payload[DATA_HEADER..DATA_HEADER + chunk].copy_from_slice(&self.chan.pending_out[..chunk]);
        self.emit_packet(&payload[..DATA_HEADER + chunk])?;
        self.chan.remote_window -= chunk as u32;
        self.chan
            .pending_out
            .copy_within(chunk..self.chan.pending_len, 0);
        self.chan.pending_len -= chunk;
        Ok(())
    }
}

/// want_reply byte of a CHANNEL_REQUEST (sits after recipient+name+…: it is
/// the byte right before the request-type-specific payload, which for our
/// purposes is whatever follows the name string).
fn request_want_reply(body: &[u8]) -> Option<u8> {
    let mut r = Reader::new(&body[1..]);
    let _recipient = r.u32().ok()?;
    let _name = r.string().ok()?;
    r.u8().ok()
}

#[cfg(test)]
mod tests {
    use crate::channel::*;
    use crate::testkit::*;
    use crate::transport::{Feed, Role, State};
    use crate::wire::Writer;

    /// Service accept + password ok: fully authenticated pair.
    fn authenticated_pair() -> (SshTransport, SshTransport) {
        let (mut s, mut c) = established_pair();
        // service request
        let mut p = [0u8; 24];
        p[0] = 5;
        p[1..5].copy_from_slice(&12u32.to_be_bytes());
        p[5..17].copy_from_slice(b"ssh-userauth");
        c.send_payload(&p).unwrap();
        s.feed(&drain(&mut c)).unwrap();
        c.feed(&drain(&mut s)).unwrap();
        // password request + verdict
        let mut payload = [0u8; 128];
        let len = craft_password_request(b"admin", b"secret", &mut payload);
        c.send_payload(&payload[..len]).unwrap();
        assert!(matches!(s.feed(&drain(&mut c)).unwrap(), Feed::AuthQuery));
        s.auth_verdict(true).unwrap();
        c.feed(&drain(&mut s)).unwrap();
        (s, c)
    }

    /// Client opens a session channel (sender id 7); server confirms.
    fn open_session(s: &mut SshTransport, c: &mut SshTransport) {
        let mut open = [0u8; 32];
        let open_len;
        {
            let mut w = Writer::new(&mut open);
            w.u8(SSH_MSG_CHANNEL_OPEN).unwrap();
            w.string(b"session").unwrap();
            w.u32(7).unwrap();
            w.u32(65536).unwrap();
            w.u32(32768).unwrap();
            open_len = w.into_written();
        }
        c.send_payload(&open[..open_len]).unwrap();
        s.feed(&drain(&mut *c)).unwrap();
        match c.feed(&drain(&mut *s)).unwrap() {
            Feed::Packet { msg_type, payload } => {
                assert_eq!(msg_type, SSH_MSG_CHANNEL_OPEN_CONFIRMATION);
                assert_eq!(&payload[1..5], &7u32.to_be_bytes());
                assert_eq!(&payload[5..9], &0u32.to_be_bytes());
                assert_eq!(&payload[9..13], &LOCAL_RECV_WINDOW.to_be_bytes());
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    /// Client asks for pty + shell; server acks both.
    fn pty_and_shell(s: &mut SshTransport, c: &mut SshTransport) {
        let mut pty = [0u8; 80];
        let pty_len;
        {
            let mut w = Writer::new(&mut pty);
            w.u8(SSH_MSG_CHANNEL_REQUEST).unwrap();
            w.u32(0).unwrap();
            w.string(b"pty-req").unwrap();
            w.u8(1).unwrap();
            w.string(b"xterm-256color").unwrap();
            w.u32(120).unwrap();
            w.u32(40).unwrap();
            w.u32(0).unwrap();
            w.u32(0).unwrap();
            w.string(b"").unwrap();
            pty_len = w.into_written();
        }
        c.send_payload(&pty[..pty_len]).unwrap();
        s.feed(&drain(&mut *c)).unwrap();
        assert_eq!(s.pty_size(), (120, 40));
        match c.feed(&drain(&mut *s)).unwrap() {
            Feed::Packet { msg_type, .. } => assert_eq!(msg_type, SSH_MSG_CHANNEL_SUCCESS),
            other => panic!("unexpected {:?}", other),
        }
        let mut shell = [0u8; 16];
        let shell_len;
        {
            let mut w = Writer::new(&mut shell);
            w.u8(SSH_MSG_CHANNEL_REQUEST).unwrap();
            w.u32(0).unwrap();
            w.string(b"shell").unwrap();
            w.u8(1).unwrap();
            shell_len = w.into_written();
        }
        c.send_payload(&shell[..shell_len]).unwrap();
        s.feed(&drain(&mut *c)).unwrap();
        match c.feed(&drain(&mut *s)).unwrap() {
            Feed::Packet { msg_type, .. } => assert_eq!(msg_type, SSH_MSG_CHANNEL_SUCCESS),
            other => panic!("unexpected {:?}", other),
        }
        assert!(s.channel_ready());
    }

    #[test]
    fn open_pty_shell_happy_path() {
        let (mut s, mut c) = authenticated_pair();
        assert!(!s.channel_ready());
        open_session(&mut s, &mut c);
        assert!(!s.channel_ready());
        pty_and_shell(&mut s, &mut c);
        assert!(s.channel_ready());
    }

    #[test]
    fn non_session_open_gets_open_failure() {
        let (mut s, mut c) = authenticated_pair();
        let mut open = [0u8; 48];
        let open_len;
        {
            let mut w = Writer::new(&mut open);
            w.u8(SSH_MSG_CHANNEL_OPEN).unwrap();
            w.string(b"direct-tcpip").unwrap();
            w.u32(9).unwrap();
            w.u32(1024).unwrap();
            w.u32(1024).unwrap();
            w.string(b"host").unwrap();
            open_len = w.into_written();
        }
        c.send_payload(&open[..open_len]).unwrap();
        s.feed(&drain(&mut c)).unwrap();
        match c.feed(&drain(&mut s)).unwrap() {
            Feed::Packet { msg_type, payload } => {
                assert_eq!(msg_type, SSH_MSG_CHANNEL_OPEN_FAILURE);
                assert_eq!(&payload[1..5], &9u32.to_be_bytes());
                assert_eq!(&payload[5..9], &OPEN_ADMIN_PROHIBITED_BE.to_be_bytes());
            }
            other => panic!("unexpected {:?}", other),
        }
        // The rejection must not consume the single session slot.
        open_session(&mut s, &mut c);
        assert_eq!(s.pty_size(), (0, 0));
    }

    #[test]
    fn duplicate_session_open_disconnects() {
        let (mut s, mut c) = authenticated_pair();
        open_session(&mut s, &mut c);
        let mut open = [0u8; 32];
        let open_len;
        {
            let mut w = Writer::new(&mut open);
            w.u8(SSH_MSG_CHANNEL_OPEN).unwrap();
            w.string(b"session").unwrap();
            w.u32(8).unwrap();
            w.u32(65536).unwrap();
            w.u32(32768).unwrap();
            open_len = w.into_written();
        }
        c.send_payload(&open[..open_len]).unwrap();
        assert!(matches!(
            s.feed(&drain(&mut c)).unwrap_err(),
            crate::error::Fail::LocalDisconnect {
                reason: crate::error::DisconnectReason::ProtocolError,
                ..
            }
        ));
    }

    #[test]
    fn exec_request_gets_channel_failure() {
        let (mut s, mut c) = authenticated_pair();
        open_session(&mut s, &mut c);
        let mut exec = [0u8; 32];
        let exec_len;
        {
            let mut w = Writer::new(&mut exec);
            w.u8(SSH_MSG_CHANNEL_REQUEST).unwrap();
            w.u32(0).unwrap();
            w.string(b"exec").unwrap();
            w.u8(1).unwrap();
            w.string(b"ls").unwrap();
            exec_len = w.into_written();
        }
        c.send_payload(&exec[..exec_len]).unwrap();
        s.feed(&drain(&mut c)).unwrap();
        match c.feed(&drain(&mut s)).unwrap() {
            Feed::Packet { msg_type, .. } => assert_eq!(msg_type, SSH_MSG_CHANNEL_FAILURE),
            other => panic!("unexpected {:?}", other),
        }
        assert!(!s.channel_ready());
    }

    #[test]
    fn client_data_flows_to_host_and_window_adjust_returns() {
        let (mut s, mut c) = authenticated_pair();
        open_session(&mut s, &mut c);
        pty_and_shell(&mut s, &mut c);
        // Client types "help\r".
        let mut data = [0u8; 16];
        {
            let mut w = Writer::new(&mut data);
            w.u8(SSH_MSG_CHANNEL_DATA).unwrap();
            w.u32(0).unwrap();
            w.string(b"help\r").unwrap();
        }
        c.send_payload(&data[..14]).unwrap();
        let data_len = match s.feed(&drain(&mut c)).unwrap() {
            Feed::ChannelData { data } => {
                assert_eq!(data, b"help\r");
                data.len()
            }
            other => panic!("unexpected {:?}", other),
        };
        s.ack_channel_data(data_len).unwrap();
        // The ack surfaces as WINDOW_ADJUST at the client: recipient channel
        // + bytes to add (RFC 4254 §5.2 — no sender field).
        match c.feed(&drain(&mut s)).unwrap() {
            Feed::Packet { msg_type, payload } => {
                assert_eq!(msg_type, SSH_MSG_CHANNEL_WINDOW_ADJUST);
                assert_eq!(&payload[1..5], &7u32.to_be_bytes());
                assert_eq!(&payload[5..9], &5u32.to_be_bytes());
                assert_eq!(payload.len(), 9);
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn server_data_reaches_client_and_window_gates() {
        let (mut s, mut c) = authenticated_pair();
        open_session(&mut s, &mut c);
        pty_and_shell(&mut s, &mut c);
        // Server sends a banner line over the channel.
        s.send_channel_data(b"serviceos> ").unwrap();
        match c.feed(&drain(&mut s)).unwrap() {
            Feed::Packet { msg_type, payload } => {
                assert_eq!(msg_type, SSH_MSG_CHANNEL_DATA);
                assert_eq!(&payload[1..5], &7u32.to_be_bytes());
                assert_eq!(&payload[5..9], &11u32.to_be_bytes());
                assert_eq!(&payload[9..], b"serviceos> ");
            }
            other => panic!("unexpected {:?}", other),
        }
        // Data before the channel exists is refused, not silently dropped.
        let (mut s2, _c2) = authenticated_pair();
        assert!(matches!(
            s2.send_channel_data(b"x"),
            Err(crate::error::Fail::NotReady)
        ));
    }

    #[test]
    fn window_exhaustion_holds_output_then_adjust_flushes() {
        let (mut s, mut c) = authenticated_pair();
        open_session(&mut s, &mut c);
        pty_and_shell(&mut s, &mut c);
        // Zero the peer window by sending 65536 bytes (the client's window
        // from open_session) in 1024-byte chunks; the tail must be held.
        let mut sent = 0usize;
        let mut held_at = None;
        for i in 0..128 {
            let accepted = s.send_channel_data(&[b'x'; 1024]).unwrap();
            sent += accepted;
            if accepted < 1024 && held_at.is_none() {
                held_at = Some(i);
            }
            let _ = drain(&mut s);
        }
        assert!(held_at.is_some(), "window never exhausted");
        assert_eq!(sent, 65536 + 4096);
        // Peer grants 4096 -> 4096 held bytes flush.
        let mut adj = [0u8; 13];
        {
            let mut w = Writer::new(&mut adj);
            w.u8(SSH_MSG_CHANNEL_WINDOW_ADJUST).unwrap();
            w.u32(0).unwrap();
            w.u32(4096).unwrap();
        }
        c.send_payload(&adj).unwrap();
        s.feed(&drain(&mut c)).unwrap();
        let out = drain(&mut s);
        assert!(!out.is_empty(), "adjust did not flush held output");
        s.send_channel_close().unwrap();
        assert_eq!(s.state(), State::Established);
        let _ = Role::Client;
    }

    #[test]
    fn eof_and_close_matrix() {
        let (mut s, mut c) = authenticated_pair();
        open_session(&mut s, &mut c);
        pty_and_shell(&mut s, &mut c);
        // Client EOF: server marks, channel stays usable server→client.
        let mut eof = [0u8; 5];
        {
            let mut w = Writer::new(&mut eof);
            w.u8(SSH_MSG_CHANNEL_EOF).unwrap();
            w.u32(0).unwrap();
        }
        c.send_payload(&eof).unwrap();
        s.feed(&drain(&mut c)).unwrap();
        assert!(s.channel_eof_in());
        // Client CLOSE: server replies CLOSE and marks itself closed.
        let mut close = [0u8; 5];
        {
            let mut w = Writer::new(&mut close);
            w.u8(SSH_MSG_CHANNEL_CLOSE).unwrap();
            w.u32(0).unwrap();
        }
        c.send_payload(&close).unwrap();
        s.feed(&drain(&mut c)).unwrap();
        assert!(s.channel_closed());
        match c.feed(&drain(&mut s)).unwrap() {
            Feed::Packet { msg_type, .. } => assert_eq!(msg_type, SSH_MSG_CHANNEL_CLOSE),
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn global_request_keepalive_gets_request_failure() {
        let (mut s, mut c) = authenticated_pair();
        let mut keepalive = [0u8; 40];
        let ka_len;
        {
            let mut w = Writer::new(&mut keepalive);
            w.u8(SSH_MSG_GLOBAL_REQUEST).unwrap();
            w.string(b"keepalive@openssh.com").unwrap();
            w.u8(1).unwrap();
            ka_len = w.into_written();
        }
        c.send_payload(&keepalive[..ka_len]).unwrap();
        s.feed(&drain(&mut c)).unwrap();
        match c.feed(&drain(&mut s)).unwrap() {
            Feed::Packet { msg_type, .. } => assert_eq!(msg_type, SSH_MSG_REQUEST_FAILURE),
            other => panic!("unexpected {:?}", other),
        }
        // want_reply = 0: no reply.
        let mut silent = [0u8; 40];
        let silent_len;
        {
            let mut w = Writer::new(&mut silent);
            w.u8(SSH_MSG_GLOBAL_REQUEST).unwrap();
            w.string(b"no-reply@x").unwrap();
            w.u8(0).unwrap();
            silent_len = w.into_written();
        }
        c.send_payload(&silent[..silent_len]).unwrap();
        s.feed(&drain(&mut c)).unwrap();
        assert!(drain(&mut s).is_empty());
    }

    #[test]
    fn oversized_inbound_batch_disconnects() {
        let (mut s, mut c) = authenticated_pair();
        open_session(&mut s, &mut c);
        pty_and_shell(&mut s, &mut c);
        // Craft a data header claiming more than MAX_INBOUND_BATCH bytes.
        let mut huge = [0u8; 16];
        huge[0] = SSH_MSG_CHANNEL_DATA;
        huge[1..5].copy_from_slice(&0u32.to_be_bytes());
        huge[5..9].copy_from_slice(&(MAX_INBOUND_BATCH as u32 + 1).to_be_bytes());
        c.send_payload(&huge[..9]).unwrap();
        assert!(matches!(
            s.feed(&drain(&mut c)).unwrap_err(),
            crate::error::Fail::LocalDisconnect {
                reason: crate::error::DisconnectReason::ProtocolError,
                ..
            }
        ));
    }

    /// OPEN_ADMIN_PROHIBITED as big-endian constant for assertions.
    const OPEN_ADMIN_PROHIBITED_BE: u32 = 1;
}

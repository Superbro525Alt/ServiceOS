//! The `SshTransport` state machine.
//!
//! Server role (the operator surface this wave) plus a minimal client role
//! used solely as a test helper for the in-process handshake harness.
//! States: `VersionExchange → KexInit → KexReply → NewKeys → Established`,
//! with `Closed` as the terminal state after any honest disconnect (local
//! or peer-initiated).
//!
//! Zero-heap design: every buffer is a fixed array sized to the RFC 4253
//! §6.1 packet bounds (~150 KiB total per transport — revisit packing before
//! static in-guest allocation). No RNG: the host-key seed, ephemeral X25519
//! seed and KEXINIT cookie are constructor parameters. Protocol failures
//! queue an honest SSH_MSG_DISCONNECT under the current cipher state and
//! close the transport; unknown packet types are answered with
//! SSH_MSG_UNIMPLEMENTED carrying the offender's sequence number.
//!
//! Packet flow: `feed()` consumes received bytes and may return a decrypted
//! post-establishment payload (borrowed from the staging buffer, valid until
//! the next `feed()`). `pending_output()` / `consume_output()` drain queued
//! wire bytes. `send_payload()` emits an encrypted packet once established.

use crate::error::{DisconnectReason, Fail};
use crate::hostkey;
use crate::kex::{self, KexInitRef};
use crate::negotiate::{self, Negotiated};
use crate::packet::{self, CipherKeys};
use crate::version::{self, IdentErr};
use serviceos_crypto::ed25519;
use serviceos_crypto::x25519;

/// Connection role. `Client` is a test helper only this wave.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Server,
    Client,
}

/// Transport state machine states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    VersionExchange,
    KexInit,
    KexReply,
    NewKeys,
    Established,
    Closed,
}

/// Outcome of `feed()`: progress, a decrypted established-state payload
/// (borrowed from the transport until the next `feed`), a parked password
/// authentication attempt (see `take_auth_request`/`auth_verdict`), or a
/// channel-data batch for the session bridge.
#[derive(Debug, PartialEq, Eq)]
pub enum Feed<'a> {
    Progress,
    Packet {
        msg_type: u8,
        payload: &'a [u8],
    },
    /// Password credentials parked; the host verifies them through its own
    /// authority and calls `auth_verdict`. Processing stalls until then.
    AuthQuery,
    /// Inbound session-channel data; the host consumes the batch and calls
    /// `ack_channel_data` to re-grant the receive window.
    ChannelData {
        data: &'a [u8],
    },
}

enum Dispatch {
    Handled,
    Passthrough,
    /// Park a password authentication attempt.
    Auth,
    /// Deliver an inbound channel-data batch (length in the staging buffer).
    DeliverData(usize),
}

/// Output capacity: one max data packet plus a small reply and slack.
const OUT_CAP: usize = 34816;
/// Input accumulation: one maximum wire packet plus pipelining slack.
const IN_CAP: usize = packet::MAX_WIRE_LEN + 512;
/// Decode staging: one decrypted packet body.
const STAGE_CAP: usize = packet::MAX_PACKET_LEN;
/// AEAD encode staging: padlen + max payload + padding.
const MSG_SCRATCH_CAP: usize = packet::MAX_PAYLOAD_LEN + packet::BLOCK_SIZE + 8;
/// Version-line accumulation bound (RFC 4253 §4.2: 255 incl. CRLF).
const LINE_CAP: usize = version::IDENT_MAX + 1;
/// Client role: tolerated pre-identification lines from a server.
const PRE_LINES_MAX: usize = 16;
/// Peer disconnect description storage.
const PEER_DESC_CAP: usize = 128;
/// Accepted KEXINIT payload cap.
const KEXINIT_CAP: usize = kex::KEXINIT_MAX;

pub struct SshTransport {
    role: Role,
    state: State,
    // Wire buffers.
    out: [u8; OUT_CAP],
    out_len: usize,
    inbuf: [u8; IN_CAP],
    in_len: usize,
    pub(crate) proc: [u8; STAGE_CAP],
    len_scratch: [u8; 4],
    msg_scratch: [u8; MSG_SCRATCH_CAP],
    // Identity material (caller-supplied; the library has no RNG).
    host_seed: [u8; 32],
    host_pub: [u8; 32],
    x25519_seed: [u8; 32],
    cookie: [u8; 16],
    // Version exchange.
    peer_ident: [u8; version::IDENT_MAX],
    peer_ident_len: usize,
    // Handshake records.
    my_kexinit: [u8; KEXINIT_CAP],
    my_kexinit_len: usize,
    peer_kexinit: [u8; KEXINIT_CAP],
    peer_kexinit_len: usize,
    negotiated: Option<Negotiated>,
    e_client: [u8; 32],
    f_server: [u8; 32],
    k_mpint: [u8; 40],
    k_mpint_len: usize,
    exchange_h: [u8; 32],
    session_id: Option<[u8; 32]>,
    // Cipher state. None = plaintext pre-KEX framing.
    tx_keys: Option<CipherKeys>,
    rx_keys: Option<CipherKeys>,
    pending_rx: Option<CipherKeys>,
    rx_seq: u32,
    tx_seq: u32,
    // Peer disconnect info.
    peer_desc: [u8; PEER_DESC_CAP],
    peer_desc_len: usize,
    peer_reason: Option<u32>,
    // RFC 4252 server authentication state (shared/ssh/src/auth.rs).
    pub(crate) auth: crate::auth::AuthState,
    // RFC 4254 session-channel state (shared/ssh/src/channel.rs).
    pub(crate) chan: crate::channel::ChannelState,
}

impl SshTransport {
    /// Server side. `host_seed` is the ed25519 host-key seed (signature
    /// authority over the exchange hash), `x25519_seed` the ephemeral KEX
    /// secret, `cookie` the KEXINIT cookie — all caller-supplied (the
    /// library contains no RNG; derive them from the kernel entropy source
    /// at the seam that instantiates the transport).
    pub fn server(host_seed: [u8; 32], x25519_seed: [u8; 32], cookie: [u8; 16]) -> SshTransport {
        let mut t = SshTransport::new(Role::Server, host_seed, x25519_seed, cookie);
        let _ = t.emit_raw(version::SERVER_BANNER);
        t
    }

    /// Client side — TEST HELPER ONLY: exists to drive the in-process
    /// handshake harness; not an operator surface this wave.
    pub fn client(x25519_seed: [u8; 32], cookie: [u8; 16]) -> SshTransport {
        let mut t = SshTransport::new(Role::Client, [0u8; 32], x25519_seed, cookie);
        let _ = t.emit_raw(version::SERVER_BANNER);
        let _ = t.emit_my_kexinit();
        t
    }

    fn new(
        role: Role,
        host_seed: [u8; 32],
        x25519_seed: [u8; 32],
        cookie: [u8; 16],
    ) -> SshTransport {
        let mut t = SshTransport::placeholder();
        t.setup(role, host_seed, x25519_seed, cookie);
        t
    }

    /// In-place server construction for static in-guest allocation.
    /// Equivalent to `server()` but initializes `self` instead of returning
    /// a fresh value: the ~139 KiB of fixed buffers must never move through
    /// a return slot, which would churn small userspace stacks (the
    /// network-service run task overflowed exactly this way). Requires a
    /// `placeholder()`-constructed or otherwise unused transport.
    pub fn init_server(&mut self, host_seed: [u8; 32], x25519_seed: [u8; 32], cookie: [u8; 16]) {
        self.setup(Role::Server, host_seed, x25519_seed, cookie);
        let _ = self.emit_raw(version::SERVER_BANNER);
    }

    /// Const placeholder for static (`.bss`) allocation: buffers zeroed,
    /// state `Closed`. Unusable until `init_server` resets it; any
    /// `feed`/`send_payload` before that fails honestly.
    pub const fn placeholder() -> SshTransport {
        SshTransport {
            role: Role::Client,
            state: State::Closed,
            out: [0; OUT_CAP],
            out_len: 0,
            inbuf: [0; IN_CAP],
            in_len: 0,
            proc: [0; STAGE_CAP],
            len_scratch: [0; 4],
            msg_scratch: [0; MSG_SCRATCH_CAP],
            host_seed: [0; 32],
            host_pub: [0; 32],
            x25519_seed: [0; 32],
            cookie: [0; 16],
            peer_ident: [0; version::IDENT_MAX],
            peer_ident_len: 0,
            my_kexinit: [0; KEXINIT_CAP],
            my_kexinit_len: 0,
            peer_kexinit: [0; KEXINIT_CAP],
            peer_kexinit_len: 0,
            negotiated: None,
            e_client: [0; 32],
            f_server: [0; 32],
            k_mpint: [0; 40],
            k_mpint_len: 0,
            exchange_h: [0; 32],
            session_id: None,
            tx_keys: None,
            rx_keys: None,
            pending_rx: None,
            rx_seq: 0,
            tx_seq: 0,
            peer_desc: [0; PEER_DESC_CAP],
            peer_desc_len: 0,
            peer_reason: None,
            auth: crate::auth::AuthState::new(),
            chan: crate::channel::ChannelState::new(),
        }
    }

    /// Shared initializer for `new()`/`init_server()`: assigns every
    /// protocol-state field (fixed storage is untouched; only lengths and
    /// state reset). `role` decides server vs client framing.
    fn setup(&mut self, role: Role, host_seed: [u8; 32], x25519_seed: [u8; 32], cookie: [u8; 16]) {
        self.role = role;
        self.state = State::VersionExchange;
        self.out_len = 0;
        self.in_len = 0;
        self.host_seed = host_seed;
        self.host_pub = ed25519::public_key(&host_seed);
        self.x25519_seed = x25519_seed;
        self.cookie = cookie;
        self.peer_ident_len = 0;
        self.my_kexinit_len = 0;
        self.peer_kexinit_len = 0;
        self.negotiated = None;
        self.e_client = [0; 32];
        self.f_server = [0; 32];
        self.k_mpint_len = 0;
        self.exchange_h = [0; 32];
        self.session_id = None;
        self.tx_keys = None;
        self.rx_keys = None;
        self.pending_rx = None;
        self.rx_seq = 0;
        self.tx_seq = 0;
        self.peer_desc_len = 0;
        self.peer_reason = None;
        self.auth = crate::auth::AuthState::new();
        self.chan = crate::channel::ChannelState::new();
    }

    // ------------------------------------------------------------------
    // Introspection
    // ------------------------------------------------------------------

    pub fn state(&self) -> State {
        self.state
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn negotiated(&self) -> Option<&Negotiated> {
        self.negotiated.as_ref()
    }

    /// Session ID (exchange hash of the first key exchange).
    pub fn session_id(&self) -> Option<&[u8; 32]> {
        self.session_id.as_ref()
    }

    pub fn exchange_hash(&self) -> &[u8; 32] {
        &self.exchange_h
    }

    /// Peer identification string (V_C / V_S) after version exchange.
    pub fn peer_version(&self) -> Option<&str> {
        if self.peer_ident_len == 0 {
            return None;
        }
        core::str::from_utf8(&self.peer_ident[..self.peer_ident_len]).ok()
    }

    pub fn peer_disconnect_reason(&self) -> Option<u32> {
        self.peer_reason
    }

    /// Peer disconnect description (truncated to 128 bytes).
    pub fn peer_disconnect_description(&self) -> Option<&str> {
        if self.peer_desc_len == 0 {
            return None;
        }
        core::str::from_utf8(&self.peer_desc[..self.peer_desc_len]).ok()
    }

    /// Queued outgoing wire bytes; drain with `consume_output`.
    pub fn pending_output(&self) -> &[u8] {
        &self.out[..self.out_len]
    }

    pub fn consume_output(&mut self, n: usize) {
        let n = n.min(self.out_len);
        self.out.copy_within(n..self.out_len, 0);
        self.out_len -= n;
    }

    // ------------------------------------------------------------------
    // I/O
    // ------------------------------------------------------------------

    /// Feed received bytes. Remaining buffered packets can be processed
    /// with further `feed(&[])` calls.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Feed<'_>, Fail> {
        if self.state == State::Closed {
            return Err(Fail::Closed);
        }
        self.push_in(bytes)?;
        loop {
            // A parked authentication attempt stalls all further processing
            // until the host delivers the verdict (the verdict is I/O the
            // pure library cannot perform).
            if self.auth.phase == crate::auth::AuthPhase::Pending {
                break;
            }
            match self.state {
                State::VersionExchange => {
                    if !self.try_version()? {
                        break;
                    }
                }
                _ => {
                    let Some((info, seq)) = self.try_packet()? else {
                        break;
                    };
                    match self.dispatch(info, seq)? {
                        Dispatch::Handled => {}
                        Dispatch::Passthrough => {
                            let start = info.payload_start;
                            let len = info.payload_len;
                            let msg = self.proc[start];
                            return Ok(Feed::Packet {
                                msg_type: msg,
                                payload: &self.proc[start..start + len],
                            });
                        }
                        Dispatch::Auth => {
                            return Ok(Feed::AuthQuery);
                        }
                        Dispatch::DeliverData(len) => {
                            // CHANNEL_DATA payload: type + recipient + len
                            // prefix + bytes; the body starts at +9.
                            let start = info.payload_start + 9;
                            return Ok(Feed::ChannelData {
                                data: &self.proc[start..start + len],
                            });
                        }
                    }
                }
            }
        }
        Ok(Feed::Progress)
    }

    /// Send a raw payload as an encrypted packet (established state only);
    /// the payload includes the message-type byte.
    pub fn send_payload(&mut self, payload: &[u8]) -> Result<(), Fail> {
        if self.state == State::Closed {
            return Err(Fail::Closed);
        }
        if self.state != State::Established || self.tx_keys.is_none() {
            return Err(Fail::NotReady);
        }
        if payload.len() > packet::MAX_PAYLOAD_LEN {
            return Err(Fail::LocalDisconnect {
                reason: DisconnectReason::ProtocolError,
                description: "payload exceeds maximum packet size",
            });
        }
        self.emit_packet(payload)
    }

    fn push_in(&mut self, bytes: &[u8]) -> Result<(), Fail> {
        if bytes.is_empty() {
            return Ok(());
        }
        if self.in_len + bytes.len() > IN_CAP {
            return Err(Fail::OutOfCapacity);
        }
        self.inbuf[self.in_len..self.in_len + bytes.len()].copy_from_slice(bytes);
        self.in_len += bytes.len();
        Ok(())
    }

    fn emit_raw(&mut self, bytes: &[u8]) -> Result<(), Fail> {
        if self.out_len + bytes.len() > OUT_CAP {
            return Err(Fail::OutOfCapacity);
        }
        self.out[self.out_len..self.out_len + bytes.len()].copy_from_slice(bytes);
        self.out_len += bytes.len();
        Ok(())
    }

    /// Encode + queue a packet under the current TX framing (plain
    /// pre-KEX, AEAD post-NEWKEYS) and advance the TX sequence number.
    pub(crate) fn emit_packet(&mut self, payload: &[u8]) -> Result<(), Fail> {
        let pad = if self.tx_keys.is_some() {
            packet::padding_len_aead(payload.len(), packet::BLOCK_SIZE)
        } else {
            packet::padding_len(payload.len(), packet::BLOCK_SIZE)
        };
        let wire_len = 4 + 1 + payload.len() + pad + if self.tx_keys.is_some() { 16 } else { 0 };
        if self.out_len + wire_len > OUT_CAP {
            return Err(Fail::OutOfCapacity);
        }
        match self.tx_keys {
            None => {
                packet::encode_plain(payload, &mut self.out[self.out_len..])
                    .map_err(|_| Fail::OutOfCapacity)?;
            }
            Some(keys) => {
                self.msg_scratch[0] = pad as u8;
                self.msg_scratch[1..1 + payload.len()].copy_from_slice(payload);
                self.msg_scratch[1 + payload.len()..1 + payload.len() + pad].fill(0);
                let msg_len = 1 + payload.len() + pad;
                packet::encode_aead_msg(
                    &self.msg_scratch[..msg_len],
                    &keys,
                    self.tx_seq,
                    &mut self.out[self.out_len..],
                )
                .map_err(|_| Fail::OutOfCapacity)?;
            }
        }
        self.out_len += wire_len;
        self.tx_seq = self.tx_seq.wrapping_add(1);
        Ok(())
    }

    /// Queue an honest DISCONNECT and close. If the output buffer is full
    /// the DISCONNECT is dropped — the connection closes either way.
    pub(crate) fn fail_disconnect(
        &mut self,
        reason: DisconnectReason,
        description: &'static str,
    ) -> Fail {
        let mut payload = [0u8; 5 + 4 + PEER_DESC_CAP + 4];
        payload[0] = 1; // SSH_MSG_DISCONNECT
        payload[1..5].copy_from_slice(&reason.code().to_be_bytes());
        let n = {
            let mut w = crate::wire::Writer::new(&mut payload[5..]);
            let _ = w.string(description.as_bytes());
            let _ = w.string(b"");
            5 + w.into_written()
        };
        let _ = self.emit_packet(&payload[..n]);
        self.state = State::Closed;
        Fail::LocalDisconnect {
            reason,
            description,
        }
    }

    // ------------------------------------------------------------------
    // Version exchange (RFC 4253 §4.2)
    // ------------------------------------------------------------------

    fn try_version(&mut self) -> Result<bool, Fail> {
        match self.role {
            Role::Server => self.try_version_server(),
            Role::Client => self.try_version_client(),
        }
    }

    fn try_version_server(&mut self) -> Result<bool, Fail> {
        let Some(lf) = self.inbuf[..self.in_len].iter().position(|&b| b == b'\n') else {
            if self.in_len >= LINE_CAP {
                return Err(self.fail_disconnect(
                    DisconnectReason::ProtocolError,
                    "identification line too long",
                ));
            }
            return Ok(false);
        };
        let mut line = &self.inbuf[..lf];
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        let consumed = lf + 1;
        match version::parse_identification(line) {
            Ok(ident) => {
                self.peer_ident[..ident.len()].copy_from_slice(ident);
                self.peer_ident_len = ident.len();
                self.inbuf.copy_within(consumed..self.in_len, 0);
                self.in_len -= consumed;
                self.state = State::KexInit;
                self.emit_my_kexinit()?;
                Ok(true)
            }
            // An SSH-x.y line with an unsupported version gets the
            // dedicated reason; anything else is a plain protocol error
            // (clients must not send pre-identification lines).
            Err(IdentErr::BadPrefix) if line.starts_with(b"SSH-") => Err(self.fail_disconnect(
                DisconnectReason::ProtocolVersionNotSupported,
                "unsupported SSH protocol version",
            )),
            Err(_) => Err(self.fail_disconnect(
                DisconnectReason::ProtocolError,
                "malformed identification line",
            )),
        }
    }

    fn try_version_client(&mut self) -> Result<bool, Fail> {
        match version::parse_server_identification(&self.inbuf[..self.in_len], PRE_LINES_MAX) {
            Ok(None) => Ok(false),
            Ok(Some((consumed, ident))) => {
                self.peer_ident[..ident.len()].copy_from_slice(ident);
                self.peer_ident_len = ident.len();
                self.inbuf.copy_within(consumed..self.in_len, 0);
                self.in_len -= consumed;
                self.state = State::KexInit;
                Ok(true)
            }
            Err(IdentErr::TooLong) => Err(self.fail_disconnect(
                DisconnectReason::ProtocolError,
                "identification line too long",
            )),
            Err(IdentErr::BadPrefix) => Err(self.fail_disconnect(
                DisconnectReason::ProtocolError,
                "too many pre-identification lines",
            )),
            Err(_) => Err(self.fail_disconnect(
                DisconnectReason::ProtocolError,
                "malformed identification line",
            )),
        }
    }

    fn emit_my_kexinit(&mut self) -> Result<(), Fail> {
        let mut buf = [0u8; 1024];
        let n = kex::build_kexinit(&self.cookie, &mut buf).map_err(|_| Fail::NotReady)?;
        self.my_kexinit[..n].copy_from_slice(&buf[..n]);
        self.my_kexinit_len = n;
        self.emit_packet(&buf[..n])
    }

    // ------------------------------------------------------------------
    // Binary packet protocol decode + dispatch
    // ------------------------------------------------------------------

    /// Decode one packet from the input accumulator into the staging
    /// buffer; consume wire bytes and advance the RX sequence number.
    fn try_packet(&mut self) -> Result<Option<(packet::FrameInfo, u32)>, Fail> {
        let seq = self.rx_seq;
        let rx_keys = self.rx_keys;
        let result = match rx_keys {
            None => packet::decode_plain(&self.inbuf[..self.in_len], &mut self.proc),
            Some(keys) => packet::decode_aead(
                &self.inbuf[..self.in_len],
                &mut self.proc,
                &keys,
                seq,
                &mut self.len_scratch,
            ),
        };
        match result {
            Ok(None) => Ok(None),
            Ok(Some(info)) => {
                self.inbuf.copy_within(info.consumed..self.in_len, 0);
                self.in_len -= info.consumed;
                self.rx_seq = self.rx_seq.wrapping_add(1);
                Ok(Some((info, seq)))
            }
            Err(reason) => {
                let description = match reason {
                    DisconnectReason::MacError => "packet authentication failure",
                    _ => "malformed binary packet",
                };
                Err(self.fail_disconnect(reason, description))
            }
        }
    }

    fn dispatch(&mut self, info: packet::FrameInfo, seq: u32) -> Result<Dispatch, Fail> {
        let msg = self.proc[info.payload_start];
        match self.state {
            State::KexInit => self.dispatch_kexinit(msg, info),
            State::KexReply => self.dispatch_kexreply(msg, info),
            State::NewKeys => self.dispatch_newkeys(msg, info),
            State::Established => self.dispatch_established(msg, info, seq),
            _ => Err(Fail::Closed),
        }
    }

    /// Common pre-established handling: DISCONNECT / IGNORE / DEBUG /
    /// UNIMPLEMENTED / SERVICE_REQUEST. `None` = fall through.
    fn dispatch_pre_established(
        &mut self,
        msg: u8,
        info: packet::FrameInfo,
    ) -> Option<Result<Dispatch, Fail>> {
        match msg {
            1 => Some(Err(self.take_peer_disconnect(info))),
            2 | 3 | 4 => Some(Ok(Dispatch::Handled)),
            5 => Some(Err(self.fail_disconnect(
                DisconnectReason::ServiceNotAvailable,
                "service requests not implemented (no authentication this wave)",
            ))),
            _ => None,
        }
    }

    /// KEX protocol messages that are wrong in the current state.
    fn unexpected_kex_message(msg: u8) -> bool {
        matches!(msg, 20 | 21 | 30 | 31)
    }

    fn dispatch_kexinit(&mut self, msg: u8, info: packet::FrameInfo) -> Result<Dispatch, Fail> {
        if let Some(r) = self.dispatch_pre_established(msg, info) {
            return r;
        }
        if msg != kex::SSH_MSG_KEXINIT {
            if Self::unexpected_kex_message(msg) {
                return Err(self.fail_disconnect(
                    DisconnectReason::ProtocolError,
                    "key exchange message out of order",
                ));
            }
            self.reply_unimplemented(self.rx_seq.wrapping_sub(1))?;
            return Ok(Dispatch::Handled);
        }
        if self.peer_kexinit_len != 0 {
            return Err(self.fail_disconnect(DisconnectReason::ProtocolError, "duplicate KEXINIT"));
        }
        let len = info.payload_len;
        if len > KEXINIT_CAP {
            return Err(
                self.fail_disconnect(DisconnectReason::ProtocolError, "KEXINIT payload too large")
            );
        }
        self.peer_kexinit[..len]
            .copy_from_slice(&self.proc[info.payload_start..info.payload_start + len]);
        self.peer_kexinit_len = len;
        let parsed = match kex::parse_kexinit(&self.peer_kexinit[..len]) {
            Ok(k) => k,
            Err(_) => {
                return Err(
                    self.fail_disconnect(DisconnectReason::ProtocolError, "malformed KEXINIT")
                );
            }
        };
        let p = &self.peer_kexinit[..len];
        let neg = match negotiate::negotiate(
            parsed.list(p, KexInitRef::KEX),
            parsed.list(p, KexInitRef::HOSTKEY),
            parsed.list(p, KexInitRef::ENC_C2S),
            parsed.list(p, KexInitRef::ENC_S2C),
            parsed.list(p, KexInitRef::MAC_C2S),
            parsed.list(p, KexInitRef::MAC_S2C),
            parsed.list(p, KexInitRef::COMP_C2S),
            parsed.list(p, KexInitRef::COMP_S2C),
        ) {
            Ok(n) => n,
            Err(class) => {
                return Err(self.fail_disconnect(DisconnectReason::KeyExchangeFailed, class));
            }
        };
        self.negotiated = Some(neg);
        match self.role {
            Role::Server => {
                self.state = State::KexReply;
            }
            Role::Client => {
                let e = x25519::x25519_public(&self.x25519_seed);
                let mut buf = [0u8; 64];
                let n = kex::build_ecdh_init(&e, &mut buf).map_err(|_| Fail::NotReady)?;
                self.emit_packet(&buf[..n])?;
                self.state = State::KexReply;
            }
        }
        Ok(Dispatch::Handled)
    }

    fn dispatch_kexreply(&mut self, msg: u8, info: packet::FrameInfo) -> Result<Dispatch, Fail> {
        if let Some(r) = self.dispatch_pre_established(msg, info) {
            return r;
        }
        match (self.role, msg) {
            (Role::Server, kex::SSH_MSG_KEX_ECDH_INIT) => self.handle_ecdh_init(info),
            (Role::Client, kex::SSH_MSG_KEX_ECDH_REPLY) => self.handle_ecdh_reply(info),
            _ => {
                if Self::unexpected_kex_message(msg) {
                    Err(self.fail_disconnect(
                        DisconnectReason::ProtocolError,
                        "key exchange message out of order",
                    ))
                } else {
                    self.reply_unimplemented(self.rx_seq.wrapping_sub(1))?;
                    Ok(Dispatch::Handled)
                }
            }
        }
    }

    fn handle_ecdh_init(&mut self, info: packet::FrameInfo) -> Result<Dispatch, Fail> {
        let start = info.payload_start;
        let end = start + info.payload_len;
        let e = match kex::parse_ecdh_init(&self.proc[start..end]) {
            Ok(e) => e,
            Err(_) => {
                return Err(self.fail_disconnect(
                    DisconnectReason::KeyExchangeFailed,
                    "invalid client ephemeral public key",
                ));
            }
        };
        let shared_le = match kex::shared_secret(&self.x25519_seed, &e) {
            Ok(s) => s,
            Err(()) => {
                return Err(self.fail_disconnect(
                    DisconnectReason::KeyExchangeFailed,
                    "all-zero X25519 shared secret",
                ));
            }
        };
        self.e_client = e;
        self.f_server = x25519::x25519_public(&self.x25519_seed);
        let k_len = {
            let mut w = crate::wire::Writer::new(&mut self.k_mpint);
            if w.mpint_be(&shared_le).is_err() {
                return Err(Fail::NotReady);
            }
            w.into_written()
        };
        self.k_mpint_len = k_len;

        let h = {
            // K_S appears in H in its full wire string encoding (the blob
            // plus its own length prefix, matching OpenSSH's hash input).
            let mut k_s_buf = [0u8; 96];
            let kl =
                hostkey::host_key_blob(&self.host_pub, &mut k_s_buf).map_err(|_| Fail::NotReady)?;
            let mut ks_full = [0u8; 100];
            ks_full[0..4].copy_from_slice(&(kl as u32).to_be_bytes());
            ks_full[4..4 + kl].copy_from_slice(&k_s_buf[..kl]);
            kex::exchange_hash(
                &self.peer_ident[..self.peer_ident_len],
                version::SERVER_BANNER_TEXT,
                &self.peer_kexinit[..self.peer_kexinit_len],
                &self.my_kexinit[..self.my_kexinit_len],
                &ks_full[..4 + kl],
                &self.e_client,
                &self.f_server,
                &self.k_mpint[..self.k_mpint_len],
            )
        };
        self.exchange_h = h;
        let signature = ed25519::sign(&self.host_seed, &h);

        let (reply, reply_len) = {
            let mut k_s_buf = [0u8; 96];
            let kl =
                hostkey::host_key_blob(&self.host_pub, &mut k_s_buf).map_err(|_| Fail::NotReady)?;
            let mut sig_buf = [0u8; 96];
            let sl =
                hostkey::signature_blob(&signature, &mut sig_buf).map_err(|_| Fail::NotReady)?;
            let mut reply = [0u8; 320];
            let n =
                kex::build_ecdh_reply(&k_s_buf[..kl], &self.f_server, &sig_buf[..sl], &mut reply)
                    .map_err(|_| Fail::NotReady)?;
            (reply, n)
        };
        self.emit_packet(&reply[..reply_len])?;
        // SSH_MSG_NEWKEYS under the old (plain) framing.
        self.emit_packet(&[kex::SSH_MSG_NEWKEYS])?;

        let (c2s, s2c) = kex::derive_session_keys(&self.k_mpint[..self.k_mpint_len], &h);
        // Client-to-server ('A') is our RX direction; server-to-client
        // ('B') is TX. TX switches right after NEWKEYS; RX switches when
        // the peer's NEWKEYS arrives. Sequence numbers CONTINUE across the
        // cipher switch: we do not advertise kex-strict-s-v00@openssh.com,
        // so the strict-KEX reset must not happen (OpenSSH keeps counting
        // for non-strict peers).
        self.pending_rx = Some(c2s);
        self.tx_keys = Some(s2c);
        self.session_id = Some(h);
        self.state = State::NewKeys;
        Ok(Dispatch::Handled)
    }

    fn handle_ecdh_reply(&mut self, info: packet::FrameInfo) -> Result<Dispatch, Fail> {
        let start = info.payload_start;
        let end = start + info.payload_len;
        let parsed = match kex::parse_ecdh_reply(&self.proc[start..end]) {
            Ok(p) => p,
            Err(_) => {
                return Err(
                    self.fail_disconnect(DisconnectReason::ProtocolError, "malformed ECDH_REPLY")
                );
            }
        };
        // host_key range includes the outer string length prefix; the
        // blob itself starts 4 bytes in.
        let host_pub = match hostkey::parse_host_key_blob(
            &self.proc[start + parsed.host_key.0 + 4..start + parsed.host_key.1],
        ) {
            Ok(pk) => pk,
            Err(_) => {
                return Err(self.fail_disconnect(
                    DisconnectReason::HostKeyNotVerifiable,
                    "unsupported or malformed host key blob",
                ));
            }
        };
        let shared_le = match kex::shared_secret(&self.x25519_seed, &parsed.f) {
            Ok(s) => s,
            Err(()) => {
                return Err(self.fail_disconnect(
                    DisconnectReason::KeyExchangeFailed,
                    "all-zero X25519 shared secret",
                ));
            }
        };
        let (k_buf, k_len) = {
            let mut k_buf = [0u8; 40];
            let mut w = crate::wire::Writer::new(&mut k_buf);
            if w.mpint_be(&shared_le).is_err() {
                return Err(Fail::NotReady);
            }
            let n = w.into_written();
            (k_buf, n)
        };
        let h = {
            // Client perspective: V_C = our banner, V_S = peer banner,
            // I_C = our KEXINIT, I_S = peer KEXINIT, K_S = the received
            // host-key blob.
            let e_client = x25519::x25519_public(&self.x25519_seed);
            kex::exchange_hash(
                version::SERVER_BANNER_TEXT,
                &self.peer_ident[..self.peer_ident_len],
                &self.my_kexinit[..self.my_kexinit_len],
                &self.peer_kexinit[..self.peer_kexinit_len],
                &self.proc[start + parsed.host_key.0..start + parsed.host_key.1],
                &e_client,
                &parsed.f,
                &k_buf[..k_len],
            )
        };

        // Verify the host-key signature over H. Trust honesty: this proves
        // the peer holds the private key for the OFFERED key — not that the
        // operator trusts that key (no known_hosts store this wave).
        let ok = hostkey::verify_exchange_signature(
            &host_pub,
            &h,
            &self.proc[start + parsed.sig.0 + 4..start + parsed.sig.1],
        )
        .unwrap_or(false);
        if !ok {
            return Err(self.fail_disconnect(
                DisconnectReason::HostKeyNotVerifiable,
                "host key signature verification failed",
            ));
        }

        let (c2s, s2c) = kex::derive_session_keys(&k_buf[..k_len], &h);
        self.emit_packet(&[kex::SSH_MSG_NEWKEYS])?;
        // Non-strict kex: TX sequence numbers continue across the switch.
        self.tx_keys = Some(c2s);
        self.pending_rx = Some(s2c);
        self.exchange_h = h;
        self.session_id = Some(h);
        self.state = State::NewKeys;
        Ok(Dispatch::Handled)
    }

    fn dispatch_newkeys(&mut self, msg: u8, info: packet::FrameInfo) -> Result<Dispatch, Fail> {
        if let Some(r) = self.dispatch_pre_established(msg, info) {
            return r;
        }
        if msg == kex::SSH_MSG_NEWKEYS {
            match self.pending_rx.take() {
                Some(keys) => {
                    self.rx_keys = Some(keys);
                    // Non-strict kex: RX sequence numbers continue.
                    self.state = State::Established;
                    Ok(Dispatch::Handled)
                }
                None => Err(self.fail_disconnect(
                    DisconnectReason::ProtocolError,
                    "NEWKEYS without pending keys",
                )),
            }
        } else if Self::unexpected_kex_message(msg) {
            Err(self.fail_disconnect(
                DisconnectReason::ProtocolError,
                "key exchange message out of order",
            ))
        } else {
            self.reply_unimplemented(self.rx_seq.wrapping_sub(1))?;
            Ok(Dispatch::Handled)
        }
    }

    fn dispatch_established(
        &mut self,
        msg: u8,
        info: packet::FrameInfo,
        seq: u32,
    ) -> Result<Dispatch, Fail> {
        match msg {
            1 => Err(self.take_peer_disconnect(info)),
            2 | 3 | 4 => Ok(Dispatch::Handled),
            5 => {
                self.handle_service_request(info.payload_start, info.payload_len)?;
                Ok(Dispatch::Handled)
            }
            crate::auth::SSH_MSG_USERAUTH_REQUEST => {
                self.handle_userauth_request(info.payload_start, info.payload_len)?;
                Ok(if self.auth.phase == crate::auth::AuthPhase::Pending {
                    Dispatch::Auth
                } else {
                    Dispatch::Handled
                })
            }
            crate::channel::SSH_MSG_GLOBAL_REQUEST => {
                self.handle_global_request(info.payload_start, info.payload_len)?;
                Ok(Dispatch::Handled)
            }
            crate::channel::SSH_MSG_CHANNEL_OPEN
            | crate::channel::SSH_MSG_CHANNEL_WINDOW_ADJUST
            | crate::channel::SSH_MSG_CHANNEL_EOF
            | crate::channel::SSH_MSG_CHANNEL_CLOSE
            | crate::channel::SSH_MSG_CHANNEL_REQUEST => {
                match msg {
                    crate::channel::SSH_MSG_CHANNEL_OPEN => {
                        self.handle_channel_open(info.payload_start, info.payload_len)?
                    }
                    crate::channel::SSH_MSG_CHANNEL_WINDOW_ADJUST => {
                        match self.handle_window_adjust(info.payload_start, info.payload_len)? {
                            crate::channel::ChanSimple::Handled => {}
                            crate::channel::ChanSimple::Passthrough => {
                                return Ok(Dispatch::Passthrough);
                            }
                        }
                    }
                    crate::channel::SSH_MSG_CHANNEL_EOF => {
                        match self.handle_channel_eof(info.payload_start, info.payload_len)? {
                            crate::channel::ChanSimple::Handled => {}
                            crate::channel::ChanSimple::Passthrough => {
                                return Ok(Dispatch::Passthrough);
                            }
                        }
                    }
                    crate::channel::SSH_MSG_CHANNEL_CLOSE => {
                        match self.handle_channel_close(info.payload_start, info.payload_len)? {
                            crate::channel::ChanSimple::Handled => {}
                            crate::channel::ChanSimple::Passthrough => {
                                return Ok(Dispatch::Passthrough);
                            }
                        }
                    }
                    _ => self.handle_channel_request(info.payload_start, info.payload_len)?,
                }
                Ok(Dispatch::Handled)
            }
            crate::channel::SSH_MSG_CHANNEL_DATA => {
                match self.try_channel_data(info.payload_start, info.payload_len)? {
                    crate::channel::ChannelOutcome::Data(len) => Ok(Dispatch::DeliverData(len)),
                    crate::channel::ChannelOutcome::Passthrough => Ok(Dispatch::Passthrough),
                    crate::channel::ChannelOutcome::Handled => Ok(Dispatch::Handled),
                }
            }
            20 => {
                Err(self.fail_disconnect(DisconnectReason::ProtocolError, "rekeying not supported"))
            }
            21 | 30 | 31 => Err(self.fail_disconnect(
                DisconnectReason::ProtocolError,
                "key exchange message outside key exchange",
            )),
            _ => {
                // Honest answer to anything unimplemented, then hand the
                // payload to the caller (session bridging is a later wave).
                self.reply_unimplemented(seq)?;
                Ok(Dispatch::Passthrough)
            }
        }
    }

    fn reply_unimplemented(&mut self, offender_seq: u32) -> Result<(), Fail> {
        let payload = [
            3u8,
            (offender_seq >> 24) as u8,
            (offender_seq >> 16) as u8,
            (offender_seq >> 8) as u8,
            offender_seq as u8,
        ];
        self.emit_packet(&payload)
    }

    fn take_peer_disconnect(&mut self, info: packet::FrameInfo) -> Fail {
        let start = info.payload_start;
        let end = start + info.payload_len;
        let (reason, desc_local, desc_len) = {
            let mut r = crate::wire::Reader::new(&self.proc[start + 1..end]);
            let reason = r.u32().unwrap_or(DisconnectReason::ByApplication.code());
            let mut desc_local = [0u8; PEER_DESC_CAP];
            let mut desc_len = 0;
            if let Ok(desc) = r.string() {
                desc_len = desc.len().min(PEER_DESC_CAP);
                desc_local[..desc_len].copy_from_slice(&desc[..desc_len]);
            }
            (reason, desc_local, desc_len)
        };
        self.peer_desc[..desc_len].copy_from_slice(&desc_local[..desc_len]);
        self.peer_desc_len = desc_len;
        self.peer_reason = Some(reason);
        self.state = State::Closed;
        Fail::PeerDisconnect {
            reason_code: reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Writer;

    fn host_seed() -> [u8; 32] {
        let mut s = [0u8; 32];
        s[0] = 0x11;
        s[15] = 0xAB;
        s[31] = 0x5E;
        s
    }

    fn srv_seed() -> [u8; 32] {
        let mut s = [0u8; 32];
        s[0] = 0x22;
        s[31] = 0x01;
        s
    }

    fn cli_seed() -> [u8; 32] {
        let mut s = [0u8; 32];
        s[0] = 0x33;
        s[31] = 0x02;
        s
    }

    fn cookie_a() -> [u8; 16] {
        let mut c = [0u8; 16];
        c[0] = 0xA0;
        c
    }

    fn cookie_b() -> [u8; 16] {
        let mut c = [0u8; 16];
        c[0] = 0xB0;
        c
    }

    fn make_server() -> SshTransport {
        SshTransport::server(host_seed(), srv_seed(), cookie_a())
    }

    fn make_client() -> SshTransport {
        SshTransport::client(cli_seed(), cookie_b())
    }

    fn drain(from: &mut SshTransport) -> Vec<u8> {
        let bytes = from.pending_output().to_vec();
        from.consume_output(bytes.len());
        bytes
    }

    /// Drive both ends until both are Established (or a Fail surfaces).
    fn pump(s: &mut SshTransport, c: &mut SshTransport) {
        for _ in 0..64 {
            let so = drain(s);
            if !so.is_empty() {
                c.feed(&so).expect("client feed");
            }
            let co = drain(c);
            if !co.is_empty() {
                s.feed(&co).expect("server feed");
            }
            if s.state() == State::Established && c.state() == State::Established {
                return;
            }
        }
        panic!(
            "handshake did not converge: s={:?} c={:?}",
            s.state(),
            c.state()
        );
    }

    /// Handshake that completes; returns the pair.
    fn handshake() -> (SshTransport, SshTransport) {
        let mut s = make_server();
        let mut c = make_client();
        pump(&mut s, &mut c);
        (s, c)
    }

    /// Craft a KEXINIT payload with custom lists (for hostile-negotiation
    /// tests): 10 name-lists in order, then flag + reserved.
    fn craft_kexinit(lists: [&[u8]; 10], out: &mut [u8]) -> usize {
        let mut w = Writer::new(out);
        w.u8(kex::SSH_MSG_KEXINIT).unwrap();
        w.raw(&[0u8; 16]).unwrap();
        for name in lists {
            w.string(name).unwrap();
        }
        w.u8(0).unwrap();
        w.u32(0).unwrap();
        w.into_written()
    }

    const GOOD_KEX: &[u8] = b"curve25519-sha256,curve25519-sha256@libssh.org";
    const GOOD_HK: &[u8] = b"ssh-ed25519";
    const GOOD_CIPHER: &[u8] = b"chacha20-poly1305@openssh.com";
    const GOOD_MAC: &[u8] = b"hmac-sha2-256";
    const GOOD_COMP: &[u8] = b"none";

    // ------------------------------------------------------------------
    // Full handshake
    // ------------------------------------------------------------------

    #[test]
    fn handshake_completes_and_agrees() {
        let (s, c) = handshake();
        assert_eq!(s.state(), State::Established);
        assert_eq!(c.state(), State::Established);
        assert_eq!(s.session_id(), c.session_id());
        let sid = s.session_id().unwrap();
        assert!(!sid.iter().all(|&b| b == 0));
        assert_eq!(s.exchange_hash(), sid);
        assert_eq!(c.exchange_hash(), sid);
        assert_eq!(s.peer_version(), Some("SSH-2.0-ServiceOS_0.1.0"));
        assert_eq!(c.peer_version(), Some("SSH-2.0-ServiceOS_0.1.0"));
    }

    #[test]
    fn negotiated_metadata() {
        let (s, _c) = handshake();
        let n = s.negotiated().unwrap();
        assert_eq!(n.kex, "curve25519-sha256");
        assert_eq!(n.host_key, "ssh-ed25519");
        assert_eq!(n.cipher_c2s, "chacha20-poly1305@openssh.com");
        assert_eq!(n.cipher_s2c, "chacha20-poly1305@openssh.com");
        assert_eq!(n.compression_c2s, "none");
        assert_eq!(n.compression_s2c, "none");
    }

    #[test]
    fn handshake_byte_at_a_time() {
        let mut s = make_server();
        let mut c = make_client();
        for _ in 0..64 {
            let so = drain(&mut s);
            for b in &so {
                c.feed(&[*b]).expect("client byte feed");
            }
            let co = drain(&mut c);
            for b in &co {
                s.feed(&[*b]).expect("server byte feed");
            }
            if s.state() == State::Established && c.state() == State::Established {
                assert_eq!(s.session_id(), c.session_id());
                return;
            }
        }
        panic!("bytewise handshake did not converge");
    }

    #[test]
    fn banner_available_immediately() {
        let s = make_server();
        assert_eq!(s.pending_output(), version::SERVER_BANNER);
        assert_eq!(s.state(), State::VersionExchange);
        let c = make_client();
        let co = c.pending_output();
        assert!(co.starts_with(version::SERVER_BANNER));
        // Followed by the client KEXINIT plain packet.
        assert!(co.len() > version::SERVER_BANNER.len());
    }

    #[test]
    fn session_keys_are_directional_and_deterministic() {
        let (s, c) = handshake();
        // Server TX keys (s2c) must equal client RX keys.
        assert_eq!(s.tx_keys.unwrap(), c.rx_keys.unwrap());
        // Server RX keys (c2s) must equal client TX keys.
        assert_eq!(s.rx_keys.unwrap(), c.tx_keys.unwrap());
        assert_ne!(s.tx_keys.unwrap(), s.rx_keys.unwrap());
        // Non-strict kex: sequence numbers continue across the cipher
        // switch (three TX packets sent: kexinit, reply, newkeys; three
        // RX packets received: kexinit, init, newkeys).
        assert_eq!(s.tx_seq, 3);
        assert_eq!(s.rx_seq, 3);
    }

    // ------------------------------------------------------------------
    // Encrypted data flow (established)
    // ------------------------------------------------------------------

    #[test]
    fn encrypted_server_to_client_payload() {
        let (mut s, mut c) = handshake();
        // Type 63 is unassigned: it exercises the raw established-state
        // passthrough (94+ are the channel layer now).
        s.send_payload(&[63, b'o', b'k']).unwrap();
        let out = drain(&mut s);
        match c.feed(&out).unwrap() {
            Feed::Packet { msg_type, payload } => {
                assert_eq!(msg_type, 63);
                assert_eq!(payload, &[63, b'o', b'k']);
            }
            other => panic!("expected payload, got {:?}", other),
        }
    }

    #[test]
    fn encrypted_client_to_server_answers_unimplemented() {
        let (mut s, mut c) = handshake();
        c.send_payload(&[100, 0, 0, 0, 9]).unwrap();
        let out = drain(&mut c);
        match s.feed(&out).unwrap() {
            Feed::Packet { msg_type, payload } => {
                assert_eq!(msg_type, 100);
                assert_eq!(payload, &[100, 0, 0, 0, 9]);
            }
            other => panic!("expected payload, got {:?}", other),
        }
        // The server answered SSH_MSG_UNIMPLEMENTED with the offender's
        // sequence number (the client's first post-NEWKEYS packet is its
        // fourth TX packet: seqno 3 — non-strict kex keeps counting). The
        // client treats inbound UNIMPLEMENTED as informational, so decrypt
        // it directly from the wire bytes.
        let reply = drain(&mut s);
        assert!(!reply.is_empty());
        let mut staging = [0u8; 64];
        let mut ls = [0u8; 4];
        let f = packet::decode_aead(&reply, &mut staging, &c.rx_keys.unwrap(), 3, &mut ls)
            .unwrap()
            .unwrap();
        assert_eq!(f.msg_type(&staging), 3);
        assert_eq!(f.payload(&staging), &[3, 0, 0, 0, 3]);
        // And the client consumed it silently.
        match c.feed(&reply).unwrap() {
            Feed::Progress => {}
            other => panic!("expected silent consume, got {:?}", other),
        }
    }

    #[test]
    fn tampered_post_kex_packet_disconnects_with_mac_error() {
        let (mut s, mut c) = handshake();
        c.send_payload(&[90, 1, 2, 3, 4]).unwrap();
        let mut out = drain(&mut c);
        assert!(out.len() > 12);
        let flip = out.len() - 20; // inside payload ciphertext
        out[flip] ^= 0x01;
        let err = s.feed(&out).unwrap_err();
        assert_eq!(
            err,
            Fail::LocalDisconnect {
                reason: DisconnectReason::MacError,
                description: "packet authentication failure"
            }
        );
        assert_eq!(s.state(), State::Closed);
        // The server queued an honest DISCONNECT; the client receives it.
        let disc = drain(&mut s);
        assert!(!disc.is_empty());
        match c.feed(&disc) {
            Err(Fail::PeerDisconnect { reason_code }) => assert_eq!(reason_code, 5),
            other => panic!("expected peer disconnect, got {:?}", other),
        }
        assert_eq!(c.state(), State::Closed);
        assert_eq!(c.feed(&[]), Err(Fail::Closed));
    }

    #[test]
    fn service_request_answers_service_accept() {
        let (mut s, mut c) = handshake();
        // SSH_MSG_SERVICE_REQUEST "ssh-userauth": 5 | string | string —
        // answered with SERVICE_ACCEPT (auth layer active; see auth.rs for
        // the full matrix including the rejected-service disconnect).
        let mut p = [0u8; 24];
        p[0] = 5;
        p[1..5].copy_from_slice(&12u32.to_be_bytes());
        p[5..17].copy_from_slice(b"ssh-userauth");
        c.send_payload(&p).unwrap();
        let out = drain(&mut c);
        s.feed(&out).unwrap();
        assert_eq!(s.state(), State::Established);
        assert_eq!(s.auth_phase(), crate::auth::AuthPhase::ServiceAccepted);
        let reply = drain(&mut s);
        match c.feed(&reply).unwrap() {
            Feed::Packet { msg_type, .. } => {
                assert_eq!(msg_type, crate::auth::SSH_MSG_SERVICE_ACCEPT)
            }
            other => panic!("expected SERVICE_ACCEPT, got {:?}", other),
        }
    }

    #[test]
    fn rekey_kexinit_disconnects_honestly() {
        let (mut s, mut c) = handshake();
        let mut p = [0u8; 20];
        p[0] = 20; // KEXINIT in established state
        c.send_payload(&p).unwrap();
        let out = drain(&mut c);
        assert_eq!(
            s.feed(&out).unwrap_err(),
            Fail::LocalDisconnect {
                reason: DisconnectReason::ProtocolError,
                description: "rekeying not supported"
            }
        );
    }

    #[test]
    fn peer_disconnect_is_reported_and_stored() {
        let (mut s, mut c) = handshake();
        let mut p = [0u8; 20];
        p[0] = 1;
        p[1..5].copy_from_slice(&2u32.to_be_bytes()); // PROTOCOL_ERROR
        p[5..9].copy_from_slice(&4u32.to_be_bytes());
        p[9..13].copy_from_slice(b"boom");
        s.send_payload(&p).unwrap();
        let out = drain(&mut s);
        match c.feed(&out) {
            Err(Fail::PeerDisconnect { reason_code }) => assert_eq!(reason_code, 2),
            other => panic!("expected peer disconnect, got {:?}", other),
        }
        assert_eq!(c.state(), State::Closed);
        assert_eq!(c.peer_disconnect_reason(), Some(2));
        assert_eq!(c.peer_disconnect_description(), Some("boom"));
        assert_eq!(c.feed(&[]), Err(Fail::Closed));
        // The server that SENT the DISCONNECT as a payload stays usable.
        assert!(s.send_payload(&[94]).is_ok());
    }

    // ------------------------------------------------------------------
    // Handshake failures
    // ------------------------------------------------------------------

    #[test]
    fn kex_mismatch_disconnects() {
        let mut s = make_server();
        s.feed(b"SSH-2.0-hostile\r\n").unwrap();
        let mut kbuf = [0u8; 1024];
        let kn = craft_kexinit(
            [
                b"ecdh-sha2-nistp256",
                GOOD_HK,
                GOOD_CIPHER,
                GOOD_CIPHER,
                GOOD_MAC,
                GOOD_MAC,
                GOOD_COMP,
                GOOD_COMP,
                b"",
                b"",
            ],
            &mut kbuf,
        );
        let mut pkt = [0u8; 2048];
        let pn = packet::encode_plain(&kbuf[..kn], &mut pkt).unwrap();
        assert_eq!(
            s.feed(&pkt[..pn]).unwrap_err(),
            Fail::LocalDisconnect {
                reason: DisconnectReason::KeyExchangeFailed,
                description: "no common kex algorithm"
            }
        );
    }

    #[test]
    fn zero_ecdh_init_rejected() {
        let mut s = make_server();
        s.feed(b"SSH-2.0-hostile\r\n").unwrap();
        let mut kbuf = [0u8; 1024];
        let kn = craft_kexinit(
            [
                GOOD_KEX,
                GOOD_HK,
                GOOD_CIPHER,
                GOOD_CIPHER,
                GOOD_MAC,
                GOOD_MAC,
                GOOD_COMP,
                GOOD_COMP,
                b"",
                b"",
            ],
            &mut kbuf,
        );
        let mut pkt = [0u8; 2048];
        let pn = packet::encode_plain(&kbuf[..kn], &mut pkt).unwrap();
        s.feed(&pkt[..pn]).unwrap();
        // All-zero e.
        let mut p = [0u8; 40];
        p[0] = 30;
        p[1..5].copy_from_slice(&32u32.to_be_bytes());
        let mut pkt2 = [0u8; 128];
        let pn2 = packet::encode_plain(&p, &mut pkt2).unwrap();
        assert_eq!(
            s.feed(&pkt2[..pn2]).unwrap_err(),
            Fail::LocalDisconnect {
                reason: DisconnectReason::KeyExchangeFailed,
                description: "invalid client ephemeral public key"
            }
        );
    }

    #[test]
    fn corrupted_host_signature_rejected() {
        let mut s = make_server();
        let mut c = make_client();
        // Drive until the server has emitted reply + NEWKEYS (plain).
        for _ in 0..8 {
            let so = drain(&mut s);
            if !so.is_empty() {
                c.feed(&so).unwrap();
            }
            let co = drain(&mut c);
            if !co.is_empty() {
                s.feed(&co).unwrap();
            }
            if s.state() == State::NewKeys {
                break;
            }
        }
        assert_eq!(s.state(), State::NewKeys);
        let mut out = drain(&mut s); // reply + NEWKEYS plain packets
        // Flip a byte inside the reply packet's signature (well past the
        // length/padlen fields of the first packet).
        let flip = out.len() - 40;
        out[flip] ^= 0x10;
        match c.feed(&out) {
            Err(Fail::LocalDisconnect {
                reason: DisconnectReason::HostKeyNotVerifiable,
                ..
            }) => {}
            other => panic!("expected host key failure, got {:?}", other),
        }
    }

    #[test]
    fn oversized_kexinit_rejected() {
        let mut s = make_server();
        s.feed(b"SSH-2.0-hostile\r\n").unwrap();
        let mut payload = vec![20u8; 8300];
        payload[1..17].copy_from_slice(&[0u8; 16]);
        let mut pkt = vec![0u8; 8400];
        let pn = packet::encode_plain(&payload, &mut pkt).unwrap();
        assert_eq!(
            s.feed(&pkt[..pn]).unwrap_err(),
            Fail::LocalDisconnect {
                reason: DisconnectReason::ProtocolError,
                description: "KEXINIT payload too large"
            }
        );
    }

    #[test]
    fn version_exchange_failures() {
        // Oversized line without LF.
        let mut s = make_server();
        assert_eq!(
            s.feed(&[b'a'; 300]).unwrap_err(),
            Fail::LocalDisconnect {
                reason: DisconnectReason::ProtocolError,
                description: "identification line too long"
            }
        );
        // Wrong protocol version.
        let mut s2 = make_server();
        assert_eq!(
            s2.feed(b"SSH-1.5-ancient\r\n").unwrap_err(),
            Fail::LocalDisconnect {
                reason: DisconnectReason::ProtocolVersionNotSupported,
                description: "unsupported SSH protocol version"
            }
        );
        // Client pre-banner lines are a protocol violation.
        let mut s3 = make_server();
        assert!(s3.feed(b"HTTP/1.1 400\r\nSSH-2.0-x\r\n").is_err());
        // Garbage first line.
        let mut s4 = make_server();
        assert!(s4.feed(b"hello there\r\n").is_err());
    }

    #[test]
    fn out_of_order_kex_messages_rejected() {
        // ECDH_INIT before any KEXINIT.
        let mut s = make_server();
        s.feed(b"SSH-2.0-hostile\r\n").unwrap();
        let mut p = [0u8; 40];
        p[0] = 30;
        p[1..5].copy_from_slice(&32u32.to_be_bytes());
        p[5] = 0x11;
        let mut pkt = [0u8; 128];
        let pn = packet::encode_plain(&p, &mut pkt).unwrap();
        assert!(s.feed(&pkt[..pn]).is_err());
        assert_eq!(s.state(), State::Closed);

        // NEWKEYS before key exchange.
        let mut s2 = make_server();
        s2.feed(b"SSH-2.0-hostile\r\n").unwrap();
        let mut pkt2 = [0u8; 64];
        let pn2 = packet::encode_plain(&[21u8], &mut pkt2).unwrap();
        assert!(s2.feed(&pkt2[..pn2]).is_err());
        assert_eq!(s2.state(), State::Closed);
    }

    // ------------------------------------------------------------------
    // API edges
    // ------------------------------------------------------------------

    #[test]
    fn send_payload_requires_established() {
        let mut s = make_server();
        assert_eq!(s.send_payload(&[94]), Err(Fail::NotReady));
        assert_eq!(s.state(), State::VersionExchange);
    }

    #[test]
    fn out_of_capacity_then_drain_recovers() {
        let (mut s, mut c) = handshake();
        // Fill the output buffer until it refuses (honest backpressure).
        let mut sent = 0;
        let mut hit_cap = false;
        for _ in 0..4000 {
            // 4-byte payload: each request's wire footprint (36) exceeds the
            // UNIMPLEMENTED reply's (32), so the client's queued replies can
            // never outgrow the space the requests occupied. Type 63 is
            // unassigned — it exercises the UNIMPLEMENTED passthrough path
            // (types 90-100 are the channel layer now).
            match s.send_payload(&[63, 0, 0, 0]) {
                Ok(()) => sent += 1,
                Err(Fail::OutOfCapacity) => {
                    hit_cap = true;
                    break;
                }
                other => panic!("unexpected {:?}", other),
            }
        }
        assert!(hit_cap);
        assert!(sent > 0);
        let out = drain(&mut s);
        c.feed(&out).unwrap();
        let mut count = 1;
        loop {
            match c.feed(&[]).unwrap() {
                Feed::Packet { msg_type, .. } => {
                    assert_eq!(msg_type, 63);
                    count += 1;
                }
                Feed::Progress => break,
                other => panic!("unexpected feed outcome {:?}", other),
            }
        }
        assert_eq!(count, sent);
        // Partial drain frees space for another packet.
        let mut freed = 0;
        loop {
            match s.send_payload(&[95, 1]) {
                Ok(()) => {}
                Err(Fail::OutOfCapacity) => {
                    s.consume_output(64);
                    freed += 1;
                    if freed > 4 {
                        panic!("drain did not free capacity");
                    }
                }
                other => panic!("unexpected {:?}", other),
            }
            if freed >= 1 {
                break;
            }
        }
        assert!(s.send_payload(&[95, 1]).is_ok());
        assert_eq!(s.state(), State::Established);
    }

    #[test]
    fn seqno_wrap_produces_distinct_nonces() {
        let (mut s, _c) = handshake();
        // Force the TX sequence to the wrap edge and check both packets
        // decrypt under their own nonce construction.
        s.tx_seq = u32::MAX;
        s.send_payload(&[94, 1]).unwrap();
        s.send_payload(&[94, 2]).unwrap();
        let out = drain(&mut s);
        let keys = s.tx_keys.unwrap();
        let mut staging = [0u8; 64];
        let mut ls = [0u8; 4];
        let f1 = packet::decode_aead(&out, &mut staging, &keys, u32::MAX, &mut ls)
            .unwrap()
            .unwrap();
        assert_eq!(f1_payload(&staging, &f1), &[94, 1]);
        let rest = &out[f1.consumed..];
        let f2 = packet::decode_aead(rest, &mut staging, &keys, 0, &mut ls)
            .unwrap()
            .unwrap();
        assert_eq!(f2.payload(&staging), &[94, 2]);
        // The two ciphertexts differ despite identical plaintext (nonce).
        assert_ne!(&out[..f1.consumed], &rest[..f1.consumed]);
    }

    fn f1_payload<'a>(staging: &'a [u8], f: &packet::FrameInfo) -> &'a [u8] {
        f.payload(staging)
    }

    #[test]
    fn ignore_and_debug_are_consumed_silently() {
        let (mut s, mut c) = handshake();
        c.send_payload(&[2, 1, 2, 3]).unwrap(); // IGNORE
        c.send_payload(&[4, 0, 0, 0, 1, 7]).unwrap(); // DEBUG
        let out = drain(&mut c);
        assert!(matches!(s.feed(&out), Ok(Feed::Progress)));
        assert!(drain(&mut s).is_empty());
        assert_eq!(s.state(), State::Established);
    }
}

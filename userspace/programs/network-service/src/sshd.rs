//! Build-gated SSH listener: drives the `shared/ssh` `SshTransport` server
//! state machine over a TCP:22 connection adopted from the generic listener
//! pool.
//!
//! Driving model (one pass per main-loop iteration, after `pump_listeners`):
//! 1. bind-once: open a listener slot on :22 through the internal listener
//!    contract (`open_listener`) with a self-addressed reply channel.
//! 2. drain that slot's accept queue: each adopted transport slot becomes an
//!    sshd session owning one `SshTransport::server(...)` instance (~139 KiB
//!    of static buffers — hence single-session v0). Surplus connections are
//!    closed cleanly via the transport-slot teardown.
//! 3. per session: socket recv -> `feed()`, then drain `pending_output()`
//!    into the socket send queue; partial writes retry on later iterations.
//!
//! Honest v0 bridging surface: the library owns the established-state policy
//! (disconnect with ServiceNotAvailable on SSH_MSG_SERVICE_REQUEST, rekey
//! refused, unknown types answered UNIMPLEMENTED and passed through — the
//! first passthrough ends the session politely). No authentication, no
//! channels this wave: the operator-visible proof is a full version exchange
//! + KEX + NEWKEYS against a real client, then a protocol-level disconnect
//! (docs/roadmap.md, SSH transport row).
//!
//! Host key: Ed25519 seed derived from guest-local entropy substitutes
//! (SHA-512 over a source label, MAC, monotonic tick — package-service's
//! honest-unique-ish substitute; no hardware RNG). Boot-local this wave:
//! network-service holds no storage handle at startup, so the key changes
//! across boots and host-side known_hosts pinning is meaningless here. Same
//! honest limits as backup-service's signing identity.

use crate::consts::SSHD_LISTEN_PORT;
use crate::types::{TcpListenerSlot, TcpTransportSlot};
use core::cell::UnsafeCell;
use serviceos_ssh::auth::AuthPhase;
use serviceos_ssh::transport::{Feed, SshTransport, State};
use serviceos_userspace_runtime as rt;
use smoltcp::iface::SocketHandle;
use smoltcp::iface::SocketSet;
use smoltcp::socket::tcp;

/// Shell public-channel wire tags. Duplicated locally (not imported)
/// following desktop-shell-service's precedent — the shell publishes these
/// in its own crate range (0x240-0x244), and network-service avoids a
/// dependency on the whole shell library for five constants.
mod shell_tag {
    pub const SESSION_OPEN_REQUEST: u32 = 0x240;
    pub const SESSION_OPEN_REPLY: u32 = 0x241;
    pub const SESSION_INPUT_LINE: u32 = 0x242;
    pub const SESSION_OUTPUT_TEXT: u32 = 0x243;
    pub const SESSION_CLOSE: u32 = 0x244;
    /// Additive verify relay (sshd bridge); served by the shell public
    /// channel with the same status-first reply convention.
    pub const VERIFY_PASSWORD_REQUEST: u32 = 0x245;
    pub const VERIFY_PASSWORD_REPLY: u32 = 0x246;
}

/// The prompt the shell echoes after each executed line; also emitted by
/// this bridge when the session opens and on empty enters (the shell client
/// path only prompts after output).
const SHELL_PROMPT: &str = "serviceos> ";
/// Line capacity: shell-service accepts MAX_LINE_BYTES (128) but its
/// client path can only carry (IPC_MAX_WORDS-1)*8 = 120 payload bytes per
/// SESSION_INPUT_LINE, so the honest cap for remote lines is 120.
const LINE_CAP: usize = 120;
/// Inbound channel-data chunks are copied here from the transport's staging
/// buffer before line processing (interactive keystroke batches are tiny).
const INPUT_CHUNK_CAP: usize = 256;
/// Account-service VERIFY_PASSWORD scratch (bounds mirror account-service).
const AUTH_USER_CAP: usize = 32;
const AUTH_PASS_CAP: usize = 64;
/// Bounded polls for the shell session open reply (one per main-loop pass).
const OPEN_WAIT_PASSES: u16 = 1000;
/// Cooldown passes before retrying an unreachable shell service.
const VERIFY_COOLDOWN_PASSES: u16 = 512;
/// Passes between a peer EOF and our channel close (output drain grace).
const EOF_GRACE_PASSES: u16 = 16;

/// Shell-mediated verify lifecycle. The shell owns the account-service
/// relationship (its manifest grants the storage lookup and the stored-image
/// launch, and it caches the account channel); this side only resolves the
/// shell service and exchanges the additive VERIFY_PASSWORD round trip.
enum AccountPhase {
    /// Nothing in flight.
    Idle,
    /// Shell service resolved; no verify in flight.
    Ready { channel: rt::Handle },
    /// VERIFY_PASSWORD sent; polling the reply channel.
    VerifyPending {
        channel: rt::Handle,
        reply: rt::Handle,
    },
    /// Shell unreachable; retry after a cooldown.
    Unavailable { cooldown: u16 },
}

/// Bridge from the SSH session channel to a shell operator session.
enum BridgePhase {
    /// No shell session yet (or the last one is gone).
    Closed,
    /// SESSION_OPEN_REQUEST sent; awaiting the reply on `carrier`.
    Opening { carrier: rt::Handle, budget: u16 },
    /// Operator session live; `endpoint` receives input lines and serves
    /// output text.
    Open { endpoint: rt::Handle },
}

/// Per-session bridge state (bounded storage in `.bss` via SshdSlot).
struct SessionBridge {
    account: AccountPhase,
    bridge: BridgePhase,
    shell_handle: Option<rt::Handle>,
    prompt_sent: bool,
    /// Credentials copied out of the transport while verification runs;
    /// zeroed once the verdict lands.
    auth_user: [u8; AUTH_USER_CAP],
    auth_user_len: usize,
    auth_pass: [u8; AUTH_PASS_CAP],
    auth_pass_len: usize,
    verify_in_flight: bool,
    /// Interactive line assembly.
    line: [u8; LINE_CAP],
    line_len: usize,
    /// Line completed before the shell operator session opened (the client
    /// can type faster than the bridge opens); submitted on open.
    pending_line: [u8; LINE_CAP],
    pending_len: usize,
    /// Passes remaining before a peer EOF closes the channel (grace so the
    /// shell's pending output drains first).
    eof_grace: u16,
}

impl SessionBridge {
    const fn new() -> SessionBridge {
        SessionBridge {
            account: AccountPhase::Idle,
            bridge: BridgePhase::Closed,
            shell_handle: None,
            prompt_sent: false,
            auth_user: [0; AUTH_USER_CAP],
            auth_user_len: 0,
            auth_pass: [0; AUTH_PASS_CAP],
            auth_pass_len: 0,
            verify_in_flight: false,
            line: [0; LINE_CAP],
            line_len: 0,
            pending_line: [0; LINE_CAP],
            pending_len: 0,
            eof_grace: 0,
        }
    }

    fn zero_credentials(&mut self) {
        self.auth_user.fill(0);
        self.auth_user_len = 0;
        self.auth_pass.fill(0);
        self.auth_pass_len = 0;
        self.verify_in_flight = false;
    }

    fn reset(&mut self) {
        self.zero_credentials();
        self.line_len = 0;
        self.pending_len = 0;
        self.eof_grace = 0;
        self.prompt_sent = false;
        // Account channel stays cached across sessions (the service keeps
        // running); the shell session endpoint does not. SESSION_CLOSE lets
        // the shell release the operator-session row cleanly.
        if let BridgePhase::Open { endpoint } = self.bridge {
            let mut close = rt::RawMessage::empty(shell_tag::SESSION_CLOSE);
            let _ = rt::channel_send(endpoint, &close);
            let _ = rt::handle_close(endpoint);
        }
        if let BridgePhase::Opening { carrier, .. } = self.bridge {
            let _ = rt::handle_close(carrier);
        }
        if let AccountPhase::VerifyPending { reply, .. } = self.account {
            let _ = rt::handle_close(reply);
        }
        self.bridge = BridgePhase::Closed;
        if let AccountPhase::VerifyPending { channel, .. } = self.account {
            self.account = AccountPhase::Ready { channel };
        }
    }
}

/// SSH listener state. Flat on purpose: the transport's ~139 KiB of fixed
/// buffers are embedded directly so the whole struct lives in `.bss` (via
/// `SshdSlot`); wrapping the transport in `Option<SshSession>` would move it
/// through a return slot on adoption and overflow the run-task stack.
pub(crate) struct SshdState {
    bound: bool,
    listener_slot: usize,
    /// True while an adopted connection owns `transport`.
    session_active: bool,
    transport_index: usize,
    established_logged: bool,
    /// Slot awaiting teardown while its queued output (the DISCONNECT) is
    /// still being transmitted; teardown must not abort the socket before
    /// smoltcp flushes the queue or the client never sees the reason.
    pending_close: Option<usize>,
    close_grace: u32,
    transport: SshTransport,
    bridge: SessionBridge,
}

/// Main-loop iterations granted to flush the disconnect before aborting.
const CLOSE_GRACE_TICKS: u32 = 8;

impl SshdState {
    pub(crate) const fn new() -> SshdState {
        SshdState {
            bound: false,
            listener_slot: 0,
            session_active: false,
            transport_index: 0,
            established_logged: false,
            pending_close: None,
            close_grace: 0,
            transport: SshTransport::placeholder(),
            bridge: SessionBridge::new(),
        }
    }

    /// Begin a session for an adopted transport slot: reset the transport
    /// in place and queue the server banner. Surplus adoption is refused by
    /// the caller while `session_active`.
    fn begin_session(&mut self, transport_index: usize, seeds: Seeds) {
        self.session_active = true;
        self.transport_index = transport_index;
        self.established_logged = false;
        self.bridge = SessionBridge::new();
        self.transport.init_server(seeds.0, seeds.1, seeds.2);
    }
}

/// Host-key material tuple: (ed25519 host seed, ephemeral KEX seed, cookie).
pub(crate) type Seeds = ([u8; 32], [u8; 32], [u8; 16]);

/// Static home for the SSH listener state. `SshdState` embeds one
/// `SshTransport` (~139 KiB of fixed buffers — see shared/ssh); it must live
/// in .bss, never on the run-task stack, which is a small fixed budget.
pub(crate) struct SshdSlot(UnsafeCell<SshdState>);

impl SshdSlot {
    pub(crate) const fn new() -> SshdSlot {
        SshdSlot(UnsafeCell::new(SshdState::new()))
    }
    /// SAFETY: the network-service task is strictly single-threaded; no
    /// other context touches this slot between pump passes.
    #[allow(clippy::mut_from_ref)]
    pub(crate) fn get(&self) -> &mut SshdState {
        unsafe { &mut *self.0.get() }
    }
}

// SAFETY: single-threaded access per the guarantee above.
unsafe impl Sync for SshdSlot {}

/// One pass's noteworthy event (for the main loop's log/witness path).
pub(crate) enum PumpNote {
    /// Nothing happened.
    Quiet,
    /// The listener bound for the first time (message is the boot line).
    Listening(&'static str),
    /// The accepted connection reached Established (KEX + NEWKEYS complete).
    Established,
    /// The session ended (peer gone or protocol-level close); message is the
    /// reason line.
    Ended(&'static str),
}

/// Derive the host-key material from guest-local entropy substitutes:
/// SHA-512 over (source label, MAC, monotonic tick). Returns
/// (host_seed, kex_seed, kexinit_cookie). HONEST LIMITS mirror
/// package-service's `derive_generated_identity`: no hardware RNG, the tick
/// may stand still — seeds are UNIQUE-ISH, not cryptographically random.
pub(crate) fn derive_host_seeds(source: &[u8], mac: &[u8; 6], tick: u64) -> Seeds {
    let mut block = [0u8; 64];
    let prefix_len = source.len().min(40);
    block[..prefix_len].copy_from_slice(&source[..prefix_len]);
    block[40..46].copy_from_slice(mac);
    block[46..54].copy_from_slice(&tick.to_le_bytes());
    let digest = serviceos_crypto::sha512::digest(&[&block]);
    let mut host_seed = [0u8; 32];
    host_seed.copy_from_slice(&digest[..32]);
    let mut kex_seed = [0u8; 32];
    kex_seed.copy_from_slice(&digest[32..64]);
    // The KEXINIT cookie needs no secrecy; derive it from a second hash so
    // it does not reuse seed material.
    let cookie_digest = serviceos_crypto::sha512::digest(&[b"network-service-sshd-cookie"]);
    let mut cookie = [0u8; 16];
    cookie.copy_from_slice(&cookie_digest[..16]);
    (host_seed, kex_seed, cookie)
}

/// One pump pass for the gated SSH listener. Called from the main loop with
/// the same machinery the listener subsystem owns; `seeds` comes from
/// `derive_host_seeds` computed once at startup.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pump(
    state: &mut SshdState,
    seeds: Seeds,
    bootstrap: rt::Handle,
    log_handle: rt::Handle,
    listeners: &mut [TcpListenerSlot; crate::consts::MAX_TCP_LISTENERS],
    transports: &mut [TcpTransportSlot; crate::consts::MAX_TCP_SOCKETS],
    tcp_handles: [SocketHandle; crate::consts::MAX_TCP_SOCKETS],
    sockets: &mut SocketSet<'_>,
) -> PumpNote {
    if !crate::consts::sshd_enabled() {
        return PumpNote::Quiet;
    }
    if let Some(index) = state.pending_close {
        return finish_close(state, log_handle, listeners, transports, sockets, index);
    }
    if !state.bound {
        if !bind_listener(log_handle, listeners, transports, tcp_handles, sockets) {
            return PumpNote::Quiet;
        }
        state.bound = true;
        state.listener_slot = listeners
            .iter()
            .position(|slot| slot.active && slot.local_port == SSHD_LISTEN_PORT)
            .unwrap_or(0);
        return PumpNote::Listening("network: sshd listening port=22");
    }
    if !state.session_active && state.pending_close.is_none() {
        adopt_inbound(state, seeds, log_handle, listeners, transports, sockets);
    }
    if !state.session_active {
        return PumpNote::Quiet;
    }
    let transport_index = state.transport_index;
    let Some(slot) = transports.get_mut(transport_index) else {
        state.session_active = false;
        return PumpNote::Ended("network: sshd session dropped (slot vanished)");
    };
    if !slot.active {
        // The transport-state pump already tore the slot down (peer reset).
        state.session_active = false;
        return PumpNote::Ended("network: sshd session dropped (peer closed)");
    }
    let Some(socket_handle) = slot.socket_handle else {
        state.session_active = false;
        return PumpNote::Ended("network: sshd session dropped (no socket)");
    };
    let outcome = {
        let socket = sockets.get_mut::<tcp::Socket>(socket_handle);
        // The generic transport pump keeps adopted slots marked active even
        // after the peer is gone (it never clears `active` on its own), so
        // the session must end itself once the TCP connection is no longer
        // alive — otherwise single-session v0 wedges on the first abrupt
        // client close and every later connect is refused pre-banner.
        // SynReceived is a normal transient right after adoption (the
        // three-way handshake may still be completing).
        let tcp_state = socket.state();
        let session_dead = !matches!(tcp_state, tcp::State::SynReceived | tcp::State::Established);
        if session_dead {
            state.session_active = false;
            crate::protocol::close_transport_slot(
                log_handle,
                sockets,
                &mut transports[transport_index],
            );
            return PumpNote::Ended("network: sshd session ended (peer closed)");
        }
        let mut stream = SmolStream(socket);
        drive_session(&mut stream, &mut state.transport, &mut state.bridge)
    };
    // Established-state service pumps: the verifier and the shell bridge
    // advance once per pass and may queue replies that need a second TX
    // flush. Both are honest no-ops until the transport reaches
    // Established — which is also the outcome drive_session reports on
    // every pass after KEX, so this must not live inside one match arm.
    if state.transport.state() == State::Established {
        pump_auth(
            &mut state.bridge,
            &mut state.transport,
            bootstrap,
            log_handle,
        );
        pump_shell(
            &mut state.bridge,
            &mut state.transport,
            bootstrap,
            log_handle,
        );
        // Peer half-close (EOF): complete our half honestly (EOF + CLOSE)
        // and release the shell session; the TCP teardown follows the
        // client's full close.
        if state.transport.channel_eof_in() && !state.transport.channel_closed() {
            let _ = state.transport.send_channel_eof();
            let _ = state.transport.send_channel_close();
            state.bridge.reset();
            let _ = rt::write_logf(
                "network",
                format_args!("sshd channel eof (client half-close)"),
            );
        }
        if transport_queued(&state.transport) {
            if let Some(socket_handle) = transports
                .get(state.transport_index)
                .and_then(|slot| slot.socket_handle)
            {
                let socket = sockets.get_mut::<tcp::Socket>(socket_handle);
                let mut stream = SmolStream(socket);
                flush_tx(&mut stream, &mut state.transport);
            }
        }
    }
    match outcome {
        SessionOutcome::Progress => PumpNote::Quiet,
        SessionOutcome::Established => {
            if state.established_logged {
                PumpNote::Quiet
            } else {
                state.established_logged = true;
                PumpNote::Established
            }
        }
        SessionOutcome::Ended(reason) => {
            // Release the session's IPC surface (shell endpoint, verify
            // reply channel, parked credentials) before the slot teardown.
            state.bridge.reset();
            // Keep the slot alive for a few iterations so smoltcp can
            // transmit the queued DISCONNECT before the teardown aborts
            // the socket; aborting immediately turns the protocol-level
            // close into a silent RST for the client.
            state.session_active = false;
            state.pending_close = Some(transport_index);
            state.close_grace = CLOSE_GRACE_TICKS;
            PumpNote::Ended(reason)
        }
    }
}

/// Wait for the closing slot's send queue to drain (bounded by
/// `CLOSE_GRACE_TICKS`), then tear the slot down.
fn finish_close(
    state: &mut SshdState,
    log_handle: rt::Handle,
    listeners: &mut [TcpListenerSlot; crate::consts::MAX_TCP_LISTENERS],
    transports: &mut [TcpTransportSlot; crate::consts::MAX_TCP_SOCKETS],
    sockets: &mut SocketSet<'_>,
    index: usize,
) -> PumpNote {
    let _ = (log_handle, listeners);
    state.close_grace = state.close_grace.saturating_sub(1);
    let drained = match transports.get(index).and_then(|slot| slot.socket_handle) {
        Some(handle) => sockets.get_mut::<tcp::Socket>(handle).send_queue() == 0,
        None => true,
    };
    if !drained && state.close_grace > 0 {
        return PumpNote::Quiet;
    }
    if let Some(slot) = transports.get_mut(index) {
        crate::protocol::close_transport_slot(log_handle, sockets, slot);
    }
    state.pending_close = None;
    PumpNote::Quiet
}

enum SessionOutcome {
    Progress,
    Established,
    Ended(&'static str),
}

/// Bind-once through the internal listener contract. Same-service bind: no
/// IPC reply channel is involved (see `open_internal_listener`).
fn bind_listener(
    log_handle: rt::Handle,
    listeners: &mut [TcpListenerSlot; crate::consts::MAX_TCP_LISTENERS],
    transports: &[TcpTransportSlot; crate::consts::MAX_TCP_SOCKETS],
    tcp_handles: [SocketHandle; crate::consts::MAX_TCP_SOCKETS],
    sockets: &mut SocketSet<'_>,
) -> bool {
    crate::protocol::open_internal_listener(
        log_handle,
        listeners,
        transports,
        tcp_handles,
        sockets,
        SSHD_LISTEN_PORT,
        1,
    )
}

#[allow(clippy::too_many_arguments)]
fn adopt_inbound(
    state: &mut SshdState,
    seeds: Seeds,
    log_handle: rt::Handle,
    listeners: &mut [TcpListenerSlot; crate::consts::MAX_TCP_LISTENERS],
    transports: &mut [TcpTransportSlot; crate::consts::MAX_TCP_SOCKETS],
    sockets: &mut SocketSet<'_>,
) {
    let Some(listener_index) = listeners
        .iter()
        .position(|slot| slot.active && slot.local_port == SSHD_LISTEN_PORT)
    else {
        return;
    };
    while let Some((transport_index, external_half)) = listeners[listener_index].pop_accept() {
        // The channel half queued for external clients is unused on the
        // internal path; close it so the handle budget stays clean.
        let _ = rt::handle_close(external_half);
        if state.session_active {
            // Single-session v0: refuse surplus connections cleanly.
            crate::protocol::close_transport_slot(
                log_handle,
                sockets,
                &mut transports[transport_index],
            );
            continue;
        }
        state.begin_session(transport_index, seeds);
    }
}

/// Minimal nonblocking stream surface the transport pump needs. The smoltcp
/// adapter wraps an adopted socket; the host tests drive a mock duplex.
trait StreamLike {
    /// Read whatever is available into `buf`; Ok(0) = nothing right now,
    /// Err = the connection is gone.
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, ()>;
    /// Enqueue as much of `data` as fits; partial writes are legal.
    fn send(&mut self, data: &[u8]) -> Result<usize, ()>;
}

/// Adapter over one adopted smoltcp TCP socket.
struct SmolStream<'a, 'b>(&'a mut tcp::Socket<'b>);

impl StreamLike for SmolStream<'_, '_> {
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        if !self.0.can_recv() {
            return Ok(0);
        }
        self.0.recv_slice(buf).map_err(|_| ())
    }
    fn send(&mut self, data: &[u8]) -> Result<usize, ()> {
        if !self.0.can_send() {
            return Ok(0);
        }
        self.0.send_slice(data).map_err(|_| ())
    }
}

/// Core transport pump: recv -> feed, then drain queued output into the
/// stream (partial writes retry on later loop iterations). Channel-data
/// batches are copied out of the staging buffer and fed through the bridge's
/// line discipline (echo + line assembly); parked authentication attempts
/// are surfaced to `pump_auth` via the transport's pending state.
fn drive_session(
    stream: &mut dyn StreamLike,
    transport: &mut SshTransport,
    bridge: &mut SessionBridge,
) -> SessionOutcome {
    let mut buffer = [0u8; 256];
    let mut protocol_failed = false;
    // RX: hand everything available to the state machine.
    loop {
        let received = match stream.recv(&mut buffer) {
            Ok(count) => count,
            Err(()) => {
                return SessionOutcome::Ended("network: sshd session closed (connection lost)");
            }
        };
        if received == 0 {
            break;
        }
        match transport.feed(&buffer[..received]) {
            Ok(Feed::Progress) => {}
            Ok(Feed::AuthQuery) => {
                // Processing stalls until the verifier delivers a verdict;
                // pump_auth advances it after the TX drain below.
            }
            Ok(Feed::ChannelData { data }) => {
                // Copy out of the staging buffer so the line discipline can
                // push echo bytes back through the transport.
                let mut chunk = [0u8; INPUT_CHUNK_CAP];
                let n = data.len().min(chunk.len());
                chunk[..n].copy_from_slice(&data[..n]);
                let consumed = data.len().min(chunk.len());
                transport.ack_channel_data(consumed).ok();
                feed_line_input(bridge, transport, &chunk[..n]);
            }
            Ok(Feed::Packet { .. }) => {
                // Unknown established-state message: the library already
                // queued SSH_MSG_UNIMPLEMENTED; keep the v0 policy of one
                // polite passthrough then close.
                protocol_failed = true;
                break;
            }
            Err(_) => {
                // fail_disconnect queued an SSH_MSG_DISCONNECT under the
                // current framing; fall through to flush it, then end.
                protocol_failed = true;
                break;
            }
        }
    }
    // TX: drain queued wire bytes into the stream.
    flush_tx(stream, transport);
    match transport.state() {
        State::Closed => {
            if protocol_failed {
                SessionOutcome::Ended("network: sshd session closed (protocol disconnect)")
            } else {
                SessionOutcome::Ended("network: sshd session closed (peer disconnect)")
            }
        }
        State::Established => SessionOutcome::Established,
        _ => SessionOutcome::Progress,
    }
}

/// Drain the transport's queued wire bytes into the stream (partial writes
/// retry on later iterations).
fn flush_tx(stream: &mut dyn StreamLike, transport: &mut SshTransport) {
    loop {
        let pending = transport.pending_output();
        if pending.is_empty() {
            break;
        }
        let copied = match stream.send(pending) {
            Ok(written) => written,
            Err(()) => return,
        };
        if copied == 0 {
            break;
        }
        transport.consume_output(copied);
    }
}

fn transport_queued(transport: &SshTransport) -> bool {
    !transport.pending_output().is_empty()
}

// ----------------------------------------------------------------------
// Authentication bridge: account-service VERIFY_PASSWORD
// ----------------------------------------------------------------------

/// Advance the account-service verifier one step per pump pass. The pure
/// SSH library parks password attempts (Feed::AuthQuery); this side performs
/// the actual credential check over IPC and feeds the verdict back.
fn pump_auth(
    bridge: &mut SessionBridge,
    transport: &mut SshTransport,
    bootstrap: rt::Handle,
    log_handle: rt::Handle,
) {
    if transport.auth_phase() != AuthPhase::Pending {
        return;
    }
    // Take freshly parked credentials once.
    if !bridge.verify_in_flight && bridge.auth_user_len == 0 {
        let mut user = [0u8; AUTH_USER_CAP];
        let mut pass = [0u8; AUTH_PASS_CAP];
        if let Some((u, p)) = transport.take_auth_request(&mut user, &mut pass) {
            bridge.auth_user[..u].copy_from_slice(&user[..u]);
            bridge.auth_user_len = u;
            bridge.auth_pass[..p].copy_from_slice(&pass[..p]);
            bridge.auth_pass_len = p;
            let user_text = core::str::from_utf8(&bridge.auth_user[..u]).unwrap_or("?");
            let _ = rt::write_logf(
                "network",
                format_args!("sshd auth attempt user={user_text}"),
            );
        }
    }
    // Advance the account channel lifecycle one step.
    advance_account_phase(bridge, bootstrap, log_handle);
    // Parked credentials + reachable account service -> send the verify RPC.
    if bridge.auth_user_len > 0 && !bridge.verify_in_flight {
        if let AccountPhase::Ready { channel } = bridge.account {
            send_verify_request(bridge, channel);
        }
    }
    // Verify in flight -> poll for the reply and deliver the verdict.
    if bridge.verify_in_flight {
        if let AccountPhase::VerifyPending { channel, reply } = bridge.account {
            let mut response = rt::RawMessage::empty(0);
            match rt::channel_receive_nonblocking(reply, &mut response) {
                Ok(()) => {
                    let _ = rt::handle_close(reply);
                    let valid = response.tag == shell_tag::VERIFY_PASSWORD_REPLY
                        && response.word_count >= 2
                        && response.words[0] == 0
                        && response.words[1] == 1;
                    bridge.account = AccountPhase::Ready { channel };
                    deliver_verdict(bridge, transport, valid, log_handle);
                }
                Err(rt::Error::QueueEmpty) => {}
                Err(_) => {
                    let _ = rt::handle_close(reply);
                    bridge.account = AccountPhase::Unavailable {
                        cooldown: VERIFY_COOLDOWN_PASSES,
                    };
                    deliver_verdict(bridge, transport, false, log_handle);
                }
            }
        }
    }
    // Account service unavailable while credentials are parked: deny after a
    // bounded number of passes so the client never hangs forever.
    if bridge.auth_user_len > 0 && !bridge.verify_in_flight {
        if let AccountPhase::Unavailable { cooldown } = bridge.account {
            if cooldown == 0 {
                let _ = rt::write_logf(
                    "network",
                    format_args!("sshd auth denied (verifier unavailable)"),
                );
                deliver_verdict(bridge, transport, false, log_handle);
            }
        }
    }
}

fn advance_account_phase(
    bridge: &mut SessionBridge,
    bootstrap: rt::Handle,
    log_handle: rt::Handle,
) {
    match bridge.account {
        AccountPhase::Idle => match rt::lookup_service(bootstrap, rt::ServiceId::Shell) {
            Ok(channel) => {
                bridge.account = AccountPhase::Ready { channel };
            }
            Err(_) => {
                let _ = rt::write_logf(
                    "network",
                    format_args!("sshd: shell service unreachable for verify"),
                );
                bridge.account = AccountPhase::Unavailable {
                    cooldown: VERIFY_COOLDOWN_PASSES,
                };
            }
        },
        AccountPhase::Unavailable { cooldown } => {
            bridge.account = if cooldown > 0 {
                AccountPhase::Unavailable {
                    cooldown: cooldown - 1,
                }
            } else {
                AccountPhase::Idle
            };
        }
        AccountPhase::Ready { .. } | AccountPhase::VerifyPending { .. } => {}
    }
    let _ = log_handle;
}

/// Send VERIFY_PASSWORD_REQUEST to the shell public channel (name length
/// word + packed name, secret length word + packed secret). The shell
/// relays to account-service's read-only verify and answers
/// [status=0][valid].
fn send_verify_request(bridge: &mut SessionBridge, channel: rt::Handle) {
    let pair = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return,
    };
    let mut request = rt::RawMessage::empty(shell_tag::VERIFY_PASSWORD_REQUEST);
    request.words[0] = bridge.auth_user_len as u64;
    let name_words = match rt::pack_bytes(
        &bridge.auth_user[..bridge.auth_user_len],
        &mut request.words[1..],
    ) {
        Ok(words) => words as usize,
        Err(_) => {
            let _ = rt::handle_close(pair.first);
            let _ = rt::handle_close(pair.second);
            return;
        }
    };
    let mut cursor = 1 + name_words;
    request.words[cursor] = bridge.auth_pass_len as u64;
    cursor += 1;
    let pass_words = match rt::pack_bytes(
        &bridge.auth_pass[..bridge.auth_pass_len],
        &mut request.words[cursor..],
    ) {
        Ok(words) => words as usize,
        Err(_) => {
            let _ = rt::handle_close(pair.first);
            let _ = rt::handle_close(pair.second);
            return;
        }
    };
    cursor += pass_words;
    request.word_count = cursor as u32;
    request.handle_count = 1;
    request.handles[0] = pair.second;
    request.handle_rights[0] = rt::rights::SEND;
    match rt::channel_send(channel, &request) {
        Ok(()) => {
            let _ = rt::handle_close(pair.second);
            bridge.account = AccountPhase::VerifyPending {
                channel,
                reply: pair.first,
            };
            bridge.verify_in_flight = true;
        }
        Err(_) => {
            let _ = rt::handle_close(pair.first);
            let _ = rt::handle_close(pair.second);
            bridge.account = AccountPhase::Idle;
        }
    }
}

/// Deliver the host's verdict into the transport (SUCCESS / FAILURE /
/// bounded lockout) and clean the parked credentials.
fn deliver_verdict(
    bridge: &mut SessionBridge,
    transport: &mut SshTransport,
    valid: bool,
    log_handle: rt::Handle,
) {
    let user_text = core::str::from_utf8(&bridge.auth_user[..bridge.auth_user_len]).unwrap_or("?");
    let outcome = transport.auth_verdict(valid);
    match &outcome {
        Ok(()) => {
            let _ = rt::write_logf(
                "network",
                format_args!(
                    "sshd auth {} user={user_text}",
                    if valid { "ok" } else { "fail" }
                ),
            );
        }
        Err(_) => {
            // Lockout disconnect (or NotReady); the DISCONNECT is queued.
            let _ = rt::write_logf(
                "network",
                format_args!("sshd auth lockout user={user_text}"),
            );
        }
    }
    bridge.zero_credentials();
    let _ = log_handle;
}

// ----------------------------------------------------------------------
// Shell bridge: remote channel <-> shell operator session
// ----------------------------------------------------------------------

/// Advance the shell operator-session bridge one step per pump pass. The
/// remote connection becomes a plain client session on the shell public
/// channel: SESSION_INPUT_LINE in, SESSION_OUTPUT_TEXT out — the same wire
/// contract the desktop login uses.
fn pump_shell(
    bridge: &mut SessionBridge,
    transport: &mut SshTransport,
    bootstrap: rt::Handle,
    log_handle: rt::Handle,
) {
    if !transport.channel_ready() || transport.channel_closed() {
        return;
    }
    if bridge.shell_handle.is_none() {
        bridge.shell_handle = rt::lookup_service(bootstrap, rt::ServiceId::Shell).ok();
        if bridge.shell_handle.is_none() {
            return;
        }
    }
    let Some(shell) = bridge.shell_handle else {
        return;
    };
    match bridge.bridge {
        BridgePhase::Closed => {
            let pair = match rt::channel_create() {
                Ok(pair) => pair,
                Err(_) => return,
            };
            let mut request = rt::RawMessage::empty(shell_tag::SESSION_OPEN_REQUEST);
            request.handle_count = 1;
            request.handles[0] = pair.second;
            request.handle_rights[0] = rt::rights::SEND;
            if rt::channel_send(shell, &request).is_err() {
                let _ = rt::handle_close(pair.first);
                let _ = rt::handle_close(pair.second);
                return;
            }
            let _ = rt::handle_close(pair.second);
            bridge.bridge = BridgePhase::Opening {
                carrier: pair.first,
                budget: OPEN_WAIT_PASSES,
            };
        }
        BridgePhase::Opening { carrier, budget } => {
            let mut reply = rt::RawMessage::empty(0);
            match rt::channel_receive_nonblocking(carrier, &mut reply) {
                Ok(()) => {
                    let _ = rt::handle_close(carrier);
                    if reply.tag == shell_tag::SESSION_OPEN_REPLY
                        && reply.word_count >= 1
                        && reply.words[0] == 0
                        && reply.handle_count >= 1
                    {
                        let endpoint = reply.handles[0];
                        bridge.bridge = BridgePhase::Open { endpoint };
                        let _ =
                            rt::write_logf("network", format_args!("sshd shell session opened"));
                        // Lines typed before the bridge opened are submitted
                        // now (the first one; later early lines were dropped
                        // with a log).
                        if bridge.pending_len > 0 {
                            let pending_len = bridge.pending_len;
                            let mut pending = [0u8; LINE_CAP];
                            pending[..pending_len]
                                .copy_from_slice(&bridge.pending_line[..pending_len]);
                            bridge.pending_len = 0;
                            send_input_line(endpoint, &pending[..pending_len]);
                        }
                    } else {
                        // Busy/unavailable: stay Closed and retry next pass.
                        bridge.bridge = BridgePhase::Closed;
                    }
                }
                Err(rt::Error::QueueEmpty) => {
                    if budget > 0 {
                        bridge.bridge = BridgePhase::Opening {
                            carrier,
                            budget: budget - 1,
                        };
                    } else {
                        let _ = rt::handle_close(carrier);
                        bridge.bridge = BridgePhase::Closed;
                        let _ = rt::write_logf(
                            "network",
                            format_args!("sshd: shell session open timed out"),
                        );
                    }
                }
                Err(_) => {
                    let _ = rt::handle_close(carrier);
                    bridge.bridge = BridgePhase::Closed;
                }
            }
        }
        BridgePhase::Open { endpoint } => {
            // Initial prompt (the shell client path only prompts after
            // executing a line).
            if !bridge.prompt_sent {
                bridge.prompt_sent = true;
                let _ = transport.send_channel_data(SHELL_PROMPT.as_bytes());
            }
            // Drain shell output into the channel.
            loop {
                let mut message = rt::RawMessage::empty(0);
                match rt::channel_receive_nonblocking(endpoint, &mut message) {
                    Ok(()) => {
                        if message.tag == shell_tag::SESSION_CLOSE {
                            // Shell released the session (logout command).
                            let _ = rt::handle_close(endpoint);
                            bridge.bridge = BridgePhase::Closed;
                            bridge.prompt_sent = false;
                            let _ = transport.send_channel_eof();
                            let _ = transport.send_channel_close();
                            break;
                        }
                        if message.tag != shell_tag::SESSION_OUTPUT_TEXT || message.word_count < 1 {
                            continue;
                        }
                        let len =
                            (message.words[0] as usize).min((message.word_count as usize - 1) * 8);
                        let mut text = [0u8; 128];
                        let len = len.min(text.len());
                        if rt::unpack_bytes(
                            &message.words[1..message.word_count as usize],
                            len,
                            &mut text,
                        )
                        .is_err()
                        {
                            continue;
                        }
                        let accepted = transport.send_channel_data(&text[..len]).unwrap_or(0);
                        if accepted < len {
                            let _ = rt::write_logf(
                                "network",
                                format_args!(
                                    "sshd: channel output truncated ({}/{})",
                                    accepted, len
                                ),
                            );
                        }
                    }
                    Err(rt::Error::QueueEmpty) => break,
                    Err(_) => {
                        // Endpoint gone: tear the channel down honestly.
                        let _ = rt::handle_close(endpoint);
                        bridge.bridge = BridgePhase::Closed;
                        bridge.prompt_sent = false;
                        let _ = transport.send_channel_close();
                        let _ = rt::write_logf("network", format_args!("sshd shell session lost"));
                        break;
                    }
                }
            }
        }
    }
    let _ = log_handle;
}

/// Line discipline for interactive input: local echo, backspace erase,
/// CR/LF submits the assembled line to the shell operator session. The
/// remote connection is the console; the shell does no echoing of its own
/// over the client path.
fn feed_line_input(bridge: &mut SessionBridge, transport: &mut SshTransport, bytes: &[u8]) {
    for &byte in bytes {
        match byte {
            b'\r' | b'\n' => {
                let _ = transport.send_channel_data(b"\r\n");
                let line_len = bridge.line_len;
                bridge.line_len = 0;
                if line_len == 0 {
                    // Empty enter: re-prompt locally.
                    let _ = transport.send_channel_data(SHELL_PROMPT.as_bytes());
                    continue;
                }
                let mut line = [0u8; LINE_CAP];
                line[..line_len].copy_from_slice(&bridge.line[..line_len]);
                match bridge.bridge {
                    BridgePhase::Open { endpoint } => {
                        send_input_line(endpoint, &line[..line_len]);
                    }
                    _ => {
                        // The shell session is not open yet: park the first
                        // early line for the open transition.
                        if bridge.pending_len == 0 {
                            bridge.pending_line[..line_len].copy_from_slice(&line[..line_len]);
                            bridge.pending_len = line_len;
                        } else {
                            let _ = rt::write_logf(
                                "network",
                                format_args!("sshd: early line dropped (bridge not open)"),
                            );
                        }
                    }
                }
            }
            0x08 | 0x7f => {
                if bridge.line_len > 0 {
                    bridge.line_len -= 1;
                    let _ = transport.send_channel_data(b"\x08 \x08");
                }
            }
            0x03 => {
                // ^C: abandon the assembled line.
                bridge.line_len = 0;
                let _ = transport.send_channel_data(b"^C\r\n");
            }
            b if b.is_ascii_graphic() || b == b' ' => {
                if bridge.line_len < LINE_CAP {
                    bridge.line[bridge.line_len] = byte;
                    bridge.line_len += 1;
                    let _ = transport.send_channel_data(&[byte]);
                }
            }
            _ => {}
        }
    }
}

/// Submit one assembled line to the shell operator session.
fn send_input_line(endpoint: rt::Handle, line: &[u8]) {
    let mut message = rt::RawMessage::empty(shell_tag::SESSION_INPUT_LINE);
    message.words[0] = line.len() as u64;
    let packed = match rt::pack_bytes(line, &mut message.words[1..]) {
        Ok(words) => words,
        Err(_) => return,
    };
    message.word_count = 1 + packed as u32;
    let _ = rt::channel_send(endpoint, &message);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock duplex half: bytes arriving from the peer (served by recv) and
    /// bytes emitted toward the peer (appended by send).
    struct MockStream<'a> {
        from_peer: &'a mut std::collections::VecDeque<u8>,
        to_peer: &'a mut Vec<u8>,
    }

    impl StreamLike for MockStream<'_> {
        fn recv(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
            let take = buf.len().min(self.from_peer.len());
            for (index, byte) in self.from_peer.range(..take).copied().enumerate() {
                buf[index] = byte;
            }
            self.from_peer.drain(..take);
            Ok(take)
        }
        fn send(&mut self, data: &[u8]) -> Result<usize, ()> {
            self.to_peer.extend_from_slice(data);
            Ok(data.len())
        }
    }

    fn test_seeds() -> Seeds {
        derive_host_seeds(
            b"network-service-sshd-hostkey",
            &[0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
            42,
        )
    }

    #[test]
    fn host_seeds_deterministic_and_tick_sensitive() {
        let mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
        let a = derive_host_seeds(b"network-service-sshd-hostkey", &mac, 7);
        let b = derive_host_seeds(b"network-service-sshd-hostkey", &mac, 7);
        assert_eq!(a, b);
        let c = derive_host_seeds(b"network-service-sshd-hostkey", &mac, 8);
        assert_ne!(a.0, c.0);
        assert_ne!(a.1, c.1);
    }

    /// Mirrors the real guest pump: two SshTransport instances (one server,
    /// one client helper) exchange bytes through queues standing in for the
    /// TCP sockets, driven by the same recv->feed / drain-output loop that
    /// `drive_session` runs per main-loop iteration.
    #[test]
    fn kex_completes_over_mock_duplex_then_service_request_is_accepted() {
        let mut wire_server_to_client: Vec<u8> = Vec::new();
        let mut wire_client_to_server: std::collections::VecDeque<u8> =
            std::collections::VecDeque::new();

        let (_, client_kex_seed, client_cookie) = test_seeds();
        let mut client = SshTransport::client(client_kex_seed, client_cookie);

        let (host_seed, kex_seed, cookie) = test_seeds();
        let mut server_transport = SshTransport::server(host_seed, kex_seed, cookie);

        // Client-side driving: same shape as drive_session.
        macro_rules! client_pump {
            () => {
                loop {
                    let pending = client.pending_output();
                    if pending.is_empty() {
                        break;
                    }
                    wire_client_to_server.extend(pending.iter().copied());
                    client.consume_output(pending.len());
                }
            };
        }
        client_pump!();

        // Run the server pump until Established.
        let mut established = false;
        let mut test_bridge = SessionBridge::new();
        for _ in 0..64 {
            let mut server_stream = MockStream {
                from_peer: &mut wire_client_to_server,
                to_peer: &mut wire_server_to_client,
            };
            let note = drive_session(&mut server_stream, &mut server_transport, &mut test_bridge);
            drop(server_stream);
            // Client RX: feed what the server emitted, drain client output.
            {
                let rx: Vec<u8> = wire_server_to_client.drain(..).collect();
                let _ = client.feed(&rx);
            }
            client_pump!();
            if matches!(note, SessionOutcome::Established) {
                established = true;
                break;
            }
        }
        assert!(established, "transport never reached Established");
        assert_eq!(server_transport.state(), State::Established);

        // Post-establishment: emulate an operator client asking for
        // ssh-userauth. The auth layer answers SERVICE_ACCEPT and parks the
        // transport in ServiceAccepted (see shared/ssh auth tests for the
        // full matrix; the in-guest verifier flow is exercised live).
        let mut request = Vec::new();
        request.push(5u8); // SSH_MSG_SERVICE_REQUEST
        request.extend_from_slice(&12u32.to_be_bytes());
        request.extend_from_slice(b"ssh-userauth");
        client.send_payload(&request).expect("client payload send");
        client_pump!();

        let mut accepted = false;
        for _ in 0..64 {
            let mut server_stream = MockStream {
                from_peer: &mut wire_client_to_server,
                to_peer: &mut wire_server_to_client,
            };
            let _note = drive_session(&mut server_stream, &mut server_transport, &mut test_bridge);
            drop(server_stream);
            {
                let rx: Vec<u8> = wire_server_to_client.drain(..).collect();
                if client.feed(&rx).is_ok() {
                    // SERVICE_ACCEPT is an unknown established-state type on
                    // the client helper, so it surfaces as a passthrough.
                    accepted = true;
                    break;
                }
            }
            if matches!(client.state(), State::Closed) {
                break;
            }
        }
        assert!(accepted, "client never observed the SERVICE_ACCEPT reply");
        assert_eq!(server_transport.state(), State::Established);
        assert_eq!(
            server_transport.auth_phase(),
            serviceos_ssh::auth::AuthPhase::ServiceAccepted
        );
    }

    // ------------------------------------------------------------------
    // Bridge line discipline (host-testable pure state)
    // ------------------------------------------------------------------

    struct LineBench {
        transport: SshTransport,
        bridge: SessionBridge,
    }

    fn line_bench() -> LineBench {
        let (host_seed, kex_seed, cookie) = test_seeds();
        let mut transport = SshTransport::server(host_seed, kex_seed, cookie);
        transport.init_server(host_seed, kex_seed, cookie);
        // Channel-open + shell requests are handled by the library even
        // before auth for the purposes of state, but send_channel_data is
        // gated on chan.open; drive a minimal real handshake here.
        let mut client = {
            let (_, client_kex_seed, client_cookie) = test_seeds();
            SshTransport::client(client_kex_seed, client_cookie)
        };
        // Handshake to established.
        for _ in 0..64 {
            let server_out = transport.pending_output().to_vec();
            transport.consume_output(server_out.len());
            let _ = client.feed(&server_out);
            let client_out = client.pending_output().to_vec();
            client.consume_output(client_out.len());
            let _ = transport.feed(&client_out);
            if transport.state() == State::Established && client.state() == State::Established {
                break;
            }
        }
        assert_eq!(transport.state(), State::Established);
        // Open the session channel so the echo path is active.
        let mut open = Vec::new();
        open.push(90u8);
        open.extend_from_slice(&7u32.to_be_bytes());
        open.extend_from_slice(b"session");
        open.extend_from_slice(&7u32.to_be_bytes());
        open.extend_from_slice(&65536u32.to_be_bytes());
        open.extend_from_slice(&32768u32.to_be_bytes());
        client.send_payload(&open).unwrap();
        {
            let out = client.pending_output().to_vec();
            client.consume_output(out.len());
            transport.feed(&out).unwrap();
        }
        let reply = transport.pending_output().to_vec();
        transport.consume_output(reply.len());
        let _ = client.feed(&reply);
        LineBench {
            transport,
            bridge: SessionBridge::new(),
        }
    }

    #[test]
    fn line_editor_assembles_echoes_and_submits() {
        let mut bench = line_bench();
        // Typing assembles the line and echoes each byte (queued output
        // grows; the echo content rides the encrypted channel).
        let queued_before = bench.transport.pending_output().len();
        feed_line_input(&mut bench.bridge, &mut bench.transport, b"he");
        assert_eq!(bench.bridge.line_len, 2);
        assert_eq!(&bench.bridge.line[..2], b"he");
        assert!(bench.transport.pending_output().len() > queued_before);
        // Backspace erases with echo.
        feed_line_input(&mut bench.bridge, &mut bench.transport, &[0x7f]);
        assert_eq!(bench.bridge.line_len, 1);
        // New bytes land after the erased one.
        feed_line_input(
            &mut bench.bridge,
            &mut bench.transport,
            b"i
",
        );
        assert_eq!(bench.bridge.line_len, 0, "CR must clear the line");
        // The assembled line is intact in the bridge's submit path: bridge
        // Closed means send_input_line was a no-op (no shell session yet),
        // which is the honest behavior pre-open.
    }

    #[test]
    fn line_editor_handles_control_bytes_and_cap() {
        let mut bench = line_bench();
        // Ctrl bytes other than BS/CR/LF/^C are ignored.
        feed_line_input(&mut bench.bridge, &mut bench.transport, &[0x01, 0x1b]);
        assert_eq!(bench.bridge.line_len, 0);
        // ^C abandons the line.
        feed_line_input(&mut bench.bridge, &mut bench.transport, b"ab");
        feed_line_input(&mut bench.bridge, &mut bench.transport, &[0x03]);
        assert_eq!(bench.bridge.line_len, 0);
        // Line cap: overlong input stops appending without wedging.
        let long = [b'a'; INPUT_CHUNK_CAP];
        feed_line_input(&mut bench.bridge, &mut bench.transport, &long);
        assert_eq!(bench.bridge.line_len, LINE_CAP);
        feed_line_input(
            &mut bench.bridge,
            &mut bench.transport,
            b"
",
        );
        assert_eq!(bench.bridge.line_len, 0);
    }
}

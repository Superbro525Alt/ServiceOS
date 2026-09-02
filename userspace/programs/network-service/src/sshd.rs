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
use serviceos_ssh::transport::{Feed, SshTransport, State};
use serviceos_userspace_runtime as rt;
use smoltcp::iface::SocketHandle;
use smoltcp::iface::SocketSet;
use smoltcp::socket::tcp;

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
        }
    }

    /// Begin a session for an adopted transport slot: reset the transport
    /// in place and queue the server banner. Surplus adoption is refused by
    /// the caller while `session_active`.
    fn begin_session(&mut self, transport_index: usize, seeds: Seeds) {
        self.session_active = true;
        self.transport_index = transport_index;
        self.established_logged = false;
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
        drive_session(&mut stream, &mut state.transport)
    };
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
/// stream (partial writes retry on later loop iterations). This is the exact
/// driving model the host tests exercise with a mock duplex.
fn drive_session(stream: &mut dyn StreamLike, transport: &mut SshTransport) -> SessionOutcome {
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
            Ok(Feed::Packet { .. }) => {
                // Library already queued SSH_MSG_UNIMPLEMENTED; no session
                // surface exists this wave, so close honestly.
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
    loop {
        let pending = transport.pending_output();
        if pending.is_empty() {
            break;
        }
        let copied = match stream.send(pending) {
            Ok(written) => written,
            Err(()) => return SessionOutcome::Ended("network: sshd session closed (send failed)"),
        };
        if copied == 0 {
            break;
        }
        transport.consume_output(copied);
    }
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
    fn kex_completes_over_mock_duplex_then_service_request_disconnects() {
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
        for _ in 0..64 {
            let mut server_stream = MockStream {
                from_peer: &mut wire_client_to_server,
                to_peer: &mut wire_server_to_client,
            };
            let note = drive_session(&mut server_stream, &mut server_transport);
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
        // ssh-userauth. The library's honest v0 policy disconnects.
        let mut request = Vec::new();
        request.push(5u8); // SSH_MSG_SERVICE_REQUEST
        request.extend_from_slice(&11u32.to_be_bytes());
        request.extend_from_slice(b"ssh-userauth");
        client.send_payload(&request).expect("client payload send");
        client_pump!();

        let mut disconnected = false;
        for _ in 0..64 {
            let mut server_stream = MockStream {
                from_peer: &mut wire_client_to_server,
                to_peer: &mut wire_server_to_client,
            };
            let _note = drive_session(&mut server_stream, &mut server_transport);
            drop(server_stream);
            {
                let rx: Vec<u8> = wire_server_to_client.drain(..).collect();
                let _ = client.feed(&rx);
            }
            if matches!(client.state(), State::Closed) {
                disconnected = true;
                break;
            }
        }
        assert!(disconnected, "server never disconnected on service request");
        assert_eq!(server_transport.state(), State::Closed);
    }
}

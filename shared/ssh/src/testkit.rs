//! Test-only harness shared by the auth and channel test modules: a fixed
//! host-key/seed pair and a converged in-process server+client transport.

use crate::transport::{SshTransport, State};

pub(crate) fn host_seed() -> [u8; 32] {
    let mut s = [0u8; 32];
    s[0] = 0x11;
    s[15] = 0xAB;
    s[31] = 0x5E;
    s
}

pub(crate) fn srv_seed() -> [u8; 32] {
    let mut s = [0u8; 32];
    s[0] = 0x22;
    s[31] = 0x01;
    s
}

pub(crate) fn cli_seed() -> [u8; 32] {
    let mut s = [0u8; 32];
    s[0] = 0x33;
    s[31] = 0x02;
    s
}

pub(crate) fn cookie_a() -> [u8; 16] {
    let mut c = [0u8; 16];
    c[0] = 0xA0;
    c
}

pub(crate) fn cookie_b() -> [u8; 16] {
    let mut c = [0u8; 16];
    c[0] = 0xB0;
    c
}

pub(crate) fn make_server() -> SshTransport {
    SshTransport::server(host_seed(), srv_seed(), cookie_a())
}

pub(crate) fn make_client() -> SshTransport {
    SshTransport::client(cli_seed(), cookie_b())
}

pub(crate) fn drain(from: &mut SshTransport) -> std::vec::Vec<u8> {
    let bytes = from.pending_output().to_vec();
    from.consume_output(bytes.len());
    bytes
}

/// Drive both ends until both are Established (or panic).
pub(crate) fn pump(s: &mut SshTransport, c: &mut SshTransport) {
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
pub(crate) fn established_pair() -> (SshTransport, SshTransport) {
    let mut s = make_server();
    let mut c = make_client();
    pump(&mut s, &mut c);
    (s, c)
}

/// Craft a USERAUTH_REQUEST password payload.
pub(crate) fn craft_password_request(user: &[u8], password: &[u8], out: &mut [u8]) -> usize {
    let mut w = crate::wire::Writer::new(out);
    w.u8(crate::auth::SSH_MSG_USERAUTH_REQUEST).unwrap();
    w.string(user).unwrap();
    w.string(b"ssh-connection").unwrap();
    w.string(b"password").unwrap();
    w.u8(0).unwrap();
    w.string(password).unwrap();
    w.into_written()
}

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

mod logging;
mod remote;
mod requests;
mod session;
mod state;

use rt::{ControlTag, RawMessage, ServiceId};
use serviceos_userspace_runtime as rt;

use crate::{
    logging::poll_lifecycle,
    remote::{bind_listener, pump_remote, selftest_loopback, RemoteBridge},
    requests::{handle_public_request, handle_session_message},
    session::release_session,
    state::{
        MAX_PUBLIC_REQUESTS_PER_TURN, MAX_SESSION_MESSAGES_PER_TURN, MAX_REMOTE_LINKS,
        MAX_SESSIONS, Session, REMOTE_LISTENER_PORT,
    },
};

#[cfg(not(test))]
rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf901;
    }
    if startup.tag != ControlTag::Startup as u32 {
        return 0xf902;
    }

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xf903,
    };
    if rt::register_service(bootstrap, ServiceId::Terminal, public.second).is_err() {
        return 0xf904;
    }
    let _ = rt::handle_close(public.second);

    let mut sessions = [Session::empty(); MAX_SESSIONS];
    let mut next_session_id = 1u32;
    // Remote (TCP) session bridges: listener plus per-connection protocol
    // state. Plaintext rsh-like framing; see remote.rs module docs.
    let listener = bind_listener(bootstrap);
    match listener {
        Some(_) => {
            let _ = rt::write_logf(
                "terminal",
                format_args!(
                    "remote listener bound port={} links={}",
                    REMOTE_LISTENER_PORT, MAX_REMOTE_LINKS
                ),
            );
        }
        None => {
            let _ = rt::write_logf(
                "terminal",
                format_args!("remote listener unavailable port={}", REMOTE_LISTENER_PORT),
            );
        }
    }
    // Loopback evidence run is deferred and gated (state.rs
    // REMOTE_LOOPBACK_SELFTEST): firing it during the boot burst races the
    // network service's own startup selftest, and cross-service loopback
    // connect is not yet drivable end-to-end.
    let mut remote_selftest_done = false;
    let mut remote_turns: u64 = 0;
    let mut bridges: [RemoteBridge; MAX_REMOTE_LINKS] =
        core::array::from_fn(|_| RemoteBridge::empty());

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xf905,
        }

        remote_turns = remote_turns.saturating_add(1);
        if !remote_selftest_done && remote_turns >= 500 {
            remote_selftest_done = true;
            if listener.is_some() && state::REMOTE_LOOPBACK_SELFTEST {
                selftest_loopback(bootstrap, REMOTE_LISTENER_PORT);
            }
        }

        if let Some(listener_handle) = listener {
            if pump_remote(
                bootstrap,
                listener_handle,
                &mut sessions,
                &mut bridges,
                &mut next_session_id,
            )
            .is_err()
            {
                return 0xf909;
            }
        }

        let mut public_budget = MAX_PUBLIC_REQUESTS_PER_TURN;
        loop {
            let mut request = RawMessage::empty(0);
            match rt::channel_receive_nonblocking(public.first, &mut request) {
                Ok(()) => {
                    if handle_public_request(
                        bootstrap,
                        &mut sessions,
                        &mut next_session_id,
                        &request,
                    )
                    .is_err()
                    {
                        return 0xf906;
                    }
                    public_budget = public_budget.saturating_sub(1);
                    if public_budget == 0 {
                        break;
                    }
                }
                Err(rt::Error::QueueEmpty) => break,
                Err(_) => return 0xf907,
            }
        }

        for session in &mut sessions {
            if !session.occupied {
                continue;
            }
            let mut session_budget = MAX_SESSION_MESSAGES_PER_TURN;
            loop {
                let mut message = RawMessage::empty(0);
                match rt::channel_receive_nonblocking(session.endpoint, &mut message) {
                    Ok(()) => {
                        if handle_session_message(bootstrap, session, &message).is_err() {
                            release_session(bootstrap, session);
                            break;
                        }
                        session_budget = session_budget.saturating_sub(1);
                        if session_budget == 0 {
                            break;
                        }
                    }
                    Err(rt::Error::QueueEmpty) => break,
                    // A detached session has no live client: transport errors
                    // from the departed pane must not kill retained state.
                    Err(_) => {
                        if session.attached {
                            release_session(bootstrap, session);
                        }
                        break;
                    }
                }
            }
        }

        if rt::yield_current().is_err() {
            return 0xf908;
        }
    }
}

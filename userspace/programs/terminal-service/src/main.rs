#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

mod logging;
mod requests;
mod session;
mod state;

use rt::{ControlTag, RawMessage, ServiceId};
use serviceos_userspace_runtime as rt;

use crate::{
    logging::poll_lifecycle,
    requests::{handle_public_request, handle_session_message},
    session::release_session,
    state::{MAX_PUBLIC_REQUESTS_PER_TURN, MAX_SESSION_MESSAGES_PER_TURN, MAX_SESSIONS, Session},
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

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xf905,
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
                    Err(_) => {
                        release_session(bootstrap, session);
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

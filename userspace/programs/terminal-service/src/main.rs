#![no_std]
#![no_main]

mod logging;
mod requests;
mod session;
mod state;

use serviceos_userspace_runtime as rt;
use rt::{ControlTag, RawMessage, ServiceId};

use crate::{
    logging::poll_lifecycle,
    requests::{handle_public_request, handle_session_message},
    session::release_session,
    state::{MAX_SESSIONS, Session},
};

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

        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                if handle_public_request(bootstrap, &mut sessions, &mut next_session_id, &request)
                    .is_err()
                {
                    return 0xf906;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xf907,
        }

        for session in &mut sessions {
            if !session.occupied {
                continue;
            }
            loop {
                let mut message = RawMessage::empty(0);
                match rt::channel_receive_nonblocking(session.endpoint, &mut message) {
                    Ok(()) => {
                        if handle_session_message(bootstrap, session, &message).is_err() {
                            release_session(bootstrap, session);
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

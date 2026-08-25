#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

mod format;
mod grid;
mod input;
mod lifecycle;
mod public;
mod session;
mod state;

use rt::{RawMessage, ServiceId};
use serviceos_userspace_runtime as rt;

use crate::input::handle_input_byte;
use crate::lifecycle::poll_lifecycle;
use crate::public::handle_public_message;
use crate::session::handle_session_message;
use crate::state::{BootProgress, MAX_SESSIONS, Session, detach_session};

#[cfg(not(test))]
rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf301;
    }

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xf302,
    };
    if rt::register_service(bootstrap, ServiceId::Console, public.second).is_err() {
        return 0xf303;
    }
    let _ = rt::handle_close(public.second);

    let mut sessions = [Session::empty(); MAX_SESSIONS];
    let mut boot_progress = BootProgress::new();
    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xf304,
        }

        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut message) {
            Ok(()) => {
                if handle_public_message(&mut sessions, &mut boot_progress, &message).is_err() {
                    return 0xf305;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xf306,
        }

        for session in &mut sessions {
            if !session.occupied {
                continue;
            }
            let mut session_message = RawMessage::empty(0);
            match rt::channel_receive_nonblocking(session.endpoint, &mut session_message) {
                Ok(()) => {
                    if handle_session_message(session, &session_message).is_err() {
                        return 0xf307;
                    }
                }
                Err(rt::Error::QueueEmpty) => {}
                // Client endpoint went away: retain the row (detached) so the
                // next SessionOpen adopts it and history survives handoff.
                Err(_) => detach_session(session),
            }
        }

        loop {
            match rt::debug_console_read_byte() {
                Ok(byte) => {
                    if handle_input_byte(&mut sessions, byte).is_err() {
                        return 0xf309;
                    }
                }
                Err(rt::Error::QueueEmpty) => break,
                Err(_) => return 0xf30a,
            }
        }

        if rt::yield_current().is_err() {
            return 0xf30b;
        }
    }
}

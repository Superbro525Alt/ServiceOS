use serviceos_userspace_runtime as rt;
use rt::{ControlTag, LifecycleEvent, RawMessage, ServiceId};

use crate::{
    consts::{MAX_ENVS, MAX_RUNS},
    protocol::{handle_public_request, handle_run_session_request, poll_run_exits},
    types::{EnvSlot, RunSlot},
    util::read_profile,
};

pub(crate) fn run() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xfc01;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 2 || startup.word_count < 5 {
        return 0xfc02;
    }

    let log_handle = startup.handles[0];
    let profile_handle = startup.handles[1];
    let profile = match read_profile(profile_handle) {
        Ok(profile) => profile,
        Err(_) => return 0xfc03,
    };

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xfc04,
    };
    if rt::register_service(bootstrap, ServiceId::Runtime, public.second).is_err() {
        return 0xfc05;
    }
    let _ = rt::handle_close(public.second);

    let storage_handle = match rt::lookup_service(bootstrap, ServiceId::Storage) {
        Ok(handle) => handle,
        Err(_) => return 0xfc06,
    };

    let mut envs = [EnvSlot::empty(); MAX_ENVS];
    let mut runs = [RunSlot::empty(); MAX_RUNS];

    loop {
        if poll_lifecycle(bootstrap).unwrap_or(false) {
            for run in &mut runs {
                if run.occupied {
                    crate::util::release_run_slot(run);
                }
            }
            let _ = rt::handle_close(storage_handle);
            return 0;
        }

        let mut had_work = false;
        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                had_work = true;
                if handle_public_request(
                    bootstrap,
                    storage_handle,
                    log_handle,
                    profile,
                    &mut envs,
                    &mut runs,
                    &request,
                )
                .is_err()
                {
                    return 0xfc07;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xfc08,
        }

        for run in &runs {
            if !run.occupied || run.session_handle == rt::INVALID_HANDLE {
                continue;
            }
            let mut session_request = RawMessage::empty(0);
            match rt::channel_receive_nonblocking(run.session_handle, &mut session_request) {
                Ok(()) => {
                    had_work = true;
                    if handle_run_session_request(
                        storage_handle,
                        log_handle,
                        &envs,
                        run,
                        &session_request,
                    )
                    .is_err()
                    {
                        return 0xfc09;
                    }
                }
                Err(rt::Error::QueueEmpty) => {}
                Err(_) => {}
            }
        }

        poll_run_exits(log_handle, &mut envs, &mut runs);

        if !had_work && rt::yield_current().is_err() {
            return 0xfc0a;
        }
    }
}

fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
    let mut lifecycle = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut lifecycle) {
        Ok(()) if lifecycle.tag == ControlTag::Lifecycle as u32 && lifecycle.word_count >= 1 => {
            Ok(lifecycle.words[0] == LifecycleEvent::Stopped as u32 as u64)
        }
        Ok(()) => Ok(false),
        Err(rt::Error::QueueEmpty) => Ok(false),
        Err(error) => Err(error),
    }
}

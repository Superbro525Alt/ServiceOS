#![no_std]
#![no_main]

use rt::{ControlTag, LifecycleEvent, LogDomain, LogEvent, LogSeverity, RawMessage, ServiceId};
use serviceos_userspace_runtime as rt;

const MAX_MESSAGE_BYTES: usize = 128;

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xfb01;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.word_count < 5 || startup.words[2] < 1 {
        return 0xfb02;
    }

    let service_grants = startup.words[2] as usize;
    let resource_grants = startup.words[3] as usize;
    if service_grants < 1 || resource_grants < 1 {
        return 0xfb03;
    }

    let log_handle = startup.handles[0];
    let resource_handle = startup.handles[service_grants];
    let resource_len = startup.words[4] as usize;
    let mut message = [0u8; MAX_MESSAGE_BYTES];
    let requested = resource_len.min(message.len());
    let loaded = match rt::storage_read_all(resource_handle, &mut message, requested) {
        Ok(loaded) => loaded,
        Err(_) => return 0xfb04,
    };
    let _ = rt::storage_blob_close(resource_handle);

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xfb05,
    };
    if rt::register_service(bootstrap, ServiceId::Announce, public.second).is_err() {
        return 0xfb06;
    }
    let _ = rt::handle_close(public.second);

    let _ = rt::send_log_record(
        log_handle,
        ServiceId::Announce,
        LogSeverity::Info,
        LogDomain::Service,
        LogEvent::ResourceOpened,
        loaded as u64,
        0,
    );

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xfb07,
        }
        if rt::yield_current().is_err() {
            return 0xfb08;
        }
    }
}

fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
    let mut message = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut message) {
        Ok(()) if message.tag == ControlTag::Lifecycle as u32 && message.word_count > 0 => {
            Ok(matches!(
                lifecycle_event_from_word(message.words[0]),
                LifecycleEvent::Restarting | LifecycleEvent::Stopped
            ))
        }
        Ok(()) => Ok(false),
        Err(rt::Error::QueueEmpty) => Ok(false),
        Err(error) => Err(error),
    }
}

fn lifecycle_event_from_word(value: u64) -> LifecycleEvent {
    match value as u32 {
        x if x == LifecycleEvent::Starting as u32 => LifecycleEvent::Starting,
        x if x == LifecycleEvent::Ready as u32 => LifecycleEvent::Ready,
        x if x == LifecycleEvent::Failed as u32 => LifecycleEvent::Failed,
        x if x == LifecycleEvent::Stopped as u32 => LifecycleEvent::Stopped,
        _ => LifecycleEvent::Restarting,
    }
}

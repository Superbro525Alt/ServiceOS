#![no_std]
#![no_main]

use serviceos_abi::{ControlTag, LifecycleEvent, RawMessage, ServiceId};
use serviceos_userspace_runtime as rt;

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf101;
    }

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xf102,
    };

    let mut register = RawMessage::empty(ControlTag::Register as u32);
    register.word_count = 1;
    register.words[0] = ServiceId::Log as u32 as u64;
    register.handle_count = 1;
    register.handles[0] = public.second;
    if rt::channel_send(bootstrap, &register).is_err() {
        return 0xf103;
    }
    let _ = rt::handle_close(public.second);

    loop {
        let mut message = RawMessage::empty(0);
        if rt::channel_receive_blocking(public.first, &mut message).is_err() {
            return 0xf104;
        }
        if message.tag == ControlTag::Lifecycle as u32 && message.word_count >= 2 {
            let _ = rt::write_logf(
                "log-service",
                format_args!(
                    "{} {} detail={}",
                    service_name(message.words[0]),
                    event_name(message.words[1]),
                    if message.word_count > 2 { message.words[2] } else { 0 }
                ),
            );
        }
    }
}

fn service_name(value: u64) -> &'static str {
    match value as u32 {
        x if x == ServiceId::RootManager as u32 => "root-manager",
        x if x == ServiceId::Log as u32 => "log-service",
        x if x == ServiceId::Echo as u32 => "echo-service",
        x if x == ServiceId::Probe as u32 => "probe-service",
        _ => "unknown-service",
    }
}

fn event_name(value: u64) -> &'static str {
    match value as u32 {
        x if x == LifecycleEvent::Starting as u32 => "starting",
        x if x == LifecycleEvent::Ready as u32 => "ready",
        x if x == LifecycleEvent::Failed as u32 => "failed",
        x if x == LifecycleEvent::Restarting as u32 => "restarting",
        _ => "stopped",
    }
}

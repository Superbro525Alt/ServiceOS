#![no_std]
#![no_main]

use serviceos_abi::{ControlTag, LifecycleEvent, RawMessage, ServiceId};
use serviceos_userspace_runtime as rt;

const PROBE_WORD: u64 = 0xfeed_cafe;

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf301;
    }
    let attempt = startup.words[1];
    let log_handle = if startup.handle_count > 0 {
        startup.handles[0]
    } else {
        rt::INVALID_HANDLE
    };

    let _ = emit_lifecycle(log_handle, LifecycleEvent::Starting, attempt);
    if attempt == 1 {
        let _ = emit_lifecycle(log_handle, LifecycleEvent::Failed, attempt);
        return 0x51;
    }

    let mut lookup = RawMessage::empty(ControlTag::LookupRequest as u32);
    lookup.word_count = 1;
    lookup.words[0] = ServiceId::Echo as u32 as u64;
    if rt::channel_send(bootstrap, &lookup).is_err() {
        return 0xf302;
    }

    let mut lookup_reply = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut lookup_reply).is_err() {
        return 0xf303;
    }
    if lookup_reply.tag != ControlTag::LookupReply as u32 || lookup_reply.handle_count == 0 {
        return 0xf304;
    }
    let echo_handle = lookup_reply.handles[0];

    let reply = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xf305,
    };
    let mut request = RawMessage::empty(ControlTag::EchoRequest as u32);
    request.word_count = 1;
    request.words[0] = PROBE_WORD;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    if rt::channel_send(echo_handle, &request).is_err() {
        return 0xf306;
    }
    let _ = rt::handle_close(reply.second);
    let _ = rt::handle_close(echo_handle);

    let mut response = RawMessage::empty(0);
    if rt::channel_receive_blocking(reply.first, &mut response).is_err() {
        return 0xf307;
    }
    let _ = rt::handle_close(reply.first);
    if response.tag != ControlTag::EchoReply as u32 || response.words[0] != PROBE_WORD {
        return 0xf308;
    }

    let _ = emit_lifecycle(log_handle, LifecycleEvent::Ready, attempt);
    0
}

fn emit_lifecycle(log_handle: rt::Handle, event: LifecycleEvent, detail: u64) -> rt::Result<()> {
    if log_handle == rt::INVALID_HANDLE {
        return Ok(());
    }
    let mut message = RawMessage::empty(ControlTag::Lifecycle as u32);
    message.word_count = 3;
    message.words[0] = ServiceId::Probe as u32 as u64;
    message.words[1] = event as u32 as u64;
    message.words[2] = detail;
    rt::channel_send(log_handle, &message)
}

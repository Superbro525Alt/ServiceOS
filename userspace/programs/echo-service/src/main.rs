#![no_std]
#![no_main]

use serviceos_abi::{ControlTag, RawMessage, ServiceId};
use serviceos_userspace_runtime as rt;

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf201;
    }
    let _log_handle = if startup.handle_count > 0 {
        startup.handles[0]
    } else {
        rt::INVALID_HANDLE
    };

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xf202,
    };

    let mut register = RawMessage::empty(ControlTag::Register as u32);
    register.word_count = 1;
    register.words[0] = ServiceId::Echo as u32 as u64;
    register.handle_count = 1;
    register.handles[0] = public.second;
    if rt::channel_send(bootstrap, &register).is_err() {
        return 0xf203;
    }
    let _ = rt::handle_close(public.second);

    loop {
        let mut request = RawMessage::empty(0);
        if rt::channel_receive_blocking(public.first, &mut request).is_err() {
            return 0xf204;
        }
        if request.tag != ControlTag::EchoRequest as u32 || request.handle_count == 0 {
            continue;
        }

        let reply_handle = request.handles[0];
        let mut response = RawMessage::empty(ControlTag::EchoReply as u32);
        response.word_count = 1;
        response.words[0] = request.words[0];
        let _ = rt::channel_send(reply_handle, &response);
        let _ = rt::handle_close(reply_handle);
    }
}

#![no_std]
#![no_main]

use core::fmt::Write;

use serviceos_userspace_runtime as rt;
use rt::{FixedLogBuffer, RawMessage};

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf801;
    }
    if startup.tag != rt::ControlTag::Startup as u32 || startup.handle_count < 1 {
        return 0xf802;
    }

    let session = startup.handles[0];
    let abi = match rt::abi_version() {
        Ok(version) => version,
        Err(_) => return 0xf803,
    };
    let ticks = match rt::monotonic_now() {
        Ok(now) => now,
        Err(_) => return 0xf804,
    };

    let _ = write_linef(session, format_args!("sysinfo-tool"));
    let _ = write_linef(session, format_args!("abi-version: {:#x}", abi));
    let _ = write_linef(session, format_args!("monotonic-ticks: {}", ticks));
    let _ = rt::handle_close(session);
    0
}

fn write_linef(session: rt::Handle, args: core::fmt::Arguments<'_>) -> rt::Result<()> {
    let mut buffer = FixedLogBuffer::<160>::new();
    let _ = buffer.write_fmt(args);
    let _ = buffer.write_str("\r\n");
    let text = core::str::from_utf8(buffer.as_bytes()).map_err(|_| rt::Error::InvalidArgument)?;
    rt::console_session_write(session, text)
}

#![no_std]
#![no_main]

use core::fmt::Write;

use serviceos_userspace_runtime as rt;
use rt::{FixedLogBuffer, RawMessage, RuntimeWorkloadKind};

const MAX_PATH_BYTES: usize = 64;

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xfd01;
    }
    if startup.tag != rt::ControlTag::Startup as u32 || startup.handle_count < 2 || startup.word_count < 2 {
        return 0xfd02;
    }

    let relay = startup.handles[0];
    let session = startup.handles[1];
    let workload = match startup.words[0] as u32 {
        x if x == RuntimeWorkloadKind::Inspect as u32 => RuntimeWorkloadKind::Inspect,
        x if x == RuntimeWorkloadKind::Env as u32 => RuntimeWorkloadKind::Env,
        x if x == RuntimeWorkloadKind::Mounts as u32 => RuntimeWorkloadKind::Mounts,
        x if x == RuntimeWorkloadKind::Cat as u32 => RuntimeWorkloadKind::Cat,
        _ => RuntimeWorkloadKind::Inspect,
    };
    let arg_len = startup.words[1] as usize;
    let mut arg = [0u8; MAX_PATH_BYTES];
    if arg_len > arg.len()
        || rt::unpack_bytes(&startup.words[2..startup.word_count as usize], arg_len, &mut arg)
            .is_err()
    {
        let _ = rt::handle_close(relay);
        let _ = rt::handle_close(session);
        return 0xfd03;
    }

    let result = match workload {
        RuntimeWorkloadKind::Inspect => write_inspect(relay, session),
        RuntimeWorkloadKind::Env => write_vars(relay, session),
        RuntimeWorkloadKind::Mounts => write_mounts(relay, session),
        RuntimeWorkloadKind::Cat => write_cat(relay, session, &arg[..arg_len]),
    };
    let _ = rt::handle_close(relay);
    let _ = rt::handle_close(session);
    if result.is_err() {
        return 0xfd04;
    }
    0
}

fn write_inspect(output: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let info = rt::runtime_session_info(session)?;
    write_linef(output, format_args!("runtime env {}", info.env_id))?;
    write_linef(output, format_args!("kind: {}", runtime_kind_name(info.kind)))?;
    write_linef(output, format_args!("state: {}", runtime_env_state_name(info.state)))?;
    write_linef(output, format_args!("caps: {:#x}", info.capabilities))?;
    write_linef(output, format_args!("mounts: {}", info.mount_count))?;
    write_linef(output, format_args!("vars: {}", info.var_count))
}

fn write_vars(output: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let mut index = 0usize;
    let mut key = [0u8; 32];
    let mut value = [0u8; 80];
    while let Some((key_len, value_len)) = rt::runtime_session_var(session, index, &mut key, &mut value)? {
        let key = core::str::from_utf8(&key[..key_len]).map_err(|_| rt::Error::InvalidArgument)?;
        let value =
            core::str::from_utf8(&value[..value_len]).map_err(|_| rt::Error::InvalidArgument)?;
        write_linef(output, format_args!("{}={}", key, value))?;
        index += 1;
    }
    Ok(())
}

fn write_mounts(output: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let mut index = 0usize;
    let mut guest = [0u8; 48];
    let mut source = [0u8; 96];
    while let Some((guest_len, source_len)) =
        rt::runtime_session_mount(session, index, &mut guest, &mut source)?
    {
        let guest =
            core::str::from_utf8(&guest[..guest_len]).map_err(|_| rt::Error::InvalidArgument)?;
        let source =
            core::str::from_utf8(&source[..source_len]).map_err(|_| rt::Error::InvalidArgument)?;
        write_linef(output, format_args!("{} -> {}", guest, source))?;
        index += 1;
    }
    Ok(())
}

fn write_cat(output: rt::Handle, session: rt::Handle, path: &[u8]) -> rt::Result<()> {
    let path = core::str::from_utf8(path).map_err(|_| rt::Error::InvalidArgument)?;
    let mut offset = 0usize;
    let mut buffer = [0u8; 80];
    let mut last_byte = None;
    loop {
        let read = rt::runtime_session_read_file(session, path, offset, &mut buffer)?;
        if read == 0 {
            break;
        }
        let text = core::str::from_utf8(&buffer[..read]).map_err(|_| rt::Error::InvalidArgument)?;
        rt::runtime_output_relay_write(output, text)?;
        last_byte = buffer[..read].last().copied();
        offset += read;
    }
    if offset == 0 {
        write_linef(output, format_args!("empty {}", path))?;
    } else if last_byte != Some(b'\n') {
        rt::runtime_output_relay_write(output, "\r\n")?;
    }
    Ok(())
}

fn write_linef(output: rt::Handle, args: core::fmt::Arguments<'_>) -> rt::Result<()> {
    let mut buffer = FixedLogBuffer::<160>::new();
    let _ = buffer.write_fmt(args);
    let _ = buffer.write_str("\r\n");
    let text = core::str::from_utf8(buffer.as_bytes()).map_err(|_| rt::Error::InvalidArgument)?;
    rt::runtime_output_relay_write(output, text)
}

fn runtime_kind_name(kind: rt::RuntimeKind) -> &'static str {
    match kind {
        rt::RuntimeKind::Posix => "posix",
        rt::RuntimeKind::Windows => "windows",
    }
}

fn runtime_env_state_name(state: rt::RuntimeEnvState) -> &'static str {
    match state {
        rt::RuntimeEnvState::Ready => "ready",
        rt::RuntimeEnvState::Busy => "busy",
        rt::RuntimeEnvState::Destroyed => "destroyed",
    }
}

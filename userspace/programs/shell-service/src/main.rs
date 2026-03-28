#![no_std]
#![no_main]

use serviceos_shell_service::{
    SHELL_PROMPT, SHELL_READY_TEXT, ShellOutput, execute_command, write_output_linef,
};
use serviceos_userspace_runtime as rt;
use rt::{LogEvent, LogSeverity, RawMessage, ServiceId};

const MAX_LINE_BYTES: usize = serviceos_shell_service::MAX_LINE_BYTES;

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf701;
    }
    if startup.tag != rt::ControlTag::Startup as u32 {
        return 0xf702;
    }

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xf703,
    };
    let console_handle = match rt::lookup_service(bootstrap, ServiceId::Console) {
        Ok(handle) => handle,
        Err(_) => return 0xf704,
    };
    let session_handle = match rt::console_session_open(console_handle) {
        Ok(handle) => handle,
        Err(_) => return 0xf705,
    };
    if rt::register_service(bootstrap, ServiceId::Shell, public.second).is_err() {
        return 0xf706;
    }
    let _ = rt::handle_close(public.second);
    let _ = rt::handle_close(console_handle);

    let output = ShellOutput::new(session_handle, rt::console_session_write);

    let _ = serviceos_shell_service::util::emit_shell_log(
        bootstrap,
        ServiceId::Shell,
        LogSeverity::Info,
        LogEvent::SessionOpened,
        1,
        0,
    );
    let _ = write_output_linef(output, format_args!("{SHELL_READY_TEXT}"));

    let mut line_buffer = [0u8; MAX_LINE_BYTES];
    loop {
        let _ = rt::console_session_write(session_handle, SHELL_PROMPT);
        let line_len = match rt::console_session_read_line(session_handle, &mut line_buffer) {
            Ok(len) => len,
            Err(_) => return 0xf707,
        };
        let Ok(raw_line) = core::str::from_utf8(&line_buffer[..line_len]) else {
            let _ = write_output_linef(output, format_args!("invalid utf-8 input"));
            continue;
        };
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if let Err(error) = execute_command(bootstrap, output, line) {
            let _ = write_output_linef(
                output,
                format_args!("command failed: {}", serviceos_shell_service::util::error_name(error)),
            );
        }
    }
}

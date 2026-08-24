#![no_std]
#![no_main]

use rt::{LogEvent, LogSeverity, RawMessage, ServiceId};
use serviceos_shell_service::{
    SHELL_PROMPT, SHELL_READY_TEXT, ShellOutput, execute_command, write_output_linef,
};
use serviceos_userspace_runtime as rt;

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
        let line_len = match read_prompt_line(session_handle, &mut line_buffer) {
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
                format_args!(
                    "command failed: {}",
                    serviceos_shell_service::util::error_name(error)
                ),
            );
        }

        // A line submitted (Enter) while `logs follow` was streaming is
        // stashed and executed here, after the follow has ended.
        let mut pending = [0u8; MAX_LINE_BYTES];
        let pending_len = serviceos_shell_service::take_pending_line(&mut pending);
        if pending_len > 0 {
            if let Ok(pending_line) = core::str::from_utf8(&pending[..pending_len]) {
                let trimmed = pending_line.trim();
                if !trimmed.is_empty() {
                    let _ = write_output_linef(output, format_args!("{SHELL_PROMPT}{trimmed}"));
                    if let Err(error) = execute_command(bootstrap, output, trimmed) {
                        let _ = write_output_linef(
                            output,
                            format_args!(
                                "command failed: {}",
                                serviceos_shell_service::util::error_name(error)
                            ),
                        );
                    }
                }
            }
        }
    }
}

/// Reads a prompt line, tolerating the transient failures that occur right
/// after `logs follow` releases an armed console read-line slot: the console
/// service keeps a single pending-reply slot, so a request issued before the
/// stale slot clears is rejected and must be retried once input arrives.
fn read_prompt_line(session_handle: rt::Handle, buffer: &mut [u8]) -> rt::Result<usize> {
    let mut attempts = 0usize;
    loop {
        match rt::console_session_read_line(session_handle, buffer) {
            Ok(len) => return Ok(len),
            Err(error) => {
                attempts += 1;
                if attempts >= 16 {
                    return Err(error);
                }
                let _ = rt::yield_current();
            }
        }
    }
}

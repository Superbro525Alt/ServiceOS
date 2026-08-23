#![no_std]

pub mod commands;
pub mod util;

use rt::{LogEvent, LogSeverity, ServiceId};
use serviceos_userspace_runtime as rt;

pub use util::{HELP_TEXT, ShellOutput, shell_output_write, write_output_linef};

pub const MAX_LINE_BYTES: usize = 128;
pub const SHELL_PROMPT: &str = "serviceos> ";
pub const SHELL_READY_TEXT: &str = "serviceos shell ready; type 'help' for commands";

pub fn execute_command_with_source(
    bootstrap: rt::Handle,
    source_service: ServiceId,
    output: ShellOutput,
    line: &str,
) -> rt::Result<()> {
    let _ = util::emit_shell_log(
        bootstrap,
        source_service,
        LogSeverity::Debug,
        LogEvent::ShellCommand,
        line.len() as u64,
        0,
    );
    commands::execute_command(bootstrap, output, line)
}

pub fn execute_command(bootstrap: rt::Handle, output: ShellOutput, line: &str) -> rt::Result<()> {
    execute_command_with_source(bootstrap, ServiceId::Shell, output, line)
}

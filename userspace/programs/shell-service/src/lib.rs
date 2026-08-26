#![cfg_attr(not(test), no_std)]

pub mod commands;
pub mod history_search;
pub mod jobs;
pub mod pipeline;
pub mod sessions;
pub mod util;

use rt::{LogEvent, LogSeverity, ServiceId};
use serviceos_userspace_runtime as rt;

pub use util::{HELP_TEXT, ShellOutput, shell_output_write, take_pending_line, write_output_linef};

pub const MAX_LINE_BYTES: usize = 128;
pub const SHELL_PROMPT: &str = "serviceos> ";
pub const SHELL_READY_TEXT: &str = "serviceos shell ready; type 'help' for commands";

/// Wire tags for operator sessions served over the shell public channel.
/// Published here in-crate (not shared/abi) following account-service's
/// precedent so no ABI edit is needed; values live in the shell's reserved
/// experimental range.
pub mod shell_tag {
    /// handles[0] = reply channel; mints an operator session.
    pub const SESSION_OPEN_REQUEST: u32 = 0x240;
    /// words[0] = status (0 ok), words[1] = session id; handles[0] = endpoint.
    pub const SESSION_OPEN_REPLY: u32 = 0x241;
    /// words[0] = byte length, words[1..] = packed line; executes a command
    /// as that session and streams `SESSION_OUTPUT_TEXT` replies back.
    pub const SESSION_INPUT_LINE: u32 = 0x242;
    /// Server -> client output, same shape as `SESSION_INPUT_LINE`.
    pub const SESSION_OUTPUT_TEXT: u32 = 0x243;
    /// Client is done; releases the operator session row.
    pub const SESSION_CLOSE: u32 = 0x244;
}

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
    commands::execute_line(bootstrap, output, line)
}

pub fn execute_command(bootstrap: rt::Handle, output: ShellOutput, line: &str) -> rt::Result<()> {
    execute_command_with_source(bootstrap, ServiceId::Shell, output, line)
}

/// Event-loop hook: executes at most one queued background job per call so
/// the prompt stays responsive between ticks.
pub fn poll_background_jobs(bootstrap: rt::Handle) {
    commands::poll_jobs(bootstrap);
}

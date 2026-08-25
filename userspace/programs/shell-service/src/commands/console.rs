//! `console` operator surface: render the console-service retained VT text
//! grid (kernel/service console messages) through the shell.
//!
//! The bridge rides existing contracts only: it opens a console session
//! (`ConsoleTag::SessionOpenRequest`), opts into the server's record stream
//! with the alternate-screen marker inside a plain `SessionWriteText`, then
//! consumes the VT-encoded payloads the service pushes back. Any ANSI text
//! renderer can draw the same byte stream, so graphical terminal panes can
//! display kernel console output wherever the executing identity holds a
//! console-service grant.

use rt::{ConsoleTag, RawMessage, ServiceId};
use serviceos_userspace_runtime as rt;

use crate::util::{ShellOutput, tables, write_output_linef};
use tables::{FOLLOW_IDLE_TIMEOUT_TICKS, FOLLOW_MAX_RECORDS};

/// Grid geometry mirrored from console-service's retained surface (80x24);
/// only used to size the snapshot collection budget.
const GRID_COLS: usize = 80;
const GRID_ROWS: usize = 24;
/// Payload budget of one full 80x24 grid frame plus escape prefix and CRLFs,
/// rounded up so collection can stop without parsing escapes.
const FRAME_BUDGET_BYTES: usize = 8 + GRID_ROWS * (GRID_COLS + 2) + 16;
/// Empty polls tolerated while waiting for the first frame push.
const FRAME_IDLE_POLLS: u32 = 64;

pub(crate) fn cmd_console(
    bootstrap: rt::Handle,
    output: ShellOutput,
    sub: Option<&str>,
) -> rt::Result<()> {
    match sub {
        Some("grid") => cmd_console_grid(bootstrap, output),
        Some("follow") => cmd_console_follow(bootstrap, output),
        _ => write_output_linef(output, format_args!("usage: console <grid|follow>")),
    }
}

fn open_subscribed_console_session(bootstrap: rt::Handle) -> rt::Result<rt::Handle> {
    let console_handle = rt::lookup_service(bootstrap, ServiceId::Console)?;
    let session = match rt::console_session_open(console_handle) {
        Ok(session) => session,
        Err(error) => {
            let _ = rt::handle_close(console_handle);
            return Err(error);
        }
    };
    // Opt in to the retained grid stream via the alternate-screen marker; the
    // console service answers with a full frame push.
    if rt::console_session_write(session, ALT_SUBSCRIBE).is_err() {
        let _ = rt::handle_close(console_handle);
        let _ = rt::handle_close(session);
        return Err(rt::Error::InvalidArgument);
    }
    Ok(session)
}

fn close_console_session(console_session: rt::Handle) {
    let _ = rt::console_session_write(console_session, ALT_UNSUBSCRIBE);
    let _ = rt::handle_close(console_session);
}

/// Decode one pushed `SessionWriteText`-shaped message into `out`.
fn decode_push(message: &RawMessage, out: &mut heapless_vec::ByteVec) -> bool {
    if message.word_count < 1 || message.tag != ConsoleTag::SessionWriteText as u32 {
        return false;
    }
    let len = message.words[0] as usize;
    let mut scratch = [0u8; MAX_PUSH_PAYLOAD];
    let payload_words = message.word_count as usize;
    if rt::unpack_bytes(&message.words[1..payload_words], len, &mut scratch).is_err() {
        return false;
    }
    out.extend(&scratch[..len])
}

const MAX_PUSH_PAYLOAD: usize = (rt::IPC_MAX_WORDS - 1) * 8;
const ALT_SUBSCRIBE: &str = "\x1b[?1049h";
const ALT_UNSUBSCRIBE: &str = "\x1b[?1049l";

pub(crate) fn cmd_console_grid(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let session = match open_subscribed_console_session(bootstrap) {
        Ok(session) => session,
        Err(_) => {
            return write_output_linef(
                output,
                format_args!(
                    "console grid unavailable: no console-service grant for this identity \
                     (console surfaces require lookup=console-service)"
                ),
            );
        }
    };

    let mut frame = heapless_vec::ByteVec::new();
    let mut empty_polls = 0u32;
    while frame.len() < FRAME_BUDGET_BYTES {
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(session, &mut message) {
            Ok(()) => {
                decode_push(&message, &mut frame);
                empty_polls = 0;
            }
            Err(rt::Error::QueueEmpty) => {
                empty_polls += 1;
                if empty_polls >= FRAME_IDLE_POLLS {
                    break;
                }
                let _ = rt::yield_current();
            }
            Err(_) => break,
        }
    }
    close_console_session(session);

    if frame.is_empty() {
        return write_output_linef(
            output,
            format_args!("console grid empty (no records received)"),
        );
    }
    let text = core::str::from_utf8(frame.as_slice()).unwrap_or("");
    shell_grid_write(output, text)?;
    write_output_linef(output, format_args!("(console grid snapshot end)"))
}

pub(crate) fn cmd_console_follow(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let session = match open_subscribed_console_session(bootstrap) {
        Ok(session) => session,
        Err(_) => {
            return write_output_linef(
                output,
                format_args!(
                    "console follow unavailable: no console-service grant for this identity \
                     (console surfaces require lookup=console-service)"
                ),
            );
        }
    };
    write_output_linef(
        output,
        format_args!(
            "(streaming console records; ends on idle timeout after {FOLLOW_IDLE_TIMEOUT_TICKS} ticks or {FOLLOW_MAX_RECORDS} lines)"
        ),
    )?;

    let mut records_seen = 0usize;
    let mut last_activity = rt::monotonic_now().unwrap_or(0);
    let stop_reason;
    loop {
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(session, &mut message) {
            Ok(()) => {
                let mut line = heapless_vec::ByteVec::new();
                if decode_push(&message, &mut line) {
                    records_seen += 1;
                    last_activity = rt::monotonic_now().unwrap_or(last_activity);
                    if let Ok(text) = core::str::from_utf8(line.as_slice()) {
                        shell_grid_write(output, text)?;
                        let _ = rt::yield_current();
                    }
                }
                if records_seen >= FOLLOW_MAX_RECORDS {
                    stop_reason = "(console follow stopped at record cap)";
                    break;
                }
            }
            Err(rt::Error::QueueEmpty) => {
                let now = rt::monotonic_now().unwrap_or(last_activity);
                if now.saturating_sub(last_activity) >= FOLLOW_IDLE_TIMEOUT_TICKS {
                    stop_reason = "(console follow idle timeout)";
                    break;
                }
                let _ = rt::yield_current();
            }
            Err(_) => {
                stop_reason = "(console follow ended: session closed)";
                break;
            }
        }
    }
    close_console_session(session);
    write_output_linef(output, format_args!("{stop_reason}"))
}

/// Chunked writer that mirrors `shell_output_write` limits so long grid frames
/// fit inside single-message inline payloads.
fn shell_grid_write(output: ShellOutput, text: &str) -> rt::Result<()> {
    const CHUNK: usize = MAX_PUSH_PAYLOAD;
    let bytes = text.as_bytes();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let end = (offset + CHUNK).min(bytes.len());
        let chunk =
            core::str::from_utf8(&bytes[offset..end]).map_err(|_| rt::Error::InvalidArgument)?;
        crate::shell_output_write(output, chunk)?;
        offset = end;
    }
    Ok(())
}

/// Tiny fixed-capacity byte vector (no_std friendly, host-testable).
mod heapless_vec {
    pub struct ByteVec {
        storage: [u8; 4096],
        len: usize,
    }

    impl ByteVec {
        pub const fn new() -> Self {
            Self {
                storage: [0; 4096],
                len: 0,
            }
        }

        pub fn extend(&mut self, bytes: &[u8]) -> bool {
            let free = self.storage.len() - self.len;
            let count = bytes.len().min(free);
            self.storage[self.len..self.len + count].copy_from_slice(&bytes[..count]);
            self.len += count;
            count == bytes.len()
        }

        pub fn as_slice(&self) -> &[u8] {
            &self.storage[..self.len]
        }

        pub fn len(&self) -> usize {
            self.len
        }

        pub const fn is_empty(&self) -> bool {
            self.len == 0
        }
    }
}

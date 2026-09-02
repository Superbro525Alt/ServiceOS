#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

use rt::{ConsoleTag, Handle, LogEvent, LogSeverity, RawMessage, ServiceId};
use serviceos_shell_service::{
    SHELL_PROMPT, SHELL_READY_TEXT, ShellOutput, execute_command, jobs, sessions, shell_tag,
    write_output_linef,
};
use serviceos_userspace_runtime as rt;

const MAX_LINE_BYTES: usize = serviceos_shell_service::MAX_LINE_BYTES;
/// Inline payload ceiling: length word plus packed bytes must fit one message.
const MAX_INLINE_BYTES: usize = (rt::IPC_MAX_WORDS - 1) * 8;
const CLIENT_SLOTS: usize = sessions::MAX_OPERATOR_SESSIONS;

#[cfg(not(test))]
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

    // Public operator-session channel: `public.first` stays with us for
    // draining; the registered duplicate hands clients the peer end.
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

    // The serial console session is itself one keyed operator session among
    // several; client sessions join over the public channel at runtime.
    let console_key = sessions::SessionKey::Console(session_handle);
    let console_id = match sessions::ensure_session(console_key, 0) {
        Ok(id) => id,
        Err(_) => return 0xf708,
    };

    let output = ShellOutput::new(session_handle, rt::console_session_write);

    let _ = serviceos_shell_service::util::emit_shell_log(
        bootstrap,
        ServiceId::Shell,
        LogSeverity::Info,
        LogEvent::SessionOpened,
        console_id as u64,
        0,
    );
    let _ = write_output_linef(output, format_args!("{SHELL_READY_TEXT}"));

    let mut armed_read: Option<Handle> = None;
    let mut read_failures = 0usize;
    loop {
        drain_public_channel(bootstrap, public.first);
        drain_client_sessions(bootstrap);
        serviceos_shell_service::poll_background_jobs(bootstrap);

        // Serial readline completion?
        if let Some(rx_end) = armed_read {
            let mut reply = RawMessage::empty(0);
            match rt::channel_receive_nonblocking(rx_end, &mut reply) {
                Ok(()) => {
                    let _ = rt::handle_close(rx_end);
                    armed_read = None;
                    read_failures = 0;
                    let line_len = reply.words[0] as usize;
                    let mut buffer = [0u8; MAX_LINE_BYTES];
                    let len = line_len.min(buffer.len());
                    if rt::unpack_bytes(
                        &reply.words[1..reply.word_count as usize],
                        len,
                        &mut buffer,
                    )
                    .is_ok()
                        && len > 0
                    {
                        run_serial_line(bootstrap, output, console_key, &buffer[..len]);
                    }
                }
                Err(rt::Error::QueueEmpty) => {}
                Err(_) => {
                    read_failures += 1;
                    if read_failures >= 512 {
                        return 0xf707;
                    }
                }
            }
        }

        // (Re)arm the console read-line request when idle. Transient rejects
        // right after `logs follow` releases the pending-reply slot are
        // tolerated with a bounded retry.
        if armed_read.is_none() {
            match arm_console_read_line(session_handle) {
                Ok(rx_end) => {
                    armed_read = Some(rx_end);
                    read_failures = 0;
                }
                Err(_) => {
                    read_failures += 1;
                    if read_failures >= 16 {
                        return 0xf707;
                    }
                }
            }
        }

        if rt::yield_current().is_err() {
            return 0xf70b;
        }
    }
}

fn run_serial_line(
    bootstrap: Handle,
    output: ShellOutput,
    console_key: sessions::SessionKey,
    bytes: &[u8],
) {
    let Ok(raw_line) = core::str::from_utf8(bytes) else {
        let _ = write_output_linef(output, format_args!("invalid utf-8 input"));
        return;
    };
    let line = raw_line.trim();
    if line.is_empty() {
        return;
    }
    execute_as_session(bootstrap, console_key, output, line);

    // A line submitted while `logs follow` streamed was stashed; run it now.
    let mut pending = [0u8; MAX_LINE_BYTES];
    let pending_len = serviceos_shell_service::take_pending_line(&mut pending);
    if pending_len == 0 {
        return;
    }
    if let Ok(pending_line) = core::str::from_utf8(&pending[..pending_len]) {
        let trimmed = pending_line.trim();
        if !trimmed.is_empty() {
            let _ = write_output_linef(output, format_args!("{SHELL_PROMPT}{trimmed}"));
            execute_as_session(bootstrap, console_key, output, trimmed);
        }
    }
}

/// Shared execution path: binds the active operator session (history +
/// ownership context), records history, runs the command, reports errors.
fn execute_as_session(
    bootstrap: Handle,
    key: sessions::SessionKey,
    output: ShellOutput,
    line: &str,
) {
    // Completed background jobs announce their exit status here so the
    // operator sees them with the next prompt interaction.
    let _ = jobs::flush_done_reports(output);
    sessions::set_active_key(key.encode());
    sessions::record_history(key, line);
    if let Err(error) = execute_command(bootstrap, output, line) {
        let _ = write_output_linef(
            output,
            format_args!(
                "command failed: {}",
                serviceos_shell_service::util::error_name(error)
            ),
        );
    }
}

fn arm_console_read_line(session_handle: Handle) -> rt::Result<Handle> {
    let pair = rt::channel_create()?;
    let mut request = RawMessage::empty(ConsoleTag::SessionReadLineRequest as u32);
    request.handle_count = 1;
    request.handles[0] = pair.second;
    match rt::channel_send(session_handle, &request) {
        Ok(()) => Ok(pair.first),
        Err(error) => {
            let _ = rt::handle_close(pair.first);
            let _ = rt::handle_close(pair.second);
            Err(error)
        }
    }
}

/// Serve open requests on the shell public channel: each accepted client gets
/// a dedicated endpoint whose server side becomes the operator-session key.
/// The additive VERIFY_PASSWORD request is served here too (sshd bridge).
fn drain_public_channel(bootstrap: Handle, public_server: Handle) {
    loop {
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public_server, &mut message) {
            Ok(()) => {
                if message.tag == shell_tag::SESSION_OPEN_REQUEST && message.handle_count >= 1 {
                    handle_session_open_request(&message);
                } else if message.tag == shell_tag::VERIFY_PASSWORD_REQUEST
                    && message.handle_count >= 1
                {
                    handle_verify_password_request(bootstrap, &message);
                }
            }
            Err(rt::Error::QueueEmpty) => return,
            Err(_) => return,
        }
    }
}

/// VERIFY_PASSWORD_REQUEST relay: decode name + secret, run account-service's
/// read-only verify through the shell's account channel, and answer
/// [status=0][valid]. Denies (valid=0) on every transport failure — the
/// caller fail-closes.
fn handle_verify_password_request(bootstrap: Handle, message: &RawMessage) {
    let reply_handle = message.handles[0];
    let mut reply = RawMessage::empty(shell_tag::VERIFY_PASSWORD_REPLY);
    reply.word_count = 2;
    reply.words[0] = 0;
    reply.words[1] = 0;

    let mut name = [0u8; 64];
    let mut secret = [0u8; 64];
    if let Some((name_len, secret_len)) = decode_verify_request(message, &mut name, &mut secret) {
        let name_str = core::str::from_utf8(&name[..name_len]).unwrap_or("");
        let secret_str = core::str::from_utf8(&secret[..secret_len]).unwrap_or("");
        match serviceos_shell_service::commands::verify_password(bootstrap, name_str, secret_str) {
            Ok(valid) => reply.words[1] = if valid { 1 } else { 0 },
            Err(_) => reply.words[1] = 0,
        }
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
}

/// Decode [name_len][name][secret_len][secret] into the caller's scratch
/// buffers; returns the two field lengths on success.
fn decode_verify_request(
    message: &RawMessage,
    name: &mut [u8; 64],
    secret: &mut [u8; 64],
) -> Option<(usize, usize)> {
    // words[0] carries the name length (network packs name at words[1..]).
    let mut cursor = 0usize;
    let name_len = (*message.words.get(cursor)?) as usize;
    cursor += 1;
    if name_len > name.len() {
        return None;
    }
    let name_words = name_len.div_ceil(8);
    rt::unpack_bytes(
        message.words.get(cursor..cursor + name_words)?,
        name_len,
        &mut name[..name_len],
    )
    .ok()?;
    cursor += name_words;
    let secret_len = (*message.words.get(cursor)?) as usize;
    cursor += 1;
    if secret_len > secret.len() {
        return None;
    }
    let secret_words = secret_len.div_ceil(8);
    rt::unpack_bytes(
        message.words.get(cursor..cursor + secret_words)?,
        secret_len,
        &mut secret[..secret_len],
    )
    .ok()?;
    Some((name_len, secret_len))
}

fn handle_session_open_request(message: &RawMessage) {
    let reply_handle = message.handles[0];
    let mut reply = RawMessage::empty(shell_tag::SESSION_OPEN_REPLY);
    reply.word_count = 2;
    reply.words[0] = 1; // busy/unavailable

    let pair = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => {
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
            return;
        }
    };
    // Server side: pair.first is both the receive endpoint and the output
    // target for this operator session; the client receives pair.second.
    let key = sessions::SessionKey::Client(pair.first);
    match sessions::ensure_session(key, pair.first) {
        Ok(id) => {
            reply.words[0] = 0;
            reply.words[1] = id as u64;
            reply.handle_count = 1;
            reply.handles[0] = pair.second;
            reply.handle_rights[0] = rt::rights::SEND | rt::rights::RECEIVE | rt::rights::DUPLICATE;
        }
        Err(_) => {
            let _ = rt::handle_close(pair.first);
        }
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    // Mirror console-service ownership: the sender's reference to a
    // transferred handle must be closed after the reply goes out.
    let _ = rt::handle_close(pair.second);
}

/// Poll every connected client session for submitted lines / closes.
fn drain_client_sessions(bootstrap: Handle) {
    let mut snapshot = [(0u64, 0u32); CLIENT_SLOTS];
    let mut count = 0usize;
    sessions::for_each(|session| {
        if count < CLIENT_SLOTS {
            snapshot[count] = (session.key.encode(), session.peer);
            count += 1;
        }
    });
    for &(key_word, peer) in &snapshot[..count] {
        if peer == rt::INVALID_HANDLE {
            continue;
        }
        let Some(key) = sessions::SessionKey::decode(key_word) else {
            continue;
        };
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(peer, &mut message) {
            Ok(()) => handle_client_message(bootstrap, key, peer, &message),
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => release_client_session(key),
        }
    }
}

fn handle_client_message(
    bootstrap: Handle,
    key: sessions::SessionKey,
    peer: Handle,
    message: &RawMessage,
) {
    if message.tag == shell_tag::SESSION_CLOSE {
        release_client_session(key);
        return;
    }
    if message.tag != shell_tag::SESSION_INPUT_LINE || message.word_count < 1 {
        return;
    }
    let len = message.words[0] as usize;
    let mut buffer = [0u8; MAX_INLINE_BYTES];
    let payload_words = message.word_count as usize;
    if rt::unpack_bytes(
        &message.words[1..payload_words],
        len.min(buffer.len()),
        &mut buffer,
    )
    .is_err()
    {
        return;
    }
    let Ok(text) = core::str::from_utf8(&buffer[..len.min(buffer.len())]) else {
        let _ = send_client_output(peer, "invalid utf-8 input\r\n");
        return;
    };
    let line = text.trim();
    if line.is_empty() {
        return;
    }
    let output = ShellOutput::new(peer, send_client_output);
    execute_as_session(bootstrap, key, output, line);
    let _ = send_client_output(peer, SHELL_PROMPT);
}

fn release_client_session(key: sessions::SessionKey) {
    if let Some(peer) = sessions_peer(key) {
        let _ = rt::handle_close(peer);
    }
    sessions::drop_session(key);
}

fn sessions_peer(key: sessions::SessionKey) -> Option<Handle> {
    let mut found = None;
    sessions::for_each(|session| {
        if session.key == key {
            found = Some(session.peer);
        }
    });
    found.filter(|peer| *peer != rt::INVALID_HANDLE)
}

/// Client-bound output writer: same wire shape as the console text contract
/// (length word + packed bytes), tagged as shell `SESSION_OUTPUT_TEXT`.
fn send_client_output(peer: Handle, text: &str) -> rt::Result<()> {
    let bytes = text.as_bytes();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let end = (offset + MAX_INLINE_BYTES).min(bytes.len());
        let mut message = RawMessage::empty(shell_tag::SESSION_OUTPUT_TEXT);
        message.words[0] = (end - offset) as u64;
        message.word_count = 1 + rt::pack_bytes(&bytes[offset..end], &mut message.words[1..])?;
        rt::channel_send(peer, &message)?;
        offset = end;
    }
    Ok(())
}

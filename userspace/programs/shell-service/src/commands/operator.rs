//! Operator-facing commands for the multi-session shell: session listing,
//! per-session history, and the login/whoami/logout ownership surface.

use serviceos_userspace_runtime as rt;

use crate::commands::account;
use crate::jobs;
use crate::pipeline::{self, INPUT_LINE_BYTES};
use crate::sessions::{self, HISTORY_LINE_BYTES, MAX_HISTORY_ENTRIES, Owner, SessionKey};
use crate::util::{ShellOutput, shell_output_write, write_output_linef};

pub(crate) fn cmd_sessions(output: ShellOutput) -> rt::Result<()> {
    let mut listed = 0usize;
    {
        let output = output;
        sessions::for_each(|session| {
            listed += 1;
            let mut name_buffer = [0u8; 48];
            let owner = match session.owner {
                Some(owner) => {
                    let len = owner.name_len.min(name_buffer.len());
                    name_buffer[..len].copy_from_slice(&owner.name[..len]);
                    core::str::from_utf8(&name_buffer[..len]).unwrap_or("-")
                }
                None => "-",
            };
            let _ = write_output_linef(
                output,
                format_args!(
                    "id={} kind={} source={:#x} history={} owner={}",
                    session.id,
                    session.key.kind_name(),
                    session.key.encode(),
                    session.history.len(),
                    owner,
                ),
            );
        });
    }
    if listed == 0 {
        write_output_linef(output, format_args!("no operator sessions"))
    } else {
        Ok(())
    }
}

pub(crate) fn cmd_history(output: ShellOutput, count: Option<usize>) -> rt::Result<()> {
    let Some(key) = sessions::active_key() else {
        return write_output_linef(output, format_args!("no active operator session"));
    };
    let total = sessions::history_len(key);
    if total == 0 {
        return write_output_linef(
            output,
            format_args!("history empty for session kind={}", key.kind_name()),
        );
    }
    let shown = count.unwrap_or(MAX_HISTORY_ENTRIES).min(total);
    let first = total - shown;
    write_output_linef(
        output,
        format_args!(
            "history (kind={}, {} of {} entries, oldest first):",
            key.kind_name(),
            shown,
            total
        ),
    )?;
    for order in first..total {
        let mut buffer = [0u8; HISTORY_LINE_BYTES];
        let Some(len) = sessions::history_entry(key, order, &mut buffer) else {
            break;
        };
        let text = core::str::from_utf8(&buffer[..len]).unwrap_or("<binary>");
        write_output_linef(output, format_args!("{:>4}  {}", order + 1, text))?;
    }
    Ok(())
}

pub(crate) fn cmd_whoami(output: ShellOutput) -> rt::Result<()> {
    let Some(key) = sessions::active_key() else {
        return write_output_linef(output, format_args!("no active operator session"));
    };
    match sessions::owner_of(key) {
        Some(owner) => write_output_linef(
            output,
            format_args!(
                "account={} id={} capabilities={:#x} binding={}/{}",
                owner.name(),
                owner.account_id,
                owner.capabilities,
                key.kind_name(),
                source_session_id(key),
            ),
        ),
        None => write_output_linef(
            output,
            format_args!(
                "unowned (kind={} session; account activation is manual)",
                key.kind_name()
            ),
        ),
    }
}

pub(crate) fn cmd_login(
    bootstrap: rt::Handle,
    output: ShellOutput,
    name: Option<&str>,
    secret: Option<&str>,
) -> rt::Result<()> {
    let (Some(name), Some(secret)) = (name, secret) else {
        return write_output_linef(output, format_args!("usage: login <name> <secret>"));
    };
    let Some(key) = sessions::active_key() else {
        return write_output_linef(output, format_args!("no active operator session"));
    };
    let session_id = source_session_id(key);
    match account::login(bootstrap, name, secret, session_id) {
        Ok((account_id, capabilities)) => {
            let Some(owner) = Owner::none_named(name, account_id, capabilities) else {
                return write_output_linef(output, format_args!("login failed: name too long"));
            };
            if !sessions::bind_owner(key, owner) {
                return write_output_linef(output, format_args!("login failed: session vanished"));
            }
            write_output_linef(
                output,
                format_args!(
                    "session bound to account={name} id={account_id} capabilities={capabilities:#x}"
                ),
            )
        }
        Err(flow) => write_output_linef(output, format_args!("{}", flow.message())),
    }
}

pub(crate) fn cmd_logout(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let Some(key) = sessions::active_key() else {
        return write_output_linef(output, format_args!("no active operator session"));
    };
    let session_id = source_session_id(key);
    // Local ownership clears regardless; a transport failure still leaves the
    // shell usable and the account claim expires with the session id.
    let flow_result = account::logout(bootstrap, session_id);
    let had_owner = sessions::unbind_owner(key);
    if !had_owner {
        return write_output_linef(output, format_args!("session already unowned"));
    }
    if let Err(flow) = flow_result {
        write_output_linef(
            output,
            format_args!("session unbound (service note: {})", flow.message()),
        )?;
    }
    write_output_linef(output, format_args!("session unbound"))
}

/// The key's source handle doubles as the account-service claim session id so
/// logout and switch target the same binding that login created.
pub(crate) fn source_session_id(key: SessionKey) -> u32 {
    match key {
        SessionKey::Console(value) | SessionKey::Client(value) => value,
    }
}

pub(crate) fn cmd_jobs(output: ShellOutput) -> rt::Result<()> {
    let mut listed = 0usize;
    jobs::job_for_each(|row| {
        listed += 1;
        let mut cmd = [0u8; jobs::JOB_CMD_BYTES];
        let text = row.cmd_text(&mut cmd);
        let _ = write_output_linef(
            output,
            format_args!(
                "[{}] {} pending={}B {}",
                row.id,
                row.state.name(),
                row.output_len(),
                text
            ),
        );
    });
    if listed == 0 {
        write_output_linef(output, format_args!("no background jobs"))
    } else {
        Ok(())
    }
}

/// fg semantics: stream whatever output the job has retained into this
/// session, report status, and free completed rows.
pub(crate) fn cmd_fg(output: ShellOutput, job_id: u32) -> rt::Result<()> {
    let Some(state) = jobs::job_state(job_id) else {
        return write_output_linef(output, format_args!("fg: no such job {job_id}"));
    };
    let mut truncated = false;
    let mut buffer = [0u8; jobs::JOB_OUTPUT_BYTES];
    loop {
        let Some((len, was_truncated)) = jobs::job_take_output(job_id, &mut buffer) else {
            break;
        };
        if was_truncated {
            truncated = true;
        }
        if len == 0 {
            break;
        }
        let chunk = core::str::from_utf8(&buffer[..len]).unwrap_or("");
        shell_output_write(output, chunk)?;
        if !chunk.ends_with('\n') {
            shell_output_write(output, "\r\n")?;
        }
    }
    if truncated {
        write_output_linef(
            output,
            format_args!("[{job_id}] retained output was truncated"),
        )?;
    }
    match state {
        jobs::JobState::Running => write_output_linef(
            output,
            format_args!("[{job_id}] still running; more output may accrue"),
        ),
        jobs::JobState::Done => {
            let ok = jobs::job_exit_ok(job_id).unwrap_or(false);
            let mut err = [0u8; jobs::JOB_ERROR_BYTES];
            let err_len = jobs::job_error_copy(job_id, &mut err).unwrap_or(0);
            let error_text = core::str::from_utf8(&err[..err_len]).unwrap_or("");
            let _ = jobs::job_mark_reported(job_id);
            let _ = jobs::job_remove(job_id);
            if ok {
                write_output_linef(
                    output,
                    format_args!("[{job_id}] foreground complete (exit ok)"),
                )
            } else {
                write_output_linef(
                    output,
                    format_args!("[{job_id}] foreground complete (exit failed: {error_text})"),
                )
            }
        }
    }
}

pub(crate) fn cmd_filter(output: ShellOutput, pattern: &str) -> rt::Result<()> {
    if !pipeline::input_active() {
        return write_output_linef(
            output,
            format_args!("filter: no piped input (use: cmdA | filter <text>)"),
        );
    }
    for order in 0..pipeline::input_count() {
        let mut line = [0u8; INPUT_LINE_BYTES];
        let Some(len) = pipeline::input_line(order, &mut line) else {
            break;
        };
        let Ok(text) = core::str::from_utf8(&line[..len]) else {
            continue;
        };
        if !text.contains(pattern) {
            continue;
        }
        write_output_linef(output, format_args!("{text}"))?;
    }
    Ok(())
}

pub(crate) fn cmd_count(output: ShellOutput) -> rt::Result<()> {
    if !pipeline::input_active() {
        return write_output_linef(
            output,
            format_args!("count: no piped input (use: cmdA | count)"),
        );
    }
    write_output_linef(output, format_args!("{}", pipeline::input_count()))
}

/// Argument-less cat echoes piped input lines; with input absent it is a
/// usage error like before.
pub(crate) fn cmd_cat_input(output: ShellOutput) -> rt::Result<()> {
    if !pipeline::input_active() {
        return write_output_linef(output, format_args!("usage: cat <path>"));
    }
    for order in 0..pipeline::input_count() {
        let mut line = [0u8; INPUT_LINE_BYTES];
        let Some(len) = pipeline::input_line(order, &mut line) else {
            break;
        };
        let Ok(text) = core::str::from_utf8(&line[..len]) else {
            continue;
        };
        write_output_linef(output, format_args!("{text}"))?;
    }
    Ok(())
}

//! Operator-facing commands for the multi-session shell: session listing,
//! per-session history, and the login/whoami/logout ownership surface.

use serviceos_userspace_runtime as rt;

use crate::commands::account;
use crate::sessions::{self, HISTORY_LINE_BYTES, MAX_HISTORY_ENTRIES, Owner, SessionKey};
use crate::util::{ShellOutput, write_output_linef};

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

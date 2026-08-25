//! Identity-switching surface: the `su` command re-authenticates an operator
//! session as a different account, and a pure verdict mapping keeps the
//! shell's local binding in step with account-service contracts.
//!
//! Semantics: switching drops the previous claim first (service-side logout
//! on this session id, then the local owner) and performs a full credential
//! login for the target identity. The service-side `switch_user` primitive —
//! which moves an existing claim across session ids without credentials —
//! stays service-facing; operators always authenticate, so `su` never
//! impersonates without a secret.

use crate::commands::account::{self, AccountFlow};
use crate::sessions::{self};
use crate::util::{ShellOutput, write_output_linef};
use serviceos_userspace_runtime as rt;

/// What the operator gets after an `su` attempt, derived from whether the
/// session previously carried an owner plus the login contract result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SwitchOutcome {
    /// Credentials accepted; local owner re-bound to the new identity.
    Bound {
        had_previous: bool,
        account_id: u32,
        capabilities: u64,
    },
    /// Credentials refused (or contracts unreachable); any previous binding
    /// is already dropped, so the session is unowned either way.
    Rejected { had_previous: bool, flow: AccountFlow },
}

/// Pure state-machine mapping: previous-owner presence + login result ->
/// operator outcome. Host-tested; the command layer only formats.
pub fn map_switch(
    had_previous: bool,
    login_result: Result<(u32, u64), AccountFlow>,
) -> SwitchOutcome {
    match login_result {
        Ok((account_id, capabilities)) => SwitchOutcome::Bound {
            had_previous,
            account_id,
            capabilities,
        },
        Err(flow) => SwitchOutcome::Rejected { had_previous, flow },
    }
}

pub(crate) fn cmd_su(
    bootstrap: rt::Handle,
    output: ShellOutput,
    name: Option<&str>,
    secret: Option<&str>,
) -> rt::Result<()> {
    let (Some(name), Some(secret)) = (name, secret) else {
        return write_output_linef(output, format_args!("usage: su <name> <secret>"));
    };
    let Some(key) = sessions::active_key() else {
        return write_output_linef(output, format_args!("no active operator session"));
    };

    // Snapshot the outgoing owner for the operator message before dropping
    // it, then clear both sides of the binding: service-side claim first,
    // local owner second (mirrors logout ordering).
    let previous = sessions::owner_of(key);
    let session_id = super::operator::source_session_id(key);
    if previous.is_some() {
        let _ = account::logout(bootstrap, session_id);
        sessions::unbind_owner(key);
    }

    let outcome = map_switch(previous.is_some(), account::login(bootstrap, name, secret, session_id));
    match outcome {
        SwitchOutcome::Bound {
            had_previous,
            account_id,
            capabilities,
        } => {
            let Some(owner) = sessions::Owner::none_named(name, account_id, capabilities) else {
                return write_output_linef(output, format_args!("switch failed: name too long"));
            };
            if !sessions::bind_owner(key, owner) {
                return write_output_linef(output, format_args!("switch failed: session vanished"));
            }
            if had_previous {
                let mut buffer = [0u8; sessions::MAX_OWNER_NAME];
                let shown = previous.map(|owner| {
                    let len = owner.name_len.min(buffer.len());
                    buffer[..len].copy_from_slice(&owner.name[..len]);
                    core::str::from_utf8(&buffer[..len]).unwrap_or("-")
                });
                write_output_linef(
                    output,
                    format_args!(
                        "switched identity: {} -> {name} id={account_id} capabilities={capabilities:#x} binding={}",
                        shown.unwrap_or("-"),
                        key.kind_name(),
                    ),
                )
            } else {
                write_output_linef(
                    output,
                    format_args!(
                        "session bound to account={name} id={account_id} capabilities={capabilities:#x}"
                    ),
                )
            }
        }
        SwitchOutcome::Rejected { had_previous, flow } => {
            let note = if had_previous {
                "previous identity dropped; session now unowned"
            } else {
                "session stays unowned"
            };
            write_output_linef(
                output,
                format_args!("switch failed: {} ({note})", flow.message()),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bound(had_previous: bool, account_id: u32, capabilities: u64) -> SwitchOutcome {
        SwitchOutcome::Bound {
            had_previous,
            account_id,
            capabilities,
        }
    }

    #[test]
    fn accepted_logins_map_to_bound_outcomes_regardless_of_history() {
        assert_eq!(
            map_switch(false, Ok((7, 0x3))),
            bound(false, 7, 0x3)
        );
        assert_eq!(map_switch(true, Ok((2, 0))), bound(true, 2, 0));
    }

    #[test]
    fn every_rejection_keeps_the_had_previous_flag() {
        for flow in [
            AccountFlow::Unavailable,
            AccountFlow::Transport,
            AccountFlow::Rejected(5),
            AccountFlow::Rejected(1),
        ] {
            let fresh = map_switch(false, Err(flow));
            assert_eq!(fresh, SwitchOutcome::Rejected { had_previous: false, flow });
            let taken = map_switch(true, Err(flow));
            assert_eq!(taken, SwitchOutcome::Rejected { had_previous: true, flow });
        }
        // Bad credentials read back with their specific message so the
        // operator sees why, not just that.
        let SwitchOutcome::Rejected { flow, .. } =
            map_switch(true, Err(AccountFlow::Rejected(4)))
        else {
            panic!("expected rejection");
        };
        assert_eq!(flow.message(), "login rejected: unknown account");
    }

    #[test]
    fn switch_state_covers_the_full_identity_matrix() {
        // unowned -> su ok == plain login shape (had_previous false)
        // owned   -> su ok == switched shape (had_previous true)
        // unowned -> su fail == stays unowned
        // owned   -> su fail == dropped, now unowned
        assert!(matches!(map_switch(false, Ok((1, 1))), SwitchOutcome::Bound { had_previous: false, .. }));
        assert!(matches!(map_switch(true, Ok((1, 1))), SwitchOutcome::Bound { had_previous: true, .. }));
        assert!(matches!(
            map_switch(false, Err(AccountFlow::Unavailable)),
            SwitchOutcome::Rejected { had_previous: false, .. }
        ));
        assert!(matches!(
            map_switch(true, Err(AccountFlow::Unavailable)),
            SwitchOutcome::Rejected { had_previous: true, .. }
        ));
    }

    #[test]
    fn owner_snapshot_survives_name_roundtrip_like_sessions_do() {
        // The command snapshots Owner before dropping it; make sure the
        // snapshot path (Copy + name()) is usable in the no_std surface.
        let owner = sessions::Owner::none_named("paul", 3, 0x10).expect("fits");
        let snapshot = owner;
        assert_eq!(snapshot.name(), "paul");
        assert_eq!(snapshot.account_id, 3);
    }
}

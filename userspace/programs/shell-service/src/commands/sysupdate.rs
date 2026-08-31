//! Whole-system update operator flows on top of the package-service
//! maintenance extension actions: `sysupdate plan|apply|rollback|history`.
//! The wire layout mirrors the service-side reply in package-service's
//! `sysupdate_ops` (status/action/count/secondary/flags + packed payload).

use rt::{RawMessage, ServiceId};
use serviceos_userspace_runtime as rt;

use crate::util::{
    MAX_VERSION_BYTES, ShellOutput, printable_version, service_name, write_output_linef,
};

use super::package::mutate::{package_status_name, simple_request, status_from_word};
use super::package::query::CatalogSnapshot;
use rt::PackageStatus;

/// Maintenance action words extending `PackageMaintenanceAction`; agreed
/// with package-service (see its ops_model constants).
const ACTION_PLAN: u64 = 6;
const ACTION_APPLY: u64 = 7;
const ACTION_ROLLBACK: u64 = 8;
const ACTION_HISTORY: u64 = 9;

/// Reply flag bits from the service.
const FLAG_ROLLED_BACK: u64 = 1;
const FLAG_COMMITTED_TXN_PRESENT: u64 = 2;

/// Upper bound of planned ids the shell renders per reply payload.
const MAX_RENDERED_IDS: usize = 22;

pub(crate) fn cmd_sysupdate<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    mut parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("plan") => cmd_sysupdate_plan(bootstrap, output),
        Some("apply") => cmd_sysupdate_apply(bootstrap, output, parts.next()),
        Some("rollback") => cmd_sysupdate_rollback(bootstrap, output, parts.next()),
        Some("history") => cmd_sysupdate_history(bootstrap, output),
        _ => write_output_linef(
            output,
            format_args!("usage: sysupdate <plan|apply [--yes]|rollback [--yes]|history>"),
        ),
    }
}

fn sysupdate_call(bootstrap: rt::Handle, action: u64) -> rt::Result<RawMessage> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut request = simple_request(rt::PackageTag::MaintenanceRequest as u32, action);
    let reply = rt::channel_call(package_handle, &mut request);
    let _ = rt::handle_close(package_handle);
    reply
}

struct SysUpdateReply {
    status: PackageStatus,
    count: usize,
    secondary: u64,
    flags: u64,
}

/// Validate and decode the shared sysupdate reply shape; `None` when the
/// reply is not a sysupdate maintenance reply for the expected action.
fn decode_sysupdate_reply(reply: &RawMessage, expected_action: u64) -> Option<SysUpdateReply> {
    if reply.tag != rt::PackageTag::MaintenanceReply as u32 || reply.word_count < 5 {
        return None;
    }
    if reply.words[1] != expected_action {
        return None;
    }
    Some(SysUpdateReply {
        status: status_from_word(reply.words[0]),
        count: reply.words[2] as usize,
        secondary: reply.words[3],
        flags: reply.words[4],
    })
}

/// Unpack the two-service-ids-per-word payload into id slots, bounded by
/// the plan count carried in the reply header.
fn unpack_plan_ids(reply: &RawMessage, limit: usize) -> ([u32; MAX_RENDERED_IDS], usize) {
    let mut ids = [0u32; MAX_RENDERED_IDS];
    let mut count = 0usize;
    for word in reply.words[5..reply.word_count as usize].iter() {
        for value in [*word & 0xffff_ffff, *word >> 32] {
            if count >= limit.min(ids.len()) {
                return (ids, count);
            }
            ids[count] = value as u32;
            count += 1;
        }
    }
    (ids, count)
}

fn cmd_sysupdate_plan(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let reply = sysupdate_call(bootstrap, ACTION_PLAN)?;
    let Some(decoded) = decode_sysupdate_reply(&reply, ACTION_PLAN) else {
        return Err(rt::Error::InvalidArgument);
    };
    if decoded.status != PackageStatus::Ok {
        if decoded.status == PackageStatus::NoChange {
            return write_output_linef(output, format_args!("sysupdate plan: system up to date"));
        }
        return write_output_linef(
            output,
            format_args!(
                "sysupdate plan failed: {}",
                package_status_name(decoded.status)
            ),
        );
    }
    let (ids, rendered) = unpack_plan_ids(&reply, decoded.count);
    write_output_linef(
        output,
        format_args!(
            "sysupdate plan: {} package(s) would be updated",
            decoded.count
        ),
    )?;
    render_planned_rows(bootstrap, output, &ids, rendered)?;
    if decoded.flags & FLAG_COMMITTED_TXN_PRESENT != 0 {
        write_output_linef(
            output,
            format_args!("note: a committed update is pending rollback (sysupdate rollback)"),
        )?;
    }
    write_output_linef(
        output,
        format_args!("run `sysupdate apply --yes` to execute this transaction"),
    )
}

fn render_planned_rows(
    bootstrap: rt::Handle,
    output: ShellOutput,
    ids: &[u32; MAX_RENDERED_IDS],
    count: usize,
) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let catalog = CatalogSnapshot::capture(package_handle);
    let mut installed = [0u8; MAX_VERSION_BYTES];
    let mut active = [0u8; MAX_VERSION_BYTES];
    for slot in ids.iter().take(count) {
        let Some(service_id) = service_id_from_word(*slot) else {
            continue;
        };
        let mut row = [0u8; MAX_VERSION_BYTES];
        let mut row_len = 0usize;
        let mut index = 0usize;
        while row_len == 0 {
            let Ok(Some(entry)) =
                rt::package_list(package_handle, index, &mut installed, &mut active)
            else {
                break;
            };
            if entry.service_id == service_id && entry.installed_version_len > 0 {
                row_len = entry.installed_version_len.min(row.len());
                row[..row_len].copy_from_slice(&installed[..row_len]);
            }
            index += 1;
        }
        let installed_text = core::str::from_utf8(&row[..row_len]).unwrap_or("");
        let target_text = catalog.latest_text(service_id).unwrap_or("?");
        write_output_linef(
            output,
            format_args!(
                "  {:<16} {} -> {}",
                service_name(service_id),
                printable_version(installed_text),
                printable_version(target_text),
            ),
        )?;
    }
    let _ = rt::handle_close(package_handle);
    Ok(())
}

fn cmd_sysupdate_apply(
    bootstrap: rt::Handle,
    output: ShellOutput,
    confirm: Option<&str>,
) -> rt::Result<()> {
    let confirmed = matches!(confirm, Some("--yes"));
    let plan_reply = sysupdate_call(bootstrap, ACTION_PLAN)?;
    let Some(plan) = decode_sysupdate_reply(&plan_reply, ACTION_PLAN) else {
        return Err(rt::Error::InvalidArgument);
    };
    if plan.count == 0 {
        return write_output_linef(output, format_args!("sysupdate apply: nothing to apply"));
    }
    if !confirmed {
        write_output_linef(
            output,
            format_args!(
                "sysupdate apply: {} package(s) in one transaction with global rollback",
                plan.count
            ),
        )?;
        return write_output_linef(
            output,
            format_args!("re-run with --yes to execute; review with `sysupdate plan` first"),
        );
    }
    let reply = sysupdate_call(bootstrap, ACTION_APPLY)?;
    let Some(decoded) = decode_sysupdate_reply(&reply, ACTION_APPLY) else {
        return Err(rt::Error::InvalidArgument);
    };
    match decoded.status {
        PackageStatus::Ok => write_output_linef(
            output,
            format_args!(
                "sysupdate applied: {} package(s) updated; commit marker written",
                decoded.count
            ),
        ),
        PackageStatus::Interrupted => {
            write_output_linef(
                output,
                format_args!(
                    "sysupdate interrupted after {} package(s); step {} failed",
                    decoded.count, decoded.secondary,
                ),
            )?;
            write_output_linef(
                output,
                format_args!(
                    "transaction parked as failed; run `pkg recover` to resume or discard"
                ),
            )
        }
        other => write_output_linef(
            output,
            format_args!("sysupdate apply failed: {}", package_status_name(other)),
        ),
    }
}

fn cmd_sysupdate_rollback(
    bootstrap: rt::Handle,
    output: ShellOutput,
    confirm: Option<&str>,
) -> rt::Result<()> {
    if !matches!(confirm, Some("--yes")) {
        write_output_linef(
            output,
            format_args!(
                "sysupdate rollback: restores every package of the last committed update in reverse order"
            ),
        )?;
        return write_output_linef(output, format_args!("re-run with --yes to execute"));
    }
    let reply = sysupdate_call(bootstrap, ACTION_ROLLBACK)?;
    let Some(decoded) = decode_sysupdate_reply(&reply, ACTION_ROLLBACK) else {
        return Err(rt::Error::InvalidArgument);
    };
    match decoded.status {
        PackageStatus::Ok => {
            let cleared = decoded.flags & FLAG_ROLLED_BACK != 0;
            write_output_linef(
                output,
                format_args!(
                    "sysupdate rolled back: {} package(s) restored to prior versions{}",
                    decoded.count,
                    if cleared {
                        "; commit marker cleared"
                    } else {
                        ""
                    },
                ),
            )
        }
        PackageStatus::NoRollback => write_output_linef(
            output,
            format_args!("sysupdate rollback: no committed update to roll back"),
        ),
        PackageStatus::Interrupted => {
            write_output_linef(
                output,
                format_args!(
                    "sysupdate rollback interrupted after {} restore(s); step {} failed",
                    decoded.count, decoded.secondary,
                ),
            )?;
            write_output_linef(
                output,
                format_args!("run `pkg recover` to resume or discard"),
            )
        }
        other => write_output_linef(
            output,
            format_args!("sysupdate rollback failed: {}", package_status_name(other)),
        ),
    }
}

fn cmd_sysupdate_history(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let reply = sysupdate_call(bootstrap, ACTION_HISTORY)?;
    let Some(decoded) = decode_sysupdate_reply(&reply, ACTION_HISTORY) else {
        return Err(rt::Error::InvalidArgument);
    };
    if decoded.count == 0 {
        return write_output_linef(
            output,
            format_args!("sysupdate history: no transactions yet"),
        );
    }
    write_output_linef(
        output,
        format_args!(
            "sysupdate history: {} transaction(s), showing newest {}",
            decoded.count, decoded.secondary
        ),
    )?;
    let words = reply.word_count as usize;
    let mut index = 5usize;
    while index + 2 <= words {
        let tick = reply.words[index];
        let meta = reply.words[index + 1];
        let applied = meta & 0xffff_ffff;
        let rolled_back = meta >> 32 & 1 != 0;
        write_output_linef(
            output,
            format_args!(
                "  tick={:<10} applied={:<3} {}",
                tick,
                applied,
                if rolled_back {
                    "rolled-back"
                } else {
                    "committed"
                },
            ),
        )?;
        index += 2;
    }
    Ok(())
}

fn service_id_from_word(value: u32) -> Option<ServiceId> {
    match value {
        1 => Some(ServiceId::RootManager),
        2 => Some(ServiceId::Storage),
        3 => Some(ServiceId::Console),
        4 => Some(ServiceId::Config),
        5 => Some(ServiceId::Log),
        6 => Some(ServiceId::Status),
        7 => Some(ServiceId::Shell),
        8 => Some(ServiceId::Package),
        9 => Some(ServiceId::Announce),
        10 => Some(ServiceId::Network),
        11 => Some(ServiceId::Graphics),
        12 => Some(ServiceId::Session),
        13 => Some(ServiceId::DesktopShell),
        14 => Some(ServiceId::Terminal),
        15 => Some(ServiceId::Audio),
        16 => Some(ServiceId::Runtime),
        17 => Some(ServiceId::Developer),
        18 => Some(ServiceId::Clipboard),
        19 => Some(ServiceId::Security),
        20 => Some(ServiceId::SetupWizard),
        21 => Some(ServiceId::Backup),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(tag: u32, words: &[u64]) -> RawMessage {
        let mut message = RawMessage::empty(tag);
        for (index, value) in words.iter().enumerate() {
            message.words[index] = *value;
        }
        message.word_count = words.len() as u32;
        message
    }

    const PLAN: u64 = ACTION_PLAN;

    #[test]
    fn decode_validates_tag_and_action() {
        let good = message(rt::PackageTag::MaintenanceReply as u32, &[0, PLAN, 2, 0, 0]);
        assert!(decode_sysupdate_reply(&good, PLAN).is_some());
        assert!(decode_sysupdate_reply(&good, ACTION_APPLY).is_none());
        let wrong_tag = message(rt::PackageTag::ListReply as u32, &[0, PLAN, 2, 0, 0]);
        assert!(decode_sysupdate_reply(&wrong_tag, PLAN).is_none());
        let short = message(rt::PackageTag::MaintenanceReply as u32, &[0, PLAN]);
        assert!(decode_sysupdate_reply(&short, PLAN).is_none());
    }

    #[test]
    fn decode_maps_status_count_secondary_flags() {
        let reply = message(
            rt::PackageTag::MaintenanceReply as u32,
            &[
                PackageStatus::Interrupted as u64,
                ACTION_APPLY,
                1,
                3,
                FLAG_ROLLED_BACK | FLAG_COMMITTED_TXN_PRESENT,
            ],
        );
        let decoded = decode_sysupdate_reply(&reply, ACTION_APPLY).unwrap();
        assert_eq!(decoded.status, PackageStatus::Interrupted);
        assert_eq!(decoded.count, 1);
        assert_eq!(decoded.secondary, 3);
        assert_eq!(decoded.flags, 3);
    }

    #[test]
    fn plan_ids_unpack_pairs_and_stop_on_zero_word() {
        let reply = message(
            rt::PackageTag::MaintenanceReply as u32,
            &[0, PLAN, 3, 0, 0, 8 | (10 << 32), 12, 0],
        );
        let (ids, count) = unpack_plan_ids(&reply, 3);
        assert_eq!(count, 3);
        assert_eq!(&ids[..3], &[8, 10, 12]);

        // Odd counts pad the second half-word with zero.
        let single = message(
            rt::PackageTag::MaintenanceReply as u32,
            &[0, PLAN, 1, 0, 0, 9, 0],
        );
        let (ids, count) = unpack_plan_ids(&single, 1);
        assert_eq!(count, 1);
        assert_eq!(ids[0], 9);

        // The limit (reply header count) bounds rendering, not zero words.
        let padded = message(
            rt::PackageTag::MaintenanceReply as u32,
            &[0, PLAN, 2, 0, 0, 4 | (5 << 32)],
        );
        let (_, count) = unpack_plan_ids(&padded, 2);
        assert_eq!(count, 2);
        let (_, clamped) = unpack_plan_ids(&padded, 99);
        assert!(clamped <= MAX_RENDERED_IDS);
    }

    #[test]
    fn plan_ids_unpack_clamps_to_slot_budget() {
        // A full reply carries at most IPC_MAX_WORDS - 5 payload words.
        let mut words = vec![0u64, PLAN, 99, 0, 0];
        for slot in 0..(rt::IPC_MAX_WORDS - 5) as u64 {
            words.push(slot | ((slot + 50) << 32));
        }
        let reply = message(rt::PackageTag::MaintenanceReply as u32, &words);
        let (ids, count) = unpack_plan_ids(&reply, 99);
        assert_eq!(count, MAX_RENDERED_IDS);
        assert_eq!(ids[0], 0);
        assert_eq!(ids[MAX_RENDERED_IDS - 1], 60);
    }

    #[test]
    fn service_id_round_trip_matches_names_table() {
        assert_eq!(
            service_id_from_word(ServiceId::Package as u32),
            Some(ServiceId::Package)
        );
        assert_eq!(
            service_id_from_word(ServiceId::Storage as u32),
            Some(ServiceId::Storage)
        );
        assert_eq!(service_id_from_word(0), None);
        assert_eq!(service_id_from_word(9999), None);
    }

    #[test]
    fn history_meta_decodes_applied_and_flag() {
        let applied = 3u64;
        let meta = applied | (1u64 << 32);
        assert_eq!(meta & 0xffff_ffff, 3);
        assert!(meta >> 32 & 1 != 0);
        let clean = 5u64;
        assert_eq!(clean & 0xffff_ffff, 5);
        assert!(clean >> 32 & 1 == 0);
    }

    #[test]
    fn action_words_match_service_contract() {
        // Keep shell/service action words locked together; the service side
        // pins the same values in ops_model.
        assert_eq!(ACTION_PLAN, 6);
        assert_eq!(ACTION_APPLY, 7);
        assert_eq!(ACTION_ROLLBACK, 8);
        assert_eq!(ACTION_HISTORY, 9);
        assert_eq!(FLAG_ROLLED_BACK, 1);
        assert_eq!(FLAG_COMMITTED_TXN_PRESENT, 2);
    }
}

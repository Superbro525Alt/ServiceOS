pub(in crate::commands) mod mutate;
mod onboard;
pub(in crate::commands) mod parse;
pub(in crate::commands) mod query;
mod repos;
mod keys;

use serviceos_userspace_runtime as rt;

use crate::util::{ShellOutput, parse_service_name, write_output_linef};

pub(crate) fn gate_run_image(output: ShellOutput, path: &str) -> rt::Result<bool> {
    onboard::sideload_image_gate(output, path)
}

pub(crate) fn cmd_pkg<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    mut parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("list") => query::cmd_pkg_list(bootstrap, output),
        Some("catalog") => query::cmd_pkg_catalog(bootstrap, output),
        Some("repos") => repos::cmd_pkg_repos(bootstrap, output),
        Some("repo") => repos::cmd_pkg_repo(bootstrap, output, parts),
        Some("info") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => query::cmd_pkg_info(bootstrap, output, service_id),
            None => write_output_linef(output, format_args!("usage: pkg info <name>")),
        },
        Some("install") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => mutate::cmd_pkg_install(bootstrap, output, service_id, parts),
            None => write_output_linef(
                output,
                format_args!(
                    "usage: pkg install <name> [version] [@source] [--yes] [--force-compat]"
                ),
            ),
        },
        Some("update") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => mutate::cmd_pkg_update(bootstrap, output, service_id, parts),
            None => write_output_linef(
                output,
                format_args!(
                    "usage: pkg update <name> [version] [@source] [--yes] [--force-compat]"
                ),
            ),
        },
        Some("remove") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => mutate::cmd_pkg_remove(bootstrap, output, service_id),
            None => write_output_linef(output, format_args!("usage: pkg remove <name>")),
        },
        Some("rollback") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => mutate::cmd_pkg_rollback(bootstrap, output, service_id),
            None => write_output_linef(output, format_args!("usage: pkg rollback <name>")),
        },
        Some("history") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => query::cmd_pkg_history(bootstrap, output, service_id),
            None => write_output_linef(output, format_args!("usage: pkg history <name>")),
        },
        Some("provenance") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => query::cmd_pkg_provenance(bootstrap, output, service_id),
            None => write_output_linef(output, format_args!("usage: pkg provenance <name>")),
        },
        Some("policy") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => query::cmd_pkg_policy(bootstrap, output, service_id),
            None => write_output_linef(output, format_args!("usage: pkg policy <name>")),
        },
        Some("pin") => match (parts.next().and_then(parse_service_name), parts.next()) {
            (Some(service_id), Some(version)) => {
                mutate::cmd_pkg_pin(bootstrap, output, service_id, version)
            }
            _ => write_output_linef(output, format_args!("usage: pkg pin <name> <version|none>")),
        },
        Some("channel") => match (parts.next().and_then(parse_service_name), parts.next()) {
            (Some(service_id), Some(channel)) => {
                mutate::cmd_pkg_channel(bootstrap, output, service_id, channel)
            }
            _ => write_output_linef(
                output,
                format_args!("usage: pkg channel <name> <stable|beta|canary>"),
            ),
        },
        Some("ring") => match (parts.next().and_then(parse_service_name), parts.next()) {
            (Some(service_id), Some(ring)) => {
                mutate::cmd_pkg_ring(bootstrap, output, service_id, ring)
            }
            _ => write_output_linef(
                output,
                format_args!("usage: pkg ring <name> <production|preview|testing>"),
            ),
        },
        Some("keys") => keys::cmd_pkg_keys(bootstrap, output, parts),
        Some("verify") => {
            mutate::cmd_pkg_maintenance(bootstrap, output, rt::PackageMaintenanceAction::Validate)
        }
        Some("repair") => {
            mutate::cmd_pkg_maintenance(bootstrap, output, rt::PackageMaintenanceAction::Repair)
        }
        Some("recover") => mutate::cmd_pkg_recover(bootstrap, output),
        Some("sideload") => onboard::cmd_pkg_sideload(output, parts),
        Some("gc") => mutate::cmd_pkg_maintenance(
            bootstrap,
            output,
            rt::PackageMaintenanceAction::GarbageCollect,
        ),
        _ => write_output_linef(
            output,
            format_args!(
                "usage: pkg <list|catalog|repos|repo|info|install|update|remove|rollback|history|provenance|policy|keys|pin|channel|ring|verify|repair|recover|gc|sideload> ..."
            ),
        ),
    }
}

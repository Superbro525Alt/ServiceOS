mod audio;
mod core;
mod desktop;
mod developer;
mod graphics;
mod network;
mod package;
mod runtime;
mod security;

use serviceos_userspace_runtime as rt;

use crate::util::{
    HELP_TEXT, ShellOutput, parse_service_name, shell_output_write, write_output_linef,
};

pub(crate) fn execute_command(
    bootstrap: rt::Handle,
    output: ShellOutput,
    line: &str,
) -> rt::Result<()> {
    let mut parts = line.split_whitespace();
    let Some(command) = parts.next() else {
        return Ok(());
    };

    match command {
        "help" => shell_output_write(output, HELP_TEXT),
        "services" => core::cmd_services(bootstrap, output),
        "service" => match parts.next().and_then(parse_service_name) {
            Some(service_id) => core::cmd_service(bootstrap, output, service_id),
            None => write_output_linef(output, format_args!("usage: service <name>")),
        },
        "service-caps" => match parts.next().and_then(parse_service_name) {
            Some(service_id) => core::cmd_service_caps(bootstrap, output, service_id),
            None => write_output_linef(output, format_args!("usage: service-caps <name>")),
        },
        "service-revoke-lookup" => match (
            parts.next().and_then(parse_service_name),
            parts.next().and_then(parse_service_name),
            parts.next(),
        ) {
            (Some(service_id), Some(target), Some("revoke")) => {
                core::cmd_service_revoke_lookup(bootstrap, output, service_id, target, true)
            }
            (Some(service_id), Some(target), Some("default")) => {
                core::cmd_service_revoke_lookup(bootstrap, output, service_id, target, false)
            }
            _ => write_output_linef(
                output,
                format_args!("usage: service-revoke-lookup <service> <target> <revoke|default>"),
            ),
        },
        "restart" => match parts.next().and_then(parse_service_name) {
            Some(service_id) => core::cmd_restart(bootstrap, output, service_id),
            None => write_output_linef(output, format_args!("usage: restart <name>")),
        },
        "logs" => match parts.next() {
            Some("stream") => {
                let count = parts
                    .next()
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(12);
                core::cmd_logs_stream(bootstrap, output, count)
            }
            maybe_count => {
                let count = maybe_count
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(12);
                core::cmd_logs(bootstrap, output, count)
            }
        },
        "config" => match parts.next() {
            None => core::cmd_config(bootstrap, output),
            Some("get") => match parts.next() {
                Some(key) => core::cmd_config_get(bootstrap, output, key),
                None => write_output_linef(output, format_args!("usage: config get <key>")),
            },
            Some("set") => match (
                parts.next(),
                parts.next().and_then(|value| value.parse::<u64>().ok()),
            ) {
                (Some(key), Some(value)) => core::cmd_config_set(bootstrap, output, key, value),
                _ => write_output_linef(output, format_args!("usage: config set <key> <value>")),
            },
            _ => write_output_linef(output, format_args!("usage: config [get|set] ...")),
        },
        "store" => match parts.next() {
            Some("ls") => core::cmd_store_ls(bootstrap, output, parts.next().unwrap_or("")),
            Some("mounts") => core::cmd_store_mounts(bootstrap, output),
            Some("mkdir") => match parts.next() {
                Some(path) => core::cmd_store_mkdir(bootstrap, output, path),
                None => write_output_linef(output, format_args!("usage: store mkdir <path>")),
            },
            Some("write") => match parts.next() {
                Some(path) => core::cmd_store_write(bootstrap, output, path, parts),
                None => {
                    write_output_linef(output, format_args!("usage: store write <path> <text>"))
                }
            },
            Some("rm") => match parts.next() {
                Some(path) => core::cmd_store_rm(bootstrap, output, path),
                None => write_output_linef(output, format_args!("usage: store rm <path>")),
            },
            _ => write_output_linef(
                output,
                format_args!("usage: store <ls|mounts|mkdir|write|rm> ..."),
            ),
        },
        "cat" => match parts.next() {
            Some(path) => core::cmd_cat(bootstrap, output, path),
            None => write_output_linef(output, format_args!("usage: cat <path>")),
        },
        "status" => match parts.next() {
            None => core::cmd_status_snapshot(bootstrap, output),
            Some("services") => core::cmd_status_services(bootstrap, output),
            Some("watch") => {
                let count = parts
                    .next()
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(8);
                core::cmd_status_watch(bootstrap, output, count)
            }
            _ => write_output_linef(
                output,
                format_args!("usage: status [services|watch [count]]"),
            ),
        },
        "net" => network::cmd_net(bootstrap, output, parts),
        "audio" => audio::cmd_audio(bootstrap, output, parts),
        "gfx" => graphics::cmd_gfx(bootstrap, output, parts),
        "desktop" => desktop::cmd_desktop(bootstrap, output, parts),
        "dev" => developer::cmd_dev(bootstrap, output, parts),
        "pkg" => package::cmd_pkg(bootstrap, output, parts),
        "runtime" => runtime::cmd_runtime(bootstrap, output, parts),
        "security" => security::cmd_security(bootstrap, output, parts),
        "run" => match parts.next() {
            Some("sysinfo") => core::cmd_run_sysinfo(bootstrap, output),
            Some("image") => match parts.next() {
                Some(path) => core::cmd_run_image(bootstrap, output, path),
                None => write_output_linef(output, format_args!("usage: run image <path>")),
            },
            _ => write_output_linef(output, format_args!("usage: run <sysinfo|image>")),
        },
        _ => write_output_linef(output, format_args!("unknown command: {command}")),
    }
}

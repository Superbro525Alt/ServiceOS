mod audio;
mod core;
mod desktop;
mod developer;
mod graphics;
mod network;
mod package;
mod runtime;

use serviceos_userspace_runtime as rt;

use crate::util::{
    parse_service_name, shell_output_write, write_output_linef, HELP_TEXT,
    ShellOutput,
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
        "restart" => match parts.next().and_then(parse_service_name) {
            Some(service_id) => core::cmd_restart(bootstrap, output, service_id),
            None => write_output_linef(output, format_args!("usage: restart <name>")),
        },
        "logs" => {
            let count = parts
                .next()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(12);
            core::cmd_logs(bootstrap, output, count)
        }
        "config" => core::cmd_config(bootstrap, output),
        "store" => match parts.next() {
            Some("ls") => core::cmd_store_ls(bootstrap, output, parts.next().unwrap_or("")),
            Some("mkdir") => match parts.next() {
                Some(path) => core::cmd_store_mkdir(bootstrap, output, path),
                None => write_output_linef(output, format_args!("usage: store mkdir <path>")),
            },
            Some("write") => match parts.next() {
                Some(path) => core::cmd_store_write(bootstrap, output, path, parts),
                None => write_output_linef(output, format_args!("usage: store write <path> <text>")),
            },
            Some("rm") => match parts.next() {
                Some(path) => core::cmd_store_rm(bootstrap, output, path),
                None => write_output_linef(output, format_args!("usage: store rm <path>")),
            },
            _ => write_output_linef(output, format_args!("usage: store <ls|mkdir|write|rm> ...")),
        },
        "cat" => match parts.next() {
            Some(path) => core::cmd_cat(bootstrap, output, path),
            None => write_output_linef(output, format_args!("usage: cat <path>")),
        },
        "status" => core::cmd_status(bootstrap, output),
        "net" => network::cmd_net(bootstrap, output, parts),
        "audio" => audio::cmd_audio(bootstrap, output, parts),
        "gfx" => graphics::cmd_gfx(bootstrap, output, parts),
        "desktop" => desktop::cmd_desktop(bootstrap, output, parts),
        "dev" => developer::cmd_dev(bootstrap, output, parts),
        "pkg" => package::cmd_pkg(bootstrap, output, parts),
        "runtime" => runtime::cmd_runtime(bootstrap, output, parts),
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

mod core;
mod desktop;
mod graphics;
mod network;
mod package;

use serviceos_userspace_runtime as rt;

use crate::util::{
    parse_desktop_app_name, parse_service_name, write_session_linef, HELP_TEXT,
};

pub(crate) fn execute_command(
    bootstrap: rt::Handle,
    session: rt::Handle,
    line: &str,
) -> rt::Result<()> {
    let mut parts = line.split_whitespace();
    let Some(command) = parts.next() else {
        return Ok(());
    };

    match command {
        "help" => crate::util::write_session_text(session, HELP_TEXT),
        "services" => core::cmd_services(bootstrap, session),
        "service" => match parts.next().and_then(parse_service_name) {
            Some(service_id) => core::cmd_service(bootstrap, session, service_id),
            None => write_session_linef(session, format_args!("usage: service <name>")),
        },
        "restart" => match parts.next().and_then(parse_service_name) {
            Some(service_id) => core::cmd_restart(bootstrap, session, service_id),
            None => write_session_linef(session, format_args!("usage: restart <name>")),
        },
        "logs" => {
            let count = parts
                .next()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(12);
            core::cmd_logs(bootstrap, session, count)
        }
        "config" => core::cmd_config(bootstrap, session),
        "store" => match parts.next() {
            Some("ls") => core::cmd_store_ls(bootstrap, session, parts.next().unwrap_or("")),
            _ => write_session_linef(session, format_args!("usage: store ls [prefix]")),
        },
        "cat" => match parts.next() {
            Some(path) => core::cmd_cat(bootstrap, session, path),
            None => write_session_linef(session, format_args!("usage: cat <path>")),
        },
        "status" => core::cmd_status(bootstrap, session),
        "net" => network::cmd_net(bootstrap, session, parts),
        "gfx" => graphics::cmd_gfx(bootstrap, session, parts),
        "desktop" => desktop::cmd_desktop(bootstrap, session, parts),
        "pkg" => package::cmd_pkg(bootstrap, session, parts),
        "run" => match parts.next() {
            Some("sysinfo") => core::cmd_run_sysinfo(bootstrap, session),
            _ => write_session_linef(session, format_args!("usage: run sysinfo")),
        },
        _ => write_session_linef(session, format_args!("unknown command: {command}")),
    }
}

#![no_std]
#![no_main]

use core::fmt::Write;

use serviceos_userspace_runtime as rt;
use rt::{
    ConfigKey, DesktopAppId, DesktopAppInfo, DesktopDragMode, DesktopWindowInfo, FixedLogBuffer,
    LogDomain, LogEvent, LogSeverity, ManagerServiceInfo, ManagerServicePhase, RawMessage,
    ServiceId, ServiceImageId,
};

const MAX_LINE_BYTES: usize = 128;
const MAX_LISTED_SERVICES: usize = 12;
const MAX_STORAGE_PATH: usize = 96;
const MAX_CAT_CHUNK: usize = 96;
const MAX_VERSION_BYTES: usize = 24;
const MAX_DESKTOP_APPS: usize = 8;
const MAX_DESKTOP_WINDOWS: usize = 8;
const MAX_SESSION_WRITE_BYTES: usize = (rt::IPC_MAX_WORDS - 1) * 8;
const HELP_TEXT: &str = "\
help: show this command list\r\n\
services: list managed services\r\n\
service <name>: show one service state\r\n\
restart <name>: request a service restart\r\n\
logs [count]: show recent structured logs\r\n\
config: show core configuration values\r\n\
store ls [prefix]: list boot-store paths\r\n\
cat <path>: print a text resource\r\n\
status: show system heartbeat status\r\n\
net ifaces: show network interfaces\r\n\
net route: show the default route\r\n\
net resolve <name>: resolve a host or literal\r\n\
net ping <name|ip>: run an ICMP reachability probe\r\n\
gfx outputs: show graphics outputs\r\n\
gfx surfaces: show compositor surfaces\r\n\
gfx sessions: show graphical sessions\r\n\
gfx focus <surface-id>: change focused session surface\r\n\
desktop status: show desktop shell status\r\n\
desktop apps: list desktop app state\r\n\
desktop windows: list desktop window state\r\n\
desktop launch <settings|files|monitor>: launch a desktop app\r\n\
desktop focus <settings|files|monitor>: focus a desktop app\r\n\
desktop next: focus the next visible window\r\n\
desktop close <settings|files|monitor>: close a desktop app window\r\n\
desktop minimize <settings|files|monitor>: minimize a desktop app window\r\n\
desktop restore <settings|files|monitor>: restore a minimized app window\r\n\
desktop maximize <settings|files|monitor>: maximize or restore a window\r\n\
desktop move <settings|files|monitor> <x> <y>: move a window\r\n\
desktop resize <settings|files|monitor> <width> <height>: resize a window\r\n\
desktop click <x> <y>: inject a pointer click into the desktop session\r\n\
pkg list: list repository packages\r\n\
pkg info <name>: inspect one package\r\n\
pkg install <name> [version]: activate a package\r\n\
pkg update <name> [version]: switch to a newer package version\r\n\
pkg remove <name>: deactivate a package\r\n\
pkg rollback <name>: restore the prior active version\r\n\
pkg history <name>: show current and rollback versions\r\n\
run sysinfo: launch a transient tool\r\n";

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

    let _ = emit_shell_log(bootstrap, LogSeverity::Info, LogEvent::SessionOpened, 1, 0);
    let _ = write_session_linef(
        session_handle,
        format_args!("serviceos shell ready; type 'help' for commands"),
    );

    let mut line_buffer = [0u8; MAX_LINE_BYTES];
    loop {
        let _ = rt::console_session_write(session_handle, "serviceos> ");
        let line_len = match rt::console_session_read_line(session_handle, &mut line_buffer) {
            Ok(len) => len,
            Err(_) => return 0xf707,
        };
        let Ok(raw_line) = core::str::from_utf8(&line_buffer[..line_len]) else {
            let _ = write_session_linef(session_handle, format_args!("invalid utf-8 input"));
            continue;
        };
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        let _ = emit_shell_log(
            bootstrap,
            LogSeverity::Debug,
            LogEvent::ShellCommand,
            line.len() as u64,
            0,
        );
        if let Err(error) = execute_command(bootstrap, session_handle, line) {
            let _ = write_session_linef(
                session_handle,
                format_args!("command failed: {}", error_name(error)),
            );
        }
    }
}

fn execute_command(bootstrap: rt::Handle, session: rt::Handle, line: &str) -> rt::Result<()> {
    let mut parts = line.split_whitespace();
    let Some(command) = parts.next() else {
        return Ok(());
    };

    match command {
        "help" => print_help(session),
        "services" => cmd_services(bootstrap, session),
        "service" => match parts.next().and_then(parse_service_name) {
            Some(service_id) => cmd_service(bootstrap, session, service_id),
            None => write_session_linef(session, format_args!("usage: service <name>")),
        },
        "restart" => match parts.next().and_then(parse_service_name) {
            Some(service_id) => cmd_restart(bootstrap, session, service_id),
            None => write_session_linef(session, format_args!("usage: restart <name>")),
        },
        "logs" => {
            let count = parts
                .next()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(12);
            cmd_logs(bootstrap, session, count)
        }
        "config" => cmd_config(bootstrap, session),
        "store" => match parts.next() {
            Some("ls") => cmd_store_ls(bootstrap, session, parts.next().unwrap_or("")),
            _ => write_session_linef(session, format_args!("usage: store ls [prefix]")),
        },
        "cat" => match parts.next() {
            Some(path) => cmd_cat(bootstrap, session, path),
            None => write_session_linef(session, format_args!("usage: cat <path>")),
        },
        "status" => cmd_status(bootstrap, session),
        "net" => cmd_net(bootstrap, session, parts),
        "gfx" => cmd_gfx(bootstrap, session, parts),
        "desktop" => cmd_desktop(bootstrap, session, parts),
        "pkg" => cmd_pkg(bootstrap, session, parts),
        "run" => match parts.next() {
            Some("sysinfo") => cmd_run_sysinfo(bootstrap, session),
            _ => write_session_linef(session, format_args!("usage: run sysinfo")),
        },
        _ => write_session_linef(session, format_args!("unknown command: {command}")),
    }
}

fn print_help(session: rt::Handle) -> rt::Result<()> {
    write_session_text(session, HELP_TEXT)
}

fn cmd_services(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let mut services = [ManagerServiceInfo {
        service_id: ServiceId::RootManager,
        phase: ManagerServicePhase::Dormant,
        attempts: 0,
    }; MAX_LISTED_SERVICES];
    let count = rt::manager_list_services(bootstrap, &mut services)?;
    for info in services[..count].iter().copied() {
        write_session_linef(
            session,
            format_args!(
                "{:<16} phase={} attempts={}",
                service_name(info.service_id),
                phase_name(info.phase),
                info.attempts,
            ),
        )?;
    }
    Ok(())
}

fn cmd_service(bootstrap: rt::Handle, session: rt::Handle, service_id: ServiceId) -> rt::Result<()> {
    let (status, phase, attempts, last_exit) = rt::manager_service_status(bootstrap, service_id)?;
    write_session_linef(
        session,
        format_args!(
            "{} status={} phase={} attempts={} last-exit={:#x}",
            service_name(service_id),
            manager_status_name(status),
            phase_name(phase),
            attempts,
            last_exit,
        ),
    )
}

fn cmd_restart(bootstrap: rt::Handle, session: rt::Handle, service_id: ServiceId) -> rt::Result<()> {
    rt::manager_restart_service(bootstrap, service_id)?;
    write_session_linef(
        session,
        format_args!("restart requested for {}", service_name(service_id)),
    )
}

fn cmd_logs(bootstrap: rt::Handle, session: rt::Handle, count: usize) -> rt::Result<()> {
    let log_handle = rt::lookup_service(bootstrap, ServiceId::Log)?;
    let (oldest, next) = rt::log_query_info(log_handle)?;
    if next == 0 || oldest == next {
        let _ = rt::handle_close(log_handle);
        return write_session_linef(session, format_args!("no log records"));
    }

    let start = oldest.max(next.saturating_sub(count as u64));
    for sequence in start..next {
        if let Some(record) = rt::log_query_record(log_handle, sequence)? {
            write_log_record(session, record)?;
        }
    }

    let _ = rt::handle_close(log_handle);
    Ok(())
}

fn cmd_config(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let config_handle = rt::lookup_service(bootstrap, ServiceId::Config)?;
    for key in [
        ConfigKey::LogMinimumSeverity,
        ConfigKey::StatusHeartbeatTicks,
        ConfigKey::StatusConsoleMirror,
        ConfigKey::StatusHeartbeatLogPeriod,
        ConfigKey::NetworkIpv4Address,
        ConfigKey::NetworkIpv4PrefixLength,
        ConfigKey::NetworkIpv4Gateway,
        ConfigKey::NetworkProbeTimeoutTicks,
    ] {
        let (_, value) = rt::config_read(config_handle, key)?;
        write_session_linef(
            session,
            format_args!("{} = {}", config_key_name(key), config_value_text(key, value)),
        )?;
    }
    let _ = rt::handle_close(config_handle);
    Ok(())
}

fn cmd_store_ls(bootstrap: rt::Handle, session: rt::Handle, prefix: &str) -> rt::Result<()> {
    let storage_handle = rt::lookup_service(bootstrap, ServiceId::Storage)?;
    let mut path_buffer = [0u8; MAX_STORAGE_PATH];
    let mut index = 0usize;
    while let Some((_, path_len)) = rt::storage_list(storage_handle, prefix, index, &mut path_buffer)? {
        let path = core::str::from_utf8(&path_buffer[..path_len]).map_err(|_| rt::Error::InvalidArgument)?;
        write_session_linef(session, format_args!("{path}"))?;
        index += 1;
    }
    let _ = rt::handle_close(storage_handle);
    if index == 0 {
        write_session_linef(session, format_args!("no entries"))
    } else {
        Ok(())
    }
}

fn cmd_cat(bootstrap: rt::Handle, session: rt::Handle, path: &str) -> rt::Result<()> {
    let storage_handle = rt::lookup_service(bootstrap, ServiceId::Storage)?;
    let (blob_handle, blob_len) = rt::storage_open(storage_handle, path)?;
    let _ = rt::handle_close(storage_handle);

    let mut offset = 0usize;
    let mut buffer = [0u8; MAX_CAT_CHUNK];
    while offset < blob_len {
        let read = rt::storage_read(blob_handle, offset, &mut buffer)?;
        if read == 0 {
            break;
        }
        let text = core::str::from_utf8(&buffer[..read]).map_err(|_| rt::Error::InvalidArgument)?;
        rt::console_session_write(session, text)?;
        offset += read;
    }
    let _ = rt::storage_blob_close(blob_handle);
    rt::console_session_write(session, "\r\n")?;
    Ok(())
}

fn cmd_status(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let status_handle = rt::lookup_service(bootstrap, ServiceId::Status)?;
    let (heartbeats, last_tick) = rt::status_snapshot(status_handle)?;
    let _ = rt::handle_close(status_handle);
    let now = rt::monotonic_now()?;
    write_session_linef(
        session,
        format_args!("ticks={} heartbeats={} last-heartbeat={}", now, heartbeats, last_tick),
    )
}

fn cmd_run_sysinfo(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let task_handle = rt::manager_launch_program(bootstrap, ServiceImageId::SysinfoTool, Some(session))?;
    let status = rt::wait_for_exit(task_handle)?;
    let _ = rt::handle_close(task_handle);
    write_session_linef(
        session,
        format_args!("sysinfo-tool exited with {:#x}", status.exit_code),
    )
}

fn cmd_net<'a, I>(bootstrap: rt::Handle, session: rt::Handle, mut parts: I) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("ifaces") => cmd_net_ifaces(bootstrap, session),
        Some("route") => cmd_net_route(bootstrap, session),
        Some("resolve") => match parts.next() {
            Some(target) => cmd_net_resolve(bootstrap, session, target),
            None => write_session_linef(session, format_args!("usage: net resolve <name>")),
        },
        Some("ping") => match parts.next() {
            Some(target) => cmd_net_ping(bootstrap, session, target),
            None => write_session_linef(session, format_args!("usage: net ping <name|ip>")),
        },
        _ => write_session_linef(session, format_args!("usage: net <ifaces|route|resolve|ping> ...")),
    }
}

fn cmd_net_ifaces(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let count = rt::network_interface_count(network_handle)?;
    if count == 0 {
        let _ = rt::handle_close(network_handle);
        return write_session_linef(session, format_args!("no interfaces"));
    }

    for index in 0..count {
        if let Some(info) = rt::network_interface_status(network_handle, index)? {
            write_session_linef(
                session,
                format_args!(
                    "net{} link={} addr={}/{} gw={} mac={} mtu={} rx={} tx={} drop={}",
                    info.index,
                    link_state_name(info.link_state),
                    format_ipv4(info.address),
                    info.prefix_len,
                    format_ipv4(info.gateway),
                    format_mac(info.mac),
                    info.mtu,
                    info.rx_packets,
                    info.tx_packets,
                    info.dropped_packets,
                ),
            )?;
        }
    }

    let _ = rt::handle_close(network_handle);
    Ok(())
}

fn cmd_net_route(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let info = rt::network_interface_status(network_handle, 0)?;
    let _ = rt::handle_close(network_handle);
    match info {
        Some(info) => write_session_linef(
            session,
            format_args!("default via {} dev net{}", format_ipv4(info.gateway), info.index),
        ),
        None => write_session_linef(session, format_args!("no default route")),
    }
}

fn cmd_net_resolve(bootstrap: rt::Handle, session: rt::Handle, target: &str) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let mut addresses = [0u32; 4];
    let count = match rt::network_resolve(network_handle, target, &mut addresses) {
        Ok(count) => count,
        Err(rt::Error::NotFound) => {
            let _ = rt::handle_close(network_handle);
            return write_session_linef(session, format_args!("no address for {}", target));
        }
        Err(error) => {
            let _ = rt::handle_close(network_handle);
            return Err(error);
        }
    };
    let _ = rt::handle_close(network_handle);
    if count == 0 {
        return write_session_linef(session, format_args!("no result"));
    }
    for address in addresses.iter().copied().take(count) {
        write_session_linef(session, format_args!("{} -> {}", target, format_ipv4(address)))?;
    }
    Ok(())
}

fn cmd_net_ping(bootstrap: rt::Handle, session: rt::Handle, target: &str) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let result = rt::network_ping(network_handle, target);
    let _ = rt::handle_close(network_handle);
    match result {
        Ok((resolved, elapsed_ms)) => write_session_linef(
            session,
            format_args!(
                "ping {} ({}) ok {}ms",
                target,
                format_ipv4(resolved),
                elapsed_ms,
            ),
        ),
        Err(rt::Error::QueueEmpty) => {
            write_session_linef(session, format_args!("ping {} timed out", target))
        }
        Err(rt::Error::NotFound) => {
            write_session_linef(session, format_args!("ping target not found: {}", target))
        }
        Err(error) => Err(error),
    }
}

fn cmd_gfx<'a, I>(bootstrap: rt::Handle, session: rt::Handle, mut parts: I) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("outputs") => cmd_gfx_outputs(bootstrap, session),
        Some("surfaces") => cmd_gfx_surfaces(bootstrap, session),
        Some("sessions") => cmd_gfx_sessions(bootstrap, session),
        Some("focus") => match parts.next().and_then(|value| value.parse::<u32>().ok()) {
            Some(surface_id) => cmd_gfx_focus(bootstrap, session, surface_id),
            None => write_session_linef(session, format_args!("usage: gfx focus <surface-id>")),
        },
        _ => write_session_linef(
            session,
            format_args!("usage: gfx <outputs|surfaces|sessions|focus> ..."),
        ),
    }
}

fn cmd_gfx_outputs(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let graphics_handle = rt::lookup_service(bootstrap, ServiceId::Graphics)?;
    let count = rt::graphics_output_count(graphics_handle)?;
    if count == 0 {
        let _ = rt::handle_close(graphics_handle);
        return write_session_linef(session, format_args!("no outputs"));
    }

    for index in 0..count {
        if let Some(output) = rt::graphics_output_status(graphics_handle, index)? {
            write_session_linef(
                session,
                format_args!(
                    "out{} backend={} state={} mode={}x{} stride={} format={} surfaces={} presents={}",
                    output.index,
                    display_backend_name(output.backend),
                    display_state_name(output.state),
                    output.width,
                    output.height,
                    output.stride,
                    pixel_format_name(output.pixel_format),
                    output.surface_count,
                    output.present_count,
                ),
            )?;
        }
    }

    let _ = rt::handle_close(graphics_handle);
    Ok(())
}

fn cmd_gfx_surfaces(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let graphics_handle = rt::lookup_service(bootstrap, ServiceId::Graphics)?;
    let mut surface_ids = [0u32; 8];
    let count = rt::graphics_surface_list(graphics_handle, &mut surface_ids)?;
    if count == 0 {
        let _ = rt::handle_close(graphics_handle);
        return write_session_linef(session, format_args!("no surfaces"));
    }

    for surface_id in surface_ids.iter().copied().take(count) {
        if let Some(surface) = rt::graphics_surface_status(graphics_handle, surface_id)? {
            write_session_linef(
                session,
                format_args!(
                    "surface{} session={} pos=({}, {}) size={}x{} z={} color=#{:06x} visible={}",
                    surface.surface_id,
                    surface.owner_session,
                    surface.x,
                    surface.y,
                    surface.width,
                    surface.height,
                    surface.z_order,
                    surface.fill_rgb,
                    surface.visible,
                ),
            )?;
        }
    }

    let _ = rt::handle_close(graphics_handle);
    Ok(())
}

fn cmd_gfx_sessions(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let session_handle = rt::lookup_service(bootstrap, ServiceId::Session)?;
    let mut session_ids = [0u32; 4];
    let count = rt::session_list(session_handle, &mut session_ids)?;
    if count == 0 {
        let _ = rt::handle_close(session_handle);
        return write_session_linef(session, format_args!("no sessions"));
    }

    for session_id in session_ids.iter().copied().take(count) {
        if let Some(status) = rt::session_status(session_handle, session_id)? {
            write_session_linef(
                session,
                format_args!(
                    "session{} input={} focused-surface={} surfaces={}",
                    status.session_id,
                    session_input_source_name(status.input_source),
                    status.focused_surface,
                    status.surface_count,
                ),
            )?;
        }
    }

    let _ = rt::handle_close(session_handle);
    Ok(())
}

fn cmd_gfx_focus(bootstrap: rt::Handle, session: rt::Handle, surface_id: u32) -> rt::Result<()> {
    let session_handle = rt::lookup_service(bootstrap, ServiceId::Session)?;
    let focused_surface = rt::session_focus(session_handle, 1, surface_id)?;
    let _ = rt::handle_close(session_handle);
    write_session_linef(
        session,
        format_args!("focused graphical surface {}", focused_surface),
    )
}

fn cmd_desktop<'a, I>(bootstrap: rt::Handle, session: rt::Handle, mut parts: I) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("status") => cmd_desktop_status(bootstrap, session),
        Some("apps") => cmd_desktop_apps(bootstrap, session),
        Some("windows") => cmd_desktop_windows(bootstrap, session),
        Some("launch") => match parts.next().and_then(parse_desktop_app_name) {
            Some(app_id) => cmd_desktop_launch(bootstrap, session, app_id),
            None => write_session_linef(
                session,
                format_args!("usage: desktop launch <settings|files|monitor>"),
            ),
        },
        Some("focus") => match parts.next().and_then(parse_desktop_app_name) {
            Some(app_id) => cmd_desktop_focus(bootstrap, session, app_id),
            None => write_session_linef(
                session,
                format_args!("usage: desktop focus <settings|files|monitor>"),
            ),
        },
        Some("next") => cmd_desktop_next(bootstrap, session),
        Some("close") => match parts.next().and_then(parse_desktop_app_name) {
            Some(app_id) => cmd_desktop_close(bootstrap, session, app_id),
            None => write_session_linef(
                session,
                format_args!("usage: desktop close <settings|files|monitor>"),
            ),
        },
        Some("minimize") => match parts.next().and_then(parse_desktop_app_name) {
            Some(app_id) => cmd_desktop_minimize(bootstrap, session, app_id),
            None => write_session_linef(
                session,
                format_args!("usage: desktop minimize <settings|files|monitor>"),
            ),
        },
        Some("restore") => match parts.next().and_then(parse_desktop_app_name) {
            Some(app_id) => cmd_desktop_restore(bootstrap, session, app_id),
            None => write_session_linef(
                session,
                format_args!("usage: desktop restore <settings|files|monitor>"),
            ),
        },
        Some("maximize") => match parts.next().and_then(parse_desktop_app_name) {
            Some(app_id) => cmd_desktop_maximize(bootstrap, session, app_id),
            None => write_session_linef(
                session,
                format_args!("usage: desktop maximize <settings|files|monitor>"),
            ),
        },
        Some("move") => match (
            parts.next().and_then(parse_desktop_app_name),
            parts.next().and_then(|value| value.parse::<i32>().ok()),
            parts.next().and_then(|value| value.parse::<i32>().ok()),
        ) {
            (Some(app_id), Some(x), Some(y)) => cmd_desktop_move(bootstrap, session, app_id, x, y),
            _ => write_session_linef(
                session,
                format_args!("usage: desktop move <settings|files|monitor> <x> <y>"),
            ),
        },
        Some("resize") => match (
            parts.next().and_then(parse_desktop_app_name),
            parts.next().and_then(|value| value.parse::<u32>().ok()),
            parts.next().and_then(|value| value.parse::<u32>().ok()),
        ) {
            (Some(app_id), Some(width), Some(height)) => {
                cmd_desktop_resize(bootstrap, session, app_id, width, height)
            }
            _ => write_session_linef(
                session,
                format_args!("usage: desktop resize <settings|files|monitor> <width> <height>"),
            ),
        },
        Some("click") => match (
            parts.next().and_then(|value| value.parse::<i32>().ok()),
            parts.next().and_then(|value| value.parse::<i32>().ok()),
        ) {
            (Some(x), Some(y)) => cmd_desktop_click(bootstrap, session, x, y),
            _ => write_session_linef(session, format_args!("usage: desktop click <x> <y>")),
        },
        _ => write_session_linef(
            session,
            format_args!(
                "usage: desktop <status|apps|windows|launch|focus|next|close|minimize|restore|maximize|move|resize|click> ..."
            ),
        ),
    }
}

fn cmd_desktop_status(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let status = rt::desktop_status(desktop_handle)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!(
            "session={} focused-app={} focused-surface={} running-apps={} drag={} pointer=({}, {})",
            status.session_id,
            status
                .focused_app
                .map(desktop_app_name)
                .unwrap_or("none"),
            status.focused_surface,
            status.running_apps,
            desktop_drag_name(status.drag_mode),
            status.pointer_x,
            status.pointer_y,
        ),
    )
}

fn cmd_desktop_apps(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let mut apps = [DesktopAppInfo {
        app_id: DesktopAppId::Settings,
        running: false,
        focused: false,
        surface_id: 0,
    }; MAX_DESKTOP_APPS];
    let count = rt::desktop_list_apps(desktop_handle, &mut apps)?;
    let _ = rt::handle_close(desktop_handle);
    if count == 0 {
        return write_session_linef(session, format_args!("no desktop apps"));
    }
    for app in apps.iter().copied().take(count) {
        write_session_linef(
            session,
            format_args!(
                "{:<10} running={} focused={} surface={}",
                desktop_app_name(app.app_id),
                app.running,
                app.focused,
                app.surface_id,
            ),
        )?;
    }
    Ok(())
}

fn cmd_desktop_windows(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let mut windows = [DesktopWindowInfo {
        app_id: DesktopAppId::Settings,
        surface_id: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        z_order: 0,
        focused: false,
        minimized: false,
        visible: false,
    }; MAX_DESKTOP_WINDOWS];
    let count = rt::desktop_list_windows(desktop_handle, &mut windows)?;
    let _ = rt::handle_close(desktop_handle);
    if count == 0 {
        return write_session_linef(session, format_args!("no desktop windows"));
    }
    for window in windows.iter().copied().take(count) {
        write_session_linef(
            session,
            format_args!(
                "{:<10} surface={} pos=({}, {}) size={}x{} z={} focused={} minimized={} visible={}",
                desktop_app_name(window.app_id),
                window.surface_id,
                window.x,
                window.y,
                window.width,
                window.height,
                window.z_order,
                window.focused,
                window.minimized,
                window.visible,
            ),
        )?;
    }
    Ok(())
}

fn cmd_desktop_launch(
    bootstrap: rt::Handle,
    session: rt::Handle,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_launch_app(desktop_handle, app_id)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!("launched {} on surface {}", desktop_app_name(app_id), surface_id),
    )
}

fn cmd_desktop_focus(
    bootstrap: rt::Handle,
    session: rt::Handle,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_focus_app(desktop_handle, app_id)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!("focused {} on surface {}", desktop_app_name(app_id), surface_id),
    )
}

fn cmd_desktop_next(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_focus_next(desktop_handle)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!("focused next visible window on surface {}", surface_id),
    )
}

fn cmd_desktop_close(
    bootstrap: rt::Handle,
    session: rt::Handle,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    rt::desktop_close_app(desktop_handle, app_id)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!("closed {}", desktop_app_name(app_id)),
    )
}

fn cmd_desktop_minimize(
    bootstrap: rt::Handle,
    session: rt::Handle,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_minimize_app(desktop_handle, app_id)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!("minimized {} on surface {}", desktop_app_name(app_id), surface_id),
    )
}

fn cmd_desktop_restore(
    bootstrap: rt::Handle,
    session: rt::Handle,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_restore_app(desktop_handle, app_id)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!("restored {} on surface {}", desktop_app_name(app_id), surface_id),
    )
}

fn cmd_desktop_maximize(
    bootstrap: rt::Handle,
    session: rt::Handle,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_maximize_app(desktop_handle, app_id)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!("maximized {} on surface {}", desktop_app_name(app_id), surface_id),
    )
}

fn cmd_desktop_move(
    bootstrap: rt::Handle,
    session: rt::Handle,
    app_id: DesktopAppId,
    x: i32,
    y: i32,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_move_app(desktop_handle, app_id, x, y)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!(
            "moved {} to ({}, {}) on surface {}",
            desktop_app_name(app_id),
            x,
            y,
            surface_id,
        ),
    )
}

fn cmd_desktop_resize(
    bootstrap: rt::Handle,
    session: rt::Handle,
    app_id: DesktopAppId,
    width: u32,
    height: u32,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_resize_app(desktop_handle, app_id, width, height)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!(
            "resized {} to {}x{} on surface {}",
            desktop_app_name(app_id),
            width,
            height,
            surface_id,
        ),
    )
}

fn cmd_desktop_click(
    bootstrap: rt::Handle,
    session: rt::Handle,
    x: i32,
    y: i32,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_pointer_click(desktop_handle, x, y)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!("desktop click at ({}, {}) targeted surface {}", x, y, surface_id),
    )
}

fn cmd_pkg<'a, I>(bootstrap: rt::Handle, session: rt::Handle, mut parts: I) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("list") => cmd_pkg_list(bootstrap, session),
        Some("info") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => cmd_pkg_info(bootstrap, session, service_id),
            None => write_session_linef(session, format_args!("usage: pkg info <name>")),
        },
        Some("install") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => cmd_pkg_install(bootstrap, session, service_id, parts.next()),
            None => write_session_linef(session, format_args!("usage: pkg install <name> [version]")),
        },
        Some("update") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => cmd_pkg_update(bootstrap, session, service_id, parts.next()),
            None => write_session_linef(session, format_args!("usage: pkg update <name> [version]")),
        },
        Some("remove") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => cmd_pkg_remove(bootstrap, session, service_id),
            None => write_session_linef(session, format_args!("usage: pkg remove <name>")),
        },
        Some("rollback") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => cmd_pkg_rollback(bootstrap, session, service_id),
            None => write_session_linef(session, format_args!("usage: pkg rollback <name>")),
        },
        Some("history") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => cmd_pkg_history(bootstrap, session, service_id),
            None => write_session_linef(session, format_args!("usage: pkg history <name>")),
        },
        _ => write_session_linef(session, format_args!("usage: pkg <list|info|install|update|remove|rollback|history> ...")),
    }
}

fn cmd_pkg_list(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut installed = [0u8; MAX_VERSION_BYTES];
    let mut active = [0u8; MAX_VERSION_BYTES];
    let mut index = 0usize;

    while let Some(entry) = rt::package_list(package_handle, index, &mut installed, &mut active)? {
        let installed_version =
            core::str::from_utf8(&installed[..entry.installed_version_len]).map_err(|_| rt::Error::InvalidArgument)?;
        let active_version =
            core::str::from_utf8(&active[..entry.active_version_len]).map_err(|_| rt::Error::InvalidArgument)?;
        write_session_linef(
            session,
            format_args!(
                "{:<16} repo={} installed={} active={} rollback={}",
                service_name(entry.service_id),
                entry.repository_versions,
                printable_version(installed_version),
                printable_version(active_version),
                if entry.rollback_available { "yes" } else { "no" },
            ),
        )?;
        index += 1;
    }

    let _ = rt::handle_close(package_handle);
    if index == 0 {
        write_session_linef(session, format_args!("no packages"))
    } else {
        Ok(())
    }
}

fn cmd_pkg_info(bootstrap: rt::Handle, session: rt::Handle, service_id: ServiceId) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut installed = [0u8; MAX_VERSION_BYTES];
    let mut active = [0u8; MAX_VERSION_BYTES];
    let mut rollback = [0u8; MAX_VERSION_BYTES];
    let mut latest = [0u8; MAX_VERSION_BYTES];
    let info = rt::package_info(
        package_handle,
        service_id,
        &mut installed,
        &mut active,
        &mut rollback,
        &mut latest,
    )?;
    let _ = rt::handle_close(package_handle);

    write_session_linef(
        session,
        format_args!(
            "{} repo={} installed={} active={} rollback={} latest={}",
            service_name(service_id),
            info.repository_versions,
            printable_version(core::str::from_utf8(&installed[..info.installed_version_len]).map_err(|_| rt::Error::InvalidArgument)?),
            printable_version(core::str::from_utf8(&active[..info.active_version_len]).map_err(|_| rt::Error::InvalidArgument)?),
            printable_version(core::str::from_utf8(&rollback[..info.rollback_version_len]).map_err(|_| rt::Error::InvalidArgument)?),
            printable_version(core::str::from_utf8(&latest[..info.latest_version_len]).map_err(|_| rt::Error::InvalidArgument)?),
        ),
    )
}

fn cmd_pkg_install(
    bootstrap: rt::Handle,
    session: rt::Handle,
    service_id: ServiceId,
    version: Option<&str>,
) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let result = rt::package_install(package_handle, service_id, version);
    let _ = rt::handle_close(package_handle);
    result?;
    write_session_linef(
        session,
        format_args!("installed {}", service_name(service_id)),
    )
}

fn cmd_pkg_update(
    bootstrap: rt::Handle,
    session: rt::Handle,
    service_id: ServiceId,
    version: Option<&str>,
) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let result = rt::package_update(package_handle, service_id, version);
    let _ = rt::handle_close(package_handle);
    result?;
    write_session_linef(session, format_args!("updated {}", service_name(service_id)))
}

fn cmd_pkg_remove(bootstrap: rt::Handle, session: rt::Handle, service_id: ServiceId) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let result = rt::package_remove(package_handle, service_id);
    let _ = rt::handle_close(package_handle);
    result?;
    write_session_linef(session, format_args!("removed {}", service_name(service_id)))
}

fn cmd_pkg_rollback(
    bootstrap: rt::Handle,
    session: rt::Handle,
    service_id: ServiceId,
) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let result = rt::package_rollback(package_handle, service_id);
    let _ = rt::handle_close(package_handle);
    result?;
    write_session_linef(session, format_args!("rolled back {}", service_name(service_id)))
}

fn cmd_pkg_history(
    bootstrap: rt::Handle,
    session: rt::Handle,
    service_id: ServiceId,
) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut current = [0u8; MAX_VERSION_BYTES];
    let mut previous = [0u8; MAX_VERSION_BYTES];
    let (current_len, previous_len) =
        rt::package_history(package_handle, service_id, &mut current, &mut previous)?;
    let _ = rt::handle_close(package_handle);
    write_session_linef(
        session,
        format_args!(
            "{} current={} rollback={}",
            service_name(service_id),
            printable_version(core::str::from_utf8(&current[..current_len]).map_err(|_| rt::Error::InvalidArgument)?),
            printable_version(core::str::from_utf8(&previous[..previous_len]).map_err(|_| rt::Error::InvalidArgument)?),
        ),
    )
}

fn emit_shell_log(
    bootstrap: rt::Handle,
    severity: LogSeverity,
    event: LogEvent,
    arg0: u64,
    arg1: u64,
) -> rt::Result<()> {
    let log_handle = rt::lookup_service(bootstrap, ServiceId::Log)?;
    let result = rt::send_log_record(
        log_handle,
        ServiceId::Shell,
        severity,
        LogDomain::Shell,
        event,
        arg0,
        arg1,
    );
    let _ = rt::handle_close(log_handle);
    result
}

fn write_session_linef(session: rt::Handle, args: core::fmt::Arguments<'_>) -> rt::Result<()> {
    let mut buffer = FixedLogBuffer::<256>::new();
    let _ = buffer.write_fmt(args);
    let _ = buffer.write_str("\r\n");
    let text = core::str::from_utf8(buffer.as_bytes()).map_err(|_| rt::Error::InvalidArgument)?;
    rt::console_session_write(session, text)
}

fn write_session_text(session: rt::Handle, text: &str) -> rt::Result<()> {
    let bytes = text.as_bytes();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let end = (offset + MAX_SESSION_WRITE_BYTES).min(bytes.len());
        let chunk = core::str::from_utf8(&bytes[offset..end]).map_err(|_| rt::Error::InvalidArgument)?;
        rt::console_session_write(session, chunk)?;
        offset = end;
    }
    Ok(())
}

fn parse_service_name(name: &str) -> Option<ServiceId> {
    match name {
        "root-manager" => Some(ServiceId::RootManager),
        "storage" | "storage-service" => Some(ServiceId::Storage),
        "console" | "console-service" => Some(ServiceId::Console),
        "config" | "config-service" => Some(ServiceId::Config),
        "log" | "log-service" => Some(ServiceId::Log),
        "status" | "status-service" => Some(ServiceId::Status),
        "shell" | "shell-service" => Some(ServiceId::Shell),
        "package" | "package-service" => Some(ServiceId::Package),
        "announce" | "announce-service" => Some(ServiceId::Announce),
        "network" | "network-service" => Some(ServiceId::Network),
        "graphics" | "graphics-service" => Some(ServiceId::Graphics),
        "session" | "session-service" => Some(ServiceId::Session),
        "desktop" | "desktop-shell" | "desktop-shell-service" => Some(ServiceId::DesktopShell),
        _ => None,
    }
}

fn parse_desktop_app_name(name: &str) -> Option<DesktopAppId> {
    match name {
        "settings" => Some(DesktopAppId::Settings),
        "files" => Some(DesktopAppId::Files),
        "monitor" => Some(DesktopAppId::Monitor),
        _ => None,
    }
}

fn service_name(service_id: ServiceId) -> &'static str {
    match service_id {
        ServiceId::RootManager => "root-manager",
        ServiceId::Storage => "storage-service",
        ServiceId::Console => "console-service",
        ServiceId::Config => "config-service",
        ServiceId::Log => "log-service",
        ServiceId::Status => "status-service",
        ServiceId::Shell => "shell-service",
        ServiceId::Package => "package-service",
        ServiceId::Announce => "announce-service",
        ServiceId::Network => "network-service",
        ServiceId::Graphics => "graphics-service",
        ServiceId::Session => "session-service",
        ServiceId::DesktopShell => "desktop-shell-service",
    }
}

fn desktop_app_name(app_id: DesktopAppId) -> &'static str {
    match app_id {
        DesktopAppId::Settings => "settings",
        DesktopAppId::Files => "files",
        DesktopAppId::Monitor => "monitor",
    }
}

fn desktop_drag_name(mode: DesktopDragMode) -> &'static str {
    match mode {
        DesktopDragMode::None => "none",
        DesktopDragMode::Move => "move",
        DesktopDragMode::Resize => "resize",
    }
}

fn phase_name(phase: ManagerServicePhase) -> &'static str {
    match phase {
        ManagerServicePhase::Dormant => "dormant",
        ManagerServicePhase::Starting => "starting",
        ManagerServicePhase::Ready => "ready",
        ManagerServicePhase::Exited => "exited",
    }
}

fn manager_status_name(status: rt::ManagerStatus) -> &'static str {
    match status {
        rt::ManagerStatus::Ok => "ok",
        rt::ManagerStatus::Denied => "denied",
        rt::ManagerStatus::NotFound => "not-found",
        rt::ManagerStatus::Busy => "busy",
        rt::ManagerStatus::Failed => "failed",
    }
}

fn config_key_name(key: ConfigKey) -> &'static str {
    match key {
        ConfigKey::LogMinimumSeverity => "log.minimum_severity",
        ConfigKey::StatusHeartbeatTicks => "status.heartbeat_ticks",
        ConfigKey::StatusConsoleMirror => "status.console_mirror",
        ConfigKey::StatusHeartbeatLogPeriod => "status.heartbeat_log_period",
        ConfigKey::NetworkIpv4Address => "network.ipv4_address",
        ConfigKey::NetworkIpv4PrefixLength => "network.ipv4_prefix_length",
        ConfigKey::NetworkIpv4Gateway => "network.ipv4_gateway",
        ConfigKey::NetworkProbeTimeoutTicks => "network.probe_timeout_ticks",
    }
}

fn severity_name(severity: LogSeverity) -> &'static str {
    match severity {
        LogSeverity::Trace => "trace",
        LogSeverity::Debug => "debug",
        LogSeverity::Info => "info",
        LogSeverity::Warn => "warn",
        LogSeverity::Error => "error",
    }
}

fn domain_name(domain: LogDomain) -> &'static str {
    match domain {
        LogDomain::Bootstrap => "bootstrap",
        LogDomain::ServiceManager => "service-manager",
        LogDomain::Service => "service",
        LogDomain::Storage => "storage",
        LogDomain::Log => "log",
        LogDomain::Config => "config",
        LogDomain::Console => "console",
        LogDomain::Status => "status",
        LogDomain::Ipc => "ipc",
        LogDomain::Shell => "shell",
        LogDomain::Package => "package",
        LogDomain::Network => "network",
        LogDomain::Graphics => "graphics",
        LogDomain::Session => "session",
        LogDomain::Desktop => "desktop",
        LogDomain::App => "app",
    }
}

fn event_name(event: LogEvent) -> &'static str {
    match event {
        LogEvent::ServiceStarted => "service-started",
        LogEvent::ServiceReady => "service-ready",
        LogEvent::ServiceFailed => "service-failed",
        LogEvent::ServiceRestarting => "service-restarting",
        LogEvent::ConfigLoaded => "config-loaded",
        LogEvent::ConfigRead => "config-read",
        LogEvent::ConsoleWrite => "console-write",
        LogEvent::StatusStarted => "status-started",
        LogEvent::StatusHeartbeat => "status-heartbeat",
        LogEvent::LookupGranted => "lookup-granted",
        LogEvent::StorageMounted => "storage-mounted",
        LogEvent::ManifestLoaded => "manifest-loaded",
        LogEvent::ResourceOpened => "resource-opened",
        LogEvent::SessionOpened => "session-opened",
        LogEvent::ShellCommand => "shell-command",
        LogEvent::ToolLaunched => "tool-launched",
        LogEvent::PackageCatalogLoaded => "package-catalog-loaded",
        LogEvent::PackageInstalled => "package-installed",
        LogEvent::PackageUpdated => "package-updated",
        LogEvent::PackageRemoved => "package-removed",
        LogEvent::PackageRolledBack => "package-rolled-back",
        LogEvent::PackageActivationFailed => "package-activation-failed",
        LogEvent::NetworkInterfaceReady => "network-interface-ready",
        LogEvent::NetworkAddressConfigured => "network-address-configured",
        LogEvent::NetworkResolveCompleted => "network-resolve-completed",
        LogEvent::NetworkProbeCompleted => "network-probe-completed",
        LogEvent::NetworkLinkChanged => "network-link-changed",
        LogEvent::DisplayOutputReady => "display-output-ready",
        LogEvent::SurfaceCreated => "surface-created",
        LogEvent::SurfaceUpdated => "surface-updated",
        LogEvent::CompositorPresented => "compositor-presented",
        LogEvent::SessionReady => "session-ready",
        LogEvent::SessionFocusChanged => "session-focus-changed",
        LogEvent::DesktopReady => "desktop-ready",
        LogEvent::DesktopAppLaunched => "desktop-app-launched",
        LogEvent::DesktopAppExited => "desktop-app-exited",
        LogEvent::DesktopFocusChanged => "desktop-focus-changed",
        LogEvent::AppRendered => "app-rendered",
        LogEvent::InputSourceReady => "input-source-ready",
        LogEvent::InputKeyDelivered => "input-key-delivered",
    }
}

fn write_log_record(session: rt::Handle, record: rt::LogRecord) -> rt::Result<()> {
    match record.event {
        LogEvent::ConfigLoaded => write_session_linef(
            session,
            format_args!(
                "#{} {} {} {}/{} minimum-severity={}",
                record.sequence,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                record.arg0,
            ),
        ),
        LogEvent::NetworkInterfaceReady => write_session_linef(
            session,
            format_args!(
                "#{} {} {} {}/{} iface={} mac={}",
                record.sequence,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                record.arg0,
                format_mac(unpack_mac(record.arg1)),
            ),
        ),
        LogEvent::NetworkAddressConfigured => write_session_linef(
            session,
            format_args!(
                "#{} {} {} {}/{} addr={} gateway={}",
                record.sequence,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                format_ipv4(record.arg0 as u32),
                format_ipv4(record.arg1 as u32),
            ),
        ),
        LogEvent::NetworkResolveCompleted => write_session_linef(
            session,
            format_args!(
                "#{} {} {} {}/{} addr={} count={}",
                record.sequence,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                format_ipv4(record.arg0 as u32),
                record.arg1,
            ),
        ),
        LogEvent::NetworkProbeCompleted => write_session_linef(
            session,
            format_args!(
                "#{} {} {} {}/{} addr={} elapsed-ms={}",
                record.sequence,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                format_ipv4(record.arg0 as u32),
                record.arg1,
            ),
        ),
        LogEvent::DisplayOutputReady => write_session_linef(
            session,
            format_args!(
                "#{} {} {} {}/{} {}x{}",
                record.sequence,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                record.arg0,
                record.arg1,
            ),
        ),
        LogEvent::SurfaceCreated | LogEvent::SessionReady | LogEvent::SessionFocusChanged => {
            write_session_linef(
                session,
                format_args!(
                    "#{} {} {} {}/{} {} {}",
                    record.sequence,
                    severity_name(record.severity),
                    service_name(record.source),
                    domain_name(record.domain),
                    event_name(record.event),
                    record.arg0,
                    record.arg1,
                ),
            )
        }
        _ => write_session_linef(
            session,
            format_args!(
                "#{} {} {} {}/{} {} {}",
                record.sequence,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                record.arg0,
                record.arg1,
            ),
        ),
    }
}

fn config_value_text(key: ConfigKey, value: u64) -> FixedValueText {
    match key {
        ConfigKey::NetworkIpv4Address | ConfigKey::NetworkIpv4Gateway => {
            FixedValueText::ipv4(value as u32)
        }
        _ => FixedValueText::unsigned(value),
    }
}

fn link_state_name(state: rt::PacketInterfaceLinkState) -> &'static str {
    match state {
        rt::PacketInterfaceLinkState::Up => "up",
        rt::PacketInterfaceLinkState::Down => "down",
    }
}

fn format_ipv4(value: u32) -> FixedValueText {
    FixedValueText::ipv4(value)
}

fn format_mac(value: [u8; 6]) -> FixedValueText {
    FixedValueText::mac(value)
}

fn display_backend_name(backend: rt::DisplayOutputBackend) -> &'static str {
    match backend {
        rt::DisplayOutputBackend::BootFramebuffer => "boot-framebuffer",
        rt::DisplayOutputBackend::Unknown => "unknown",
    }
}

fn display_state_name(state: rt::DisplayOutputState) -> &'static str {
    match state {
        rt::DisplayOutputState::Connected => "connected",
        rt::DisplayOutputState::Disconnected => "disconnected",
    }
}

fn pixel_format_name(format: rt::DisplayPixelFormat) -> &'static str {
    match format {
        rt::DisplayPixelFormat::Xrgb8888 => "xrgb8888",
        rt::DisplayPixelFormat::Bgrx8888 => "bgrx8888",
        rt::DisplayPixelFormat::Unknown => "unknown",
    }
}

fn session_input_source_name(source: rt::SessionInputSource) -> &'static str {
    match source {
        rt::SessionInputSource::ServiceControl => "service-control",
        rt::SessionInputSource::Hardware => "hardware",
        rt::SessionInputSource::None => "none",
    }
}

fn unpack_mac(value: u64) -> [u8; 6] {
    [
        (value & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        ((value >> 16) & 0xff) as u8,
        ((value >> 24) & 0xff) as u8,
        ((value >> 32) & 0xff) as u8,
        ((value >> 40) & 0xff) as u8,
    ]
}

fn error_name(error: rt::Error) -> &'static str {
    match error {
        rt::Error::Unsupported => "unsupported",
        rt::Error::InvalidCall => "invalid-call",
        rt::Error::PermissionDenied => "permission-denied",
        rt::Error::NotInitialized => "not-initialized",
        rt::Error::InvalidArgument => "invalid-argument",
        rt::Error::BufferTooSmall => "buffer-too-small",
        rt::Error::QueueEmpty => "timeout",
        rt::Error::NotFound => "not-found",
        rt::Error::Busy => "busy",
        rt::Error::CapacityExceeded => "capacity-exceeded",
        rt::Error::Unknown(_) => "unknown",
    }
}

struct FixedValueText {
    bytes: [u8; 32],
    len: usize,
}

impl FixedValueText {
    fn unsigned(value: u64) -> Self {
        let mut text = Self {
            bytes: [0; 32],
            len: 0,
        };
        let _ = write!(&mut text, "{value}");
        text
    }

    fn ipv4(value: u32) -> Self {
        let mut text = Self {
            bytes: [0; 32],
            len: 0,
        };
        let _ = write!(
            &mut text,
            "{}.{}.{}.{}",
            (value >> 24) & 0xff,
            (value >> 16) & 0xff,
            (value >> 8) & 0xff,
            value & 0xff,
        );
        text
    }

    fn mac(value: [u8; 6]) -> Self {
        let mut text = Self {
            bytes: [0; 32],
            len: 0,
        };
        let _ = write!(
            &mut text,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            value[0], value[1], value[2], value[3], value[4], value[5],
        );
        text
    }
}

impl core::fmt::Display for FixedValueText {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let text = core::str::from_utf8(&self.bytes[..self.len]).map_err(|_| core::fmt::Error)?;
        f.write_str(text)
    }
}

impl Write for FixedValueText {
    fn write_str(&mut self, value: &str) -> core::fmt::Result {
        let bytes = value.as_bytes();
        let remaining = self.bytes.len().saturating_sub(self.len);
        let copy_len = remaining.min(bytes.len());
        self.bytes[self.len..self.len + copy_len].copy_from_slice(&bytes[..copy_len]);
        self.len += copy_len;
        Ok(())
    }
}

fn printable_version(value: &str) -> &str {
    if value.is_empty() {
        "-"
    } else {
        value
    }
}

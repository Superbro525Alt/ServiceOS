#![no_std]
#![no_main]

use core::fmt::Write;

use serviceos_userspace_runtime as rt;
use rt::{
    ConfigKey, FixedLogBuffer, LogDomain, LogEvent, LogSeverity, ManagerServiceInfo,
    ManagerServicePhase, RawMessage, ServiceId, ServiceImageId,
};

const MAX_LINE_BYTES: usize = 128;
const MAX_LISTED_SERVICES: usize = 12;
const MAX_STORAGE_PATH: usize = 96;
const MAX_CAT_CHUNK: usize = 96;
const MAX_VERSION_BYTES: usize = 24;

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

    let _ = emit_shell_log(bootstrap, LogEvent::SessionOpened, 1, 0);
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

        let _ = emit_shell_log(bootstrap, LogEvent::ShellCommand, line.len() as u64, 0);
        if execute_command(bootstrap, session_handle, line).is_err() {
            let _ = write_session_linef(session_handle, format_args!("command failed"));
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
        "pkg" => cmd_pkg(bootstrap, session, parts),
        "run" => match parts.next() {
            Some("sysinfo") => cmd_run_sysinfo(bootstrap, session),
            _ => write_session_linef(session, format_args!("usage: run sysinfo")),
        },
        _ => write_session_linef(session, format_args!("unknown command: {command}")),
    }
}

fn print_help(session: rt::Handle) -> rt::Result<()> {
    write_session_linef(session, format_args!("help: show this command list"))?;
    write_session_linef(session, format_args!("services: list managed services"))?;
    write_session_linef(session, format_args!("service <name>: show one service state"))?;
    write_session_linef(session, format_args!("restart <name>: request a service restart"))?;
    write_session_linef(session, format_args!("logs [count]: show recent structured logs"))?;
    write_session_linef(session, format_args!("config: show core configuration values"))?;
    write_session_linef(session, format_args!("store ls [prefix]: list boot-store paths"))?;
    write_session_linef(session, format_args!("cat <path>: print a text resource"))?;
    write_session_linef(session, format_args!("status: show system heartbeat status"))?;
    write_session_linef(session, format_args!("pkg list: list repository packages"))?;
    write_session_linef(session, format_args!("pkg info <name>: inspect one package"))?;
    write_session_linef(session, format_args!("pkg install <name> [version]: activate a package"))?;
    write_session_linef(session, format_args!("pkg update <name> [version]: switch to a newer package version"))?;
    write_session_linef(session, format_args!("pkg remove <name>: deactivate a package"))?;
    write_session_linef(session, format_args!("pkg rollback <name>: restore the prior active version"))?;
    write_session_linef(session, format_args!("pkg history <name>: show current and rollback versions"))?;
    write_session_linef(session, format_args!("run sysinfo: launch a transient tool"))?;
    Ok(())
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
            )?;
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
    ] {
        let (_, value) = rt::config_read(config_handle, key)?;
        write_session_linef(
            session,
            format_args!("{} = {}", config_key_name(key), value),
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
    event: LogEvent,
    arg0: u64,
    arg1: u64,
) -> rt::Result<()> {
    let log_handle = rt::lookup_service(bootstrap, ServiceId::Log)?;
    let result = rt::send_log_record(
        log_handle,
        ServiceId::Shell,
        LogSeverity::Info,
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
    }
}

fn printable_version(value: &str) -> &str {
    if value.is_empty() {
        "-"
    } else {
        value
    }
}

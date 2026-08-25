use core::fmt::Write;

use rt::{
    ConfigKey, FixedLogBuffer, ManagerLookupPolicy, ManagerServiceInfo, ManagerServicePhase,
    ServiceId, ServiceImageId, StorageEntryKind, StorageMountKind,
};
use serviceos_userspace_runtime as rt;

use crate::util::{
    MAX_CAT_CHUNK, MAX_LISTED_SERVICES, MAX_STORAGE_PATH, MAX_VERSION_BYTES, ShellOutput,
    availability_name, config_key_name, config_value_text, manager_status_name, phase_name,
    printable_version, service_name, shell_output_write, startup_name, write_log_record,
    write_output_linef,
};

const MAX_SERVICE_LOOKUPS: usize = 8;
const MAX_STATUS_SERVICES: usize = 24;

pub(crate) fn cmd_services(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let graph = rt::manager_graph_status(bootstrap)?;
    write_output_linef(
        output,
        format_args!(
            "graph degraded={} blocked={} degraded-services={} total={}",
            graph.degraded_boot,
            graph.blocked_services,
            graph.degraded_services,
            graph.service_count,
        ),
    )?;
    let mut services = [ManagerServiceInfo {
        service_id: ServiceId::RootManager,
        phase: ManagerServicePhase::Dormant,
        attempts: 0,
    }; MAX_LISTED_SERVICES];
    let count = rt::manager_list_services(bootstrap, &mut services)?;
    for info in services[..count].iter().copied() {
        write_output_linef(
            output,
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

pub(crate) fn cmd_service(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
) -> rt::Result<()> {
    let info = rt::manager_service_status(bootstrap, service_id)?;
    let template = rt::manager_service_template(bootstrap, service_id)?;
    write_output_linef(
        output,
        format_args!(
            "{} status={} phase={} startup={} availability={} attempts={} last-exit={:#x}",
            service_name(service_id),
            manager_status_name(info.status),
            phase_name(info.phase),
            startup_name(info.startup),
            availability_name(info.availability),
            info.attempts,
            info.last_exit,
        ),
    )?;
    write_output_linef(
        output,
        format_args!(
            "blocked-on={} last-start={} last-ready={} next-restart={} ready-timeout={} restart-limit={} restart-backoff={} grants={} lookups={}",
            service_name(info.blocked_dependency),
            info.last_start_tick,
            info.last_ready_tick,
            info.next_restart_tick,
            template.ready_timeout_ticks,
            template.restart_limit,
            template.restart_backoff_ticks,
            template.grant_count,
            template.lookup_count,
        ),
    )
}

pub(crate) fn cmd_service_caps(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
) -> rt::Result<()> {
    let template = rt::manager_service_template(bootstrap, service_id)?;
    write_output_linef(
        output,
        format_args!(
            "{} grants={} lookups={}",
            service_name(service_id),
            template.grant_count,
            template.lookup_count,
        ),
    )?;
    if template.lookup_count == 0 {
        return write_output_linef(output, format_args!("no delegated lookups"));
    }
    let mut lookups = [rt::ManagerServiceLookupInfo {
        target: ServiceId::RootManager,
        rights: 0,
        policy: ManagerLookupPolicy::Default,
    }; MAX_SERVICE_LOOKUPS];
    let count = rt::manager_service_lookups(bootstrap, service_id, &mut lookups)?;
    for lookup in lookups[..count].iter().copied() {
        write_output_linef(
            output,
            format_args!(
                "lookup {:<16} rights={} policy={}",
                service_name(lookup.target),
                format_rights(lookup.rights).as_str(),
                lookup_policy_name(lookup.policy),
            ),
        )?;
    }
    Ok(())
}

pub(crate) fn cmd_service_revoke_lookup(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
    target: ServiceId,
    revoked: bool,
) -> rt::Result<()> {
    rt::manager_set_service_lookup_policy(
        bootstrap,
        service_id,
        target,
        if revoked {
            ManagerLookupPolicy::Revoked
        } else {
            ManagerLookupPolicy::Default
        },
    )?;
    write_output_linef(
        output,
        format_args!(
            "{} -> {} policy={}",
            service_name(service_id),
            service_name(target),
            if revoked { "revoked" } else { "default" }
        ),
    )
}

pub(crate) fn cmd_restart(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
) -> rt::Result<()> {
    rt::manager_restart_service(bootstrap, service_id)?;
    write_output_linef(
        output,
        format_args!("restart requested for {}", service_name(service_id)),
    )
}

pub(crate) fn cmd_logs(bootstrap: rt::Handle, output: ShellOutput, count: usize) -> rt::Result<()> {
    const MAX_LOG_QUERY_COUNT: usize = 32;
    let log_handle = rt::lookup_service(bootstrap, ServiceId::Log)?;
    let (oldest, next) = rt::log_query_info(log_handle)?;
    if next == 0 || oldest == next {
        let _ = rt::handle_close(log_handle);
        return write_output_linef(output, format_args!("no log records"));
    }

    let requested = count.max(1);
    let limited = requested.min(MAX_LOG_QUERY_COUNT);
    if limited != requested {
        write_output_linef(
            output,
            format_args!(
                "showing latest {} records (requested {})",
                limited, requested
            ),
        )?;
    }

    let start = oldest.max(next.saturating_sub(limited as u64));
    for (index, sequence) in (start..next).enumerate() {
        if let Some(record) = rt::log_query_record(log_handle, sequence)? {
            write_log_record(output, record)?;
        }
        if (index + 1) % 4 == 0 {
            rt::yield_current()?;
        }
    }

    let _ = rt::handle_close(log_handle);
    Ok(())
}

pub(crate) fn cmd_logs_stream(
    bootstrap: rt::Handle,
    output: ShellOutput,
    count: usize,
) -> rt::Result<()> {
    let log_handle = rt::lookup_service(bootstrap, ServiceId::Log)?;
    let subscription = rt::log_subscribe(log_handle, rt::LogSeverity::Trace, None, None)?;
    let _ = rt::handle_close(log_handle);
    for _ in 0..count {
        let record = rt::log_receive_record(subscription)?;
        write_log_record(output, record)?;
    }
    let _ = rt::handle_close(subscription);
    Ok(())
}

pub(crate) fn cmd_config(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
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
        ConfigKey::NetworkDynamicIpv4,
        ConfigKey::NetworkDnsServer,
        ConfigKey::NetworkDnsQueryTimeoutTicks,
        ConfigKey::NetworkDhcpAcquireTimeoutTicks,
        ConfigKey::NetworkTcpConnectTimeoutTicks,
        ConfigKey::NetworkTcpIdleTimeoutTicks,
    ] {
        let (_, value) = rt::config_read(config_handle, key)?;
        write_output_linef(
            output,
            format_args!(
                "{} = {}",
                config_key_name(key),
                config_value_text(key, value)
            ),
        )?;
    }
    let _ = rt::handle_close(config_handle);
    Ok(())
}

pub(crate) fn cmd_config_get(
    bootstrap: rt::Handle,
    output: ShellOutput,
    key_name: &str,
) -> rt::Result<()> {
    let Some(key) = parse_config_key(key_name) else {
        return write_output_linef(output, format_args!("unknown config key: {}", key_name));
    };
    let config_handle = rt::lookup_service(bootstrap, ServiceId::Config)?;
    let (_, value) = rt::config_read(config_handle, key)?;
    let _ = rt::handle_close(config_handle);
    write_output_linef(
        output,
        format_args!(
            "{} = {}",
            config_key_name(key),
            config_value_text(key, value)
        ),
    )
}

pub(crate) fn cmd_config_set(
    bootstrap: rt::Handle,
    output: ShellOutput,
    key_name: &str,
    value: u64,
) -> rt::Result<()> {
    let Some(key) = parse_config_key(key_name) else {
        return write_output_linef(output, format_args!("unknown config key: {}", key_name));
    };
    let config_handle = rt::lookup_service(bootstrap, ServiceId::Config)?;
    rt::config_write(config_handle, key, value)?;
    let _ = rt::handle_close(config_handle);
    write_output_linef(
        output,
        format_args!(
            "updated {} = {}",
            config_key_name(key),
            config_value_text(key, value)
        ),
    )
}

pub(crate) fn cmd_store_ls(
    bootstrap: rt::Handle,
    output: ShellOutput,
    prefix: &str,
) -> rt::Result<()> {
    let storage_handle = rt::lookup_service(bootstrap, ServiceId::Storage)?;
    let namespace_root = rt::storage_open_directory(storage_handle, "", false)?;
    let _ = rt::handle_close(storage_handle);
    let listing_handle = if prefix.is_empty() {
        namespace_root
    } else {
        match rt::storage_directory_open_path(namespace_root, prefix.trim_matches('/'), false) {
            Ok(handle) => {
                let _ = rt::handle_close(namespace_root);
                handle
            }
            Err(error) => {
                let _ = rt::handle_close(namespace_root);
                return Err(error);
            }
        }
    };

    let mut path_buffer = [0u8; MAX_STORAGE_PATH];
    let mut cursor = 0usize;
    while let Some((next_cursor, _, path_len)) =
        rt::storage_directory_read(listing_handle, cursor, &mut path_buffer)?
    {
        let path = core::str::from_utf8(&path_buffer[..path_len])
            .map_err(|_| rt::Error::InvalidArgument)?;
        write_output_linef(output, format_args!("{path}"))?;
        cursor = next_cursor;
    }
    let _ = rt::handle_close(listing_handle);
    if cursor == 0 {
        write_output_linef(output, format_args!("no entries"))
    } else {
        Ok(())
    }
}

pub(crate) fn cmd_store_mounts(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let storage_handle = rt::lookup_service(bootstrap, ServiceId::Storage)?;
    let mut path_buffer = [0u8; MAX_STORAGE_PATH];
    let mut cursor = 0usize;
    let mut listed = 0usize;
    while let Some(mount) = rt::storage_mount_list(storage_handle, cursor, &mut path_buffer)? {
        let path = core::str::from_utf8(&path_buffer[..mount.path_len])
            .map_err(|_| rt::Error::InvalidArgument)?;
        let kind = match mount.kind {
            StorageMountKind::Boot => "boot",
            StorageMountKind::Persistent => "persistent",
            StorageMountKind::Ephemeral => "ephemeral",
            StorageMountKind::Temp => "temp",
        };
        let mount_path = if path.is_empty() { "/" } else { path };
        write_output_linef(
            output,
            format_args!(
                "{} kind={} writable={} persistent={}",
                mount_path, kind, mount.writable, mount.persistent
            ),
        )?;
        cursor = mount.next_cursor;
        listed += 1;
    }
    let _ = rt::handle_close(storage_handle);
    if listed == 0 {
        write_output_linef(output, format_args!("no mounts"))
    } else {
        Ok(())
    }
}

pub(crate) fn cmd_store_mkdir(
    bootstrap: rt::Handle,
    output: ShellOutput,
    path: &str,
) -> rt::Result<()> {
    let storage_handle = rt::lookup_service(bootstrap, ServiceId::Storage)?;
    let directory_handle = rt::storage_open_directory(storage_handle, "", true)?;
    let _ = rt::handle_close(storage_handle);
    let mut parent = FixedLogBuffer::<96>::new();
    let name = split_parent_path(path, &mut parent)?;
    let parent_handle = open_parent_directory(directory_handle, parent.as_str(), true)?;
    let _ = rt::handle_close(directory_handle);
    let result = rt::storage_directory_create(parent_handle, name, StorageEntryKind::Directory);
    let _ = rt::handle_close(parent_handle);
    result?;
    write_output_linef(
        output,
        format_args!("created directory {}", path.trim_end_matches('/')),
    )
}

pub(crate) fn cmd_store_write<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    path: &str,
    parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    let storage_handle = rt::lookup_service(bootstrap, ServiceId::Storage)?;
    let directory_handle = rt::storage_open_directory(storage_handle, "", true)?;
    let _ = rt::handle_close(storage_handle);
    let mut parent = FixedLogBuffer::<96>::new();
    let name = split_parent_path(path, &mut parent)?;
    let parent_handle = open_parent_directory(directory_handle, parent.as_str(), true)?;
    let _ = rt::handle_close(directory_handle);

    let mut content = FixedLogBuffer::<128>::new();
    let mut wrote_any = false;
    for part in parts {
        if wrote_any {
            let _ = content.write_str(" ");
        }
        let _ = content.write_str(part);
        wrote_any = true;
    }
    let bytes = content.as_bytes();
    if bytes.is_empty() {
        let _ = rt::handle_close(parent_handle);
        return Err(rt::Error::InvalidArgument);
    }

    let (file_handle, _) = rt::storage_directory_open_file(parent_handle, name, true, true)?;
    let _ = rt::handle_close(parent_handle);
    let result = rt::storage_write(file_handle, 0, bytes.len(), bytes);
    let _ = rt::storage_blob_close(file_handle);
    let _ = result?;
    write_output_linef(
        output,
        format_args!("wrote {} bytes to {}", bytes.len(), path),
    )
}

pub(crate) fn cmd_store_rm(
    bootstrap: rt::Handle,
    output: ShellOutput,
    path: &str,
) -> rt::Result<()> {
    let storage_handle = rt::lookup_service(bootstrap, ServiceId::Storage)?;
    let directory_handle = rt::storage_open_directory(storage_handle, "", true)?;
    let _ = rt::handle_close(storage_handle);
    let mut parent = FixedLogBuffer::<96>::new();
    let name = split_parent_path(path, &mut parent)?;
    let parent_handle = open_parent_directory(directory_handle, parent.as_str(), true)?;
    let _ = rt::handle_close(directory_handle);
    let result = rt::storage_directory_remove(parent_handle, name);
    let _ = rt::handle_close(parent_handle);
    result?;
    write_output_linef(
        output,
        format_args!("removed {}", path.trim_end_matches('/')),
    )
}

pub(crate) fn cmd_cat(bootstrap: rt::Handle, output: ShellOutput, path: &str) -> rt::Result<()> {
    let storage_handle = rt::lookup_service(bootstrap, ServiceId::Storage)?;
    let directory_handle = rt::storage_open_directory(storage_handle, "", false)?;
    let _ = rt::handle_close(storage_handle);
    let (blob_handle, blob_len) =
        rt::storage_directory_open_path_file(directory_handle, path.trim_matches('/'), false)?;
    let _ = rt::handle_close(directory_handle);

    let mut offset = 0usize;
    let mut buffer = [0u8; MAX_CAT_CHUNK];
    while offset < blob_len {
        let read = rt::storage_read(blob_handle, offset, &mut buffer)?;
        if read == 0 {
            break;
        }
        let text = core::str::from_utf8(&buffer[..read]).map_err(|_| rt::Error::InvalidArgument)?;
        shell_output_write(output, text)?;
        offset += read;
    }
    let _ = rt::storage_blob_close(blob_handle);
    shell_output_write(output, "\r\n")?;
    Ok(())
}

fn open_parent_directory(
    root_directory: rt::Handle,
    parent: &str,
    writable: bool,
) -> rt::Result<rt::Handle> {
    if parent.is_empty() {
        return rt::handle_duplicate(
            root_directory,
            rt::rights::SEND | rt::rights::RECEIVE | rt::rights::DUPLICATE | rt::rights::TRANSFER,
        );
    }
    rt::storage_directory_open_path(root_directory, parent.trim_matches('/'), writable)
}

fn split_parent_path<'a>(
    path: &'a str,
    parent_buffer: &mut FixedLogBuffer<96>,
) -> rt::Result<&'a str> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return Err(rt::Error::InvalidArgument);
    }
    match trimmed.rsplit_once('/') {
        Some((parent, name)) if !name.is_empty() => {
            let _ = parent_buffer.write_str(parent);
            let _ = parent_buffer.write_str("/");
            Ok(name)
        }
        Some(_) => Err(rt::Error::InvalidArgument),
        None => Ok(trimmed),
    }
}

fn parse_config_key(value: &str) -> Option<ConfigKey> {
    match value {
        "log.minimum_severity" => Some(ConfigKey::LogMinimumSeverity),
        "status.heartbeat_ticks" => Some(ConfigKey::StatusHeartbeatTicks),
        "status.console_mirror" => Some(ConfigKey::StatusConsoleMirror),
        "status.heartbeat_log_period" => Some(ConfigKey::StatusHeartbeatLogPeriod),
        "network.ipv4_address" => Some(ConfigKey::NetworkIpv4Address),
        "network.ipv4_prefix_length" => Some(ConfigKey::NetworkIpv4PrefixLength),
        "network.ipv4_gateway" => Some(ConfigKey::NetworkIpv4Gateway),
        "network.probe_timeout_ticks" => Some(ConfigKey::NetworkProbeTimeoutTicks),
        "network.dynamic_ipv4" => Some(ConfigKey::NetworkDynamicIpv4),
        "network.dns_server" => Some(ConfigKey::NetworkDnsServer),
        "network.dns_query_timeout_ticks" => Some(ConfigKey::NetworkDnsQueryTimeoutTicks),
        "network.dhcp_acquire_timeout_ticks" => Some(ConfigKey::NetworkDhcpAcquireTimeoutTicks),
        "network.tcp_connect_timeout_ticks" => Some(ConfigKey::NetworkTcpConnectTimeoutTicks),
        "network.tcp_idle_timeout_ticks" => Some(ConfigKey::NetworkTcpIdleTimeoutTicks),
        _ => None,
    }
}

pub(crate) fn cmd_status_snapshot(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let status_handle = rt::lookup_service(bootstrap, ServiceId::Status)?;
    let (heartbeats, last_tick, tracked_services) = rt::status_snapshot(status_handle)?;
    let _ = rt::handle_close(status_handle);
    let now = rt::monotonic_now()?;
    write_output_linef(
        output,
        format_args!(
            "ticks={} heartbeats={} last-heartbeat={} tracked-services={}",
            now, heartbeats, last_tick, tracked_services
        ),
    )
}

pub(crate) fn cmd_status_services(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let status_handle = rt::lookup_service(bootstrap, ServiceId::Status)?;
    let mut entries = [rt::StatusServiceInfo {
        service_id: ServiceId::RootManager,
        phase: rt::ManagerServicePhase::Dormant,
        health: rt::StatusHealth::Unknown,
        detail_kind: 0,
        detail0: 0,
        detail1: 0,
        updated_tick: 0,
    }; MAX_STATUS_SERVICES];
    let count = rt::status_list_services(status_handle, &mut entries)?;
    let _ = rt::handle_close(status_handle);
    for entry in entries[..count].iter().copied() {
        write_output_linef(
            output,
            format_args!(
                "{:<16} phase={} health={} detail={} {} {} updated={}",
                service_name(entry.service_id),
                phase_name(entry.phase),
                health_name(entry.health),
                detail_kind_name(entry.detail_kind),
                entry.detail0,
                entry.detail1,
                entry.updated_tick,
            ),
        )?;
    }
    Ok(())
}

pub(crate) fn cmd_status_watch(
    bootstrap: rt::Handle,
    output: ShellOutput,
    count: usize,
) -> rt::Result<()> {
    let status_handle = rt::lookup_service(bootstrap, ServiceId::Status)?;
    let subscription = rt::status_subscribe(status_handle, None)?;
    let _ = rt::handle_close(status_handle);
    for _ in 0..count {
        let entry = rt::status_receive_event(subscription)?;
        write_output_linef(
            output,
            format_args!(
                "{:<16} phase={} health={} detail={} {} {} updated={}",
                service_name(entry.service_id),
                phase_name(entry.phase),
                health_name(entry.health),
                detail_kind_name(entry.detail_kind),
                entry.detail0,
                entry.detail1,
                entry.updated_tick,
            ),
        )?;
    }
    let _ = rt::handle_close(subscription);
    Ok(())
}

fn lookup_policy_name(policy: ManagerLookupPolicy) -> &'static str {
    match policy {
        ManagerLookupPolicy::Default => "default",
        ManagerLookupPolicy::Revoked => "revoked",
    }
}

fn health_name(health: rt::StatusHealth) -> &'static str {
    match health {
        rt::StatusHealth::Healthy => "healthy",
        rt::StatusHealth::Degraded => "degraded",
        rt::StatusHealth::Failing => "failing",
        rt::StatusHealth::Recovering => "recovering",
        rt::StatusHealth::Dormant => "dormant",
        rt::StatusHealth::Unknown => "unknown",
    }
}

fn detail_kind_name(kind: u32) -> &'static str {
    match kind {
        x if x == rt::status_detail_kind::LIFECYCLE => "lifecycle",
        x if x == rt::status_detail_kind::BLOCKED_DEPENDENCY => "blocked",
        x if x == rt::status_detail_kind::RESTART_BACKOFF => "backoff",
        x if x == rt::status_detail_kind::HEARTBEAT => "heartbeat",
        _ => "none",
    }
}

fn format_rights(rights: u64) -> FixedLogBuffer<64> {
    let mut buffer = FixedLogBuffer::<64>::new();
    let mut wrote = false;
    for (name, bit) in [
        ("read", rt::rights::READ),
        ("write", rt::rights::WRITE),
        ("map", rt::rights::MAP),
        ("signal", rt::rights::SIGNAL),
        ("wait", rt::rights::WAIT),
        ("send", rt::rights::SEND),
        ("recv", rt::rights::RECEIVE),
        ("dup", rt::rights::DUPLICATE),
        ("xfer", rt::rights::TRANSFER),
        ("manage", rt::rights::MANAGE),
    ] {
        if rights & bit == 0 {
            continue;
        }
        if wrote {
            let _ = write!(buffer, "|");
        }
        let _ = write!(buffer, "{name}");
        wrote = true;
    }
    if !wrote {
        let _ = write!(buffer, "none");
    }
    buffer
}

pub(crate) fn cmd_run_sysinfo(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let task_handle = match rt::manager_launch_program(
        bootstrap,
        ServiceImageId::SysinfoTool,
        Some(output.handle),
    ) {
        Ok(handle) => handle,
        Err(rt::Error::PermissionDenied) => {
            return explain_native_launch_denial(
                bootstrap,
                output,
                super::deny::DenialSubject::App { name: "sysinfo" },
                Some(ServiceImageId::SysinfoTool),
            );
        }
        Err(error) => return Err(error),
    };
    let status = rt::wait_for_exit(task_handle)?;
    let _ = rt::handle_close(task_handle);
    write_output_linef(
        output,
        format_args!("sysinfo-tool exited with {:#x}", status.exit_code),
    )
}

pub(crate) fn cmd_run_image(
    bootstrap: rt::Handle,
    output: ShellOutput,
    path: &str,
) -> rt::Result<()> {
    let task_handle = match rt::manager_launch_stored_program_with_payload(
        bootstrap,
        path,
        &[1],
        &[rt::StartupHandle {
            handle: output.handle,
            rights: rt::rights::SEND
                | rt::rights::RECEIVE
                | rt::rights::DUPLICATE
                | rt::rights::TRANSFER,
        }],
    ) {
        Ok(handle) => handle,
        Err(rt::Error::PermissionDenied) => {
            return explain_native_launch_denial(
                bootstrap,
                output,
                super::deny::DenialSubject::StoredImage { path },
                None,
            );
        }
        Err(error) => return Err(error),
    };
    let status = rt::wait_for_exit(task_handle)?;
    let _ = rt::handle_close(task_handle);
    write_output_linef(
        output,
        format_args!("image exited with {:#x}", status.exit_code),
    )
}

fn explain_native_launch_denial(
    bootstrap: rt::Handle,
    output: ShellOutput,
    subject: super::deny::DenialSubject<'_>,
    image_id: Option<ServiceImageId>,
) -> rt::Result<()> {
    let observation = super::deny::observe_native_denial(bootstrap, image_id);
    let explanation = super::deny::classify_denial(&subject, &observation);
    super::deny::render_denial_explanation(output, &subject, &explanation)
}

/// Launch an installed package through the manager-mediated stored-image
/// path. The package identity comes from package-service (installed check +
/// version for the report) and the program image rides the same manager
/// launch contract as `run image`.
pub(crate) fn cmd_run_package(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
) -> rt::Result<()> {
    let name = service_name(service_id);
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut installed = [0u8; MAX_VERSION_BYTES];
    let mut active = [0u8; MAX_VERSION_BYTES];
    let mut rollback = [0u8; MAX_VERSION_BYTES];
    let mut latest = [0u8; MAX_VERSION_BYTES];
    let info_result = rt::package_info(
        package_handle,
        service_id,
        &mut installed,
        &mut active,
        &mut rollback,
        &mut latest,
    );
    let _ = rt::handle_close(package_handle);
    let Ok(info) = info_result else {
        return write_output_linef(
            output,
            format_args!("{name} is not a known package; try pkg catalog"),
        );
    };
    if !info.installed {
        return write_output_linef(
            output,
            format_args!("{name} is not installed; install it with: pkg install {name}"),
        );
    }
    let installed_version = printable_version(
        core::str::from_utf8(&installed[..info.installed_version_len])
            .map_err(|_| rt::Error::InvalidArgument)?,
    );

    // Visibility first: an already-running service needs focus/restart, not a
    // second launch.
    let running_phase = rt::manager_service_status(bootstrap, service_id)
        .ok()
        .map(|status| {
            let running = matches!(
                status.phase,
                rt::ManagerServicePhase::Starting
                    | rt::ManagerServicePhase::Ready
                    | rt::ManagerServicePhase::Backoff
                    | rt::ManagerServicePhase::Degraded
            );
            running.then_some(status.phase)
        })
        .flatten();
    if let Some(phase) = running_phase {
        return write_output_linef(
            output,
            format_args!(
                "{name} {installed_version} already running (phase={})",
                crate::util::phase_name(phase),
            ),
        );
    }

    let image_path = package_program_image_path(name);
    match rt::manager_launch_stored_program_with_payload(bootstrap, image_path.as_str(), &[0], &[])
    {
        Ok(task_handle) => {
            let _ = rt::handle_close(task_handle);
            write_output_linef(
                output,
                format_args!("launched {name} {installed_version} via manager ({image_path})"),
            )
        }
        Err(rt::Error::PermissionDenied) => explain_native_launch_denial(
            bootstrap,
            output,
            super::deny::DenialSubject::App { name },
            None,
        ),
        Err(rt::Error::NotFound) => write_output_linef(
            output,
            format_args!("no launchable image for {name}; package ships no program"),
        ),
        Err(error) => Err(error),
    }
}

/// Installed packages materialize their service program at a deterministic
/// boot-store path; this mirrors the catalog layout used by the manager.
fn package_program_image_path(name: &str) -> heapless_path::Path {
    let mut path = heapless_path::Path::new();
    path.push_str("services/");
    path.push_str(name);
    path.push_str("/program.img");
    path
}

mod heapless_path {
    /// Bounded fixed buffer for boot-store paths built at runtime.
    pub(crate) struct Path {
        bytes: [u8; 64],
        len: usize,
    }

    impl Path {
        pub(crate) const fn new() -> Self {
            Self {
                bytes: [0; 64],
                len: 0,
            }
        }

        pub(crate) fn push_str(&mut self, piece: &str) {
            let bytes = piece.as_bytes();
            let remaining = self.bytes.len() - self.len;
            let take = bytes.len().min(remaining);
            self.bytes[self.len..self.len + take].copy_from_slice(&bytes[..take]);
            self.len += take;
        }

        pub(crate) fn as_str(&self) -> &str {
            core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
        }
    }

    impl core::fmt::Display for Path {
        fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            formatter.write_str(self.as_str())
        }
    }
}

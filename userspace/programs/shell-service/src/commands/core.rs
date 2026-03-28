use serviceos_userspace_runtime as rt;
use rt::{
    ConfigKey, ManagerServiceInfo, ManagerServicePhase, ServiceId, ServiceImageId,
};

use crate::util::{
    config_key_name, config_value_text, manager_status_name, phase_name,
    service_name, write_log_record, write_output_linef, shell_output_write, ShellOutput,
    MAX_CAT_CHUNK, MAX_LISTED_SERVICES, MAX_STORAGE_PATH,
};

pub(crate) fn cmd_services(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
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

pub(crate) fn cmd_service(bootstrap: rt::Handle, output: ShellOutput, service_id: ServiceId) -> rt::Result<()> {
    let (status, phase, attempts, last_exit) = rt::manager_service_status(bootstrap, service_id)?;
    write_output_linef(
        output,
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

pub(crate) fn cmd_restart(bootstrap: rt::Handle, output: ShellOutput, service_id: ServiceId) -> rt::Result<()> {
    rt::manager_restart_service(bootstrap, service_id)?;
    write_output_linef(
        output,
        format_args!("restart requested for {}", service_name(service_id)),
    )
}

pub(crate) fn cmd_logs(
    bootstrap: rt::Handle,
    output: ShellOutput,
    count: usize,
) -> rt::Result<()> {
    let log_handle = rt::lookup_service(bootstrap, ServiceId::Log)?;
    let (oldest, next) = rt::log_query_info(log_handle)?;
    if next == 0 || oldest == next {
        let _ = rt::handle_close(log_handle);
        return write_output_linef(output, format_args!("no log records"));
    }

    let start = oldest.max(next.saturating_sub(count as u64));
    for sequence in start..next {
        if let Some(record) = rt::log_query_record(log_handle, sequence)? {
            write_log_record(output, record)?;
        }
    }

    let _ = rt::handle_close(log_handle);
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
            format_args!("{} = {}", config_key_name(key), config_value_text(key, value)),
        )?;
    }
    let _ = rt::handle_close(config_handle);
    Ok(())
}

pub(crate) fn cmd_store_ls(
    bootstrap: rt::Handle,
    output: ShellOutput,
    prefix: &str,
) -> rt::Result<()> {
    let storage_handle = rt::lookup_service(bootstrap, ServiceId::Storage)?;
    let mut path_buffer = [0u8; MAX_STORAGE_PATH];
    let mut index = 0usize;
    while let Some((_, path_len)) = rt::storage_list(storage_handle, prefix, index, &mut path_buffer)? {
        let path =
            core::str::from_utf8(&path_buffer[..path_len]).map_err(|_| rt::Error::InvalidArgument)?;
        write_output_linef(output, format_args!("{path}"))?;
        index += 1;
    }
    let _ = rt::handle_close(storage_handle);
    if index == 0 {
        write_output_linef(output, format_args!("no entries"))
    } else {
        Ok(())
    }
}

pub(crate) fn cmd_cat(bootstrap: rt::Handle, output: ShellOutput, path: &str) -> rt::Result<()> {
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
        let text =
            core::str::from_utf8(&buffer[..read]).map_err(|_| rt::Error::InvalidArgument)?;
        shell_output_write(output, text)?;
        offset += read;
    }
    let _ = rt::storage_blob_close(blob_handle);
    shell_output_write(output, "\r\n")?;
    Ok(())
}

pub(crate) fn cmd_status(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let status_handle = rt::lookup_service(bootstrap, ServiceId::Status)?;
    let (heartbeats, last_tick) = rt::status_snapshot(status_handle)?;
    let _ = rt::handle_close(status_handle);
    let now = rt::monotonic_now()?;
    write_output_linef(
        output,
        format_args!("ticks={} heartbeats={} last-heartbeat={}", now, heartbeats, last_tick),
    )
}

pub(crate) fn cmd_run_sysinfo(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let task_handle =
        rt::manager_launch_program(bootstrap, ServiceImageId::SysinfoTool, Some(output.handle))?;
    let status = rt::wait_for_exit(task_handle)?;
    let _ = rt::handle_close(task_handle);
    write_output_linef(
        output,
        format_args!("sysinfo-tool exited with {:#x}", status.exit_code),
    )
}

use rt::{PackageMaintenanceAction, ServiceId};
use serviceos_userspace_runtime as rt;

use crate::util::{
    MAX_VERSION_BYTES, ShellOutput, printable_version, service_name, write_output_linef,
};

use super::parse::{channel_name, maintenance_action_name, parse_channel, parse_ring, ring_name};

pub(super) fn cmd_pkg_install(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
    version: Option<&str>,
) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let result = rt::package_install(package_handle, service_id, version);
    let _ = rt::handle_close(package_handle);
    result?;
    write_output_linef(
        output,
        format_args!("installed {}", service_name(service_id)),
    )
}

pub(super) fn cmd_pkg_update(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
    version: Option<&str>,
) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let result = rt::package_update(package_handle, service_id, version);
    let _ = rt::handle_close(package_handle);
    result?;
    write_output_linef(output, format_args!("updated {}", service_name(service_id)))
}

pub(super) fn cmd_pkg_remove(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let result = rt::package_remove(package_handle, service_id);
    let _ = rt::handle_close(package_handle);
    result?;
    write_output_linef(output, format_args!("removed {}", service_name(service_id)))
}

pub(super) fn cmd_pkg_rollback(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let result = rt::package_rollback(package_handle, service_id);
    let _ = rt::handle_close(package_handle);
    result?;
    write_output_linef(
        output,
        format_args!("rolled back {}", service_name(service_id)),
    )
}

pub(super) fn cmd_pkg_pin(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
    version: &str,
) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut pinned = [0u8; MAX_VERSION_BYTES];
    let existing = rt::package_policy(package_handle, service_id, &mut pinned)?;
    rt::package_policy_set(
        package_handle,
        service_id,
        existing.channel,
        existing.ring,
        if version == "none" {
            None
        } else {
            Some(version)
        },
    )?;
    let _ = rt::handle_close(package_handle);
    write_output_linef(
        output,
        format_args!(
            "pinned {} to {}",
            service_name(service_id),
            printable_version(version)
        ),
    )
}

pub(super) fn cmd_pkg_channel(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
    channel_text: &str,
) -> rt::Result<()> {
    let Some(channel) = parse_channel(channel_text) else {
        return write_output_linef(
            output,
            format_args!("channel must be stable, beta, or canary"),
        );
    };
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut pinned = [0u8; MAX_VERSION_BYTES];
    let existing = rt::package_policy(package_handle, service_id, &mut pinned)?;
    let pin = core::str::from_utf8(&pinned[..existing.pinned_version_len])
        .map_err(|_| rt::Error::InvalidArgument)?;
    rt::package_policy_set(
        package_handle,
        service_id,
        channel,
        existing.ring,
        if pin.is_empty() { None } else { Some(pin) },
    )?;
    let _ = rt::handle_close(package_handle);
    write_output_linef(
        output,
        format_args!(
            "set {} channel to {}",
            service_name(service_id),
            channel_name(channel)
        ),
    )
}

pub(super) fn cmd_pkg_ring(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
    ring_text: &str,
) -> rt::Result<()> {
    let Some(ring) = parse_ring(ring_text) else {
        return write_output_linef(
            output,
            format_args!("ring must be production, preview, or testing"),
        );
    };
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut pinned = [0u8; MAX_VERSION_BYTES];
    let existing = rt::package_policy(package_handle, service_id, &mut pinned)?;
    let pin = core::str::from_utf8(&pinned[..existing.pinned_version_len])
        .map_err(|_| rt::Error::InvalidArgument)?;
    rt::package_policy_set(
        package_handle,
        service_id,
        existing.channel,
        ring,
        if pin.is_empty() { None } else { Some(pin) },
    )?;
    let _ = rt::handle_close(package_handle);
    write_output_linef(
        output,
        format_args!(
            "set {} ring to {}",
            service_name(service_id),
            ring_name(ring)
        ),
    )
}

pub(super) fn cmd_pkg_maintenance(
    bootstrap: rt::Handle,
    output: ShellOutput,
    action: PackageMaintenanceAction,
) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let info = rt::package_maintenance(package_handle, action)?;
    let _ = rt::handle_close(package_handle);
    write_output_linef(
        output,
        format_args!(
            "{} repaired={} collected={}",
            maintenance_action_name(info.action),
            info.repaired_entries,
            info.garbage_collected_entries,
        ),
    )
}

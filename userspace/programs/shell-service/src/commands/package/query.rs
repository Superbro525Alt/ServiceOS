use rt::ServiceId;
use serviceos_userspace_runtime as rt;

use crate::util::{
    MAX_VERSION_BYTES, ShellOutput, printable_version, service_name, write_output_linef,
};

use super::parse::{MAX_PACKAGE_TEXT, channel_name, ring_name, trust_state_name};

pub(super) fn cmd_pkg_list(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut installed = [0u8; MAX_VERSION_BYTES];
    let mut active = [0u8; MAX_VERSION_BYTES];
    let mut index = 0usize;

    while let Some(entry) = rt::package_list(package_handle, index, &mut installed, &mut active)? {
        let installed_version = core::str::from_utf8(&installed[..entry.installed_version_len])
            .map_err(|_| rt::Error::InvalidArgument)?;
        let active_version = core::str::from_utf8(&active[..entry.active_version_len])
            .map_err(|_| rt::Error::InvalidArgument)?;
        write_output_linef(
            output,
            format_args!(
                "{:<16} repo={} installed={} active={} rollback={}",
                service_name(entry.service_id),
                entry.repository_versions,
                printable_version(installed_version),
                printable_version(active_version),
                if entry.rollback_available {
                    "yes"
                } else {
                    "no"
                },
            ),
        )?;
        index += 1;
    }

    let _ = rt::handle_close(package_handle);
    if index == 0 {
        write_output_linef(output, format_args!("no packages"))
    } else {
        Ok(())
    }
}

pub(super) fn cmd_pkg_catalog(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut latest = [0u8; MAX_VERSION_BYTES];
    let mut category = [0u8; MAX_PACKAGE_TEXT];
    let mut summary = [0u8; MAX_PACKAGE_TEXT];
    let mut index = 0usize;

    while let Some(entry) = rt::package_catalog(
        package_handle,
        index,
        &mut latest,
        &mut category,
        &mut summary,
    )? {
        let latest_text = core::str::from_utf8(&latest[..entry.latest_version_len])
            .map_err(|_| rt::Error::InvalidArgument)?;
        let category_text = core::str::from_utf8(&category[..entry.category_len])
            .map_err(|_| rt::Error::InvalidArgument)?;
        let summary_text = core::str::from_utf8(&summary[..entry.summary_len])
            .map_err(|_| rt::Error::InvalidArgument)?;
        write_output_linef(
            output,
            format_args!(
                "{:<16} repo={} latest={} category={} summary={} flags={}{}{}",
                service_name(entry.service_id),
                entry.repo_index,
                printable_version(latest_text),
                category_text,
                summary_text,
                if entry.installed { "I" } else { "-" },
                if entry.active { "A" } else { "-" },
                if entry.rollback_available { "R" } else { "-" },
            ),
        )?;
        index += 1;
    }

    let _ = rt::handle_close(package_handle);
    if index == 0 {
        write_output_linef(output, format_args!("empty catalog"))
    } else {
        Ok(())
    }
}

pub(super) fn cmd_pkg_info(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
) -> rt::Result<()> {
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

    write_output_linef(
        output,
        format_args!(
            "{} repo={} installed={} active={} rollback={} latest={}",
            service_name(service_id),
            info.repository_versions,
            printable_version(
                core::str::from_utf8(&installed[..info.installed_version_len])
                    .map_err(|_| rt::Error::InvalidArgument)?
            ),
            printable_version(
                core::str::from_utf8(&active[..info.active_version_len])
                    .map_err(|_| rt::Error::InvalidArgument)?
            ),
            printable_version(
                core::str::from_utf8(&rollback[..info.rollback_version_len])
                    .map_err(|_| rt::Error::InvalidArgument)?
            ),
            printable_version(
                core::str::from_utf8(&latest[..info.latest_version_len])
                    .map_err(|_| rt::Error::InvalidArgument)?
            ),
        ),
    )
}

pub(super) fn cmd_pkg_history(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut current = [0u8; MAX_VERSION_BYTES];
    let mut previous = [0u8; MAX_VERSION_BYTES];
    let (current_len, previous_len) =
        rt::package_history(package_handle, service_id, &mut current, &mut previous)?;
    let _ = rt::handle_close(package_handle);
    write_output_linef(
        output,
        format_args!(
            "{} current={} rollback={}",
            service_name(service_id),
            printable_version(
                core::str::from_utf8(&current[..current_len])
                    .map_err(|_| rt::Error::InvalidArgument)?
            ),
            printable_version(
                core::str::from_utf8(&previous[..previous_len])
                    .map_err(|_| rt::Error::InvalidArgument)?
            ),
        ),
    )
}

pub(super) fn cmd_pkg_provenance(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut installed = [0u8; MAX_VERSION_BYTES];
    let mut active = [0u8; MAX_VERSION_BYTES];
    let mut rollback = [0u8; MAX_VERSION_BYTES];
    let mut latest = [0u8; MAX_VERSION_BYTES];
    let mut source = [0u8; MAX_PACKAGE_TEXT];
    let info = rt::package_provenance(
        package_handle,
        service_id,
        &mut installed,
        &mut active,
        &mut rollback,
        &mut latest,
        &mut source,
    )?;
    let _ = rt::handle_close(package_handle);

    write_output_linef(
        output,
        format_args!(
            "{} repo={} trust={} channel={} ring={} installed={} active={} rollback={} latest={} source={}",
            service_name(service_id),
            info.repo_index,
            trust_state_name(info.trust_state),
            channel_name(info.channel),
            ring_name(info.ring),
            printable_version(
                core::str::from_utf8(&installed[..info.installed_version_len])
                    .map_err(|_| rt::Error::InvalidArgument)?
            ),
            printable_version(
                core::str::from_utf8(&active[..info.active_version_len])
                    .map_err(|_| rt::Error::InvalidArgument)?
            ),
            printable_version(
                core::str::from_utf8(&rollback[..info.rollback_version_len])
                    .map_err(|_| rt::Error::InvalidArgument)?
            ),
            printable_version(
                core::str::from_utf8(&latest[..info.latest_version_len])
                    .map_err(|_| rt::Error::InvalidArgument)?
            ),
            core::str::from_utf8(&source[..info.source_len])
                .map_err(|_| rt::Error::InvalidArgument)?,
        ),
    )
}

pub(super) fn cmd_pkg_policy(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut pinned = [0u8; MAX_VERSION_BYTES];
    let info = rt::package_policy(package_handle, service_id, &mut pinned)?;
    let _ = rt::handle_close(package_handle);
    write_output_linef(
        output,
        format_args!(
            "{} channel={} ring={} pin={}",
            service_name(service_id),
            channel_name(info.channel),
            ring_name(info.ring),
            printable_version(
                core::str::from_utf8(&pinned[..info.pinned_version_len])
                    .map_err(|_| rt::Error::InvalidArgument)?
            ),
        ),
    )
}

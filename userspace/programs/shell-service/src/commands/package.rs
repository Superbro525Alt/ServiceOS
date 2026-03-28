use serviceos_userspace_runtime as rt;
use rt::ServiceId;

use crate::util::{
    parse_service_name, printable_version, service_name, write_session_linef, MAX_VERSION_BYTES,
};

pub(crate) fn cmd_pkg<'a, I>(
    bootstrap: rt::Handle,
    session: rt::Handle,
    mut parts: I,
) -> rt::Result<()>
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
        let installed_version = core::str::from_utf8(&installed[..entry.installed_version_len])
            .map_err(|_| rt::Error::InvalidArgument)?;
        let active_version = core::str::from_utf8(&active[..entry.active_version_len])
            .map_err(|_| rt::Error::InvalidArgument)?;
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
    write_session_linef(session, format_args!("installed {}", service_name(service_id)))
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

use rt::ServiceId;
use serviceos_userspace_runtime as rt;

use crate::util::{
    MAX_VERSION_BYTES, ShellOutput, UpdateDecision, decide_update, printable_version, service_name,
    write_output_linef,
};

use super::parse::{
    MAX_PACKAGE_TEXT, channel_name, ring_name, signing_state_name, trust_state_name,
};

/// Upper bound for the catalog snapshot used to annotate `pkg list` rows
/// with an update-available flag; matches the package-service slot count.
const MAX_CATALOG_SNAPSHOT: usize = 32;

/// Snapshot of the catalog's newest version per service, used to decide the
/// per-row update flag without a second round trip per row.
pub(in crate::commands) struct CatalogSnapshot {
    service_ids: [ServiceId; MAX_CATALOG_SNAPSHOT],
    latest: [[u8; MAX_VERSION_BYTES]; MAX_CATALOG_SNAPSHOT],
    latest_lens: [usize; MAX_CATALOG_SNAPSHOT],
    count: usize,
}

impl CatalogSnapshot {
    pub(in crate::commands) fn capture(package_handle: rt::Handle) -> Self {
        let mut snapshot = Self {
            service_ids: [ServiceId::RootManager; MAX_CATALOG_SNAPSHOT],
            latest: [[0; MAX_VERSION_BYTES]; MAX_CATALOG_SNAPSHOT],
            latest_lens: [0; MAX_CATALOG_SNAPSHOT],
            count: 0,
        };
        let mut latest = [0u8; MAX_VERSION_BYTES];
        let mut category = [0u8; MAX_PACKAGE_TEXT];
        let mut summary = [0u8; MAX_PACKAGE_TEXT];
        while snapshot.count < MAX_CATALOG_SNAPSHOT {
            let Ok(Some(entry)) = rt::package_catalog(
                package_handle,
                snapshot.count,
                &mut latest,
                &mut category,
                &mut summary,
            ) else {
                break;
            };
            let index = snapshot.count;
            snapshot.service_ids[index] = entry.service_id;
            snapshot.latest[index] = latest;
            snapshot.latest_lens[index] = entry.latest_version_len.min(MAX_VERSION_BYTES);
            snapshot.count += 1;
        }
        snapshot
    }

    /// Most recent event tick for a service from the log ring, if any.
    pub(in crate::commands) fn latest_text(&self, service_id: ServiceId) -> Option<&str> {
        for index in 0..self.count {
            if self.service_ids[index] == service_id {
                return core::str::from_utf8(&self.latest[index][..self.latest_lens[index]]).ok();
            }
        }
        None
    }

    fn decision(&self, service_id: ServiceId, installed: Option<&str>) -> UpdateDecision {
        decide_update(installed, self.latest_text(service_id))
    }
}

pub(super) fn cmd_pkg_list(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let catalog = CatalogSnapshot::capture(package_handle);
    let mut installed = [0u8; MAX_VERSION_BYTES];
    let mut active = [0u8; MAX_VERSION_BYTES];
    let mut index = 0usize;

    while let Some(entry) = rt::package_list(package_handle, index, &mut installed, &mut active)? {
        let installed_version = core::str::from_utf8(&installed[..entry.installed_version_len])
            .map_err(|_| rt::Error::InvalidArgument)?;
        let active_version = core::str::from_utf8(&active[..entry.active_version_len])
            .map_err(|_| rt::Error::InvalidArgument)?;
        let decision = catalog.decision(entry.service_id, Some(installed_version));
        write_output_linef(
            output,
            format_args!(
                "{:<16} repo={} installed={} active={} rollback={} update={}",
                service_name(entry.service_id),
                entry.repository_versions,
                printable_version(installed_version),
                printable_version(active_version),
                if entry.rollback_available {
                    "yes"
                } else {
                    "no"
                },
                decision.flag(),
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

    // Trust/provenance view: reuse the provenance and history contracts so the
    // operator sees signing state and rollback provenance alongside versions.
    let mut prov_installed = [0u8; MAX_VERSION_BYTES];
    let mut prov_active = [0u8; MAX_VERSION_BYTES];
    let mut prov_rollback = [0u8; MAX_PACKAGE_TEXT];
    let mut prov_latest = [0u8; MAX_VERSION_BYTES];
    let mut source = [0u8; MAX_PACKAGE_TEXT];
    let provenance = rt::package_provenance(
        package_handle,
        service_id,
        &mut prov_installed,
        &mut prov_active,
        &mut prov_rollback,
        &mut prov_latest,
        &mut source,
    );

    let mut current = [0u8; MAX_VERSION_BYTES];
    let mut previous = [0u8; MAX_VERSION_BYTES];
    let history = rt::package_history(package_handle, service_id, &mut current, &mut previous);
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
    )?;

    if let Ok(provenance) = provenance {
        let source_text = core::str::from_utf8(&source[..provenance.source_len]).unwrap_or("?");
        write_output_linef(
            output,
            format_args!(
                "  trust={} signing={} channel={} ring={} source={}",
                trust_state_name(provenance.trust_state),
                signing_state_name(provenance.trust_state),
                channel_name(provenance.channel),
                ring_name(provenance.ring),
                source_text,
            ),
        )?;
        write_output_linef(
            output,
            format_args!(
                "  rollback-provenance: available={} previous={} source={}",
                if info.rollback_available || provenance.rollback_available {
                    "yes"
                } else {
                    "no"
                },
                printable_version(
                    core::str::from_utf8(&previous[..history.unwrap_or((0, 0)).1])
                        .or_else(|_| core::str::from_utf8(
                            &prov_rollback[..provenance.rollback_version_len.min(MAX_PACKAGE_TEXT)]
                        ))
                        .unwrap_or("?"),
                ),
                source_text,
            ),
        )?;
    }

    // Update/remove visibility: compare the installed version against the
    // newest catalog version and report when this package last changed.
    let installed_text = core::str::from_utf8(&installed[..info.installed_version_len]).ok();
    let latest_text = core::str::from_utf8(&latest[..info.latest_version_len]).ok();
    let decision = decide_update(installed_text, latest_text);
    let mut last_change = rt::FixedLogBuffer::<48>::new();
    match last_package_event(bootstrap, service_id) {
        Some((kind, tick)) => {
            let _ = core::fmt::Write::write_fmt(
                &mut last_change,
                format_args!("{} at tick {}", kind, tick),
            );
        }
        None => {
            let _ = core::fmt::Write::write_fmt(&mut last_change, format_args!("never"));
        }
    }
    write_output_linef(
        output,
        format_args!(
            "  update-available={} ({}) last-change={}",
            decision.flag(),
            decision.label(),
            last_change.as_str(),
        ),
    )?;
    Ok(())
}

/// Scan depth for the last-change lookup; bounded so a large retained ring
/// cannot stall the operator prompt.
const LAST_CHANGE_SCAN_DEPTH: u64 = 256;

/// Most recent install/update event for a package from the log ring,
/// newest-first. Returns ("update"|"install", tick).
fn last_package_event(bootstrap: rt::Handle, service_id: ServiceId) -> Option<(&'static str, u64)> {
    let log_handle = rt::lookup_service(bootstrap, ServiceId::Log).ok()?;
    let result = (|| {
        let (oldest, next) = rt::log_query_info(log_handle).ok()?;
        let start = next.saturating_sub(LAST_CHANGE_SCAN_DEPTH).max(oldest);
        for sequence in (start..next).rev() {
            let record = rt::log_query_record(log_handle, sequence).ok()??;
            if record.source != ServiceId::Package || record.arg0 != service_id as u32 as u64 {
                continue;
            }
            let kind = match record.event {
                rt::LogEvent::PackageUpdated => "update",
                rt::LogEvent::PackageInstalled => "install",
                _ => continue,
            };
            return Some((kind, record.tick));
        }
        None
    })();
    let _ = rt::handle_close(log_handle);
    result
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

use serviceos_userspace_runtime as rt;
use rt::{PackageChannel, PackageMaintenanceAction, PackageRepositoryTrustMode, PackageRing, ServiceId};

use crate::util::{
    parse_service_name, printable_version, service_name, write_output_linef, ShellOutput,
    MAX_VERSION_BYTES,
};

const MAX_PACKAGE_TEXT: usize = 96;

pub(crate) fn cmd_pkg<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    mut parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("list") => cmd_pkg_list(bootstrap, output),
        Some("catalog") => cmd_pkg_catalog(bootstrap, output),
        Some("repos") => cmd_pkg_repos(bootstrap, output),
        Some("repo") => cmd_pkg_repo(bootstrap, output, parts),
        Some("info") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => cmd_pkg_info(bootstrap, output, service_id),
            None => write_output_linef(output, format_args!("usage: pkg info <name>")),
        },
        Some("install") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => cmd_pkg_install(bootstrap, output, service_id, parts.next()),
            None => write_output_linef(output, format_args!("usage: pkg install <name> [version]")),
        },
        Some("update") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => cmd_pkg_update(bootstrap, output, service_id, parts.next()),
            None => write_output_linef(output, format_args!("usage: pkg update <name> [version]")),
        },
        Some("remove") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => cmd_pkg_remove(bootstrap, output, service_id),
            None => write_output_linef(output, format_args!("usage: pkg remove <name>")),
        },
        Some("rollback") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => cmd_pkg_rollback(bootstrap, output, service_id),
            None => write_output_linef(output, format_args!("usage: pkg rollback <name>")),
        },
        Some("history") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => cmd_pkg_history(bootstrap, output, service_id),
            None => write_output_linef(output, format_args!("usage: pkg history <name>")),
        },
        Some("provenance") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => cmd_pkg_provenance(bootstrap, output, service_id),
            None => write_output_linef(output, format_args!("usage: pkg provenance <name>")),
        },
        Some("policy") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => cmd_pkg_policy(bootstrap, output, service_id),
            None => write_output_linef(output, format_args!("usage: pkg policy <name>")),
        },
        Some("pin") => match (parts.next().and_then(parse_service_name), parts.next()) {
            (Some(service_id), Some(version)) => cmd_pkg_pin(bootstrap, output, service_id, version),
            _ => write_output_linef(output, format_args!("usage: pkg pin <name> <version|none>")),
        },
        Some("channel") => match (parts.next().and_then(parse_service_name), parts.next()) {
            (Some(service_id), Some(channel)) => cmd_pkg_channel(bootstrap, output, service_id, channel),
            _ => write_output_linef(output, format_args!("usage: pkg channel <name> <stable|beta|canary>")),
        },
        Some("ring") => match (parts.next().and_then(parse_service_name), parts.next()) {
            (Some(service_id), Some(ring)) => cmd_pkg_ring(bootstrap, output, service_id, ring),
            _ => write_output_linef(output, format_args!("usage: pkg ring <name> <production|preview|testing>")),
        },
        Some("verify") => cmd_pkg_maintenance(bootstrap, output, PackageMaintenanceAction::Validate),
        Some("repair") => cmd_pkg_maintenance(bootstrap, output, PackageMaintenanceAction::Repair),
        Some("gc") => cmd_pkg_maintenance(bootstrap, output, PackageMaintenanceAction::GarbageCollect),
        _ => write_output_linef(
            output,
            format_args!(
                "usage: pkg <list|catalog|repos|repo|info|install|update|remove|rollback|history|provenance|policy|pin|channel|ring|verify|repair|gc> ..."
            ),
        ),
    }
}

fn cmd_pkg_list(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
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
                if entry.rollback_available { "yes" } else { "no" },
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

fn cmd_pkg_catalog(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut latest = [0u8; MAX_VERSION_BYTES];
    let mut category = [0u8; MAX_PACKAGE_TEXT];
    let mut summary = [0u8; MAX_PACKAGE_TEXT];
    let mut index = 0usize;

    while let Some(entry) =
        rt::package_catalog(package_handle, index, &mut latest, &mut category, &mut summary)?
    {
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

fn cmd_pkg_repos(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut name = [0u8; MAX_PACKAGE_TEXT];
    let mut url = [0u8; MAX_PACKAGE_TEXT];
    let mut index = 0usize;

    while let Some(repo) = rt::package_repository_list(package_handle, index, &mut name, &mut url)? {
        let name_text = core::str::from_utf8(&name[..repo.name_len]).map_err(|_| rt::Error::InvalidArgument)?;
        let url_text = core::str::from_utf8(&url[..repo.url_len]).map_err(|_| rt::Error::InvalidArgument)?;
        write_output_linef(
            output,
            format_args!(
                "#{} {} pkgs={} trust={} sync={} channel={} ring={} enabled={} digest={:016x} source={}",
                repo.repo_index,
                name_text,
                repo.package_count,
                trust_mode_name(repo.trust_mode),
                repo_sync_state_name(repo.sync_state),
                channel_name(repo.channel),
                ring_name(repo.ring),
                if repo.enabled { "yes" } else { "no" },
                repo.last_digest,
                url_text,
            ),
        )?;
        index += 1;
    }

    let _ = rt::handle_close(package_handle);
    if index == 0 {
        write_output_linef(output, format_args!("no repositories"))
    } else {
        Ok(())
    }
}

fn cmd_pkg_repo<'a, I>(bootstrap: rt::Handle, output: ShellOutput, mut parts: I) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("add") => {
            let Some(name) = parts.next() else {
                return write_output_linef(output, format_args!("usage: pkg repo add <name> <url> [unsigned|pinned:<hex>] [stable|beta|canary] [production|preview|testing]"));
            };
            let Some(url) = parts.next() else {
                return write_output_linef(output, format_args!("usage: pkg repo add <name> <url> [unsigned|pinned:<hex>] [stable|beta|canary] [production|preview|testing]"));
            };
            let trust = parts.next().unwrap_or("unsigned");
            let channel = parts.next().unwrap_or("stable");
            let ring = parts.next().unwrap_or("user");
            cmd_pkg_repo_add(bootstrap, output, name, url, trust, channel, ring)
        }
        Some("sync") => match parts.next() {
            Some("all") | None => cmd_pkg_repo_sync(bootstrap, output, None),
            Some(index) => match parse_usize(index) {
                Some(value) => cmd_pkg_repo_sync(bootstrap, output, Some(value)),
                None => write_output_linef(output, format_args!("usage: pkg repo sync [all|index]")),
            },
        },
        _ => write_output_linef(output, format_args!("usage: pkg repo <add|sync> ...")),
    }
}

fn cmd_pkg_info(bootstrap: rt::Handle, output: ShellOutput, service_id: ServiceId) -> rt::Result<()> {
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
            printable_version(core::str::from_utf8(&installed[..info.installed_version_len]).map_err(|_| rt::Error::InvalidArgument)?),
            printable_version(core::str::from_utf8(&active[..info.active_version_len]).map_err(|_| rt::Error::InvalidArgument)?),
            printable_version(core::str::from_utf8(&rollback[..info.rollback_version_len]).map_err(|_| rt::Error::InvalidArgument)?),
            printable_version(core::str::from_utf8(&latest[..info.latest_version_len]).map_err(|_| rt::Error::InvalidArgument)?),
        ),
    )
}

fn cmd_pkg_install(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
    version: Option<&str>,
) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let result = rt::package_install(package_handle, service_id, version);
    let _ = rt::handle_close(package_handle);
    result?;
    write_output_linef(output, format_args!("installed {}", service_name(service_id)))
}

fn cmd_pkg_update(
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

fn cmd_pkg_remove(bootstrap: rt::Handle, output: ShellOutput, service_id: ServiceId) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let result = rt::package_remove(package_handle, service_id);
    let _ = rt::handle_close(package_handle);
    result?;
    write_output_linef(output, format_args!("removed {}", service_name(service_id)))
}

fn cmd_pkg_rollback(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let result = rt::package_rollback(package_handle, service_id);
    let _ = rt::handle_close(package_handle);
    result?;
    write_output_linef(output, format_args!("rolled back {}", service_name(service_id)))
}

fn cmd_pkg_history(
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
            printable_version(core::str::from_utf8(&current[..current_len]).map_err(|_| rt::Error::InvalidArgument)?),
            printable_version(core::str::from_utf8(&previous[..previous_len]).map_err(|_| rt::Error::InvalidArgument)?),
        ),
    )
}

fn cmd_pkg_provenance(
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
            printable_version(core::str::from_utf8(&installed[..info.installed_version_len]).map_err(|_| rt::Error::InvalidArgument)?),
            printable_version(core::str::from_utf8(&active[..info.active_version_len]).map_err(|_| rt::Error::InvalidArgument)?),
            printable_version(core::str::from_utf8(&rollback[..info.rollback_version_len]).map_err(|_| rt::Error::InvalidArgument)?),
            printable_version(core::str::from_utf8(&latest[..info.latest_version_len]).map_err(|_| rt::Error::InvalidArgument)?),
            core::str::from_utf8(&source[..info.source_len]).map_err(|_| rt::Error::InvalidArgument)?,
        ),
    )
}

fn cmd_pkg_policy(
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
            printable_version(core::str::from_utf8(&pinned[..info.pinned_version_len]).map_err(|_| rt::Error::InvalidArgument)?),
        ),
    )
}

fn cmd_pkg_pin(
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
        if version == "none" { None } else { Some(version) },
    )?;
    let _ = rt::handle_close(package_handle);
    write_output_linef(
        output,
        format_args!("pinned {} to {}", service_name(service_id), printable_version(version)),
    )
}

fn cmd_pkg_channel(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
    channel_text: &str,
) -> rt::Result<()> {
    let Some(channel) = parse_channel(channel_text) else {
        return write_output_linef(output, format_args!("channel must be stable, beta, or canary"));
    };
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut pinned = [0u8; MAX_VERSION_BYTES];
    let existing = rt::package_policy(package_handle, service_id, &mut pinned)?;
    let pin = core::str::from_utf8(&pinned[..existing.pinned_version_len]).map_err(|_| rt::Error::InvalidArgument)?;
    rt::package_policy_set(
        package_handle,
        service_id,
        channel,
        existing.ring,
        if pin.is_empty() { None } else { Some(pin) },
    )?;
    let _ = rt::handle_close(package_handle);
    write_output_linef(output, format_args!("set {} channel to {}", service_name(service_id), channel_name(channel)))
}

fn cmd_pkg_ring(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
    ring_text: &str,
) -> rt::Result<()> {
    let Some(ring) = parse_ring(ring_text) else {
        return write_output_linef(output, format_args!("ring must be production, preview, or testing"));
    };
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut pinned = [0u8; MAX_VERSION_BYTES];
    let existing = rt::package_policy(package_handle, service_id, &mut pinned)?;
    let pin = core::str::from_utf8(&pinned[..existing.pinned_version_len]).map_err(|_| rt::Error::InvalidArgument)?;
    rt::package_policy_set(
        package_handle,
        service_id,
        existing.channel,
        ring,
        if pin.is_empty() { None } else { Some(pin) },
    )?;
    let _ = rt::handle_close(package_handle);
    write_output_linef(output, format_args!("set {} ring to {}", service_name(service_id), ring_name(ring)))
}

fn cmd_pkg_maintenance(
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

fn cmd_pkg_repo_add(
    bootstrap: rt::Handle,
    output: ShellOutput,
    name: &str,
    url: &str,
    trust_text: &str,
    channel_text: &str,
    ring_text: &str,
) -> rt::Result<()> {
    let Some((trust_mode, digest)) = parse_repo_trust(trust_text) else {
        return write_output_linef(output, format_args!("trust must be unsigned or pinned:<hex-digest>"));
    };
    let Some(channel) = parse_channel(channel_text) else {
        return write_output_linef(output, format_args!("channel must be stable, beta, or nightly"));
    };
    let Some(ring) = parse_ring(ring_text) else {
        return write_output_linef(output, format_args!("ring must be user, beta, or canary"));
    };
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let result = rt::package_repository_add(
        package_handle,
        name,
        url,
        trust_mode,
        channel,
        ring,
        true,
        digest,
    );
    let _ = rt::handle_close(package_handle);
    result?;
    write_output_linef(output, format_args!("added repository {}", name))
}

fn cmd_pkg_repo_sync(
    bootstrap: rt::Handle,
    output: ShellOutput,
    repo_index: Option<usize>,
) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let result = rt::package_repository_sync(package_handle, repo_index);
    let _ = rt::handle_close(package_handle);
    let info = result?;
    write_output_linef(
        output,
        format_args!("synced={} failed={}", info.synced, info.failed),
    )
}

fn parse_channel(value: &str) -> Option<PackageChannel> {
    match value {
        "stable" => Some(PackageChannel::Stable),
        "beta" => Some(PackageChannel::Beta),
        "canary" => Some(PackageChannel::Canary),
        _ => None,
    }
}

fn parse_ring(value: &str) -> Option<PackageRing> {
    match value {
        "production" => Some(PackageRing::Production),
        "preview" => Some(PackageRing::Preview),
        "testing" => Some(PackageRing::Testing),
        _ => None,
    }
}

fn parse_repo_trust(value: &str) -> Option<(PackageRepositoryTrustMode, u64)> {
    if value == "unsigned" {
        Some((PackageRepositoryTrustMode::Unsigned, 0))
    } else if let Some(hex) = value.strip_prefix("pinned:") {
        parse_hex_u64(hex).map(|digest| (PackageRepositoryTrustMode::PinnedDigest, digest))
    } else {
        None
    }
}

fn parse_hex_u64(value: &str) -> Option<u64> {
    let trimmed = value.strip_prefix("0x").unwrap_or(value);
    u64::from_str_radix(trimmed, 16).ok()
}

fn parse_usize(value: &str) -> Option<usize> {
    value.parse::<usize>().ok()
}

fn trust_mode_name(value: PackageRepositoryTrustMode) -> &'static str {
    match value {
        PackageRepositoryTrustMode::Boot => "boot",
        PackageRepositoryTrustMode::Unsigned => "unsigned",
        PackageRepositoryTrustMode::PinnedDigest => "pinned",
    }
}

fn repo_sync_state_name(value: rt::PackageRepositorySyncState) -> &'static str {
    match value {
        rt::PackageRepositorySyncState::Idle => "idle",
        rt::PackageRepositorySyncState::Ready => "ready",
        rt::PackageRepositorySyncState::Offline => "offline",
        rt::PackageRepositorySyncState::Failed => "failed",
    }
}

fn trust_state_name(value: rt::PackageTrustState) -> &'static str {
    match value {
        rt::PackageTrustState::BootTrusted => "boot-trusted",
        rt::PackageTrustState::Unverified => "unverified",
        rt::PackageTrustState::DigestPinned => "digest-pinned",
        rt::PackageTrustState::VerificationFailed => "verification-failed",
    }
}

fn channel_name(value: PackageChannel) -> &'static str {
    match value {
        PackageChannel::Stable => "stable",
        PackageChannel::Beta => "beta",
        PackageChannel::Canary => "canary",
    }
}

fn ring_name(value: PackageRing) -> &'static str {
    match value {
        PackageRing::Production => "production",
        PackageRing::Preview => "preview",
        PackageRing::Testing => "testing",
    }
}

fn maintenance_action_name(value: PackageMaintenanceAction) -> &'static str {
    match value {
        PackageMaintenanceAction::Validate => "validated",
        PackageMaintenanceAction::Repair => "repaired",
        PackageMaintenanceAction::GarbageCollect => "garbage-collected",
    }
}

use rt::{PackageMaintenanceAction, PackageStatus, ServiceId};
use serviceos_userspace_runtime as rt;

use crate::util::{
    MAX_VERSION_BYTES, ShellOutput, printable_version, service_name, write_output_linef,
};

use super::onboard::{self, SourceGateDecision};
use super::parse::{
    channel_name, maintenance_action_name, parse_channel, parse_ring, repo_sync_state_name,
    ring_name, trust_mode_name,
};

/// Progress phases reported by package-service; mirrors the service-side
/// model (five equal phases, percent = phase share + step share).
const PROGRESS_PHASES: u32 = 5;

/// Maintenance action word extending `PackageMaintenanceAction` for the
/// interrupted-update recovery flow; agreed with package-service.
const MAINTENANCE_ACTION_RECOVER: u64 = 4;

pub(super) struct MutationOptions<'a> {
    version: Option<&'a str>,
    source: Option<&'a str>,
    yes: bool,
    force_compat: bool,
}

pub(super) fn parse_mutation_options<'a, I>(parts: I) -> MutationOptions<'a>
where
    I: Iterator<Item = &'a str>,
{
    let mut options = MutationOptions {
        version: None,
        source: None,
        yes: false,
        force_compat: false,
    };
    for token in parts {
        if token == "--yes" {
            options.yes = true;
            continue;
        }
        if token == "--force-compat" {
            options.force_compat = true;
            continue;
        }
        match token.rsplit_once('@') {
            Some(("", source)) => options.source = Some(source),
            Some((version, source)) => {
                if !version.is_empty() && options.version.is_none() {
                    options.version = Some(version);
                }
                if !source.is_empty() {
                    options.source = Some(source);
                }
            }
            None => {
                if options.version.is_none() {
                    options.version = Some(token);
                }
            }
        }
    }
    options
}

fn compose_version_argument<'a>(
    buffer: &'a mut [u8],
    version: Option<&str>,
    source: Option<&str>,
) -> Option<&'a str> {
    if version.is_none() && source.is_none() {
        return None;
    }
    let mut len = 0usize;
    if let Some(version_text) = version {
        for (target, byte) in buffer[len..].iter_mut().zip(version_text.as_bytes()) {
            *target = *byte;
            len += 1;
        }
    }
    if let Some(source) = source {
        if len < buffer.len() {
            buffer[len] = b'@';
            len += 1;
        }
        for (target, byte) in buffer[len..].iter_mut().zip(source.as_bytes()) {
            *target = *byte;
            len += 1;
        }
    }
    core::str::from_utf8(&buffer[..len]).ok()
}

fn mutation_request(
    request_tag: u32,
    service_id: ServiceId,
    argument: Option<&str>,
) -> rt::Result<rt::RawMessage> {
    let mut request = rt::RawMessage::empty(request_tag);
    let bytes = argument.unwrap_or("").as_bytes();
    request.word_count = 2 + rt::pack_bytes(bytes, &mut request.words[2..])?;
    request.words[0] = service_id as u32 as u64;
    request.words[1] = bytes.len() as u64;
    Ok(request)
}

fn simple_request(request_tag: u32, word0: u64) -> rt::RawMessage {
    let mut request = rt::RawMessage::empty(request_tag);
    request.word_count = 1;
    request.words[0] = word0;
    request
}

fn phase_name(phase: u32) -> &'static str {
    match phase {
        0 => "resolve",
        1 => "materialize",
        2 => "verify",
        3 => "activate",
        4 => "persist",
        _ => "unknown",
    }
}

/// Whole-operation percent across five equally weighted phases.
fn progress_percent(phase: u32, step: u32, total: u32) -> u32 {
    if phase >= PROGRESS_PHASES || total == 0 {
        return 0;
    }
    let per_phase = 100 / PROGRESS_PHASES;
    phase * per_phase + step.min(total) * per_phase / total
}

fn status_from_word(word: u64) -> PackageStatus {
    match word as u32 {
        0 => PackageStatus::Ok,
        1 => PackageStatus::NotFound,
        2 => PackageStatus::AlreadyInstalled,
        3 => PackageStatus::NotInstalled,
        4 => PackageStatus::Busy,
        5 => PackageStatus::Denied,
        6 => PackageStatus::IntegrityFailed,
        7 => PackageStatus::End,
        8 => PackageStatus::NoChange,
        9 => PackageStatus::NoRollback,
        10 => PackageStatus::Unsupported,
        11 => PackageStatus::Offline,
        12 => PackageStatus::Interrupted,
        _ => PackageStatus::VerificationFailed,
    }
}

fn package_status_name(status: PackageStatus) -> &'static str {
    match status {
        PackageStatus::Ok => "ok",
        PackageStatus::NotFound => "not-found",
        PackageStatus::AlreadyInstalled => "already-installed",
        PackageStatus::NotInstalled => "not-installed",
        PackageStatus::Busy => "busy",
        PackageStatus::Denied => "denied",
        PackageStatus::IntegrityFailed => "integrity-failed",
        PackageStatus::End => "end",
        PackageStatus::NoChange => "no-change",
        PackageStatus::NoRollback => "no-rollback",
        PackageStatus::Unsupported => "unsupported",
        PackageStatus::Offline => "offline",
        PackageStatus::Interrupted => "interrupted",
        PackageStatus::VerificationFailed => "verification-failed",
    }
}

fn trust_explanation(mode: rt::PackageRepositoryTrustMode) -> &'static str {
    match mode {
        rt::PackageRepositoryTrustMode::Boot => "verified against the boot image",
        rt::PackageRepositoryTrustMode::PinnedDigest => "contents checked against a pinned digest",
        rt::PackageRepositoryTrustMode::Unsigned => "UNSIGNED - contents are not verified",
    }
}

pub(super) struct SourceInfo {
    pub(super) url_len: usize,
    pub(super) url_bytes: [u8; MAX_VERSION_BYTES],
    pub(super) trust_mode: rt::PackageRepositoryTrustMode,
    pub(super) sync_state: rt::PackageRepositorySyncState,
    pub(super) enabled: bool,
    pub(super) pinned_digest: u64,
}

pub(super) fn find_source_repo(
    package_handle: rt::Handle,
    source: &str,
) -> rt::Result<Option<SourceInfo>> {
    let mut name = [0u8; MAX_VERSION_BYTES];
    let mut url = [0u8; MAX_VERSION_BYTES];
    let mut index = 0usize;
    while let Some(repo) = rt::package_repository_list(package_handle, index, &mut name, &mut url)?
    {
        if core::str::from_utf8(&name[..repo.name_len])
            .map(|text| text == source)
            .unwrap_or(false)
        {
            return Ok(Some(SourceInfo {
                url_len: repo.url_len,
                url_bytes: url,
                trust_mode: repo.trust_mode,
                sync_state: repo.sync_state,
                enabled: repo.enabled,
                pinned_digest: repo.pinned_digest,
            }));
        }
        index += 1;
    }
    Ok(None)
}

/// Print the trust preview for an explicitly selected install/update source.
/// Returns false when the source is unknown or disabled.
fn show_source_trust(
    output: ShellOutput,
    package_handle: rt::Handle,
    source: &str,
) -> rt::Result<bool> {
    let Some(repo) = find_source_repo(package_handle, source)? else {
        write_output_linef(
            output,
            format_args!("unknown source {}; see pkg repos", source),
        )?;
        return Ok(false);
    };
    let url_text = core::str::from_utf8(&repo.url_bytes[..repo.url_len])
        .map_err(|_| rt::Error::InvalidArgument)?;
    write_output_linef(
        output,
        format_args!(
            "source {} url={} trust={} ({})",
            source,
            url_text,
            trust_mode_name(repo.trust_mode),
            trust_explanation(repo.trust_mode),
        ),
    )?;
    if repo.trust_mode == rt::PackageRepositoryTrustMode::PinnedDigest {
        write_output_linef(
            output,
            format_args!("source digest {:016x}", repo.pinned_digest),
        )?;
    }
    if !repo.enabled {
        write_output_linef(output, format_args!("source {} is disabled", source))?;
        return Ok(false);
    }
    if repo.sync_state != rt::PackageRepositorySyncState::Ready {
        write_output_linef(
            output,
            format_args!(
                "note: source sync state {} (pkg repo sync refreshes it)",
                repo_sync_state_name(repo.sync_state),
            ),
        )?;
    }
    Ok(true)
}

pub(super) fn cmd_pkg_install<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
    parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    run_mutation(
        bootstrap,
        output,
        service_id,
        parts,
        rt::PackageTag::InstallRequest as u32,
        rt::PackageTag::InstallReply as u32,
        "installed",
    )
}

pub(super) fn cmd_pkg_update<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
    parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    run_mutation(
        bootstrap,
        output,
        service_id,
        parts,
        rt::PackageTag::UpdateRequest as u32,
        rt::PackageTag::UpdateReply as u32,
        "updated",
    )
}

/// Resolve the install/update candidate version: the explicit argument when
/// given, otherwise package-service's latest catalog version.
fn candidate_version_text<'a>(
    package_handle: rt::Handle,
    service_id: ServiceId,
    explicit: Option<&'a str>,
    latest_buffer: &'a mut [u8; MAX_VERSION_BYTES],
) -> Option<&'a str> {
    if let Some(version) = explicit {
        return Some(version);
    }
    let mut installed = [0u8; MAX_VERSION_BYTES];
    let mut active = [0u8; MAX_VERSION_BYTES];
    let mut rollback = [0u8; MAX_VERSION_BYTES];
    let info = rt::package_info(
        package_handle,
        service_id,
        &mut installed,
        &mut active,
        &mut rollback,
        latest_buffer,
    )
    .ok()?;
    let text = core::str::from_utf8(&latest_buffer[..info.latest_version_len]).ok()?;
    Some(text)
}

fn compat_gate(
    output: ShellOutput,
    package_handle: rt::Handle,
    service_id: ServiceId,
    explicit_version: Option<&str>,
    force_compat: bool,
) -> rt::Result<bool> {
    let mut latest_buffer = [0u8; MAX_VERSION_BYTES];
    let Some(candidate) =
        candidate_version_text(package_handle, service_id, explicit_version, &mut latest_buffer)
    else {
        return Ok(true);
    };
    let verdict = onboard::compat_verdict(candidate);
    if !onboard::compat_requires_override(&verdict) {
        return Ok(true);
    }
    let onboard::CompatVerdict::Mismatch { declared } = verdict else {
        return Ok(true);
    };
    if !force_compat {
        write_output_linef(
            output,
            format_args!(
                "compat warning: package target {declared} does not match host {}",
                onboard::HOST_ARCH,
            ),
        )?;
        return write_output_linef(
            output,
            format_args!("blocked by compatibility policy; re-run with --force-compat to override"),
        )
        .map(|_| false);
    }
    write_output_linef(
        output,
        format_args!(
            "proceeding with --force-compat: package target {declared} on host {} may fail to run",
            onboard::HOST_ARCH,
        ),
    )?;
    Ok(true)
}

fn run_mutation<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
    parts: I,
    request_tag: u32,
    reply_tag: u32,
    verb: &'static str,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    let options = parse_mutation_options(parts);
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut argument_buffer = [0u8; 2 * MAX_VERSION_BYTES];

    if let Some(source) = options.source {
        if !show_source_trust(output, package_handle, source)? {
            let _ = rt::handle_close(package_handle);
            return Ok(());
        }
        if let SourceGateDecision::BlockedDisabled = onboard::source_gate_decision(onboard::onboard_lookup(source)) {
            write_output_linef(
                output,
                format_args!(
                    "blocked: source {source} was disabled by operator review; re-enable with pkg repo enable {source}"
                ),
            )?;
            let _ = rt::handle_close(package_handle);
            return Ok(());
        }
        if !options.yes {
            // Confirmation gate: the operator must acknowledge the chosen
            // source's trust state before a non-boot-trusted install runs.
            let _ = write_output_linef(
                output,
                format_args!("review the source above; re-run with --yes to proceed"),
            );
            let _ = rt::handle_close(package_handle);
            return Ok(());
        }
    }

    if !compat_gate(
        output,
        package_handle,
        service_id,
        options.version,
        options.force_compat,
    )? {
        let _ = rt::handle_close(package_handle);
        return Ok(());
    }

    let argument = compose_version_argument(&mut argument_buffer, options.version, options.source);
    let mut request = mutation_request(request_tag, service_id, argument)?;
    let reply = rt::channel_call(package_handle, &mut request);
    let _ = rt::handle_close(package_handle);
    let reply = reply?;
    if reply.tag != reply_tag || reply.word_count < 1 {
        return Err(rt::Error::InvalidArgument);
    }
    let status = status_from_word(reply.words[0]);
    if status != PackageStatus::Ok {
        return write_output_linef(
            output,
            format_args!("{} failed: {}", verb, package_status_name(status),),
        );
    }
    let (phase, step, total) = decode_progress(&reply);
    write_output_linef(
        output,
        format_args!(
            "{} {} ({}/{} steps, {} {}%)",
            verb,
            service_name(service_id),
            step,
            total,
            phase_name(phase),
            progress_percent(phase, step, total),
        ),
    )
}

fn decode_progress(reply: &rt::RawMessage) -> (u32, u32, u32) {
    if reply.word_count < 4 {
        return (0, 0, 0);
    }
    let word = reply.words[3];
    (
        (word & 0xff) as u32,
        ((word >> 8) & 0xffff) as u32,
        ((word >> 24) & 0xffff) as u32,
    )
}

pub(super) fn cmd_pkg_remove(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
) -> rt::Result<()> {
    let name = service_name(service_id);
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;

    // Pre-remove snapshot: what is being taken away.
    let mut installed = [0u8; MAX_VERSION_BYTES];
    let mut active = [0u8; MAX_VERSION_BYTES];
    let mut rollback = [0u8; MAX_VERSION_BYTES];
    let mut latest = [0u8; MAX_VERSION_BYTES];
    let pre = rt::package_info(
        package_handle,
        service_id,
        &mut installed,
        &mut active,
        &mut rollback,
        &mut latest,
    )
    .ok();
    let removed_version = pre.as_ref().and_then(|info| {
        core::str::from_utf8(&installed[..info.installed_version_len])
            .ok()
            .map(printable_version)
    });
    let was_active = pre.as_ref().is_some_and(|info| info.active);

    let result = rt::package_remove(package_handle, service_id);

    // Post-remove state drives the cleanup summary (rollback slot retention).
    let post = if result.is_ok() {
        let mut post_installed = [0u8; MAX_VERSION_BYTES];
        let mut post_active = [0u8; MAX_VERSION_BYTES];
        let mut post_rollback = [0u8; MAX_VERSION_BYTES];
        let mut post_latest = [0u8; MAX_VERSION_BYTES];
        match rt::package_info(
            package_handle,
            service_id,
            &mut post_installed,
            &mut post_active,
            &mut post_rollback,
            &mut post_latest,
        ) {
            Ok(info) => {
                let mut version = rt::FixedLogBuffer::<MAX_VERSION_BYTES>::new();
                if let Ok(text) = core::str::from_utf8(&post_rollback[..info.rollback_version_len])
                {
                    let _ = core::fmt::Write::write_fmt(
                        &mut version,
                        format_args!("{}", printable_version(text)),
                    );
                }
                Some((info.rollback_available, version))
            }
            Err(_) => None,
        }
    } else {
        None
    };
    let _ = rt::handle_close(package_handle);
    result?;

    write_output_linef(output, format_args!("removed {}", name))?;
    write_output_linef(
        output,
        format_args!(
            "  cleanup: version={} deactivated={} journal=cleared",
            removed_version.unwrap_or("-"),
            if was_active { "yes" } else { "not-running" },
        ),
    )?;
    match post {
        Some((true, version)) if !version.as_str().is_empty() && version.as_str() != "-" => {
            write_output_linef(
                output,
                format_args!(
                    "  cleanup: rollback-slot=retained ({}) ; reclaim storage: pkg gc",
                    version.as_str()
                ),
            )
        }
        Some((true, _)) => write_output_linef(
            output,
            format_args!("  cleanup: rollback-slot=retained ; reclaim storage: pkg gc"),
        ),
        _ => write_output_linef(
            output,
            format_args!("  cleanup: rollback-slot=none ; reclaim storage: pkg gc"),
        ),
    }
}

pub(super) fn cmd_pkg_rollback(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;

    // Capture the pre-rollback state so the summary can show what moved.
    let mut installed_version = [0u8; MAX_VERSION_BYTES];
    let mut active_version = [0u8; MAX_VERSION_BYTES];
    let mut rollback_version = [0u8; MAX_VERSION_BYTES];
    let mut latest_version = [0u8; MAX_VERSION_BYTES];
    let mut source_buffer = [0u8; MAX_VERSION_BYTES];
    let provenance = rt::package_provenance(
        package_handle,
        service_id,
        &mut installed_version,
        &mut active_version,
        &mut rollback_version,
        &mut latest_version,
        &mut source_buffer,
    );

    let mut request = simple_request(
        rt::PackageTag::RollbackRequest as u32,
        service_id as u32 as u64,
    );
    let reply = rt::channel_call(package_handle, &mut request);
    let _ = rt::handle_close(package_handle);
    let reply = reply?;
    if reply.tag != rt::PackageTag::RollbackReply as u32 || reply.word_count < 6 {
        return Err(rt::Error::InvalidArgument);
    }
    let status = status_from_word(reply.words[0]);
    if status != PackageStatus::Ok {
        return write_output_linef(
            output,
            format_args!("rollback failed: {}", package_status_name(status)),
        );
    }

    // Summary: previous active version -> restored version. Prefer the
    // service-reported words; fall back to the provenance snapshot.
    let previous = version_word_text(reply.words[4]).or_else(|| {
        provenance
            .as_ref()
            .ok()
            .map(|_| decode_version_buffer(&active_version))
    });
    let restored = version_word_text(reply.words[5]).or_else(|| {
        provenance
            .as_ref()
            .ok()
            .map(|_| decode_version_buffer(&rollback_version))
    });
    match (previous, restored) {
        (Some((from_major, from_minor, from_patch)), Some((to_major, to_minor, to_patch))) => {
            write_output_linef(
                output,
                format_args!(
                    "rolled back {} {}.{}.{} -> {}.{}.{} (trigger: operator)",
                    service_name(service_id),
                    from_major,
                    from_minor,
                    from_patch,
                    to_major,
                    to_minor,
                    to_patch,
                ),
            )
        }
        _ => write_output_linef(
            output,
            format_args!(
                "rolled back {} (trigger: operator)",
                service_name(service_id)
            ),
        ),
    }
}

fn version_word_text(word: u64) -> Option<(u64, u64, u64)> {
    if word == 0 {
        return None;
    }
    Some((word >> 32, (word >> 16) & 0xffff, word & 0xffff))
}

fn decode_version_buffer(buffer: &[u8]) -> (u64, u64, u64) {
    let text = core::str::from_utf8(buffer).unwrap_or("");
    let mut parts = text.split('.');
    let major = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (major, minor, patch)
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
    maintenance_call(
        bootstrap,
        output,
        action as u32 as u64,
        maintenance_action_name(action),
    )
}

/// Interrupted-update recovery: resume or discard the stale journal entry
/// detected at startup, reporting exactly what happened.
pub(super) fn cmd_pkg_recover(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    maintenance_call(bootstrap, output, MAINTENANCE_ACTION_RECOVER, "recover")
}

fn maintenance_call(
    bootstrap: rt::Handle,
    output: ShellOutput,
    action_word: u64,
    action_label: &'static str,
) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut request = simple_request(rt::PackageTag::MaintenanceRequest as u32, action_word);
    let reply = rt::channel_call(package_handle, &mut request);
    let _ = rt::handle_close(package_handle);
    let reply = reply?;
    if reply.tag != rt::PackageTag::MaintenanceReply as u32 || reply.word_count < 3 {
        return Err(rt::Error::InvalidArgument);
    }
    let status = status_from_word(reply.words[0]);
    match status {
        PackageStatus::Ok | PackageStatus::NoChange | PackageStatus::Interrupted => {}
        other => {
            return write_output_linef(
                output,
                format_args!("{} failed: {}", action_label, package_status_name(other)),
            );
        }
    }

    let repaired = reply.words[1] as u32;
    let collected = reply.words[2] as u32;
    write_output_linef(
        output,
        format_args!(
            "{} repaired={} collected={}",
            action_label, repaired, collected,
        ),
    )?;

    if reply.word_count >= 9 {
        render_journal_status(output, &reply)?;
    }
    Ok(())
}

/// Operator-visible journal/recovery status appended to maintenance replies:
/// [.., pending_action, service_id, journaled_version, stale_at_boot,
///  outcome, name_len, <package-name bytes>]
fn render_journal_status(output: ShellOutput, reply: &rt::RawMessage) -> rt::Result<()> {
    let pending_action = reply.words[3] as u32;
    let stale_at_boot = reply.words[6] as u32;
    if pending_action == 0 && stale_at_boot == 0 {
        return write_output_linef(output, format_args!("journal: clean"));
    }

    // Decode the inline package name.
    let name_len = reply.words[8] as usize;
    let mut name_bytes = [0u8; MAX_VERSION_BYTES];
    let decoded_len = name_len.min(name_bytes.len());
    if decoded_len > 0 {
        rt::unpack_bytes(
            &reply.words[9..reply.word_count as usize],
            decoded_len,
            &mut name_bytes,
        )?;
    }
    let name_text = core::str::from_utf8(&name_bytes[..decoded_len]).unwrap_or("?");

    if pending_action == 0 {
        return write_output_linef(
            output,
            format_args!(
                "journal: clean (interrupted {} detected at boot was handled)",
                journal_action_label(stale_at_boot),
            ),
        );
    }

    let (major, minor, patch) = version_word_text(reply.words[5]).unwrap_or((0, 0, 0));
    write_output_linef(
        output,
        format_args!(
            "journal: stale {} {} {}.{}.{} (stale since boot)",
            journal_action_label(pending_action),
            name_text,
            major,
            minor,
            patch,
        ),
    )?;
    match reply.words[7] {
        1 => write_output_linef(
            output,
            format_args!("recovery: resumed the journaled operation"),
        ),
        2 => write_output_linef(output, format_args!("recovery: journal discarded")),
        3 => write_output_linef(
            output,
            format_args!("recovery: resume failed; journal discarded"),
        ),
        _ => write_output_linef(
            output,
            format_args!("recovery: run pkg recover to resume or discard"),
        ),
    }
}

fn journal_action_label(action: u32) -> &'static str {
    match action {
        1 => "install",
        2 => "update",
        3 => "remove",
        4 => "rollback",
        _ => "none",
    }
}

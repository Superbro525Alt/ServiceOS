use super::*;

pub(crate) fn sort_package_versions(slot: &mut PackageSlot) {
    let mut index = 1usize;
    while index < slot.version_count {
        let mut inner = index;
        while inner > 0
            && compare_versions(version_text(slot, inner - 1), version_text(slot, inner))
                == Ordering::Greater
        {
            slot.versions.swap(inner - 1, inner);
            inner -= 1;
        }
        index += 1;
    }
}

pub(crate) fn compare_versions(left: &str, right: &str) -> Ordering {
    parse_version_triplet(left).cmp(&parse_version_triplet(right))
}

fn parse_version_triplet(value: &str) -> (u32, u32, u32) {
    let mut parts = value.split('.');
    let major = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or(0);
    let minor = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or(0);
    let patch = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or(0);
    (major, minor, patch)
}

pub(crate) fn latest_version_index(slot: &PackageSlot) -> Option<usize> {
    if slot.version_count == 0 {
        None
    } else {
        Some(slot.version_count - 1)
    }
}

pub(crate) fn find_version_by_name(slot: &PackageSlot, version: &str) -> Option<usize> {
    slot.versions[..slot.version_count]
        .iter()
        .position(|entry| entry.occupied && entry.version.as_str().ok() == Some(version))
}

pub(crate) fn version_text(slot: &PackageSlot, index: usize) -> &str {
    slot.versions[index].version.as_str().unwrap_or("")
}

pub(crate) fn version_bytes(slot: &PackageSlot, index: Option<usize>) -> &[u8] {
    index
        .and_then(|index| slot.versions.get(index))
        .and_then(|slot| slot.version.as_str().ok())
        .map(|value| value.as_bytes())
        .unwrap_or(&[])
}

pub(crate) fn package_flags(slot: &PackageSlot) -> u32 {
    u32::from(slot.installed.is_some())
        | (u32::from(slot.active.is_some()) << 1)
        | (u32::from(slot.rollback.is_some()) << 2)
}

pub(crate) fn pack_repo_flags(repo: RepositorySlot) -> u32 {
    (repo.trust_mode as u32)
        | ((repo.sync_state as u32) << 8)
        | ((repo.channel as u32) << 16)
        | ((repo.ring as u32) << 24)
        | ((u32::from(repo.enabled)) << 30)
        | ((u32::from(repo.builtin)) << 31)
}

pub(crate) fn pack_provenance_flags(
    trust: PackageTrustState,
    channel: PackageChannel,
    ring: PackageRing,
    package_flags: u32,
) -> u32 {
    trust as u32 | ((channel as u32) << 8) | ((ring as u32) << 16) | (package_flags << 24)
}

pub(crate) fn resolve_source_index(
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    repo_count: usize,
    source: Option<&str>,
) -> Result<Option<usize>, PackageStatus> {
    let Some(name) = source else {
        return Ok(None);
    };
    let index = find_repository_index(repos, repo_count, name).ok_or(PackageStatus::NotFound)?;
    if !repos[index].enabled {
        return Err(PackageStatus::Denied);
    }
    Ok(Some(index))
}

pub(crate) fn select_install_target(
    slot: &PackageSlot,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    _repo_count: usize,
    explicit_version: Option<&str>,
    resolved_source: Option<usize>,
) -> rt::Result<usize> {
    if let Some(version) = explicit_version {
        let index = find_version_by_name(slot, version).ok_or(rt::Error::NotFound)?;
        let entry = &slot.versions[index];
        if !ops_model::source_permits(resolved_source, entry.repo_index, entry.occupied) {
            return Err(rt::Error::NotFound);
        }
        return Ok(index);
    }
    if resolved_source.is_none() {
        if let Ok(pin) = slot.pin_version.as_str() {
            if !pin.is_empty() {
                return find_version_by_name(slot, pin).ok_or(rt::Error::NotFound);
            }
        }
    }
    for index in (0..slot.version_count).rev() {
        let entry = &slot.versions[index];
        if ops_model::source_permits(resolved_source, entry.repo_index, entry.occupied)
            && version_allowed(slot, entry, repos)
        {
            return Ok(index);
        }
    }
    Err(rt::Error::NotFound)
}

pub(crate) fn select_update_target(
    slot: &PackageSlot,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    repo_count: usize,
    explicit_version: Option<&str>,
    resolved_source: Option<usize>,
) -> rt::Result<Option<usize>> {
    let Some(current) = slot.installed else {
        return Ok(None);
    };
    let target = select_install_target(slot, repos, repo_count, explicit_version, resolved_source)?;
    if target == current {
        Ok(None)
    } else if compare_versions(version_text(slot, target), version_text(slot, current))
        == Ordering::Greater
        || explicit_version.is_some()
        || resolved_source.is_some()
    {
        Ok(Some(target))
    } else {
        Ok(None)
    }
}

pub(crate) fn version_allowed(
    slot: &PackageSlot,
    version: &PackageVersionSlot,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
) -> bool {
    let Some(repo) = repos
        .get(version.repo_index)
        .copied()
        .filter(|repo| repo.occupied)
    else {
        return false;
    };
    channel_rank(repo.channel) <= channel_rank(slot.channel)
        && ring_rank(repo.ring) <= ring_rank(slot.ring)
}

fn channel_rank(channel: PackageChannel) -> u32 {
    match channel {
        PackageChannel::Stable => 0,
        PackageChannel::Beta => 1,
        PackageChannel::Canary => 2,
    }
}

fn ring_rank(ring: PackageRing) -> u32 {
    match ring {
        PackageRing::Production => 0,
        PackageRing::Preview => 1,
        PackageRing::Testing => 2,
    }
}

pub(crate) fn find_package_slot(
    packages: &[PackageSlot; MAX_PACKAGE_SLOTS],
    service_id: ServiceId,
    package_count: usize,
) -> Option<usize> {
    (0..package_count)
        .find(|index| packages[*index].occupied && packages[*index].service_id == service_id)
}

pub(crate) fn find_repository_index(
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    repo_count: usize,
    name: &str,
) -> Option<usize> {
    (0..repo_count)
        .find(|index| repos[*index].occupied && repos[*index].name.as_str().ok() == Some(name))
}

pub(crate) fn total_versions(
    packages: &[PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
) -> usize {
    packages[..package_count]
        .iter()
        .filter(|slot| slot.occupied)
        .map(|slot| slot.version_count)
        .sum()
}

pub(crate) fn active_manifest_path(version: &PackageVersionSlot) -> &str {
    version
        .local_manifest_path
        .as_str()
        .ok()
        .filter(|path| !path.is_empty())
        .or_else(|| version.repo_manifest_path.as_str().ok())
        .unwrap_or("")
}

pub(crate) fn emit_package_event(
    log_handle: rt::Handle,
    severity: LogSeverity,
    event: LogEvent,
    arg0: u64,
    arg1: u64,
) -> rt::Result<()> {
    rt::send_log_record(
        log_handle,
        ServiceId::Package,
        severity,
        LogDomain::Package,
        event,
        arg0,
        arg1,
    )
}

pub(crate) fn parse_version_argument<'a>(
    message: &RawMessage,
    buffer: &'a mut [u8],
) -> rt::Result<Option<&'a str>> {
    let version_len = message.words[1] as usize;
    if version_len == 0 {
        return Ok(None);
    }
    unpack_bytes(
        &message.words[2..message.word_count as usize],
        version_len,
        buffer,
    )?;
    let text =
        core::str::from_utf8(&buffer[..version_len]).map_err(|_| rt::Error::InvalidArgument)?;
    Ok(Some(text))
}

pub(crate) fn send_status_reply(
    reply_handle: rt::Handle,
    tag: PackageTag,
    status: PackageStatus,
) -> rt::Result<()> {
    let mut reply = RawMessage::empty(tag as u32);
    reply.word_count = 1;
    reply.words[0] = status as u32 as u64;
    // Reply-send failures must not tear the service down; the requesting
    // client owns its wait timeout/hangup handling.
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

/// Operation reply with progress and summary payload appended. Extra words
/// are ignored by clients using the plain status wrappers.
/// Layout: [status, steps_done, steps_total, packed_progress,
///          aux0, aux1] where install/update use aux1 = trigger code and
/// rollback uses aux0 = previous version, aux1 = restored version.
pub(crate) fn send_operation_reply(
    reply_handle: rt::Handle,
    tag: PackageTag,
    status: PackageStatus,
    progress: &ops_model::ProgressTracker,
    aux0: u64,
    aux1: u64,
) -> rt::Result<()> {
    let mut reply = RawMessage::empty(tag as u32);
    reply.word_count = 6;
    reply.words[0] = status as u32 as u64;
    reply.words[1] = progress.step as u64;
    reply.words[2] = progress.total_steps as u64;
    reply.words[3] = progress.pack();
    reply.words[4] = aux0;
    reply.words[5] = aux1;
    // See send_status_reply: never propagate reply-send failures.
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

pub(crate) fn encode_version_text(version: &str) -> u64 {
    let (major, minor, patch) = parse_version_triplet(version);
    ((major as u64) << 32) | ((minor as u64) << 16) | patch as u64
}

pub(crate) fn copy_into(destination: &mut [u8], source: &[u8]) -> rt::Result<usize> {
    if source.len() > destination.len() {
        return Err(rt::Error::BufferTooSmall);
    }
    destination[..source.len()].copy_from_slice(source);
    Ok(source.len())
}

pub(crate) fn pack_bytes(source: &[u8], words: &mut [u64]) -> rt::Result<u32> {
    let required = source.len().div_ceil(8);
    if required > words.len() {
        return Err(rt::Error::BufferTooSmall);
    }
    for (index, chunk) in source.chunks(8).enumerate() {
        let mut bytes = [0u8; 8];
        bytes[..chunk.len()].copy_from_slice(chunk);
        words[index] = u64::from_le_bytes(bytes);
    }
    Ok(required as u32)
}

pub(crate) fn unpack_bytes(words: &[u64], len: usize, destination: &mut [u8]) -> rt::Result<()> {
    if len > destination.len() || len > words.len() * 8 {
        return Err(rt::Error::BufferTooSmall);
    }
    let mut copied = 0usize;
    for word in words.iter().copied() {
        if copied >= len {
            break;
        }
        let bytes = word.to_le_bytes();
        let chunk = (len - copied).min(bytes.len());
        destination[copied..copied + chunk].copy_from_slice(&bytes[..chunk]);
        copied += chunk;
    }
    Ok(())
}

pub(crate) fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
    let mut message = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut message) {
        Ok(()) if message.tag == ControlTag::Lifecycle as u32 && message.word_count > 0 => {
            Ok(matches!(
                lifecycle_event_from_word(message.words[0]),
                LifecycleEvent::Restarting | LifecycleEvent::Stopped
            ))
        }
        Ok(()) => Ok(false),
        Err(rt::Error::QueueEmpty) => Ok(false),
        Err(error) => Err(error),
    }
}

fn lifecycle_event_from_word(value: u64) -> LifecycleEvent {
    match value as u32 {
        x if x == LifecycleEvent::Starting as u32 => LifecycleEvent::Starting,
        x if x == LifecycleEvent::Ready as u32 => LifecycleEvent::Ready,
        x if x == LifecycleEvent::Failed as u32 => LifecycleEvent::Failed,
        x if x == LifecycleEvent::Stopped as u32 => LifecycleEvent::Stopped,
        _ => LifecycleEvent::Restarting,
    }
}

pub(crate) fn service_id_from_word(value: u64) -> ServiceId {
    match value as u32 {
        x if x == ServiceId::Storage as u32 => ServiceId::Storage,
        x if x == ServiceId::Console as u32 => ServiceId::Console,
        x if x == ServiceId::Config as u32 => ServiceId::Config,
        x if x == ServiceId::Log as u32 => ServiceId::Log,
        x if x == ServiceId::Status as u32 => ServiceId::Status,
        x if x == ServiceId::Shell as u32 => ServiceId::Shell,
        x if x == ServiceId::Package as u32 => ServiceId::Package,
        x if x == ServiceId::Announce as u32 => ServiceId::Announce,
        x if x == ServiceId::Network as u32 => ServiceId::Network,
        x if x == ServiceId::Graphics as u32 => ServiceId::Graphics,
        x if x == ServiceId::Session as u32 => ServiceId::Session,
        x if x == ServiceId::DesktopShell as u32 => ServiceId::DesktopShell,
        x if x == ServiceId::Terminal as u32 => ServiceId::Terminal,
        x if x == ServiceId::Audio as u32 => ServiceId::Audio,
        x if x == ServiceId::Runtime as u32 => ServiceId::Runtime,
        x if x == ServiceId::Developer as u32 => ServiceId::Developer,
        x if x == ServiceId::Clipboard as u32 => ServiceId::Clipboard,
        _ => ServiceId::RootManager,
    }
}

pub(crate) fn service_id_from_name(value: &str) -> Option<ServiceId> {
    Some(match value {
        "storage-service" | "storage" => ServiceId::Storage,
        "console-service" | "console" => ServiceId::Console,
        "config-service" | "config" => ServiceId::Config,
        "log-service" | "log" => ServiceId::Log,
        "status-service" | "status" => ServiceId::Status,
        "shell-service" | "shell" => ServiceId::Shell,
        "package-service" | "package" => ServiceId::Package,
        "announce-service" | "announce" => ServiceId::Announce,
        "network-service" | "network" => ServiceId::Network,
        "graphics-service" | "graphics" => ServiceId::Graphics,
        "session-service" | "session" => ServiceId::Session,
        "desktop-shell-service" | "desktop-shell" => ServiceId::DesktopShell,
        "terminal-service" | "terminal" => ServiceId::Terminal,
        "audio-service" | "audio" => ServiceId::Audio,
        "runtime-service" | "runtime" => ServiceId::Runtime,
        "developer-service" | "developer" => ServiceId::Developer,
        "clipboard-service" | "clipboard" => ServiceId::Clipboard,
        _ => return None,
    })
}

pub(crate) fn trust_mode_from_word(value: u64) -> PackageRepositoryTrustMode {
    match value as u32 {
        x if x == PackageRepositoryTrustMode::Boot as u32 => PackageRepositoryTrustMode::Boot,
        x if x == PackageRepositoryTrustMode::PinnedDigest as u32 => {
            PackageRepositoryTrustMode::PinnedDigest
        }
        _ => PackageRepositoryTrustMode::Unsigned,
    }
}

pub(crate) fn package_channel_from_word(value: u64) -> PackageChannel {
    match value as u32 {
        x if x == PackageChannel::Beta as u32 => PackageChannel::Beta,
        x if x == PackageChannel::Canary as u32 => PackageChannel::Canary,
        _ => PackageChannel::Stable,
    }
}

pub(crate) fn package_ring_from_word(value: u64) -> PackageRing {
    match value as u32 {
        x if x == PackageRing::Preview as u32 => PackageRing::Preview,
        x if x == PackageRing::Testing as u32 => PackageRing::Testing,
        _ => PackageRing::Production,
    }
}

pub(crate) fn maintenance_action_from_word(value: u64) -> PackageMaintenanceAction {
    match value as u32 {
        x if x == PackageMaintenanceAction::Repair as u32 => PackageMaintenanceAction::Repair,
        x if x == PackageMaintenanceAction::GarbageCollect as u32 => {
            PackageMaintenanceAction::GarbageCollect
        }
        _ => PackageMaintenanceAction::Validate,
    }
}

pub(crate) fn service_name(service_id: ServiceId) -> &'static str {
    match service_id {
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
        ServiceId::Terminal => "terminal-service",
        ServiceId::Audio => "audio-service",
        ServiceId::Runtime => "runtime-service",
        ServiceId::Developer => "developer-service",
        ServiceId::Clipboard => "clipboard-service",
        ServiceId::Security => "security-service",
        ServiceId::SetupWizard => "setup-wizard",
        ServiceId::Backup => "backup-service",
        ServiceId::RootManager => "root-manager",
    }
}

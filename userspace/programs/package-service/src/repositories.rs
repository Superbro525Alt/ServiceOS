use super::*;

pub(crate) fn initialize_builtin_repository(repo: &mut RepositorySlot) {
    let _ = repo.name.set("boot");
    let _ = repo.url.set("boot://packages/index.txt");
    repo.trust_mode = PackageRepositoryTrustMode::Boot;
    repo.sync_state = PackageRepositorySyncState::Ready;
    repo.channel = PackageChannel::Stable;
    repo.ring = PackageRing::Production;
    repo.enabled = true;
    repo.builtin = true;
    repo.occupied = true;
}

pub(crate) fn load_boot_catalog(
    storage_handle: rt::Handle,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
) -> rt::Result<usize> {
    let (index_handle, index_len) = rt::storage_open(storage_handle, "packages/index.txt")?;
    let mut index_buffer = [0u8; MAX_INDEX_BYTES];
    let requested = index_len.min(index_buffer.len());
    let loaded = rt::storage_read_all(index_handle, &mut index_buffer, requested)?;
    let _ = rt::storage_blob_close(index_handle);

    let index_text =
        core::str::from_utf8(&index_buffer[..loaded]).map_err(|_| rt::Error::InvalidArgument)?;
    let mut count = 0usize;
    for line in index_text
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
    {
        let (manifest_handle, manifest_len) = rt::storage_open(storage_handle, line)?;
        let mut manifest_buffer = [0u8; MAX_PACKAGE_BYTES];
        let requested = manifest_len.min(manifest_buffer.len());
        let loaded = rt::storage_read_all(manifest_handle, &mut manifest_buffer, requested)?;
        let _ = rt::storage_blob_close(manifest_handle);
        let manifest = parse_package_manifest(&manifest_buffer[..loaded])
            .map_err(|_| rt::Error::InvalidArgument)?;
        let latest = add_or_update_version(
            packages,
            &mut count,
            manifest.service_id,
            manifest.package.as_str().unwrap_or("package"),
            manifest.version.as_str().unwrap_or("0.0.0"),
            manifest
                .compatibility
                .as_str()
                .unwrap_or("serviceos.bootstore.v1"),
            line,
            "",
            manifest.package.as_str().unwrap_or("SYSTEM"),
            manifest.package.as_str().unwrap_or("SERVICE PACKAGE"),
            BUILTIN_REPOSITORY_INDEX,
            PackageTrustState::BootTrusted,
            Some(&manifest),
            None,
            repos[BUILTIN_REPOSITORY_INDEX].channel,
            repos[BUILTIN_REPOSITORY_INDEX].ring,
        )?;
        let index = find_package_slot(packages, manifest.service_id, count).unwrap();
        packages[index].versions[latest].manifest_loaded = true;
    }
    Ok(count)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn add_or_update_version(
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: &mut usize,
    service_id: ServiceId,
    package_name: &str,
    version: &str,
    compatibility: &str,
    repo_manifest_path: &str,
    local_manifest_path: &str,
    category: &str,
    summary: &str,
    repo_index: usize,
    trust_state: PackageTrustState,
    manifest: Option<&PackageManifest>,
    pin_version: Option<&str>,
    channel: PackageChannel,
    ring: PackageRing,
) -> rt::Result<usize> {
    let slot_index = if let Some(index) = find_package_slot(packages, service_id, *package_count) {
        index
    } else {
        if *package_count == packages.len() {
            return Err(rt::Error::CapacityExceeded);
        }
        let index = *package_count;
        let slot = &mut packages[index];
        *slot = PackageSlot::empty();
        slot.service_id = service_id;
        let _ = slot.package_name.set(package_name);
        slot.channel = channel;
        slot.ring = ring;
        slot.occupied = true;
        *package_count += 1;
        index
    };

    if let Some(existing) = find_version_by_name(&packages[slot_index], version) {
        let version_slot = &mut packages[slot_index].versions[existing];
        let _ = version_slot.version.set(version);
        let _ = version_slot.compatibility.set(compatibility);
        let _ = version_slot.repo_manifest_path.set(repo_manifest_path);
        if !local_manifest_path.is_empty() {
            let _ = version_slot.local_manifest_path.set(local_manifest_path);
        }
        let _ = version_slot.category.set(category);
        let _ = version_slot.summary.set(summary);
        version_slot.repo_index = repo_index;
        version_slot.trust_state = trust_state;
        if let Some(manifest) = manifest {
            version_slot.manifest = *manifest;
            version_slot.manifest_loaded = true;
        }
        if let Some(pin) = pin_version {
            let _ = packages[slot_index].pin_version.set(pin);
        }
        packages[slot_index].channel = channel;
        packages[slot_index].ring = ring;
        return Ok(existing);
    }

    let slot = &mut packages[slot_index];
    if slot.version_count == slot.versions.len() {
        return Err(rt::Error::CapacityExceeded);
    }
    let version_index = slot.version_count;
    let version_slot = &mut slot.versions[version_index];
    *version_slot = PackageVersionSlot::empty();
    let _ = version_slot.version.set(version);
    let _ = version_slot.compatibility.set(compatibility);
    let _ = version_slot.repo_manifest_path.set(repo_manifest_path);
    let _ = version_slot.local_manifest_path.set(local_manifest_path);
    let _ = version_slot.category.set(category);
    let _ = version_slot.summary.set(summary);
    version_slot.repo_index = repo_index;
    version_slot.trust_state = trust_state;
    version_slot.occupied = true;
    if let Some(manifest) = manifest {
        version_slot.manifest = *manifest;
        version_slot.manifest_loaded = true;
    }
    slot.version_count += 1;
    if let Some(pin) = pin_version {
        let _ = slot.pin_version.set(pin);
    }
    slot.channel = channel;
    slot.ring = ring;
    sort_package_versions(slot);
    find_version_by_name(slot, version).ok_or(rt::Error::NotFound)
}

fn add_repository(
    storage_handle: rt::Handle,
    log_handle: rt::Handle,
    repos: &mut [RepositorySlot; MAX_REPOSITORIES],
    repo_count: &mut usize,
    name: &str,
    url: &str,
    trust_mode: PackageRepositoryTrustMode,
    channel: PackageChannel,
    ring: PackageRing,
    enabled: bool,
    pinned_digest: u64,
) -> rt::Result<PackageStatus> {
    if *repo_count == repos.len() {
        return Ok(PackageStatus::Busy);
    }
    if find_repository_index(repos, *repo_count, name).is_some() {
        return Ok(PackageStatus::AlreadyInstalled);
    }
    if parse_http_url(url).is_err() {
        return Ok(PackageStatus::Unsupported);
    }
    let index = *repo_count;
    let mut repo = RepositorySlot::empty();
    let _ = repo.name.set(name);
    let _ = repo.url.set(url);
    repo.trust_mode = trust_mode;
    repo.sync_state = PackageRepositorySyncState::Idle;
    repo.channel = channel;
    repo.ring = ring;
    repo.enabled = enabled;
    repo.pinned_digest = pinned_digest;
    repo.occupied = true;
    repos[index] = repo;
    *repo_count += 1;
    crate::storage::persist_repositories(storage_handle, repos, *repo_count)?;
    let _ = emit_package_event(
        log_handle,
        LogSeverity::Info,
        LogEvent::PackageRepositoryAdded,
        index as u64,
        trust_mode as u32 as u64,
    );
    Ok(PackageStatus::Ok)
}

pub(crate) fn sync_repository(
    storage_handle: rt::Handle,
    network_handle: rt::Handle,
    log_handle: rt::Handle,
    repos: &mut [RepositorySlot; MAX_REPOSITORIES],
    repo_index: usize,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: &mut usize,
) -> rt::Result<PackageStatus> {
    if repo_index >= repos.len() || !repos[repo_index].occupied || repos[repo_index].builtin {
        return Ok(PackageStatus::NotFound);
    }
    let url = repos[repo_index]
        .url
        .as_str()
        .map_err(|_| rt::Error::InvalidArgument)?;
    let mut bytes = [0u8; MAX_FEED_BYTES];
    let loaded = match http_fetch_text(network_handle, url, &mut bytes) {
        Ok(len) => len,
        Err(_) => {
            repos[repo_index].sync_state = PackageRepositorySyncState::Offline;
            let _ = emit_package_event(
                log_handle,
                LogSeverity::Warn,
                LogEvent::PackageRepositorySyncFailed,
                repo_index as u64,
                0,
            );
            crate::storage::persist_repositories(storage_handle, repos, count_repositories(repos))?;
            return Ok(PackageStatus::Offline);
        }
    };
    let digest = crate::operations::compute_fnv64(&bytes[..loaded]);
    let trust_state = match repos[repo_index].trust_mode {
        PackageRepositoryTrustMode::Boot => PackageTrustState::BootTrusted,
        PackageRepositoryTrustMode::Unsigned => PackageTrustState::Unverified,
        PackageRepositoryTrustMode::PinnedDigest => {
            if repos[repo_index].pinned_digest == digest {
                PackageTrustState::DigestPinned
            } else {
                repos[repo_index].sync_state = PackageRepositorySyncState::Failed;
                let _ = emit_package_event(
                    log_handle,
                    LogSeverity::Error,
                    LogEvent::PackageRepositorySyncFailed,
                    repo_index as u64,
                    digest,
                );
                crate::storage::persist_repositories(
                    storage_handle,
                    repos,
                    count_repositories(repos),
                )?;
                return Ok(PackageStatus::VerificationFailed);
            }
        }
    };

    remove_versions_for_repo(packages, *package_count, repo_index);
    repos[repo_index].package_count = 0;
    let base_path = repository_base_path(url);
    parse_feed_catalog(
        &bytes[..loaded],
        repos,
        repo_index,
        packages,
        package_count,
        trust_state,
        base_path.as_str(),
    )?;
    repos[repo_index].last_digest = digest;
    repos[repo_index].sync_state = PackageRepositorySyncState::Ready;
    crate::storage::persist_repositories(storage_handle, repos, count_repositories(repos))?;
    crate::storage::persist_repo_feed_cache(storage_handle, repos[repo_index], &bytes[..loaded])?;
    let _ = emit_package_event(
        log_handle,
        LogSeverity::Info,
        LogEvent::PackageRepositorySynced,
        repo_index as u64,
        repos[repo_index].package_count as u64,
    );
    Ok(PackageStatus::Ok)
}

pub(crate) fn handle_repository_add_request(
    storage_handle: rt::Handle,
    log_handle: rt::Handle,
    repos: &mut [RepositorySlot; MAX_REPOSITORIES],
    repo_count: &mut usize,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 4 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let packed = message.words[0];
    let trust_mode = trust_mode_from_word(packed & 0xffff);
    let channel = package_channel_from_word((packed >> 16) & 0xffff);
    let ring = package_ring_from_word((packed >> 32) & 0xffff);
    let enabled = (packed >> 48) != 0;
    let pinned_digest = message.words[1];
    let name_len = message.words[2] as usize;
    let url_len = message.words[3] as usize;
    let mut bytes = [0u8; (IPC_MAX_WORDS - 4) * 8];
    let total = name_len + url_len;
    let status = if total > bytes.len() {
        PackageStatus::Denied
    } else {
        unpack_bytes(
            &message.words[4..message.word_count as usize],
            total,
            &mut bytes,
        )?;
        let name =
            core::str::from_utf8(&bytes[..name_len]).map_err(|_| rt::Error::InvalidArgument)?;
        let url = core::str::from_utf8(&bytes[name_len..name_len + url_len])
            .map_err(|_| rt::Error::InvalidArgument)?;
        add_repository(
            storage_handle,
            log_handle,
            repos,
            repo_count,
            name,
            url,
            trust_mode,
            channel,
            ring,
            enabled,
            pinned_digest,
        )?
    };
    send_status_reply(reply_handle, PackageTag::RepositoryAddReply, status)
}

pub(crate) fn handle_repository_sync_request(
    storage_handle: rt::Handle,
    network_handle: Option<rt::Handle>,
    log_handle: rt::Handle,
    repos: &mut [RepositorySlot; MAX_REPOSITORIES],
    repo_count: usize,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: &mut usize,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 1 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let target = message.words[0] as usize;
    let mut synced = 0u32;
    let mut failed = 0u32;
    let status = if let Some(network) = network_handle {
        if target == usize::MAX {
            for repo_index in 1..repo_count {
                match sync_repository(
                    storage_handle,
                    network,
                    log_handle,
                    repos,
                    repo_index,
                    packages,
                    package_count,
                )? {
                    PackageStatus::Ok => synced += 1,
                    _ => failed += 1,
                }
            }
            if failed == 0 {
                PackageStatus::Ok
            } else if synced == 0 {
                PackageStatus::Offline
            } else {
                PackageStatus::Busy
            }
        } else if target < repo_count {
            let result = sync_repository(
                storage_handle,
                network,
                log_handle,
                repos,
                target,
                packages,
                package_count,
            )?;
            if result == PackageStatus::Ok {
                synced = 1;
            } else {
                failed = 1;
            }
            result
        } else {
            PackageStatus::NotFound
        }
    } else {
        PackageStatus::Offline
    };

    let mut reply = RawMessage::empty(PackageTag::RepositorySyncReply as u32);
    reply.word_count = 3;
    reply.words[0] = status as u32 as u64;
    reply.words[1] = synced as u64;
    reply.words[2] = failed as u64;
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

pub(crate) fn parse_feed_catalog(
    bytes: &[u8],
    repos: &mut [RepositorySlot; MAX_REPOSITORIES],
    repo_index: usize,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: &mut usize,
    trust_state: PackageTrustState,
    _base_path: &str,
) -> rt::Result<()> {
    let text = core::str::from_utf8(bytes).map_err(|_| rt::Error::InvalidArgument)?;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if line.starts_with("version=") {
            continue;
        }
        let Some(payload) = line.strip_prefix("entry=") else {
            continue;
        };
        let mut parts = payload.split('|');
        let Some(package) = parts.next() else {
            continue;
        };
        let Some(service) = parts.next().and_then(service_id_from_name) else {
            continue;
        };
        let Some(version) = parts.next() else {
            continue;
        };
        let compatibility = parts.next().unwrap_or("serviceos.bootstore.v1");
        let manifest_path = parts.next().unwrap_or("");
        let category = parts.next().unwrap_or("SERVICE");
        let summary = parts.next().unwrap_or(package);
        let _ = add_or_update_version(
            packages,
            package_count,
            service,
            package,
            version,
            compatibility,
            manifest_path,
            "",
            category,
            summary,
            repo_index,
            trust_state,
            None,
            None,
            repos[repo_index].channel,
            repos[repo_index].ring,
        )?;
        repos[repo_index].package_count = repos[repo_index].package_count.saturating_add(1);
    }
    Ok(())
}

pub(crate) fn remove_versions_for_repo(
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    repo_index: usize,
) {
    for slot in packages[..package_count]
        .iter_mut()
        .filter(|slot| slot.occupied)
    {
        let mut new_versions = [PackageVersionSlot::empty(); MAX_PACKAGE_VERSIONS];
        let mut new_count = 0usize;
        for index in 0..slot.version_count {
            let keep = slot.versions[index].occupied
                && !(slot.versions[index].repo_index == repo_index
                    && slot.versions[index]
                        .local_manifest_path
                        .as_str()
                        .ok()
                        .unwrap_or("")
                        .is_empty());
            if keep {
                new_versions[new_count] = slot.versions[index];
                new_count += 1;
            }
        }
        slot.versions = new_versions;
        slot.version_count = new_count;
        slot.installed = remap_index(slot.installed, &slot.versions, new_count);
        slot.active = remap_index(slot.active, &slot.versions, new_count);
        slot.rollback = remap_index(slot.rollback, &slot.versions, new_count);
    }
}

fn remap_index(
    current: Option<usize>,
    versions: &[PackageVersionSlot; MAX_PACKAGE_VERSIONS],
    count: usize,
) -> Option<usize> {
    current.filter(|index| *index < count && versions[*index].occupied)
}

pub(crate) fn count_repositories(repos: &[RepositorySlot; MAX_REPOSITORIES]) -> usize {
    repos.iter().filter(|repo| repo.occupied).count()
}

pub(crate) fn http_fetch_text(
    network_handle: rt::Handle,
    url: &str,
    buffer: &mut [u8],
) -> rt::Result<usize> {
    let (host, port, path) = parse_http_url(url)?;
    let socket = rt::network_socket_open(
        network_handle,
        rt::NetworkSocketKind::TcpStream,
        host.as_str(),
        port,
    )?;
    let result = http_fetch_into(socket, host.as_str(), path.as_str(), buffer);
    let _ = rt::network_socket_close(socket);
    let _ = rt::handle_close(socket);
    result
}

fn http_fetch_into(
    socket_handle: rt::Handle,
    host: &str,
    path: &str,
    buffer: &mut [u8],
) -> rt::Result<usize> {
    wait_for_socket_established(socket_handle, HTTP_TIMEOUT_TICKS)?;
    let mut request = rt::FixedLogBuffer::<256>::new();
    let _ = write!(
        &mut request,
        "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: serviceos-package\r\nConnection: close\r\n\r\n",
        path, host,
    );
    let _ = rt::network_socket_send(socket_handle, request.as_bytes())?;

    let mut scratch = [0u8; HTTP_CHUNK_BYTES];
    let mut loaded = 0usize;
    let mut last_progress = rt::monotonic_now()?;
    loop {
        match rt::network_socket_receive(socket_handle, &mut scratch) {
            Ok(count) if count > 0 => {
                let copy_len = count.min(buffer.len().saturating_sub(loaded));
                if copy_len == 0 {
                    return Err(rt::Error::BufferTooSmall);
                }
                buffer[loaded..loaded + copy_len].copy_from_slice(&scratch[..copy_len]);
                loaded += copy_len;
                last_progress = rt::monotonic_now()?;
            }
            Ok(_) => {}
            Err(rt::Error::Busy) | Err(rt::Error::NotFound) => {}
            Err(error) => return Err(error),
        }
        let status = rt::network_socket_status(socket_handle)?;
        if matches!(
            status.state,
            rt::NetworkSocketState::Closed | rt::NetworkSocketState::Failed
        ) {
            break;
        }
        if rt::monotonic_now()?.saturating_sub(last_progress) >= HTTP_TIMEOUT_TICKS {
            break;
        }
        rt::yield_current()?;
    }
    let header_end = find_http_header_end(&buffer[..loaded]).ok_or(rt::Error::InvalidArgument)?;
    let status = parse_http_status(&buffer[..header_end])?;
    if status != 200 {
        return Err(rt::Error::NotFound);
    }
    let body_len = loaded.saturating_sub(header_end);
    buffer.copy_within(header_end..loaded, 0);
    Ok(body_len)
}

fn wait_for_socket_established(socket_handle: rt::Handle, timeout_ticks: u64) -> rt::Result<()> {
    let start = rt::monotonic_now()?;
    loop {
        let status = rt::network_socket_status(socket_handle)?;
        match status.state {
            rt::NetworkSocketState::Established => return Ok(()),
            rt::NetworkSocketState::Failed | rt::NetworkSocketState::Closed => {
                return Err(rt::Error::NotFound);
            }
            _ => {}
        }
        if rt::monotonic_now()?.saturating_sub(start) >= timeout_ticks {
            return Err(rt::Error::QueueEmpty);
        }
        rt::yield_current()?;
    }
}

fn parse_http_url(
    url: &str,
) -> rt::Result<(
    rt::FixedLogBuffer<REPO_NAME_MAX>,
    u16,
    rt::FixedLogBuffer<REPO_URL_MAX>,
)> {
    let Some(rest) = url.strip_prefix("http://") else {
        return Err(rt::Error::InvalidArgument);
    };
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, path),
        None => (rest, ""),
    };
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (
            host,
            port.parse::<u16>()
                .map_err(|_| rt::Error::InvalidArgument)?,
        ),
        None => (authority, 80),
    };
    let mut host_buf = rt::FixedLogBuffer::<REPO_NAME_MAX>::new();
    let _ = host_buf.write_str(host);
    let mut path_buf = rt::FixedLogBuffer::<REPO_URL_MAX>::new();
    let _ = path_buf.write_str("/");
    let _ = path_buf.write_str(path);
    Ok((host_buf, port, path_buf))
}

pub(crate) fn repository_base_path(url: &str) -> rt::FixedLogBuffer<REPO_URL_MAX> {
    let mut base = rt::FixedLogBuffer::<REPO_URL_MAX>::new();
    if let Some((prefix, _)) = url.rsplit_once('/') {
        let _ = base.write_str(prefix);
    } else {
        let _ = base.write_str(url);
    }
    base
}

pub(crate) fn join_repo_url(
    base_url: &str,
    relative: &str,
) -> rt::Result<rt::FixedLogBuffer<REPO_URL_MAX>> {
    let base = repository_base_path(base_url);
    let mut out = rt::FixedLogBuffer::<REPO_URL_MAX>::new();
    let _ = out.write_str(base.as_str());
    if !relative.starts_with('/') {
        let _ = out.write_str("/");
    }
    let _ = out.write_str(relative);
    Ok(out)
}

fn find_http_header_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
}

fn parse_http_status(bytes: &[u8]) -> rt::Result<u16> {
    let text = core::str::from_utf8(bytes).map_err(|_| rt::Error::InvalidArgument)?;
    let Some(status) = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
    else {
        return Err(rt::Error::InvalidArgument);
    };
    Ok(status)
}

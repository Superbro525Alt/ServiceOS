use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_install_request(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    network_handle: Option<rt::Handle>,
    log_handle: rt::Handle,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    repo_count: usize,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    journal: &mut JournalState,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 2 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let service_id = service_id_from_word(message.words[0]);
    let mut version_buffer = [0u8; BOOT_STORE_PATH_MAX];
    let version = parse_version_argument(message, &mut version_buffer)?;
    let status = if let Some(index) = find_package_slot(packages, service_id, package_count) {
        let target = select_install_target(&packages[index], repos, version)?;
        journal.pending_action = JOURNAL_INSTALL;
        journal.service_id = service_id;
        journal.version = InlinePath::empty();
        let _ = journal.version.set(version_text(&packages[index], target));
        journal.manifest_path = InlinePath::empty();
        crate::storage::persist_journal_state(storage_handle, *journal)?;
        let status = activate_package_version(
            bootstrap,
            storage_handle,
            network_handle,
            log_handle,
            repos,
            repo_count,
            &mut packages[index],
            target,
            LogEvent::PackageInstalled,
        );
        if status == PackageStatus::Ok {
            let _ =
                crate::storage::persist_installed_state(storage_handle, packages, package_count);
            *journal = JournalState::empty();
            let _ = crate::storage::persist_journal_state(storage_handle, *journal);
        }
        status
    } else {
        PackageStatus::NotFound
    };
    send_status_reply(reply_handle, PackageTag::InstallReply, status)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_update_request(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    network_handle: Option<rt::Handle>,
    log_handle: rt::Handle,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    repo_count: usize,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    journal: &mut JournalState,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 2 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let service_id = service_id_from_word(message.words[0]);
    let mut version_buffer = [0u8; BOOT_STORE_PATH_MAX];
    let version = parse_version_argument(message, &mut version_buffer)?;
    let status = if let Some(index) = find_package_slot(packages, service_id, package_count) {
        let current = packages[index].installed;
        if current.is_none() {
            PackageStatus::NotInstalled
        } else {
            let target = select_update_target(&packages[index], repos, version)?;
            match target {
                None => PackageStatus::NoChange,
                Some(target) => {
                    journal.pending_action = JOURNAL_UPDATE;
                    journal.service_id = service_id;
                    journal.version = InlinePath::empty();
                    let _ = journal.version.set(version_text(&packages[index], target));
                    journal.manifest_path = InlinePath::empty();
                    crate::storage::persist_journal_state(storage_handle, *journal)?;
                    let status = activate_package_version(
                        bootstrap,
                        storage_handle,
                        network_handle,
                        log_handle,
                        repos,
                        repo_count,
                        &mut packages[index],
                        target,
                        LogEvent::PackageUpdated,
                    );
                    if status == PackageStatus::Ok {
                        let _ = crate::storage::persist_installed_state(
                            storage_handle,
                            packages,
                            package_count,
                        );
                        *journal = JournalState::empty();
                        let _ = crate::storage::persist_journal_state(storage_handle, *journal);
                    }
                    status
                }
            }
        }
    } else {
        PackageStatus::NotFound
    };
    send_status_reply(reply_handle, PackageTag::UpdateReply, status)
}

pub(crate) fn handle_remove_request(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    log_handle: rt::Handle,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    journal: &mut JournalState,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 1 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let service_id = service_id_from_word(message.words[0]);
    let status = if let Some(index) = find_package_slot(packages, service_id, package_count) {
        let slot = &mut packages[index];
        if let Some(active) = slot.active {
            journal.pending_action = JOURNAL_REMOVE;
            journal.service_id = service_id;
            journal.version = InlinePath::empty();
            let _ = journal.version.set(version_text(slot, active));
            journal.manifest_path = InlinePath::empty();
            crate::storage::persist_journal_state(storage_handle, *journal)?;
            match rt::manager_deactivate_service(bootstrap, slot.service_id) {
                Ok(()) => {
                    slot.rollback = Some(active);
                    slot.installed = None;
                    slot.active = None;
                    let _ = emit_package_event(
                        log_handle,
                        LogSeverity::Warn,
                        LogEvent::PackageRemoved,
                        slot.service_id as u32 as u64,
                        encode_version_text(version_text(slot, active)),
                    );
                    let _ = crate::storage::persist_installed_state(
                        storage_handle,
                        packages,
                        package_count,
                    );
                    *journal = JournalState::empty();
                    let _ = crate::storage::persist_journal_state(storage_handle, *journal);
                    PackageStatus::Ok
                }
                Err(_) => PackageStatus::Busy,
            }
        } else {
            PackageStatus::NotInstalled
        }
    } else {
        PackageStatus::NotFound
    };
    send_status_reply(reply_handle, PackageTag::RemoveReply, status)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_rollback_request(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    network_handle: Option<rt::Handle>,
    log_handle: rt::Handle,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    repo_count: usize,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    journal: &mut JournalState,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 1 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let service_id = service_id_from_word(message.words[0]);
    let status = if let Some(index) = find_package_slot(packages, service_id, package_count) {
        let slot = &mut packages[index];
        if let Some(target) = slot.rollback {
            journal.pending_action = JOURNAL_ROLLBACK;
            journal.service_id = service_id;
            journal.version = InlinePath::empty();
            let _ = journal.version.set(version_text(slot, target));
            journal.manifest_path = InlinePath::empty();
            crate::storage::persist_journal_state(storage_handle, *journal)?;
            let status = activate_package_version(
                bootstrap,
                storage_handle,
                network_handle,
                log_handle,
                repos,
                repo_count,
                slot,
                target,
                LogEvent::PackageRolledBack,
            );
            if status == PackageStatus::Ok {
                let previous = slot.active;
                slot.active = Some(target);
                slot.installed = Some(target);
                slot.rollback = previous;
                let _ = crate::storage::persist_installed_state(
                    storage_handle,
                    packages,
                    package_count,
                );
                *journal = JournalState::empty();
                let _ = crate::storage::persist_journal_state(storage_handle, *journal);
            }
            status
        } else {
            PackageStatus::NoRollback
        }
    } else {
        PackageStatus::NotFound
    };
    send_status_reply(reply_handle, PackageTag::RollbackReply, status)
}

#[allow(clippy::too_many_arguments)]
fn activate_package_version(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    network_handle: Option<rt::Handle>,
    log_handle: rt::Handle,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    _repo_count: usize,
    slot: &mut PackageSlot,
    target: usize,
    event: LogEvent,
) -> PackageStatus {
    if target >= slot.version_count {
        return PackageStatus::NotFound;
    }

    let materialized =
        ensure_version_materialized(storage_handle, network_handle, slot, target, repos);
    if materialized != PackageStatus::Ok {
        return materialized;
    }

    let manifest_path = active_manifest_path(&slot.versions[target]);
    let manifest = match load_manifest_from_storage_path(storage_handle, manifest_path) {
        Ok(manifest) => manifest,
        Err(_) => return PackageStatus::NotFound,
    };
    let service_manifest = match load_service_manifest(storage_handle, manifest) {
        Ok(manifest) => manifest,
        Err(_) => return PackageStatus::Unsupported,
    };
    match verify_package_integrity(storage_handle, manifest) {
        Ok(true) => {}
        Ok(false) => return PackageStatus::IntegrityFailed,
        Err(_) => return PackageStatus::IntegrityFailed,
    }

    let previous = slot.active;
    match rt::manager_activate_service(bootstrap, manifest.service_manifest.as_str().unwrap_or(""))
    {
        Ok(_) => {
            slot.rollback = previous;
            slot.installed = Some(target);
            slot.active = if service_manifest.startup == ServiceStartupMode::OnDemand {
                None
            } else {
                Some(target)
            };
            let _ = emit_package_event(
                log_handle,
                LogSeverity::Info,
                event,
                slot.service_id as u32 as u64,
                encode_version_text(version_text(slot, target)),
            );
            PackageStatus::Ok
        }
        Err(_) => {
            let _ = emit_package_event(
                log_handle,
                LogSeverity::Error,
                LogEvent::PackageActivationFailed,
                slot.service_id as u32 as u64,
                encode_version_text(version_text(slot, target)),
            );
            if let Some(previous) = previous {
                let previous_path = active_manifest_path(&slot.versions[previous]);
                if let Ok(previous_manifest) =
                    load_manifest_from_storage_path(storage_handle, previous_path)
                {
                    let _ = rt::manager_activate_service(
                        bootstrap,
                        previous_manifest.service_manifest.as_str().unwrap_or(""),
                    );
                    slot.installed = Some(previous);
                    slot.active = Some(previous);
                    slot.rollback = Some(target);
                }
            }
            PackageStatus::Busy
        }
    }
}

fn ensure_version_materialized(
    storage_handle: rt::Handle,
    network_handle: Option<rt::Handle>,
    slot: &mut PackageSlot,
    target: usize,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
) -> PackageStatus {
    let version = &mut slot.versions[target];
    if !version.occupied {
        return PackageStatus::NotFound;
    }
    if let Ok(path) = version.local_manifest_path.as_str() {
        if !path.is_empty() && rt::storage_open(storage_handle, path).is_ok() {
            return PackageStatus::Ok;
        }
    }
    if version.manifest_loaded && version.repo_index == BUILTIN_REPOSITORY_INDEX {
        return PackageStatus::Ok;
    }
    let Some(network) = network_handle else {
        return PackageStatus::Offline;
    };
    materialize_remote_version(storage_handle, network, slot, target, repos)
}

fn materialize_remote_version(
    storage_handle: rt::Handle,
    network_handle: rt::Handle,
    slot: &mut PackageSlot,
    target: usize,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
) -> PackageStatus {
    let repo_index = slot.versions[target].repo_index;
    let Some(repo) = repos.get(repo_index).copied().filter(|repo| repo.occupied) else {
        return PackageStatus::NotFound;
    };
    let repo_url = match repo.url.as_str() {
        Ok(url) => url,
        Err(_) => return PackageStatus::Unsupported,
    };
    let manifest_rel = match slot.versions[target].repo_manifest_path.as_str() {
        Ok(path) => path,
        Err(_) => return PackageStatus::Unsupported,
    };
    let mut manifest_bytes = [0u8; MAX_PACKAGE_BYTES];
    let manifest_url = match crate::repositories::join_repo_url(repo_url, manifest_rel) {
        Ok(url) => url,
        Err(_) => return PackageStatus::Unsupported,
    };
    let manifest_loaded = match crate::repositories::http_fetch_text(
        network_handle,
        manifest_url.as_str(),
        &mut manifest_bytes,
    ) {
        Ok(len) => len,
        Err(_) => return PackageStatus::Offline,
    };
    let remote_manifest = match parse_package_manifest(&manifest_bytes[..manifest_loaded]) {
        Ok(manifest) => manifest,
        Err(_) => return PackageStatus::Unsupported,
    };

    let install_root = match crate::storage::install_root_path(
        slot.package_name.as_str().unwrap_or("package"),
        version_text(slot, target),
    ) {
        Ok(path) => path,
        Err(_) => return PackageStatus::Busy,
    };
    if crate::storage::create_install_root(storage_handle, install_root.as_str())
        != rt::Result::Ok(())
    {
        return PackageStatus::Busy;
    }
    for content in remote_manifest.contents[..remote_manifest.content_count].iter() {
        let Ok(remote_path) = content.as_str() else {
            return PackageStatus::Unsupported;
        };
        let url = match crate::repositories::join_repo_url(repo_url, remote_path) {
            Ok(url) => url,
            Err(_) => return PackageStatus::Unsupported,
        };
        let local_path = match crate::storage::local_installed_content_path(
            install_root.as_str(),
            remote_path,
        ) {
            Ok(path) => path,
            Err(_) => return PackageStatus::Busy,
        };
        let mut bytes = [0u8; MAX_HTTP_BYTES];
        let loaded =
            match crate::repositories::http_fetch_text(network_handle, url.as_str(), &mut bytes) {
                Ok(len) => len,
                Err(_) => return PackageStatus::Offline,
            };
        if crate::storage::ensure_parent_directories(storage_handle, local_path.as_str()).is_err() {
            return PackageStatus::Busy;
        }
        if crate::storage::write_storage_file(storage_handle, local_path.as_str(), &bytes[..loaded])
            .is_err()
        {
            return PackageStatus::Busy;
        }
    }
    let rewritten = match crate::storage::rewrite_manifest_for_install(
        remote_manifest,
        install_root.as_str(),
    ) {
        Ok(manifest) => manifest,
        Err(_) => return PackageStatus::Busy,
    };
    let manifest_text = match crate::storage::serialize_package_manifest(rewritten) {
        Ok(text) => text,
        Err(_) => return PackageStatus::Busy,
    };
    let local_manifest_path =
        match crate::storage::local_installed_manifest_path(install_root.as_str()) {
            Ok(path) => path,
            Err(_) => return PackageStatus::Busy,
        };
    if crate::storage::write_storage_file(
        storage_handle,
        local_manifest_path.as_str().unwrap_or(""),
        manifest_text.as_bytes(),
    )
    .is_err()
    {
        return PackageStatus::Busy;
    }
    slot.versions[target].manifest = rewritten;
    slot.versions[target].manifest_loaded = true;
    slot.versions[target].local_manifest_path = local_manifest_path;
    PackageStatus::Ok
}

pub(crate) fn load_manifest_from_storage_path(
    storage_handle: rt::Handle,
    path: &str,
) -> rt::Result<PackageManifest> {
    let (handle, len) = rt::storage_open(storage_handle, path)?;
    let mut bytes = [0u8; MAX_PACKAGE_BYTES];
    let requested = len.min(bytes.len());
    let loaded = rt::storage_read_all(handle, &mut bytes, requested)?;
    let _ = rt::storage_blob_close(handle);
    parse_package_manifest(&bytes[..loaded]).map_err(|_| rt::Error::InvalidArgument)
}

fn load_service_manifest(
    storage_handle: rt::Handle,
    manifest: PackageManifest,
) -> rt::Result<ServiceManifest> {
    let path = manifest
        .service_manifest
        .as_str()
        .map_err(|_| rt::Error::InvalidArgument)?;
    let (blob_handle, blob_len) = rt::storage_open(storage_handle, path)?;
    let mut bytes = [0u8; MAX_PACKAGE_BYTES];
    let requested = blob_len.min(bytes.len());
    let loaded = rt::storage_read_all(blob_handle, &mut bytes, requested)?;
    let _ = rt::storage_blob_close(blob_handle);
    parse_manifest(&bytes[..loaded]).map_err(|_| rt::Error::InvalidArgument)
}

fn verify_package_integrity(
    storage_handle: rt::Handle,
    manifest: PackageManifest,
) -> rt::Result<bool> {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut buffer = [0u8; 96];
    for content in manifest.contents[..manifest.content_count].iter() {
        let path = content.as_str().map_err(|_| rt::Error::InvalidArgument)?;
        update_hash(&mut hash, path.as_bytes());
        let (blob_handle, blob_len) = rt::storage_open(storage_handle, path)?;
        let mut offset = 0usize;
        while offset < blob_len {
            let read = rt::storage_read(blob_handle, offset, &mut buffer)?;
            if read == 0 {
                break;
            }
            update_hash(&mut hash, &buffer[..read]);
            offset += read;
        }
        let _ = rt::storage_blob_close(blob_handle);
    }
    if manifest.integrity == 0 {
        Ok(true)
    } else {
        Ok(hash == manifest.integrity)
    }
}

fn update_hash(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes.iter().copied() {
        *hash ^= byte as u64;
        *hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
}

pub(crate) fn compute_fnv64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    update_hash(&mut hash, bytes);
    hash
}

use super::*;

pub(crate) const OPERATION_TOTAL_STEPS: u32 = 5;

fn optional_text<'a>(text: &'a str) -> Option<&'a str> {
    if text.is_empty() { None } else { Some(text) }
}

/// Emits one live-operation progress record on the log stream at a phase
/// transition. Budget: one record per phase entry (five per operation), so
/// the package log stream stays quiet during a mutation. `op` reuses the
/// journal action codes (JOURNAL_INSTALL/UPDATE/ROLLBACK) and `progress.pack()`
/// carries the phase/step/total word the operation reply already uses, so the
/// shell decodes both with the same helpers it applies to final replies.
fn emit_operation_progress(log_handle: rt::Handle, op: u32, progress: &ops_model::ProgressTracker) {
    let _ = emit_package_event(
        log_handle,
        LogSeverity::Info,
        LogEvent::PackageOperationProgress,
        op as u64,
        progress.pack(),
    );
}

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
    let argument = parse_version_argument(message, &mut version_buffer)?.unwrap_or("");
    let (version_part, source_name) = ops_model::split_version_source(argument);
    let mut progress = ops_model::ProgressTracker::new(OPERATION_TOTAL_STEPS);
    emit_operation_progress(log_handle, ops_model::JOURNAL_INSTALL, &progress);
    let resolved_source = match resolve_source_index(repos, repo_count, source_name) {
        Ok(value) => value,
        Err(status) => {
            return send_operation_reply(
                reply_handle,
                PackageTag::InstallReply,
                status,
                &progress,
                0,
                ops_model::TRIGGER_OPERATOR,
            );
        }
    };
    if let Some(index) = find_package_slot(packages, service_id, package_count) {
        let previous = packages[index]
            .installed
            .map(|current| encode_version_text(version_text(&packages[index], current)))
            .unwrap_or(0);
        let target = match select_install_target(
            &packages[index],
            repos,
            repo_count,
            optional_text(version_part),
            resolved_source,
        ) {
            Ok(target) => target,
            Err(_) => {
                return send_operation_reply(
                    reply_handle,
                    PackageTag::InstallReply,
                    PackageStatus::NotFound,
                    &progress,
                    0,
                    ops_model::TRIGGER_OPERATOR,
                );
            }
        };
        journal.pending_action = JOURNAL_INSTALL;
        journal.service_id = service_id;
        journal.version = InlinePath::empty();
        let _ = journal.version.set(version_text(&packages[index], target));
        journal.manifest_path = InlinePath::empty();
        if crate::storage::persist_journal_state(storage_handle, *journal).is_err() {
            return send_operation_reply(
                reply_handle,
                PackageTag::InstallReply,
                PackageStatus::Busy,
                &progress,
                0,
                ops_model::TRIGGER_OPERATOR,
            );
        }
        progress.complete_step();
        let mut auto_restored = false;
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
            ops_model::JOURNAL_INSTALL,
            &mut progress,
            &mut auto_restored,
        );
        if status == PackageStatus::Ok {
            progress.enter_phase(ops_model::PROGRESS_PHASE_PERSIST);
            emit_operation_progress(log_handle, ops_model::JOURNAL_INSTALL, &progress);
            let _ =
                crate::storage::persist_installed_state(storage_handle, packages, package_count);
            progress.complete_step();
            *journal = JournalState::empty();
            let _ = crate::storage::persist_journal_state(storage_handle, *journal);
        }
        send_operation_reply(
            reply_handle,
            PackageTag::InstallReply,
            status,
            &progress,
            previous,
            if auto_restored {
                ops_model::TRIGGER_AUTO_RESTORE
            } else {
                ops_model::TRIGGER_OPERATOR
            },
        )?;
        Ok(())
    } else {
        send_operation_reply(
            reply_handle,
            PackageTag::InstallReply,
            PackageStatus::NotFound,
            &progress,
            0,
            ops_model::TRIGGER_OPERATOR,
        )?;
        Ok(())
    }
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
    let argument = parse_version_argument(message, &mut version_buffer)?.unwrap_or("");
    let (version_part, source_name) = ops_model::split_version_source(argument);
    let mut progress = ops_model::ProgressTracker::new(OPERATION_TOTAL_STEPS);
    emit_operation_progress(log_handle, ops_model::JOURNAL_UPDATE, &progress);
    let resolved_source = match resolve_source_index(repos, repo_count, source_name) {
        Ok(value) => value,
        Err(status) => {
            return send_operation_reply(
                reply_handle,
                PackageTag::UpdateReply,
                status,
                &progress,
                0,
                ops_model::TRIGGER_OPERATOR,
            );
        }
    };
    let status = if let Some(index) = find_package_slot(packages, service_id, package_count) {
        let previous = packages[index]
            .installed
            .map(|current| encode_version_text(version_text(&packages[index], current)))
            .unwrap_or(0);
        if packages[index].installed.is_none() {
            PackageStatus::NotInstalled
        } else {
            let target = match select_update_target(
                &packages[index],
                repos,
                repo_count,
                optional_text(version_part),
                resolved_source,
            ) {
                Ok(target) => target,
                Err(_) => {
                    return send_operation_reply(
                        reply_handle,
                        PackageTag::UpdateReply,
                        PackageStatus::NotFound,
                        &progress,
                        0,
                        ops_model::TRIGGER_OPERATOR,
                    );
                }
            };
            match target {
                None => PackageStatus::NoChange,
                Some(target) => {
                    journal.pending_action = JOURNAL_UPDATE;
                    journal.service_id = service_id;
                    journal.version = InlinePath::empty();
                    let _ = journal.version.set(version_text(&packages[index], target));
                    journal.manifest_path = InlinePath::empty();
                    if crate::storage::persist_journal_state(storage_handle, *journal).is_err() {
                        return send_operation_reply(
                            reply_handle,
                            PackageTag::UpdateReply,
                            PackageStatus::Busy,
                            &progress,
                            0,
                            ops_model::TRIGGER_OPERATOR,
                        );
                    }
                    progress.complete_step();
                    let mut auto_restored = false;
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
                        ops_model::JOURNAL_UPDATE,
                        &mut progress,
                        &mut auto_restored,
                    );
                    if status == PackageStatus::Ok {
                        progress.enter_phase(ops_model::PROGRESS_PHASE_PERSIST);
                        emit_operation_progress(log_handle, ops_model::JOURNAL_UPDATE, &progress);
                        let _ = crate::storage::persist_installed_state(
                            storage_handle,
                            packages,
                            package_count,
                        );
                        progress.complete_step();
                        *journal = JournalState::empty();
                        let _ = crate::storage::persist_journal_state(storage_handle, *journal);
                    }
                    send_operation_reply(
                        reply_handle,
                        PackageTag::UpdateReply,
                        status,
                        &progress,
                        previous,
                        if auto_restored {
                            ops_model::TRIGGER_AUTO_RESTORE
                        } else {
                            ops_model::TRIGGER_OPERATOR
                        },
                    )?;
                    return Ok(());
                }
            }
        }
    } else {
        PackageStatus::NotFound
    };
    send_operation_reply(
        reply_handle,
        PackageTag::UpdateReply,
        status,
        &progress,
        0,
        ops_model::TRIGGER_OPERATOR,
    )
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
            if crate::storage::persist_journal_state(storage_handle, *journal).is_err() {
                return send_status_reply(
                    reply_handle,
                    PackageTag::RemoveReply,
                    PackageStatus::Busy,
                );
            }
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
    let mut progress = ops_model::ProgressTracker::new(OPERATION_TOTAL_STEPS);
    emit_operation_progress(log_handle, ops_model::JOURNAL_ROLLBACK, &progress);
    if let Some(index) = find_package_slot(packages, service_id, package_count) {
        let slot = &mut packages[index];
        if let Some(target) = slot.rollback {
            // Capture the rollback summary before activation mutates the
            // slot: previous = currently active version, next = the version
            // being rolled back to.
            let previous_text = slot
                .active
                .map(|active| encode_version_text(version_text(slot, active)))
                .unwrap_or(0);
            let target_text = encode_version_text(version_text(slot, target));
            journal.pending_action = JOURNAL_ROLLBACK;
            journal.service_id = service_id;
            journal.version = InlinePath::empty();
            let _ = journal.version.set(version_text(slot, target));
            journal.manifest_path = InlinePath::empty();
            if crate::storage::persist_journal_state(storage_handle, *journal).is_err() {
                return send_operation_reply(
                    reply_handle,
                    PackageTag::RollbackReply,
                    PackageStatus::Busy,
                    &progress,
                    0,
                    0,
                );
            }
            progress.complete_step();
            let mut auto_restored = false;
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
                ops_model::JOURNAL_ROLLBACK,
                &mut progress,
                &mut auto_restored,
            );
            if status == PackageStatus::Ok {
                let previous = slot.active;
                progress.enter_phase(ops_model::PROGRESS_PHASE_PERSIST);
                emit_operation_progress(log_handle, ops_model::JOURNAL_ROLLBACK, &progress);
                slot.active = Some(target);
                slot.installed = Some(target);
                slot.rollback = previous;
                let _ = crate::storage::persist_installed_state(
                    storage_handle,
                    packages,
                    package_count,
                );
                progress.complete_step();
                *journal = JournalState::empty();
                let _ = crate::storage::persist_journal_state(storage_handle, *journal);
            }
            send_operation_reply(
                reply_handle,
                PackageTag::RollbackReply,
                status,
                &progress,
                previous_text,
                target_text,
            )?;
        } else {
            send_operation_reply(
                reply_handle,
                PackageTag::RollbackReply,
                PackageStatus::NoRollback,
                &progress,
                0,
                0,
            )?;
        }
    } else {
        send_operation_reply(
            reply_handle,
            PackageTag::RollbackReply,
            PackageStatus::NotFound,
            &progress,
            0,
            0,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn activate_package_version(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    network_handle: Option<rt::Handle>,
    log_handle: rt::Handle,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    _repo_count: usize,
    slot: &mut PackageSlot,
    target: usize,
    event: LogEvent,
    op: u32,
    progress: &mut ops_model::ProgressTracker,
    auto_restored: &mut bool,
) -> PackageStatus {
    if target >= slot.version_count {
        return PackageStatus::NotFound;
    }

    progress.enter_phase(ops_model::PROGRESS_PHASE_MATERIALIZE);
    emit_operation_progress(log_handle, op, progress);
    let materialized =
        ensure_version_materialized(storage_handle, network_handle, slot, target, repos);
    if materialized != PackageStatus::Ok {
        return materialized;
    }
    progress.complete_step();

    progress.enter_phase(ops_model::PROGRESS_PHASE_VERIFY);
    emit_operation_progress(log_handle, op, progress);
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
    progress.complete_step();

    progress.enter_phase(ops_model::PROGRESS_PHASE_ACTIVATE);
    emit_operation_progress(log_handle, op, progress);
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
            progress.complete_step();
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
                    *auto_restored = true;
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

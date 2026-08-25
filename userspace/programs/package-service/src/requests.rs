use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_request(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    network_handle: Option<rt::Handle>,
    log_handle: rt::Handle,
    repos: &mut [RepositorySlot; MAX_REPOSITORIES],
    repo_count: &mut usize,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: &mut usize,
    journal: &mut JournalState,
    message: &RawMessage,
) -> rt::Result<()> {
    match message.tag {
        x if x == PackageTag::ListRequest as u32 => {
            handle_list_request(packages, *package_count, message)
        }
        x if x == PackageTag::InfoRequest as u32 => {
            handle_info_request(packages, *package_count, message)
        }
        x if x == PackageTag::HistoryRequest as u32 => {
            handle_history_request(packages, *package_count, message)
        }
        x if x == PackageTag::CatalogRequest as u32 => {
            handle_catalog_request(packages, *package_count, message)
        }
        x if x == PackageTag::RepositoryListRequest as u32 => {
            handle_repository_list_request(repos, *repo_count, message)
        }
        x if x == PackageTag::RepositoryAddRequest as u32 => {
            crate::repositories::handle_repository_add_request(
                storage_handle,
                log_handle,
                repos,
                repo_count,
                message,
            )
        }
        x if x == PackageTag::RepositorySyncRequest as u32 => {
            crate::repositories::handle_repository_sync_request(
                storage_handle,
                network_handle,
                log_handle,
                repos,
                *repo_count,
                packages,
                package_count,
                message,
            )
        }
        x if x == PackageTag::ProvenanceRequest as u32 => {
            handle_provenance_request(repos, packages, *package_count, message)
        }
        x if x == PackageTag::PolicyRequest as u32 => {
            handle_policy_request(packages, *package_count, message)
        }
        x if x == PackageTag::PolicySetRequest as u32 => {
            handle_policy_set_request(storage_handle, packages, *package_count, message)
        }
        x if x == PackageTag::MaintenanceRequest as u32 => handle_maintenance_request(
            bootstrap,
            storage_handle,
            network_handle,
            log_handle,
            repos,
            *repo_count,
            packages,
            *package_count,
            journal,
            message,
        ),
        x if x == PackageTag::InstallRequest as u32 => crate::operations::handle_install_request(
            bootstrap,
            storage_handle,
            network_handle,
            log_handle,
            repos,
            *repo_count,
            packages,
            *package_count,
            journal,
            message,
        ),
        x if x == PackageTag::UpdateRequest as u32 => crate::operations::handle_update_request(
            bootstrap,
            storage_handle,
            network_handle,
            log_handle,
            repos,
            *repo_count,
            packages,
            *package_count,
            journal,
            message,
        ),
        x if x == PackageTag::RemoveRequest as u32 => crate::operations::handle_remove_request(
            bootstrap,
            storage_handle,
            log_handle,
            packages,
            *package_count,
            journal,
            message,
        ),
        x if x == PackageTag::RollbackRequest as u32 => crate::operations::handle_rollback_request(
            bootstrap,
            storage_handle,
            network_handle,
            log_handle,
            repos,
            *repo_count,
            packages,
            *package_count,
            journal,
            message,
        ),
        _ => Ok(()),
    }
}

fn handle_list_request(
    packages: &[PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 1 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let mut reply = RawMessage::empty(PackageTag::ListReply as u32);
    reply.word_count = 7;
    reply.words[0] = PackageStatus::End as u32 as u64;

    let index = message.words[0] as usize;
    if let Some(slot) = packages[..package_count]
        .get(index)
        .copied()
        .filter(|slot| slot.occupied)
    {
        reply.words[0] = PackageStatus::Ok as u32 as u64;
        reply.words[1] = slot.service_id as u32 as u64;
        reply.words[2] = package_flags(&slot) as u64;
        reply.words[3] = slot.version_count as u64;
        let installed_len = version_bytes(&slot, slot.installed).len();
        let active_len = version_bytes(&slot, slot.active).len();
        reply.words[4] = installed_len as u64;
        reply.words[5] = active_len as u64;
        reply.words[6] = 0;
        let mut combined = [0u8; (IPC_MAX_WORDS - 7) * 8];
        let mut total = 0usize;
        total += copy_into(&mut combined[total..], version_bytes(&slot, slot.installed))?;
        total += copy_into(&mut combined[total..], version_bytes(&slot, slot.active))?;
        reply.word_count += pack_bytes(&combined[..total], &mut reply.words[7..])?;
    }

    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn handle_info_request(
    packages: &[PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 1 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let requested = service_id_from_word(message.words[0]);
    let mut reply = RawMessage::empty(PackageTag::InfoReply as u32);
    reply.word_count = 8;
    reply.words[0] = PackageStatus::NotFound as u32 as u64;

    if let Some(index) = find_package_slot(packages, requested, package_count) {
        let slot = packages[index];
        let latest = latest_version_index(&slot);
        reply.words[0] = PackageStatus::Ok as u32 as u64;
        reply.words[1] = package_flags(&slot) as u64;
        reply.words[2] = slot.version_count as u64;
        reply.words[3] = version_bytes(&slot, slot.installed).len() as u64;
        reply.words[4] = version_bytes(&slot, slot.active).len() as u64;
        reply.words[5] = version_bytes(&slot, slot.rollback).len() as u64;
        reply.words[6] = version_bytes(&slot, latest).len() as u64;
        reply.words[7] = 0;

        let mut combined = [0u8; (IPC_MAX_WORDS - 8) * 8];
        let mut total = 0usize;
        total += copy_into(&mut combined[total..], version_bytes(&slot, slot.installed))?;
        total += copy_into(&mut combined[total..], version_bytes(&slot, slot.active))?;
        total += copy_into(&mut combined[total..], version_bytes(&slot, slot.rollback))?;
        total += copy_into(&mut combined[total..], version_bytes(&slot, latest))?;
        reply.word_count += pack_bytes(&combined[..total], &mut reply.words[8..])?;
    }

    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn handle_history_request(
    packages: &[PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 1 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let service_id = service_id_from_word(message.words[0]);
    let mut reply = RawMessage::empty(PackageTag::HistoryReply as u32);
    reply.word_count = 4;
    reply.words[0] = PackageStatus::NotFound as u32 as u64;

    if let Some(index) = find_package_slot(packages, service_id, package_count) {
        let slot = packages[index];
        reply.words[0] = PackageStatus::Ok as u32 as u64;
        reply.words[1] = version_bytes(&slot, slot.active).len() as u64;
        reply.words[2] = version_bytes(&slot, slot.rollback).len() as u64;
        reply.words[3] = 0;
        let mut combined = [0u8; (IPC_MAX_WORDS - 4) * 8];
        let mut total = 0usize;
        total += copy_into(&mut combined[total..], version_bytes(&slot, slot.active))?;
        total += copy_into(&mut combined[total..], version_bytes(&slot, slot.rollback))?;
        reply.word_count += pack_bytes(&combined[..total], &mut reply.words[4..])?;
    }

    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn handle_catalog_request(
    packages: &[PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 1 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let mut reply = RawMessage::empty(PackageTag::CatalogReply as u32);
    reply.word_count = 7;
    reply.words[0] = PackageStatus::End as u32 as u64;
    let index = message.words[0] as usize;
    if let Some(slot) = packages[..package_count]
        .get(index)
        .copied()
        .filter(|slot| slot.occupied)
    {
        let latest = latest_version_index(&slot);
        let latest_text = version_bytes(&slot, latest);
        let category = slot.versions[latest.unwrap_or(0)]
            .category
            .as_str()
            .unwrap_or("SERVICE");
        let summary = slot.versions[latest.unwrap_or(0)]
            .summary
            .as_str()
            .unwrap_or("PACKAGE");
        reply.words[0] = PackageStatus::Ok as u32 as u64;
        reply.words[1] = slot.service_id as u32 as u64;
        reply.words[2] = package_flags(&slot) as u64;
        reply.words[3] = latest.map(|i| slot.versions[i].repo_index).unwrap_or(0) as u64;
        reply.words[4] = latest_text.len() as u64;
        reply.words[5] = category.len() as u64;
        reply.words[6] = summary.len() as u64;
        let mut combined = [0u8; (IPC_MAX_WORDS - 7) * 8];
        let mut total = 0usize;
        total += copy_into(&mut combined[total..], latest_text)?;
        total += copy_into(&mut combined[total..], category.as_bytes())?;
        total += copy_into(&mut combined[total..], summary.as_bytes())?;
        reply.word_count += pack_bytes(&combined[..total], &mut reply.words[7..])?;
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn handle_repository_list_request(
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    repo_count: usize,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 1 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let mut reply = RawMessage::empty(PackageTag::RepositoryListReply as u32);
    reply.word_count = 8;
    reply.words[0] = PackageStatus::End as u32 as u64;
    let index = message.words[0] as usize;
    if let Some(repo) = repos[..repo_count]
        .get(index)
        .copied()
        .filter(|repo| repo.occupied)
    {
        let name = repo.name.as_str().unwrap_or("");
        let url = repo.url.as_str().unwrap_or("");
        reply.words[0] = PackageStatus::Ok as u32 as u64;
        reply.words[1] = index as u64;
        reply.words[2] = repo.package_count as u64;
        reply.words[3] = pack_repo_flags(repo) as u64;
        reply.words[4] = name.len() as u64;
        reply.words[5] = url.len() as u64;
        reply.words[6] = repo.pinned_digest;
        reply.words[7] = repo.last_digest;
        let mut combined = [0u8; (IPC_MAX_WORDS - 8) * 8];
        let mut total = 0usize;
        total += copy_into(&mut combined[total..], name.as_bytes())?;
        total += copy_into(&mut combined[total..], url.as_bytes())?;
        reply.word_count += pack_bytes(&combined[..total], &mut reply.words[8..])?;
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn handle_provenance_request(
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    packages: &[PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 1 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let service_id = service_id_from_word(message.words[0]);
    let mut reply = RawMessage::empty(PackageTag::ProvenanceReply as u32);
    reply.word_count = 8;
    reply.words[0] = PackageStatus::NotFound as u32 as u64;
    if let Some(index) = find_package_slot(packages, service_id, package_count) {
        let slot = packages[index];
        let latest = latest_version_index(&slot);
        let repo_index = latest.map(|i| slot.versions[i].repo_index).unwrap_or(0);
        let source = if let Some(version_index) = slot.active {
            active_manifest_path(&slot.versions[version_index])
        } else {
            latest
                .and_then(|version_index| {
                    slot.versions[version_index]
                        .repo_manifest_path
                        .as_str()
                        .ok()
                })
                .unwrap_or("")
        };
        reply.words[0] = PackageStatus::Ok as u32 as u64;
        reply.words[1] = repo_index as u64;
        reply.words[2] = pack_provenance_flags(
            latest
                .map(|version_index| slot.versions[version_index].trust_state)
                .unwrap_or(PackageTrustState::Unverified),
            slot.channel,
            slot.ring,
            package_flags(&slot),
        ) as u64;
        reply.words[3] = version_bytes(&slot, slot.installed).len() as u64;
        reply.words[4] = version_bytes(&slot, slot.active).len() as u64;
        reply.words[5] = version_bytes(&slot, slot.rollback).len() as u64;
        reply.words[6] = version_bytes(&slot, latest).len() as u64;
        reply.words[7] = source.len() as u64;
        let mut combined = [0u8; (IPC_MAX_WORDS - 8) * 8];
        let mut total = 0usize;
        total += copy_into(&mut combined[total..], version_bytes(&slot, slot.installed))?;
        total += copy_into(&mut combined[total..], version_bytes(&slot, slot.active))?;
        total += copy_into(&mut combined[total..], version_bytes(&slot, slot.rollback))?;
        total += copy_into(&mut combined[total..], version_bytes(&slot, latest))?;
        total += copy_into(&mut combined[total..], source.as_bytes())?;
        reply.word_count += pack_bytes(&combined[..total], &mut reply.words[8..])?;
        let _ = repos;
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn handle_policy_request(
    packages: &[PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 1 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let service_id = service_id_from_word(message.words[0]);
    let mut reply = RawMessage::empty(PackageTag::PolicyReply as u32);
    reply.word_count = 4;
    reply.words[0] = PackageStatus::NotFound as u32 as u64;
    if let Some(index) = find_package_slot(packages, service_id, package_count) {
        let pin = packages[index].pin_version.as_str().unwrap_or("");
        reply.words[0] = PackageStatus::Ok as u32 as u64;
        reply.words[1] = packages[index].channel as u32 as u64;
        reply.words[2] = packages[index].ring as u32 as u64;
        reply.words[3] = pin.len() as u64;
        reply.word_count += pack_bytes(pin.as_bytes(), &mut reply.words[4..])?;
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn handle_policy_set_request(
    storage_handle: rt::Handle,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 4 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let service_id = service_id_from_word(message.words[0]);
    let channel = package_channel_from_word(message.words[1]);
    let ring = package_ring_from_word(message.words[2]);
    let pin_len = message.words[3] as usize;
    let mut pin_bytes = [0u8; BOOT_STORE_PATH_MAX];
    let pin = if pin_len == 0 {
        None
    } else {
        unpack_bytes(
            &message.words[4..message.word_count as usize],
            pin_len,
            &mut pin_bytes,
        )?;
        Some(core::str::from_utf8(&pin_bytes[..pin_len]).map_err(|_| rt::Error::InvalidArgument)?)
    };
    let status = if let Some(index) = find_package_slot(packages, service_id, package_count) {
        packages[index].channel = channel;
        packages[index].ring = ring;
        packages[index].pin_version = InlinePath::empty();
        if let Some(pin) = pin {
            let _ = packages[index].pin_version.set(pin);
        }
        crate::storage::persist_installed_state(storage_handle, packages, package_count)?;
        PackageStatus::Ok
    } else {
        PackageStatus::NotFound
    };
    send_status_reply(reply_handle, PackageTag::PolicySetReply, status)
}

/// Feed signing-key rotation: promote an already-enrolled keystore key to
/// active for a source, retire the previous active key at `now`, and
/// re-sign (rewrite) the persisted verification config.
fn rotate_feed_key(
    storage_handle: rt::Handle,
    log_handle: rt::Handle,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    message: &RawMessage,
) -> PackageStatus {
    if message.word_count < 4 {
        return PackageStatus::Unsupported;
    }
    let repo_index = message.words[1] as usize;
    let key_slot = message.words[2] as usize;
    let now = message.words[3];
    if repo_index >= repos.len() || !repos[repo_index].occupied || repos[repo_index].builtin {
        return PackageStatus::NotFound;
    }
    let source = repos[repo_index].name.as_str().unwrap_or("");
    let rotated = unsafe {
        let keystore = &mut *core::ptr::addr_of_mut!(FEED_KEYSTORE);
        match keystore.source_keys_mut(source) {
            Some(entry) if key_slot < entry.key_count => {
                let key_id_bytes = entry.keys[key_slot].key_id.as_str().as_bytes();
                let mut id_buffer = [0u8; crate::signing::KEY_ID_MAX];
                let len = key_id_bytes.len().min(id_buffer.len());
                id_buffer[..len].copy_from_slice(&key_id_bytes[..len]);
                match core::str::from_utf8(&id_buffer[..len]) {
                    Ok(key_id) => entry.rotate_active(key_id, now).is_ok(),
                    Err(_) => false,
                }
            }
            _ => false,
        }
    };
    match rotated {
        true => {
            if crate::storage::persist_feed_keystore(storage_handle).is_err() {
                return PackageStatus::VerificationFailed;
            }
            let _ = emit_package_event(
                log_handle,
                LogSeverity::Info,
                LogEvent::PackageRepositorySynced,
                repo_index as u64,
                0,
            );
            PackageStatus::Ok
        }
        false => PackageStatus::Unsupported,
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_maintenance_request(    bootstrap: rt::Handle,
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
    let raw_action = message.words[0];

    // Recovery is an extension to the maintenance actions agreed between the
    // package service and the shell: it resumes or discards a stale journal
    // entry left by an interrupted install/update/rollback.
    let (mut status, mut repaired, mut collected, mut outcome) = (
        PackageStatus::Ok,
        0u32,
        0u32,
        ops_model::RECOVERY_OUTCOME_NONE,
    );
    let mut handled = false;
    if raw_action == ops_model::MAINTENANCE_ACTION_RECOVER {
        handled = true;
        let (recover_status, recover_outcome) = recover_interrupted_operation(
            bootstrap,
            storage_handle,
            network_handle,
            log_handle,
            repos,
            repo_count,
            packages,
            package_count,
            journal,
        );
        status = recover_status;
        outcome = recover_outcome;
        repaired = u32::from(outcome != ops_model::RECOVERY_OUTCOME_NONE);
    }
    if raw_action == ops_model::MAINTENANCE_ACTION_ROTATE_FEED_KEY {
        handled = true;
        status = rotate_feed_key(storage_handle, log_handle, repos, message);
        repaired = u32::from(status == PackageStatus::Ok);
    }

    if !handled {
        let action = maintenance_action_from_word(raw_action);
        let result = match action {
            PackageMaintenanceAction::Validate => {
                let repaired = crate::storage::validate_package_state(
                    storage_handle,
                    packages,
                    package_count,
                )?;
                (PackageStatus::Ok, repaired, 0)
            }
            PackageMaintenanceAction::Repair => {
                let mut repaired = crate::storage::validate_package_state(
                    storage_handle,
                    packages,
                    package_count,
                )?;
                if journal.pending_action != JOURNAL_NONE {
                    *journal = JournalState::empty();
                    crate::storage::persist_journal_state(storage_handle, *journal)?;
                    repaired = repaired.saturating_add(1);
                }
                (PackageStatus::Ok, repaired, 0)
            }
            PackageMaintenanceAction::GarbageCollect => {
                let repaired = crate::storage::validate_package_state(
                    storage_handle,
                    packages,
                    package_count,
                )?;
                let collected = crate::storage::garbage_collect_packages(
                    storage_handle,
                    packages,
                    package_count,
                )?;
                (PackageStatus::Ok, repaired, collected)
            }
        };
        status = result.0;
        repaired = result.1;
        collected = result.2;
        crate::storage::persist_installed_state(storage_handle, packages, package_count)?;
    }

    let _ = emit_package_event(
        log_handle,
        LogSeverity::Info,
        if collected > 0 {
            LogEvent::PackageGarbageCollected
        } else {
            LogEvent::PackageRepairCompleted
        },
        repaired as u64,
        collected as u64,
    );
    // Extended reply layout:
    // [status, repaired, collected, pending_action, service_id,
    //  journaled_version, stale_at_boot, recovery_outcome, name_len,
    //  <package-name bytes>]
    let stale_at_boot = recovery_state()
        .map(|entry| entry.pending_action)
        .unwrap_or(JOURNAL_NONE);
    let journaled_version = encode_version_text(journal.version.as_str().unwrap_or(""));
    let package_name_slot = find_package_slot(packages, journal.service_id, package_count);
    let package_name = package_name_slot
        .and_then(|index| packages[index].package_name.as_str().ok())
        .unwrap_or("");
    let mut reply = RawMessage::empty(PackageTag::MaintenanceReply as u32);
    reply.words[0] = status as u32 as u64;
    reply.words[1] = repaired as u64;
    reply.words[2] = collected as u64;
    reply.words[3] = journal.pending_action as u64;
    reply.words[4] = journal.service_id as u32 as u64;
    reply.words[5] = journaled_version;
    reply.words[6] = stale_at_boot as u64;
    reply.words[7] = outcome;
    reply.words[8] = package_name.len() as u64;
    reply.word_count = 9 + pack_bytes(package_name.as_bytes(), &mut reply.words[9..])?;
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

/// Resume-or-discard flow for a stale operation journal detected at startup.
/// Resumable actions (install/update/rollback) retry the recorded target
/// activation; anything else is discarded so the next operation starts from
/// a clean journal. A failed resume also discards, reporting the failure.
#[allow(clippy::too_many_arguments)]
fn recover_interrupted_operation(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    network_handle: Option<rt::Handle>,
    log_handle: rt::Handle,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    repo_count: usize,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    journal: &mut JournalState,
) -> (PackageStatus, u64) {
    if journal.pending_action == JOURNAL_NONE {
        return (PackageStatus::NoChange, ops_model::RECOVERY_OUTCOME_NONE);
    }
    let action = journal.pending_action;
    let resumable = matches!(action, JOURNAL_INSTALL | JOURNAL_UPDATE | JOURNAL_ROLLBACK);
    let completion_event = match action {
        JOURNAL_INSTALL => LogEvent::PackageInstalled,
        JOURNAL_UPDATE => LogEvent::PackageUpdated,
        _ => LogEvent::PackageRolledBack,
    };
    if resumable {
        let service_id = journal.service_id;
        let mut version_buffer = [0u8; BOOT_STORE_PATH_MAX];
        let version_text_copied = copy_into(
            &mut version_buffer,
            journal.version.as_str().unwrap_or("").as_bytes(),
        )
        .unwrap_or(0);
        let version_name = core::str::from_utf8(&version_buffer[..version_text_copied]).ok();
        let slot_index = find_package_slot(packages, service_id, package_count);
        let target = slot_index.and_then(|index| {
            version_name.and_then(|name| find_version_by_name(&packages[index], name))
        });
        if let Some(index) = slot_index {
            if let Some(target) = target {
                let mut progress =
                    ops_model::ProgressTracker::new(crate::operations::OPERATION_TOTAL_STEPS);
                progress.complete_step();
                let mut auto_restored = false;
                let status = crate::operations::activate_package_version(
                    bootstrap,
                    storage_handle,
                    network_handle,
                    log_handle,
                    repos,
                    repo_count,
                    &mut packages[index],
                    target,
                    completion_event,
                    &mut progress,
                    &mut auto_restored,
                );
                if status == PackageStatus::Ok {
                    let _ = crate::storage::persist_installed_state(
                        storage_handle,
                        packages,
                        package_count,
                    );
                    *journal = JournalState::empty();
                    let _ = crate::storage::persist_journal_state(storage_handle, *journal);
                    return (PackageStatus::Ok, ops_model::RECOVERY_OUTCOME_RESUMED);
                }
            }
        }
    }
    // Discard path: nothing resumable or resume failed; restore a clean
    // journal so subsequent operations are not blocked by stale state.
    *journal = JournalState::empty();
    let _ = crate::storage::persist_journal_state(storage_handle, *journal);
    (
        PackageStatus::Interrupted,
        ops_model::RECOVERY_OUTCOME_RESUME_FAILED,
    )
}

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
        x if x == PackageTag::KeysListRequest as u32 => handle_keys_list_request(message),
        x if x == PackageTag::KeysEnrollRequest as u32 => {
            handle_keys_enroll_request(storage_handle, message)
        }
        x if x == PackageTag::KeysActivateRequest as u32 => {
            handle_keys_activate_request(storage_handle, message)
        }
        x if x == PackageTag::KeysRotateRequest as u32 => {
            handle_keys_rotate_request(storage_handle, message)
        }
        x if x == PackageTag::KeysGenRequest as u32 => {
            handle_keys_gen_request(storage_handle, message)
        }
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
        let provenance_index = slot.active.or(latest);
        let repo_index = provenance_index
            .map(|i| slot.versions[i].repo_index)
            .unwrap_or(0);
        let trust_state = provenance_index
            .map(|version_index| slot.versions[version_index].trust_state)
            .unwrap_or(PackageTrustState::Unverified);
        let signed_key_fingerprint = if trust_state == PackageTrustState::SignedKeyTrusted {
            repos
                .get(repo_index)
                .map(|repo| repo.bound_key_fingerprint)
                .unwrap_or(0)
        } else {
            0
        };
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
        reply.words[2] =
            pack_provenance_flags(trust_state, slot.channel, slot.ring, package_flags(&slot))
                as u64;
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
        let extended = signed_key_fingerprint != 0 && total <= (IPC_MAX_WORDS - 9) * 8;
        if extended {
            reply.words[8] = signed_key_fingerprint;
            reply.word_count = 9;
            reply.word_count += pack_bytes(&combined[..total], &mut reply.words[9..])?;
        } else {
            reply.word_count += pack_bytes(&combined[..total], &mut reply.words[8..])?;
        }
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
fn handle_maintenance_request(
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
    if matches!(
        raw_action,
        ops_model::MAINTENANCE_ACTION_SYSUPDATE_PLAN
            | ops_model::MAINTENANCE_ACTION_SYSUPDATE_APPLY
            | ops_model::MAINTENANCE_ACTION_SYSUPDATE_ROLLBACK
            | ops_model::MAINTENANCE_ACTION_SYSUPDATE_HISTORY
    ) {
        return crate::sysupdate_ops::handle_sysupdate_request(
            bootstrap,
            storage_handle,
            network_handle,
            log_handle,
            repos,
            repo_count,
            packages,
            package_count,
            journal,
            message,
            raw_action,
        );
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
                    // A discarded sysupdate transaction must also drop its
                    // persisted cursor file so the next apply starts clean.
                    if journal.pending_action == JOURNAL_SYSUPDATE {
                        let _ = crate::storage::clear_sysupdate_txn(storage_handle);
                    }
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
    // Whole-system update transactions carry their own resumable cursor in
    // the persisted transaction file and are handled end-to-end there.
    if action == JOURNAL_SYSUPDATE {
        return crate::sysupdate_ops::recover_interrupted_sysupdate(
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
    }
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

// ---------------------------------------------------------------------------
// Feed-keystore key management (additive shell surface, PackageTag 0x720..)
//
// Variable-length fields ride inline as [len words][packed bytes], matching
// the repository-add convention (`pack_bytes`/`unpack_bytes`). The IPC word
// budget caps any single reply at 16 words, so list replies carry source+
// key-id only; the full pinned hex is echoed where the flow already holds
// it (enroll/gen inputs, gen replies). Mutations persist the keystore
// before the reply so a crash never advertises un-persisted state.
// ---------------------------------------------------------------------------

/// Per-boot counter mixed into generated key seeds.
static KEYS_GEN_COUNTER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// GenRequest flag: reply carries the SECRET SEED instead of the pubkey.
pub(crate) const KEYS_GEN_FLAG_SHOW_SEED: u64 = 1;

fn keystore_error_word(error: crate::signing::KeystoreError) -> u64 {
    use crate::signing::KeystoreError as E;
    let status = match error {
        E::UnknownSource | E::UnknownKey | E::NoActiveKey => PackageStatus::NotFound,
        E::SourceFull => PackageStatus::Busy,
        E::DuplicateKey => PackageStatus::AlreadyExists,
        E::SameKeyActive => PackageStatus::NoChange,
        E::InvalidKeyId | E::InvalidKeyHex => PackageStatus::InvalidParameter,
    };
    status as u32 as u64
}

fn keys_send_reply(reply_handle: rt::Handle, reply: &RawMessage) {
    let _ = rt::channel_send(reply_handle, reply);
    let _ = rt::handle_close(reply_handle);
}

fn keys_reject(reply_handle: rt::Handle, reply_tag: PackageTag, status: PackageStatus) {
    let mut reply = RawMessage::empty(reply_tag as u32);
    reply.word_count = 1;
    reply.words[0] = status as u32 as u64;
    keys_send_reply(reply_handle, &reply);
}

/// Unpack `[len_a][len_b][a bytes][b bytes]` starting at `words[0]` into
/// `combined`, returning borrowed UTF-8 slices valid as long as it lives.
fn keys_read_two_fields<'a>(words: &[u64], combined: &'a mut [u8]) -> Option<(&'a str, &'a str)> {
    use crate::signing::{KEY_HEX_MAX, SOURCE_NAME_MAX};
    if words.len() < 2 || combined.len() < SOURCE_NAME_MAX + KEY_HEX_MAX {
        return None;
    }
    let len_a = words[0] as usize;
    let len_b = words[1] as usize;
    if len_a == 0 || len_a > SOURCE_NAME_MAX || len_b == 0 || len_b > KEY_HEX_MAX {
        return None;
    }
    unpack_bytes(&words[2..words.len()], len_a + len_b, &mut combined[..]).ok()?;
    let source = core::str::from_utf8(&combined[..len_a]).ok()?;
    let field_b = core::str::from_utf8(&combined[len_a..len_a + len_b]).ok()?;
    Some((source, field_b))
}

/// Unpack `[len_a][a bytes]` starting at `words[0]` into `combined`.
fn keys_read_one_field<'a>(words: &[u64], combined: &'a mut [u8]) -> Option<&'a str> {
    use crate::signing::SOURCE_NAME_MAX;
    if words.is_empty() || combined.len() < SOURCE_NAME_MAX {
        return None;
    }
    let len_a = words[0] as usize;
    if len_a == 0 || len_a > SOURCE_NAME_MAX {
        return None;
    }
    unpack_bytes(&words[1..words.len()], len_a, &mut combined[..]).ok()?;
    core::str::from_utf8(&combined[..len_a]).ok()
}

/// KeysListRequest: words[0] = flat index across every source and key.
/// Reply per entry: [status][index][alg][state][retired_tick][source_len]
/// [id_len][reserved] + packed(source, id). End-of-list is status=End with
/// word_count=1, mirroring the repository-list contract.
fn handle_keys_list_request(message: &RawMessage) -> rt::Result<()> {
    if message.word_count < 1 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let target_index = message.words[0] as usize;
    let keystore = unsafe { &*core::ptr::addr_of!(FEED_KEYSTORE) };

    let mut flat = 0usize;
    let mut hit: Option<(usize, usize)> = None;
    'outer: for (source_index, entry) in
        keystore.sources[..keystore.source_count].iter().enumerate()
    {
        for key_index in 0..entry.key_count {
            if flat == target_index {
                hit = Some((source_index, key_index));
                break 'outer;
            }
            flat += 1;
        }
    }

    let Some((source_index, key_index)) = hit else {
        keys_reject(reply_handle, PackageTag::KeysListReply, PackageStatus::End);
        return Ok(());
    };
    let entry = &keystore.sources[source_index];
    let key = &entry.keys[key_index];
    let source_bytes = entry.source.as_str().as_bytes();
    let id_bytes = key.key_id.as_str().as_bytes();
    let total = source_bytes.len() + id_bytes.len();
    if total > (IPC_MAX_WORDS - 8) * 8 {
        return Ok(());
    }

    let mut reply = RawMessage::empty(PackageTag::KeysListReply as u32);
    reply.word_count = 8;
    reply.words[0] = PackageStatus::Ok as u32 as u64;
    reply.words[1] = target_index as u64;
    reply.words[2] = crate::signing::alg_word(key.alg);
    reply.words[3] = crate::signing::state_word(key.state);
    reply.words[4] = key.retired_tick;
    reply.words[5] = source_bytes.len() as u64;
    reply.words[6] = id_bytes.len() as u64;
    reply.words[7] = 0;

    let mut combined = [0u8; (IPC_MAX_WORDS - 8) * 8];
    let mut cursor = 0usize;
    for chunk in [source_bytes, id_bytes] {
        let _ = copy_into(&mut combined[cursor..], chunk);
        cursor += chunk.len();
    }
    reply.word_count += pack_bytes(&combined[..total], &mut reply.words[8..])?;
    keys_send_reply(reply_handle, &reply);
    Ok(())
}

/// Post-mutation state lookup: state word of `key_id` under `source`.
unsafe fn keys_state_word_of(
    keystore: &crate::signing::Keystore,
    source: &str,
    key_id: &str,
) -> u64 {
    keystore
        .source_keys(source)
        .and_then(|entry| entry.find_key(key_id))
        .map(|key| crate::signing::state_word(key.state))
        .unwrap_or(0)
}

/// Store fingerprint mixed into generated seeds: FNV-1a64 over every
/// (id, hex) pair currently pinned plus each source name.
unsafe fn keys_store_fingerprint() -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x100_0000_01b3;
    let keystore = unsafe { &*core::ptr::addr_of!(FEED_KEYSTORE) };
    let mut word = OFFSET;
    for entry in keystore.sources[..keystore.source_count].iter() {
        for byte in entry.source.as_str().as_bytes() {
            word ^= u64::from(*byte);
            word = word.wrapping_mul(PRIME);
        }
        for key in entry.keys[..entry.key_count].iter() {
            for byte in key
                .key_id
                .as_str()
                .as_bytes()
                .iter()
                .copied()
                .chain(key.key_hex.as_str().as_bytes().iter().copied())
            {
                word ^= u64::from(byte);
                word = word.wrapping_mul(PRIME);
            }
        }
    }
    word
}

/// KeysEnrollRequest: words[1]=source_len, words[2]=hex_len, then packed
/// (source, pubkey-hex). Key id derives from the key material (`k-<16hex>`).
/// The first key of a fresh source bootstraps active; later ones enroll
/// retired until rotation promotes them. Only the public half is stored.
/// Reply: [status][state_word].
fn handle_keys_enroll_request(storage_handle: rt::Handle, message: &RawMessage) -> rt::Result<()> {
    const HEADER_WORDS: u32 = 3; // reserved + two length words
    if message.word_count < HEADER_WORDS || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let fields_words = &message.words[HEADER_WORDS as usize - 2..message.word_count as usize];
    let mut field_bytes = [0u8; crate::signing::SOURCE_NAME_MAX + crate::signing::KEY_HEX_MAX];
    let Some((source, key_hex)) = keys_read_two_fields(fields_words, &mut field_bytes) else {
        keys_reject(
            reply_handle,
            PackageTag::KeysEnrollReply,
            PackageStatus::InvalidParameter,
        );
        return Ok(());
    };
    // Validate once here so id derivation and enrollment cannot disagree.
    let Some(key_bytes) = crate::signing::decode_pubkey_hex(key_hex) else {
        keys_reject(
            reply_handle,
            PackageTag::KeysEnrollReply,
            PackageStatus::InvalidParameter,
        );
        return Ok(());
    };
    let (id_buffer, id_len) = crate::signing::auto_key_id(&key_bytes);

    let mut reply = RawMessage::empty(PackageTag::KeysEnrollReply as u32);
    reply.word_count = 2;

    let outcome_word: Result<(u64, u64), crate::signing::KeystoreError> =
        match core::str::from_utf8(&id_buffer[..id_len]) {
            Err(_) => Err(crate::signing::KeystoreError::InvalidKeyId),
            Ok(id_text) => unsafe {
                let keystore = &mut *core::ptr::addr_of_mut!(FEED_KEYSTORE);
                match keystore.ensure_source(source) {
                    Err(error) => Err(error),
                    Ok(entry) => entry.enroll_ed25519(id_text, key_hex).map(|_| {
                        (
                            PackageStatus::Ok as u32 as u64,
                            keys_state_word_of(keystore, source, id_text),
                        )
                    }),
                }
            },
        };

    let mut persist_needed = false;
    match outcome_word {
        Err(error) => {
            reply.words[0] = keystore_error_word(error);
            reply.words[1] = 0;
        }
        Ok((status_word, state_word)) => {
            reply.words[0] = status_word;
            reply.words[1] = state_word;
            persist_needed = true;
        }
    }
    if persist_needed && crate::storage::persist_feed_keystore(storage_handle).is_err() {
        reply.words[0] = PackageStatus::VerificationFailed as u32 as u64;
        reply.words[1] = 0;
    }
    keys_send_reply(reply_handle, &reply);
    Ok(())
}

/// KeysActivateRequest: words[0]=now_tick, words[1]=source_len,
/// words[2]=id_len, then packed (source, key-id). Promotes the named key to
/// active, retiring the current active key at `now`. Reply: [status].
fn handle_keys_activate_request(
    storage_handle: rt::Handle,
    message: &RawMessage,
) -> rt::Result<()> {
    const HEADER_WORDS: u32 = 3; // now tick + two length words
    if message.word_count < HEADER_WORDS || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let now = message.words[0];
    let fields_words = &message.words[HEADER_WORDS as usize - 2..message.word_count as usize];
    let mut field_bytes = [0u8; crate::signing::SOURCE_NAME_MAX + crate::signing::KEY_HEX_MAX];
    let Some((source, key_id)) = keys_read_two_fields(fields_words, &mut field_bytes) else {
        keys_reject(
            reply_handle,
            PackageTag::KeysActivateReply,
            PackageStatus::InvalidParameter,
        );
        return Ok(());
    };

    let mut reply = RawMessage::empty(PackageTag::KeysActivateReply as u32);
    reply.word_count = 1;
    let mut persist_needed = false;
    unsafe {
        let keystore = &mut *core::ptr::addr_of_mut!(FEED_KEYSTORE);
        let outcome = match keystore.source_keys_mut(source) {
            Some(entry) => entry.rotate_active(key_id, now),
            None => Err(crate::signing::KeystoreError::UnknownSource),
        };
        match outcome {
            Err(error) => reply.words[0] = keystore_error_word(error),
            Ok(()) => {
                reply.words[0] = PackageStatus::Ok as u32 as u64;
                persist_needed = true;
            }
        }
    }
    if persist_needed && crate::storage::persist_feed_keystore(storage_handle).is_err() {
        reply.words[0] = PackageStatus::VerificationFailed as u32 as u64;
    }
    keys_send_reply(reply_handle, &reply);
    Ok(())
}

/// KeysRotateRequest: words[0]=now_tick, words[1]=source_len, then packed
/// (source). Promotes the MOST RECENTLY enrolled retired key of the source
/// (the provisioned standby) and retires the active key at `now`.
/// Reply: [status][id_len] + packed(promoted-id).
fn handle_keys_rotate_request(storage_handle: rt::Handle, message: &RawMessage) -> rt::Result<()> {
    const HEADER_WORDS: u32 = 2; // now tick + length word
    if message.word_count < HEADER_WORDS || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let now = message.words[0];
    let mut field_bytes = [0u8; crate::signing::SOURCE_NAME_MAX];
    let Some(source) = keys_read_one_field(
        &message.words[HEADER_WORDS as usize - 1..message.word_count as usize],
        &mut field_bytes,
    ) else {
        keys_reject(
            reply_handle,
            PackageTag::KeysRotateReply,
            PackageStatus::InvalidParameter,
        );
        return Ok(());
    };

    let mut promoted_buffer = [0u8; crate::signing::KEY_ID_MAX];
    let mut promoted_len = 0usize;
    let mut fail_word: Option<u64> = None;

    unsafe {
        let keystore = &mut *core::ptr::addr_of_mut!(FEED_KEYSTORE);
        match keystore.source_keys_mut(source) {
            None => {
                fail_word = Some(keystore_error_word(
                    crate::signing::KeystoreError::UnknownSource,
                ))
            }
            Some(entry) => match entry.rotate_source(now) {
                Ok(slot_index) => {
                    let id_bytes = entry.keys[slot_index].key_id.as_str().as_bytes();
                    promoted_len = id_bytes.len().min(promoted_buffer.len());
                    promoted_buffer[..promoted_len].copy_from_slice(&id_bytes[..promoted_len]);
                }
                Err(error) => fail_word = Some(keystore_error_word(error)),
            },
        }
    }

    let mut reply = RawMessage::empty(PackageTag::KeysRotateReply as u32);
    reply.word_count = 2;
    match fail_word {
        Some(word) => {
            reply.words[0] = word;
            reply.words[1] = 0;
        }
        None => {
            reply.words[0] = PackageStatus::Ok as u32 as u64;
            reply.words[1] = promoted_len as u64;
            if crate::storage::persist_feed_keystore(storage_handle).is_err() {
                reply.words[0] = PackageStatus::VerificationFailed as u32 as u64;
                reply.words[1] = 0;
            }
        }
    }
    if reply.words[1] > 0 {
        reply.word_count += pack_bytes(&promoted_buffer[..promoted_len], &mut reply.words[2..])?;
    }
    keys_send_reply(reply_handle, &reply);
    Ok(())
}

/// KeysGenRequest: words[0]=flags(bit0 = reply carries the secret seed hex
/// instead of the pubkey hex), words[1]=source_len, then packed (source).
///
/// Generates a fresh Ed25519 seed guest-side (SHA-512 mix of source name,
/// monotonic tick, per-boot counter, store fingerprint), derives the
/// compressed public key, enrolls it under an auto id, and persists ONLY
/// the public half. When bit0 is clear the reply carries the PUBLIC KEY so
/// the operator can record it; with bit0 set it carries the SECRET SEED —
/// shown once, never stored anywhere in the guest.
///
/// HONEST LIMITS (see signing::derive_generated_identity): the kernel has
/// no RNG yet, so this seed is entropy-substituted, not CSPRNG output. It
/// varies across calls and boots but is predictable given complete host
/// timing knowledge; treat it as tooling-grade until an RNG lands.
/// Reply: [status][state_word][field0_len][field1_len] + packed(id, field).
fn handle_keys_gen_request(storage_handle: rt::Handle, message: &RawMessage) -> rt::Result<()> {
    const HEADER_WORDS: u32 = 2; // flags + length word
    if message.word_count < HEADER_WORDS || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let show_seed = message.words[0] & KEYS_GEN_FLAG_SHOW_SEED != 0;
    let mut field_bytes = [0u8; crate::signing::SOURCE_NAME_MAX];
    let Some(source) = keys_read_one_field(
        &message.words[HEADER_WORDS as usize - 1..message.word_count as usize],
        &mut field_bytes,
    ) else {
        keys_reject(
            reply_handle,
            PackageTag::KeysGenReply,
            PackageStatus::InvalidParameter,
        );
        return Ok(());
    };

    let tick = rt::monotonic_now().unwrap_or(0);
    let counter = KEYS_GEN_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1;
    let identity = unsafe {
        crate::signing::derive_generated_identity(
            source.as_bytes(),
            tick,
            counter,
            keys_store_fingerprint(),
        )
    };
    let Some(id_text) = identity.id_str() else {
        keys_reject(
            reply_handle,
            PackageTag::KeysGenReply,
            PackageStatus::Unsupported,
        );
        return Ok(());
    };
    let pub_hex_array = identity.public_hex();
    let seed_hex_array = identity.seed_hex();
    let secret_field: &[u8] = if show_seed {
        &seed_hex_array
    } else {
        &pub_hex_array
    };
    let pub_text = core::str::from_utf8(&pub_hex_array).unwrap_or("");

    let mut reply = RawMessage::empty(PackageTag::KeysGenReply as u32);
    reply.word_count = 4;
    let mut persist_needed = false;

    unsafe {
        let keystore = &mut *core::ptr::addr_of_mut!(FEED_KEYSTORE);
        let outcome = match keystore.ensure_source(source) {
            Err(error) => Err(error),
            Ok(entry) => entry.enroll_ed25519(id_text, pub_text).map(|_| ()),
        };
        match outcome {
            Err(error) => reply.words[0] = keystore_error_word(error),
            Ok(()) => {
                reply.words[0] = PackageStatus::Ok as u32 as u64;
                reply.words[1] = keys_state_word_of(keystore, source, id_text);
                persist_needed = true;
            }
        }
    }

    if persist_needed && crate::storage::persist_feed_keystore(storage_handle).is_err() {
        reply.words[0] = PackageStatus::VerificationFailed as u32 as u64;
        reply.words[1] = 0;
    }

    if reply.words[0] == PackageStatus::Ok as u32 as u64 {
        let id_bytes = id_text.as_bytes();
        let total = id_bytes.len() + secret_field.len();
        if total <= (IPC_MAX_WORDS - 4) * 8 {
            reply.words[2] = id_bytes.len() as u64;
            reply.words[3] = secret_field.len() as u64;
            let mut combined = [0u8; (IPC_MAX_WORDS - 4) * 8];
            let mut cursor = 0usize;
            for chunk in [id_bytes, secret_field] {
                let _ = copy_into(&mut combined[cursor..], chunk);
                cursor += chunk.len();
            }
            reply.word_count += pack_bytes(&combined[..total], &mut reply.words[4..])?;
        } else {
            reply.words[2] = 0;
            reply.words[3] = 0;
        }
    } else {
        reply.words[2] = 0;
        reply.words[3] = 0;
    }
    keys_send_reply(reply_handle, &reply);
    Ok(())
}

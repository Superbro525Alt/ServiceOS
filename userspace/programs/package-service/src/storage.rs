use super::*;

pub(crate) fn initialize_state_directories(storage_handle: rt::Handle) -> rt::Result<()> {
    ensure_directory(storage_handle, "state/")?;
    ensure_directory(storage_handle, "state/packages/")?;
    ensure_directory(storage_handle, "state/packages/repos/")?;
    ensure_directory(storage_handle, "state/packages/install/")?;
    Ok(())
}

fn write_repo_record(text: &mut rt::FixedLogBuffer<MAX_STATE_BYTES>, repo: RepositorySlot) {
    let _ = write!(
        text,
        "repo={}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{:016x}\n",
        repo.name.as_str().unwrap_or("repo"),
        repo.url.as_str().unwrap_or(""),
        repo.trust_mode as u32,
        repo.pinned_digest,
        repo.channel as u32,
        repo.ring as u32,
        u32::from(repo.enabled),
        repo.last_digest,
        repo.sync_state as u32,
        repo.bound_key_id.as_str(),
        repo.bound_key_fingerprint,
    );
}

fn parse_repo_record(payload: &str) -> Option<RepositorySlot> {
    let mut parts = payload.split('|');
    let name = parts.next()?;
    let url = parts.next()?;
    let trust_mode = parts
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .map(|value| trust_mode_from_word(value as u64))
        .unwrap_or(PackageRepositoryTrustMode::Unsigned);
    let pinned_digest = parts
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let channel = parts
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .map(|value| package_channel_from_word(value as u64))
        .unwrap_or(PackageChannel::Stable);
    let ring = parts
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .map(|value| package_ring_from_word(value as u64))
        .unwrap_or(PackageRing::Production);
    let enabled = parts.next().map(|value| value == "1").unwrap_or(true);
    let last_digest = parts
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let sync_state = parts
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .map(|value| match value {
            x if x == PackageRepositorySyncState::Ready as u32 => PackageRepositorySyncState::Ready,
            x if x == PackageRepositorySyncState::Offline as u32 => {
                PackageRepositorySyncState::Offline
            }
            x if x == PackageRepositorySyncState::Failed as u32 => {
                PackageRepositorySyncState::Failed
            }
            _ => PackageRepositorySyncState::Idle,
        })
        .unwrap_or(PackageRepositorySyncState::Idle);
    let bound_key_id = parts.next().unwrap_or("");
    let bound_key_fingerprint = parts
        .next()
        .and_then(crate::signing::parse_hex_u64)
        .unwrap_or(0);
    let mut repo = RepositorySlot::empty();
    let _ = repo.name.set(name);
    let _ = repo.url.set(url);
    repo.trust_mode = trust_mode;
    repo.channel = channel;
    repo.ring = ring;
    repo.enabled = enabled;
    repo.last_digest = last_digest;
    repo.pinned_digest = pinned_digest;
    repo.sync_state = sync_state;
    let _ = repo.bound_key_id.set(bound_key_id);
    repo.bound_key_fingerprint = bound_key_fingerprint;
    repo.occupied = true;
    Some(repo)
}

pub(crate) fn persist_repositories(
    storage_handle: rt::Handle,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    repo_count: usize,
) -> rt::Result<()> {
    let mut text = rt::FixedLogBuffer::<MAX_STATE_BYTES>::new();
    let _ = write!(&mut text, "version=1\n");
    for repo in repos[..repo_count]
        .iter()
        .copied()
        .filter(|repo| repo.occupied && !repo.builtin)
    {
        write_repo_record(&mut text, repo);
    }
    write_storage_file(storage_handle, "state/packages/repos.cfg", text.as_bytes())
}

pub(crate) fn load_persisted_repositories(
    storage_handle: rt::Handle,
    repos: &mut [RepositorySlot; MAX_REPOSITORIES],
    repo_count: &mut usize,
) -> rt::Result<()> {
    let (blob, len) = match rt::storage_open(storage_handle, "state/packages/repos.cfg") {
        Ok(value) => value,
        Err(rt::Error::NotFound) => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut bytes = [0u8; MAX_STATE_BYTES];
    let requested = len.min(bytes.len());
    let loaded = rt::storage_read_all(blob, &mut bytes, requested)?;
    let _ = rt::storage_blob_close(blob);
    let text = core::str::from_utf8(&bytes[..loaded]).map_err(|_| rt::Error::InvalidArgument)?;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if line.starts_with("version=") {
            continue;
        }
        let Some(payload) = line.strip_prefix("repo=") else {
            continue;
        };
        let Some(repo) = parse_repo_record(payload) else {
            continue;
        };
        if *repo_count < repos.len() {
            repos[*repo_count] = repo;
            *repo_count += 1;
        }
    }
    Ok(())
}

pub(crate) fn repo_feed_cache_path(
    repo_name: &str,
) -> rt::Result<rt::FixedLogBuffer<INSTALL_PATH_MAX>> {
    let mut path = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
    let _ = write!(&mut path, "state/packages/repos/{}/feed.idx", repo_name);
    Ok(path)
}

pub(crate) fn persist_repo_feed_cache(
    storage_handle: rt::Handle,
    repo: RepositorySlot,
    bytes: &[u8],
) -> rt::Result<()> {
    let cache_path = repo_feed_cache_path(repo.name.as_str().unwrap_or("repo"))?;
    ensure_parent_directories(storage_handle, cache_path.as_str())?;
    write_storage_file(storage_handle, cache_path.as_str(), bytes)
}

pub(crate) fn load_repo_feed_cache(
    storage_handle: rt::Handle,
    repos: &mut [RepositorySlot; MAX_REPOSITORIES],
    repo_index: usize,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: &mut usize,
) -> rt::Result<()> {
    if !repos[repo_index].occupied || repos[repo_index].builtin {
        return Ok(());
    }
    let cache_path = repo_feed_cache_path(repos[repo_index].name.as_str().unwrap_or("repo"))?;
    let (blob, len) = match rt::storage_open(storage_handle, cache_path.as_str()) {
        Ok(value) => value,
        Err(rt::Error::NotFound) => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut bytes = [0u8; MAX_FEED_BYTES];
    let requested = len.min(bytes.len());
    let loaded = rt::storage_read_all(blob, &mut bytes, requested)?;
    let _ = rt::storage_blob_close(blob);
    let now = rt::monotonic_now().unwrap_or(0);
    let source_name = repos[repo_index].name.as_str().unwrap_or("");
    let feed_text = core::str::from_utf8(&bytes[..loaded]).ok();
    let trust_state = match feed_text.map(|text| {
        crate::repositories::remote_feed_trust_state(
            &repos[repo_index],
            text,
            crate::operations::compute_fnv64(&bytes[..loaded]),
            now,
        )
    }) {
        Some(Ok(trust_state)) => trust_state,
        Some(Err(verdict)) => {
            repos[repo_index].sync_state = PackageRepositorySyncState::Failed;
            let _ = crate::repositories::record_feed_rejection(
                storage_handle,
                source_name,
                crate::signing::reject_reason(verdict),
                crate::operations::compute_fnv64(&bytes[..loaded]),
                now,
            );
            return Ok(());
        }
        _ => match repos[repo_index].trust_mode {
            PackageRepositoryTrustMode::Boot => PackageTrustState::BootTrusted,
            PackageRepositoryTrustMode::Unsigned => PackageTrustState::Unverified,
            PackageRepositoryTrustMode::PinnedDigest => {
                if repos[repo_index].last_digest
                    == crate::operations::compute_fnv64(&bytes[..loaded])
                {
                    PackageTrustState::DigestPinned
                } else {
                    PackageTrustState::VerificationFailed
                }
            }
            PackageRepositoryTrustMode::SignedKey => PackageTrustState::VerificationFailed,
        },
    };
    let base_path =
        crate::repositories::repository_base_path(repos[repo_index].url.as_str().unwrap_or(""));
    crate::repositories::parse_feed_catalog(
        &bytes[..loaded],
        repos,
        repo_index,
        packages,
        package_count,
        trust_state,
        base_path.as_str(),
    )
}

pub(crate) fn persist_installed_state(
    storage_handle: rt::Handle,
    packages: &[PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
) -> rt::Result<()> {
    let mut text = rt::FixedLogBuffer::<MAX_STATE_BYTES>::new();
    let _ = write!(&mut text, "version=1\n");
    for slot in packages[..package_count]
        .iter()
        .filter(|slot| slot.occupied)
    {
        let active_manifest = slot
            .active
            .and_then(|index| slot.versions[index].local_manifest_path.as_str().ok())
            .unwrap_or("");
        let rollback_manifest = slot
            .rollback
            .and_then(|index| slot.versions[index].local_manifest_path.as_str().ok())
            .unwrap_or("");
        let _ = write!(
            &mut text,
            "pkg={}|{}|{}|{}|{}|{}|{}|{}|{}\n",
            slot.service_id as u32,
            version_text_or_empty(slot, slot.installed),
            version_text_or_empty(slot, slot.active),
            version_text_or_empty(slot, slot.rollback),
            slot.pin_version.as_str().unwrap_or(""),
            slot.channel as u32,
            slot.ring as u32,
            active_manifest,
            rollback_manifest,
        );
    }
    write_storage_file(
        storage_handle,
        "state/packages/installed.cfg",
        text.as_bytes(),
    )
}

pub(crate) fn load_installed_state(
    storage_handle: rt::Handle,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: &mut usize,
) -> rt::Result<()> {
    let (blob, len) = match rt::storage_open(storage_handle, "state/packages/installed.cfg") {
        Ok(value) => value,
        Err(rt::Error::NotFound) => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut bytes = [0u8; MAX_STATE_BYTES];
    let requested = len.min(bytes.len());
    let loaded = rt::storage_read_all(blob, &mut bytes, requested)?;
    let _ = rt::storage_blob_close(blob);
    let text = core::str::from_utf8(&bytes[..loaded]).map_err(|_| rt::Error::InvalidArgument)?;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if line.starts_with("version=") {
            continue;
        }
        let Some(payload) = line.strip_prefix("pkg=") else {
            continue;
        };
        let mut parts = payload.split('|');
        let service_id = parts
            .next()
            .and_then(|value| value.parse::<u32>().ok())
            .map(|value| service_id_from_word(value as u64))
            .unwrap_or(ServiceId::RootManager);
        let installed_version = parts.next().unwrap_or("");
        let active_version = parts.next().unwrap_or("");
        let rollback_version = parts.next().unwrap_or("");
        let pin_version = parts.next().unwrap_or("");
        let channel = parts
            .next()
            .and_then(|value| value.parse::<u32>().ok())
            .map(|value| package_channel_from_word(value as u64))
            .unwrap_or(PackageChannel::Stable);
        let ring = parts
            .next()
            .and_then(|value| value.parse::<u32>().ok())
            .map(|value| package_ring_from_word(value as u64))
            .unwrap_or(PackageRing::Production);
        let active_manifest_path = parts.next().unwrap_or("");
        let rollback_manifest_path = parts.next().unwrap_or("");
        let Some(index) = find_package_slot(packages, service_id, *package_count) else {
            continue;
        };
        packages[index].channel = channel;
        packages[index].ring = ring;
        packages[index].pin_version = InlinePath::empty();
        if !pin_version.is_empty() {
            let _ = packages[index].pin_version.set(pin_version);
        }
        if !active_manifest_path.is_empty() {
            let _ = load_local_manifest_slot(
                storage_handle,
                repos,
                &mut packages[index],
                package_count,
                active_manifest_path,
                PackageTrustState::DigestPinned,
            );
        }
        if !rollback_manifest_path.is_empty() {
            let _ = load_local_manifest_slot(
                storage_handle,
                repos,
                &mut packages[index],
                package_count,
                rollback_manifest_path,
                PackageTrustState::DigestPinned,
            );
        }
        packages[index].installed = find_version_by_name(&packages[index], installed_version);
        packages[index].active = find_version_by_name(&packages[index], active_version);
        packages[index].rollback = find_version_by_name(&packages[index], rollback_version);
    }
    Ok(())
}

fn load_local_manifest_slot(
    storage_handle: rt::Handle,
    _repos: &[RepositorySlot; MAX_REPOSITORIES],
    slot: &mut PackageSlot,
    _package_count: &mut usize,
    manifest_path: &str,
    trust_state: PackageTrustState,
) -> rt::Result<()> {
    let manifest =
        crate::operations::load_manifest_from_storage_path(storage_handle, manifest_path)?;
    let version = manifest.version.as_str().unwrap_or("0.0.0");
    let (index, inherited_repo_index, inherited_trust_state) =
        if let Some(existing) = find_version_by_name(slot, version) {
            (
                existing,
                slot.versions[existing].repo_index,
                slot.versions[existing].trust_state,
            )
        } else {
            (
                {
                    if slot.version_count == slot.versions.len() {
                        return Err(rt::Error::CapacityExceeded);
                    }
                    slot.version_count += 1;
                    slot.version_count - 1
                },
                BUILTIN_REPOSITORY_INDEX,
                trust_state,
            )
        };
    slot.versions[index] = PackageVersionSlot::empty();
    slot.versions[index].manifest = manifest;
    slot.versions[index].manifest_loaded = true;
    slot.versions[index].repo_index = inherited_repo_index;
    let _ = slot.versions[index].repo_manifest_path.set(manifest_path);
    let _ = slot.versions[index].local_manifest_path.set(manifest_path);
    let _ = slot.versions[index].version.set(version);
    let _ = slot.versions[index].compatibility.set(
        manifest
            .compatibility
            .as_str()
            .unwrap_or("serviceos.bootstore.v1"),
    );
    let _ = slot.versions[index]
        .category
        .set(slot.package_name.as_str().unwrap_or("PACKAGE"));
    let _ = slot.versions[index]
        .summary
        .set(slot.package_name.as_str().unwrap_or("PACKAGE"));
    slot.versions[index].trust_state = inherited_trust_state;
    slot.versions[index].occupied = true;
    sort_package_versions(slot);
    Ok(())
}

pub(crate) fn persist_journal_state(
    storage_handle: rt::Handle,
    journal: JournalState,
) -> rt::Result<()> {
    let mut text = rt::FixedLogBuffer::<256>::new();
    let _ = write!(&mut text, "version=1\n");
    if journal.pending_action != JOURNAL_NONE {
        let _ = write!(
            &mut text,
            "pending={}|{}|{}|{}\n",
            journal.pending_action,
            journal.service_id as u32,
            journal.version.as_str().unwrap_or(""),
            journal.manifest_path.as_str().unwrap_or(""),
        );
    }
    write_storage_file(
        storage_handle,
        "state/packages/journal.cfg",
        text.as_bytes(),
    )
}

pub(crate) fn load_journal_state(
    storage_handle: rt::Handle,
    journal: &mut JournalState,
) -> rt::Result<()> {
    let (blob, len) = match rt::storage_open(storage_handle, "state/packages/journal.cfg") {
        Ok(value) => value,
        Err(rt::Error::NotFound) => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut bytes = [0u8; 256];
    let requested = len.min(bytes.len());
    let loaded = rt::storage_read_all(blob, &mut bytes, requested)?;
    let _ = rt::storage_blob_close(blob);
    let text = core::str::from_utf8(&bytes[..loaded]).map_err(|_| rt::Error::InvalidArgument)?;
    for line in text.lines().map(str::trim) {
        let Some(payload) = line.strip_prefix("pending=") else {
            continue;
        };
        let mut parts = payload.split('|');
        journal.pending_action = parts
            .next()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(JOURNAL_NONE);
        journal.service_id = parts
            .next()
            .and_then(|v| v.parse::<u32>().ok())
            .map(|v| service_id_from_word(v as u64))
            .unwrap_or(ServiceId::RootManager);
        journal.version = InlinePath::empty();
        let _ = journal.version.set(parts.next().unwrap_or(""));
        journal.manifest_path = InlinePath::empty();
        let _ = journal.manifest_path.set(parts.next().unwrap_or(""));
    }
    Ok(())
}

pub(crate) const FEED_KEYS_PATH: &str = "state/packages/feed-keys.cfg";
pub(crate) const REJECT_JOURNAL_PATH: &str = "state/packages/feed-journal.cfg";
pub(crate) const ROLLOUT_POLICY_PATH: &str = "state/packages/policy.cfg";

pub(crate) const SYSUPDATE_TXN_PATH: &str = "state/packages/sysupdate-txn.cfg";
pub(crate) const SYSUPDATE_HISTORY_PATH: &str = "state/packages/sysupdate-history.cfg";

/// Persist the whole-system update transaction file (state machine marker,
/// step cursor, ordered package ids). An empty write clears it.
pub(crate) fn persist_sysupdate_txn(
    storage_handle: rt::Handle,
    txn: &sysupdate_model::ParsedTxn,
) -> rt::Result<()> {
    let mut text = crate::sysupdate_model::ModelTextBuffer::<512>::new();
    if txn.count > 0 || txn.state != crate::sysupdate_model::TXN_STATE_PLANNING {
        crate::sysupdate_model::encode_txn_file(
            txn.state, txn.done, &txn.ids, txn.count, &mut text,
        );
    } else {
        let _ = write!(&mut text, "version=1\n");
    }
    write_storage_file(storage_handle, SYSUPDATE_TXN_PATH, text.as_bytes())
}

pub(crate) fn clear_sysupdate_txn(storage_handle: rt::Handle) -> rt::Result<()> {
    persist_sysupdate_txn(storage_handle, &crate::sysupdate_model::ParsedTxn::empty())
}

pub(crate) fn load_sysupdate_txn(
    storage_handle: rt::Handle,
) -> rt::Result<Option<crate::sysupdate_model::ParsedTxn>> {
    let (blob, len) = match rt::storage_open(storage_handle, SYSUPDATE_TXN_PATH) {
        Ok(value) => value,
        Err(rt::Error::NotFound) => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = [0u8; 512];
    let requested = len.min(bytes.len());
    let loaded = rt::storage_read_all(blob, &mut bytes, requested)?;
    let _ = rt::storage_blob_close(blob);
    let text = core::str::from_utf8(&bytes[..loaded]).map_err(|_| rt::Error::InvalidArgument)?;
    Ok(crate::sysupdate_model::parse_txn_file(text))
}

/// Append one history row by rewriting the bounded ring file.
pub(crate) fn append_sysupdate_history(
    storage_handle: rt::Handle,
    seq: u64,
    tick: u64,
    applied: u64,
    rolled_back: bool,
) -> rt::Result<()> {
    let loaded = load_sysupdate_history(storage_handle)
        .unwrap_or_else(|_| crate::sysupdate_model::parse_history_rows(""));
    let mut ring = loaded;
    crate::sysupdate_model::push_history_row(
        &mut ring,
        crate::sysupdate_model::HistoryRow {
            seq,
            tick,
            applied,
            rolled_back,
        },
    );
    persist_sysupdate_history(storage_handle, &ring)
}

fn persist_sysupdate_history(
    storage_handle: rt::Handle,
    ring: &(
        [crate::sysupdate_model::HistoryRow; crate::sysupdate_model::SYSUPDATE_HISTORY_CAP],
        usize,
    ),
) -> rt::Result<()> {
    let (rows, count) = ring;
    let mut body = crate::sysupdate_model::ModelTextBuffer::<1024>::new();
    let _ = write!(&mut body, "version=1\n");
    let kept = (*count).min(crate::sysupdate_model::SYSUPDATE_HISTORY_CAP);
    for row in rows[..kept].iter() {
        let mut line = crate::sysupdate_model::ModelTextBuffer::<128>::new();
        crate::sysupdate_model::encode_history_line(
            row.seq,
            row.tick,
            row.applied,
            row.rolled_back,
            &mut line,
        );
        let _ = body.write_str(core::str::from_utf8(line.as_bytes()).unwrap_or(""));
    }
    write_storage_file(storage_handle, SYSUPDATE_HISTORY_PATH, body.as_bytes())
}

pub(crate) fn load_sysupdate_history(
    storage_handle: rt::Handle,
) -> rt::Result<(
    [crate::sysupdate_model::HistoryRow; crate::sysupdate_model::SYSUPDATE_HISTORY_CAP],
    usize,
)> {
    let (blob, len) = match rt::storage_open(storage_handle, SYSUPDATE_HISTORY_PATH) {
        Ok(value) => value,
        Err(rt::Error::NotFound) => return Ok(crate::sysupdate_model::parse_history_rows("")),
        Err(error) => return Err(error),
    };
    let mut bytes = [0u8; 1024];
    let requested = len.min(bytes.len());
    let loaded = rt::storage_read_all(blob, &mut bytes, requested)?;
    let _ = rt::storage_blob_close(blob);
    let text = core::str::from_utf8(&bytes[..loaded]).map_err(|_| rt::Error::InvalidArgument)?;
    Ok(crate::sysupdate_model::parse_history_rows(text))
}

pub(crate) fn persist_feed_keystore(storage_handle: rt::Handle) -> rt::Result<()> {
    let mut text = rt::FixedLogBuffer::<MAX_STATE_BYTES>::new();
    crate::signing::serialize_keystore(unsafe { &*core::ptr::addr_of!(FEED_KEYSTORE) }, &mut text);
    write_storage_file(storage_handle, FEED_KEYS_PATH, text.as_bytes())
}

pub(crate) fn persist_rollout_policy(storage_handle: rt::Handle) -> rt::Result<()> {
    let mut text = rt::FixedLogBuffer::<MAX_STATE_BYTES>::new();
    crate::rollout::serialize_policy(unsafe { &*core::ptr::addr_of!(ROLLOUT_POLICY) }, &mut text);
    write_storage_file(storage_handle, ROLLOUT_POLICY_PATH, text.as_bytes())
}

pub(crate) fn load_rollout_policy(storage_handle: rt::Handle) -> rt::Result<()> {
    let (blob, len) = match rt::storage_open(storage_handle, ROLLOUT_POLICY_PATH) {
        Ok(value) => value,
        Err(rt::Error::NotFound) => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut bytes = [0u8; MAX_STATE_BYTES];
    let requested = len.min(bytes.len());
    let loaded = rt::storage_read_all(blob, &mut bytes, requested)?;
    let _ = rt::storage_blob_close(blob);
    let text = core::str::from_utf8(&bytes[..loaded]).map_err(|_| rt::Error::InvalidArgument)?;
    let parsed = crate::rollout::parse_policy(text);
    unsafe {
        ROLLOUT_POLICY = parsed;
    }
    Ok(())
}

pub(crate) fn load_feed_keystore(storage_handle: rt::Handle) -> rt::Result<()> {
    let (blob, len) = match rt::storage_open(storage_handle, FEED_KEYS_PATH) {
        Ok(value) => value,
        Err(rt::Error::NotFound) => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut bytes = [0u8; MAX_STATE_BYTES];
    let requested = len.min(bytes.len());
    let loaded = rt::storage_read_all(blob, &mut bytes, requested)?;
    let _ = rt::storage_blob_close(blob);
    let text = core::str::from_utf8(&bytes[..loaded]).map_err(|_| rt::Error::InvalidArgument)?;
    let parsed = crate::signing::parse_keystore(text);
    unsafe {
        FEED_KEYSTORE = parsed;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    #[test]
    fn repo_record_parser_preserves_legacy_rows() {
        let repo = parse_repo_record("edge|http://example.test/feed|3|77|2|3|1|99|4")
            .expect("legacy repo");
        assert_eq!(repo.trust_mode, PackageRepositoryTrustMode::PinnedDigest);
        assert_eq!(repo.pinned_digest, 77);
        assert_eq!(repo.last_digest, 99);
        assert_eq!(repo.sync_state, PackageRepositorySyncState::Failed);
        assert!(repo.bound_key_id.is_empty());
        assert_eq!(repo.bound_key_fingerprint, 0);
    }

    #[test]
    fn repo_record_parser_roundtrips_signed_key_binding() {
        let mut repo = RepositorySlot::empty();
        let _ = repo.name.set("edge");
        let _ = repo.url.set("http://example.test/feed");
        repo.trust_mode = PackageRepositoryTrustMode::SignedKey;
        repo.channel = PackageChannel::Beta;
        repo.ring = PackageRing::Preview;
        repo.enabled = true;
        repo.last_digest = 42;
        repo.bound_key_fingerprint = 0x0123_4567_89ab_cdef;
        let _ = repo.bound_key_id.set("k-0123456789abcdef");

        let mut text = rt::FixedLogBuffer::<MAX_STATE_BYTES>::new();
        write_repo_record(&mut text, repo);
        let payload = text.as_str().trim().trim_start_matches("repo=");
        let parsed = parse_repo_record(payload).expect("signed repo");
        assert_eq!(parsed.trust_mode, PackageRepositoryTrustMode::SignedKey);
        assert_eq!(parsed.bound_key_id.as_str(), "k-0123456789abcdef");
        assert_eq!(parsed.bound_key_fingerprint, 0x0123_4567_89ab_cdef);
        assert_eq!(parsed.last_digest, 42);
    }
}

pub(crate) fn persist_reject_journal(storage_handle: rt::Handle) -> rt::Result<()> {
    let mut text = rt::FixedLogBuffer::<MAX_STATE_BYTES>::new();
    unsafe { (*core::ptr::addr_of!(REJECT_JOURNAL)).serialize(&mut text) };
    write_storage_file(storage_handle, REJECT_JOURNAL_PATH, text.as_bytes())
}

pub(crate) fn load_reject_journal(storage_handle: rt::Handle) -> rt::Result<()> {
    let (blob, len) = match rt::storage_open(storage_handle, REJECT_JOURNAL_PATH) {
        Ok(value) => value,
        Err(rt::Error::NotFound) => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut bytes = [0u8; MAX_STATE_BYTES];
    let requested = len.min(bytes.len());
    let loaded = rt::storage_read_all(blob, &mut bytes, requested)?;
    let _ = rt::storage_blob_close(blob);
    let text = core::str::from_utf8(&bytes[..loaded]).map_err(|_| rt::Error::InvalidArgument)?;
    unsafe {
        REJECT_JOURNAL = crate::signing::RejectJournal::parse(text);
    }
    Ok(())
}

fn version_text_or_empty(slot: &PackageSlot, index: Option<usize>) -> &str {
    index.map(|i| version_text(slot, i)).unwrap_or("")
}

pub(crate) fn ensure_directory(storage_handle: rt::Handle, path: &str) -> rt::Result<()> {
    if path.is_empty() {
        return Ok(());
    }
    if rt::storage_open_directory(storage_handle, path, true).is_ok() {
        return Ok(());
    }
    let mut parent = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
    let name = split_parent_path(path, &mut parent)?;
    if !parent.as_str().is_empty() {
        ensure_directory(storage_handle, parent.as_str())?;
    }
    let directory = rt::storage_open_directory(storage_handle, parent.as_str(), true)?;
    let result = rt::storage_directory_create(directory, name, rt::StorageEntryKind::Directory);
    let _ = rt::handle_close(directory);
    match result {
        Ok(()) | Err(rt::Error::Busy) => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) fn ensure_parent_directories(storage_handle: rt::Handle, path: &str) -> rt::Result<()> {
    let mut parent = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
    let _ = split_parent_path(path, &mut parent)?;
    if parent.as_str().is_empty() {
        Ok(())
    } else {
        ensure_directory(storage_handle, parent.as_str())
    }
}

pub(crate) fn write_storage_file(
    storage_handle: rt::Handle,
    path: &str,
    bytes: &[u8],
) -> rt::Result<()> {
    ensure_parent_directories(storage_handle, path)?;
    let mut parent = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
    let name = split_parent_path(path, &mut parent)?;
    let directory = rt::storage_open_directory(storage_handle, parent.as_str(), true)?;
    let (file, _) = rt::storage_directory_open_file(directory, name, true, true)?;
    let _ = rt::handle_close(directory);
    let mut offset = 0usize;
    while offset < bytes.len() {
        let chunk_len = (bytes.len() - offset).min((rt::IPC_MAX_WORDS - 3) * 8);
        let _ = rt::storage_write(
            file,
            offset,
            bytes.len(),
            &bytes[offset..offset + chunk_len],
        )?;
        offset += chunk_len;
    }
    let _ = rt::storage_blob_close(file);
    Ok(())
}

fn split_parent_path<'a>(
    path: &'a str,
    parent_buffer: &mut rt::FixedLogBuffer<INSTALL_PATH_MAX>,
) -> rt::Result<&'a str> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return Err(rt::Error::InvalidArgument);
    }
    match trimmed.rsplit_once('/') {
        Some((parent, name)) if !name.is_empty() => {
            let _ = parent_buffer.write_str(parent);
            let _ = parent_buffer.write_str("/");
            Ok(name)
        }
        Some(_) => Err(rt::Error::InvalidArgument),
        None => Ok(trimmed),
    }
}

pub(crate) fn validate_package_state(
    storage_handle: rt::Handle,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
) -> rt::Result<u32> {
    let mut repaired = 0u32;
    for slot in packages[..package_count]
        .iter_mut()
        .filter(|slot| slot.occupied)
    {
        for index in 0..slot.version_count {
            if let Ok(path) = slot.versions[index].local_manifest_path.as_str() {
                if !path.is_empty() && rt::storage_open(storage_handle, path).is_err() {
                    slot.versions[index].local_manifest_path = InlinePath::empty();
                    if slot.installed == Some(index) {
                        slot.installed = None;
                    }
                    if slot.active == Some(index) {
                        slot.active = None;
                    }
                    if slot.rollback == Some(index) {
                        slot.rollback = None;
                    }
                    repaired = repaired.saturating_add(1);
                }
            }
        }
    }
    Ok(repaired)
}

pub(crate) fn garbage_collect_packages(
    storage_handle: rt::Handle,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
) -> rt::Result<u32> {
    let mut collected = 0u32;
    for slot in packages[..package_count]
        .iter_mut()
        .filter(|slot| slot.occupied)
    {
        for index in 0..slot.version_count {
            if slot.active == Some(index) || slot.rollback == Some(index) {
                continue;
            }
            if let Ok(path) = slot.versions[index].local_manifest_path.as_str() {
                if !path.is_empty() {
                    let root = local_install_root_from_manifest(path)?;
                    recursive_remove(storage_handle, root.as_str())?;
                    slot.versions[index].local_manifest_path = InlinePath::empty();
                    slot.versions[index].manifest_loaded = false;
                    collected = collected.saturating_add(1);
                }
            }
        }
    }
    Ok(collected)
}

fn recursive_remove(storage_handle: rt::Handle, path: &str) -> rt::Result<()> {
    if let Ok(directory) = rt::storage_open_directory(storage_handle, path, true) {
        let mut names = [[0u8; BOOT_STORE_PATH_MAX]; 16];
        let mut kinds = [rt::StorageEntryKind::File; 16];
        let mut name_lens = [0usize; 16];
        let mut count = 0usize;
        let mut cursor = 0usize;
        while let Some((next_cursor, kind, name_len)) =
            rt::storage_directory_read(directory, cursor, &mut names[count])?
        {
            kinds[count] = kind;
            name_lens[count] = name_len;
            count += 1;
            cursor = next_cursor;
            if count == names.len() {
                break;
            }
        }
        let _ = rt::handle_close(directory);
        for index in 0..count {
            let name = core::str::from_utf8(&names[index][..name_lens[index]])
                .map_err(|_| rt::Error::InvalidArgument)?;
            let child = join_path(path, name)?;
            match kinds[index] {
                rt::StorageEntryKind::File => {
                    let mut parent = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
                    let entry = split_parent_path(child.as_str(), &mut parent)?;
                    let dir = rt::storage_open_directory(storage_handle, parent.as_str(), true)?;
                    let _ = rt::storage_directory_remove(dir, entry);
                    let _ = rt::handle_close(dir);
                }
                rt::StorageEntryKind::Directory => {
                    recursive_remove(storage_handle, child.as_str())?
                }
            }
        }
        let mut parent = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
        let entry = split_parent_path(path, &mut parent)?;
        let dir = rt::storage_open_directory(storage_handle, parent.as_str(), true)?;
        let _ = rt::storage_directory_remove(dir, entry);
        let _ = rt::handle_close(dir);
    }
    Ok(())
}

pub(crate) fn install_root_path(
    package: &str,
    version: &str,
) -> rt::Result<rt::FixedLogBuffer<INSTALL_PATH_MAX>> {
    let mut path = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
    let _ = write!(&mut path, "state/packages/install/{}/{}/", package, version);
    Ok(path)
}

pub(crate) fn create_install_root(storage_handle: rt::Handle, path: &str) -> rt::Result<()> {
    ensure_directory(storage_handle, path)
}

pub(crate) fn local_installed_content_path(
    install_root: &str,
    remote_path: &str,
) -> rt::Result<rt::FixedLogBuffer<INSTALL_PATH_MAX>> {
    let mut path = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
    let _ = write!(
        &mut path,
        "{}root/{}",
        install_root,
        remote_path.trim_start_matches('/')
    );
    Ok(path)
}

pub(crate) fn local_installed_manifest_path(install_root: &str) -> rt::Result<InlinePath> {
    let mut path = InlinePath::empty();
    let mut text = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
    let _ = write!(&mut text, "{}package.pkg", install_root);
    path.set(text.as_str())
        .map_err(|_| rt::Error::InvalidArgument)?;
    Ok(path)
}

pub(crate) fn local_install_root_from_manifest(
    path: &str,
) -> rt::Result<rt::FixedLogBuffer<INSTALL_PATH_MAX>> {
    let mut parent = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
    let _ = split_parent_path(path, &mut parent)?;
    Ok(parent)
}

pub(crate) fn join_path(
    left: &str,
    right: &str,
) -> rt::Result<rt::FixedLogBuffer<INSTALL_PATH_MAX>> {
    let mut out = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
    let _ = out.write_str(left.trim_end_matches('/'));
    let _ = out.write_str("/");
    let _ = out.write_str(right.trim_start_matches('/'));
    Ok(out)
}

pub(crate) fn rewrite_manifest_for_install(
    mut manifest: PackageManifest,
    install_root: &str,
) -> rt::Result<PackageManifest> {
    let mut manifest_path = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
    let _ = write!(
        &mut manifest_path,
        "{}root/{}",
        install_root,
        manifest
            .service_manifest
            .as_str()
            .unwrap_or("")
            .trim_start_matches('/')
    );
    let _ = manifest.service_manifest.set(manifest_path.as_str());
    for content in manifest.contents[..manifest.content_count].iter_mut() {
        let mut path = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
        let _ = write!(
            &mut path,
            "{}root/{}",
            install_root,
            content.as_str().unwrap_or("").trim_start_matches('/')
        );
        let _ = content.set(path.as_str());
    }
    Ok(manifest)
}

pub(crate) fn serialize_package_manifest(
    manifest: PackageManifest,
) -> rt::Result<rt::FixedLogBuffer<MAX_PACKAGE_BYTES>> {
    let mut out = rt::FixedLogBuffer::<MAX_PACKAGE_BYTES>::new();
    let _ = write!(
        &mut out,
        "package={}\nversion={}\ncompat={}\nservice={}\nservice_manifest={}\nactivation={}\n",
        manifest.package.as_str().unwrap_or("package"),
        manifest.version.as_str().unwrap_or("0.0.0"),
        manifest
            .compatibility
            .as_str()
            .unwrap_or("serviceos.bootstore.v1"),
        service_name(manifest.service_id),
        manifest.service_manifest.as_str().unwrap_or(""),
        match manifest.activation {
            serviceos_bundle::PackageActivationMode::Manual => "manual",
            serviceos_bundle::PackageActivationMode::Auto => "auto",
        }
    );
    if manifest.dependency_count > 0 {
        let _ = out.write_str("depends=");
        for index in 0..manifest.dependency_count {
            if index > 0 {
                let _ = out.write_str(",");
            }
            let _ = out.write_str(service_name(manifest.dependencies[index]));
        }
        let _ = out.write_str("\n");
    }
    for content in manifest.contents[..manifest.content_count].iter() {
        let _ = write!(&mut out, "content={}\n", content.as_str().unwrap_or(""));
    }
    let _ = write!(&mut out, "integrity=fnv64:0x{:016x}\n", manifest.integrity);
    Ok(out)
}

#![no_std]
#![no_main]

use core::{
    cmp::Ordering,
    fmt::Write,
    str,
};

use serviceos_bundle::{
    BOOT_STORE_PATH_MAX, InlinePath, PackageManifest, parse_package_manifest,
};
use serviceos_userspace_runtime as rt;
use rt::{
    ControlTag, LifecycleEvent, LogDomain, LogEvent, LogSeverity, PackageChannel,
    PackageMaintenanceAction, PackageRepositorySyncState, PackageRepositoryTrustMode, PackageRing,
    PackageStatus, PackageTag, PackageTrustState, RawMessage, ServiceId, IPC_MAX_WORDS,
};

const MAX_INDEX_BYTES: usize = 512;
const MAX_PACKAGE_BYTES: usize = 2048;
const MAX_FEED_BYTES: usize = 4096;
const MAX_HTTP_BYTES: usize = 4096;
const MAX_STATE_BYTES: usize = 2048;
const MAX_PACKAGE_SLOTS: usize = 12;
const MAX_PACKAGE_VERSIONS: usize = 8;
const MAX_REPOSITORIES: usize = 4;
const BUILTIN_REPOSITORY_INDEX: usize = 0;
const REPO_NAME_MAX: usize = 24;
const REPO_URL_MAX: usize = 88;
const INSTALL_PATH_MAX: usize = BOOT_STORE_PATH_MAX;
const HTTP_TIMEOUT_TICKS: u64 = 600;
const HTTP_CHUNK_BYTES: usize = (IPC_MAX_WORDS - 2) * 8;
const JOURNAL_NONE: u32 = 0;
const JOURNAL_INSTALL: u32 = 1;
const JOURNAL_UPDATE: u32 = 2;
const JOURNAL_REMOVE: u32 = 3;
const JOURNAL_ROLLBACK: u32 = 4;

#[derive(Clone, Copy)]
struct PackageVersionSlot {
    manifest: PackageManifest,
    manifest_loaded: bool,
    repo_index: usize,
    repo_manifest_path: InlinePath,
    local_manifest_path: InlinePath,
    version: InlinePath,
    compatibility: InlinePath,
    category: InlinePath,
    summary: InlinePath,
    trust_state: PackageTrustState,
    occupied: bool,
}

impl PackageVersionSlot {
    const fn empty() -> Self {
        Self {
            manifest: PackageManifest::empty(),
            manifest_loaded: false,
            repo_index: 0,
            repo_manifest_path: InlinePath::empty(),
            local_manifest_path: InlinePath::empty(),
            version: InlinePath::empty(),
            compatibility: InlinePath::empty(),
            category: InlinePath::empty(),
            summary: InlinePath::empty(),
            trust_state: PackageTrustState::BootTrusted,
            occupied: false,
        }
    }
}

#[derive(Clone, Copy)]
struct PackageSlot {
    service_id: ServiceId,
    package_name: InlinePath,
    versions: [PackageVersionSlot; MAX_PACKAGE_VERSIONS],
    version_count: usize,
    installed: Option<usize>,
    active: Option<usize>,
    rollback: Option<usize>,
    pin_version: InlinePath,
    channel: PackageChannel,
    ring: PackageRing,
    occupied: bool,
}

impl PackageSlot {
    const fn empty() -> Self {
        Self {
            service_id: ServiceId::RootManager,
            package_name: InlinePath::empty(),
            versions: [PackageVersionSlot::empty(); MAX_PACKAGE_VERSIONS],
            version_count: 0,
            installed: None,
            active: None,
            rollback: None,
            pin_version: InlinePath::empty(),
            channel: PackageChannel::Stable,
            ring: PackageRing::Production,
            occupied: false,
        }
    }
}

#[derive(Clone, Copy)]
struct RepositorySlot {
    name: InlinePath,
    url: InlinePath,
    trust_mode: PackageRepositoryTrustMode,
    sync_state: PackageRepositorySyncState,
    channel: PackageChannel,
    ring: PackageRing,
    enabled: bool,
    builtin: bool,
    pinned_digest: u64,
    last_digest: u64,
    package_count: u32,
    occupied: bool,
}

impl RepositorySlot {
    const fn empty() -> Self {
        Self {
            name: InlinePath::empty(),
            url: InlinePath::empty(),
            trust_mode: PackageRepositoryTrustMode::Unsigned,
            sync_state: PackageRepositorySyncState::Idle,
            channel: PackageChannel::Stable,
            ring: PackageRing::Production,
            enabled: false,
            builtin: false,
            pinned_digest: 0,
            last_digest: 0,
            package_count: 0,
            occupied: false,
        }
    }
}

#[derive(Clone, Copy)]
struct JournalState {
    pending_action: u32,
    service_id: ServiceId,
    version: InlinePath,
    manifest_path: InlinePath,
}

impl JournalState {
    const fn empty() -> Self {
        Self {
            pending_action: JOURNAL_NONE,
            service_id: ServiceId::RootManager,
            version: InlinePath::empty(),
            manifest_path: InlinePath::empty(),
        }
    }
}

static mut REPOSITORY_SLOTS: [RepositorySlot; MAX_REPOSITORIES] =
    [RepositorySlot::empty(); MAX_REPOSITORIES];
static mut PACKAGE_SLOTS: [PackageSlot; MAX_PACKAGE_SLOTS] = [PackageSlot::empty(); MAX_PACKAGE_SLOTS];
static mut JOURNAL_SLOT: JournalState = JournalState::empty();

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xfa01;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.word_count < 5 || startup.words[2] < 1 {
        return 0xfa02;
    }

    let log_handle = startup.handles[0];
    let storage_handle = match rt::lookup_service(bootstrap, ServiceId::Storage) {
        Ok(handle) => handle,
        Err(_) => return 0xfa03,
    };
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network).ok();

    let repos = unsafe { &mut *core::ptr::addr_of_mut!(REPOSITORY_SLOTS) };
    let packages = unsafe { &mut *core::ptr::addr_of_mut!(PACKAGE_SLOTS) };
    let journal = unsafe { &mut *core::ptr::addr_of_mut!(JOURNAL_SLOT) };
    *repos = [RepositorySlot::empty(); MAX_REPOSITORIES];
    *packages = [PackageSlot::empty(); MAX_PACKAGE_SLOTS];
    *journal = JournalState::empty();
    initialize_builtin_repository(&mut repos[BUILTIN_REPOSITORY_INDEX]);

    let mut package_count = match load_boot_catalog(storage_handle, repos, packages) {
        Ok(count) => count,
        Err(_) => return 0xfa04,
    };
    if initialize_state_directories(storage_handle).is_err() {
        let _ = emit_package_event(
            log_handle,
            LogSeverity::Warn,
            LogEvent::PackageRepairCompleted,
            0,
            0,
        );
    }
    let mut repo_count = 1usize;
    let _ = load_persisted_repositories(storage_handle, repos, &mut repo_count);
    for repo_index in 1..repo_count {
        let _ = load_repo_feed_cache(
            storage_handle,
            repos,
            repo_index,
            packages,
            &mut package_count,
        );
    }
    let _ = load_journal_state(storage_handle, journal);
    let _ = load_installed_state(
        storage_handle,
        repos,
        packages,
        &mut package_count,
    );

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xfa05,
    };
    if rt::register_service(bootstrap, ServiceId::Package, public.second).is_err() {
        return 0xfa06;
    }
    let _ = rt::handle_close(public.second);

    let _ = emit_package_event(
        log_handle,
        LogSeverity::Info,
        LogEvent::PackageCatalogLoaded,
        package_count as u64,
        total_versions(&packages, package_count) as u64,
    );

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xfa07,
        }

        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                if handle_request(
                    bootstrap,
                    storage_handle,
                    network_handle,
                    log_handle,
                    repos,
                    &mut repo_count,
                    packages,
                    &mut package_count,
                    journal,
                    &request,
                )
                .is_err()
                {
                    return 0xfa08;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xfa09,
        }

        if rt::yield_current().is_err() {
            return 0xfa0a;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_request(
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
        x if x == PackageTag::ListRequest as u32 => handle_list_request(packages, *package_count, message),
        x if x == PackageTag::InfoRequest as u32 => handle_info_request(packages, *package_count, message),
        x if x == PackageTag::HistoryRequest as u32 => handle_history_request(packages, *package_count, message),
        x if x == PackageTag::CatalogRequest as u32 => handle_catalog_request(packages, *package_count, message),
        x if x == PackageTag::RepositoryListRequest as u32 => handle_repository_list_request(repos, *repo_count, message),
        x if x == PackageTag::RepositoryAddRequest as u32 => handle_repository_add_request(
            storage_handle,
            log_handle,
            repos,
            repo_count,
            message,
        ),
        x if x == PackageTag::RepositorySyncRequest as u32 => handle_repository_sync_request(
            storage_handle,
            network_handle,
            log_handle,
            repos,
            *repo_count,
            packages,
            package_count,
            message,
        ),
        x if x == PackageTag::ProvenanceRequest as u32 => {
            handle_provenance_request(repos, packages, *package_count, message)
        }
        x if x == PackageTag::PolicyRequest as u32 => handle_policy_request(packages, *package_count, message),
        x if x == PackageTag::PolicySetRequest as u32 => handle_policy_set_request(
            storage_handle,
            packages,
            *package_count,
            message,
        ),
        x if x == PackageTag::MaintenanceRequest as u32 => handle_maintenance_request(
            storage_handle,
            log_handle,
            repos,
            *repo_count,
            packages,
            *package_count,
            journal,
            message,
        ),
        x if x == PackageTag::InstallRequest as u32 => handle_install_request(
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
        x if x == PackageTag::UpdateRequest as u32 => handle_update_request(
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
        x if x == PackageTag::RemoveRequest as u32 => handle_remove_request(
            bootstrap,
            storage_handle,
            log_handle,
            packages,
            *package_count,
            journal,
            message,
        ),
        x if x == PackageTag::RollbackRequest as u32 => handle_rollback_request(
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

fn initialize_builtin_repository(repo: &mut RepositorySlot) {
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

fn load_boot_catalog(
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
    for line in index_text.lines().map(|line| line.trim()).filter(|line| !line.is_empty()) {
        let (manifest_handle, manifest_len) = rt::storage_open(storage_handle, line)?;
        let mut manifest_buffer = [0u8; MAX_PACKAGE_BYTES];
        let requested = manifest_len.min(manifest_buffer.len());
        let loaded = rt::storage_read_all(manifest_handle, &mut manifest_buffer, requested)?;
        let _ = rt::storage_blob_close(manifest_handle);
        let manifest =
            parse_package_manifest(&manifest_buffer[..loaded]).map_err(|_| rt::Error::InvalidArgument)?;
        let latest = add_or_update_version(
            packages,
            &mut count,
            manifest.service_id,
            manifest.package.as_str().unwrap_or("package"),
            manifest.version.as_str().unwrap_or("0.0.0"),
            manifest.compatibility.as_str().unwrap_or("serviceos.bootstore.v1"),
            line,
            "",
            manifest.package.as_str().unwrap_or("SYSTEM"),
            manifest.package.as_str().unwrap_or("SERVICE PACKAGE"),
            BUILTIN_REPOSITORY_INDEX,
            PackageTrustState::BootTrusted,
            Some(manifest),
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
fn add_or_update_version(
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
    manifest: Option<PackageManifest>,
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
        let mut slot = PackageSlot::empty();
        slot.service_id = service_id;
        let _ = slot.package_name.set(package_name);
        slot.channel = channel;
        slot.ring = ring;
        slot.occupied = true;
        packages[*package_count] = slot;
        let index = *package_count;
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
            version_slot.manifest = manifest;
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
    let mut version_slot = PackageVersionSlot::empty();
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
        version_slot.manifest = manifest;
        version_slot.manifest_loaded = true;
    }
    slot.versions[slot.version_count] = version_slot;
    slot.version_count += 1;
    if let Some(pin) = pin_version {
        let _ = slot.pin_version.set(pin);
    }
    slot.channel = channel;
    slot.ring = ring;
    sort_package_versions(slot);
    find_version_by_name(slot, version).ok_or(rt::Error::NotFound)
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
    if let Some(slot) = packages[..package_count].get(index).copied().filter(|slot| slot.occupied) {
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
    if let Some(slot) = packages[..package_count].get(index).copied().filter(|slot| slot.occupied) {
        let latest = latest_version_index(&slot);
        let latest_text = version_bytes(&slot, latest);
        let category = slot
            .versions[latest.unwrap_or(0)]
            .category
            .as_str()
            .unwrap_or("SERVICE");
        let summary = slot
            .versions[latest.unwrap_or(0)]
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
    if let Some(repo) = repos[..repo_count].get(index).copied().filter(|repo| repo.occupied) {
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

fn handle_repository_add_request(
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
        let name = core::str::from_utf8(&bytes[..name_len]).map_err(|_| rt::Error::InvalidArgument)?;
        let url =
            core::str::from_utf8(&bytes[name_len..name_len + url_len]).map_err(|_| rt::Error::InvalidArgument)?;
        add_repository(storage_handle, log_handle, repos, repo_count, name, url, trust_mode, channel, ring, enabled, pinned_digest)?
    };
    send_status_reply(reply_handle, PackageTag::RepositoryAddReply, status)
}

#[allow(clippy::too_many_arguments)]
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
    persist_repositories(storage_handle, repos, *repo_count)?;
    let _ = emit_package_event(
        log_handle,
        LogSeverity::Info,
        LogEvent::PackageRepositoryAdded,
        index as u64,
        trust_mode as u32 as u64,
    );
    Ok(PackageStatus::Ok)
}

fn handle_repository_sync_request(
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

fn sync_repository(
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
    let url = repos[repo_index].url.as_str().map_err(|_| rt::Error::InvalidArgument)?;
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
            persist_repositories(storage_handle, repos, count_repositories(repos))?;
            return Ok(PackageStatus::Offline);
        }
    };
    let digest = compute_fnv64(&bytes[..loaded]);
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
                persist_repositories(storage_handle, repos, count_repositories(repos))?;
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
    persist_repositories(storage_handle, repos, count_repositories(repos))?;
    persist_repo_feed_cache(storage_handle, repos[repo_index], &bytes[..loaded])?;
    let _ = emit_package_event(
        log_handle,
        LogSeverity::Info,
        LogEvent::PackageRepositorySynced,
        repo_index as u64,
        repos[repo_index].package_count as u64,
    );
    Ok(PackageStatus::Ok)
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
                .and_then(|version_index| slot.versions[version_index].repo_manifest_path.as_str().ok())
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
        persist_installed_state(storage_handle, packages, package_count)?;
        PackageStatus::Ok
    } else {
        PackageStatus::NotFound
    };
    send_status_reply(reply_handle, PackageTag::PolicySetReply, status)
}

fn handle_maintenance_request(
    storage_handle: rt::Handle,
    log_handle: rt::Handle,
    _repos: &[RepositorySlot; MAX_REPOSITORIES],
    _repo_count: usize,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    journal: &mut JournalState,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 1 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let action = maintenance_action_from_word(message.words[0]);
    let (status, repaired, collected) = match action {
        PackageMaintenanceAction::Validate => {
            (
                PackageStatus::Ok,
                validate_package_state(storage_handle, packages, package_count)?,
                0,
            )
        }
        PackageMaintenanceAction::Repair => {
            let mut repaired = validate_package_state(storage_handle, packages, package_count)?;
            if journal.pending_action != JOURNAL_NONE {
                journal.pending_action = JOURNAL_NONE;
                persist_journal_state(storage_handle, *journal)?;
                repaired = repaired.saturating_add(1);
            }
            (PackageStatus::Ok, repaired, 0)
        }
        PackageMaintenanceAction::GarbageCollect => {
            (
                PackageStatus::Ok,
                validate_package_state(storage_handle, packages, package_count)?,
                garbage_collect_packages(storage_handle, packages, package_count)?,
            )
        }
    };
    persist_installed_state(storage_handle, packages, package_count)?;
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
    let mut reply = RawMessage::empty(PackageTag::MaintenanceReply as u32);
    reply.word_count = 3;
    reply.words[0] = status as u32 as u64;
    reply.words[1] = repaired as u64;
    reply.words[2] = collected as u64;
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_install_request(
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
        persist_journal_state(storage_handle, *journal)?;
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
            let _ = persist_installed_state(storage_handle, packages, package_count);
            *journal = JournalState::empty();
            let _ = persist_journal_state(storage_handle, *journal);
        }
        status
    } else {
        PackageStatus::NotFound
    };
    send_status_reply(reply_handle, PackageTag::InstallReply, status)
}

#[allow(clippy::too_many_arguments)]
fn handle_update_request(
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
                    persist_journal_state(storage_handle, *journal)?;
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
                        let _ = persist_installed_state(storage_handle, packages, package_count);
                        *journal = JournalState::empty();
                        let _ = persist_journal_state(storage_handle, *journal);
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

fn handle_remove_request(
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
            persist_journal_state(storage_handle, *journal)?;
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
                    let _ = persist_installed_state(storage_handle, packages, package_count);
                    *journal = JournalState::empty();
                    let _ = persist_journal_state(storage_handle, *journal);
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
fn handle_rollback_request(
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
            persist_journal_state(storage_handle, *journal)?;
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
                let _ = persist_installed_state(storage_handle, packages, package_count);
                *journal = JournalState::empty();
                let _ = persist_journal_state(storage_handle, *journal);
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

    let materialized = ensure_version_materialized(
        storage_handle,
        network_handle,
        slot,
        target,
        repos,
    );
    if materialized != PackageStatus::Ok {
        return materialized;
    }

    let manifest_path = active_manifest_path(&slot.versions[target]);
    let manifest = match load_manifest_from_storage_path(storage_handle, manifest_path) {
        Ok(manifest) => manifest,
        Err(_) => return PackageStatus::NotFound,
    };
    match verify_package_integrity(storage_handle, manifest) {
        Ok(true) => {}
        Ok(false) => return PackageStatus::IntegrityFailed,
        Err(_) => return PackageStatus::IntegrityFailed,
    }

    let previous = slot.active;
    match rt::manager_activate_service(
        bootstrap,
        manifest.service_manifest.as_str().unwrap_or(""),
    ) {
        Ok(_) => {
            slot.rollback = previous;
            slot.installed = Some(target);
            slot.active = Some(target);
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
                if let Ok(previous_manifest) = load_manifest_from_storage_path(storage_handle, previous_path) {
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
    let manifest_url = match join_repo_url(repo_url, manifest_rel) {
        Ok(url) => url,
        Err(_) => return PackageStatus::Unsupported,
    };
    let manifest_loaded = match http_fetch_text(network_handle, manifest_url.as_str(), &mut manifest_bytes) {
        Ok(len) => len,
        Err(_) => return PackageStatus::Offline,
    };
    let remote_manifest = match parse_package_manifest(&manifest_bytes[..manifest_loaded]) {
        Ok(manifest) => manifest,
        Err(_) => return PackageStatus::Unsupported,
    };

    let install_root =
        match install_root_path(slot.package_name.as_str().unwrap_or("package"), version_text(slot, target)) {
        Ok(path) => path,
        Err(_) => return PackageStatus::Busy,
    };
    if create_install_root(storage_handle, install_root.as_str()) != rt::Result::Ok(()) {
        return PackageStatus::Busy;
    }
    for content in remote_manifest.contents[..remote_manifest.content_count].iter() {
        let Ok(remote_path) = content.as_str() else {
            return PackageStatus::Unsupported;
        };
        let url = match join_repo_url(repo_url, remote_path) {
            Ok(url) => url,
            Err(_) => return PackageStatus::Unsupported,
        };
        let local_path = match local_installed_content_path(install_root.as_str(), remote_path) {
            Ok(path) => path,
            Err(_) => return PackageStatus::Busy,
        };
        let mut bytes = [0u8; MAX_HTTP_BYTES];
        let loaded = match http_fetch_text(network_handle, url.as_str(), &mut bytes) {
            Ok(len) => len,
            Err(_) => return PackageStatus::Offline,
        };
        if ensure_parent_directories(storage_handle, local_path.as_str()).is_err() {
            return PackageStatus::Busy;
        }
        if write_storage_file(storage_handle, local_path.as_str(), &bytes[..loaded]).is_err() {
            return PackageStatus::Busy;
        }
    }
    let rewritten = match rewrite_manifest_for_install(remote_manifest, install_root.as_str()) {
        Ok(manifest) => manifest,
        Err(_) => return PackageStatus::Busy,
    };
    let manifest_text = match serialize_package_manifest(rewritten) {
        Ok(text) => text,
        Err(_) => return PackageStatus::Busy,
    };
    let local_manifest_path = match local_installed_manifest_path(install_root.as_str()) {
        Ok(path) => path,
        Err(_) => return PackageStatus::Busy,
    };
    if write_storage_file(
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
    if slot.versions[target].trust_state == PackageTrustState::Unverified
        && repo.trust_mode == PackageRepositoryTrustMode::Unsigned
    {
        slot.versions[target].trust_state = PackageTrustState::Unverified;
    }
    PackageStatus::Ok
}

fn load_manifest_from_storage_path(
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

fn verify_package_integrity(storage_handle: rt::Handle, manifest: PackageManifest) -> rt::Result<bool> {
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

fn compute_fnv64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    update_hash(&mut hash, bytes);
    hash
}

fn sort_package_versions(slot: &mut PackageSlot) {
    let mut index = 1usize;
    while index < slot.version_count {
        let mut inner = index;
        while inner > 0
            && compare_versions(
                version_text(slot, inner - 1),
                version_text(slot, inner),
            ) == Ordering::Greater
        {
            slot.versions.swap(inner - 1, inner);
            inner -= 1;
        }
        index += 1;
    }
}

fn compare_versions(left: &str, right: &str) -> Ordering {
    parse_version_triplet(left).cmp(&parse_version_triplet(right))
}

fn parse_version_triplet(value: &str) -> (u32, u32, u32) {
    let mut parts = value.split('.');
    let major = parts.next().and_then(|part| part.parse::<u32>().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|part| part.parse::<u32>().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|part| part.parse::<u32>().ok()).unwrap_or(0);
    (major, minor, patch)
}

fn latest_version_index(slot: &PackageSlot) -> Option<usize> {
    if slot.version_count == 0 {
        None
    } else {
        Some(slot.version_count - 1)
    }
}

fn find_version_by_name(slot: &PackageSlot, version: &str) -> Option<usize> {
    slot.versions[..slot.version_count]
        .iter()
        .position(|entry| entry.occupied && entry.version.as_str().ok() == Some(version))
}

fn version_text<'a>(slot: &'a PackageSlot, index: usize) -> &'a str {
    slot.versions[index].version.as_str().unwrap_or("")
}

fn version_bytes<'a>(slot: &'a PackageSlot, index: Option<usize>) -> &'a [u8] {
    index
        .and_then(|index| slot.versions.get(index))
        .and_then(|slot| slot.version.as_str().ok())
        .map(|value| value.as_bytes())
        .unwrap_or(&[])
}

fn package_flags(slot: &PackageSlot) -> u32 {
    u32::from(slot.installed.is_some())
        | (u32::from(slot.active.is_some()) << 1)
        | (u32::from(slot.rollback.is_some()) << 2)
}

fn pack_repo_flags(repo: RepositorySlot) -> u32 {
    (repo.trust_mode as u32)
        | ((repo.sync_state as u32) << 8)
        | ((repo.channel as u32) << 16)
        | ((repo.ring as u32) << 24)
        | ((u32::from(repo.enabled)) << 30)
        | ((u32::from(repo.builtin)) << 31)
}

fn pack_provenance_flags(
    trust: PackageTrustState,
    channel: PackageChannel,
    ring: PackageRing,
    package_flags: u32,
) -> u32 {
    trust as u32 | ((channel as u32) << 8) | ((ring as u32) << 16) | (package_flags << 24)
}

fn select_install_target(
    slot: &PackageSlot,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    explicit_version: Option<&str>,
) -> rt::Result<usize> {
    if let Some(version) = explicit_version {
        return find_version_by_name(slot, version).ok_or(rt::Error::NotFound);
    }
    if let Ok(pin) = slot.pin_version.as_str() {
        if !pin.is_empty() {
            return find_version_by_name(slot, pin).ok_or(rt::Error::NotFound);
        }
    }
    for index in (0..slot.version_count).rev() {
        if version_allowed(slot, &slot.versions[index], repos) {
            return Ok(index);
        }
    }
    Err(rt::Error::NotFound)
}

fn select_update_target(
    slot: &PackageSlot,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    explicit_version: Option<&str>,
) -> rt::Result<Option<usize>> {
    let Some(current) = slot.installed else {
        return Ok(None);
    };
    let target = select_install_target(slot, repos, explicit_version)?;
    if target == current {
        Ok(None)
    } else if compare_versions(version_text(slot, target), version_text(slot, current))
        == Ordering::Greater
        || explicit_version.is_some()
    {
        Ok(Some(target))
    } else {
        Ok(None)
    }
}

fn version_allowed(
    slot: &PackageSlot,
    version: &PackageVersionSlot,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
) -> bool {
    let Some(repo) = repos.get(version.repo_index).copied().filter(|repo| repo.occupied) else {
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

fn find_package_slot(
    packages: &[PackageSlot; MAX_PACKAGE_SLOTS],
    service_id: ServiceId,
    package_count: usize,
) -> Option<usize> {
    (0..package_count).find(|index| packages[*index].occupied && packages[*index].service_id == service_id)
}

fn find_repository_index(
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    repo_count: usize,
    name: &str,
) -> Option<usize> {
    (0..repo_count).find(|index| repos[*index].occupied && repos[*index].name.as_str().ok() == Some(name))
}

fn total_versions(packages: &[PackageSlot; MAX_PACKAGE_SLOTS], package_count: usize) -> usize {
    packages[..package_count]
        .iter()
        .filter(|slot| slot.occupied)
        .map(|slot| slot.version_count)
        .sum()
}

fn active_manifest_path(version: &PackageVersionSlot) -> &str {
    version
        .local_manifest_path
        .as_str()
        .ok()
        .filter(|path| !path.is_empty())
        .or_else(|| version.repo_manifest_path.as_str().ok())
        .unwrap_or("")
}

fn emit_package_event(
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

fn parse_version_argument<'a>(
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
    let text = core::str::from_utf8(&buffer[..version_len]).map_err(|_| rt::Error::InvalidArgument)?;
    Ok(Some(text))
}

fn send_status_reply(reply_handle: rt::Handle, tag: PackageTag, status: PackageStatus) -> rt::Result<()> {
    let mut reply = RawMessage::empty(tag as u32);
    reply.word_count = 1;
    reply.words[0] = status as u32 as u64;
    let result = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    result
}

fn encode_version_text(version: &str) -> u64 {
    let (major, minor, patch) = parse_version_triplet(version);
    ((major as u64) << 32) | ((minor as u64) << 16) | patch as u64
}

fn copy_into(destination: &mut [u8], source: &[u8]) -> rt::Result<usize> {
    if source.len() > destination.len() {
        return Err(rt::Error::BufferTooSmall);
    }
    destination[..source.len()].copy_from_slice(source);
    Ok(source.len())
}

fn pack_bytes(source: &[u8], words: &mut [u64]) -> rt::Result<u32> {
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

fn unpack_bytes(words: &[u64], len: usize, destination: &mut [u8]) -> rt::Result<()> {
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

fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
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

fn service_id_from_word(value: u64) -> ServiceId {
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

fn service_id_from_name(value: &str) -> Option<ServiceId> {
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

fn trust_mode_from_word(value: u64) -> PackageRepositoryTrustMode {
    match value as u32 {
        x if x == PackageRepositoryTrustMode::Boot as u32 => PackageRepositoryTrustMode::Boot,
        x if x == PackageRepositoryTrustMode::PinnedDigest as u32 => {
            PackageRepositoryTrustMode::PinnedDigest
        }
        _ => PackageRepositoryTrustMode::Unsigned,
    }
}

fn package_channel_from_word(value: u64) -> PackageChannel {
    match value as u32 {
        x if x == PackageChannel::Beta as u32 => PackageChannel::Beta,
        x if x == PackageChannel::Canary as u32 => PackageChannel::Canary,
        _ => PackageChannel::Stable,
    }
}

fn package_ring_from_word(value: u64) -> PackageRing {
    match value as u32 {
        x if x == PackageRing::Preview as u32 => PackageRing::Preview,
        x if x == PackageRing::Testing as u32 => PackageRing::Testing,
        _ => PackageRing::Production,
    }
}

fn maintenance_action_from_word(value: u64) -> PackageMaintenanceAction {
    match value as u32 {
        x if x == PackageMaintenanceAction::Repair as u32 => PackageMaintenanceAction::Repair,
        x if x == PackageMaintenanceAction::GarbageCollect as u32 => {
            PackageMaintenanceAction::GarbageCollect
        }
        _ => PackageMaintenanceAction::Validate,
    }
}

// persistence helpers

fn initialize_state_directories(storage_handle: rt::Handle) -> rt::Result<()> {
    ensure_directory(storage_handle, "state/")?;
    ensure_directory(storage_handle, "state/packages/")?;
    ensure_directory(storage_handle, "state/packages/repos/")?;
    ensure_directory(storage_handle, "state/packages/install/")?;
    Ok(())
}

fn persist_repositories(
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
        let _ = write!(
            &mut text,
            "repo={}|{}|{}|{}|{}|{}|{}|{}|{}\n",
            repo.name.as_str().unwrap_or("repo"),
            repo.url.as_str().unwrap_or(""),
            repo.trust_mode as u32,
            repo.pinned_digest,
            repo.channel as u32,
            repo.ring as u32,
            u32::from(repo.enabled),
            repo.last_digest,
            repo.sync_state as u32,
        );
    }
    write_storage_file(storage_handle, "state/packages/repos.cfg", text.as_bytes())
}

fn load_persisted_repositories(
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
        let mut parts = payload.split('|');
        let Some(name) = parts.next() else { continue };
        let Some(url) = parts.next() else { continue };
        let trust_mode = parts
            .next()
            .and_then(|value| value.parse::<u32>().ok())
            .map(|value| trust_mode_from_word(value as u64))
            .unwrap_or(PackageRepositoryTrustMode::Unsigned);
        let pinned_digest = parts.next().and_then(|value| value.parse::<u64>().ok()).unwrap_or(0);
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
        let last_digest = parts.next().and_then(|value| value.parse::<u64>().ok()).unwrap_or(0);
        let sync_state = parts
            .next()
            .and_then(|value| value.parse::<u32>().ok())
            .map(|value| match value {
                x if x == PackageRepositorySyncState::Ready as u32 => PackageRepositorySyncState::Ready,
                x if x == PackageRepositorySyncState::Offline as u32 => PackageRepositorySyncState::Offline,
                x if x == PackageRepositorySyncState::Failed as u32 => PackageRepositorySyncState::Failed,
                _ => PackageRepositorySyncState::Idle,
            })
            .unwrap_or(PackageRepositorySyncState::Idle);
        if *repo_count < repos.len() {
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
            repo.occupied = true;
            repos[*repo_count] = repo;
            *repo_count += 1;
        }
    }
    Ok(())
}

fn repo_feed_cache_path(repo_name: &str) -> rt::Result<rt::FixedLogBuffer<INSTALL_PATH_MAX>> {
    let mut path = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
    let _ = write!(&mut path, "state/packages/repos/{}/feed.idx", repo_name);
    Ok(path)
}

fn persist_repo_feed_cache(
    storage_handle: rt::Handle,
    repo: RepositorySlot,
    bytes: &[u8],
) -> rt::Result<()> {
    let cache_path = repo_feed_cache_path(repo.name.as_str().unwrap_or("repo"))?;
    ensure_parent_directories(storage_handle, cache_path.as_str())?;
    write_storage_file(storage_handle, cache_path.as_str(), bytes)
}

fn load_repo_feed_cache(
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
    let trust_state = match repos[repo_index].trust_mode {
        PackageRepositoryTrustMode::Boot => PackageTrustState::BootTrusted,
        PackageRepositoryTrustMode::Unsigned => PackageTrustState::Unverified,
        PackageRepositoryTrustMode::PinnedDigest => {
            if repos[repo_index].last_digest == compute_fnv64(&bytes[..loaded]) {
                PackageTrustState::DigestPinned
            } else {
                PackageTrustState::VerificationFailed
            }
        }
    };
    let base_path = repository_base_path(repos[repo_index].url.as_str().unwrap_or(""));
    parse_feed_catalog(
        &bytes[..loaded],
        repos,
        repo_index,
        packages,
        package_count,
        trust_state,
        base_path.as_str(),
    )
}

fn persist_installed_state(
    storage_handle: rt::Handle,
    packages: &[PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
) -> rt::Result<()> {
    let mut text = rt::FixedLogBuffer::<MAX_STATE_BYTES>::new();
    let _ = write!(&mut text, "version=1\n");
    for slot in packages[..package_count].iter().filter(|slot| slot.occupied) {
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
            version_text_or_empty(&slot, slot.installed),
            version_text_or_empty(&slot, slot.active),
            version_text_or_empty(&slot, slot.rollback),
            slot.pin_version.as_str().unwrap_or(""),
            slot.channel as u32,
            slot.ring as u32,
            active_manifest,
            rollback_manifest,
        );
    }
    write_storage_file(storage_handle, "state/packages/installed.cfg", text.as_bytes())
}

fn load_installed_state(
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
    let manifest = load_manifest_from_storage_path(storage_handle, manifest_path)?;
    let version = manifest.version.as_str().unwrap_or("0.0.0");
    let index = if let Some(existing) = find_version_by_name(slot, version) {
        existing
    } else {
        if slot.version_count == slot.versions.len() {
            return Err(rt::Error::CapacityExceeded);
        }
        slot.version_count += 1;
        slot.version_count - 1
    };
    slot.versions[index] = PackageVersionSlot::empty();
    slot.versions[index].manifest = manifest;
    slot.versions[index].manifest_loaded = true;
    slot.versions[index].repo_index = BUILTIN_REPOSITORY_INDEX;
    let _ = slot.versions[index].repo_manifest_path.set(manifest_path);
    let _ = slot.versions[index].local_manifest_path.set(manifest_path);
    let _ = slot.versions[index].version.set(version);
    let _ = slot.versions[index]
        .compatibility
        .set(manifest.compatibility.as_str().unwrap_or("serviceos.bootstore.v1"));
    let _ = slot.versions[index].category.set(slot.package_name.as_str().unwrap_or("PACKAGE"));
    let _ = slot.versions[index].summary.set(slot.package_name.as_str().unwrap_or("PACKAGE"));
    slot.versions[index].trust_state = trust_state;
    slot.versions[index].occupied = true;
    sort_package_versions(slot);
    Ok(())
}

fn persist_journal_state(storage_handle: rt::Handle, journal: JournalState) -> rt::Result<()> {
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
    write_storage_file(storage_handle, "state/packages/journal.cfg", text.as_bytes())
}

fn load_journal_state(storage_handle: rt::Handle, journal: &mut JournalState) -> rt::Result<()> {
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
        journal.pending_action = parts.next().and_then(|v| v.parse::<u32>().ok()).unwrap_or(JOURNAL_NONE);
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

fn version_text_or_empty<'a>(slot: &'a PackageSlot, index: Option<usize>) -> &'a str {
    index.map(|i| version_text(slot, i)).unwrap_or("")
}

fn ensure_directory(storage_handle: rt::Handle, path: &str) -> rt::Result<()> {
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

fn ensure_parent_directories(storage_handle: rt::Handle, path: &str) -> rt::Result<()> {
    let mut parent = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
    let _ = split_parent_path(path, &mut parent)?;
    if parent.as_str().is_empty() {
        Ok(())
    } else {
        ensure_directory(storage_handle, parent.as_str())
    }
}

fn write_storage_file(storage_handle: rt::Handle, path: &str, bytes: &[u8]) -> rt::Result<()> {
    ensure_parent_directories(storage_handle, path)?;
    let mut parent = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
    let name = split_parent_path(path, &mut parent)?;
    let directory = rt::storage_open_directory(storage_handle, parent.as_str(), true)?;
    let (file, _) = rt::storage_directory_open_file(directory, name, true, true)?;
    let _ = rt::handle_close(directory);
    let mut offset = 0usize;
    while offset < bytes.len() {
        let chunk_len = (bytes.len() - offset).min((rt::IPC_MAX_WORDS - 3) * 8);
        let _ = rt::storage_write(file, offset, bytes.len(), &bytes[offset..offset + chunk_len])?;
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

fn parse_feed_catalog(
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
        let Some(package) = parts.next() else { continue };
        let Some(service) = parts.next().and_then(service_id_from_name) else { continue };
        let Some(version) = parts.next() else { continue };
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

fn remove_versions_for_repo(
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    repo_index: usize,
) {
    for slot in packages[..package_count].iter_mut().filter(|slot| slot.occupied) {
        let mut new_versions = [PackageVersionSlot::empty(); MAX_PACKAGE_VERSIONS];
        let mut new_count = 0usize;
        for index in 0..slot.version_count {
            let keep = slot.versions[index].occupied
                && !(slot.versions[index].repo_index == repo_index
                    && slot.versions[index].local_manifest_path.as_str().ok().unwrap_or("").is_empty());
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

fn validate_package_state(
    storage_handle: rt::Handle,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
) -> rt::Result<u32> {
    let mut repaired = 0u32;
    for slot in packages[..package_count].iter_mut().filter(|slot| slot.occupied) {
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

fn garbage_collect_packages(
    storage_handle: rt::Handle,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
) -> rt::Result<u32> {
    let mut collected = 0u32;
    for slot in packages[..package_count].iter_mut().filter(|slot| slot.occupied) {
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
                rt::StorageEntryKind::Directory => recursive_remove(storage_handle, child.as_str())?,
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

fn count_repositories(repos: &[RepositorySlot; MAX_REPOSITORIES]) -> usize {
    repos.iter().filter(|repo| repo.occupied).count()
}

fn http_fetch_text(network_handle: rt::Handle, url: &str, buffer: &mut [u8]) -> rt::Result<usize> {
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
        path,
        host,
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

fn parse_http_url(url: &str) -> rt::Result<(rt::FixedLogBuffer<REPO_NAME_MAX>, u16, rt::FixedLogBuffer<REPO_URL_MAX>)> {
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
            port.parse::<u16>().map_err(|_| rt::Error::InvalidArgument)?,
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

fn repository_base_path(url: &str) -> rt::FixedLogBuffer<REPO_URL_MAX> {
    let mut base = rt::FixedLogBuffer::<REPO_URL_MAX>::new();
    if let Some((prefix, _)) = url.rsplit_once('/') {
        let _ = base.write_str(prefix);
    } else {
        let _ = base.write_str(url);
    }
    base
}

fn join_repo_url(
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
    bytes.windows(4)
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

fn install_root_path(package: &str, version: &str) -> rt::Result<rt::FixedLogBuffer<INSTALL_PATH_MAX>> {
    let mut path = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
    let _ = write!(&mut path, "state/packages/install/{}/{}/", package, version);
    Ok(path)
}

fn create_install_root(storage_handle: rt::Handle, path: &str) -> rt::Result<()> {
    ensure_directory(storage_handle, path)
}

fn local_installed_content_path(
    install_root: &str,
    remote_path: &str,
) -> rt::Result<rt::FixedLogBuffer<INSTALL_PATH_MAX>> {
    let mut path = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
    let _ = write!(&mut path, "{}root/{}", install_root, remote_path.trim_start_matches('/'));
    Ok(path)
}

fn local_installed_manifest_path(
    install_root: &str,
) -> rt::Result<InlinePath> {
    let mut path = InlinePath::empty();
    let mut text = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
    let _ = write!(&mut text, "{}package.pkg", install_root);
    path.set(text.as_str()).map_err(|_| rt::Error::InvalidArgument)?;
    Ok(path)
}

fn local_install_root_from_manifest(path: &str) -> rt::Result<rt::FixedLogBuffer<INSTALL_PATH_MAX>> {
    let mut parent = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
    let _ = split_parent_path(path, &mut parent)?;
    Ok(parent)
}

fn join_path(
    left: &str,
    right: &str,
) -> rt::Result<rt::FixedLogBuffer<INSTALL_PATH_MAX>> {
    let mut out = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
    let _ = out.write_str(left.trim_end_matches('/'));
    let _ = out.write_str("/");
    let _ = out.write_str(right.trim_start_matches('/'));
    Ok(out)
}

fn rewrite_manifest_for_install(
    mut manifest: PackageManifest,
    install_root: &str,
) -> rt::Result<PackageManifest> {
    let mut manifest_path = rt::FixedLogBuffer::<INSTALL_PATH_MAX>::new();
    let _ = write!(
        &mut manifest_path,
        "{}root/{}",
        install_root,
        manifest.service_manifest.as_str().unwrap_or("").trim_start_matches('/')
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

fn serialize_package_manifest(manifest: PackageManifest) -> rt::Result<rt::FixedLogBuffer<MAX_PACKAGE_BYTES>> {
    let mut out = rt::FixedLogBuffer::<MAX_PACKAGE_BYTES>::new();
    let _ = write!(
        &mut out,
        "package={}\nversion={}\ncompat={}\nservice={}\nservice_manifest={}\nactivation={}\n",
        manifest.package.as_str().unwrap_or("package"),
        manifest.version.as_str().unwrap_or("0.0.0"),
        manifest.compatibility.as_str().unwrap_or("serviceos.bootstore.v1"),
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

fn service_name(service_id: ServiceId) -> &'static str {
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
        ServiceId::RootManager => "root-manager",
    }
}

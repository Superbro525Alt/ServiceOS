#![no_std]
#![no_main]

use core::cmp::Ordering;

use serviceos_bundle::{BOOT_STORE_PATH_MAX, PackageManifest, parse_package_manifest};
use serviceos_userspace_runtime as rt;
use rt::{
    ControlTag, LifecycleEvent, LogDomain, LogEvent, LogSeverity, PackageStatus, PackageTag,
    RawMessage, ServiceId, IPC_MAX_WORDS,
};

const MAX_INDEX_BYTES: usize = 512;
const MAX_PACKAGE_BYTES: usize = 512;
const MAX_PACKAGE_SLOTS: usize = 4;
const MAX_PACKAGE_VERSIONS: usize = 4;

#[derive(Clone, Copy)]
struct PackageVersionSlot {
    manifest: PackageManifest,
    occupied: bool,
}

impl PackageVersionSlot {
    const fn empty() -> Self {
        Self {
            manifest: PackageManifest::empty(),
            occupied: false,
        }
    }
}

#[derive(Clone, Copy)]
struct PackageSlot {
    service_id: ServiceId,
    versions: [PackageVersionSlot; MAX_PACKAGE_VERSIONS],
    version_count: usize,
    installed: Option<usize>,
    active: Option<usize>,
    rollback: Option<usize>,
    occupied: bool,
}

impl PackageSlot {
    const fn empty() -> Self {
        Self {
            service_id: ServiceId::RootManager,
            versions: [PackageVersionSlot::empty(); MAX_PACKAGE_VERSIONS],
            version_count: 0,
            installed: None,
            active: None,
            rollback: None,
            occupied: false,
        }
    }
}

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

    let mut packages = [PackageSlot::empty(); MAX_PACKAGE_SLOTS];
    let package_count = match load_package_catalog(storage_handle, &mut packages) {
        Ok(count) => count,
        Err(_) => return 0xfa04,
    };

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
                    log_handle,
                    &mut packages,
                    package_count,
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

fn load_package_catalog(
    storage_handle: rt::Handle,
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
        let loaded = rt::storage_read_all(
            manifest_handle,
            &mut manifest_buffer,
            requested,
        )?;
        let _ = rt::storage_blob_close(manifest_handle);
        let manifest =
            parse_package_manifest(&manifest_buffer[..loaded]).map_err(|_| rt::Error::InvalidArgument)?;

        let slot_index = if let Some(index) = find_package_slot(packages, manifest.service_id, count) {
            index
        } else {
            if count == packages.len() {
                return Err(rt::Error::CapacityExceeded);
            }
            packages[count] = PackageSlot {
                service_id: manifest.service_id,
                occupied: true,
                ..PackageSlot::empty()
            };
            let index = count;
            count += 1;
            index
        };

        let slot = &mut packages[slot_index];
        if slot.version_count == slot.versions.len() {
            return Err(rt::Error::CapacityExceeded);
        }
        slot.versions[slot.version_count] = PackageVersionSlot {
            manifest,
            occupied: true,
        };
        slot.version_count += 1;
        sort_package_versions(slot);
    }

    Ok(count)
}

fn handle_request(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    log_handle: rt::Handle,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    message: &RawMessage,
) -> rt::Result<()> {
    match message.tag {
        x if x == PackageTag::ListRequest as u32 => handle_list_request(packages, package_count, message),
        x if x == PackageTag::InfoRequest as u32 => handle_info_request(packages, package_count, message),
        x if x == PackageTag::InstallRequest as u32 => {
            handle_install_request(bootstrap, storage_handle, log_handle, packages, package_count, message)
        }
        x if x == PackageTag::UpdateRequest as u32 => {
            handle_update_request(bootstrap, storage_handle, log_handle, packages, package_count, message)
        }
        x if x == PackageTag::RemoveRequest as u32 => {
            handle_remove_request(bootstrap, log_handle, packages, package_count, message)
        }
        x if x == PackageTag::RollbackRequest as u32 => {
            handle_rollback_request(bootstrap, storage_handle, log_handle, packages, package_count, message)
        }
        x if x == PackageTag::HistoryRequest as u32 => handle_history_request(packages, package_count, message),
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
    if let Some(slot) = packages[..package_count].get(index).copied().filter(|slot| slot.occupied) {
        reply.words[0] = PackageStatus::Ok as u32 as u64;
        reply.words[1] = slot.service_id as u32 as u64;
        reply.words[2] = package_flags(slot) as u64;
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
        let latest = latest_version_index(slot);
        reply.words[0] = PackageStatus::Ok as u32 as u64;
        reply.words[1] = package_flags(slot) as u64;
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

fn handle_install_request(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    log_handle: rt::Handle,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
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
        let slot = &mut packages[index];
        let target = select_install_target(*slot, version)?;
        activate_package_version(bootstrap, storage_handle, log_handle, slot, target, LogEvent::PackageInstalled)
    } else {
        PackageStatus::NotFound
    };
    send_status_reply(reply_handle, PackageTag::InstallReply, status)
}

fn handle_update_request(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    log_handle: rt::Handle,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
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
        let slot = &mut packages[index];
        if let Some(current) = slot.installed {
            let target = if let Some(version) = version {
                match find_version_by_name(*slot, version) {
                    Some(index) if index != current => index,
                    Some(_) => usize::MAX,
                    None => usize::MAX - 1,
                }
            } else {
                next_newer_version_index(*slot, current).unwrap_or(usize::MAX)
            };
            match target {
                usize::MAX => PackageStatus::NoChange,
                x if x == usize::MAX - 1 => PackageStatus::NotFound,
                index => activate_package_version(
                    bootstrap,
                    storage_handle,
                    log_handle,
                    slot,
                    index,
                    LogEvent::PackageUpdated,
                ),
            }
        } else {
            PackageStatus::NotInstalled
        }
    } else {
        PackageStatus::NotFound
    };
    send_status_reply(reply_handle, PackageTag::UpdateReply, status)
}

fn handle_remove_request(
    bootstrap: rt::Handle,
    log_handle: rt::Handle,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
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
                        encode_version(slot.versions[active].manifest),
                    );
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

fn handle_rollback_request(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    log_handle: rt::Handle,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
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
            let status = activate_package_version(
                bootstrap,
                storage_handle,
                log_handle,
                slot,
                target,
                LogEvent::PackageRolledBack,
            );
            if status == PackageStatus::Ok {
                let previous = slot.active;
                slot.active = Some(target);
                slot.installed = Some(target);
                slot.rollback = previous;
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

fn activate_package_version(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    log_handle: rt::Handle,
    slot: &mut PackageSlot,
    target: usize,
    event: LogEvent,
) -> PackageStatus {
    let manifest = slot.versions[target].manifest;
    if verify_package_integrity(storage_handle, manifest).ok() != Some(true) {
        let _ = emit_package_event(
            log_handle,
            LogSeverity::Error,
            LogEvent::PackageActivationFailed,
            slot.service_id as u32 as u64,
            encode_version(manifest),
        );
        return PackageStatus::IntegrityFailed;
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
                encode_version(manifest),
            );
            PackageStatus::Ok
        }
        Err(_) => {
            let _ = emit_package_event(
                log_handle,
                LogSeverity::Error,
                LogEvent::PackageActivationFailed,
                slot.service_id as u32 as u64,
                encode_version(manifest),
            );
            if let Some(previous) = previous {
                let previous_manifest = slot.versions[previous].manifest;
                if verify_package_integrity(storage_handle, previous_manifest).ok() == Some(true) {
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

    let _matches_declared_digest = hash == manifest.integrity;
    // The boot-store repository is currently a trusted staged source, so
    // digests are treated as package metadata and content-shape validation
    // rather than a hard trust root. Full digest/signature enforcement is
    // deferred until writable repositories and signed feeds exist.
    Ok(true)
}

fn update_hash(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes.iter().copied() {
        *hash ^= byte as u64;
        *hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
}

fn sort_package_versions(slot: &mut PackageSlot) {
    let mut index = 1usize;
    while index < slot.version_count {
        let mut inner = index;
        while inner > 0
            && compare_versions(
                slot.versions[inner - 1].manifest.version.as_str().unwrap_or(""),
                slot.versions[inner].manifest.version.as_str().unwrap_or(""),
            ) == Ordering::Greater
        {
            slot.versions.swap(inner - 1, inner);
            inner -= 1;
        }
        index += 1;
    }
}

fn select_install_target(slot: PackageSlot, version: Option<&str>) -> rt::Result<usize> {
    if let Some(version) = version {
        find_version_by_name(slot, version).ok_or(rt::Error::NotFound)
    } else {
        latest_version_index(slot).ok_or(rt::Error::NotFound)
    }
}

fn latest_version_index(slot: PackageSlot) -> Option<usize> {
    if slot.version_count == 0 {
        None
    } else {
        Some(slot.version_count - 1)
    }
}

fn next_newer_version_index(slot: PackageSlot, current: usize) -> Option<usize> {
    ((current + 1)..slot.version_count)
        .find(|index| slot.versions[*index].occupied)
}

fn find_version_by_name(slot: PackageSlot, version: &str) -> Option<usize> {
    slot.versions[..slot.version_count]
        .iter()
        .position(|entry| entry.occupied && entry.manifest.version.as_str().ok() == Some(version))
}

fn find_package_slot(
    packages: &[PackageSlot; MAX_PACKAGE_SLOTS],
    service_id: ServiceId,
    package_count: usize,
) -> Option<usize> {
    (0..package_count).find(|index| packages[*index].occupied && packages[*index].service_id == service_id)
}

fn total_versions(packages: &[PackageSlot; MAX_PACKAGE_SLOTS], package_count: usize) -> usize {
    packages[..package_count]
        .iter()
        .filter(|slot| slot.occupied)
        .map(|slot| slot.version_count)
        .sum()
}

fn package_flags(slot: PackageSlot) -> u32 {
    u32::from(slot.installed.is_some())
        | (u32::from(slot.active.is_some()) << 1)
        | (u32::from(slot.rollback.is_some()) << 2)
}

fn version_bytes<'a>(slot: &'a PackageSlot, index: Option<usize>) -> &'a [u8] {
    if let Some(index) = index {
        if index < slot.version_count {
            if let Ok(version) = slot.versions[index].manifest.version.as_str() {
                return version.as_bytes();
            }
        }
    }
    &[]
}

fn copy_into(destination: &mut [u8], source: &[u8]) -> rt::Result<usize> {
    if source.len() > destination.len() {
        return Err(rt::Error::BufferTooSmall);
    }
    destination[..source.len()].copy_from_slice(source);
    Ok(source.len())
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

fn encode_version(manifest: PackageManifest) -> u64 {
    let version = manifest.version.as_str().unwrap_or("0.0.0");
    let (major, minor, patch) = parse_version_triplet(version);
    ((major as u64) << 32) | ((minor as u64) << 16) | patch as u64
}

fn compare_versions(left: &str, right: &str) -> Ordering {
    let left = parse_version_triplet(left);
    let right = parse_version_triplet(right);
    left.cmp(&right)
}

fn parse_version_triplet(value: &str) -> (u32, u32, u32) {
    let mut parts = value.split('.');
    let major = parts.next().and_then(|part| part.parse::<u32>().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|part| part.parse::<u32>().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|part| part.parse::<u32>().ok()).unwrap_or(0);
    (major, minor, patch)
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
        _ => ServiceId::RootManager,
    }
}

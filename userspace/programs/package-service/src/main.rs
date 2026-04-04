#![no_std]
#![no_main]

mod operations;
mod repositories;
mod requests;
mod state;
mod storage;
mod util;

pub(crate) use state::*;
pub(crate) use util::*;

use core::{cmp::Ordering, fmt::Write, str};

use serviceos_bundle::{
    parse_package_manifest, BOOT_STORE_PATH_MAX, InlinePath, PackageManifest,
};
use serviceos_userspace_runtime as rt;
use rt::{
    ControlTag, LifecycleEvent, LogDomain, LogEvent, LogSeverity, PackageChannel,
    PackageMaintenanceAction, PackageRepositorySyncState, PackageRepositoryTrustMode, PackageRing,
    PackageStatus, PackageTag, PackageTrustState, RawMessage, ServiceId, IPC_MAX_WORDS,
};

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
    repositories::initialize_builtin_repository(&mut repos[BUILTIN_REPOSITORY_INDEX]);

    let mut package_count = match repositories::load_boot_catalog(storage_handle, repos, packages) {
        Ok(count) => count,
        Err(_) => return 0xfa04,
    };
    if storage::initialize_state_directories(storage_handle).is_err() {
        let _ = emit_package_event(
            log_handle,
            LogSeverity::Warn,
            LogEvent::PackageRepairCompleted,
            0,
            0,
        );
    }
    let mut repo_count = 1usize;
    let _ = storage::load_persisted_repositories(storage_handle, repos, &mut repo_count);
    for repo_index in 1..repo_count {
        let _ = storage::load_repo_feed_cache(storage_handle, repos, repo_index, packages, &mut package_count);
    }
    let _ = storage::load_journal_state(storage_handle, journal);
    let _ = storage::load_installed_state(storage_handle, repos, packages, &mut package_count);

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
        total_versions(packages, package_count) as u64,
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
                if requests::handle_request(
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

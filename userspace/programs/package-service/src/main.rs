#![no_std]
#![no_main]

mod operations;
#[allow(dead_code)]
mod ops_model;
mod repositories;
mod requests;
mod rollout;
mod signing;
mod state;
mod storage;
#[allow(dead_code)]
mod sysupdate_model;
mod sysupdate_ops;
mod util;

pub(crate) use state::*;
pub(crate) use util::*;

use core::{cmp::Ordering, fmt::Write, str};

use rt::{
    ControlTag, IPC_MAX_WORDS, LifecycleEvent, LogDomain, LogEvent, LogSeverity, PackageChannel,
    PackageMaintenanceAction, PackageRepositorySyncState, PackageRepositoryTrustMode, PackageRing,
    PackageStatus, PackageTag, PackageTrustState, RawMessage, ServiceId,
};
use serviceos_bundle::{
    BOOT_STORE_PATH_MAX, InlinePath, PackageManifest, ServiceManifest, ServiceStartupMode,
    parse_manifest, parse_package_manifest,
};
use serviceos_userspace_runtime as rt;

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

    let mut package_count =
        match repositories::load_boot_catalog(storage_handle, log_handle, repos, packages) {
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
    let _ = storage::load_feed_keystore(storage_handle);
    let _ = storage::load_reject_journal(storage_handle);
    let _ = storage::load_rollout_policy(storage_handle);
    for repo_index in 1..repo_count {
        let _ = storage::load_repo_feed_cache(
            storage_handle,
            repos,
            repo_index,
            packages,
            &mut package_count,
        );
    }
    let _ = storage::load_journal_state(storage_handle, journal);
    let recovery = if ops_model::journal_is_stale(journal.pending_action) {
        // Stale operation journal from an interrupted run: surface it to
        // operators (Warn-level log) and keep it queryable via maintenance
        // replies until it is resumed or discarded explicitly.
        let _ = emit_package_event(
            log_handle,
            LogSeverity::Warn,
            LogEvent::PackageRepairCompleted,
            journal.pending_action as u64,
            journal.service_id as u32 as u64,
        );
        Some(*journal)
    } else {
        None
    };
    set_recovery_state(recovery);
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
        let mut did_work = false;
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xfa07,
        }

        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                did_work = true;
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

        if !did_work && rt::yield_current().is_err() {
            return 0xfa0a;
        }
    }
}

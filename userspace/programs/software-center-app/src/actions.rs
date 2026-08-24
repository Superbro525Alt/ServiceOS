use core::{fmt::Write, str};

use rt::{FixedLogBuffer, PackageChannel, PackageRing};
use serviceos_userspace_runtime as rt;

use crate::state::{
    AppState, CatalogEntry, MAX_CATEGORY_BYTES, MAX_ENTRIES, MAX_STATUS_BYTES, MAX_SUMMARY_BYTES,
    rebuild_view, select_service,
};

pub(crate) fn reload_catalog(package_handle: rt::Handle, state: &mut AppState) -> rt::Result<()> {
    state.entry_count = 0;
    state.selected_index = 0;
    state.scroll_offset = 0;
    let mut latest = [0u8; 24];
    let mut category = [0u8; MAX_CATEGORY_BYTES];
    let mut summary = [0u8; MAX_SUMMARY_BYTES];
    for index in 0..MAX_ENTRIES {
        let Some(entry) = rt::package_catalog(
            package_handle,
            index,
            &mut latest,
            &mut category,
            &mut summary,
        )?
        else {
            break;
        };
        state.entries[state.entry_count] = CatalogEntry {
            service_id: entry.service_id,
            repo_index: entry.repo_index,
            installed: entry.installed,
            active: entry.active,
            rollback: entry.rollback_available,
            latest_version: latest,
            latest_version_len: entry.latest_version_len,
            category,
            category_len: entry.category_len,
            summary,
            summary_len: entry.summary_len,
        };
        state.entry_count += 1;
    }
    rebuild_view(state);
    if state.selected_index >= state.view_count && state.view_count > 0 {
        state.selected_index = state.view_count - 1;
    }
    let entry_count = state.entry_count;
    set_statusf(
        state,
        format_args!("catalog loaded: {} entries", entry_count),
    );
    Ok(())
}

pub(crate) fn sync_repositories(package_handle: rt::Handle, state: &mut AppState) {
    match rt::package_repository_sync(package_handle, None) {
        Ok(sync) => {
            if reload_catalog(package_handle, state).is_ok() {
                set_statusf(
                    state,
                    format_args!("sync complete: {} ok, {} failed", sync.synced, sync.failed),
                );
            } else {
                set_statusf(
                    state,
                    format_args!("sync complete but catalog reload failed"),
                );
            }
        }
        Err(error) => set_statusf(state, format_args!("sync failed: {}", error_label(error))),
    }
}

#[derive(Clone, Copy)]
pub(crate) enum PackageAction {
    InstallOrUpdate,
    Remove,
}

pub(crate) fn apply_selected_package_action(
    package_handle: rt::Handle,
    state: &mut AppState,
    entry: CatalogEntry,
    action: PackageAction,
) {
    let result = match action {
        PackageAction::InstallOrUpdate => {
            if entry.installed {
                rt::package_update(package_handle, entry.service_id, None)
            } else {
                rt::package_install(package_handle, entry.service_id, None)
            }
        }
        PackageAction::Remove => rt::package_remove(package_handle, entry.service_id),
    };

    match result {
        Ok(()) => {
            if reload_catalog(package_handle, state).is_ok() {
                select_service(state, entry.service_id);
                match action {
                    PackageAction::InstallOrUpdate => {
                        if entry.installed {
                            set_statusf(
                                state,
                                format_args!(
                                    "updated {}",
                                    crate::state::service_label(entry.service_id)
                                ),
                            );
                        } else {
                            set_statusf(
                                state,
                                format_args!(
                                    "installed {}",
                                    crate::state::service_label(entry.service_id)
                                ),
                            );
                        }
                    }
                    PackageAction::Remove => {
                        set_statusf(
                            state,
                            format_args!(
                                "removed {}",
                                crate::state::service_label(entry.service_id)
                            ),
                        );
                    }
                }
            } else {
                set_statusf(
                    state,
                    format_args!("package action completed but reload failed"),
                );
            }
        }
        Err(error) => {
            let verb = match action {
                PackageAction::InstallOrUpdate => {
                    if entry.installed {
                        "update"
                    } else {
                        "install"
                    }
                }
                PackageAction::Remove => "remove",
            };
            set_statusf(
                state,
                format_args!("{} failed: {}", verb, error_label(error)),
            );
        }
    }
}

pub(crate) fn set_statusf(state: &mut AppState, args: core::fmt::Arguments<'_>) {
    let mut buffer = FixedLogBuffer::<MAX_STATUS_BYTES>::new();
    let _ = buffer.write_fmt(args);
    state.status_len = buffer.as_bytes().len().min(state.status.len());
    state.status[..state.status_len].copy_from_slice(&buffer.as_bytes()[..state.status_len]);
}

pub(crate) fn error_label(error: rt::Error) -> &'static str {
    match error {
        rt::Error::NotFound => "not found",
        rt::Error::PermissionDenied => "denied",
        rt::Error::Busy => "busy",
        rt::Error::NotInitialized => "not ready",
        rt::Error::InvalidArgument => "invalid",
        rt::Error::InvalidCall => "verification failed",
        rt::Error::Unsupported => "unsupported",
        rt::Error::BufferTooSmall => "buffer too small",
        rt::Error::CapacityExceeded => "capacity exceeded",
        rt::Error::QueueEmpty => "timeout",
        rt::Error::Unknown(_) => "unknown",
    }
}

pub(crate) fn trust_badge(value: rt::PackageTrustState) -> &'static str {
    match value {
        rt::PackageTrustState::BootTrusted => "boot-trusted",
        rt::PackageTrustState::Unverified => "unverified",
        rt::PackageTrustState::DigestPinned => "digest-pinned",
        rt::PackageTrustState::VerificationFailed => "verification-failed",
    }
}

pub(crate) fn channel_label(value: PackageChannel) -> &'static str {
    match value {
        PackageChannel::Stable => "stable",
        PackageChannel::Beta => "beta",
        PackageChannel::Canary => "canary",
    }
}

pub(crate) fn ring_label(value: PackageRing) -> &'static str {
    match value {
        PackageRing::Production => "production",
        PackageRing::Preview => "preview",
        PackageRing::Testing => "testing",
    }
}

pub(crate) fn action_label(entry: Option<CatalogEntry>) -> &'static str {
    match entry {
        Some(entry) if entry.installed => "UPDATE",
        Some(_) => "INSTALL",
        None => "INSTALL",
    }
}

pub(crate) fn text_or_dash(bytes: &[u8]) -> &str {
    if bytes.is_empty() {
        "-"
    } else {
        str::from_utf8(bytes).unwrap_or("?")
    }
}

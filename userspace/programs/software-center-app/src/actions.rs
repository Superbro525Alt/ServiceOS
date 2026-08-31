use core::{fmt::Write, str};

use rt::{FixedLogBuffer, PackageChannel, PackageRing, PackageTag, ServiceId};
use serviceos_userspace_runtime as rt;

use crate::progress;
use crate::state::{
    AppState, CatalogEntry, MAX_CATEGORY_BYTES, MAX_ENTRIES, MAX_SOURCE_BYTES, MAX_STATUS_BYTES,
    MAX_SUMMARY_BYTES, OperationState, rebuild_view, select_service, service_label,
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

/// Entry point for the detail-panel action buttons. Install/update stream
/// live phase progress while the main loop pumps (see crate::progress);
/// remove stays on the plain blocking call because package-service emits no
/// progress records for it. Both degrade to today's final-reply-only
/// behavior whenever the log channel or a subscription slot is unavailable.
pub(crate) fn apply_selected_package_action(
    package_handle: rt::Handle,
    log_handle: rt::Handle,
    state: &mut AppState,
    entry: CatalogEntry,
    action: PackageAction,
) {
    match action {
        PackageAction::InstallOrUpdate => {
            let (request_tag, op) = if entry.installed {
                (PackageTag::UpdateRequest, progress::OP_UPDATE)
            } else {
                (PackageTag::InstallRequest, progress::OP_INSTALL)
            };
            begin_streamed_mutation(package_handle, log_handle, state, entry, request_tag, op);
        }
        PackageAction::Remove => {
            // Pre-remove snapshot feeds the cleanup summary shown after removal.
            let snapshot = capture_remove_snapshot(package_handle, entry.service_id);
            let result = rt::package_remove(package_handle, entry.service_id);
            report_remove_result(package_handle, state, entry.service_id, snapshot, result);
        }
    }
}

/// Starts an install/update as a streamed operation: the log subscription
/// opens before the mutation request is sent (so early phase records are not
/// lost), then the request goes out with a reply channel and the main loop
/// pumps progress until the reply lands. Any failure along the way — no log
/// channel, subscribe handshake refused, send error — falls back to the
/// exact blocking call today's code makes, silently.
fn begin_streamed_mutation(
    package_handle: rt::Handle,
    log_handle: rt::Handle,
    state: &mut AppState,
    entry: CatalogEntry,
    request_tag: PackageTag,
    op: u32,
) {
    let service_id = entry.service_id;
    let was_installed = entry.installed;
    let verb = if was_installed { "update" } else { "install" };
    let reply_pair = match rt::channel_create() {
        Ok(pair) => pair,
        Err(error) => {
            report_install_update_result(
                package_handle,
                state,
                service_id,
                was_installed,
                Err(error),
            );
            return;
        }
    };
    let subscription = match progress::open_subscription(log_handle) {
        Some(subscription) => subscription,
        None => {
            // Degrade: final-reply-only, byte-identical to the old path.
            let _ = rt::handle_close(reply_pair.first);
            let _ = rt::handle_close(reply_pair.second);
            let result = blocking_install_or_update(package_handle, was_installed, service_id);
            report_install_update_result(package_handle, state, service_id, was_installed, result);
            return;
        }
    };
    let mut request = progress::build_mutation_request(reply_pair.second, request_tag, service_id);
    let sent = rt::channel_send_blocking(package_handle, &mut request);
    // The request consumed the send right; the receive end stays with us.
    let _ = rt::handle_close(reply_pair.second);
    if let Err(error) = sent {
        let _ = rt::handle_close(subscription);
        let _ = rt::handle_close(reply_pair.first);
        report_install_update_result(package_handle, state, service_id, was_installed, Err(error));
        return;
    }
    let now = rt::monotonic_now().unwrap_or(0);
    state.operation = Some(OperationState {
        op,
        reply_tag: progress::reply_tag_for(request_tag),
        service_id,
        subscription,
        reply_pair: reply_pair.first,
        last_activity_tick: now,
        records_seen: 0,
        rendered: (0, 0, 0),
        degraded: false,
        note_shown: false,
    });
    set_statusf(
        state,
        format_args!("{} {}...", verb, service_label(service_id)),
    );
}

/// The blocking mutation the plain path always made (also the degrade path).
fn blocking_install_or_update(
    package_handle: rt::Handle,
    was_installed: bool,
    service_id: ServiceId,
) -> rt::Result<()> {
    if was_installed {
        rt::package_update(package_handle, service_id, None)
    } else {
        rt::package_install(package_handle, service_id, None)
    }
}

/// Main-loop pump for an active streamed operation: drains progress records,
/// repaints the bounded status line on change, and renders the final status
/// when the reply lands. Returns true when the frame changed.
pub(crate) fn pump_active_operation(package_handle: rt::Handle, state: &mut AppState) -> bool {
    let (service_id, was_installed) = match state.operation.as_ref() {
        Some(operation) => (operation.service_id, operation.op == progress::OP_UPDATE),
        None => return false,
    };
    progress::pump_operation(state, |state, result| {
        report_install_update_result(package_handle, state, service_id, was_installed, result);
    })
}

/// Renders an install/update result exactly as the single-shot code always
/// did: catalog reload, selection, session bookkeeping, and the final status
/// line (success summary or "<verb> failed: <label>").
fn report_install_update_result(
    package_handle: rt::Handle,
    state: &mut AppState,
    service_id: ServiceId,
    was_installed: bool,
    result: rt::Result<()>,
) {
    match result {
        Ok(()) => {
            if reload_catalog(package_handle, state).is_ok() {
                select_service(state, service_id);
                if was_installed {
                    let tick = rt::monotonic_now().unwrap_or(0);
                    state.record_session_update(service_id, tick);
                    set_statusf(
                        state,
                        format_args!("updated {} (at tick {})", service_label(service_id), tick),
                    );
                } else {
                    set_statusf(
                        state,
                        format_args!("installed {}", service_label(service_id)),
                    );
                }
            } else {
                set_statusf(
                    state,
                    format_args!("package action completed but reload failed"),
                );
            }
        }
        Err(error) => {
            let verb = if was_installed { "update" } else { "install" };
            set_statusf(
                state,
                format_args!("{} failed: {}", verb, error_label(error)),
            );
        }
    }
}

/// Renders a remove result exactly as the single-shot code always did.
fn report_remove_result(
    package_handle: rt::Handle,
    state: &mut AppState,
    service_id: ServiceId,
    snapshot: Option<RemoveSnapshot>,
    result: rt::Result<()>,
) {
    match result {
        Ok(()) => {
            if reload_catalog(package_handle, state).is_ok() {
                select_service(state, service_id);
                set_statusf(
                    state,
                    format_args!(
                        "removed {}: {}",
                        service_label(service_id),
                        remove_cleanup_summary(snapshot)
                    ),
                );
            } else {
                set_statusf(
                    state,
                    format_args!("package action completed but reload failed"),
                );
            }
        }
        Err(error) => {
            set_statusf(state, format_args!("remove failed: {}", error_label(error)));
        }
    }
}

/// What the pre-remove view knew: version and running state.
struct RemoveSnapshot {
    version: [u8; 24],
    version_len: usize,
    was_active: bool,
}

fn capture_remove_snapshot(
    package_handle: rt::Handle,
    service_id: ServiceId,
) -> Option<RemoveSnapshot> {
    let mut installed = [0u8; 24];
    let mut active = [0u8; 24];
    let mut rollback = [0u8; 24];
    let mut latest = [0u8; 24];
    let mut source = [0u8; MAX_SOURCE_BYTES];
    let provenance = rt::package_provenance(
        package_handle,
        service_id,
        &mut installed,
        &mut active,
        &mut rollback,
        &mut latest,
        &mut source,
    )
    .ok()?;
    Some(RemoveSnapshot {
        version: installed,
        version_len: provenance.installed_version_len.min(24),
        was_active: provenance.active,
    })
}

/// Human summary of what a successful uninstall cleaned up. The manager
/// deactivation and journal clear happen service-side; storage reclamation
/// is an explicit `pkg gc` step, so the summary says exactly that.
fn remove_cleanup_summary(snapshot: Option<RemoveSnapshot>) -> heapless_string::String {
    let mut text = heapless_string::String::new();
    match snapshot {
        Some(snapshot) => {
            let version = str::from_utf8(&snapshot.version[..snapshot.version_len]).unwrap_or("-");
            let _ = core::fmt::Write::write_fmt(
                &mut text,
                format_args!(
                    "v{} deactivated={} journal-cleared rollback-kept gc=reclaims",
                    if snapshot.version_len > 0 {
                        version
                    } else {
                        "-"
                    },
                    if snapshot.was_active {
                        "yes"
                    } else {
                        "not-running"
                    },
                ),
            );
        }
        None => {
            text.push_str("journal-cleared gc=reclaims");
        }
    }
    text
}

mod heapless_string {
    use core::fmt;

    use crate::state::MAX_STATUS_BYTES;

    pub(crate) struct String {
        bytes: [u8; MAX_STATUS_BYTES],
        len: usize,
    }

    impl String {
        pub(crate) const fn new() -> Self {
            Self {
                bytes: [0; MAX_STATUS_BYTES],
                len: 0,
            }
        }

        pub(crate) fn push_str(&mut self, piece: &str) {
            let bytes = piece.as_bytes();
            let remaining = self.bytes.len() - self.len;
            let take = bytes.len().min(remaining);
            self.bytes[self.len..self.len + take].copy_from_slice(&bytes[..take]);
            self.len += take;
        }

        pub(crate) fn as_str(&self) -> &str {
            str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
        }
    }

    impl fmt::Display for String {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.as_str())
        }
    }

    impl fmt::Write for String {
        fn write_str(&mut self, piece: &str) -> fmt::Result {
            self.push_str(piece);
            Ok(())
        }
    }
}

/// Operator guidance for launching an installed package: launches ride the
/// manager via the shell (`run pkg <name>`); this app has no manager channel.
pub(crate) fn launch_guidance(state: &mut AppState, entry: CatalogEntry) {
    if !entry.installed {
        set_statusf(
            state,
            format_args!(
                "{} is not installed yet",
                crate::state::service_label(entry.service_id)
            ),
        );
        return;
    }
    set_statusf(
        state,
        format_args!(
            "launch {} via shell: run pkg {}",
            crate::state::service_label(entry.service_id),
            crate::state::service_label(entry.service_id),
        ),
    );
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
        rt::Error::BrokenPipe => "broken-pipe",
        rt::Error::Unknown(_) => "unknown",
    }
}

pub(crate) fn trust_badge(value: rt::PackageTrustState) -> &'static str {
    match value {
        rt::PackageTrustState::BootTrusted => "boot-trusted",
        rt::PackageTrustState::Unverified => "unverified",
        rt::PackageTrustState::DigestPinned => "digest-pinned",
        rt::PackageTrustState::SignedKeyTrusted => "signed-key-trusted",
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

//! Backup page machinery. The backup service is a manual-activation image
//! (`services/backup-service/program.img`) with no registration under its
//! named `ServiceId`; it publishes its public channel over the launcher
//! handshake. Root-manager performs that handshake when it launches this
//! app and delivers the channel as startup handles[7] (present only when
//! the Runtime grant landed at handles[6]; see root-manager
//! `control/launch.rs`). This module owns both halves of that route: the
//! transport glue that drives the request/reply machine over the granted
//! channel, and the honest explainer that replaces the page when the grant
//! is absent or the service unreachable. Wire shapes mirror backup-service
//! `protocol.rs` exactly (status-first replies, 0 = Ok).

use serviceos_userspace_runtime as rt;

pub(crate) const BACKUP_TAG_EXPORT_REQUEST: u32 = 0x230;
pub(crate) const BACKUP_TAG_EXPORT_REPLY: u32 = 0x231;
pub(crate) const BACKUP_TAG_RESTORE_REQUEST: u32 = 0x232;
pub(crate) const BACKUP_TAG_RESTORE_REPLY: u32 = 0x233;
pub(crate) const BACKUP_TAG_LIST_REQUEST: u32 = 0x234;
pub(crate) const BACKUP_TAG_LIST_REPLY: u32 = 0x235;
pub(crate) const BACKUP_TAG_DELETE_REQUEST: u32 = 0x236;
pub(crate) const BACKUP_TAG_DELETE_REPLY: u32 = 0x237;

pub(crate) const BACKUP_ERROR_INVALID: u64 = 1;
pub(crate) const BACKUP_ERROR_UNKNOWN_SCOPE: u64 = 2;
pub(crate) const BACKUP_ERROR_CAPACITY: u64 = 3;
pub(crate) const BACKUP_ERROR_NOT_FOUND: u64 = 4;
pub(crate) const BACKUP_ERROR_BAD_MAGIC: u64 = 5;
pub(crate) const BACKUP_ERROR_BAD_VERSION: u64 = 6;
pub(crate) const BACKUP_ERROR_CORRUPT: u64 = 7;
pub(crate) const BACKUP_ERROR_STORAGE: u64 = 8;
pub(crate) const BACKUP_ERROR_UNSIGNED: u64 = 9;
pub(crate) const BACKUP_ERROR_BAD_SIG: u64 = 10;

pub(crate) const BACKUP_SCOPE_CONFIG: u32 = 1 << 0;
pub(crate) const BACKUP_SCOPE_ACCOUNTS: u32 = 1 << 1;
pub(crate) const BACKUP_SCOPE_PACKAGES: u32 = 1 << 2;
pub(crate) const BACKUP_SCOPE_KNOWN_MASK: u32 =
    BACKUP_SCOPE_CONFIG | BACKUP_SCOPE_ACCOUNTS | BACKUP_SCOPE_PACKAGES;

/// Mirrors backup-service `MAX_BACKUP_NAME`.
pub(crate) const BACKUP_NAME_MAX_BYTES: usize = 32;
/// List paths are `backups/<name>`; bounded like the service's own decode.
pub(crate) const BACKUP_PATH_MAX_BYTES: usize = 48;
/// Capacity-bounded list window for the page (service pages via index echo).
pub(crate) const BACKUP_LIST_ROWS: usize = 8;

pub(crate) const BACKUP_LIST_END_STATUS: u64 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BackupSnapshotEntry {
    pub(crate) index: usize,
    pub(crate) path: [u8; BACKUP_PATH_MAX_BYTES],
    pub(crate) path_len: usize,
    /// Plain snapshot name (the `backups/` prefix stripped) — the delete and
    /// restore contracts take this form.
    pub(crate) name: [u8; BACKUP_NAME_MAX_BYTES],
    pub(crate) name_len: usize,
    /// Additive list tail: a signature sidecar exists for this snapshot and
    /// the key id it names (0 when the sidecar is unreadable).
    pub(crate) signed: bool,
    pub(crate) key_id: u64,
}

impl BackupSnapshotEntry {
    pub(crate) fn path_str(&self) -> Option<&str> {
        core::str::from_utf8(&self.path[..self.path_len]).ok()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BackupExportInfo {
    pub(crate) name: [u8; BACKUP_NAME_MAX_BYTES],
    pub(crate) name_len: usize,
    pub(crate) record_count: u32,
    pub(crate) blob_size: usize,
    /// Additive tail: the stored snapshot carries a detached signature.
    pub(crate) signed: bool,
    pub(crate) key_id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BackupRestoreReport {
    pub(crate) dry_run: bool,
    pub(crate) selected_scope_mask: u32,
    pub(crate) selected_records: u32,
    pub(crate) total_bytes: u64,
    /// Additive tail: the signature gate passed for this snapshot.
    pub(crate) verified: bool,
    pub(crate) key_id: u64,
}

/// Honest classification of a failed backup operation: service error codes
/// plus the two locally-detected failure shapes (transport, malformed reply).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackupOpError {
    Invalid,
    UnknownScope,
    CapacityExceeded,
    NotFound,
    BadMagic,
    UnsupportedVersion,
    Corrupt,
    StorageFailure,
    /// Snapshot has no signature sidecar; restore refuses by policy.
    Unsigned,
    /// Signature malformed, foreign, or failing over the blob bytes.
    BadSignature,
    Transport,
    Malformed,
}

pub(crate) fn backup_error_from_code(code: u64) -> Option<BackupOpError> {
    match code {
        BACKUP_ERROR_INVALID => Some(BackupOpError::Invalid),
        BACKUP_ERROR_UNKNOWN_SCOPE => Some(BackupOpError::UnknownScope),
        BACKUP_ERROR_CAPACITY => Some(BackupOpError::CapacityExceeded),
        BACKUP_ERROR_NOT_FOUND => Some(BackupOpError::NotFound),
        BACKUP_ERROR_BAD_MAGIC => Some(BackupOpError::BadMagic),
        BACKUP_ERROR_BAD_VERSION => Some(BackupOpError::UnsupportedVersion),
        BACKUP_ERROR_CORRUPT => Some(BackupOpError::Corrupt),
        BACKUP_ERROR_STORAGE => Some(BackupOpError::StorageFailure),
        BACKUP_ERROR_UNSIGNED => Some(BackupOpError::Unsigned),
        BACKUP_ERROR_BAD_SIG => Some(BackupOpError::BadSignature),
        _ => None,
    }
}

pub(crate) fn backup_error_name(error: BackupOpError) -> &'static str {
    match error {
        BackupOpError::Invalid => "INVALID",
        BackupOpError::UnknownScope => "SCOPE",
        BackupOpError::CapacityExceeded => "CAPACITY",
        BackupOpError::NotFound => "NOT-FOUND",
        BackupOpError::BadMagic => "MAGIC",
        BackupOpError::UnsupportedVersion => "VERSION",
        BackupOpError::Corrupt => "CORRUPT",
        BackupOpError::StorageFailure => "STORAGE",
        BackupOpError::Unsigned => "UNSIGNED",
        BackupOpError::BadSignature => "BAD-SIG",
        BackupOpError::Transport => "TRANSPORT",
        BackupOpError::Malformed => "MALFORMED",
    }
}

/// Why the page cannot reach the service. Rendered verbatim.
/// Read the additive signing tail (signed, key_id) that follows the packed
/// payload: `base` fixed words + ceil(payload_len/8) packed words + 2 tail
/// words. Older services omit the tail; decode degrades to (false, 0).
fn signing_tail(reply: &rt::RawMessage, base: usize, payload_len: usize) -> (bool, u64) {
    let tail = base + payload_len.div_ceil(8);
    if reply.word_count as usize >= tail + 2 {
        (reply.words[tail] != 0, reply.words[tail + 1])
    } else {
        (false, 0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackupUnavailable {
    /// Documented default: manual-activation service, no registered public
    /// channel, no granted handle in this app's startup set.
    NoRoute,
    /// A route existed before but the round-trip failed.
    TransportFailure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackupPhase {
    Idle,
    Listing,
    Ready,
    Exporting,
    Restoring,
    Deleting,
}

/// Modal sub-states. Restore is two-step by design: a dry-run report is
/// shown before the destructive apply; delete shows one confirm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackupPrompt {
    RestoreConfirm(BackupRestoreReport),
    DeleteConfirm,
}

pub(crate) struct BackupUiState {
    pub(crate) unavailable: Option<BackupUnavailable>,
    pub(crate) phase: BackupPhase,
    pub(crate) entries: [BackupSnapshotEntry; BACKUP_LIST_ROWS],
    pub(crate) entry_count: usize,
    pub(crate) entry_total: usize,
    pub(crate) list_error: Option<BackupOpError>,
    pub(crate) selected: usize,
    pub(crate) prompt: Option<BackupPrompt>,
    pub(crate) export_outcome: Option<Result<BackupExportInfo, BackupOpError>>,
    pub(crate) restore_outcome: Option<Result<BackupRestoreReport, BackupOpError>>,
    pub(crate) delete_outcome: Option<Result<(), BackupOpError>>,
    pub(crate) latest_export: Option<BackupExportInfo>,
    /// Pending dry-run filter captured while the confirm prompt is up.
    pub(crate) pending_restore_name: [u8; BACKUP_NAME_MAX_BYTES],
    pub(crate) pending_restore_len: usize,
}

impl BackupUiState {
    pub(crate) fn new() -> Self {
        Self {
            unavailable: Some(BackupUnavailable::NoRoute),
            phase: BackupPhase::Idle,
            entries: [BackupSnapshotEntry {
                index: 0,
                path: [0; BACKUP_PATH_MAX_BYTES],
                path_len: 0,
                name: [0; BACKUP_NAME_MAX_BYTES],
                name_len: 0,
                signed: false,
                key_id: 0,
            }; BACKUP_LIST_ROWS],
            entry_count: 0,
            entry_total: 0,
            list_error: None,
            selected: 0,
            prompt: None,
            export_outcome: None,
            restore_outcome: None,
            delete_outcome: None,
            latest_export: None,
            pending_restore_name: [0; BACKUP_NAME_MAX_BYTES],
            pending_restore_len: 0,
        }
    }

    pub(crate) fn stop_editing(&mut self) {
        self.prompt = None;
        self.pending_restore_len = 0;
        self.pending_restore_name = [0; BACKUP_NAME_MAX_BYTES];
    }

    // ---- request builders (pure; the transport layer sends them) ----

    /// LIST request: [index]; the service replies once per index with
    /// [status, index_echo, kind, name_len, packed path] (or status-only on
    /// failure), so the page pages through `backups/` one row per round trip
    /// until the End status.
    pub(crate) fn encode_list_request(&self, index: usize) -> rt::RawMessage {
        let mut request = rt::RawMessage::empty(BACKUP_TAG_LIST_REQUEST);
        request.word_count = 1;
        request.words[0] = index as u64;
        request
    }

    pub(crate) fn encode_export_request(
        &self,
        scope_mask: u32,
    ) -> Result<rt::RawMessage, BackupOpError> {
        if scope_mask & !BACKUP_SCOPE_KNOWN_MASK != 0 {
            return Err(BackupOpError::UnknownScope);
        }
        let mut request = rt::RawMessage::empty(BACKUP_TAG_EXPORT_REQUEST);
        request.word_count = 1;
        request.words[0] = scope_mask as u64;
        Ok(request)
    }

    fn encode_restore_request_inner(
        &self,
        filter_mask: u32,
        dry_run: bool,
        name: &[u8],
    ) -> Result<rt::RawMessage, BackupOpError> {
        if filter_mask & !BACKUP_SCOPE_KNOWN_MASK != 0 {
            return Err(BackupOpError::UnknownScope);
        }
        if name.len() == 0 || name.len() > BACKUP_NAME_MAX_BYTES {
            return Err(BackupOpError::Invalid);
        }
        let mut request = rt::RawMessage::empty(BACKUP_TAG_RESTORE_REQUEST);
        request.word_count = 3;
        request.words[0] = filter_mask as u64;
        request.words[1] = u64::from(dry_run);
        request.words[2] = name.len() as u64;
        let packed =
            rt::pack_bytes(name, &mut request.words[3..]).map_err(|_| BackupOpError::Invalid)?;
        request.word_count += packed;
        Ok(request)
    }

    pub(crate) fn begin_restore_dry_run(
        &mut self,
        filter_mask: u32,
    ) -> Result<rt::RawMessage, BackupOpError> {
        let (name, name_len) = self.selected_name()?;
        let request = self.encode_restore_request_inner(filter_mask, true, name)?;
        let mut stored = [0u8; BACKUP_NAME_MAX_BYTES];
        stored[..name_len].copy_from_slice(name);
        self.pending_restore_name = stored;
        self.pending_restore_len = name_len;
        self.restore_outcome = None;
        self.phase = BackupPhase::Restoring;
        Ok(request)
    }

    pub(crate) fn confirm_restore_apply(
        &mut self,
        filter_mask: u32,
    ) -> Result<rt::RawMessage, BackupOpError> {
        // Gated: only valid while a restore dry-run confirm prompt is up.
        let Some(BackupPrompt::RestoreConfirm(_)) = self.prompt else {
            return Err(BackupOpError::Invalid);
        };
        let name_len = self.pending_restore_len;
        let request = self.encode_restore_request_inner(
            filter_mask,
            false,
            &self.pending_restore_name[..name_len],
        )?;
        self.prompt = None;
        self.restore_outcome = None;
        self.phase = BackupPhase::Restoring;
        Ok(request)
    }

    pub(crate) fn encode_delete_request(
        &self,
        name: &[u8],
    ) -> Result<rt::RawMessage, BackupOpError> {
        if name.len() == 0 || name.len() > BACKUP_NAME_MAX_BYTES {
            return Err(BackupOpError::Invalid);
        }
        let mut request = rt::RawMessage::empty(BACKUP_TAG_DELETE_REQUEST);
        request.word_count = 1;
        request.words[0] = name.len() as u64;
        let packed =
            rt::pack_bytes(name, &mut request.words[1..]).map_err(|_| BackupOpError::Invalid)?;
        request.word_count += packed;
        Ok(request)
    }

    pub(crate) fn confirm_delete(&mut self) -> Result<rt::RawMessage, BackupOpError> {
        // Gated: only valid while a delete confirm prompt is up.
        if self.prompt != Some(BackupPrompt::DeleteConfirm) {
            return Err(BackupOpError::Invalid);
        }
        let name_len = self.selected_name_len()?;
        let request = self.encode_delete_request(&self.entries[self.selected].name[..name_len])?;
        self.prompt = None;
        self.delete_outcome = None;
        self.phase = BackupPhase::Deleting;
        Ok(request)
    }

    // ---- reply decoders (pure; mirror protocol.rs reply shapes) ----

    /// LIST reply: [status, index_echo, kind, name_len, packed path]; status 2
    /// ends the listing; errors arrive status-only (word_count 1). Returns
    /// Ok(None) on End.
    pub(crate) fn decode_list_reply(
        &mut self,
        reply: &rt::RawMessage,
    ) -> Result<Option<BackupSnapshotEntry>, BackupOpError> {
        if reply.tag != BACKUP_TAG_LIST_REPLY || reply.word_count < 1 {
            self.note_transport();
            return Err(BackupOpError::Malformed);
        }
        let status = reply.words[0];
        if status != 0 && status != BACKUP_LIST_END_STATUS {
            // Service-side list failures are status-only replies.
            let error = backup_error_from_code(status).unwrap_or(BackupOpError::Malformed);
            self.list_error = Some(error);
            self.phase = BackupPhase::Idle;
            return Err(error);
        }
        if status == BACKUP_LIST_END_STATUS {
            self.phase = BackupPhase::Ready;
            return Ok(None);
        }
        if reply.word_count < 4 {
            self.note_transport();
            return Err(BackupOpError::Malformed);
        }
        let index = reply.words[1] as usize;
        let path_len = reply.words[3] as usize;
        if path_len == 0 || path_len > BACKUP_PATH_MAX_BYTES {
            self.note_transport();
            return Err(BackupOpError::Malformed);
        }
        let mut path = [0u8; BACKUP_PATH_MAX_BYTES];
        if rt::unpack_bytes(
            &reply.words[4..reply.word_count as usize],
            path_len,
            &mut path,
        )
        .is_err()
        {
            self.note_transport();
            return Err(BackupOpError::Malformed);
        }
        let mut name = [0u8; BACKUP_NAME_MAX_BYTES];
        let name_start = match path[..path_len].iter().position(|&byte| byte == b'/') {
            Some(position) => position + 1,
            None => 0,
        };
        let name_bytes = &path[name_start..path_len];
        if name_bytes.is_empty() || name_bytes.len() > BACKUP_NAME_MAX_BYTES {
            self.note_transport();
            return Err(BackupOpError::Malformed);
        }
        name[..name_bytes.len()].copy_from_slice(name_bytes);
        let (signed, key_id) = signing_tail(reply, 4, path_len);
        let entry = BackupSnapshotEntry {
            index,
            path,
            path_len,
            name,
            name_len: name_bytes.len(),
            signed,
            key_id,
        };
        if self.entry_count < BACKUP_LIST_ROWS {
            self.entries[self.entry_count] = entry;
            self.entry_count += 1;
        }
        self.entry_total += 1;
        Ok(Some(entry))
    }

    pub(crate) fn begin_listing(&mut self) {
        self.phase = BackupPhase::Listing;
        self.entry_count = 0;
        self.entry_total = 0;
        self.list_error = None;
        self.selected = 0;
        self.export_outcome = None;
        self.restore_outcome = None;
        self.delete_outcome = None;
        self.stop_editing();
    }

    /// EXPORT reply: [status, name_len, record_count, blob_size, packed name].
    pub(crate) fn on_export_reply(
        &mut self,
        reply: &rt::RawMessage,
    ) -> Result<BackupExportInfo, BackupOpError> {
        self.phase = BackupPhase::Ready;
        let decoded = decode_export_reply(reply);
        match decoded {
            Ok(info) => {
                self.export_outcome = Some(Ok(info));
                self.latest_export = Some(info);
                Ok(info)
            }
            Err(error) => {
                self.export_outcome = Some(Err(error));
                Err(error)
            }
        }
    }

    pub(crate) fn begin_export(
        &mut self,
        scope_mask: u32,
    ) -> Result<rt::RawMessage, BackupOpError> {
        let request = self.encode_export_request(scope_mask)?;
        self.export_outcome = None;
        self.phase = BackupPhase::Exporting;
        Ok(request)
    }

    /// RESTORE reply: [status, dry_run, selected_scope_mask, selected_records,
    /// total_bytes]. A dry-run reply opens the confirm prompt; an apply reply
    /// closes it with the final outcome.
    pub(crate) fn on_restore_reply(
        &mut self,
        reply: &rt::RawMessage,
    ) -> Result<BackupRestoreReport, BackupOpError> {
        if reply.tag != BACKUP_TAG_RESTORE_REPLY || reply.word_count < 5 {
            self.phase = BackupPhase::Ready;
            self.note_transport();
            let error = BackupOpError::Malformed;
            self.restore_outcome = Some(Err(error));
            return Err(error);
        }
        let status = reply.words[0];
        if status != 0 {
            let error = backup_error_from_code(status).unwrap_or(BackupOpError::Malformed);
            self.phase = BackupPhase::Ready;
            self.prompt = None;
            self.restore_outcome = Some(Err(error));
            return Err(error);
        }
        let (verified, key_id) = signing_tail(reply, 5, 0);
        let report = BackupRestoreReport {
            dry_run: reply.words[1] != 0,
            selected_scope_mask: reply.words[2] as u32,
            selected_records: reply.words[3] as u32,
            total_bytes: reply.words[4],
            verified,
            key_id,
        };
        if report.dry_run {
            self.prompt = Some(BackupPrompt::RestoreConfirm(report));
            self.phase = BackupPhase::Ready;
        } else {
            self.prompt = None;
            self.phase = BackupPhase::Ready;
        }
        self.restore_outcome = Some(Ok(report));
        Ok(report)
    }

    /// DELETE reply: status-only.
    pub(crate) fn on_delete_reply(&mut self, reply: &rt::RawMessage) -> Result<(), BackupOpError> {
        self.phase = BackupPhase::Ready;
        if reply.tag != BACKUP_TAG_DELETE_REPLY || reply.word_count < 1 {
            self.note_transport();
            let error = BackupOpError::Malformed;
            self.delete_outcome = Some(Err(error));
            return Err(error);
        }
        let status = reply.words[0];
        if status == 0 {
            self.remove_selected();
            self.delete_outcome = Some(Ok(()));
            Ok(())
        } else {
            let error = backup_error_from_code(status).unwrap_or(BackupOpError::Malformed);
            self.delete_outcome = Some(Err(error));
            Err(error)
        }
    }

    pub(crate) fn begin_delete(&mut self) -> Result<(), BackupOpError> {
        // Selection must resolve to a real row before the confirm prompt.
        let _ = self.selected_name_len()?;
        self.prompt = Some(BackupPrompt::DeleteConfirm);
        Ok(())
    }

    pub(crate) fn cancel_prompt(&mut self) {
        self.prompt = None;
        self.pending_restore_len = 0;
        self.pending_restore_name = [0; BACKUP_NAME_MAX_BYTES];
    }

    pub(crate) fn select(&mut self, row: usize) {
        if row < self.entry_count {
            self.selected = row;
        }
    }

    fn selected_name(&self) -> Result<(&[u8], usize), BackupOpError> {
        let row = self.selected_name_len()?;
        let name = &self.entries[self.selected].name;
        Ok((&name[..row], row))
    }

    fn selected_name_len(&self) -> Result<usize, BackupOpError> {
        if self.selected >= self.entry_count {
            return Err(BackupOpError::NotFound);
        }
        Ok(self.entries[self.selected].name_len)
    }

    fn remove_selected(&mut self) {
        if self.selected >= self.entry_count {
            return;
        }
        for index in self.selected..self.entry_count - 1 {
            self.entries[index] = self.entries[index + 1];
        }
        self.entry_count -= 1;
        self.entry_total = self.entry_total.saturating_sub(1);
        if self.selected >= self.entry_count && self.entry_count > 0 {
            self.selected = self.entry_count - 1;
        }
    }

    fn note_transport(&mut self) {
        self.unavailable = Some(BackupUnavailable::TransportFailure);
    }
}

/// EXPORT reply decode without state (mirrors protocol.rs word layout).
pub(crate) fn decode_export_reply(
    reply: &rt::RawMessage,
) -> Result<BackupExportInfo, BackupOpError> {
    if reply.tag != BACKUP_TAG_EXPORT_REPLY || reply.word_count < 4 {
        return Err(BackupOpError::Malformed);
    }
    let status = reply.words[0];
    if status != 0 {
        return Err(backup_error_from_code(status).unwrap_or(BackupOpError::Malformed));
    }
    let name_len = reply.words[1] as usize;
    if name_len == 0 || name_len > BACKUP_NAME_MAX_BYTES {
        return Err(BackupOpError::Malformed);
    }
    let mut name = [0u8; BACKUP_NAME_MAX_BYTES];
    if rt::unpack_bytes(
        &reply.words[4..reply.word_count as usize],
        name_len,
        &mut name,
    )
    .is_err()
    {
        return Err(BackupOpError::Malformed);
    }
    let (signed, key_id) = signing_tail(reply, 4, name_len);
    Ok(BackupExportInfo {
        name,
        name_len,
        record_count: reply.words[2] as u32,
        blob_size: reply.words[3] as usize,
        signed,
        key_id,
    })
}

// ---- transport glue (the only code that touches the granted channel) ----

/// Positional startup contract with root-manager's SettingsApp launch grants:
/// handles[6] = Runtime, handles[7] = backup-service public channel, and the
/// backup grant is appended only when the Runtime grant succeeded, so the
/// handle count alone disambiguates the tail.
pub(crate) fn backup_route_position(handle_count: usize) -> Option<usize> {
    if handle_count >= 8 { Some(7) } else { None }
}

/// The page shows live controls only while a granted route is trusted.
pub(crate) fn page_live(backup: &BackupUiState) -> bool {
    backup.unavailable.is_none()
}

/// Hard bound on LIST round trips so a misbehaving service cannot stall the
/// page loop; the window stays capacity-bounded and the surplus is reported
/// honestly as `+N MORE`.
const LIST_CALL_CAP: usize = 32;

/// Shared mid-operation transport-failure settle: the route is no longer
/// trusted (explainer on next render) and the in-flight phase resolves so
/// the footer can report the failure. Never panics; returns the error the
/// caller stores in its outcome slot.
fn note_transport_outcome(backup: &mut BackupUiState) -> BackupOpError {
    backup.note_transport();
    backup.phase = BackupPhase::Ready;
    BackupOpError::Transport
}

/// Page entry: connect or explain. A granted channel clears the explainer
/// and refreshes the snapshot list; an absent grant keeps (or restores) the
/// manual-activation explainer.
pub(crate) fn on_page_enter(handle: rt::Handle, backup: &mut BackupUiState) {
    backup.stop_editing();
    if handle == rt::INVALID_HANDLE {
        if backup.unavailable.is_none() {
            backup.unavailable = Some(BackupUnavailable::NoRoute);
        }
        return;
    }
    backup.unavailable = None;
    refresh_listing(handle, backup);
}

/// Page the service's `backups/` directory into the bounded list window.
pub(crate) fn refresh_listing(handle: rt::Handle, backup: &mut BackupUiState) {
    backup.begin_listing();
    for index in 0..LIST_CALL_CAP {
        let mut request = backup.encode_list_request(index);
        match rt::channel_call(handle, &mut request) {
            Ok(reply) => match backup.decode_list_reply(&reply) {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => break,
            },
            Err(_) => {
                let _ = note_transport_outcome(backup);
                break;
            }
        }
    }
    if backup.phase == BackupPhase::Listing {
        // Call cap reached without an End status: show what arrived.
        backup.phase = BackupPhase::Ready;
    }
}

/// EXPORT over the granted channel.
pub(crate) fn perform_export(handle: rt::Handle, backup: &mut BackupUiState, scope_mask: u32) {
    let mut request = match backup.begin_export(scope_mask) {
        Ok(request) => request,
        Err(error) => {
            backup.phase = BackupPhase::Ready;
            backup.export_outcome = Some(Err(error));
            return;
        }
    };
    match rt::channel_call(handle, &mut request) {
        Ok(reply) => {
            let _ = backup.on_export_reply(&reply);
        }
        Err(_) => {
            backup.export_outcome = Some(Err(note_transport_outcome(backup)));
        }
    }
}

/// RESTORE dry-run over the granted channel; a clean report opens the
/// confirm prompt (the machine owns that transition).
pub(crate) fn perform_restore_dry_run(
    handle: rt::Handle,
    backup: &mut BackupUiState,
    scope_mask: u32,
) {
    let mut request = match backup.begin_restore_dry_run(scope_mask) {
        Ok(request) => request,
        Err(error) => {
            backup.phase = BackupPhase::Ready;
            backup.restore_outcome = Some(Err(error));
            return;
        }
    };
    match rt::channel_call(handle, &mut request) {
        Ok(reply) => {
            let _ = backup.on_restore_reply(&reply);
        }
        Err(_) => {
            backup.restore_outcome = Some(Err(note_transport_outcome(backup)));
        }
    }
}

/// Confirmed RESTORE apply over the granted channel.
pub(crate) fn perform_restore_apply(
    handle: rt::Handle,
    backup: &mut BackupUiState,
    scope_mask: u32,
) {
    let mut request = match backup.confirm_restore_apply(scope_mask) {
        Ok(request) => request,
        Err(error) => {
            backup.phase = BackupPhase::Ready;
            backup.restore_outcome = Some(Err(error));
            return;
        }
    };
    match rt::channel_call(handle, &mut request) {
        Ok(reply) => {
            let _ = backup.on_restore_reply(&reply);
        }
        Err(_) => {
            backup.restore_outcome = Some(Err(note_transport_outcome(backup)));
        }
    }
}

/// Confirmed DELETE over the granted channel.
pub(crate) fn perform_delete(handle: rt::Handle, backup: &mut BackupUiState) {
    let mut request = match backup.confirm_delete() {
        Ok(request) => request,
        Err(error) => {
            backup.phase = BackupPhase::Ready;
            backup.delete_outcome = Some(Err(error));
            return;
        }
    };
    match rt::channel_call(handle, &mut request) {
        Ok(reply) => {
            let _ = backup.on_delete_reply(&reply);
        }
        Err(_) => {
            backup.delete_outcome = Some(Err(note_transport_outcome(backup)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a reply word-exact to backup-service protocol.rs encoders.
    fn reply(tag: u32, words: &[u64]) -> rt::RawMessage {
        let mut message = rt::RawMessage::empty(tag);
        message.word_count = words.len() as u32;
        message.words[..words.len()].copy_from_slice(words);
        message
    }

    fn packed_name_suffix(name: &[u8]) -> Vec<u64> {
        let mut words = vec![0u64; name.len().div_ceil(8)];
        let packed = rt::pack_bytes(name, &mut words).unwrap();
        assert_eq!(packed as usize, words.len());
        words
    }

    #[test]
    fn export_request_validates_scope_mask() {
        let state = BackupUiState::new();
        let request = state
            .encode_export_request(BACKUP_SCOPE_KNOWN_MASK)
            .unwrap();
        assert_eq!(request.tag, BACKUP_TAG_EXPORT_REQUEST);
        assert_eq!(request.word_count, 1);
        assert_eq!(request.words[0], 7);
        assert_eq!(
            state.encode_export_request(1 << 7).unwrap_err(),
            BackupOpError::UnknownScope
        );
        // Mask 0 is wire-legal (service-side selects nothing).
        assert!(state.encode_export_request(0).is_ok());
    }

    #[test]
    fn export_reply_roundtrip() {
        let name = b"20260830T101500";
        let mut words = vec![0u64, name.len() as u64, 12, 4096];
        words.extend_from_slice(&packed_name_suffix(name));
        let reply = reply(BACKUP_TAG_EXPORT_REPLY, &words);
        let mut state = BackupUiState::new();
        let info = state.on_export_reply(&reply).unwrap();
        assert_eq!(&info.name[..info.name_len], name);
        assert_eq!(info.record_count, 12);
        assert_eq!(info.blob_size, 4096);
        assert_eq!(state.phase, BackupPhase::Ready);
        assert_eq!(state.export_outcome, Some(Ok(info)));
        assert_eq!(state.latest_export, Some(info));
    }

    #[test]
    fn export_reply_error_codes_map_honestly() {
        let mut state = BackupUiState::new();
        for (code, expected) in [
            (BACKUP_ERROR_CAPACITY, BackupOpError::CapacityExceeded),
            (BACKUP_ERROR_STORAGE, BackupOpError::StorageFailure),
            (99, BackupOpError::Malformed),
        ] {
            let reply = reply(BACKUP_TAG_EXPORT_REPLY, &[code, 0, 0, 0]);
            assert_eq!(state.on_export_reply(&reply).unwrap_err(), expected);
            assert_eq!(state.export_outcome, Some(Err(expected)));
            assert_eq!(state.phase, BackupPhase::Ready);
        }
    }

    #[test]
    fn export_reply_rejects_short_and_bad_name() {
        let mut state = BackupUiState::new();
        assert_eq!(
            state
                .on_export_reply(&reply(BACKUP_TAG_EXPORT_REPLY, &[0, 0, 0, 0]))
                .unwrap_err(),
            BackupOpError::Malformed
        );
        // name_len over the service's MAX_BACKUP_NAME is malformed.
        let over_long_name_reply = reply(BACKUP_TAG_EXPORT_REPLY, &[0, 33, 0, 0]);
        assert_eq!(
            state.on_export_reply(&over_long_name_reply).unwrap_err(),
            BackupOpError::Malformed
        );
        // Wrong tag is malformed too.
        let wrong_tag_reply = reply(0x999, &[0, 4, 0, 0]);
        assert_eq!(
            state.on_export_reply(&wrong_tag_reply).unwrap_err(),
            BackupOpError::Malformed
        );
    }

    #[test]
    fn list_roundtrip_capacity_bound_and_end() {
        let mut state = BackupUiState::new();
        state.begin_listing();
        assert_eq!(state.phase, BackupPhase::Listing);
        let path = b"backups/20260830T101500";
        let mut words = vec![0u64, 0, 1, path.len() as u64];
        words.extend_from_slice(&packed_name_suffix(path));
        for index in 0..BACKUP_LIST_ROWS as u64 + 2 {
            words[1] = index;
            let reply = reply(BACKUP_TAG_LIST_REPLY, &words);
            state.decode_list_reply(&reply).unwrap();
        }
        assert_eq!(state.entry_count, BACKUP_LIST_ROWS);
        assert_eq!(state.entry_total, BACKUP_LIST_ROWS + 2);
        assert_eq!(state.entries[0].index, 0);
        assert_eq!(
            state.entries[BACKUP_LIST_ROWS - 1].index,
            (BACKUP_LIST_ROWS - 1) as u64 as usize
        );
        assert_eq!(state.entries[0].path_str(), Some("backups/20260830T101500"));
        // End reply: word_count 3, no name.
        let end = reply(BACKUP_TAG_LIST_REPLY, &[BACKUP_LIST_END_STATUS, 0, 1]);
        assert_eq!(state.decode_list_reply(&end).unwrap(), None);
        assert_eq!(state.phase, BackupPhase::Ready);
    }

    #[test]
    fn list_error_and_malformed_degrade() {
        let mut state = BackupUiState::new();
        state.begin_listing();
        // Service-side list failures are status-only replies (word_count 1).
        let storage_reply = reply(BACKUP_TAG_LIST_REPLY, &[BACKUP_ERROR_STORAGE]);
        assert_eq!(
            state.decode_list_reply(&storage_reply).unwrap_err(),
            BackupOpError::StorageFailure
        );
        assert_eq!(state.list_error, Some(BackupOpError::StorageFailure));
        assert_eq!(state.phase, BackupPhase::Idle);
        // A status error reply means the service was REACHED: the route is
        // unchanged, only the operation failed.
        assert_eq!(state.unavailable, Some(BackupUnavailable::NoRoute));

        state.begin_listing();
        // Short reply is malformed, not a panic.
        assert_eq!(
            state
                .decode_list_reply(&reply(BACKUP_TAG_LIST_REPLY, &[0, 0]))
                .unwrap_err(),
            BackupOpError::Malformed
        );
        // Over-long path is malformed.
        let mut words = vec![0u64, 0, 1, BACKUP_PATH_MAX_BYTES as u64 + 1];
        words.extend_from_slice(&[0u64; 8]);
        assert_eq!(
            state
                .decode_list_reply(&reply(BACKUP_TAG_LIST_REPLY, &words))
                .unwrap_err(),
            BackupOpError::Malformed
        );
    }

    #[test]
    fn restore_two_step_dry_run_then_apply_is_gated() {
        let mut state = BackupUiState::new();
        state.begin_listing();
        let path = b"backups/20260830T101500";
        let mut words = vec![0u64, 0, 1, path.len() as u64];
        words.extend_from_slice(&packed_name_suffix(path));
        state
            .decode_list_reply(&reply(BACKUP_TAG_LIST_REPLY, &words))
            .unwrap();
        state
            .decode_list_reply(&reply(
                BACKUP_TAG_LIST_REPLY,
                &[BACKUP_LIST_END_STATUS, 0, 1],
            ))
            .unwrap();
        assert_eq!(state.phase, BackupPhase::Ready);

        // Apply is refused before any dry-run prompt exists.
        assert_eq!(
            state
                .confirm_restore_apply(BACKUP_SCOPE_KNOWN_MASK)
                .unwrap_err(),
            BackupOpError::Invalid
        );

        // Step 1: dry-run request carries dry_run=1 and the plain name.
        let request = state
            .begin_restore_dry_run(BACKUP_SCOPE_KNOWN_MASK)
            .unwrap();
        assert_eq!(request.tag, BACKUP_TAG_RESTORE_REQUEST);
        assert_eq!(request.words[0] as u32, BACKUP_SCOPE_KNOWN_MASK);
        assert_eq!(request.words[1], 1);
        assert_eq!(request.words[2], b"20260830T101500".len() as u64);
        assert_eq!(state.phase, BackupPhase::Restoring);

        // Dry-run reply opens the confirm prompt with the report.
        let report_words = [0u64, 1, 7, 12, 4096];
        let report = state
            .on_restore_reply(&reply(BACKUP_TAG_RESTORE_REPLY, &report_words))
            .unwrap();
        assert!(report.dry_run);
        assert_eq!(report.selected_records, 12);
        assert_eq!(state.prompt, Some(BackupPrompt::RestoreConfirm(report)));
        assert_eq!(state.phase, BackupPhase::Ready);

        // Step 2: apply request carries dry_run=0 and the same snapshot.
        let apply = state
            .confirm_restore_apply(BACKUP_SCOPE_KNOWN_MASK)
            .unwrap();
        assert_eq!(apply.words[1], 0);
        assert_eq!(apply.words[2], request.words[2]);
        assert_eq!(state.prompt, None);
        assert_eq!(state.phase, BackupPhase::Restoring);

        // Apply reply lands as the final outcome.
        let final_words = [0u64, 0, 7, 12, 4096];
        let applied = state
            .on_restore_reply(&reply(BACKUP_TAG_RESTORE_REPLY, &final_words))
            .unwrap();
        assert!(!applied.dry_run);
        assert_eq!(state.restore_outcome, Some(Ok(applied)));
        assert_eq!(state.prompt, None);
    }

    #[test]
    fn restore_errors_and_malformed_degrade() {
        let mut state = BackupUiState::new();
        state.begin_listing();
        let path = b"backups/x1";
        let mut words = vec![0u64, 0, 1, path.len() as u64];
        words.extend_from_slice(&packed_name_suffix(path));
        state
            .decode_list_reply(&reply(BACKUP_TAG_LIST_REPLY, &words))
            .unwrap();

        // Not-found snapshot maps to the service code.
        let error_words = [BACKUP_ERROR_NOT_FOUND, 0, 0, 0, 0];
        assert_eq!(
            state
                .on_restore_reply(&reply(BACKUP_TAG_RESTORE_REPLY, &error_words))
                .unwrap_err(),
            BackupOpError::NotFound
        );
        assert_eq!(state.restore_outcome, Some(Err(BackupOpError::NotFound)));
        assert_eq!(state.phase, BackupPhase::Ready);

        // Short reply is malformed and flags transport, never panics.
        state
            .begin_restore_dry_run(BACKUP_SCOPE_KNOWN_MASK)
            .unwrap();
        assert_eq!(
            state
                .on_restore_reply(&reply(BACKUP_TAG_RESTORE_REPLY, &[0, 0]))
                .unwrap_err(),
            BackupOpError::Malformed
        );
        assert_eq!(state.unavailable, Some(BackupUnavailable::TransportFailure));

        // Corrupt blob code surfaces during dry-run decode.
        state
            .begin_restore_dry_run(BACKUP_SCOPE_KNOWN_MASK)
            .unwrap();
        let corrupt_words = [BACKUP_ERROR_CORRUPT, 0, 0, 0, 0];
        assert_eq!(
            state
                .on_restore_reply(&reply(BACKUP_TAG_RESTORE_REPLY, &corrupt_words))
                .unwrap_err(),
            BackupOpError::Corrupt
        );
    }

    #[test]
    fn delete_flow_gated_and_compacts_rows() {
        let mut state = BackupUiState::new();
        state.begin_listing();
        for (index, path) in
            [0usize, 1, 2]
                .iter()
                .zip([b"backups/a1", b"backups/b2", b"backups/c3"])
        {
            let mut words = vec![0u64, *index as u64, 1, path.len() as u64];
            words.extend_from_slice(&packed_name_suffix(path));
            state
                .decode_list_reply(&reply(BACKUP_TAG_LIST_REPLY, &words))
                .unwrap();
        }
        state
            .decode_list_reply(&reply(
                BACKUP_TAG_LIST_REPLY,
                &[BACKUP_LIST_END_STATUS, 0, 1],
            ))
            .unwrap();

        // Confirm refused without prompt; begin_delete needs a selection.
        assert_eq!(state.confirm_delete().unwrap_err(), BackupOpError::Invalid);
        state.select(1);
        state.begin_delete().unwrap();
        assert_eq!(state.prompt, Some(BackupPrompt::DeleteConfirm));

        // Cancel closes the prompt and the next confirm is refused again.
        state.cancel_prompt();
        assert_eq!(state.confirm_delete().unwrap_err(), BackupOpError::Invalid);

        state.begin_delete().unwrap();
        let request = state.confirm_delete().unwrap();
        assert_eq!(request.tag, BACKUP_TAG_DELETE_REQUEST);
        assert_eq!(request.words[0], 2);
        assert_eq!(state.phase, BackupPhase::Deleting);

        state
            .on_delete_reply(&reply(BACKUP_TAG_DELETE_REPLY, &[0]))
            .unwrap();
        assert_eq!(state.delete_outcome, Some(Ok(())));
        assert_eq!(state.entry_count, 2);
        assert_eq!(state.entries[0].path_str(), Some("backups/a1"));
        assert_eq!(state.entries[1].path_str(), Some("backups/c3"));
        assert_eq!(state.phase, BackupPhase::Ready);

        // Service rejection maps honestly.
        state.begin_delete().unwrap();
        state.confirm_delete().unwrap();
        assert_eq!(
            state
                .on_delete_reply(&reply(BACKUP_TAG_DELETE_REPLY, &[BACKUP_ERROR_STORAGE]))
                .unwrap_err(),
            BackupOpError::StorageFailure
        );
        assert_eq!(
            state.delete_outcome,
            Some(Err(BackupOpError::StorageFailure))
        );
        assert_eq!(state.entry_count, 2);
    }

    #[test]
    fn selection_bounds_and_empty_list() {
        let mut state = BackupUiState::new();
        // No rows: delete/restore targets are refused, not panicked.
        assert_eq!(state.begin_delete().unwrap_err(), BackupOpError::NotFound);
        state.select(3);
        assert_eq!(state.selected, 0);
        assert_eq!(
            state
                .begin_restore_dry_run(BACKUP_SCOPE_KNOWN_MASK)
                .unwrap_err(),
            BackupOpError::NotFound
        );
    }

    #[test]
    fn restore_request_validates_name_and_scope() {
        let state = BackupUiState::new();
        assert_eq!(
            state
                .encode_restore_request_inner(BACKUP_SCOPE_KNOWN_MASK, true, b"")
                .unwrap_err(),
            BackupOpError::Invalid
        );
        assert_eq!(
            state
                .encode_restore_request_inner(BACKUP_SCOPE_KNOWN_MASK, true, &[0xa1; 33])
                .unwrap_err(),
            BackupOpError::Invalid
        );
        assert_eq!(
            state
                .encode_restore_request_inner(1 << 9, true, b"nm")
                .unwrap_err(),
            BackupOpError::UnknownScope
        );
        let request = state.encode_restore_request_inner(0, false, b"nm").unwrap();
        assert_eq!(request.word_count, 4);
        assert_eq!(request.words[1], 0);
    }

    #[test]
    fn delete_request_layout_matches_service_decode() {
        let state = BackupUiState::new();
        assert_eq!(
            state.encode_delete_request(b"").unwrap_err(),
            BackupOpError::Invalid
        );
        assert_eq!(
            state.encode_delete_request(&[0x61; 33]).unwrap_err(),
            BackupOpError::Invalid
        );
        let request = state.encode_delete_request(b"20260830").unwrap();
        assert_eq!(request.tag, BACKUP_TAG_DELETE_REQUEST);
        assert_eq!(request.words[0], 8);
        assert_eq!(request.word_count, 2);
        let mut name = [0u8; BACKUP_NAME_MAX_BYTES];
        rt::unpack_bytes(&request.words[1..request.word_count as usize], 8, &mut name).unwrap();
        assert_eq!(&name[..8], b"20260830");
    }

    #[test]
    fn list_request_layout_matches_service_decode() {
        let state = BackupUiState::new();
        let request = state.encode_list_request(4);
        assert_eq!(request.tag, BACKUP_TAG_LIST_REQUEST);
        assert_eq!(request.word_count, 1);
        assert_eq!(request.words[0], 4);
    }

    #[test]
    fn list_reply_signing_tail_is_additive() {
        let mut state = BackupUiState::new();
        state.begin_listing();
        let path = b"backups/backup-77";
        let mut words = vec![0u64, 0, 1, path.len() as u64];
        words.extend_from_slice(&packed_name_suffix(path));
        // Without the tail (old service): unsigned, key id 0.
        let entry = state
            .decode_list_reply(&reply(BACKUP_TAG_LIST_REPLY, &words.clone()))
            .unwrap()
            .unwrap();
        assert!(!entry.signed);
        assert_eq!(entry.key_id, 0);
        // With the tail: signed + key id ride along.
        words.push(1); // signed
        words.push(0x0123_4567_89ab_cdef); // key id
        let mut state = BackupUiState::new();
        state.begin_listing();
        let entry = state
            .decode_list_reply(&reply(BACKUP_TAG_LIST_REPLY, &words))
            .unwrap()
            .unwrap();
        assert!(entry.signed);
        assert_eq!(entry.key_id, 0x0123_4567_89ab_cdef);
        // End replies stay tail-free.
        let mut state = BackupUiState::new();
        state.begin_listing();
        assert_eq!(
            state.decode_list_reply(&reply(
                BACKUP_TAG_LIST_REPLY,
                &[BACKUP_LIST_END_STATUS, 0, 1]
            )),
            Ok(None)
        );
    }

    #[test]
    fn export_reply_signing_tail_is_additive() {
        let name = b"20260830T101500";
        let mut words = vec![0u64, name.len() as u64, 3, 512];
        words.extend_from_slice(&packed_name_suffix(name));
        words.push(1);
        words.push(0xffeeddccbbaa9988);
        let info = decode_export_reply(&reply(BACKUP_TAG_EXPORT_REPLY, &words)).unwrap();
        assert!(info.signed);
        assert_eq!(info.key_id, 0xffeeddccbbaa9988);
        // Old-service reply (no tail) still decodes unsigned.
        let mut words = vec![0u64, name.len() as u64, 3, 512];
        words.extend_from_slice(&packed_name_suffix(name));
        let info = decode_export_reply(&reply(BACKUP_TAG_EXPORT_REPLY, &words)).unwrap();
        assert!(!info.signed);
        assert_eq!(info.key_id, 0);
    }

    #[test]
    fn restore_reply_verified_tail_is_additive() {
        let mut state = BackupUiState::new();
        // Tail present: verified + key id.
        let report = state
            .on_restore_reply(&reply(
                BACKUP_TAG_RESTORE_REPLY,
                &[0, 1, 7, 12, 4096, 1, 0x1122_3344_5566_7788],
            ))
            .unwrap();
        assert!(report.verified);
        assert_eq!(report.key_id, 0x1122_3344_5566_7788);
        // Old-service reply (5 words) degrades to unverified.
        let mut state = BackupUiState::new();
        let report = state
            .on_restore_reply(&reply(BACKUP_TAG_RESTORE_REPLY, &[0, 1, 7, 12, 4096]))
            .unwrap();
        assert!(!report.verified);
        assert_eq!(report.key_id, 0);
    }

    #[test]
    fn signing_refusals_map_to_distinct_honest_names() {
        let mut state = BackupUiState::new();
        state.begin_listing();
        let path = b"backups/backup-9";
        let mut words = vec![0u64, 0, 1, path.len() as u64];
        words.extend_from_slice(&packed_name_suffix(path));
        state
            .decode_list_reply(&reply(BACKUP_TAG_LIST_REPLY, &words))
            .unwrap();
        for (code, expected, name) in [
            (BACKUP_ERROR_UNSIGNED, BackupOpError::Unsigned, "UNSIGNED"),
            (BACKUP_ERROR_BAD_SIG, BackupOpError::BadSignature, "BAD-SIG"),
        ] {
            assert_eq!(backup_error_from_code(code), Some(expected));
            assert_eq!(backup_error_name(expected), name);
            state
                .begin_restore_dry_run(BACKUP_SCOPE_KNOWN_MASK)
                .unwrap();
            assert_eq!(
                state.on_restore_reply(&reply(BACKUP_TAG_RESTORE_REPLY, &[code, 0, 0, 0, 0, 0, 0])),
                Err(expected)
            );
            assert_eq!(state.restore_outcome, Some(Err(expected)));
            // The prompt never opens on a refused restore.
            assert_eq!(state.prompt, None);
        }
    }

    #[test]
    fn route_position_follows_the_startup_contract() {
        assert_eq!(backup_route_position(7), None);
        assert_eq!(backup_route_position(8), Some(7));
        assert_eq!(backup_route_position(12), Some(7));
    }

    #[test]
    fn page_enter_without_grant_keeps_the_honest_explainer() {
        let mut state = BackupUiState::new();
        assert_eq!(state.unavailable, Some(BackupUnavailable::NoRoute));
        on_page_enter(rt::INVALID_HANDLE, &mut state);
        assert_eq!(state.unavailable, Some(BackupUnavailable::NoRoute));
        assert!(!page_live(&state));
        // A prior transport failure is never downgraded to NoRoute.
        state.unavailable = Some(BackupUnavailable::TransportFailure);
        on_page_enter(rt::INVALID_HANDLE, &mut state);
        assert_eq!(state.unavailable, Some(BackupUnavailable::TransportFailure));
    }

    #[test]
    fn page_enter_clears_prompts() {
        let mut state = BackupUiState::new();
        state.unavailable = None;
        state.phase = BackupPhase::Ready;
        state.entries[0] = BackupSnapshotEntry {
            index: 0,
            path: [0; BACKUP_PATH_MAX_BYTES],
            path_len: 0,
            name: {
                let mut name = [0; BACKUP_NAME_MAX_BYTES];
                name[..8].copy_from_slice(b"backup-1");
                name
            },
            name_len: 8,
            signed: false,
            key_id: 0,
        };
        state.entry_count = 1;
        state.selected = 0;
        let _ = state.begin_delete();
        assert!(state.prompt.is_some());
        on_page_enter(rt::INVALID_HANDLE, &mut state);
        assert!(state.prompt.is_none());
    }

    #[test]
    fn page_live_switch_drives_the_render_contract() {
        let mut state = BackupUiState::new();
        assert!(!page_live(&state));
        state.unavailable = None;
        assert!(page_live(&state));
        // Transport failure flips the page back to the explainer.
        let error = note_transport_outcome(&mut state);
        assert_eq!(error, BackupOpError::Transport);
        assert_eq!(state.phase, BackupPhase::Ready);
        assert_eq!(state.unavailable, Some(BackupUnavailable::TransportFailure));
        assert!(!page_live(&state));
    }
}

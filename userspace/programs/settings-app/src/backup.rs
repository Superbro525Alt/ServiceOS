//! Backup page machinery. The backup service is a manual-activation image
//! (`services/backup-service/program.img`) with no named `ServiceId`; it
//! publishes its public channel only over the launcher handshake, which the
//! settings app does not receive, so this page has no transport route today:
//! the runtime surface is an honest manual-activation explainer pointing at
//! the shell's `backup` command family, and the request/reply state machine
//! below is exercised by host tests only, ready for the future register/
//! lookup route. Wire shapes mirror backup-service `protocol.rs` exactly
//! (status-first replies, 0 = Ok).
//!
//! The unused-code allowance is deliberate: the request builders and reply
//! decoders are dormant until a route exists, and host tests keep them
//! correct in the meantime.
#![allow(dead_code)]

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BackupRestoreReport {
    pub(crate) dry_run: bool,
    pub(crate) selected_scope_mask: u32,
    pub(crate) selected_records: u32,
    pub(crate) total_bytes: u64,
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
        BackupOpError::Transport => "TRANSPORT",
        BackupOpError::Malformed => "MALFORMED",
    }
}

/// Why the page cannot reach the service. Rendered verbatim.
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

    // ---- request builders (pure; the future control layer sends them) ----

    pub(crate) fn encode_list_request(&self) -> rt::RawMessage {
        let mut request = rt::RawMessage::empty(BACKUP_TAG_LIST_REQUEST);
        request.word_count = 1;
        request.words[0] = BACKUP_LIST_ROWS as u64;
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
        let entry = BackupSnapshotEntry {
            index,
            path,
            path_len,
            name,
            name_len: name_bytes.len(),
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
        let report = BackupRestoreReport {
            dry_run: reply.words[1] != 0,
            selected_scope_mask: reply.words[2] as u32,
            selected_records: reply.words[3] as u32,
            total_bytes: reply.words[4],
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
    Ok(BackupExportInfo {
        name,
        name_len,
        record_count: reply.words[2] as u32,
        blob_size: reply.words[3] as usize,
    })
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
    fn list_request_capacity_bounded() {
        let state = BackupUiState::new();
        let request = state.encode_list_request();
        assert_eq!(request.tag, BACKUP_TAG_LIST_REQUEST);
        assert_eq!(request.word_count, 1);
        assert_eq!(request.words[0], BACKUP_LIST_ROWS as u64);
    }
}

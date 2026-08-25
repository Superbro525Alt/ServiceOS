//! Wire protocol for the backup service's own control channel. Requests
//! carry a reply channel as handles[0]; replies are status-first
//! (`BackupError::to_code`, 0 = Ok) followed by op-specific words.

use serviceos_backup_service::{BackupError, MAX_BACKUP_NAME, RestoreReport, backup_tag};
use serviceos_userspace_runtime::RawMessage;

pub struct RequestScratch {
    pub name: [u8; MAX_BACKUP_NAME],
}

impl RequestScratch {
    pub fn new() -> Self {
        Self {
            name: [0; MAX_BACKUP_NAME],
        }
    }
}

impl Default for RequestScratch {
    fn default() -> Self {
        Self::new()
    }
}

pub fn validate_scope_mask(mask: u32) -> Result<(), BackupError> {
    use serviceos_backup_service::scope;
    if mask & !scope::KNOWN_MASK != 0 {
        return Err(BackupError::UnknownScope);
    }
    Ok(())
}

pub fn decode_export_request(request: &RawMessage) -> Result<u32, BackupError> {
    let mask = *request.words.first().ok_or(BackupError::InvalidArgument)? as u32;
    validate_scope_mask(mask)?;
    Ok(mask)
}

/// Reply: [status, name_len, record_count, blob_size, packed name...].
pub fn encode_export_reply(
    response: &mut RawMessage,
    error: Option<BackupError>,
    name: &[u8],
    record_count: u32,
    blob_size: usize,
) {
    response.tag = backup_tag::EXPORT_REPLY;
    response.word_count = 4;
    response.words[0] = match error {
        Some(error) => error.to_code() as u64,
        None => 0,
    };
    response.words[1] = name.len() as u64;
    response.words[2] = record_count as u64;
    response.words[3] = blob_size as u64;
    if let Ok(packed) = serviceos_userspace_runtime::pack_bytes(name, &mut response.words[4..]) {
        response.word_count += packed;
    }
}

pub fn decode_restore_request(
    request: &RawMessage,
    scratch: &mut RequestScratch,
) -> Result<(u32, bool, usize), BackupError> {
    let filter = *request.words.first().ok_or(BackupError::InvalidArgument)? as u32;
    validate_scope_mask(filter)?;
    let dry_run = *request.words.get(1).ok_or(BackupError::InvalidArgument)? != 0;
    let name_offset = 2;
    let len = *request
        .words
        .get(name_offset)
        .ok_or(BackupError::InvalidArgument)? as usize;
    if len > MAX_BACKUP_NAME {
        return Err(BackupError::InvalidArgument);
    }
    serviceos_userspace_runtime::unpack_bytes(
        &request.words[name_offset + 1..request.word_count as usize],
        len,
        &mut scratch.name,
    )
    .map_err(|_| BackupError::InvalidArgument)?;
    Ok((filter, dry_run, len))
}

/// Reply: [status, dry_run, selected_scope_mask, selected_records, total_bytes].
pub fn encode_restore_reply(
    response: &mut RawMessage,
    error: Option<BackupError>,
    dry_run: bool,
    report: RestoreReport,
) {
    response.tag = backup_tag::RESTORE_REPLY;
    response.word_count = 5;
    response.words[0] = match error {
        Some(error) => error.to_code() as u64,
        None => 0,
    };
    response.words[1] = u64::from(dry_run);
    response.words[2] = report.selected_scope_mask as u64;
    response.words[3] = report.selected_records as u64;
    response.words[4] = report.total_bytes;
}

pub fn decode_list_request(request: &RawMessage) -> Result<usize, BackupError> {
    request
        .words
        .first()
        .copied()
        .map(|word| word as usize)
        .ok_or(BackupError::InvalidArgument)
}

/// Reply mirrors the storage list shape: [status, index_echo, kind, name_len,
/// packed path]; status End carries no name.
pub fn encode_list_reply(response: &mut RawMessage, end: bool, index: usize, path: &[u8]) {
    response.tag = backup_tag::LIST_REPLY;
    response.word_count = 4;
    response.words[0] = if end { 2 } else { 0 };
    response.words[1] = index as u64;
    response.words[2] = 1; // StorageEntryKind::File
    response.words[3] = path.len() as u64;
    if !end {
        if let Ok(packed) = serviceos_userspace_runtime::pack_bytes(path, &mut response.words[4..])
        {
            response.word_count += packed;
        }
    } else {
        response.word_count = 3;
    }
}

pub fn decode_delete_request(
    request: &RawMessage,
    scratch: &mut RequestScratch,
) -> Result<usize, BackupError> {
    let len = *request.words.first().ok_or(BackupError::InvalidArgument)? as usize;
    if len > MAX_BACKUP_NAME || len == 0 {
        return Err(BackupError::InvalidArgument);
    }
    serviceos_userspace_runtime::unpack_bytes(
        &request.words[1..request.word_count as usize],
        len,
        &mut scratch.name,
    )
    .map_err(|_| BackupError::InvalidArgument)?;
    Ok(len)
}

pub fn encode_status_reply(response: &mut RawMessage, tag: u32, error: Option<BackupError>) {
    response.tag = tag;
    response.word_count = 1;
    response.words[0] = match error {
        Some(error) => error.to_code() as u64,
        None => 0,
    };
}

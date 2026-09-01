//! Backup service: operator-level snapshot, restore, listing, and deletion
//! of system state (configuration trees, account store, package manifests)
//! as a single versioned backup blob persisted via storage-service
//! contracts under `backups/<name>`.
//!
//! Activation (manual, not in the default boot graph): the image is built
//! into the boot store as `services/backup-service/program.img` and spawned
//! on demand via the manager's stored-image launch path. The service is NOT
//! registered under a named `ServiceId`, mirroring account-service.
//!
//! Startup handle convention (positional): handles[0] = storage-service
//! channel (required: without it the service cannot reach persistent state
//! and exits at startup); handles[1] = launcher announcer channel when the
//! spawner provided one (shell-driven launches do). With an announcer the
//! service publishes its public channel's send-half back to the launcher
//! (peripheral-service handshake shape: protocol-version word plus the
//! handle); with storage only it degrades honestly to a startup log line
//! and stays unreachable through the launcher route.
//!
//! Scopes snapshot these persistent files:
//! - config   -> state/config/<namespace>/settings.cfg (each namespace)
//! - accounts -> state/account/accounts.cfg
//! - packages -> state/packages/{installed.cfg,repos.cfg}
//!
//! Restore validates the backup blob (magic/version/checksum) before any
//! write, supports a dry-run report mode, and maps record names back onto
//! their storage paths inside their scope directories.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

mod protocol;

use core::str;

use rt::{ControlTag, LifecycleEvent, RawMessage};
use serviceos_backup_service::{
    ACCOUNTS_PATH, BACKUPS_DIR, BackupError, BlobView, BlobWriter, CONFIG_DIR, LIST_SCAN_CAP,
    MAX_BACKUP_NAME, MAX_SIGNING_TEXT_BYTES, RestoreReport, SIGNATURE_SUFFIX, SIGNING_CONFIG_PATH,
    SigningIdentity, backup_tag, derive_signing_identity, format_backup_name,
    format_signature_file, format_signing_config, is_signature_name, parse_backup_name,
    parse_signature_file, parse_signing_config, plan_restore, record_storage_path, scope,
    signature_path, verify_blob_signature,
};

use crate::protocol::RequestScratch;

use serviceos_userspace_runtime as rt;

const MAX_PATH: usize = 96;
const MAX_FILE_BYTES: usize = 4096;
const EXIT_OK: u64 = 0;
const EXIT_STARTUP: u64 = 0xfb01;
const EXIT_LOOP: u64 = 0xfb02;

#[cfg(not(test))]
rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return EXIT_STARTUP;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 1 {
        return EXIT_STARTUP;
    }
    let storage_handle = startup.handles[0];

    // Signing identity: load or create the service-local Ed25519 key. Kept
    // in the service's own namespace so a restore can never rewrite the
    // verifier's key; persistence failure degrades to a boot-local identity
    // (exports then stop verifying across restarts — logged honestly).
    let identity = load_or_create_identity(storage_handle);
    let mut note = [0u8; 48];
    let _ = rt::debug_log(identity_note(&identity, &mut note));

    // Public control channel; handed to clients by whoever spawns us. When
    // the launcher passed an announcer (handles[1], account-service's
    // positional contract with storage first), its send-half receives our
    // public channel's send-half; bare storage-only launches stay
    // unreachable through that route and say so.
    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return EXIT_STARTUP,
    };
    if let Some(announcer) = announcer_index(startup.handle_count as usize) {
        let announce = announce_message(public.second);
        let _ = rt::channel_send(startup.handles[announcer], &announce);
    } else {
        let _ = rt::debug_log(b"backup-service: no announcer handle; public channel not published");
    }

    loop {
        if lifecycle_stop_requested(bootstrap) {
            let _ = rt::handle_close(public.first);
            let _ = rt::handle_close(storage_handle);
            return EXIT_OK;
        }

        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                let mut response = RawMessage::empty(0);
                let mut scratch = RequestScratch::new();
                handle_request(
                    storage_handle,
                    &identity,
                    &request,
                    &mut response,
                    &mut scratch,
                );
                if response.tag != 0 {
                    let _ = rt::channel_send(request.handles[0], &response);
                    let _ = rt::handle_close(request.handles[0]);
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return EXIT_LOOP,
        }

        if rt::yield_current().is_err() {
            return EXIT_LOOP;
        }
    }
}

/// Startup handle convention: handles[0] = storage (required), handles[1] =
/// launcher announcer when present. Returns the announcer's index, or None
/// for bare storage-only launches (no publish route) or missing storage
/// (caller already exits before this).
fn announcer_index(handle_count: usize) -> Option<usize> {
    match handle_count {
        0 | 1 => None,
        _ => Some(1),
    }
}

/// The launch handshake reply (peripheral-service shape): protocol-version
/// word plus our public channel's send-half with relay-capable rights.
fn announce_message(public_send: rt::Handle) -> RawMessage {
    let mut announce = RawMessage::empty(0);
    announce.word_count = 1;
    announce.words[0] = 1; // protocol version
    announce.handle_count = 1;
    announce.handles[0] = public_send;
    announce.handle_rights[0] = rt::rights::SEND | rt::rights::DUPLICATE | rt::rights::TRANSFER;
    announce
}

fn lifecycle_stop_requested(bootstrap: rt::Handle) -> bool {
    let mut lifecycle = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut lifecycle) {
        Ok(()) => {
            lifecycle.tag == ControlTag::Lifecycle as u32
                && lifecycle.word_count >= 1
                && lifecycle.words[0] == LifecycleEvent::Stopped as u32 as u64
        }
        Err(rt::Error::QueueEmpty) => false,
        Err(_) => false,
    }
}

fn handle_request(
    storage_handle: rt::Handle,
    identity: &SigningIdentity,
    request: &RawMessage,
    response: &mut RawMessage,
    scratch: &mut RequestScratch,
) {
    match request.tag {
        x if x == backup_tag::EXPORT_REQUEST => {
            handle_export(storage_handle, identity, request, response)
        }
        x if x == backup_tag::RESTORE_REQUEST => {
            handle_restore(storage_handle, identity, request, response, scratch)
        }
        x if x == backup_tag::LIST_REQUEST => handle_list(storage_handle, request, response),
        x if x == backup_tag::DELETE_REQUEST => {
            handle_delete(storage_handle, request, response, scratch)
        }
        x if x == backup_tag::INFO_REQUEST => handle_info(identity, response),
        _ => {}
    }
}

/// Load the signing identity from `state/backup/signing.cfg`, or derive and
/// persist a fresh one (package-service's guest entropy substitute: SHA-512
/// over source, tick, counter, store fingerprint). A config that fails
/// self-validation is treated as corrupt and regenerated; a config that
/// cannot be persisted leaves a boot-local identity (logged at startup).
fn load_or_create_identity(storage_handle: rt::Handle) -> SigningIdentity {
    let mut buffer = [0u8; MAX_SIGNING_TEXT_BYTES];
    if let Ok(loaded) = read_storage_file(storage_handle, SIGNING_CONFIG_PATH, &mut buffer) {
        if let Some(identity) =
            parse_signing_config(str::from_utf8(&buffer[..loaded]).unwrap_or(""))
        {
            return identity;
        }
        let _ = rt::debug_log(
            b"backup-service: signing.cfg unreadable; regenerating identity (old signatures will not verify)",
        );
    }
    let tick = rt::monotonic_now().unwrap_or(0);
    let mut accounts = [0u8; 512];
    let fingerprint = match read_storage_file(storage_handle, ACCOUNTS_PATH, &mut accounts) {
        Ok(loaded) => serviceos_backup_service::fnv1a64(&accounts[..loaded]),
        Err(_) => 0,
    };
    let identity = derive_signing_identity(b"backup-service-signing", tick, 0, fingerprint);
    let mut config = [0u8; MAX_SIGNING_TEXT_BYTES];
    match format_signing_config(&identity, &mut config)
        .ok()
        .and_then(|used| {
            write_storage_file(storage_handle, SIGNING_CONFIG_PATH, &config[..used]).ok()
        }) {
        Some(()) => {}
        None => {
            let _ = rt::debug_log(
                b"backup-service: signing.cfg persist failed; identity is boot-local only",
            );
        }
    }
    identity
}

/// Startup log line: `backup-service: signing key-id=<16 hex>`.
fn identity_note<'a>(identity: &SigningIdentity, out: &'a mut [u8]) -> &'a [u8] {
    let prefix = b"backup-service: signing key-id=";
    out[..prefix.len()].copy_from_slice(prefix);
    let mut id_hex = [0u8; 16];
    let _ = serviceos_backup_service::emit_hex(&identity.key_id.to_be_bytes(), &mut id_hex);
    out[prefix.len()..prefix.len() + 16].copy_from_slice(&id_hex);
    &out[..prefix.len() + 16]
}

// ---------------------------------------------------------------- export

fn handle_export(
    storage_handle: rt::Handle,
    identity: &SigningIdentity,
    request: &RawMessage,
    response: &mut RawMessage,
) {
    let mask = match protocol::decode_export_request(request) {
        Ok(mask) => mask,
        Err(error) => {
            protocol::encode_export_reply(response, Some(error), b"", 0, 0, false, 0);
            return;
        }
    };

    let mut writer = BlobWriter::new();
    if let Err(error) = gather_scopes(storage_handle, mask, &mut writer) {
        protocol::encode_export_reply(response, Some(error), b"", 0, 0, false, 0);
        return;
    }

    writer.finish();
    let record_count = writer.record_count();

    // Name from the monotonic tick; bump past collisions so exports never
    // silently overwrite an existing snapshot.
    let mut tick = rt::monotonic_now().unwrap_or(0);
    let mut name = [0u8; MAX_BACKUP_NAME];
    let name_len = loop {
        let len = format_backup_name(tick, &mut name);
        if !backup_exists(storage_handle, &name[..len]) {
            break len;
        }
        tick += 1;
    };
    let name_bytes = &name[..name_len];

    let mut path = [0u8; MAX_PATH];
    let path_len = backup_path(name_bytes, &mut path);
    let path = str::from_utf8(&path[..path_len]).unwrap_or("");
    if write_storage_file(storage_handle, path, writer.as_slice()).is_err() {
        protocol::encode_export_reply(
            response,
            Some(BackupError::StorageFailure),
            b"",
            0,
            0,
            false,
            0,
        );
        return;
    }

    // Detached signature beside the blob, over the exact stored bytes. A
    // failed sidecar write removes the blob again: an unsigned export would
    // be unrestorable by policy, so it must not linger.
    let signature = serviceos_crypto::ed25519::sign(&identity.seed, writer.as_slice());
    let mut sig_file = [0u8; MAX_SIGNING_TEXT_BYTES];
    let sig_len = match format_signature_file(identity.key_id, &signature, &mut sig_file) {
        Ok(sig_len) => sig_len,
        Err(_) => {
            let _ = remove_backup_file(storage_handle, name_bytes);
            protocol::encode_export_reply(
                response,
                Some(BackupError::StorageFailure),
                b"",
                0,
                0,
                false,
                0,
            );
            return;
        }
    };
    let mut sig_path = [0u8; MAX_PATH];
    let sig_written = signature_path(str::from_utf8(name_bytes).unwrap_or(""), &mut sig_path);
    let sig_written = match sig_written {
        Ok(sig_written) => sig_written,
        Err(_) => {
            let _ = remove_backup_file(storage_handle, name_bytes);
            protocol::encode_export_reply(
                response,
                Some(BackupError::StorageFailure),
                b"",
                0,
                0,
                false,
                0,
            );
            return;
        }
    };
    let sig_path = str::from_utf8(&sig_path[..sig_written]).unwrap_or("");
    match write_storage_file(storage_handle, sig_path, &sig_file[..sig_len]) {
        Ok(()) => protocol::encode_export_reply(
            response,
            None,
            name_bytes,
            record_count,
            writer.as_slice().len(),
            true,
            identity.key_id,
        ),
        Err(_) => {
            let _ = remove_backup_file(storage_handle, name_bytes);
            protocol::encode_export_reply(
                response,
                Some(BackupError::StorageFailure),
                b"",
                0,
                0,
                false,
                0,
            );
        }
    }
}

fn gather_scopes(
    storage_handle: rt::Handle,
    mask: u32,
    writer: &mut BlobWriter,
) -> Result<(), BackupError> {
    if mask & scope::CONFIG != 0 {
        gather_config(storage_handle, writer)?;
    }
    if mask & scope::ACCOUNTS != 0 {
        capture_file(
            storage_handle,
            scope::ACCOUNTS,
            "accounts.cfg",
            ACCOUNTS_PATH,
            writer,
        )?;
    }
    if mask & scope::PACKAGES != 0 {
        capture_file(
            storage_handle,
            scope::PACKAGES,
            "installed.cfg",
            "state/packages/installed.cfg",
            writer,
        )?;
        capture_file(
            storage_handle,
            scope::PACKAGES,
            "repos.cfg",
            "state/packages/repos.cfg",
            writer,
        )?;
    }
    Ok(())
}

fn gather_config(storage_handle: rt::Handle, writer: &mut BlobWriter) -> Result<(), BackupError> {
    let mut child = [0u8; MAX_PATH];
    let mut cursor = 0usize;
    while let Some((kind, next_cursor, len)) =
        list_directory_child(storage_handle, CONFIG_DIR, cursor, &mut child)
    {
        cursor = next_cursor;
        if kind != rt::StorageEntryKind::Directory {
            continue;
        }
        // Children come back as full paths with a trailing slash for
        // directories ("state/config/desktop/"); the namespace is the
        // component between the config prefix and that slash.
        let end = if len > 0 && child[len - 1] == b'/' {
            len - 1
        } else {
            len
        };
        if end <= CONFIG_DIR.len() {
            continue;
        }
        let namespace = match str::from_utf8(&child[CONFIG_DIR.len()..end]) {
            Ok(namespace) => namespace,
            Err(_) => continue,
        };
        let mut record_name = [0u8; MAX_PATH];
        let record_len = push_joined(&mut record_name, &[namespace, "/settings.cfg"]);
        let record_name = str::from_utf8(&record_name[..record_len]).unwrap_or("");
        let mut path = [0u8; MAX_PATH];
        let path_len = push_joined(&mut path, &[CONFIG_DIR, namespace, "/settings.cfg"]);
        let path = str::from_utf8(&path[..path_len]).unwrap_or("");
        capture_file(storage_handle, scope::CONFIG, record_name, path, writer)?;
    }
    Ok(())
}

/// Join path parts into `out`; returns total length.
fn push_joined(out: &mut [u8], parts: &[&str]) -> usize {
    let mut length = 0usize;
    for part in parts {
        out[length..length + part.len()].copy_from_slice(part.as_bytes());
        length += part.len();
    }
    length
}

/// Read one persistent file into the blob as a scope record. A missing file
/// contributes nothing (the scope still counts as captured).
fn capture_file(
    storage_handle: rt::Handle,
    scope_bit: u32,
    record_name: &str,
    storage_path: &str,
    writer: &mut BlobWriter,
) -> Result<(), BackupError> {
    let mut data = [0u8; MAX_FILE_BYTES];
    let loaded = match read_storage_file(storage_handle, storage_path, &mut data) {
        Ok(loaded) => loaded,
        Err(BackupError::NotFound) => return Ok(()),
        Err(error) => return Err(error),
    };
    writer.push_record(scope_bit, record_name, &data[..loaded])
}

// ---------------------------------------------------------------- restore

fn handle_restore(
    storage_handle: rt::Handle,
    identity: &SigningIdentity,
    request: &RawMessage,
    response: &mut RawMessage,
    scratch: &mut RequestScratch,
) {
    let (filter, dry_run, name_len) = match protocol::decode_restore_request(request, scratch) {
        Ok(decoded) => decoded,
        Err(error) => {
            protocol::encode_restore_reply(
                response,
                Some(error),
                false,
                RestoreReport::default(),
                false,
                0,
            );
            return;
        }
    };
    let name = match str::from_utf8(&scratch.name[..name_len]) {
        Ok(name) => name,
        Err(_) => {
            protocol::encode_restore_reply(
                response,
                Some(BackupError::InvalidArgument),
                dry_run,
                RestoreReport::default(),
                false,
                0,
            );
            return;
        }
    };

    let mut blob = [0u8; serviceos_backup_service::MAX_BLOB_BYTES];
    let mut path_buffer = [0u8; MAX_PATH];
    let path_len = backup_path(name.as_bytes(), &mut path_buffer);
    let loaded = match read_storage_file(
        storage_handle,
        str::from_utf8(&path_buffer[..path_len]).unwrap_or(""),
        &mut blob,
    ) {
        Ok(loaded) => loaded,
        Err(error @ BackupError::NotFound) => {
            protocol::encode_restore_reply(
                response,
                Some(error),
                dry_run,
                RestoreReport::default(),
                false,
                0,
            );
            return;
        }
        Err(_) => {
            protocol::encode_restore_reply(
                response,
                Some(BackupError::StorageFailure),
                dry_run,
                RestoreReport::default(),
                false,
                0,
            );
            return;
        }
    };
    let view = match BlobView::open(&blob[..loaded]) {
        Ok(view) => view,
        Err(error) => {
            protocol::encode_restore_reply(
                response,
                Some(error),
                dry_run,
                RestoreReport::default(),
                false,
                0,
            );
            return;
        }
    };

    // Signature gate: before any planning or writing (dry-run included),
    // verify the detached signature over the exact blob bytes. Missing
    // sidecar = Unsigned (policy: unsigned snapshots refuse restore);
    // anything unverifiable = BadSignature. Nothing is written on refusal.
    if let Err(error) = verify_snapshot_signature(storage_handle, identity, name, &blob[..loaded]) {
        protocol::encode_restore_reply(
            response,
            Some(error),
            dry_run,
            RestoreReport::default(),
            false,
            0,
        );
        return;
    }

    let report = match plan_restore(&view, filter) {
        Ok(report) => report,
        Err(error) => {
            protocol::encode_restore_reply(
                response,
                Some(error),
                dry_run,
                RestoreReport::default(),
                false,
                0,
            );
            return;
        }
    };

    if dry_run {
        protocol::encode_restore_reply(response, None, true, report, true, identity.key_id);
        return;
    }

    match apply_restore(storage_handle, &view, filter) {
        Ok(applied) => {
            protocol::encode_restore_reply(response, None, false, applied, true, identity.key_id)
        }
        Err(error) => protocol::encode_restore_reply(
            response,
            Some(error),
            dry_run,
            RestoreReport::default(),
            false,
            0,
        ),
    }
}

/// Verify the detached signature sidecar for `name` against the service's
/// identity. Missing sidecar maps to Unsigned; malformed or mismatched or
/// failing signatures map to BadSignature.
fn verify_snapshot_signature(
    storage_handle: rt::Handle,
    identity: &SigningIdentity,
    name: &str,
    blob: &[u8],
) -> Result<(), BackupError> {
    let mut sig_path = [0u8; MAX_PATH];
    let sig_len = signature_path(name, &mut sig_path)?;
    let mut buffer = [0u8; MAX_SIGNING_TEXT_BYTES];
    let loaded = match read_storage_file(
        storage_handle,
        str::from_utf8(&sig_path[..sig_len]).unwrap_or(""),
        &mut buffer,
    ) {
        Ok(loaded) => loaded,
        Err(BackupError::NotFound) => return Err(BackupError::Unsigned),
        Err(_) => return Err(BackupError::StorageFailure),
    };
    let text = str::from_utf8(&buffer[..loaded]).map_err(|_| BackupError::BadSignature)?;
    let record = parse_signature_file(text).ok_or(BackupError::BadSignature)?;
    verify_blob_signature(&identity.public, blob, &record, identity.key_id)
}

fn apply_restore(
    storage_handle: rt::Handle,
    view: &BlobView<'_>,
    filter: u32,
) -> Result<RestoreReport, BackupError> {
    let mut applied = RestoreReport {
        selected_scope_mask: 0,
        selected_records: 0,
        total_bytes: 0,
    };
    let mut cursor = 0usize;
    while let Some(record) = view.next_record(&mut cursor)? {
        if record.scope & filter == 0 {
            continue;
        }
        let mut path = [0u8; MAX_PATH];
        let path_len = record_storage_path(record, &mut path)?;
        let path = str::from_utf8(&path[..path_len]).unwrap_or("");
        write_storage_file(storage_handle, path, record.data)
            .map_err(|_| BackupError::StorageFailure)?;
        applied.selected_scope_mask |= record.scope;
        applied.selected_records += 1;
        applied.total_bytes += record.data.len() as u64;
    }
    Ok(applied)
}

// ---------------------------------------------------------------- list / delete

fn handle_list(storage_handle: rt::Handle, request: &RawMessage, response: &mut RawMessage) {
    let index = match protocol::decode_list_request(request) {
        Ok(index) => index,
        Err(error) => {
            protocol::encode_status_reply(response, backup_tag::LIST_REPLY, Some(error));
            return;
        }
    };
    // Enumerate backups/ skipping `<name>.sig` sidecars: the requested index
    // counts real snapshots only. Sidecar presence + key id ride the
    // additive reply tail.
    let mut probe = 0usize;
    let mut seen = 0usize;
    let mut path_buffer = [0u8; MAX_PATH];
    while probe < LIST_SCAN_CAP {
        match rt::storage_list(storage_handle, BACKUPS_DIR, probe, &mut path_buffer) {
            Ok(Some((_status, len))) => {
                probe += 1;
                let path = &path_buffer[..len];
                if is_signature_name(str::from_utf8(path).unwrap_or("")) {
                    continue;
                }
                if seen == index {
                    let (signed, key_id) = probe_snapshot_signature(storage_handle, path);
                    protocol::encode_list_reply(response, false, index, path, signed, key_id);
                    return;
                }
                seen += 1;
            }
            Ok(None) => break,
            Err(_) => {
                protocol::encode_status_reply(
                    response,
                    backup_tag::LIST_REPLY,
                    Some(BackupError::StorageFailure),
                );
                return;
            }
        }
    }
    protocol::encode_list_reply(response, true, index, &[], false, 0);
}

/// Sidecar probe for one listed snapshot: presence of `<path>.sig` sets the
/// signed flag; a parseable key id rides along (0 when unreadable — restore
/// still verifies cryptographically, so a bogus probe can never pass).
fn probe_snapshot_signature(storage_handle: rt::Handle, path: &[u8]) -> (bool, u64) {
    let mut sig_path = [0u8; MAX_PATH];
    if path.len() + SIGNATURE_SUFFIX.len() > sig_path.len() {
        return (false, 0);
    }
    sig_path[..path.len()].copy_from_slice(path);
    sig_path[path.len()..path.len() + SIGNATURE_SUFFIX.len()]
        .copy_from_slice(SIGNATURE_SUFFIX.as_bytes());
    let sig_path = str::from_utf8(&sig_path[..path.len() + SIGNATURE_SUFFIX.len()]).unwrap_or("");
    let mut buffer = [0u8; MAX_SIGNING_TEXT_BYTES];
    let loaded = match read_storage_file(storage_handle, sig_path, &mut buffer) {
        Ok(loaded) => loaded,
        Err(_) => return (false, 0),
    };
    let text = str::from_utf8(&buffer[..loaded]).unwrap_or("");
    match parse_signature_file(text) {
        Some(record) => (true, record.key_id),
        None => (true, 0),
    }
}

fn handle_delete(
    storage_handle: rt::Handle,
    request: &RawMessage,
    response: &mut RawMessage,
    scratch: &mut RequestScratch,
) {
    let name_len = match protocol::decode_delete_request(request, scratch) {
        Ok(len) => len,
        Err(error) => {
            protocol::encode_status_reply(response, backup_tag::DELETE_REPLY, Some(error));
            return;
        }
    };
    let name = str::from_utf8(&scratch.name[..name_len]).unwrap_or("");
    // Only plain snapshot names are deletable through this channel, so an
    // operator can never remove anything outside backups/ via this path.
    if parse_backup_name(name).is_none() {
        protocol::encode_status_reply(
            response,
            backup_tag::DELETE_REPLY,
            Some(BackupError::InvalidArgument),
        );
        return;
    }
    delete_backup(storage_handle, response, name);
}

fn delete_backup(storage_handle: rt::Handle, response: &mut RawMessage, name: &str) {
    match remove_backup_file(storage_handle, name.as_bytes()) {
        Ok(()) => protocol::encode_status_reply(response, backup_tag::DELETE_REPLY, None),
        Err(error) => {
            protocol::encode_status_reply(response, backup_tag::DELETE_REPLY, Some(error))
        }
    }
}

/// Remove a snapshot blob plus its signature sidecar (best effort — a
/// missing or unremovable sidecar must not fail a delete of the blob).
/// Only ever called with names that passed `parse_backup_name`, so both
/// paths stay name-confined under backups/.
fn remove_backup_file(storage_handle: rt::Handle, name: &[u8]) -> Result<(), BackupError> {
    let directory = match rt::storage_open_directory(storage_handle, BACKUPS_DIR, true) {
        Ok(directory) => directory,
        Err(rt::Error::NotFound) => return Err(BackupError::NotFound),
        Err(_) => return Err(BackupError::StorageFailure),
    };
    let name = str::from_utf8(name).unwrap_or("");
    let blob_result = rt::storage_directory_remove(directory, name);
    let mut sidecar = [0u8; 48];
    if let Ok(sig_len) = signature_path(name, &mut sidecar) {
        // signature_path yields "backups/<name>.sig"; the directory-relative
        // remove takes the `<name>.sig` tail.
        let _ = rt::storage_directory_remove(
            directory,
            str::from_utf8(&sidecar[BACKUPS_DIR.len()..sig_len]).unwrap_or(""),
        );
    }
    let _ = rt::handle_close(directory);
    match blob_result {
        Ok(()) => Ok(()),
        Err(rt::Error::NotFound) => Err(BackupError::NotFound),
        Err(_) => Err(BackupError::StorageFailure),
    }
}

/// INFO: expose the active signing identity (key id + packed public key).
/// Introspection only — no client flow depends on it; wire tails carry the
/// key id everywhere it matters.
fn handle_info(identity: &SigningIdentity, response: &mut RawMessage) {
    protocol::encode_info_reply(response, None, identity.key_id, &identity.public);
}

// ---------------------------------------------------------------- storage helpers

fn backup_path(name: &[u8], out: &mut [u8]) -> usize {
    out[..BACKUPS_DIR.len()].copy_from_slice(BACKUPS_DIR.as_bytes());
    out[BACKUPS_DIR.len()..BACKUPS_DIR.len() + name.len()].copy_from_slice(name);
    BACKUPS_DIR.len() + name.len()
}

fn backup_exists(storage_handle: rt::Handle, name: &[u8]) -> bool {
    let mut path = [0u8; MAX_PATH];
    let len = backup_path(name, &mut path);
    let path = str::from_utf8(&path[..len]).unwrap_or("");
    match rt::storage_open(storage_handle, path) {
        Ok((blob, _)) => {
            let _ = rt::storage_blob_close(blob);
            true
        }
        Err(_) => false,
    }
}

/// List one direct child of `directory_prefix` starting at `cursor`.
/// Returns (kind, next_cursor, child_path_length) with the full child path
/// written into `out`.
fn list_directory_child(
    storage_handle: rt::Handle,
    directory_prefix: &str,
    cursor: usize,
    out: &mut [u8],
) -> Option<(rt::StorageEntryKind, usize, usize)> {
    match rt::storage_list_directory(storage_handle, directory_prefix, cursor, out) {
        Ok(Some((next_cursor, kind, len))) => Some((kind, next_cursor, len)),
        Ok(None) => None,
        Err(_) => None,
    }
}

fn read_storage_file(
    storage_handle: rt::Handle,
    path: &str,
    buffer: &mut [u8],
) -> Result<usize, BackupError> {
    let (blob, expected_len) = match rt::storage_open(storage_handle, path) {
        Ok(opened) => opened,
        Err(rt::Error::NotFound) => return Err(BackupError::NotFound),
        Err(_) => return Err(BackupError::StorageFailure),
    };
    if expected_len > buffer.len() {
        let _ = rt::storage_blob_close(blob);
        return Err(BackupError::CapacityExceeded);
    }
    let loaded = match rt::storage_read_all(blob, buffer, expected_len) {
        Ok(loaded) => loaded,
        Err(_) => {
            let _ = rt::storage_blob_close(blob);
            return Err(BackupError::StorageFailure);
        }
    };
    let _ = rt::storage_blob_close(blob);
    Ok(loaded)
}

fn write_storage_file(
    storage_handle: rt::Handle,
    path: &str,
    bytes: &[u8],
) -> Result<(), BackupError> {
    let (parent, file_name) = split_parent(path);
    ensure_directory(storage_handle, parent)?;
    let directory = rt::storage_open_directory(storage_handle, parent, true)
        .map_err(|_| BackupError::StorageFailure)?;
    let file = match rt::storage_directory_open_file(directory, file_name, true, true) {
        Ok((file, _)) => file,
        Err(_) => {
            let _ = rt::handle_close(directory);
            return Err(BackupError::StorageFailure);
        }
    };
    let _ = rt::handle_close(directory);
    let mut offset = 0usize;
    while offset < bytes.len() {
        let chunk_len = (bytes.len() - offset).min((rt::IPC_MAX_WORDS - 3) * 8);
        if rt::storage_write(
            file,
            offset,
            bytes.len(),
            &bytes[offset..offset + chunk_len],
        )
        .is_err()
        {
            let _ = rt::storage_blob_close(file);
            return Err(BackupError::StorageFailure);
        }
        offset += chunk_len;
    }
    let _ = rt::storage_blob_close(file);
    Ok(())
}

fn ensure_directory(storage_handle: rt::Handle, path: &str) -> Result<(), BackupError> {
    if path.is_empty() {
        return Ok(());
    }
    if rt::storage_open_directory(storage_handle, path, true).is_ok() {
        return Ok(());
    }
    let (parent, name) = split_parent(path);
    let directory = rt::storage_open_directory(storage_handle, parent, true)
        .map_err(|_| BackupError::StorageFailure)?;
    let created = rt::storage_directory_create(directory, name, rt::StorageEntryKind::Directory);
    let _ = rt::handle_close(directory);
    match created {
        Ok(()) | Err(rt::Error::Busy) => Ok(()),
        Err(_) => Err(BackupError::StorageFailure),
    }
}

/// Split "a/b/c.cfg" into ("a/b", "c.cfg"). Paths reaching here always have
/// a directory component.
fn split_parent(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(position) => (&path[..position], &path[position + 1..]),
        None => ("", path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announcer_index_follows_positional_storage_first_contract() {
        // Missing storage: main exits before reaching the publish decision.
        assert_eq!(announcer_index(0), None);
        // Storage only: bare launch, no publish route (honest degrade).
        assert_eq!(announcer_index(1), None);
        // Shell-driven launch: storage at [0], announcer at [1].
        assert_eq!(announcer_index(2), Some(1));
        assert_eq!(announcer_index(3), Some(1));
    }

    #[test]
    fn announce_message_carries_version_and_relay_capable_rights() {
        let announce = announce_message(7);
        assert_eq!(announce.word_count, 1);
        assert_eq!(announce.words[0], 1);
        assert_eq!(announce.handle_count, 1);
        assert_eq!(announce.handles[0], 7);
        assert_eq!(
            announce.handle_rights[0],
            rt::rights::SEND | rt::rights::DUPLICATE | rt::rights::TRANSFER
        );
    }
}

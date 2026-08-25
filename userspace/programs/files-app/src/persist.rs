use serviceos_userspace_runtime as rt;

use crate::assoc::{ASSOC_MAX, AssocTable};
use crate::recent::{RECENT_MAX, RecentRing};
use crate::state::MAX_STORAGE_PATH;

const STORE_DIR_NAME: &str = "files-app";
const STORE_DIR_PREFIX: &str = "state/files-app/";
const STATE_ROOT: &str = "state/";
const ASSOC_FILE: &str = "associations.cfg";
const RECENT_FILE: &str = "recent.cfg";

/// Opens (creating if needed) the writable per-app store directory under the
/// persistent `state/` mount. Returns INVALID_HANDLE when unavailable; every
/// caller degrades gracefully to in-memory-only operation then.
pub(crate) fn ensure_store_dir(storage: rt::Handle) -> rt::Handle {
    if let Ok(handle) = rt::storage_open_directory(storage, STORE_DIR_PREFIX, true) {
        return handle;
    }
    let Ok(root) = rt::storage_open_directory(storage, STATE_ROOT, true) else {
        return rt::INVALID_HANDLE;
    };
    let _ = rt::storage_directory_create(root, STORE_DIR_NAME, rt::StorageEntryKind::Directory);
    let _ = rt::handle_close(root);
    match rt::storage_open_directory(storage, STORE_DIR_PREFIX, true) {
        Ok(handle) => handle,
        Err(_) => rt::INVALID_HANDLE,
    }
}

fn save_file(dir: rt::Handle, name: &str, bytes: &[u8]) -> rt::Result<()> {
    if dir == rt::INVALID_HANDLE {
        return Err(rt::Error::NotFound);
    }
    let (file, _) = rt::storage_directory_open_file(dir, name, true, true)?;
    let result = rt::storage_write(file, 0, bytes.len(), bytes);
    let _ = rt::handle_close(file);
    result.map(|_| ())
}

fn load_file(dir: rt::Handle, name: &str, buffer: &mut [u8]) -> rt::Result<usize> {
    if dir == rt::INVALID_HANDLE {
        return Err(rt::Error::NotFound);
    }
    let (file, size) = rt::storage_directory_open_file(dir, name, false, false)?;
    let read = rt::storage_read_all(file, buffer, size.min(buffer.len()));
    let _ = rt::handle_close(file);
    read
}

pub(crate) fn save_associations(dir: rt::Handle, table: &AssocTable) {
    let mut buffer = [0u8; ASSOC_MAX * 12];
    let len = table.encode(&mut buffer);
    let _ = save_file(dir, ASSOC_FILE, &buffer[..len]);
}

pub(crate) fn load_associations(dir: rt::Handle, table: &mut AssocTable) {
    let mut buffer = [0u8; ASSOC_MAX * 12];
    let Ok(len) = load_file(dir, ASSOC_FILE, &mut buffer) else {
        return;
    };
    *table = AssocTable::decode(&buffer[..len]);
}

pub(crate) fn save_recent(dir: rt::Handle, ring: &RecentRing) {
    let mut buffer = [0u8; RECENT_MAX * (MAX_STORAGE_PATH + 1)];
    let len = ring.encode(&mut buffer);
    let _ = save_file(dir, RECENT_FILE, &buffer[..len]);
}

pub(crate) fn load_recent(dir: rt::Handle, ring: &mut RecentRing) {
    let mut buffer = [0u8; RECENT_MAX * (MAX_STORAGE_PATH + 1)];
    let Ok(len) = load_file(dir, RECENT_FILE, &mut buffer) else {
        return;
    };
    *ring = RecentRing::decode(&buffer[..len]);
}

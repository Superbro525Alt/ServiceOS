use serviceos_bundle::{BootStoreEntryKind, BootStoreEntryRecord};
use serviceos_userspace_runtime as rt;
use rt::{Handle, StorageEntryKind};

pub(crate) const MAX_BOOTSTORE_ENTRIES: usize = 128;
pub(crate) const MAX_BLOB_SESSIONS: usize = 24;
pub(crate) const MAX_DIRECTORY_SESSIONS: usize = 24;
pub(crate) const MAX_MUTABLE_ENTRIES: usize = 128;
pub(crate) const BOOT_ENTRY_BYTES: usize = BootStoreEntryRecord::encoded_len();
pub(crate) const MAX_STORAGE_PATH: usize = serviceos_bundle::BOOT_STORE_PATH_MAX;
pub(crate) const INITIAL_FILE_CAPACITY: usize = 256;
pub(crate) const PERSISTENT_MAGIC: [u8; 8] = *b"SOSPSTR1";
pub(crate) const PERSISTENT_VERSION: u32 = 1;
pub(crate) const PERSISTENT_RECORD_BYTES: usize = 128;
pub(crate) const BLOCK_BUFFER_BYTES: usize = 512;

pub(crate) const MUTABLE_ROOT_HOME: &[u8] = b"home/";
pub(crate) const MUTABLE_ROOT_TMP: &[u8] = b"tmp/";
pub(crate) const MUTABLE_ROOT_STATE: &[u8] = b"state/";
pub(crate) const MUTABLE_ROOT_PROJECTS: &[u8] = b"projects/";
pub(crate) const MUTABLE_ROOTS: [&[u8]; 4] = [
    MUTABLE_ROOT_HOME,
    MUTABLE_ROOT_TMP,
    MUTABLE_ROOT_STATE,
    MUTABLE_ROOT_PROJECTS,
];
pub(crate) const PERSISTENT_ROOTS: [&[u8]; 3] =
    [MUTABLE_ROOT_HOME, MUTABLE_ROOT_STATE, MUTABLE_ROOT_PROJECTS];

#[derive(Clone, Copy)]
pub(crate) struct EntrySlot {
    pub(crate) kind: BootStoreEntryKind,
    pub(crate) data_offset: usize,
    pub(crate) data_len: usize,
    pub(crate) path: [u8; MAX_STORAGE_PATH],
    pub(crate) path_len: usize,
}

impl EntrySlot {
    pub(crate) const fn empty() -> Self {
        Self {
            kind: BootStoreEntryKind::Data,
            data_offset: 0,
            data_len: 0,
            path: [0; MAX_STORAGE_PATH],
            path_len: 0,
        }
    }

    pub(crate) fn matches(&self, path: &[u8]) -> bool {
        self.path_len == path.len() && self.path[..self.path_len] == *path
    }

    pub(crate) fn matches_prefix(&self, prefix: &[u8]) -> bool {
        prefix.len() <= self.path_len && self.path[..prefix.len()] == *prefix
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum BlobSource {
    BootStore,
    Mutable,
}

#[derive(Clone, Copy)]
pub(crate) struct BlobSession {
    pub(crate) endpoint: Handle,
    pub(crate) source: BlobSource,
    pub(crate) data_offset: usize,
    pub(crate) data_len: usize,
    pub(crate) data_handle: Handle,
    pub(crate) entry_index: usize,
    pub(crate) writable: bool,
    pub(crate) occupied: bool,
}

impl BlobSession {
    pub(crate) const fn empty() -> Self {
        Self {
            endpoint: rt::INVALID_HANDLE,
            source: BlobSource::BootStore,
            data_offset: 0,
            data_len: 0,
            data_handle: rt::INVALID_HANDLE,
            entry_index: usize::MAX,
            writable: false,
            occupied: false,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct DirectorySession {
    pub(crate) endpoint: Handle,
    pub(crate) path: [u8; MAX_STORAGE_PATH],
    pub(crate) path_len: usize,
    pub(crate) writable: bool,
    pub(crate) occupied: bool,
}

impl DirectorySession {
    pub(crate) const fn empty() -> Self {
        Self {
            endpoint: rt::INVALID_HANDLE,
            path: [0; MAX_STORAGE_PATH],
            path_len: 0,
            writable: false,
            occupied: false,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct MutableEntry {
    pub(crate) kind: StorageEntryKind,
    pub(crate) path: [u8; MAX_STORAGE_PATH],
    pub(crate) path_len: usize,
    pub(crate) data_handle: Handle,
    pub(crate) data_len: usize,
    pub(crate) data_capacity: usize,
    pub(crate) occupied: bool,
}

impl MutableEntry {
    pub(crate) const fn empty() -> Self {
        Self {
            kind: StorageEntryKind::File,
            path: [0; MAX_STORAGE_PATH],
            path_len: 0,
            data_handle: rt::INVALID_HANDLE,
            data_len: 0,
            data_capacity: 0,
            occupied: false,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct PersistentStore {
    pub(crate) handle: Handle,
    pub(crate) block_size: usize,
    pub(crate) slot_blocks: usize,
    pub(crate) active_slot: usize,
    pub(crate) generation: u64,
}

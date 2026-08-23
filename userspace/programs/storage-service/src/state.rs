use rt::{Handle, StorageEntryKind};
use serviceos_bundle::{BootStoreEntryKind, BootStoreEntryRecord};
use serviceos_userspace_runtime as rt;

pub(crate) const MAX_BOOTSTORE_ENTRIES: usize = 128;
pub(crate) const MAX_BLOB_SESSIONS: usize = 24;
pub(crate) const MAX_DIRECTORY_SESSIONS: usize = 24;
pub(crate) const MAX_MUTABLE_ENTRIES: usize = 128;
pub(crate) const BOOT_ENTRY_BYTES: usize = BootStoreEntryRecord::encoded_len();
pub(crate) const MAX_STORAGE_PATH: usize = serviceos_bundle::BOOT_STORE_PATH_MAX;
pub(crate) const INITIAL_FILE_CAPACITY: usize = 256;
pub(crate) const PERSISTENT_MAGIC: [u8; 8] = *b"SOSPSTR1";
pub(crate) const PERSISTENT_VERSION: u32 = 2;
pub(crate) const PERSISTENT_RECORD_BYTES: usize = 128;
pub(crate) const BLOCK_BUFFER_BYTES: usize = 512;

pub(crate) type MountTable = [rt::StorageMount; rt::STORAGE_MOUNT_TABLE_MAX];
pub(crate) const MOUNT_RECORD_BYTES: usize = 128;

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
    pub(crate) path: [u8; MAX_STORAGE_PATH],
    pub(crate) path_len: usize,
    pub(crate) mount_path: [u8; rt::STORAGE_MOUNT_PATH_MAX],
    pub(crate) mount_path_len: usize,
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
            path: [0; MAX_STORAGE_PATH],
            path_len: 0,
            mount_path: [0; rt::STORAGE_MOUNT_PATH_MAX],
            mount_path_len: 0,
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
    pub(crate) mount_path: [u8; rt::STORAGE_MOUNT_PATH_MAX],
    pub(crate) mount_path_len: usize,
    pub(crate) writable: bool,
    pub(crate) occupied: bool,
}

impl DirectorySession {
    pub(crate) const fn empty() -> Self {
        Self {
            endpoint: rt::INVALID_HANDLE,
            path: [0; MAX_STORAGE_PATH],
            path_len: 0,
            mount_path: [0; rt::STORAGE_MOUNT_PATH_MAX],
            mount_path_len: 0,
            writable: false,
            occupied: false,
        }
    }

    /// Path of the mount this session was opened under; empty slice for the boot root.
    pub(crate) fn mount_prefix(&self) -> &[u8] {
        &self.mount_path[..self.mount_path_len]
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
    /// Survives unmount and reboot (backed by a persistent mount when created).
    pub(crate) persistent: bool,
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
            persistent: false,
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

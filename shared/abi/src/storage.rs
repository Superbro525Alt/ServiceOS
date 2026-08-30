#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageTag {
    OpenRequest = 0x500,
    OpenReply = 0x501,
    ReadRequest = 0x502,
    ReadReply = 0x503,
    ListRequest = 0x504,
    ListReply = 0x505,
    CloseRequest = 0x506,
    DirectoryListRequest = 0x507,
    DirectoryListReply = 0x508,
    DirectoryOpenRequest = 0x509,
    DirectoryOpenReply = 0x50a,
    DirectoryCreateRequest = 0x50b,
    DirectoryCreateReply = 0x50c,
    DirectoryRemoveRequest = 0x50d,
    DirectoryRemoveReply = 0x50e,
    DirectoryOpenFileRequest = 0x50f,
    DirectoryOpenFileReply = 0x510,
    WriteRequest = 0x511,
    WriteReply = 0x512,
    DirectoryReadRequest = 0x513,
    DirectoryReadReply = 0x514,
    MountListRequest = 0x515,
    MountListReply = 0x516,
    DirectoryTraverseRequest = 0x517,
    DirectoryTraverseReply = 0x518,
    MountRequest = 0x519,
    MountReply = 0x51a,
    UnmountRequest = 0x51b,
    UnmountReply = 0x51c,
    StatRequest = 0x51d,
    StatReply = 0x51e,
    FindRequest = 0x51f,
    FindReply = 0x520,
    RenameRequest = 0x527,
    RenameReply = 0x528,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageStatus {
    Ok = 0,
    NotFound = 1,
    InvalidPath = 2,
    InvalidOffset = 3,
    End = 4,
    Busy = 5,
    Denied = 6,
    AlreadyExists = 7,
    NotDirectory = 8,
    NotMounted = 9,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageEntryKind {
    File = 0,
    Directory = 1,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageMountKind {
    Boot = 0,
    Persistent = 1,
    Ephemeral = 2,
    Temp = 3,
}

pub const STORAGE_MOUNT_TABLE_MAX: usize = 16;
pub const STORAGE_MOUNT_PATH_MAX: usize = 96;
pub const STORAGE_MOUNT_FLAG_WRITABLE: u64 = 1 << 0;
pub const STORAGE_MOUNT_FLAG_PERSISTENT: u64 = 1 << 1;

pub const STORAGE_ROOT_AUTHORITY: u64 = 0x5354_4f52_4155_5448;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StorageMount {
    pub occupied: bool,
    pub path: [u8; STORAGE_MOUNT_PATH_MAX],
    pub path_len: usize,
    pub kind: StorageMountKind,
    pub flags: u64,
    pub authority: u64,
}

impl StorageMount {
    pub const fn empty() -> Self {
        Self {
            occupied: false,
            path: [0; STORAGE_MOUNT_PATH_MAX],
            path_len: 0,
            kind: StorageMountKind::Boot,
            flags: 0,
            authority: 0,
        }
    }

    pub fn install(
        &mut self,
        path: &[u8],
        kind: StorageMountKind,
        flags: u64,
        authority: u64,
    ) -> Result<(), StorageStatus> {
        if authority != STORAGE_ROOT_AUTHORITY {
            return Err(StorageStatus::Denied);
        }
        storage_validate_mount_path(path)?;
        slot_install(self, path, kind, flags, authority)
    }

    pub fn clear(&mut self) {
        *self = Self::empty();
    }

    pub fn writable(&self) -> bool {
        self.flags & STORAGE_MOUNT_FLAG_WRITABLE != 0
    }

    pub fn persistent(&self) -> bool {
        self.flags & STORAGE_MOUNT_FLAG_PERSISTENT != 0
    }

    pub fn matches_prefix(&self, path: &[u8]) -> bool {
        self.occupied
            && path.len() >= self.path_len
            && path[..self.path_len] == self.path[..self.path_len]
    }
}

pub fn storage_validate_mount_path(path: &[u8]) -> Result<(), StorageStatus> {
    if path.is_empty() {
        return Ok(());
    }
    if !path.ends_with(b"/") || path[0] == b'/' {
        return Err(StorageStatus::InvalidPath);
    }
    let mut component_start = 0usize;
    for index in 0..path.len() {
        if path[index] == b'/' {
            let component = &path[component_start..index];
            if component.is_empty() || component == b"." || component == b".." {
                return Err(StorageStatus::InvalidPath);
            }
            component_start = index + 1;
        }
    }
    Ok(())
}

pub fn storage_resolve_mount<'a>(
    mounts: &'a [StorageMount],
    path: &[u8],
) -> Option<&'a StorageMount> {
    mounts
        .iter()
        .filter(|mount| mount.matches_prefix(path))
        .max_by_key(|mount| mount.path_len)
}

pub fn storage_mount_authority_ok(mount: &StorageMount, authority: u64) -> bool {
    mount.authority == authority && authority != 0
}

pub fn storage_mount_add(
    mounts: &mut [StorageMount],
    path: &[u8],
    kind: StorageMountKind,
    flags: u64,
    authority: u64,
) -> Result<usize, StorageStatus> {
    if authority != STORAGE_ROOT_AUTHORITY {
        return Err(StorageStatus::Denied);
    }
    storage_validate_mount_path(path)?;
    if storage_find_mount_by_path(mounts, path).is_some() {
        return Err(StorageStatus::AlreadyExists);
    }
    let slot = mounts
        .iter_mut()
        .position(|mount| !mount.occupied)
        .ok_or(StorageStatus::Busy)?;
    slot_install(&mut mounts[slot], path, kind, flags, authority)?;
    Ok(slot)
}

fn slot_install(
    mount: &mut StorageMount,
    path: &[u8],
    kind: StorageMountKind,
    flags: u64,
    authority: u64,
) -> Result<(), StorageStatus> {
    if path.len() > STORAGE_MOUNT_PATH_MAX {
        return Err(StorageStatus::InvalidPath);
    }
    mount.occupied = true;
    mount.path = [0; STORAGE_MOUNT_PATH_MAX];
    mount.path[..path.len()].copy_from_slice(path);
    mount.path_len = path.len();
    mount.kind = kind;
    mount.flags = flags;
    mount.authority = authority;
    Ok(())
}

pub fn storage_find_mount_by_path(mounts: &[StorageMount], path: &[u8]) -> Option<usize> {
    mounts.iter().position(|mount| {
        mount.occupied && mount.path_len == path.len() && mount.path[..mount.path_len] == *path
    })
}

/// True when `path` names a mounted namespace root exactly (e.g. `data/`,
/// `home/`). Enumeration advertises these as virtual directories even when no
/// concrete entry backs them yet, so opening one must not require an existing
/// entry.
pub fn storage_path_is_mount_root(mounts: &[StorageMount], path: &[u8]) -> bool {
    !path.is_empty()
        && storage_resolve_mount(mounts, path).is_some_and(|mount| {
            mount.path_len > 0
                && path.len() == mount.path_len
                && path == &mount.path[..mount.path_len]
        })
}

pub fn storage_unmount_busy(open_paths: &[&[u8]], prefix: &[u8]) -> bool {
    open_paths
        .iter()
        .any(|open| open.len() >= prefix.len() && open[..prefix.len()] == *prefix)
}

pub fn storage_name_matches(pattern: &[u8], name: &[u8]) -> bool {
    match pattern.first() {
        None => name.is_empty(),
        Some(b'*') => {
            for skip in 0..=name.len() {
                if storage_name_matches(&pattern[1..], &name[skip..]) {
                    return true;
                }
            }
            false
        }
        Some(first) => {
            name.first() == Some(first) && storage_name_matches(&pattern[1..], &name[1..])
        }
    }
}

pub fn storage_find_entry_matches(root: &[u8], pattern: &[u8], path: &[u8]) -> bool {
    if path.len() <= root.len() || !path.starts_with(root) {
        return false;
    }
    let remainder = &path[root.len()..];
    if remainder.ends_with(b"/") {
        return storage_name_matches(pattern, &remainder[..remainder.len() - 1]);
    }
    storage_name_matches(pattern, remainder)
}

pub fn storage_relative_components_valid(relative: &[u8]) -> bool {
    if relative.is_empty() || relative[0] == b'/' {
        return false;
    }
    relative
        .split(|byte| *byte == b'/')
        .all(|component| !component.is_empty() && component != b"." && component != b"..")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mount(path: &[u8], kind: StorageMountKind, flags: u64) -> StorageMount {
        let mut mount = StorageMount::empty();
        assert_eq!(
            mount.install(path, kind, flags, STORAGE_ROOT_AUTHORITY),
            Ok(())
        );
        mount
    }

    fn table() -> [StorageMount; STORAGE_MOUNT_TABLE_MAX] {
        let mut mounts = [StorageMount::empty(); STORAGE_MOUNT_TABLE_MAX];
        let seeded = [
            (b"" as &[u8], StorageMountKind::Boot, 0u64),
            (
                b"home/",
                StorageMountKind::Persistent,
                STORAGE_MOUNT_FLAG_WRITABLE | STORAGE_MOUNT_FLAG_PERSISTENT,
            ),
            (
                b"state/",
                StorageMountKind::Persistent,
                STORAGE_MOUNT_FLAG_WRITABLE | STORAGE_MOUNT_FLAG_PERSISTENT,
            ),
            (
                b"tmp/",
                StorageMountKind::Ephemeral,
                STORAGE_MOUNT_FLAG_WRITABLE,
            ),
            (
                b"scratch/",
                StorageMountKind::Temp,
                STORAGE_MOUNT_FLAG_WRITABLE,
            ),
        ];
        for (slot, (path, kind, flags)) in mounts.iter_mut().zip(seeded.iter()) {
            assert_eq!(
                slot.install(path, *kind, *flags, STORAGE_ROOT_AUTHORITY),
                Ok(())
            );
        }
        mounts
    }

    #[test]
    fn longest_prefix_resolves_composed_namespace() {
        let mounts = table();
        let resolve = |path: &[u8]| {
            storage_resolve_mount(&mounts, path).map(|m| m.path[..m.path_len].to_vec())
        };
        assert_eq!(resolve(b"home/user/a.txt").as_deref(), Some(&b"home/"[..]));
        assert_eq!(resolve(b"tmp/x").as_deref(), Some(&b"tmp/"[..]));
        assert_eq!(resolve(b"scratch/f").as_deref(), Some(&b"scratch/"[..]));
        assert_eq!(resolve(b"boot.bin").as_deref(), Some(&b""[..]));
    }

    #[test]
    fn mount_roots_identified_without_backing_entries() {
        let mounts = table();
        assert!(storage_path_is_mount_root(&mounts, b"home/"));
        assert!(storage_path_is_mount_root(&mounts, b"tmp/"));
        // Child inside a mount is not itself a mount root.
        assert!(!storage_path_is_mount_root(&mounts, b"home/user/a.txt"));
        // Look-alikes and non-paths are not mount roots.
        assert!(!storage_path_is_mount_root(&mounts, b"homes/"));
        assert!(!storage_path_is_mount_root(&mounts, b"home"));
        assert!(!storage_path_is_mount_root(&mounts, b"boot.bin"));
        assert!(!storage_path_is_mount_root(&mounts, b""));
    }

    #[test]
    fn mount_rejects_duplicate_prefix() {
        let mut mounts = table();
        assert_eq!(
            storage_mount_add(
                &mut mounts,
                b"home/",
                StorageMountKind::Temp,
                STORAGE_MOUNT_FLAG_WRITABLE,
                STORAGE_ROOT_AUTHORITY
            ),
            Err(StorageStatus::AlreadyExists)
        );
        assert_eq!(
            storage_mount_add(
                &mut mounts,
                b"data/",
                StorageMountKind::Persistent,
                STORAGE_MOUNT_FLAG_WRITABLE | STORAGE_MOUNT_FLAG_PERSISTENT,
                STORAGE_ROOT_AUTHORITY
            ),
            Ok(5)
        );
        assert_eq!(
            storage_resolve_mount(&mounts, b"data/a.txt").map(|m| m.kind),
            Some(StorageMountKind::Persistent)
        );
    }

    #[test]
    fn unmount_busy_detection_over_open_paths() {
        let open: [&[u8]; 2] = [b"data/a.txt", b"other/b"];
        assert!(storage_unmount_busy(&open, b"data/"));
        assert!(storage_unmount_busy(&[&b"data/"[..]], b"data/"));
        assert!(!storage_unmount_busy(&open, b"scratch/"));
        assert!(!storage_unmount_busy(&[], b"data/"));
    }

    #[test]
    fn mount_validation_gates_authority_and_paths() {
        let mut target = StorageMount::empty();
        assert_eq!(
            target.install(b"data/", StorageMountKind::Persistent, 0b11, 0xdead),
            Err(StorageStatus::Denied)
        );
        assert_eq!(
            target.install(
                b"data/",
                StorageMountKind::Persistent,
                0b11,
                STORAGE_ROOT_AUTHORITY
            ),
            Ok(())
        );
        assert_eq!(target.path_len, 5);
        assert!(target.persistent() && target.writable());

        let mounts = [mount(b"data/", StorageMountKind::Persistent, 0b11)];
        assert_eq!(storage_find_mount_by_path(&mounts, b"data/"), Some(0));
        assert_eq!(storage_find_mount_by_path(&mounts, b"dat/"), None);

        for bad in [&b"data"[..], &b"/data/"[..], &b"a//b/"[..], &b"x/../y/"[..]] {
            assert_eq!(
                storage_validate_mount_path(bad),
                Err(StorageStatus::InvalidPath),
                "path {:?} should be invalid",
                core::str::from_utf8(bad)
            );
        }
        assert_eq!(storage_validate_mount_path(b""), Ok(()));
        assert_eq!(storage_validate_mount_path(b"ok/dir/"), Ok(()));
    }

    #[test]
    fn glob_query_filters_names_and_subtrees() {
        assert!(storage_name_matches(b"*.txt", b"a.txt"));
        assert!(storage_name_matches(b"*.txt", b"dir/c.txt"));
        assert!(storage_name_matches(b"report*", b"report-final"));
        assert!(!storage_name_matches(b"*.txt", b"b.log"));
        assert!(!storage_name_matches(b"", b"x"));

        assert!(storage_find_entry_matches(
            b"data/",
            b"*.txt",
            b"data/a.txt"
        ));
        assert!(storage_find_entry_matches(
            b"data/",
            b"*",
            b"data/sub/nested"
        ));
        assert!(!storage_find_entry_matches(
            b"data/",
            b"*.txt",
            b"home/a.txt"
        ));
        assert!(!storage_find_entry_matches(
            b"data/",
            b"*.log",
            b"data/a.txt"
        ));
    }

    #[test]
    fn relative_capability_boundary_rules() {
        for bad in [
            &b""[..],
            &b"/abs"[..],
            &b"../escape"[..],
            &b"a/../b"[..],
            &b"./here"[..],
        ] {
            assert!(
                !storage_relative_components_valid(bad),
                "relative {:?} must be rejected",
                core::str::from_utf8(bad)
            );
        }
        assert!(storage_relative_components_valid(b"a/b.txt"));
        assert!(storage_relative_components_valid(b"dir-name"));
    }
}

use serviceos_userspace_runtime::StorageEntryKind;

use crate::state::{EntrySlot, MAX_MUTABLE_ENTRIES, MAX_STORAGE_PATH, MountTable, MutableEntry};

pub(crate) fn find_mutable_entry(
    entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    path: &[u8],
) -> Option<usize> {
    entries.iter().position(|entry| {
        entry.occupied && entry.path_len == path.len() && entry.path[..entry.path_len] == *path
    })
}

pub(crate) fn find_mutable_directory(
    entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    path: &[u8],
) -> Option<usize> {
    find_mutable_entry(entries, path)
        .filter(|index| entries[*index].kind == StorageEntryKind::Directory)
}

/// Longest-prefix mount resolution; `None` means no mount claims this path.
pub(crate) fn resolve_mount<'a>(
    mounts: &'a MountTable,
    path: &[u8],
) -> Option<&'a serviceos_userspace_runtime::StorageMount> {
    serviceos_userspace_runtime::storage_resolve_mount(mounts, path)
}

/// A path is writable when its owning mount grants write access.
pub(crate) fn mount_allows_write(mounts: &MountTable, path: &[u8]) -> bool {
    resolve_mount(mounts, path).is_some_and(|mount| mount.writable())
}

/// Legacy alias kept for readability at call sites: mutable == writable mount.
pub(crate) fn is_mutable_path(mounts: &MountTable, path: &[u8]) -> bool {
    mount_allows_write(mounts, path)
}

pub(crate) fn valid_directory_path(path: &[u8]) -> bool {
    path.is_empty() || path.ends_with(b"/")
}

pub(crate) fn path_matches_prefix(path: &[u8], prefix: &[u8]) -> bool {
    prefix.len() <= path.len() && path[..prefix.len()] == *prefix
}

pub(crate) fn boot_directory_exists(entries: &[EntrySlot], path: &[u8]) -> bool {
    entries.iter().any(|entry| {
        entry.path_len > path.len() && path_matches_prefix(&entry.path[..entry.path_len], path)
    })
}

pub(crate) fn mutable_directory_has_children(
    entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    path: &[u8],
) -> bool {
    entries.iter().any(|entry| {
        entry.occupied
            && entry.path_len > path.len()
            && path_matches_prefix(&entry.path[..entry.path_len], path)
    })
}

pub(crate) fn subtree_has_entries(
    entries: &[EntrySlot],
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    root: &[u8],
) -> bool {
    if root.is_empty() {
        return false;
    }
    boot_directory_exists(entries, root)
        || mutable_entries.iter().any(|entry| {
            entry.occupied && entry.path_len >= root.len() && entry.path[..root.len()] == *root
        })
}

pub(crate) fn directory_child_from_path<'a>(
    path: &'a [u8],
    prefix: &[u8],
) -> Option<(&'a [u8], StorageEntryKind)> {
    if !path_matches_prefix(path, prefix) || path.len() == prefix.len() {
        return None;
    }
    let relative = &path[prefix.len()..];
    let Some(component_len) = relative.iter().position(|byte| *byte == b'/') else {
        return Some((path, StorageEntryKind::File));
    };
    if component_len == 0 {
        return None;
    }
    let child_len = prefix.len() + component_len + 1;
    Some((&path[..child_len], StorageEntryKind::Directory))
}

pub(crate) fn compose_child_path(
    parent: &[u8],
    name: &[u8],
    kind: StorageEntryKind,
) -> Option<([u8; MAX_STORAGE_PATH], usize)> {
    if name.is_empty() || name.contains(&b'/') {
        return None;
    }
    let suffix = if kind == StorageEntryKind::Directory {
        1
    } else {
        0
    };
    let total_len = parent.len().checked_add(name.len())?.checked_add(suffix)?;
    if total_len > MAX_STORAGE_PATH {
        return None;
    }
    let mut path = [0u8; MAX_STORAGE_PATH];
    path[..parent.len()].copy_from_slice(parent);
    path[parent.len()..parent.len() + name.len()].copy_from_slice(name);
    if suffix == 1 {
        path[total_len - 1] = b'/';
    }
    Some((path, total_len))
}

pub(crate) fn compose_relative_path(
    parent: &[u8],
    relative: &[u8],
    kind: StorageEntryKind,
) -> Option<([u8; MAX_STORAGE_PATH], usize)> {
    // Boundary rule for relative capability traversal: reject absolute paths,
    // empty components, and any `.`/`..` escape before composing.
    if !serviceos_userspace_runtime::storage_relative_components_valid(relative) {
        return None;
    }

    let mut path = [0u8; MAX_STORAGE_PATH];
    let mut len = parent.len();
    if len > MAX_STORAGE_PATH {
        return None;
    }
    path[..len].copy_from_slice(parent);

    for (index, component) in relative.split(|byte| *byte == b'/').enumerate() {
        if index != 0 && (len == 0 || path[len - 1] != b'/') {
            if len >= MAX_STORAGE_PATH {
                return None;
            }
            path[len] = b'/';
            len += 1;
        }
        let end = len.checked_add(component.len())?;
        if end > MAX_STORAGE_PATH {
            return None;
        }
        path[len..end].copy_from_slice(component);
        len = end;
        if len < MAX_STORAGE_PATH {
            path[len] = b'/';
        }
    }

    match kind {
        StorageEntryKind::Directory => {
            if len == 0 || path[len - 1] != b'/' {
                if len >= MAX_STORAGE_PATH {
                    return None;
                }
                path[len] = b'/';
                len += 1;
            }
        }
        StorageEntryKind::File => {
            if len > 0 && path[len - 1] == b'/' {
                len -= 1;
            }
        }
    }

    Some((path, len))
}

pub(crate) fn directory_exists(
    entries: &[EntrySlot],
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    path: &[u8],
) -> bool {
    path.is_empty()
        || boot_directory_exists(entries, path)
        || find_mutable_directory(mutable_entries, path).is_some()
}

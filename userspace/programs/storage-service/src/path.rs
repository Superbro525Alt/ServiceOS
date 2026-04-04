use serviceos_userspace_runtime::StorageEntryKind;

use crate::{
    state::{
        EntrySlot, MutableEntry, MAX_MUTABLE_ENTRIES, MAX_STORAGE_PATH, MUTABLE_ROOTS,
    },
};

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

pub(crate) fn valid_directory_path(path: &[u8]) -> bool {
    path.is_empty() || path.ends_with(b"/")
}

pub(crate) fn is_mutable_root(path: &[u8]) -> bool {
    MUTABLE_ROOTS.iter().any(|root| *root == path)
}

pub(crate) fn is_mutable_path(path: &[u8]) -> bool {
    MUTABLE_ROOTS
        .iter()
        .any(|root| path.len() >= root.len() && path[..root.len()] == **root)
}

pub(crate) fn is_mutable_directory_path(path: &[u8]) -> bool {
    valid_directory_path(path) && (path.is_empty() || is_mutable_root(path) || is_mutable_path(path))
}

pub(crate) fn path_matches_prefix(path: &[u8], prefix: &[u8]) -> bool {
    prefix.len() <= path.len() && path[..prefix.len()] == *prefix
}

pub(crate) fn boot_directory_exists(entries: &[EntrySlot], path: &[u8]) -> bool {
    entries
        .iter()
        .any(|entry| entry.path_len > path.len() && path_matches_prefix(&entry.path[..entry.path_len], path))
}

pub(crate) fn mutable_directory_has_children(
    entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    path: &[u8],
) -> bool {
    entries
        .iter()
        .any(|entry| entry.occupied && entry.path_len > path.len() && path_matches_prefix(&entry.path[..entry.path_len], path))
}

pub(crate) fn mutable_root_has_materialized_children(
    entries: &[EntrySlot],
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    root: &[u8],
) -> bool {
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
    if name.is_empty() || name.iter().any(|byte| *byte == b'/') {
        return None;
    }
    let suffix = if kind == StorageEntryKind::Directory { 1 } else { 0 };
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
    if relative.is_empty() || relative[0] == b'/' {
        return None;
    }

    let mut path = [0u8; MAX_STORAGE_PATH];
    let mut len = parent.len();
    if len > MAX_STORAGE_PATH {
        return None;
    }
    path[..len].copy_from_slice(parent);

    for (index, component) in relative.split(|byte| *byte == b'/').enumerate() {
        if component.is_empty() || component == b"." || component == b".." {
            return None;
        }
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
        || is_mutable_root(path)
        || boot_directory_exists(entries, path)
        || find_mutable_directory(mutable_entries, path).is_some()
}

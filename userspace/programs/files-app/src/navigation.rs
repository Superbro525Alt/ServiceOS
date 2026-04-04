use core::{cmp::Ordering, fmt::Write, str};

use serviceos_userspace_runtime as rt;
use rt::FixedLogBuffer;

use crate::state::{
    EntryKind, ExplorerEntry, ExplorerState, MAX_ENTRIES, MAX_STORAGE_PATH, ROW_HEIGHT,
    LIST_BOTTOM_MARGIN, LIST_Y, BUFFER_HEIGHT,
};

pub(crate) fn visible_row_count(state: &ExplorerState) -> usize {
    let height = state.height.min(BUFFER_HEIGHT) as usize;
    height
        .saturating_sub(LIST_Y + LIST_BOTTOM_MARGIN)
        .checked_div(ROW_HEIGHT)
        .unwrap_or(0)
}

pub(crate) fn reopen_directory(
    state: &mut ExplorerState,
    storage_handle: rt::Handle,
) -> rt::Result<()> {
    if state.current_directory_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(state.current_directory_handle);
        state.current_directory_handle = rt::INVALID_HANDLE;
    }

    let prefix = str::from_utf8(&state.current_path[..state.current_path_len]).unwrap_or("");
    state.current_directory_handle = rt::storage_open_directory(storage_handle, prefix, false)?;
    Ok(())
}

pub(crate) fn reload_directory(state: &mut ExplorerState) -> rt::Result<()> {
    state.entry_count = 0;
    state.scroll_offset = 0;
    state.selected_index = 0;
    state.load_failed = false;

    if state.current_path_len != 0 {
        let parent_len = parent_path_bytes(
            &state.current_path[..state.current_path_len],
            &mut state.entries[0].path,
        );
        state.entries[0].kind = EntryKind::Parent;
        state.entries[0].path_len = parent_len;
        state.entry_count = 1;
    }
    let mut index = 0usize;
    let mut path_buffer = [0u8; MAX_STORAGE_PATH];
    loop {
        match rt::storage_directory_read(state.current_directory_handle, index, &mut path_buffer) {
            Ok(Some((next_index, kind, path_len))) => {
                insert_unique_entry(
                    state,
                    match kind {
                        rt::StorageEntryKind::Directory => EntryKind::Directory,
                        rt::StorageEntryKind::File => EntryKind::File,
                    },
                    &path_buffer[..path_len],
                );
                if next_index <= index {
                    break;
                }
                index = next_index;
            }
            Ok(None) => break,
            Err(error) => {
                state.load_failed = true;
                return Err(error);
            }
        }
    }

    sort_entries(state);
    clamp_view(state);
    Ok(())
}

pub(crate) fn entry_name_bytes(entry: &ExplorerEntry) -> &[u8] {
    if entry.kind == EntryKind::Parent {
        return b"..";
    }
    let path = &entry.path[..entry.path_len];
    let end = if entry.kind == EntryKind::Directory && entry.path_len > 0 {
        entry.path_len - 1
    } else {
        entry.path_len
    };
    let start = path[..end]
        .iter()
        .rposition(|byte| *byte == b'/')
        .map(|index| index + 1)
        .unwrap_or(0);
    &path[start..end]
}

pub(crate) fn push_selected_path(buffer: &mut FixedLogBuffer<128>, entry: ExplorerEntry) {
    if entry.kind == EntryKind::Parent {
        let _ = write!(buffer, "UP /");
        return;
    }
    if let Ok(path) = str::from_utf8(&entry.path[..entry.path_len]) {
        let _ = write!(buffer, "/{path}");
    } else {
        let _ = write!(buffer, "INVALID");
    }
}

pub(crate) fn open_selected(
    state: &mut ExplorerState,
    storage_handle: rt::Handle,
) -> rt::Result<()> {
    if state.entry_count == 0 {
        return Ok(());
    }
    let selected = state.entries[state.selected_index.min(state.entry_count - 1)];
    match selected.kind {
        EntryKind::Parent | EntryKind::Directory => {
            state.current_path_len = selected.path_len;
            state.current_path[..selected.path_len]
                .copy_from_slice(&selected.path[..selected.path_len]);
            reopen_directory(state, storage_handle)?;
            reload_directory(state)
        }
        EntryKind::File => Ok(()),
    }
}

pub(crate) fn open_path_in_explorer(
    state: &mut ExplorerState,
    storage_handle: rt::Handle,
    path: &[u8],
) -> rt::Result<()> {
    if path.len() > MAX_STORAGE_PATH {
        return Err(rt::Error::BufferTooSmall);
    }
    let path_text = str::from_utf8(path).map_err(|_| rt::Error::InvalidArgument)?;
    let is_directory = rt::storage_open_directory(storage_handle, path_text, false)
        .map(|handle| {
            let _ = rt::handle_close(handle);
            true
        })
        .unwrap_or(false);

    if is_directory || path.ends_with(b"/") {
        state.current_path_len = path.len();
        state.current_path[..path.len()].copy_from_slice(path);
        reopen_directory(state, storage_handle)?;
        reload_directory(state)?;
        return Ok(());
    }

    let mut parent = [0u8; MAX_STORAGE_PATH];
    let parent_len = parent_path_bytes(path, &mut parent);
    state.current_path_len = parent_len;
    state.current_path[..parent_len].copy_from_slice(&parent[..parent_len]);
    reopen_directory(state, storage_handle)?;
    reload_directory(state)?;
    for index in 0..state.entry_count {
        let entry = state.entries[index];
        if entry.kind == EntryKind::File
            && entry.path_len == path.len()
            && entry.path[..entry.path_len] == path[..]
        {
            state.selected_index = index;
            ensure_selected_visible(state);
            break;
        }
    }
    Ok(())
}

pub(crate) fn navigate_parent(state: &mut ExplorerState) {
    let mut parent = [0u8; MAX_STORAGE_PATH];
    let len = parent_path_bytes(&state.current_path[..state.current_path_len], &mut parent);
    state.current_path[..len].copy_from_slice(&parent[..len]);
    state.current_path_len = len;
}

pub(crate) fn ensure_selected_visible(state: &mut ExplorerState) {
    let visible = visible_row_count(state).max(1);
    if state.selected_index < state.scroll_offset {
        state.scroll_offset = state.selected_index;
    } else if state.selected_index >= state.scroll_offset + visible {
        state.scroll_offset = state.selected_index + 1 - visible;
    }
}

pub(crate) fn clamp_view(state: &mut ExplorerState) {
    if state.entry_count == 0 {
        state.selected_index = 0;
        state.scroll_offset = 0;
        return;
    }
    state.selected_index = state.selected_index.min(state.entry_count - 1);
    let visible = visible_row_count(state).max(1);
    let max_scroll = state.entry_count.saturating_sub(visible);
    state.scroll_offset = state.scroll_offset.min(max_scroll);
    ensure_selected_visible(state);
}

pub(crate) fn scroll_up(state: &mut ExplorerState, amount: usize) {
    state.scroll_offset = state.scroll_offset.saturating_sub(amount);
    if state.selected_index > state.scroll_offset + visible_row_count(state).saturating_sub(1) {
        state.selected_index = state.scroll_offset;
    }
}

pub(crate) fn scroll_down(state: &mut ExplorerState, amount: usize) {
    let visible = visible_row_count(state).max(1);
    let max_scroll = state.entry_count.saturating_sub(visible);
    state.scroll_offset = (state.scroll_offset + amount).min(max_scroll);
    if state.selected_index < state.scroll_offset {
        state.selected_index = state.scroll_offset;
    }
}

fn insert_unique_entry(state: &mut ExplorerState, kind: EntryKind, path: &[u8]) {
    if state.entry_count >= MAX_ENTRIES {
        return;
    }
    for entry in state.entries.iter().take(state.entry_count) {
        if entry.kind == kind && entry.path_len == path.len() && entry.path[..entry.path_len] == path[..] {
            return;
        }
    }
    let entry = &mut state.entries[state.entry_count];
    entry.kind = kind;
    entry.path_len = path.len();
    entry.path[..path.len()].copy_from_slice(path);
    state.entry_count += 1;
}

fn sort_entries(state: &mut ExplorerState) {
    let start = if state.entry_count > 0 && state.entries[0].kind == EntryKind::Parent {
        1
    } else {
        0
    };
    let mut index = start + 1;
    while index < state.entry_count {
        let current = state.entries[index];
        let mut scan = index;
        while scan > start {
            let previous = state.entries[scan - 1];
            if compare_entries(previous, current) != Ordering::Greater {
                break;
            }
            state.entries[scan] = previous;
            scan -= 1;
        }
        state.entries[scan] = current;
        index += 1;
    }
}

fn compare_entries(left: ExplorerEntry, right: ExplorerEntry) -> Ordering {
    match (left.kind, right.kind) {
        (EntryKind::Directory, EntryKind::File) => Ordering::Less,
        (EntryKind::File, EntryKind::Directory) => Ordering::Greater,
        _ => compare_case_fold(entry_name_bytes(&left), entry_name_bytes(&right)),
    }
}

fn compare_case_fold(left: &[u8], right: &[u8]) -> Ordering {
    let shared = left.len().min(right.len());
    for index in 0..shared {
        let left_byte = left[index].to_ascii_lowercase();
        let right_byte = right[index].to_ascii_lowercase();
        match left_byte.cmp(&right_byte) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    left.len().cmp(&right.len())
}

fn parent_path_bytes(path: &[u8], output: &mut [u8; MAX_STORAGE_PATH]) -> usize {
    if path.is_empty() {
        return 0;
    }
    let trimmed = &path[..path.len().saturating_sub(1)];
    let Some(separator) = trimmed.iter().rposition(|byte| *byte == b'/') else {
        return 0;
    };
    let len = separator + 1;
    output[..len].copy_from_slice(&trimmed[..len]);
    len
}

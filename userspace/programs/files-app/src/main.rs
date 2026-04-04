#![no_std]
#![no_main]

use core::{array, cmp::Ordering, fmt::Write, str};

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{
    AppControlTag, AppKeyAction, AppPointerAction, ControlTag, FixedLogBuffer, LifecycleEvent,
    RawMessage,
};

const BUFFER_WIDTH: u32 = 1024;
const BUFFER_HEIGHT: u32 = 768;
const BUFFER_BYTES: usize = BUFFER_WIDTH as usize * BUFFER_HEIGHT as usize * 4;
const SURFACE_BUFFER_SLOTS: usize = 2;
const PIXEL_STRIDE: usize = BUFFER_WIDTH as usize;
const MAX_STORAGE_PATH: usize = 96;
const MAX_ENTRIES: usize = 64;
const LIST_X: usize = 12;
const LIST_Y: usize = ui::TITLEBAR_HEIGHT as usize + 34;
const LIST_BOTTOM_MARGIN: usize = 18;
const ROW_HEIGHT: usize = 14;
const KEY_BACKSPACE: u32 = 14;
const KEY_ENTER: u32 = 28;
const KEY_UP: u32 = 103;
const KEY_PAGE_UP: u32 = 104;
const KEY_LEFT: u32 = 105;
const KEY_RIGHT: u32 = 106;
const KEY_DOWN: u32 = 108;
const KEY_PAGE_DOWN: u32 = 109;
const MOD_SHIFT: u32 = 1 << 0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntryKind {
    Parent,
    Directory,
    File,
}

#[derive(Clone, Copy)]
struct ExplorerEntry {
    kind: EntryKind,
    path: [u8; MAX_STORAGE_PATH],
    path_len: usize,
}

impl ExplorerEntry {
    const fn empty() -> Self {
        Self {
            kind: EntryKind::File,
            path: [0; MAX_STORAGE_PATH],
            path_len: 0,
        }
    }
}

struct ExplorerState {
    width: u32,
    height: u32,
    focused: bool,
    current_directory_handle: rt::Handle,
    current_path: [u8; MAX_STORAGE_PATH],
    current_path_len: usize,
    entries: [ExplorerEntry; MAX_ENTRIES],
    entry_count: usize,
    selected_index: usize,
    scroll_offset: usize,
    load_failed: bool,
}

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf101;
    }
    if startup.tag != ControlTag::Startup as u32
        || startup.handle_count < 3
        || startup.word_count < 4
    {
        return 0xf102;
    }

    let surface_handle = startup.handles[0];
    let control_handle = startup.handles[1];
    let storage_handle = startup.handles[2];
    let mut state = ExplorerState {
        width: startup.words[1] as u32,
        height: startup.words[2] as u32,
        focused: startup.words[3] != 0,
        current_directory_handle: rt::INVALID_HANDLE,
        current_path: [0; MAX_STORAGE_PATH],
        current_path_len: 0,
        entries: [ExplorerEntry::empty(); MAX_ENTRIES],
        entry_count: 0,
        selected_index: 0,
        scroll_offset: 0,
        load_failed: false,
    };

    let mut buffer_handles = [rt::INVALID_HANDLE; SURFACE_BUFFER_SLOTS];
    let mut mapped_buffers: [Option<rt::MappedMemory>; SURFACE_BUFFER_SLOTS] =
        array::from_fn(|_| None);
    for slot in 0..SURFACE_BUFFER_SLOTS {
        let buffer_handle = match rt::memory_create(BUFFER_BYTES, true) {
            Ok(handle) => handle,
            Err(_) => return 0xf103,
        };
        if rt::surface_attach_buffer_slot(
            surface_handle,
            slot as u32,
            buffer_handle,
            BUFFER_WIDTH,
            BUFFER_HEIGHT,
            BUFFER_WIDTH,
        )
        .is_err()
        {
            let _ = rt::handle_close(buffer_handle);
            return 0xf104;
        }
        let mapped_buffer = match rt::MappedMemory::map(buffer_handle, BUFFER_BYTES, true) {
            Ok(buffer) => buffer,
            Err(_) => {
                let _ = rt::handle_close(buffer_handle);
                return 0xf108;
            }
        };
        buffer_handles[slot] = buffer_handle;
        mapped_buffers[slot] = Some(mapped_buffer);
    }
    let mut front_buffer_slot = 0usize;

    let _ = reopen_directory(&mut state, storage_handle);
    let _ = reload_directory(&mut state);
    let _ = render(
        surface_handle,
        front_buffer_slot as u32,
        mapped_buffers[front_buffer_slot].as_mut().unwrap(),
        &state,
    );

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => break,
            Ok(false) => {}
            Err(_) => return 0xf105,
        }

        match poll_control(
            control_handle,
            surface_handle,
            &mut mapped_buffers,
            &mut front_buffer_slot,
            storage_handle,
            &mut state,
        ) {
            Ok(ControlFlow::Idle) => {}
            Ok(ControlFlow::Worked) => continue,
            Ok(ControlFlow::Exit) => break,
            Err(_) => return 0xf106,
        }

        if rt::yield_current().is_err() {
            return 0xf107;
        }
    }

    if state.current_directory_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(state.current_directory_handle);
    }
    for handle in buffer_handles {
        if handle != rt::INVALID_HANDLE {
            let _ = rt::handle_close(handle);
        }
    }
    0
}

enum ControlFlow {
    Idle,
    Worked,
    Exit,
}

fn poll_control(
    control_handle: rt::Handle,
    surface_handle: rt::Handle,
    buffers: &mut [Option<rt::MappedMemory>; SURFACE_BUFFER_SLOTS],
    front_buffer_slot: &mut usize,
    storage_handle: rt::Handle,
    state: &mut ExplorerState,
) -> rt::Result<ControlFlow> {
    let mut changed = false;
    let mut did_work = false;
    loop {
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(control_handle, &mut message) {
            Ok(()) if message.tag == AppControlTag::FocusChanged as u32 && message.word_count > 0 => {
                did_work = true;
                state.focused = message.words[0] != 0;
                changed = true;
            }
            Ok(()) if message.tag == AppControlTag::Resize as u32 && message.word_count >= 2 => {
                did_work = true;
                state.width = message.words[0] as u32;
                state.height = message.words[1] as u32;
                clamp_view(state);
                changed = true;
            }
            Ok(()) if message.tag == AppControlTag::Pointer as u32 && message.word_count >= 5 => {
                did_work = true;
                let action = pointer_action_from_word(message.words[0]);
                let x = message.words[1] as i64 as i32;
                let y = message.words[2] as i64 as i32;
                let detail = message.words[4] as i64 as i32;
                match action {
                    Some(AppPointerAction::Down) => {
                        changed |= handle_pointer_down(state, storage_handle, x, y)?;
                    }
                    Some(AppPointerAction::Scroll) => {
                        if detail > 0 {
                            scroll_up(state, detail as usize);
                            changed = true;
                        } else if detail < 0 {
                            scroll_down(state, (-detail) as usize);
                            changed = true;
                        }
                    }
                    _ => {}
                }
            }
            Ok(()) if message.tag == AppControlTag::Key as u32 && message.word_count >= 2 => {
                did_work = true;
                if matches!(key_action_from_word(message.words[0]), Some(AppKeyAction::Down)) {
                    changed |= handle_key_down(
                        state,
                        storage_handle,
                        message.words[1] as u32,
                        message.words.get(2).copied().unwrap_or(0) as u32,
                    )?;
                }
            }
            Ok(()) if message.tag == AppControlTag::OpenPath as u32 && message.word_count >= 1 => {
                did_work = true;
                let requested = message.words[0] as usize;
                let mut path = [0u8; MAX_STORAGE_PATH];
                if rt::unpack_bytes(
                    &message.words[1..message.word_count as usize],
                    requested,
                    &mut path,
                )
                .is_ok()
                {
                    changed |= open_path_in_explorer(state, storage_handle, &path[..requested]).is_ok();
                }
            }
            Ok(()) if message.tag == AppControlTag::Close as u32 => return Ok(ControlFlow::Exit),
            Ok(()) => {
                did_work = true;
            }
            Err(rt::Error::QueueEmpty) => break,
            Err(error) => return Err(error),
        }
    }

    if changed {
        *front_buffer_slot = (*front_buffer_slot + 1) % SURFACE_BUFFER_SLOTS;
        render(
            surface_handle,
            *front_buffer_slot as u32,
            buffers[*front_buffer_slot].as_mut().unwrap(),
            state,
        )?;
        return Ok(ControlFlow::Worked);
    }

    if did_work {
        Ok(ControlFlow::Worked)
    } else {
        Ok(ControlFlow::Idle)
    }
}

fn handle_pointer_down(
    state: &mut ExplorerState,
    storage_handle: rt::Handle,
    x: i32,
    y: i32,
) -> rt::Result<bool> {
    if x < LIST_X as i32 || y < LIST_Y as i32 {
        return Ok(false);
    }
    let visible_rows = visible_row_count(state);
    if visible_rows == 0 {
        return Ok(false);
    }
    let row = ((y as usize).saturating_sub(LIST_Y)) / ROW_HEIGHT;
    if row >= visible_rows {
        return Ok(false);
    }
    let index = state.scroll_offset + row;
    if index >= state.entry_count {
        return Ok(false);
    }

    state.selected_index = index;
    ensure_selected_visible(state);
    if matches!(
        state.entries[index].kind,
        EntryKind::Parent | EntryKind::Directory
    ) {
        open_selected(state, storage_handle)?;
    }
    Ok(true)
}

fn handle_key_down(
    state: &mut ExplorerState,
    storage_handle: rt::Handle,
    key_code: u32,
    modifiers: u32,
) -> rt::Result<bool> {
    let visible_rows = visible_row_count(state).max(1);
    match key_code {
        KEY_UP => {
            if state.selected_index > 0 {
                state.selected_index -= 1;
                ensure_selected_visible(state);
                return Ok(true);
            }
        }
        KEY_DOWN => {
            if state.selected_index + 1 < state.entry_count {
                state.selected_index += 1;
                ensure_selected_visible(state);
                return Ok(true);
            }
        }
        KEY_PAGE_UP => {
            let amount = visible_rows.saturating_sub(1).max(1);
            state.selected_index = state.selected_index.saturating_sub(amount);
            ensure_selected_visible(state);
            return Ok(true);
        }
        KEY_PAGE_DOWN => {
            let amount = visible_rows.saturating_sub(1).max(1);
            state.selected_index = (state.selected_index + amount).min(state.entry_count.saturating_sub(1));
            ensure_selected_visible(state);
            return Ok(true);
        }
        KEY_ENTER | KEY_RIGHT => {
            return open_selected(state, storage_handle).map(|_| true);
        }
        KEY_LEFT | KEY_BACKSPACE => {
            if modifiers & MOD_SHIFT != 0 {
                state.scroll_offset = 0;
                state.selected_index = 0;
                return Ok(true);
            }
            if state.current_path_len != 0 {
                navigate_parent(state);
                reopen_directory(state, storage_handle)?;
                reload_directory(state)?;
                return Ok(true);
            }
        }
        _ => {}
    }
    Ok(false)
}

fn render(
    surface_handle: rt::Handle,
    buffer_slot: u32,
    buffer: &mut rt::MappedMemory,
    state: &ExplorerState,
) -> rt::Result<()> {
    let width = state.width.min(BUFFER_WIDTH) as usize;
    let height = state.height.min(BUFFER_HEIGHT) as usize;
    let bytes = &mut buffer.as_slice_mut()[..BUFFER_BYTES];

    fill_rect(bytes, 0, 0, width, height, ui::BG_WINDOW_ALT);
    fill_rect(
        bytes,
        0,
        0,
        width,
        ui::TITLEBAR_HEIGHT as usize,
        if state.focused {
            ui::ACCENT
        } else {
            ui::ACCENT_DIM
        },
    );
    fill_rect(
        bytes,
        0,
        ui::TITLEBAR_HEIGHT as usize,
        width,
        height.saturating_sub(ui::TITLEBAR_HEIGHT as usize),
        ui::BG_WINDOW_ALT,
    );
    draw_titlebar(bytes, width, state.focused);
    draw_header(bytes, state);
    draw_list(bytes, state);
    draw_footer(bytes, state);

    rt::surface_present_buffer_slot(
        surface_handle,
        buffer_slot,
        0,
        0,
        state.width.min(BUFFER_WIDTH),
        state.height.min(BUFFER_HEIGHT),
    )
}

fn draw_titlebar(bytes: &mut [u8], width: usize, focused: bool) {
    let close_x = width as i32 - ui::WINDOW_BUTTON_RIGHT_MARGIN - ui::WINDOW_BUTTON_SIZE as i32;
    let minimize_x = close_x - ui::WINDOW_BUTTON_GAP - ui::WINDOW_BUTTON_SIZE as i32;
    let maximize_x = minimize_x - ui::WINDOW_BUTTON_GAP - ui::WINDOW_BUTTON_SIZE as i32;
    fill_rect(
        bytes,
        maximize_x.max(0) as usize,
        ui::WINDOW_BUTTON_TOP.max(0) as usize,
        ui::WINDOW_BUTTON_SIZE as usize,
        ui::WINDOW_BUTTON_SIZE as usize,
        ui::ACCENT,
    );
    fill_rect(
        bytes,
        minimize_x.max(0) as usize,
        ui::WINDOW_BUTTON_TOP.max(0) as usize,
        ui::WINDOW_BUTTON_SIZE as usize,
        ui::WINDOW_BUTTON_SIZE as usize,
        ui::TEXT_MUTED,
    );
    fill_rect(
        bytes,
        close_x.max(0) as usize,
        ui::WINDOW_BUTTON_TOP.max(0) as usize,
        ui::WINDOW_BUTTON_SIZE as usize,
        ui::WINDOW_BUTTON_SIZE as usize,
        ui::STATUS_WARN,
    );
    fill_rect(
        bytes,
        (maximize_x + 3).max(0) as usize,
        (ui::WINDOW_BUTTON_TOP + 3).max(0) as usize,
        6,
        6,
        ui::BG_PANEL,
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        minimize_x + 3,
        ui::WINDOW_BUTTON_TOP + 2,
        ui::BG_PANEL,
        "_",
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        close_x + 3,
        ui::WINDOW_BUTTON_TOP + 2,
        ui::BG_PANEL,
        "X",
    );
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, 10, 9, ui::TEXT_PRIMARY, "FILES");
}

fn draw_header(bytes: &mut [u8], state: &ExplorerState) {
    let mut path_line = FixedLogBuffer::<128>::new();
    let _ = write!(&mut path_line, "PATH ");
    if state.current_path_len == 0 {
        let _ = write!(&mut path_line, "/");
    } else if let Ok(path) = str::from_utf8(&state.current_path[..state.current_path_len]) {
        let _ = write!(&mut path_line, "/{}", path);
    } else {
        let _ = write!(&mut path_line, "/INVALID");
    }
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        LIST_X as i32,
        ui::TITLEBAR_HEIGHT as i32 + 10,
        ui::TEXT_PRIMARY,
        str::from_utf8(path_line.as_bytes()).unwrap_or("PATH /"),
    );
}

fn draw_list(bytes: &mut [u8], state: &ExplorerState) {
    let width = state.width.min(BUFFER_WIDTH) as usize;
    let height = state.height.min(BUFFER_HEIGHT) as usize;
    let list_height = height.saturating_sub(LIST_Y + LIST_BOTTOM_MARGIN);
    fill_rect(
        bytes,
        LIST_X,
        LIST_Y,
        width.saturating_sub(LIST_X * 2),
        list_height,
        ui::BG_WINDOW,
    );

    if state.load_failed {
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            LIST_X as i32 + 6,
            LIST_Y as i32 + 8,
            ui::STATUS_WARN,
            "LIST FAILED",
        );
        return;
    }

    if state.entry_count == 0 {
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            LIST_X as i32 + 6,
            LIST_Y as i32 + 8,
            ui::TEXT_MUTED,
            "EMPTY",
        );
        return;
    }

    let visible = visible_row_count(state);
    for row in 0..visible {
        let index = state.scroll_offset + row;
        if index >= state.entry_count {
            break;
        }
        let y = LIST_Y + row * ROW_HEIGHT;
        let selected = index == state.selected_index;
        if selected {
            fill_rect(
                bytes,
                LIST_X + 4,
                y + 1,
                width.saturating_sub(LIST_X * 2 + 8),
                ROW_HEIGHT.saturating_sub(1),
                ui::ACCENT_DIM,
            );
        }

        let entry = state.entries[index];
        let color = match entry.kind {
            EntryKind::Parent => ui::STATUS_WARN,
            EntryKind::Directory => ui::ACCENT,
            EntryKind::File => {
                if selected { ui::TEXT_PRIMARY } else { ui::TEXT_SECONDARY }
            }
        };
        draw_entry_label(bytes, entry, (LIST_X + 8) as i32, (y + 4) as i32, color);
    }
}

fn draw_footer(bytes: &mut [u8], state: &ExplorerState) {
    let height = state.height.min(BUFFER_HEIGHT) as usize;
    let footer_y = height.saturating_sub(14);
    let mut footer = FixedLogBuffer::<128>::new();
    if state.entry_count == 0 {
        let _ = write!(&mut footer, "0 entries");
    } else {
        let selected = state.entries[state.selected_index.min(state.entry_count - 1)];
        let _ = write!(
            &mut footer,
            "{} of {}  ",
            state.selected_index.min(state.entry_count - 1) + 1,
            state.entry_count,
        );
        push_selected_path(&mut footer, selected);
    }
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        LIST_X as i32,
        footer_y as i32,
        ui::TEXT_MUTED,
        str::from_utf8(footer.as_bytes()).unwrap_or("FILES"),
    );
}

fn fill_rect(bytes: &mut [u8], x: usize, y: usize, width: usize, height: usize, rgb: u32) {
    let end_x = (x + width).min(BUFFER_WIDTH as usize);
    let end_y = (y + height).min(BUFFER_HEIGHT as usize);
    for py in y..end_y {
        for px in x..end_x {
            rt::set_pixel_rgba8888(bytes, PIXEL_STRIDE, px, py, rgb);
        }
    }
}

fn visible_row_count(state: &ExplorerState) -> usize {
    let height = state.height.min(BUFFER_HEIGHT) as usize;
    height
        .saturating_sub(LIST_Y + LIST_BOTTOM_MARGIN)
        .checked_div(ROW_HEIGHT)
        .unwrap_or(0)
}

fn reopen_directory(state: &mut ExplorerState, storage_handle: rt::Handle) -> rt::Result<()> {
    if state.current_directory_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(state.current_directory_handle);
        state.current_directory_handle = rt::INVALID_HANDLE;
    }

    let prefix = str::from_utf8(&state.current_path[..state.current_path_len]).unwrap_or("");
    state.current_directory_handle = rt::storage_open_directory(storage_handle, prefix, false)?;
    Ok(())
}

fn reload_directory(state: &mut ExplorerState) -> rt::Result<()> {
    state.entry_count = 0;
    state.scroll_offset = 0;
    state.selected_index = 0;
    state.load_failed = false;

    if state.current_path_len != 0 {
        let parent_len = parent_path_bytes(&state.current_path[..state.current_path_len], &mut state.entries[0].path);
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

fn entry_name_bytes(entry: &ExplorerEntry) -> &[u8] {
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

fn draw_entry_label(
    bytes: &mut [u8],
    entry: ExplorerEntry,
    x: i32,
    y: i32,
    color: u32,
) {
    let mut label = FixedLogBuffer::<96>::new();
    if entry.kind == EntryKind::Parent {
        let _ = write!(&mut label, "UP   ..");
    } else {
        match entry.kind {
            EntryKind::Directory => {
                let _ = write!(&mut label, "DIR  ");
            }
            EntryKind::File => {
                let _ = write!(&mut label, "FILE ");
            }
            EntryKind::Parent => {}
        }
        if let Ok(name) = str::from_utf8(entry_name_bytes(&entry)) {
            let _ = write!(&mut label, "{name}");
            if entry.kind == EntryKind::Directory {
                let _ = write!(&mut label, "/");
            }
        } else {
            let _ = write!(&mut label, "INVALID");
        }
    }
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        x,
        y,
        color,
        str::from_utf8(label.as_bytes()).unwrap_or("INVALID"),
    );
}

fn push_selected_path(buffer: &mut FixedLogBuffer<128>, entry: ExplorerEntry) {
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

fn open_selected(state: &mut ExplorerState, storage_handle: rt::Handle) -> rt::Result<()> {
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

fn open_path_in_explorer(
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

fn navigate_parent(state: &mut ExplorerState) {
    let mut parent = [0u8; MAX_STORAGE_PATH];
    let len = parent_path_bytes(&state.current_path[..state.current_path_len], &mut parent);
    state.current_path[..len].copy_from_slice(&parent[..len]);
    state.current_path_len = len;
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

fn ensure_selected_visible(state: &mut ExplorerState) {
    let visible = visible_row_count(state).max(1);
    if state.selected_index < state.scroll_offset {
        state.scroll_offset = state.selected_index;
    } else if state.selected_index >= state.scroll_offset + visible {
        state.scroll_offset = state.selected_index + 1 - visible;
    }
}

fn clamp_view(state: &mut ExplorerState) {
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

fn scroll_up(state: &mut ExplorerState, amount: usize) {
    state.scroll_offset = state.scroll_offset.saturating_sub(amount);
    if state.selected_index > state.scroll_offset + visible_row_count(state).saturating_sub(1) {
        state.selected_index = state.scroll_offset;
    }
}

fn scroll_down(state: &mut ExplorerState, amount: usize) {
    let visible = visible_row_count(state).max(1);
    let max_scroll = state.entry_count.saturating_sub(visible);
    state.scroll_offset = (state.scroll_offset + amount).min(max_scroll);
    if state.selected_index < state.scroll_offset {
        state.selected_index = state.scroll_offset;
    }
}

fn pointer_action_from_word(value: u64) -> Option<AppPointerAction> {
    match value as u32 {
        x if x == AppPointerAction::Down as u32 => Some(AppPointerAction::Down),
        x if x == AppPointerAction::Move as u32 => Some(AppPointerAction::Move),
        x if x == AppPointerAction::Up as u32 => Some(AppPointerAction::Up),
        x if x == AppPointerAction::Scroll as u32 => Some(AppPointerAction::Scroll),
        _ => None,
    }
}

fn key_action_from_word(value: u64) -> Option<AppKeyAction> {
    match value as u32 {
        x if x == AppKeyAction::Down as u32 => Some(AppKeyAction::Down),
        x if x == AppKeyAction::Up as u32 => Some(AppKeyAction::Up),
        _ => None,
    }
}

fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
    let mut message = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut message) {
        Ok(()) if message.tag == ControlTag::Lifecycle as u32 && message.word_count > 0 => Ok(
            matches!(
                lifecycle_event_from_word(message.words[0]),
                LifecycleEvent::Restarting | LifecycleEvent::Stopped
            ),
        ),
        Ok(()) => Ok(false),
        Err(rt::Error::QueueEmpty) => Ok(false),
        Err(error) => Err(error),
    }
}

fn lifecycle_event_from_word(value: u64) -> LifecycleEvent {
    match value as u32 {
        x if x == LifecycleEvent::Starting as u32 => LifecycleEvent::Starting,
        x if x == LifecycleEvent::Ready as u32 => LifecycleEvent::Ready,
        x if x == LifecycleEvent::Failed as u32 => LifecycleEvent::Failed,
        x if x == LifecycleEvent::Stopped as u32 => LifecycleEvent::Stopped,
        _ => LifecycleEvent::Restarting,
    }
}

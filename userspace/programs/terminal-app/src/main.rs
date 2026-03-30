#![no_std]
#![no_main]

use core::{array, cell::UnsafeCell, fmt::Write};

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{AppControlTag, ConsoleTag, ControlTag, LifecycleEvent, RawMessage, TerminalTag};

const BUFFER_WIDTH: u32 = 1024;
const BUFFER_HEIGHT: u32 = 768;
const BUFFER_BYTES: usize = BUFFER_WIDTH as usize * BUFFER_HEIGHT as usize * 4;
const SURFACE_BUFFER_SLOTS: usize = 2;
const MAX_TABS: usize = 4;
const MAX_COLS: usize = 120;
const MAX_SCROLLBACK_LINES: usize = 256;
const CLIPBOARD_BYTES: usize = 1024;
const MAX_TITLE_BYTES: usize = 24;
const MAX_OSC_BYTES: usize = 64;
const CELL_WIDTH: usize = 6;
const CELL_HEIGHT: usize = 10;
const CONTENT_PADDING_X: usize = 10;
const CONTENT_PADDING_Y: usize = 8;
const TAB_STRIP_HEIGHT: usize = 18;
const TAB_WIDTH: usize = 100;
const KEY_1: u32 = 2;
const KEY_2: u32 = 3;
const KEY_3: u32 = 4;
const KEY_BACKSPACE: u32 = 14;
const KEY_TAB: u32 = 15;
const KEY_W: u32 = 17;
const KEY_T: u32 = 20;
const KEY_C: u32 = 46;
const KEY_V: u32 = 47;
const KEY_UP: u32 = 103;
const KEY_PAGE_UP: u32 = 104;
const KEY_LEFT: u32 = 105;
const KEY_RIGHT: u32 = 106;
const KEY_DOWN: u32 = 108;
const KEY_PAGE_DOWN: u32 = 109;
const MOD_CTRL: u32 = 1 << 2;
const MOD_SHIFT: u32 = 1 << 0;
const PIXEL_STRIDE: usize = BUFFER_WIDTH as usize;

const COLOR_DEFAULT: u8 = 0;
const CELL_FLAG_BOLD: u8 = 1 << 0;
const CELL_FLAG_INVERSE: u8 = 1 << 1;

#[derive(Clone, Copy)]
struct Cell {
    ch: u8,
    fg: u8,
    bg: u8,
    flags: u8,
}

impl Cell {
    const fn blank() -> Self {
        Self {
            ch: b' ',
            fg: COLOR_DEFAULT,
            bg: COLOR_DEFAULT,
            flags: 0,
        }
    }
}

struct GlobalCells(UnsafeCell<[[[Cell; MAX_COLS]; MAX_SCROLLBACK_LINES]; MAX_TABS]>);
struct GlobalWraps(UnsafeCell<[[bool; MAX_SCROLLBACK_LINES]; MAX_TABS]>);
struct ReflowCells(UnsafeCell<[[Cell; MAX_COLS]; MAX_SCROLLBACK_LINES]>);
struct ReflowWraps(UnsafeCell<[bool; MAX_SCROLLBACK_LINES]>);

unsafe impl Sync for GlobalCells {}
unsafe impl Sync for GlobalWraps {}
unsafe impl Sync for ReflowCells {}
unsafe impl Sync for ReflowWraps {}

impl GlobalCells {
    const fn new() -> Self {
        Self(UnsafeCell::new([[[Cell::blank(); MAX_COLS]; MAX_SCROLLBACK_LINES]; MAX_TABS]))
    }

    unsafe fn tab(&self, index: usize) -> &[[Cell; MAX_COLS]; MAX_SCROLLBACK_LINES] {
        unsafe { &(*self.0.get())[index] }
    }

    unsafe fn tab_mut(&self, index: usize) -> &mut [[Cell; MAX_COLS]; MAX_SCROLLBACK_LINES] {
        unsafe { &mut (*self.0.get())[index] }
    }
}

impl GlobalWraps {
    const fn new() -> Self {
        Self(UnsafeCell::new([[false; MAX_SCROLLBACK_LINES]; MAX_TABS]))
    }

    unsafe fn tab_mut(&self, index: usize) -> &mut [bool; MAX_SCROLLBACK_LINES] {
        unsafe { &mut (*self.0.get())[index] }
    }
}

impl ReflowCells {
    const fn new() -> Self {
        Self(UnsafeCell::new([[Cell::blank(); MAX_COLS]; MAX_SCROLLBACK_LINES]))
    }

    unsafe fn get(&self) -> &mut [[Cell; MAX_COLS]; MAX_SCROLLBACK_LINES] {
        unsafe { &mut *self.0.get() }
    }
}

impl ReflowWraps {
    const fn new() -> Self {
        Self(UnsafeCell::new([false; MAX_SCROLLBACK_LINES]))
    }

    unsafe fn get(&self) -> &mut [bool; MAX_SCROLLBACK_LINES] {
        unsafe { &mut *self.0.get() }
    }
}

static GRIDS: GlobalCells = GlobalCells::new();
static WRAPS: GlobalWraps = GlobalWraps::new();
static REFLOW_CELLS: ReflowCells = ReflowCells::new();
static REFLOW_WRAPS: ReflowWraps = ReflowWraps::new();

#[derive(Clone, Copy)]
struct Theme {
    name: &'static str,
    bg: u32,
    panel: u32,
    panel_alt: u32,
    fg: u32,
    muted: u32,
    selection: u32,
    ansi: [u32; 16],
}

const THEMES: [Theme; 3] = [
    Theme {
        name: "MIDNIGHT",
        bg: 0x0b1220,
        panel: 0x10151d,
        panel_alt: 0x122035,
        fg: 0xe6edf5,
        muted: 0x8fa4ba,
        selection: 0x23496f,
        ansi: [
            0x0b1220, 0xd05858, 0x65b35c, 0xd1af47, 0x5d8bd6, 0xb470d0, 0x57b8c4, 0xc7d3df,
            0x405469, 0xff8b8b, 0x8ce17f, 0xf4d46f, 0x89b4ff, 0xd7a7ff, 0x7de2ef, 0xf8fbff,
        ],
    },
    Theme {
        name: "PAPER",
        bg: 0xf2efe8,
        panel: 0xe7e0d6,
        panel_alt: 0xd8d0c3,
        fg: 0x1f242a,
        muted: 0x61686f,
        selection: 0xbfd7ff,
        ansi: [
            0xf2efe8, 0xb53c3c, 0x3f8d3c, 0xa76d10, 0x2e63ad, 0x8a47a6, 0x287d82, 0x3f474f,
            0xa89f92, 0xd95a5a, 0x52ad4f, 0xc88d1f, 0x447fd4, 0xa663c4, 0x3b9fa5, 0x101316,
        ],
    },
    Theme {
        name: "AMBER",
        bg: 0x140f08,
        panel: 0x20170d,
        panel_alt: 0x2a1d0d,
        fg: 0xf0d0a2,
        muted: 0xb59363,
        selection: 0x5b3a12,
        ansi: [
            0x140f08, 0xc35b4c, 0x9ea95b, 0xe0a14a, 0x7e90c4, 0xb986c8, 0x6ca2b8, 0xf0d0a2,
            0x6b5135, 0xe48a73, 0xc6d47d, 0xffc46b, 0x9fb3e8, 0xd3a5df, 0x88bfd4, 0xffefd0,
        ],
    },
];

#[derive(Clone, Copy)]
struct TerminalState {
    width: u32,
    height: u32,
    focused: bool,
    terminal_handle: rt::Handle,
    clipboard_handle: rt::Handle,
    columns: usize,
    rows: usize,
    active_tab: usize,
    theme_index: usize,
    tabs: [TerminalTab; MAX_TABS],
    selection: Option<Selection>,
    clipboard: [u8; CLIPBOARD_BYTES],
    clipboard_len: usize,
}

#[derive(Clone, Copy)]
struct TerminalTab {
    occupied: bool,
    session_handle: rt::Handle,
    session_id: u32,
    line_count: usize,
    cursor_line: usize,
    cursor_col: usize,
    saved_cursor_line: usize,
    saved_cursor_col: usize,
    scroll_offset: usize,
    parse_state: ParseState,
    csi_params: [usize; 8],
    csi_count: usize,
    csi_private: bool,
    osc_bytes: [u8; MAX_OSC_BYTES],
    osc_len: usize,
    osc_esc_pending: bool,
    title: [u8; MAX_TITLE_BYTES],
    title_len: usize,
    current_fg: u8,
    current_bg: u8,
    current_flags: u8,
    cursor_visible: bool,
}

impl TerminalTab {
    const fn empty() -> Self {
        Self {
            occupied: false,
            session_handle: rt::INVALID_HANDLE,
            session_id: 0,
            line_count: 1,
            cursor_line: 0,
            cursor_col: 0,
            saved_cursor_line: 0,
            saved_cursor_col: 0,
            scroll_offset: 0,
            parse_state: ParseState::Ground,
            csi_params: [0; 8],
            csi_count: 0,
            csi_private: false,
            osc_bytes: [0; MAX_OSC_BYTES],
            osc_len: 0,
            osc_esc_pending: false,
            title: [0; MAX_TITLE_BYTES],
            title_len: 0,
            current_fg: COLOR_DEFAULT,
            current_bg: COLOR_DEFAULT,
            current_flags: 0,
            cursor_visible: true,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ParseState {
    Ground,
    Esc,
    Csi,
    Osc,
}

#[derive(Clone, Copy)]
struct CellPos {
    line: usize,
    col: usize,
}

#[derive(Clone, Copy)]
struct Selection {
    anchor: CellPos,
    focus: CellPos,
    dragging: bool,
}

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xfa01;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 3 || startup.word_count < 4 {
        return 0xfa02;
    }

    let surface_handle = startup.handles[0];
    let control_handle = startup.handles[1];
    let terminal_handle = startup.handles[2];
    let clipboard_handle = if startup.handle_count > 3 {
        startup.handles[3]
    } else {
        rt::INVALID_HANDLE
    };
    let mut width = startup.words[1] as u32;
    let mut height = startup.words[2] as u32;
    let mut focused = startup.words[3] != 0;

    let mut buffer_handles = [rt::INVALID_HANDLE; SURFACE_BUFFER_SLOTS];
    let mut mapped_buffers: [Option<rt::MappedMemory>; SURFACE_BUFFER_SLOTS] =
        array::from_fn(|_| None);
    for slot in 0..SURFACE_BUFFER_SLOTS {
        let buffer_handle = match rt::memory_create(BUFFER_BYTES, true) {
            Ok(handle) => handle,
            Err(_) => return 0xfa03,
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
            return 0xfa04;
        }
        let mapped_buffer = match rt::MappedMemory::map(buffer_handle, BUFFER_BYTES, true) {
            Ok(buffer) => buffer,
            Err(_) => {
                let _ = rt::handle_close(buffer_handle);
                return 0xfa0a;
            }
        };
        buffer_handles[slot] = buffer_handle;
        mapped_buffers[slot] = Some(mapped_buffer);
    }
    let mut front_buffer_slot = 0usize;

    clear_all_tabs();
    let mut state = TerminalState {
        width,
        height,
        focused,
        terminal_handle,
        clipboard_handle,
        columns: 0,
        rows: 0,
        active_tab: 0,
        theme_index: 0,
        tabs: [TerminalTab::empty(); MAX_TABS],
        selection: None,
        clipboard: [0; CLIPBOARD_BYTES],
        clipboard_len: 0,
    };
    recompute_layout(&mut state);
    if open_new_tab(&mut state).is_err() {
        return 0xfa05;
    }
    let _ = render(
        surface_handle,
        front_buffer_slot as u32,
        mapped_buffers[front_buffer_slot].as_mut().unwrap(),
        &state,
    );

    loop {
        let mut did_work = false;
        let mut changed = false;

        match poll_lifecycle(bootstrap) {
            Ok(true) => break,
            Ok(false) => {}
            Err(_) => return 0xfa06,
        }

        match poll_control(control_handle, &mut state, &mut width, &mut height, &mut focused) {
            Ok((ControlFlow::Continue, control_changed, control_worked)) => {
                changed |= control_changed;
                did_work |= control_worked;
            }
            Ok((ControlFlow::Exit, _, _)) => break,
            Err(_) => return 0xfa07,
        }

        if width != state.width || height != state.height || focused != state.focused {
            let old_width = state.width;
            let old_height = state.height;
            let old_columns = state.columns;
            let old_rows = state.rows;
            state.width = width;
            state.height = height;
            state.focused = focused;
            recompute_layout(&mut state);
            if state.columns != old_columns {
                for tab_index in 0..MAX_TABS {
                    if state.tabs[tab_index].occupied {
                        reflow_tab(&mut state.tabs[tab_index], tab_index, old_columns, state.columns);
                    }
                }
            }
            if state.columns != old_columns
                || state.rows != old_rows
                || state.width != old_width
                || state.height != old_height
            {
                for tab in state.tabs.iter().copied().filter(|tab| tab.occupied) {
                    let _ = rt::terminal_session_resize(
                        tab.session_handle,
                        state.columns as u32,
                        state.rows as u32,
                        state.width,
                        state.height,
                    );
                }
            }
            changed = true;
        }

        let mut data = [0u8; (rt::IPC_MAX_WORDS - 1) * 8];
        for tab_index in 0..MAX_TABS {
            if !state.tabs[tab_index].occupied {
                continue;
            }
            loop {
                match receive_terminal_message(state.tabs[tab_index].session_handle, &mut data) {
                    Ok(Some(TerminalMessage::Output(len))) => {
                        apply_output(&mut state, tab_index, &data[..len]);
                        changed = true;
                        did_work = true;
                    }
                    Ok(Some(TerminalMessage::Closed)) => {
                        close_tab(&mut state, tab_index);
                        changed = true;
                        did_work = true;
                        if active_tab_count(&state) == 0 {
                            let _ = open_new_tab(&mut state);
                        }
                        break;
                    }
                    Ok(None) => break,
                    Err(rt::Error::QueueEmpty) => break,
                    Err(_) => return 0xfa08,
                }
            }
        }

        if changed {
            front_buffer_slot = (front_buffer_slot + 1) % SURFACE_BUFFER_SLOTS;
            let _ = render(
                surface_handle,
                front_buffer_slot as u32,
                mapped_buffers[front_buffer_slot].as_mut().unwrap(),
                &state,
            );
        }
        if did_work {
            continue;
        }

        if rt::yield_current().is_err() {
            return 0xfa09;
        }
    }

    for tab in state.tabs.iter().copied().filter(|tab| tab.occupied) {
        let _ = rt::terminal_session_close(tab.session_handle);
        let _ = rt::handle_close(tab.session_handle);
    }
    for handle in buffer_handles {
        if handle != rt::INVALID_HANDLE {
            let _ = rt::handle_close(handle);
        }
    }
    0
}

enum ControlFlow {
    Continue,
    Exit,
}

enum TerminalMessage {
    Output(usize),
    Closed,
}

fn poll_control(
    control_handle: rt::Handle,
    state: &mut TerminalState,
    width: &mut u32,
    height: &mut u32,
    focused: &mut bool,
) -> rt::Result<(ControlFlow, bool, bool)> {
    let mut changed = false;
    let mut did_work = false;
    loop {
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(control_handle, &mut message) {
            Ok(()) if message.tag == AppControlTag::FocusChanged as u32 && message.word_count > 0 => {
                *focused = message.words[0] != 0;
                changed = true;
                did_work = true;
            }
            Ok(()) if message.tag == AppControlTag::Resize as u32 && message.word_count >= 2 => {
                *width = message.words[0] as u32;
                *height = message.words[1] as u32;
                changed = true;
                did_work = true;
            }
            Ok(()) if message.tag == AppControlTag::Close as u32 => return Ok((ControlFlow::Exit, false, true)),
            Ok(()) if message.tag == AppControlTag::Pointer as u32 && message.word_count >= 5 => {
                did_work = true;
                let action = app_pointer_action_from_word(message.words[0]);
                let x = message.words[1] as i64 as i32;
                let y = message.words[2] as i64 as i32;
                let detail = message.words[4] as i64 as i32;
                match action {
                    Some(rt::AppPointerAction::Down) => changed |= handle_pointer_down(state, x, y),
                    Some(rt::AppPointerAction::Move) => changed |= handle_pointer_move(state, x, y),
                    Some(rt::AppPointerAction::Up) => changed |= handle_pointer_up(state, x, y),
                    Some(rt::AppPointerAction::Scroll) => {
                        handle_pointer_scroll(state, detail);
                        changed = true;
                    }
                    _ => {}
                }
            }
            Ok(()) if message.tag == AppControlTag::Text as u32 && message.word_count > 0 => {
                if let Some(ch) = core::char::from_u32(message.words[0] as u32) {
                    state.selection = None;
                    if let Some(tab) = active_tab_mut(state) {
                        let mut bytes = [0u8; 4];
                        let encoded = ch.encode_utf8(&mut bytes);
                        tab.scroll_offset = 0;
                        let _ = rt::terminal_session_send_input(tab.session_handle, encoded.as_bytes());
                        changed = true;
                        did_work = true;
                    }
                }
            }
            Ok(()) if message.tag == AppControlTag::Key as u32 && message.word_count >= 2 => {
                did_work = true;
                if message.words[0] as u32 == rt::AppKeyAction::Down as u32 {
                    let key_code = message.words[1] as u32;
                    let modifiers = message.words.get(2).copied().unwrap_or(0) as u32;
                    changed |= handle_key_down(state, key_code, modifiers)?;
                }
            }
            Ok(()) => {}
            Err(rt::Error::QueueEmpty) => break,
            Err(error) => return Err(error),
        }
    }
    Ok((ControlFlow::Continue, changed, did_work))
}

fn handle_key_down(state: &mut TerminalState, key_code: u32, modifiers: u32) -> rt::Result<bool> {
    if modifiers & MOD_CTRL != 0 && modifiers & MOD_SHIFT != 0 {
        match key_code {
            KEY_T => {
                open_new_tab(state)?;
                return Ok(true);
            }
            KEY_W => {
                close_active_tab(state);
                return Ok(true);
            }
            KEY_C => {
                copy_selection(state);
                return Ok(true);
            }
            KEY_V => {
                let _ = paste_clipboard(state);
                return Ok(true);
            }
            KEY_1 => {
                state.theme_index = 0;
                return Ok(true);
            }
            KEY_2 => {
                state.theme_index = 1;
                return Ok(true);
            }
            KEY_3 => {
                state.theme_index = 2;
                return Ok(true);
            }
            _ => {}
        }
    }

    if modifiers & MOD_CTRL != 0 && key_code == KEY_TAB {
        if modifiers & MOD_SHIFT != 0 {
            focus_previous_tab(state);
        } else {
            focus_next_tab(state);
        }
        return Ok(true);
    }
    if modifiers & MOD_CTRL != 0 && key_code == KEY_C {
        state.selection = None;
        if let Some(tab) = active_tab_mut(state) {
            tab.scroll_offset = 0;
            rt::terminal_session_send_input(tab.session_handle, &[0x03])?;
            return Ok(true);
        }
    }
    if key_code == KEY_PAGE_UP || (modifiers & MOD_SHIFT != 0 && key_code == KEY_UP) {
        let rows = state.rows;
        if let Some(tab) = active_tab_mut(state) {
            scroll_up_view(tab, if key_code == KEY_PAGE_UP { rows.saturating_sub(1).max(1) } else { 1 }, rows);
            return Ok(true);
        }
    }
    if key_code == KEY_PAGE_DOWN || (modifiers & MOD_SHIFT != 0 && key_code == KEY_DOWN) {
        let rows = state.rows;
        if let Some(tab) = active_tab_mut(state) {
            scroll_down_view(tab, if key_code == KEY_PAGE_DOWN { rows.saturating_sub(1).max(1) } else { 1 });
            return Ok(true);
        }
    }

    state.selection = None;
    let Some(tab) = active_tab_mut(state) else {
        return Ok(false);
    };
    tab.scroll_offset = 0;
    match key_code {
        KEY_BACKSPACE => rt::terminal_session_send_input(tab.session_handle, &[0x7f])?,
        KEY_UP => rt::terminal_session_send_input(tab.session_handle, b"\x1b[A")?,
        KEY_DOWN => rt::terminal_session_send_input(tab.session_handle, b"\x1b[B")?,
        KEY_RIGHT => rt::terminal_session_send_input(tab.session_handle, b"\x1b[C")?,
        KEY_LEFT => rt::terminal_session_send_input(tab.session_handle, b"\x1b[D")?,
        _ => return Ok(false),
    }
    Ok(true)
}

fn app_pointer_action_from_word(value: u64) -> Option<rt::AppPointerAction> {
    match value as u32 {
        x if x == rt::AppPointerAction::Down as u32 => Some(rt::AppPointerAction::Down),
        x if x == rt::AppPointerAction::Move as u32 => Some(rt::AppPointerAction::Move),
        x if x == rt::AppPointerAction::Up as u32 => Some(rt::AppPointerAction::Up),
        x if x == rt::AppPointerAction::Scroll as u32 => Some(rt::AppPointerAction::Scroll),
        _ => None,
    }
}

fn handle_pointer_down(state: &mut TerminalState, x: i32, y: i32) -> bool {
    if let Some(tab_index) = tab_strip_hit_index(x, y) {
        if tab_index != state.active_tab && state.tabs[tab_index].occupied {
            state.active_tab = tab_index;
            state.selection = None;
            return true;
        }
        return false;
    }
    let Some(cell) = pointer_to_cell(state, x, y) else {
        state.selection = None;
        return false;
    };
    state.selection = Some(Selection {
        anchor: cell,
        focus: cell,
        dragging: true,
    });
    true
}

fn handle_pointer_move(state: &mut TerminalState, x: i32, y: i32) -> bool {
    let Some(mut selection) = state.selection else {
        return false;
    };
    if !selection.dragging {
        return false;
    }
    let Some(cell) = pointer_to_cell(state, x, y) else {
        return false;
    };
    selection.focus = cell;
    state.selection = Some(selection);
    true
}

fn handle_pointer_up(state: &mut TerminalState, x: i32, y: i32) -> bool {
    let Some(mut selection) = state.selection else {
        return false;
    };
    if let Some(cell) = pointer_to_cell(state, x, y) {
        selection.focus = cell;
    }
    selection.dragging = false;
    state.selection = Some(selection);
    copy_selection(state);
    true
}

fn handle_pointer_scroll(state: &mut TerminalState, delta_y: i32) {
    let rows = state.rows;
    if let Some(tab) = active_tab_mut(state) {
        if delta_y > 0 {
            scroll_up_view(tab, delta_y as usize, rows);
        } else if delta_y < 0 {
            scroll_down_view(tab, (-delta_y) as usize);
        }
    }
}

fn receive_terminal_message(
    session_handle: rt::Handle,
    buffer: &mut [u8],
) -> rt::Result<Option<TerminalMessage>> {
    let mut message = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(session_handle, &mut message) {
        Ok(()) if message.tag == ConsoleTag::SessionWriteText as u32 && message.word_count >= 1 => {
            let len = message.words[0] as usize;
            unpack_bytes(&message.words[1..message.word_count as usize], len, buffer)?;
            Ok(Some(TerminalMessage::Output(len)))
        }
        Ok(()) if message.tag == TerminalTag::SessionClosed as u32 => Ok(Some(TerminalMessage::Closed)),
        Ok(()) => Err(rt::Error::InvalidArgument),
        Err(rt::Error::QueueEmpty) => Ok(None),
        Err(error) => Err(error),
    }
}

fn recompute_layout(state: &mut TerminalState) {
    let content_width = state.width.saturating_sub((CONTENT_PADDING_X as u32) * 2);
    let content_height = state
        .height
        .saturating_sub(ui::TITLEBAR_HEIGHT + TAB_STRIP_HEIGHT as u32 + (CONTENT_PADDING_Y as u32) * 2 + 4);
    state.columns = ((content_width as usize) / CELL_WIDTH).clamp(20, MAX_COLS);
    state.rows = ((content_height as usize) / CELL_HEIGHT).clamp(8, MAX_SCROLLBACK_LINES);
    for tab in state.tabs.iter_mut().filter(|tab| tab.occupied) {
        clamp_scroll_offset(tab, state.rows);
        if tab.cursor_col >= state.columns {
            tab.cursor_col = state.columns.saturating_sub(1);
        }
    }
}

fn clear_all_tabs() {
    for tab_index in 0..MAX_TABS {
        clear_tab_grid(tab_index);
    }
}

fn clear_tab_grid(tab_index: usize) {
    let lines = unsafe { GRIDS.tab_mut(tab_index) };
    let wraps = unsafe { WRAPS.tab_mut(tab_index) };
    for row in lines.iter_mut() {
        for cell in row.iter_mut() {
            *cell = Cell::blank();
        }
    }
    wraps.fill(false);
}

fn open_new_tab(state: &mut TerminalState) -> rt::Result<()> {
    let Some(index) = state.tabs.iter().position(|tab| !tab.occupied) else {
        return Err(rt::Error::CapacityExceeded);
    };
    let (_, session_handle, _, _) = rt::terminal_session_open(state.terminal_handle)?;
    let _ = rt::terminal_session_resize(
        session_handle,
        state.columns as u32,
        state.rows as u32,
        state.width,
        state.height,
    );
    clear_tab_grid(index);
    state.tabs[index] = TerminalTab {
        occupied: true,
        session_handle,
        session_id: (index + 1) as u32,
        line_count: 1,
        cursor_line: 0,
        cursor_col: 0,
        saved_cursor_line: 0,
        saved_cursor_col: 0,
        scroll_offset: 0,
        parse_state: ParseState::Ground,
        csi_params: [0; 8],
        csi_count: 0,
        csi_private: false,
        osc_bytes: [0; MAX_OSC_BYTES],
        osc_len: 0,
        osc_esc_pending: false,
        title: [0; MAX_TITLE_BYTES],
        title_len: 0,
        current_fg: COLOR_DEFAULT,
        current_bg: COLOR_DEFAULT,
        current_flags: 0,
        cursor_visible: true,
    };
    state.active_tab = index;
    state.selection = None;
    Ok(())
}

fn close_active_tab(state: &mut TerminalState) {
    let active = state.active_tab;
    if active_tab_count(state) <= 1 {
        return;
    }
    close_tab(state, active);
    if !state.tabs[state.active_tab].occupied {
        focus_next_tab(state);
    }
    state.selection = None;
}

fn close_tab(state: &mut TerminalState, tab_index: usize) {
    let tab = state.tabs[tab_index];
    if !tab.occupied {
        return;
    }
    let _ = rt::terminal_session_close(tab.session_handle);
    let _ = rt::handle_close(tab.session_handle);
    state.tabs[tab_index] = TerminalTab::empty();
    clear_tab_grid(tab_index);
    if state.active_tab == tab_index {
        focus_next_tab(state);
    }
}

fn active_tab_count(state: &TerminalState) -> usize {
    state.tabs.iter().filter(|tab| tab.occupied).count()
}

fn focus_next_tab(state: &mut TerminalState) {
    for offset in 1..=MAX_TABS {
        let index = (state.active_tab + offset) % MAX_TABS;
        if state.tabs[index].occupied {
            state.active_tab = index;
            state.selection = None;
            return;
        }
    }
}

fn focus_previous_tab(state: &mut TerminalState) {
    for offset in 1..=MAX_TABS {
        let index = (state.active_tab + MAX_TABS - offset) % MAX_TABS;
        if state.tabs[index].occupied {
            state.active_tab = index;
            state.selection = None;
            return;
        }
    }
}

fn active_tab_mut(state: &mut TerminalState) -> Option<&mut TerminalTab> {
    state.tabs.get_mut(state.active_tab).filter(|tab| tab.occupied)
}

fn active_tab_ref(state: &TerminalState) -> Option<&TerminalTab> {
    state.tabs.get(state.active_tab).filter(|tab| tab.occupied)
}

fn apply_output(state: &mut TerminalState, tab_index: usize, bytes: &[u8]) {
    let columns = state.columns;
    let tab = &mut state.tabs[tab_index];
    for byte in bytes.iter().copied() {
        match tab.parse_state {
            ParseState::Ground => match byte {
                0x1b => {
                    tab.parse_state = ParseState::Esc;
                    reset_escape(tab);
                }
                b'\r' => tab.cursor_col = 0,
                b'\n' => explicit_new_line(tab, tab_index),
                b'\t' => advance_tab_stop(tab, columns, tab_index),
                0x08 => tab.cursor_col = tab.cursor_col.saturating_sub(1),
                0x20..=0x7e => put_char(tab, columns, tab_index, byte),
                _ => {}
            },
            ParseState::Esc => match byte {
                b'[' => {
                    tab.parse_state = ParseState::Csi;
                    reset_escape(tab);
                    tab.csi_count = 1;
                }
                b']' => {
                    tab.parse_state = ParseState::Osc;
                    tab.osc_len = 0;
                    tab.osc_esc_pending = false;
                }
                b'7' => {
                    tab.saved_cursor_line = tab.cursor_line;
                    tab.saved_cursor_col = tab.cursor_col;
                    tab.parse_state = ParseState::Ground;
                }
                b'8' => {
                    tab.cursor_line = tab.saved_cursor_line.min(MAX_SCROLLBACK_LINES - 1);
                    tab.cursor_col = tab.saved_cursor_col.min(columns.saturating_sub(1));
                    tab.parse_state = ParseState::Ground;
                }
                b'D' => {
                    explicit_new_line(tab, tab_index);
                    tab.parse_state = ParseState::Ground;
                }
                b'E' => {
                    tab.cursor_col = 0;
                    explicit_new_line(tab, tab_index);
                    tab.parse_state = ParseState::Ground;
                }
                b'c' => {
                    reset_terminal_tab(tab, tab_index);
                    tab.parse_state = ParseState::Ground;
                }
                _ => tab.parse_state = ParseState::Ground,
            },
            ParseState::Csi => {
                if byte == b'?' && tab.csi_count == 1 && tab.csi_params[0] == 0 {
                    tab.csi_private = true;
                    continue;
                }
                if byte.is_ascii_digit() {
                    let index = tab.csi_count.saturating_sub(1).min(tab.csi_params.len() - 1);
                    tab.csi_params[index] =
                        tab.csi_params[index].saturating_mul(10) + (byte - b'0') as usize;
                    continue;
                }
                if byte == b';' {
                    if tab.csi_count < tab.csi_params.len() {
                        tab.csi_count += 1;
                    }
                    continue;
                }
                apply_csi(tab, columns, tab_index, byte);
                tab.parse_state = ParseState::Ground;
                reset_escape(tab);
            }
            ParseState::Osc => {
                if tab.osc_esc_pending {
                    if byte == b'\\' {
                        finish_osc(tab);
                        tab.parse_state = ParseState::Ground;
                        tab.osc_esc_pending = false;
                        continue;
                    }
                    tab.osc_esc_pending = false;
                }
                match byte {
                    0x07 => {
                        finish_osc(tab);
                        tab.parse_state = ParseState::Ground;
                    }
                    0x1b => tab.osc_esc_pending = true,
                    _ if tab.osc_len < tab.osc_bytes.len() => {
                        tab.osc_bytes[tab.osc_len] = byte;
                        tab.osc_len += 1;
                    }
                    _ => {}
                }
            }
        }
    }
    clamp_scroll_offset(tab, state.rows);
}

fn reset_escape(tab: &mut TerminalTab) {
    tab.csi_params = [0; 8];
    tab.csi_count = 0;
    tab.csi_private = false;
}

fn finish_osc(tab: &mut TerminalTab) {
    let bytes = &tab.osc_bytes[..tab.osc_len];
    let Some(separator) = bytes.iter().position(|byte| *byte == b';') else {
        tab.osc_len = 0;
        return;
    };
    let command = &bytes[..separator];
    if command != b"0" && command != b"2" {
        tab.osc_len = 0;
        return;
    }
    let title = &bytes[separator + 1..];
    let mut copied = 0usize;
    for byte in title.iter().copied() {
        if copied >= tab.title.len() {
            break;
        }
        if byte.is_ascii_graphic() || byte == b' ' {
            tab.title[copied] = byte;
            copied += 1;
        }
    }
    tab.title_len = copied;
    tab.osc_len = 0;
}

fn reset_terminal_tab(tab: &mut TerminalTab, tab_index: usize) {
    clear_tab_grid(tab_index);
    tab.line_count = 1;
    tab.cursor_line = 0;
    tab.cursor_col = 0;
    tab.saved_cursor_line = 0;
    tab.saved_cursor_col = 0;
    tab.scroll_offset = 0;
    tab.current_fg = COLOR_DEFAULT;
    tab.current_bg = COLOR_DEFAULT;
    tab.current_flags = 0;
    tab.cursor_visible = true;
}

fn apply_csi(tab: &mut TerminalTab, columns: usize, tab_index: usize, opcode: u8) {
    let param0 = csi_param(tab, 0, 1);
    let param1 = csi_param(tab, 1, 1);
    match opcode {
        b'A' => tab.cursor_line = tab.cursor_line.saturating_sub(param0),
        b'B' => tab.cursor_line = (tab.cursor_line + param0).min(MAX_SCROLLBACK_LINES.saturating_sub(1)),
        b'C' => tab.cursor_col = (tab.cursor_col + param0).min(columns.saturating_sub(1)),
        b'D' => tab.cursor_col = tab.cursor_col.saturating_sub(param0),
        b'G' => tab.cursor_col = param0.saturating_sub(1).min(columns.saturating_sub(1)),
        b'H' | b'f' => {
            tab.cursor_line = param0.saturating_sub(1).min(MAX_SCROLLBACK_LINES.saturating_sub(1));
            tab.cursor_col = param1.saturating_sub(1).min(columns.saturating_sub(1));
        }
        b'J' => clear_display(tab, tab_index, columns, csi_param(tab, 0, 0)),
        b'K' => clear_line_mode(tab, columns, tab_index, csi_param(tab, 0, 0)),
        b'X' => erase_chars(tab, columns, tab_index, param0),
        b'P' => delete_chars(tab, columns, tab_index, param0),
        b'@' => insert_blank_chars(tab, columns, tab_index, param0),
        b'L' => insert_lines(tab, tab_index, param0),
        b'M' => delete_lines(tab, tab_index, param0),
        b'm' => apply_sgr(tab),
        b's' => {
            tab.saved_cursor_line = tab.cursor_line;
            tab.saved_cursor_col = tab.cursor_col;
        }
        b'u' => {
            tab.cursor_line = tab.saved_cursor_line.min(MAX_SCROLLBACK_LINES - 1);
            tab.cursor_col = tab.saved_cursor_col.min(columns.saturating_sub(1));
        }
        b'h' if tab.csi_private && csi_param(tab, 0, 0) == 25 => tab.cursor_visible = true,
        b'l' if tab.csi_private && csi_param(tab, 0, 0) == 25 => tab.cursor_visible = false,
        _ => {}
    }
}

fn apply_sgr(tab: &mut TerminalTab) {
    let count = tab.csi_count.max(1);
    for index in 0..count.min(tab.csi_params.len()) {
        let param = tab.csi_params[index] as u8;
        match param {
            0 => {
                tab.current_fg = COLOR_DEFAULT;
                tab.current_bg = COLOR_DEFAULT;
                tab.current_flags = 0;
            }
            1 => tab.current_flags |= CELL_FLAG_BOLD,
            22 => tab.current_flags &= !CELL_FLAG_BOLD,
            7 => tab.current_flags |= CELL_FLAG_INVERSE,
            27 => tab.current_flags &= !CELL_FLAG_INVERSE,
            30..=37 => tab.current_fg = 1 + (param - 30),
            39 => tab.current_fg = COLOR_DEFAULT,
            40..=47 => tab.current_bg = 1 + (param - 40),
            49 => tab.current_bg = COLOR_DEFAULT,
            90..=97 => tab.current_fg = 9 + (param - 90),
            100..=107 => tab.current_bg = 9 + (param - 100),
            _ => {}
        }
    }
}

fn csi_param(tab: &TerminalTab, index: usize, default: usize) -> usize {
    let value = tab.csi_params.get(index).copied().unwrap_or(0);
    if value == 0 { default } else { value }
}

fn put_char(tab: &mut TerminalTab, columns: usize, tab_index: usize, byte: u8) {
    if tab.cursor_col >= columns {
        wrap_to_next_line(tab, tab_index);
    }
    let lines = unsafe { GRIDS.tab_mut(tab_index) };
    if tab.cursor_line >= MAX_SCROLLBACK_LINES {
        scroll_grid(lines, unsafe { WRAPS.tab_mut(tab_index) });
        tab.cursor_line = MAX_SCROLLBACK_LINES - 1;
    }
    lines[tab.cursor_line][tab.cursor_col] = Cell {
        ch: byte,
        fg: tab.current_fg,
        bg: tab.current_bg,
        flags: tab.current_flags,
    };
    tab.cursor_col += 1;
}

fn advance_tab_stop(tab: &mut TerminalTab, columns: usize, tab_index: usize) {
    let next = ((tab.cursor_col / 8) + 1) * 8;
    while tab.cursor_col < next.min(columns) {
        put_char(tab, columns, tab_index, b' ');
    }
}

fn explicit_new_line(tab: &mut TerminalTab, tab_index: usize) {
    advance_line(tab, tab_index, false);
}

fn wrap_to_next_line(tab: &mut TerminalTab, tab_index: usize) {
    advance_line(tab, tab_index, true);
}

fn advance_line(tab: &mut TerminalTab, tab_index: usize, wrapped: bool) {
    (unsafe { WRAPS.tab_mut(tab_index) })[tab.cursor_line.min(MAX_SCROLLBACK_LINES - 1)] = wrapped;
    tab.cursor_col = 0;
    tab.cursor_line += 1;
    if tab.line_count < tab.cursor_line + 1 {
        tab.line_count = tab.cursor_line + 1;
    }
    if tab.cursor_line >= MAX_SCROLLBACK_LINES {
        let lines = unsafe { GRIDS.tab_mut(tab_index) };
        let wraps = unsafe { WRAPS.tab_mut(tab_index) };
        scroll_grid(lines, wraps);
        tab.cursor_line = MAX_SCROLLBACK_LINES - 1;
        tab.line_count = MAX_SCROLLBACK_LINES;
        wraps[MAX_SCROLLBACK_LINES - 1] = false;
    }
}

fn clear_display(tab: &mut TerminalTab, tab_index: usize, columns: usize, mode: usize) {
    let lines = unsafe { GRIDS.tab_mut(tab_index) };
    let wraps = unsafe { WRAPS.tab_mut(tab_index) };
    match mode {
        1 => {
            for row in 0..tab.cursor_line {
                for cell in lines[row].iter_mut().take(columns) {
                    *cell = Cell::blank();
                }
                wraps[row] = false;
            }
            for cell in lines[tab.cursor_line.min(MAX_SCROLLBACK_LINES - 1)]
                .iter_mut()
                .take(tab.cursor_col.saturating_add(1).min(columns))
            {
                *cell = Cell::blank();
            }
        }
        2 => {
            for row in lines.iter_mut() {
                for cell in row.iter_mut().take(columns) {
                    *cell = Cell::blank();
                }
            }
            wraps.fill(false);
            tab.cursor_line = 0;
            tab.cursor_col = 0;
            tab.line_count = 1;
            tab.scroll_offset = 0;
        }
        _ => {
            clear_line_mode(tab, columns, tab_index, 0);
            for row in tab.cursor_line + 1..tab.line_count.min(MAX_SCROLLBACK_LINES) {
                for cell in lines[row].iter_mut().take(columns) {
                    *cell = Cell::blank();
                }
                wraps[row] = false;
            }
        }
    }
}

fn clear_line_mode(tab: &mut TerminalTab, columns: usize, tab_index: usize, mode: usize) {
    let lines = unsafe { GRIDS.tab_mut(tab_index) };
    let row = &mut lines[tab.cursor_line.min(MAX_SCROLLBACK_LINES - 1)];
    match mode {
        1 => {
            for cell in row.iter_mut().take(tab.cursor_col.saturating_add(1).min(columns)) {
                *cell = Cell::blank();
            }
        }
        2 => {
            for cell in row.iter_mut().take(columns) {
                *cell = Cell::blank();
            }
        }
        _ => {
            for cell in row.iter_mut().take(columns).skip(tab.cursor_col) {
                *cell = Cell::blank();
            }
        }
    }
}

fn erase_chars(tab: &mut TerminalTab, columns: usize, tab_index: usize, count: usize) {
    let row = &mut unsafe { GRIDS.tab_mut(tab_index) }[tab.cursor_line.min(MAX_SCROLLBACK_LINES - 1)];
    let end = (tab.cursor_col + count).min(columns);
    for cell in row.iter_mut().take(end).skip(tab.cursor_col) {
        *cell = Cell::blank();
    }
}

fn delete_chars(tab: &mut TerminalTab, columns: usize, tab_index: usize, count: usize) {
    let row = &mut unsafe { GRIDS.tab_mut(tab_index) }[tab.cursor_line.min(MAX_SCROLLBACK_LINES - 1)];
    let start = tab.cursor_col.min(columns);
    let count = count.min(columns.saturating_sub(start));
    for col in start..columns {
        let source = col.saturating_add(count);
        row[col] = if source < columns { row[source] } else { Cell::blank() };
    }
}

fn insert_blank_chars(tab: &mut TerminalTab, columns: usize, tab_index: usize, count: usize) {
    let row = &mut unsafe { GRIDS.tab_mut(tab_index) }[tab.cursor_line.min(MAX_SCROLLBACK_LINES - 1)];
    let start = tab.cursor_col.min(columns);
    let count = count.min(columns.saturating_sub(start));
    if count == 0 {
        return;
    }
    let mut col = columns;
    while col > start {
        let target = col - 1;
        row[target] = if target >= start + count {
            row[target - count]
        } else {
            Cell::blank()
        };
        col -= 1;
    }
}

fn insert_lines(tab: &mut TerminalTab, tab_index: usize, count: usize) {
    let lines = unsafe { GRIDS.tab_mut(tab_index) };
    let wraps = unsafe { WRAPS.tab_mut(tab_index) };
    let row = tab.cursor_line.min(MAX_SCROLLBACK_LINES - 1);
    let count = count.min(MAX_SCROLLBACK_LINES.saturating_sub(row));
    if count == 0 {
        return;
    }
    let mut index = MAX_SCROLLBACK_LINES;
    while index > row {
        let target = index - 1;
        lines[target] = if target >= row + count {
            lines[target - count]
        } else {
            [Cell::blank(); MAX_COLS]
        };
        wraps[target] = if target >= row + count {
            wraps[target - count]
        } else {
            false
        };
        index -= 1;
    }
    tab.line_count = (tab.line_count + count).min(MAX_SCROLLBACK_LINES);
}

fn delete_lines(tab: &mut TerminalTab, tab_index: usize, count: usize) {
    let lines = unsafe { GRIDS.tab_mut(tab_index) };
    let wraps = unsafe { WRAPS.tab_mut(tab_index) };
    let row = tab.cursor_line.min(MAX_SCROLLBACK_LINES - 1);
    let count = count.min(MAX_SCROLLBACK_LINES.saturating_sub(row));
    if count == 0 {
        return;
    }
    for target in row..MAX_SCROLLBACK_LINES {
        let source = target + count;
        lines[target] = if source < MAX_SCROLLBACK_LINES {
            lines[source]
        } else {
            [Cell::blank(); MAX_COLS]
        };
        wraps[target] = if source < MAX_SCROLLBACK_LINES {
            wraps[source]
        } else {
            false
        };
    }
    tab.line_count = tab.line_count.saturating_sub(count).max(1);
}

fn scroll_grid(
    lines: &mut [[Cell; MAX_COLS]; MAX_SCROLLBACK_LINES],
    wraps: &mut [bool; MAX_SCROLLBACK_LINES],
) {
    let mut row = 1usize;
    while row < MAX_SCROLLBACK_LINES {
        lines[row - 1] = lines[row];
        wraps[row - 1] = wraps[row];
        row += 1;
    }
    lines[MAX_SCROLLBACK_LINES - 1] = [Cell::blank(); MAX_COLS];
    wraps[MAX_SCROLLBACK_LINES - 1] = false;
}

fn reflow_tab(tab: &mut TerminalTab, tab_index: usize, old_columns: usize, new_columns: usize) {
    if old_columns == 0 || old_columns == new_columns {
        return;
    }

    let lines = unsafe { GRIDS.tab_mut(tab_index) };
    let wraps = unsafe { WRAPS.tab_mut(tab_index) };
    let scratch_lines = unsafe { REFLOW_CELLS.get() };
    let scratch_wraps = unsafe { REFLOW_WRAPS.get() };
    *scratch_lines = *lines;
    *scratch_wraps = *wraps;

    let source_line_count = tab.line_count.min(MAX_SCROLLBACK_LINES);
    let source_cursor_line = tab.cursor_line.min(source_line_count.saturating_sub(1));
    let source_cursor_col = tab.cursor_col.min(old_columns);
    clear_tab_grid(tab_index);

    let lines = unsafe { GRIDS.tab_mut(tab_index) };
    let wraps = unsafe { WRAPS.tab_mut(tab_index) };
    let mut dest_line = 0usize;
    let mut dest_col = 0usize;
    let mut cursor_set = false;

    for source_line in 0..source_line_count {
        let row = &scratch_lines[source_line];
        let row_len = row_visual_len(row, old_columns);
        if source_line == source_cursor_line && source_cursor_col == 0 && !cursor_set {
            tab.cursor_line = dest_line;
            tab.cursor_col = dest_col;
            cursor_set = true;
        }
        for source_col in 0..row_len {
            if source_line == source_cursor_line && source_col == source_cursor_col && !cursor_set {
                tab.cursor_line = dest_line;
                tab.cursor_col = dest_col;
                cursor_set = true;
            }
            if dest_col >= new_columns {
                wraps[dest_line.min(MAX_SCROLLBACK_LINES - 1)] = true;
                dest_line = (dest_line + 1).min(MAX_SCROLLBACK_LINES - 1);
                dest_col = 0;
            }
            lines[dest_line][dest_col] = row[source_col];
            dest_col += 1;
        }

        if source_line == source_cursor_line && source_cursor_col == row_len && !cursor_set {
            tab.cursor_line = dest_line;
            tab.cursor_col = dest_col.min(new_columns.saturating_sub(1));
            cursor_set = true;
        }

        if source_line + 1 < source_line_count && !scratch_wraps[source_line] {
            wraps[dest_line.min(MAX_SCROLLBACK_LINES - 1)] = false;
            dest_line = (dest_line + 1).min(MAX_SCROLLBACK_LINES - 1);
            dest_col = 0;
        }
    }

    tab.line_count = (dest_line + usize::from(dest_col > 0)).max(1).min(MAX_SCROLLBACK_LINES);
    if !cursor_set {
        tab.cursor_line = dest_line.min(MAX_SCROLLBACK_LINES - 1);
        tab.cursor_col = dest_col.min(new_columns.saturating_sub(1));
    } else {
        tab.cursor_line = tab.cursor_line.min(MAX_SCROLLBACK_LINES - 1);
        tab.cursor_col = tab.cursor_col.min(new_columns.saturating_sub(1));
    }
    clamp_scroll_offset(tab, new_columns.min(MAX_SCROLLBACK_LINES));
}

fn row_visual_len(row: &[Cell; MAX_COLS], columns: usize) -> usize {
    let limit = columns.min(MAX_COLS);
    let mut len = limit;
    while len > 0 && row[len - 1].ch == b' ' {
        len -= 1;
    }
    len
}

fn render(
    surface_handle: rt::Handle,
    buffer_slot: u32,
    buffer: &mut rt::MappedMemory,
    state: &TerminalState,
) -> rt::Result<()> {
    let width = state.width.min(BUFFER_WIDTH) as usize;
    let height = state.height.min(BUFFER_HEIGHT) as usize;
    let bytes = &mut buffer.as_slice_mut()[..BUFFER_BYTES];
    let theme = &THEMES[state.theme_index];

    fill_rect(bytes, 0, 0, width, height, theme.panel);
    fill_rect(
        bytes,
        0,
        0,
        width,
        ui::TITLEBAR_HEIGHT as usize,
        if state.focused { ui::ACCENT } else { ui::ACCENT_DIM },
    );
    fill_rect(
        bytes,
        0,
        ui::TITLEBAR_HEIGHT as usize,
        width,
        height.saturating_sub(ui::TITLEBAR_HEIGHT as usize),
        theme.bg,
    );

    draw_titlebar(bytes, width, theme);
    draw_tab_strip(bytes, width, state, theme);
    draw_terminal_contents(bytes, width, height, state, theme);
    rt::surface_present_buffer_slot(
        surface_handle,
        buffer_slot,
        0,
        0,
        state.width.min(BUFFER_WIDTH),
        state.height.min(BUFFER_HEIGHT),
    )
}

fn draw_titlebar(bytes: &mut [u8], width: usize, theme: &Theme) {
    let close_x = width as i32 - ui::WINDOW_BUTTON_RIGHT_MARGIN - ui::WINDOW_BUTTON_SIZE as i32;
    let minimize_x = close_x - ui::WINDOW_BUTTON_GAP - ui::WINDOW_BUTTON_SIZE as i32;
    let maximize_x = minimize_x - ui::WINDOW_BUTTON_GAP - ui::WINDOW_BUTTON_SIZE as i32;
    fill_rect(bytes, maximize_x.max(0) as usize, ui::WINDOW_BUTTON_TOP.max(0) as usize, ui::WINDOW_BUTTON_SIZE as usize, ui::WINDOW_BUTTON_SIZE as usize, theme.panel_alt);
    fill_rect(bytes, minimize_x.max(0) as usize, ui::WINDOW_BUTTON_TOP.max(0) as usize, ui::WINDOW_BUTTON_SIZE as usize, ui::WINDOW_BUTTON_SIZE as usize, theme.muted);
    fill_rect(bytes, close_x.max(0) as usize, ui::WINDOW_BUTTON_TOP.max(0) as usize, ui::WINDOW_BUTTON_SIZE as usize, ui::WINDOW_BUTTON_SIZE as usize, ui::STATUS_WARN);
    fill_rect(bytes, (maximize_x + 3).max(0) as usize, (ui::WINDOW_BUTTON_TOP + 3).max(0) as usize, 6, 6, theme.bg);
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, minimize_x + 3, ui::WINDOW_BUTTON_TOP + 2, theme.bg, "_");
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, close_x + 3, ui::WINDOW_BUTTON_TOP + 2, theme.bg, "X");
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, 10, 9, ui::TEXT_PRIMARY, "TERMINAL");
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, 76, 9, theme.muted, theme.name);
}

fn draw_tab_strip(bytes: &mut [u8], width: usize, state: &TerminalState, theme: &Theme) {
    let strip_y = ui::TITLEBAR_HEIGHT as usize + 4;
    fill_rect(bytes, CONTENT_PADDING_X, strip_y, width.saturating_sub(CONTENT_PADDING_X * 2), TAB_STRIP_HEIGHT, theme.panel_alt);
    for index in 0..MAX_TABS {
        if !state.tabs[index].occupied {
            continue;
        }
        let x = CONTENT_PADDING_X + index * TAB_WIDTH;
        let fill = if index == state.active_tab { ui::ACCENT } else { theme.panel };
        fill_rect(bytes, x, strip_y, TAB_WIDTH.saturating_sub(4), TAB_STRIP_HEIGHT.saturating_sub(2), fill);
        if state.tabs[index].title_len > 0 {
            let text = core::str::from_utf8(&state.tabs[index].title[..state.tabs[index].title_len]).unwrap_or("TAB");
            rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, (x + 8) as i32, (strip_y + 4) as i32, ui::TEXT_PRIMARY, text);
        } else {
            let mut label = rt::FixedLogBuffer::<16>::new();
            let _ = write!(&mut label, "SHELL {}", state.tabs[index].session_id);
            if let Ok(text) = core::str::from_utf8(label.as_bytes()) {
                rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, (x + 8) as i32, (strip_y + 4) as i32, ui::TEXT_PRIMARY, text);
            }
        }
    }
}

fn draw_terminal_contents(bytes: &mut [u8], width: usize, height: usize, state: &TerminalState, theme: &Theme) {
    let Some(tab) = active_tab_ref(state) else {
        return;
    };
    let start_x = CONTENT_PADDING_X;
    let start_y = ui::TITLEBAR_HEIGHT as usize + TAB_STRIP_HEIGHT + CONTENT_PADDING_Y;
    let visible_rows = state.rows.min(MAX_SCROLLBACK_LINES);
    let first_line = first_visible_line(tab, visible_rows);
    let lines = unsafe { GRIDS.tab(state.active_tab) };

    for row in 0..visible_rows {
        let grid_line = first_line + row;
        if grid_line >= MAX_SCROLLBACK_LINES {
            break;
        }
        let y = start_y + row * CELL_HEIGHT;
        if y + CELL_HEIGHT >= height {
            break;
        }
        for col in 0..state.columns {
            let x = start_x + col * CELL_WIDTH;
            if x + CELL_WIDTH >= width {
                break;
            }
            let cell = lines[grid_line][col];
            let (fg, bg) = resolve_cell_colors(cell, theme);
            let highlight = selection_contains(state.selection, grid_line, col);
            if highlight || bg != theme.bg {
                fill_rect(bytes, x, y, CELL_WIDTH, CELL_HEIGHT, if highlight { theme.selection } else { bg });
            }
            if cell.ch != b' ' {
                rt::draw_glyph_rgba8888(bytes, PIXEL_STRIDE, x as i32, y as i32, fg, rt::normalize_bitmap_glyph(cell.ch));
            }
        }
    }

    if tab.scroll_offset > 0 {
        let mut status = rt::FixedLogBuffer::<32>::new();
        let _ = write!(&mut status, "SCROLL -{}", tab.scroll_offset);
        if let Ok(label) = core::str::from_utf8(status.as_bytes()) {
            let label_width = label.len() * rt::BITMAP_GLYPH_ADVANCE;
            let label_x = width.saturating_sub(label_width + CONTENT_PADDING_X);
            rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, label_x as i32, start_y as i32, theme.muted, label);
        }
    }

    if state.focused && tab.scroll_offset == 0 && tab.cursor_visible {
        let cursor_visible_row = tab.cursor_line.saturating_sub(first_line);
        if cursor_visible_row < visible_rows && tab.cursor_col < state.columns {
            let cursor_x = start_x + tab.cursor_col * CELL_WIDTH;
            let cursor_y = start_y + cursor_visible_row * CELL_HEIGHT;
            fill_rect(bytes, cursor_x, cursor_y + CELL_HEIGHT - 2, CELL_WIDTH, 2, ui::ACCENT);
        }
    }
}

fn resolve_cell_colors(cell: Cell, theme: &Theme) -> (u32, u32) {
    let mut fg = if cell.fg == COLOR_DEFAULT {
        theme.fg
    } else {
        theme.ansi[(cell.fg - 1).min(15) as usize]
    };
    let mut bg = if cell.bg == COLOR_DEFAULT {
        theme.bg
    } else {
        theme.ansi[(cell.bg - 1).min(15) as usize]
    };
    if cell.flags & CELL_FLAG_BOLD != 0 && cell.fg != COLOR_DEFAULT && cell.fg <= 8 {
        fg = theme.ansi[(cell.fg + 7).min(16) as usize - 1];
    }
    if cell.flags & CELL_FLAG_INVERSE != 0 {
        core::mem::swap(&mut fg, &mut bg);
    }
    (fg, bg)
}

fn selection_contains(selection: Option<Selection>, line: usize, col: usize) -> bool {
    let Some(selection) = selection else {
        return false;
    };
    let (start, end) = ordered_selection(selection);
    if line < start.line || line > end.line {
        return false;
    }
    if start.line == end.line {
        return line == start.line && col >= start.col && col <= end.col;
    }
    if line == start.line {
        return col >= start.col;
    }
    if line == end.line {
        return col <= end.col;
    }
    true
}

fn ordered_selection(selection: Selection) -> (CellPos, CellPos) {
    if selection.anchor.line < selection.focus.line
        || (selection.anchor.line == selection.focus.line && selection.anchor.col <= selection.focus.col)
    {
        (selection.anchor, selection.focus)
    } else {
        (selection.focus, selection.anchor)
    }
}

fn first_visible_line(tab: &TerminalTab, visible_rows: usize) -> usize {
    let scroll_offset = tab
        .scroll_offset
        .min(tab.line_count.saturating_sub(visible_rows));
    tab.line_count.saturating_sub(visible_rows + scroll_offset)
}

fn clamp_scroll_offset(tab: &mut TerminalTab, rows: usize) {
    let max_offset = tab.line_count.saturating_sub(rows.min(MAX_SCROLLBACK_LINES));
    tab.scroll_offset = tab.scroll_offset.min(max_offset);
}

fn scroll_up_view(tab: &mut TerminalTab, lines: usize, rows: usize) {
    let max_offset = tab.line_count.saturating_sub(rows.min(MAX_SCROLLBACK_LINES));
    tab.scroll_offset = tab.scroll_offset.saturating_add(lines).min(max_offset);
}

fn scroll_down_view(tab: &mut TerminalTab, lines: usize) {
    tab.scroll_offset = tab.scroll_offset.saturating_sub(lines);
}

fn copy_selection(state: &mut TerminalState) {
    let len = copy_selection_into_local_buffer(state);
    if len > 0 && state.clipboard_handle != rt::INVALID_HANDLE {
        let _ = rt::clipboard_write(state.clipboard_handle, &state.clipboard[..len]);
    }
}

fn copy_selection_into_local_buffer(state: &mut TerminalState) -> usize {
    let Some(selection) = state.selection else {
        state.clipboard_len = 0;
        return 0;
    };
    let lines = unsafe { GRIDS.tab(state.active_tab) };
    let (start, end) = ordered_selection(selection);
    let mut len = 0usize;
    for line in start.line..=end.line {
        let start_col = if line == start.line { start.col } else { 0 };
        let end_col = if line == end.line { end.col } else { state.columns.saturating_sub(1) };
        for col in start_col..=end_col.min(MAX_COLS.saturating_sub(1)) {
            if len >= state.clipboard.len() {
                break;
            }
            state.clipboard[len] = lines[line.min(MAX_SCROLLBACK_LINES - 1)][col].ch;
            len += 1;
        }
        if line != end.line && len < state.clipboard.len() {
            state.clipboard[len] = b'\n';
            len += 1;
        }
        if len >= state.clipboard.len() {
            break;
        }
    }
    state.clipboard_len = len;
    len
}

fn paste_clipboard(state: &mut TerminalState) -> rt::Result<()> {
    let mut len = state.clipboard_len;
    if state.clipboard_handle != rt::INVALID_HANDLE {
        if let Ok(read) = rt::clipboard_read(state.clipboard_handle, &mut state.clipboard) {
            state.clipboard_len = read;
            len = read;
        }
    }
    if len > 0 {
        let session_handle = {
            let Some(tab) = active_tab_mut(state) else {
                return Ok(());
            };
            tab.scroll_offset = 0;
            tab.session_handle
        };
        state.selection = None;
        rt::terminal_session_send_input(session_handle, &state.clipboard[..len])?;
    }
    Ok(())
}

fn tab_strip_hit_index(x: i32, y: i32) -> Option<usize> {
    let strip_y = ui::TITLEBAR_HEIGHT as i32 + 4;
    if y < strip_y || y >= strip_y + TAB_STRIP_HEIGHT as i32 {
        return None;
    }
    let local_x = x - CONTENT_PADDING_X as i32;
    if local_x < 0 {
        return None;
    }
    let index = (local_x as usize) / TAB_WIDTH;
    (index < MAX_TABS).then_some(index)
}

fn pointer_to_cell(state: &TerminalState, x: i32, y: i32) -> Option<CellPos> {
    let tab = active_tab_ref(state)?;
    let start_x = CONTENT_PADDING_X as i32;
    let start_y = (ui::TITLEBAR_HEIGHT as usize + TAB_STRIP_HEIGHT + CONTENT_PADDING_Y) as i32;
    if x < start_x || y < start_y {
        return None;
    }
    let col = ((x - start_x) as usize) / CELL_WIDTH;
    let row = ((y - start_y) as usize) / CELL_HEIGHT;
    if col >= state.columns || row >= state.rows {
        return None;
    }
    let line = first_visible_line(tab, state.rows.min(MAX_SCROLLBACK_LINES)) + row;
    Some(CellPos { line, col })
}

fn fill_rect(bytes: &mut [u8], x: usize, y: usize, width: usize, height: usize, rgb: u32) {
    let end_x = (x + width).min(BUFFER_WIDTH as usize);
    let end_y = (y + height).min(BUFFER_HEIGHT as usize);
    for py in y..end_y {
        for px in x..end_x {
            rt::set_pixel_rgba8888(bytes, BUFFER_WIDTH as usize, px, py, rgb);
        }
    }
}

fn unpack_bytes(words: &[u64], len: usize, destination: &mut [u8]) -> rt::Result<()> {
    if len > destination.len() || len > words.len() * 8 {
        return Err(rt::Error::BufferTooSmall);
    }
    let mut copied = 0usize;
    for word in words.iter().copied() {
        if copied >= len {
            break;
        }
        let bytes = word.to_le_bytes();
        let chunk = (len - copied).min(bytes.len());
        destination[copied..copied + chunk].copy_from_slice(&bytes[..chunk]);
        copied += chunk;
    }
    Ok(())
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

#![no_std]
#![no_main]

use core::cell::UnsafeCell;

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{AppControlTag, ConsoleTag, ControlTag, LifecycleEvent, RawMessage, TerminalTag};

const BUFFER_WIDTH: u32 = 1024;
const BUFFER_HEIGHT: u32 = 768;
const BUFFER_BYTES: usize = BUFFER_WIDTH as usize * BUFFER_HEIGHT as usize * 4;
const MAX_COLS: usize = 120;
const MAX_SCROLLBACK_LINES: usize = 256;
const CELL_WIDTH: usize = 6;
const CELL_HEIGHT: usize = 10;
const CELL_GLYPH_WIDTH: usize = 5;
const CELL_GLYPH_HEIGHT: usize = 7;
const CONTENT_PADDING_X: usize = 10;
const CONTENT_PADDING_Y: usize = 8;
const KEY_BACKSPACE: u32 = 14;
const KEY_UP: u32 = 103;
const KEY_LEFT: u32 = 105;
const KEY_RIGHT: u32 = 106;
const KEY_DOWN: u32 = 108;
const MOD_CTRL: u32 = 1 << 2;

struct GlobalBuffer(UnsafeCell<[u8; BUFFER_BYTES]>);

unsafe impl Sync for GlobalBuffer {}

impl GlobalBuffer {
    const fn new() -> Self {
        Self(UnsafeCell::new([0; BUFFER_BYTES]))
    }

    unsafe fn as_mut(&self) -> &mut [u8; BUFFER_BYTES] {
        unsafe { &mut *self.0.get() }
    }
}

struct GlobalGrid(UnsafeCell<[[u8; MAX_COLS]; MAX_SCROLLBACK_LINES]>);

unsafe impl Sync for GlobalGrid {}

impl GlobalGrid {
    const fn new() -> Self {
        Self(UnsafeCell::new([[b' '; MAX_COLS]; MAX_SCROLLBACK_LINES]))
    }

    unsafe fn as_ref(&self) -> &[[u8; MAX_COLS]; MAX_SCROLLBACK_LINES] {
        unsafe { &*self.0.get() }
    }

    unsafe fn as_mut(&self) -> &mut [[u8; MAX_COLS]; MAX_SCROLLBACK_LINES] {
        unsafe { &mut *self.0.get() }
    }
}

static BUFFER: GlobalBuffer = GlobalBuffer::new();
static GRID: GlobalGrid = GlobalGrid::new();

#[derive(Clone, Copy)]
struct TerminalState {
    width: u32,
    height: u32,
    focused: bool,
    session_handle: rt::Handle,
    columns: usize,
    rows: usize,
    line_count: usize,
    cursor_line: usize,
    cursor_col: usize,
    parse_state: ParseState,
    csi_param: usize,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ParseState {
    Ground,
    Esc,
    Csi,
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
    let mut width = startup.words[1] as u32;
    let mut height = startup.words[2] as u32;
    let mut focused = startup.words[3] != 0;

    let buffer_handle = match rt::memory_create(BUFFER_BYTES, true) {
        Ok(handle) => handle,
        Err(_) => return 0xfa03,
    };
    if rt::surface_attach_buffer(surface_handle, buffer_handle, BUFFER_WIDTH, BUFFER_HEIGHT, BUFFER_WIDTH).is_err() {
        let _ = rt::handle_close(buffer_handle);
        return 0xfa04;
    }

    let (_, session_handle, _, _) = match rt::terminal_session_open(terminal_handle) {
        Ok(info) => info,
        Err(_) => return 0xfa05,
    };

    clear_grid();
    let mut state = TerminalState {
        width,
        height,
        focused,
        session_handle,
        columns: 0,
        rows: 0,
        line_count: 1,
        cursor_line: 0,
        cursor_col: 0,
        parse_state: ParseState::Ground,
        csi_param: 0,
    };
    recompute_layout(&mut state);
    let _ = rt::terminal_session_resize(
        state.session_handle,
        state.columns as u32,
        state.rows as u32,
        state.width,
        state.height,
    );
    let _ = render(surface_handle, buffer_handle, &state);

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => break,
            Ok(false) => {}
            Err(_) => return 0xfa06,
        }

        match poll_control(control_handle, state.session_handle, &mut width, &mut height, &mut focused) {
            Ok(ControlFlow::Continue) => {}
            Ok(ControlFlow::Exit) => break,
            Err(_) => return 0xfa07,
        }

        if width != state.width || height != state.height || focused != state.focused {
            state.width = width;
            state.height = height;
            state.focused = focused;
            recompute_layout(&mut state);
            let _ = rt::terminal_session_resize(
                state.session_handle,
                state.columns as u32,
                state.rows as u32,
                state.width,
                state.height,
            );
            let _ = render(surface_handle, buffer_handle, &state);
        }

        let mut changed = false;
        let mut data = [0u8; (rt::IPC_MAX_WORDS - 1) * 8];
        loop {
            match receive_terminal_message(state.session_handle, &mut data) {
                Ok(Some(TerminalMessage::Output(len))) => {
                    apply_output(&mut state, &data[..len]);
                    changed = true;
                }
                Ok(Some(TerminalMessage::Closed)) => return 0,
                Ok(None) => break,
                Err(rt::Error::QueueEmpty) => break,
                Err(_) => return 0xfa08,
            }
        }

        if changed {
            let _ = render(surface_handle, buffer_handle, &state);
        }

        if rt::yield_current().is_err() {
            return 0xfa09;
        }
    }

    let _ = rt::terminal_session_close(session_handle);
    let _ = rt::handle_close(session_handle);
    let _ = rt::handle_close(buffer_handle);
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
    session_handle: rt::Handle,
    width: &mut u32,
    height: &mut u32,
    focused: &mut bool,
) -> rt::Result<ControlFlow> {
    loop {
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(control_handle, &mut message) {
            Ok(()) if message.tag == AppControlTag::FocusChanged as u32 && message.word_count > 0 => {
                *focused = message.words[0] != 0;
            }
            Ok(()) if message.tag == AppControlTag::Resize as u32 && message.word_count >= 2 => {
                *width = message.words[0] as u32;
                *height = message.words[1] as u32;
            }
            Ok(()) if message.tag == AppControlTag::Close as u32 => return Ok(ControlFlow::Exit),
            Ok(()) if message.tag == AppControlTag::Text as u32 && message.word_count > 0 => {
                if let Some(ch) = core::char::from_u32(message.words[0] as u32) {
                    let mut bytes = [0u8; 4];
                    let encoded = ch.encode_utf8(&mut bytes);
                    let _ = rt::terminal_session_send_input(session_handle, encoded.as_bytes());
                }
            }
            Ok(()) if message.tag == AppControlTag::Key as u32 && message.word_count >= 2 => {
                let action = message.words[0] as u32;
                if action == rt::AppKeyAction::Down as u32 {
                    let key_code = message.words[1] as u32;
                    let modifiers = message.words.get(2).copied().unwrap_or(0) as u32;
                    handle_key_down(session_handle, key_code, modifiers)?;
                }
            }
            Ok(()) => {}
            Err(rt::Error::QueueEmpty) => break,
            Err(error) => return Err(error),
        }
    }
    Ok(ControlFlow::Continue)
}

fn handle_key_down(session_handle: rt::Handle, key_code: u32, modifiers: u32) -> rt::Result<()> {
    if modifiers & MOD_CTRL != 0 && key_code == 46 {
        return rt::terminal_session_send_input(session_handle, &[0x03]);
    }
    match key_code {
        KEY_BACKSPACE => rt::terminal_session_send_input(session_handle, &[0x7f]),
        KEY_UP => rt::terminal_session_send_input(session_handle, b"\x1b[A"),
        KEY_DOWN => rt::terminal_session_send_input(session_handle, b"\x1b[B"),
        KEY_RIGHT => rt::terminal_session_send_input(session_handle, b"\x1b[C"),
        KEY_LEFT => rt::terminal_session_send_input(session_handle, b"\x1b[D"),
        _ => Ok(()),
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
        .saturating_sub(ui::TITLEBAR_HEIGHT + (CONTENT_PADDING_Y as u32) * 2 + 4);
    state.columns = ((content_width as usize) / CELL_WIDTH).clamp(20, MAX_COLS);
    state.rows = ((content_height as usize) / CELL_HEIGHT).clamp(8, MAX_SCROLLBACK_LINES);
}

fn clear_grid() {
    let lines = unsafe { GRID.as_mut() };
    for row in lines.iter_mut() {
        for cell in row.iter_mut() {
            *cell = b' ';
        }
    }
}

fn apply_output(state: &mut TerminalState, bytes: &[u8]) {
    for byte in bytes.iter().copied() {
        match state.parse_state {
            ParseState::Ground => match byte {
                0x1b => {
                    state.parse_state = ParseState::Esc;
                    state.csi_param = 0;
                }
                b'\r' => state.cursor_col = 0,
                b'\n' => new_line(state),
                0x08 => {
                    if state.cursor_col > 0 {
                        state.cursor_col -= 1;
                    }
                }
                0x20..=0x7e => put_char(state, byte),
                _ => {}
            },
            ParseState::Esc => {
                if byte == b'[' {
                    state.parse_state = ParseState::Csi;
                    state.csi_param = 0;
                } else {
                    state.parse_state = ParseState::Ground;
                }
            }
            ParseState::Csi => {
                if byte.is_ascii_digit() {
                    state.csi_param = state.csi_param.saturating_mul(10) + (byte - b'0') as usize;
                    continue;
                }
                let param = if state.csi_param == 0 { 1 } else { state.csi_param };
                match byte {
                    b'D' => state.cursor_col = state.cursor_col.saturating_sub(param),
                    b'C' => state.cursor_col = (state.cursor_col + param).min(state.columns.saturating_sub(1)),
                    b'K' => clear_line_mode(state, param),
                    _ => {}
                }
                state.parse_state = ParseState::Ground;
                state.csi_param = 0;
            }
        }
    }
}

fn put_char(state: &mut TerminalState, byte: u8) {
    if state.cursor_col >= state.columns {
        new_line(state);
    }
    let lines = unsafe { GRID.as_mut() };
    if state.cursor_line >= MAX_SCROLLBACK_LINES {
        scroll_up(lines);
        state.cursor_line = MAX_SCROLLBACK_LINES - 1;
    }
    lines[state.cursor_line][state.cursor_col] = byte;
    state.cursor_col += 1;
}

fn new_line(state: &mut TerminalState) {
    state.cursor_col = 0;
    state.cursor_line += 1;
    if state.line_count < state.cursor_line + 1 {
        state.line_count = state.cursor_line + 1;
    }
    if state.cursor_line >= MAX_SCROLLBACK_LINES {
        let lines = unsafe { GRID.as_mut() };
        scroll_up(lines);
        state.cursor_line = MAX_SCROLLBACK_LINES - 1;
        state.line_count = MAX_SCROLLBACK_LINES;
    }
}

fn clear_line_mode(state: &mut TerminalState, mode: usize) {
    let lines = unsafe { GRID.as_mut() };
    let row = &mut lines[state.cursor_line.min(MAX_SCROLLBACK_LINES - 1)];
    match mode {
        2 => {
            for cell in row.iter_mut().take(state.columns) {
                *cell = b' ';
            }
        }
        _ => {
            for cell in row.iter_mut().take(state.columns).skip(state.cursor_col) {
                *cell = b' ';
            }
        }
    }
}

fn scroll_up(lines: &mut [[u8; MAX_COLS]; MAX_SCROLLBACK_LINES]) {
    let mut row = 1usize;
    while row < MAX_SCROLLBACK_LINES {
        lines[row - 1] = lines[row];
        row += 1;
    }
    lines[MAX_SCROLLBACK_LINES - 1] = [b' '; MAX_COLS];
}

fn render(
    surface_handle: rt::Handle,
    buffer_handle: rt::Handle,
    state: &TerminalState,
) -> rt::Result<()> {
    let width = state.width.min(BUFFER_WIDTH) as usize;
    let height = state.height.min(BUFFER_HEIGHT) as usize;
    let bytes = unsafe { &mut BUFFER.as_mut()[..BUFFER_BYTES] };

    fill_rect(bytes, 0, 0, width, height, 0x10151d);
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
        0x0b1220,
    );

    draw_titlebar(bytes, width, state.focused);
    draw_terminal_contents(bytes, width, height, state);
    rt::memory_write(buffer_handle, 0, &bytes[..BUFFER_BYTES]).map(|_| ())?;
    rt::surface_clear_scene(surface_handle)
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
    draw_text(bytes, width, 10, 9, ui::TEXT_PRIMARY, if focused { "TERMINAL ACTIVE" } else { "TERMINAL" });
}

fn draw_terminal_contents(bytes: &mut [u8], width: usize, height: usize, state: &TerminalState) {
    let start_x = CONTENT_PADDING_X;
    let start_y = ui::TITLEBAR_HEIGHT as usize + CONTENT_PADDING_Y;
    let visible_rows = state.rows.min(MAX_SCROLLBACK_LINES);
    let first_line = state.line_count.saturating_sub(visible_rows);
    let lines = unsafe { GRID.as_ref() };

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
            let ch = normalize_glyph(lines[grid_line][col]);
            draw_glyph(bytes, width, x as i32, y as i32, ui::TEXT_PRIMARY, ch);
        }
    }

    if state.focused {
        let cursor_visible_row = state.cursor_line.saturating_sub(first_line);
        if cursor_visible_row < visible_rows && state.cursor_col < state.columns {
            let cursor_x = start_x + state.cursor_col * CELL_WIDTH;
            let cursor_y = start_y + cursor_visible_row * CELL_HEIGHT;
            fill_rect(bytes, cursor_x, cursor_y + CELL_HEIGHT - 2, CELL_WIDTH, 2, ui::ACCENT);
        }
    }
}

fn fill_rect(bytes: &mut [u8], x: usize, y: usize, width: usize, height: usize, rgb: u32) {
    let end_x = (x + width).min(BUFFER_WIDTH as usize);
    let end_y = (y + height).min(BUFFER_HEIGHT as usize);
    for py in y..end_y {
        for px in x..end_x {
            set_pixel(bytes, px, py, rgb);
        }
    }
}

fn draw_text(bytes: &mut [u8], stride: usize, x: i32, y: i32, color: u32, text: &str) {
    for (index, byte) in text.as_bytes().iter().copied().enumerate() {
        draw_glyph(bytes, stride, x + (index as i32 * CELL_WIDTH as i32), y, color, normalize_glyph(byte));
    }
}

fn draw_glyph(bytes: &mut [u8], stride: usize, x: i32, y: i32, color: u32, ch: u8) {
    let rows = glyph_rows(ch);
    for (row_index, bits) in rows.iter().copied().enumerate() {
        for column in 0..CELL_GLYPH_WIDTH {
            if (bits >> (CELL_GLYPH_WIDTH - 1 - column)) & 1 == 0 {
                continue;
            }
            let px = x + column as i32;
            let py = y + row_index as i32;
            if px < 0 || py < 0 {
                continue;
            }
            set_pixel(bytes, px as usize, py as usize, color);
        }
    }
    let _ = stride;
}

fn set_pixel(bytes: &mut [u8], x: usize, y: usize, rgb: u32) {
    let index = ((y * BUFFER_WIDTH as usize) + x) * 4;
    if index + 4 > bytes.len() {
        return;
    }
    bytes[index..index + 4].copy_from_slice(&(rgb & 0x00ff_ffff).to_le_bytes());
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

fn normalize_glyph(byte: u8) -> u8 {
    if byte.is_ascii_lowercase() {
        byte - 32
    } else {
        byte
    }
}

fn glyph_rows(ch: u8) -> [u8; CELL_GLYPH_HEIGHT] {
    match ch {
        b'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        b'B' => [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e],
        b'C' => [0x0f, 0x10, 0x10, 0x10, 0x10, 0x10, 0x0f],
        b'D' => [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e],
        b'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f],
        b'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10],
        b'G' => [0x0f, 0x10, 0x10, 0x17, 0x11, 0x11, 0x0f],
        b'H' => [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        b'I' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x1f],
        b'J' => [0x01, 0x01, 0x01, 0x01, 0x11, 0x11, 0x0e],
        b'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        b'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f],
        b'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11],
        b'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        b'O' => [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        b'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10],
        b'Q' => [0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d],
        b'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11],
        b'S' => [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e],
        b'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        b'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        b'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04],
        b'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1b, 0x11],
        b'X' => [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11],
        b'Y' => [0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04],
        b'Z' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f],
        b'0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
        b'1' => [0x04, 0x0c, 0x14, 0x04, 0x04, 0x04, 0x1f],
        b'2' => [0x0e, 0x11, 0x01, 0x06, 0x08, 0x10, 0x1f],
        b'3' => [0x1f, 0x02, 0x04, 0x06, 0x01, 0x11, 0x0e],
        b'4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
        b'5' => [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e],
        b'6' => [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e],
        b'7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        b'8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
        b'9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c],
        b'.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x0c],
        b':' => [0x00, 0x0c, 0x0c, 0x00, 0x0c, 0x0c, 0x00],
        b'-' => [0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00],
        b'/' => [0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10],
        b'_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1f],
        b'>' => [0x10, 0x08, 0x04, 0x02, 0x04, 0x08, 0x10],
        b'<' => [0x01, 0x02, 0x04, 0x08, 0x04, 0x02, 0x01],
        b'=' => [0x00, 0x1f, 0x00, 0x1f, 0x00, 0x00, 0x00],
        b'?' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],
        b'!' => [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04],
        b'[' => [0x0e, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0e],
        b']' => [0x0e, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0e],
        b'(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        b')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        b'+' => [0x00, 0x04, 0x04, 0x1f, 0x04, 0x04, 0x00],
        b' ' => [0x00; CELL_GLYPH_HEIGHT],
        _ => [0x0e, 0x11, 0x01, 0x06, 0x04, 0x00, 0x04],
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

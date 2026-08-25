use core::fmt::Write as _;
use core::str;

use rt::FixedLogBuffer;
use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::state::{
    BUFFER_BYTES, BUFFER_HEIGHT, BUFFER_WIDTH, BUTTON_H, BUTTON_W, BUTTON_Y, LIST_X, LIST_Y,
    MediaState, PIXEL_STRIDE, PLAY_X, PlayState, ROW_HEIGHT, STOP_X,
};

const VISIBLE_ROWS: usize = 20;

pub(crate) fn render(
    presenter: &mut ui::FirstPresentSurface,
    buffer_slot: u32,
    buffer: &mut rt::MappedMemory,
    state: &MediaState,
) -> rt::Result<()> {
    let width = state.width.min(BUFFER_WIDTH) as usize;
    let height = state.height.min(BUFFER_HEIGHT) as usize;
    let bytes = &mut buffer.as_slice_mut()[..BUFFER_BYTES];

    ui::draw_window_frame_rgba8888(
        bytes,
        PIXEL_STRIDE,
        width,
        height,
        state.focused,
        ui::BG_WINDOW_ALT,
        "MEDIA",
    );
    draw_header(bytes, state);
    draw_list(bytes, state);
    draw_transport(bytes, state);

    presenter.present(
        buffer_slot,
        state.width.min(BUFFER_WIDTH),
        state.height.min(BUFFER_HEIGHT),
    )
}

fn line_buffer() -> FixedLogBuffer<112> {
    FixedLogBuffer::<112>::new()
}

fn draw_text(bytes: &mut [u8], x: i32, y: i32, rgb: u32, buffer: &FixedLogBuffer<112>) {
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        x,
        y,
        rgb,
        str::from_utf8(buffer.as_bytes()).unwrap_or(""),
    );
}

fn draw_header(bytes: &mut [u8], state: &MediaState) {
    let mut header = line_buffer();
    let _ = header.write_fmt(format_args!(
        "{} TRACKS  VOL {}%{}",
        state.track_count,
        state.volume_percent,
        if state.muted { " MUTE" } else { "" }
    ));
    if state.scan_failed {
        let _ = header.write_fmt(format_args!("  SCAN PARTIAL"));
    }
    draw_text(
        bytes,
        LIST_X,
        ui::TITLEBAR_HEIGHT as i32 + 8,
        ui::TEXT_PRIMARY,
        &header,
    );
}

fn draw_list(bytes: &mut [u8], state: &MediaState) {
    if !state.scan_done {
        return;
    }
    if state.track_count == 0 {
        let mut empty = line_buffer();
        let _ = empty.write_fmt(format_args!("NO AUDIO FILES FOUND"));
        draw_text(bytes, LIST_X, LIST_Y, ui::TEXT_MUTED, &empty);
        return;
    }
    for row in 0..VISIBLE_ROWS.min(state.track_count) {
        let y = LIST_Y + row as i32 * ROW_HEIGHT;
        let selected = row == state.selected;
        let playing = state.play_state != PlayState::Idle && row == state.playing_track;
        if selected {
            ui::fill_rgba8888_rect(
                bytes,
                PIXEL_STRIDE,
                BUFFER_WIDTH as usize,
                BUFFER_HEIGHT as usize,
                LIST_X as usize,
                (y - 2).max(0) as usize,
                (BUFFER_WIDTH as i32 - 2 * LIST_X) as usize,
                ROW_HEIGHT as usize,
                ui::ACCENT_DIM,
            );
        }
        let track = &state.tracks[row];
        let name = str::from_utf8(track.name_bytes()).unwrap_or("TRACK");
        let mut row_line = line_buffer();
        let _ = if playing {
            row_line.write_fmt(format_args!("> {name}"))
        } else {
            row_line.write_fmt(format_args!("{name}"))
        };
        draw_text(
            bytes,
            LIST_X,
            y,
            if playing {
                ui::STATUS_OK
            } else {
                ui::TEXT_PRIMARY
            },
            &row_line,
        );
    }
}

fn draw_transport(bytes: &mut [u8], state: &MediaState) {
    draw_button(bytes, PLAY_X, "PLAY", ui::STATUS_OK);
    draw_button(bytes, STOP_X, "STOP", ui::STATUS_WARN);

    let mut status = line_buffer();
    match state.play_state {
        PlayState::Idle => {
            let _ = status.write_fmt(format_args!("IDLE"));
        }
        PlayState::Playing => {
            let percent = if state.total_frames == 0 {
                0
            } else {
                (state.frame_cursor * 100 / state.total_frames).min(100)
            };
            let _ = status.write_fmt(format_args!(
                "PLAYING {}%  {} / {} FRAMES  EST {}s",
                percent,
                state.frame_cursor,
                state.total_frames,
                state.total_ms / 1000
            ));
        }
    }
    if state.file_truncated && state.play_state != PlayState::Idle {
        let _ = status.write_fmt(format_args!(" (TRUNCATED)"));
    }
    draw_text(
        bytes,
        STOP_X + BUTTON_W + 12,
        BUTTON_Y + 10,
        ui::TEXT_PRIMARY,
        &status,
    );

    let mut note = line_buffer();
    let _ = note.write_fmt(format_args!(
        "{}",
        str::from_utf8(state.note_bytes()).unwrap_or("")
    ));
    draw_text(
        bytes,
        LIST_X,
        BUTTON_Y + BUTTON_H + 14,
        ui::TEXT_SECONDARY,
        &note,
    );

    // Volume bar.
    let bar_width = 200usize;
    let bar_y = (BUTTON_Y + BUTTON_H + 34) as usize;
    ui::fill_rgba8888_rect(
        bytes,
        PIXEL_STRIDE,
        BUFFER_WIDTH as usize,
        BUFFER_HEIGHT as usize,
        LIST_X as usize,
        bar_y,
        bar_width,
        10,
        ui::BG_PANEL,
    );
    let filled = bar_width * state.volume_percent as usize / 100;
    ui::fill_rgba8888_rect(
        bytes,
        PIXEL_STRIDE,
        BUFFER_WIDTH as usize,
        BUFFER_HEIGHT as usize,
        LIST_X as usize,
        bar_y,
        filled,
        10,
        ui::ACCENT,
    );
}

fn draw_button(bytes: &mut [u8], x: i32, label: &str, rgb: u32) {
    ui::fill_rgba8888_rect(
        bytes,
        PIXEL_STRIDE,
        BUFFER_WIDTH as usize,
        BUFFER_HEIGHT as usize,
        x as usize,
        BUTTON_Y as usize,
        BUTTON_W as usize,
        BUTTON_H as usize,
        ui::BG_PANEL,
    );
    ui::fill_rgba8888_rect(
        bytes,
        PIXEL_STRIDE,
        BUFFER_WIDTH as usize,
        BUFFER_HEIGHT as usize,
        x as usize,
        BUTTON_Y as usize,
        BUTTON_W as usize,
        2,
        rgb,
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        x + 24,
        BUTTON_Y + 10,
        ui::TEXT_PRIMARY,
        label,
    );
}

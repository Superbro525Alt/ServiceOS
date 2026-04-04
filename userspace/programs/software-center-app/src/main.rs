#![no_std]
#![no_main]

use core::{array, fmt::Write, str};

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{
    AppControlTag, AppKeyAction, AppPointerAction, ControlTag, FixedLogBuffer, LifecycleEvent,
    RawMessage, ServiceId,
};

const MAX_ENTRIES: usize = 24;
const MAX_CATEGORY_BYTES: usize = 24;
const MAX_SUMMARY_BYTES: usize = 72;
const MAX_STATUS_BYTES: usize = 80;
const MAX_SOURCE_BYTES: usize = 96;
const BUFFER_WIDTH: u32 = 1024;
const BUFFER_HEIGHT: u32 = 768;
const BUFFER_BYTES: usize = BUFFER_WIDTH as usize * BUFFER_HEIGHT as usize * 4;
const SURFACE_BUFFER_SLOTS: usize = 2;
const PIXEL_STRIDE: usize = BUFFER_WIDTH as usize;
const OUTER_PAD: i32 = 14;
const CONTENT_GAP: i32 = 12;
const HEADER_HEIGHT: i32 = 56;
const PANEL_TITLE_HEIGHT: i32 = 26;
const ROW_HEIGHT: i32 = 30;
const BUTTON_HEIGHT: i32 = 22;
const ACTION_BUTTON_WIDTH: i32 = 104;
const ACTION_BUTTON_GAP: i32 = 10;
const STATUS_BAR_HEIGHT: i32 = 24;
const KEY_ENTER: u32 = 28;
const KEY_BACKSPACE: u32 = 14;
const KEY_DELETE: u32 = 111;
const KEY_R: u32 = 19;
const KEY_UP: u32 = 103;
const KEY_PAGE_UP: u32 = 104;
const KEY_DOWN: u32 = 108;
const KEY_PAGE_DOWN: u32 = 109;

#[derive(Clone, Copy)]
struct Layout {
    header_x: i32,
    header_y: i32,
    header_w: i32,
    left_x: i32,
    left_y: i32,
    left_w: i32,
    left_h: i32,
    right_x: i32,
    right_y: i32,
    right_w: i32,
    right_h: i32,
    list_rows_y: i32,
    list_rows_h: i32,
    sync_x0: i32,
    sync_x1: i32,
    sync_y0: i32,
    sync_y1: i32,
    install_x0: i32,
    install_x1: i32,
    install_y0: i32,
    install_y1: i32,
    remove_x0: i32,
    remove_x1: i32,
    remove_y0: i32,
    remove_y1: i32,
    detail_title_y: i32,
    detail_body_y: i32,
    detail_chip_y: i32,
    detail_text_w: i32,
    status_y: i32,
}

impl Layout {
    fn visible_rows(self) -> usize {
        self.list_rows_h.max(0) as usize / ROW_HEIGHT as usize
    }
}

#[derive(Clone, Copy)]
struct CatalogEntry {
    service_id: ServiceId,
    repo_index: u32,
    installed: bool,
    active: bool,
    rollback: bool,
    latest_version: [u8; 24],
    latest_version_len: usize,
    category: [u8; MAX_CATEGORY_BYTES],
    category_len: usize,
    summary: [u8; MAX_SUMMARY_BYTES],
    summary_len: usize,
}

impl CatalogEntry {
    const fn empty() -> Self {
        Self {
            service_id: ServiceId::RootManager,
            repo_index: 0,
            installed: false,
            active: false,
            rollback: false,
            latest_version: [0; 24],
            latest_version_len: 0,
            category: [0; MAX_CATEGORY_BYTES],
            category_len: 0,
            summary: [0; MAX_SUMMARY_BYTES],
            summary_len: 0,
        }
    }
}

struct AppState {
    width: u32,
    height: u32,
    focused: bool,
    entries: [CatalogEntry; MAX_ENTRIES],
    entry_count: usize,
    selected_index: usize,
    scroll_offset: usize,
    status: [u8; MAX_STATUS_BYTES],
    status_len: usize,
}

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf501;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 3 || startup.word_count < 4 {
        return 0xf502;
    }

    let surface_handle = startup.handles[0];
    let control_handle = startup.handles[1];
    let package_handle = startup.handles[2];
    let mut state = AppState {
        width: startup.words[1] as u32,
        height: startup.words[2] as u32,
        focused: startup.words[3] != 0,
        entries: [CatalogEntry::empty(); MAX_ENTRIES],
        entry_count: 0,
        selected_index: 0,
        scroll_offset: 0,
        status: [0; MAX_STATUS_BYTES],
        status_len: 0,
    };
    let mut buffer_handles = [rt::INVALID_HANDLE; SURFACE_BUFFER_SLOTS];
    let mut mapped_buffers: [Option<rt::MappedMemory>; SURFACE_BUFFER_SLOTS] =
        array::from_fn(|_| None);
    for slot in 0..SURFACE_BUFFER_SLOTS {
        let buffer_handle = match rt::memory_create(BUFFER_BYTES, true) {
            Ok(handle) => handle,
            Err(_) => return 0xf507,
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
            return 0xf508;
        }
        let mapped_buffer = match rt::MappedMemory::map(buffer_handle, BUFFER_BYTES, true) {
            Ok(buffer) => buffer,
            Err(_) => {
                let _ = rt::handle_close(buffer_handle);
                return 0xf509;
            }
        };
        buffer_handles[slot] = buffer_handle;
        mapped_buffers[slot] = Some(mapped_buffer);
    }
    let mut front_buffer_slot = 0usize;

    let _ = reload_catalog(package_handle, &mut state);
    if render(
        surface_handle,
        front_buffer_slot as u32,
        mapped_buffers[front_buffer_slot].as_mut().unwrap(),
        package_handle,
        &state,
    )
    .is_err()
    {
        return 0xf503;
    }

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => break,
            Ok(false) => {}
            Err(_) => return 0xf504,
        }

        match poll_control(
            control_handle,
            surface_handle,
            package_handle,
            &mut mapped_buffers,
            &mut front_buffer_slot,
            &mut state,
        ) {
            Ok(ControlFlow::Idle) => {}
            Ok(ControlFlow::Worked) => continue,
            Ok(ControlFlow::Exit) => break,
            Err(_) => return 0xf505,
        }

        if rt::yield_current().is_err() {
            return 0xf506;
        }
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
    package_handle: rt::Handle,
    buffers: &mut [Option<rt::MappedMemory>; SURFACE_BUFFER_SLOTS],
    front_buffer_slot: &mut usize,
    state: &mut AppState,
) -> rt::Result<ControlFlow> {
    let mut changed = false;
    let mut did_work = false;
    loop {
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(control_handle, &mut message) {
            Ok(()) if message.tag == AppControlTag::FocusChanged as u32 && message.word_count > 0 => {
                state.focused = message.words[0] != 0;
                changed = true;
                did_work = true;
            }
            Ok(()) if message.tag == AppControlTag::Resize as u32 && message.word_count >= 2 => {
                state.width = message.words[0] as u32;
                state.height = message.words[1] as u32;
                clamp_view(state);
                changed = true;
                did_work = true;
            }
            Ok(()) if message.tag == AppControlTag::Pointer as u32 && message.word_count >= 5 => {
                let action = pointer_action_from_word(message.words[0]);
                let x = message.words[1] as i64 as i32;
                let y = message.words[2] as i64 as i32;
                let detail = message.words[4] as i64 as i32;
                did_work = true;
                match action {
                    Some(AppPointerAction::Down) => {
                        changed |= handle_pointer_down(package_handle, state, x, y)?;
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
                    changed |= handle_key_down(package_handle, state, message.words[1] as u32)?;
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
            package_handle,
            state,
        )?;
        return Ok(ControlFlow::Worked);
    }
    if did_work {
        return Ok(ControlFlow::Worked);
    }
    Ok(ControlFlow::Idle)
}

fn render(
    surface_handle: rt::Handle,
    buffer_slot: u32,
    buffer: &mut rt::MappedMemory,
    package_handle: rt::Handle,
    state: &AppState,
) -> rt::Result<()> {
    let layout = compute_layout(state);
    let width = state.width.min(BUFFER_WIDTH) as usize;
    let height = state.height.min(BUFFER_HEIGHT) as usize;
    let bytes = &mut buffer.as_slice_mut()[..BUFFER_BYTES];
    let mut detail0 = FixedLogBuffer::<64>::new();
    let mut detail1 = FixedLogBuffer::<80>::new();
    let mut detail2 = FixedLogBuffer::<80>::new();
    let mut detail3 = FixedLogBuffer::<96>::new();
    if let Some(entry) = selected_entry(state) {
        let mut installed = [0u8; 24];
        let mut active = [0u8; 24];
        let mut rollback = [0u8; 24];
        let mut latest = [0u8; 24];
        let mut source = [0u8; MAX_SOURCE_BYTES];
        if let Ok(provenance) = rt::package_provenance(
            package_handle,
            entry.service_id,
            &mut installed,
            &mut active,
            &mut rollback,
            &mut latest,
            &mut source,
        ) {
            let _ = write!(
                &mut detail0,
                "{}",
                service_title(entry.service_id),
            );
            let _ = write!(
                &mut detail1,
                "{}  repo={}  {}",
                text_or_dash(&entry.summary[..entry.summary_len]),
                provenance.repo_index,
                trust_badge(provenance.trust_state),
            );
            let _ = write!(
                &mut detail1,
                ""
            );
            let _ = write!(
                &mut detail2,
                "latest={}  installed={}  active={}",
                text_or_dash(&latest[..provenance.latest_version_len]),
                text_or_dash(&installed[..provenance.installed_version_len]),
                text_or_dash(&active[..provenance.active_version_len]),
            );
            let _ = write!(
                &mut detail3,
                "channel={}  ring={}  rollback={}",
                channel_label(provenance.channel),
                ring_label(provenance.ring),
                text_or_dash(&rollback[..provenance.rollback_version_len]),
            );
            let _ = source;
        } else {
            let _ = write!(&mut detail0, "{}", service_title(entry.service_id));
            let _ = write!(&mut detail1, "{}", text_or_dash(&entry.summary[..entry.summary_len]));
            let _ = write!(&mut detail2, "latest={}", text_or_dash(&entry.latest_version[..entry.latest_version_len]));
            let _ = write!(&mut detail3, "category={}", text_or_dash(&entry.category[..entry.category_len]));
        }
    } else {
        let _ = write!(&mut detail0, "Select a package");
        let _ = write!(&mut detail1, "Browse the catalog and inspect trust before installing.");
    }

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
    draw_titlebar(bytes, width);
    draw_header(bytes, layout, state);
    draw_panel(bytes, layout.left_x, layout.left_y, layout.left_w, layout.left_h, ui::BG_PANEL);
    draw_panel(bytes, layout.right_x, layout.right_y, layout.right_w, layout.right_h, ui::BG_PANEL);
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, layout.left_x + 12, layout.left_y + 10, ui::TEXT_PRIMARY, "CATALOG");
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, layout.right_x + 12, layout.right_y + 10, ui::TEXT_PRIMARY, "DETAILS");
    draw_button(
        bytes,
        layout.sync_x0,
        layout.sync_y0,
        layout.sync_x1,
        layout.sync_y1,
        ui::ACCENT_DIM,
        "SYNC ALL",
        ui::TEXT_PRIMARY,
    );
    draw_details(
        bytes,
        layout,
        str::from_utf8(detail0.as_bytes()).unwrap_or("PACKAGE"),
        str::from_utf8(detail1.as_bytes()).unwrap_or("DETAILS"),
        str::from_utf8(detail2.as_bytes()).unwrap_or("DETAILS"),
        str::from_utf8(detail3.as_bytes()).unwrap_or("DETAILS"),
        str::from_utf8(&state.status[..state.status_len]).unwrap_or(""),
        selected_entry(state),
    );
    draw_button(
        bytes,
        layout.install_x0,
        layout.install_y0,
        layout.install_x1,
        layout.install_y1,
        if selected_entry(state).is_some_and(|entry| entry.installed) {
            ui::STATUS_OK
        } else {
            ui::ACCENT
        },
        action_label(selected_entry(state)),
        ui::BG_PANEL,
    );
    draw_button(
        bytes,
        layout.remove_x0,
        layout.remove_y0,
        layout.remove_x1,
        layout.remove_y1,
        ui::STATUS_WARN,
        "REMOVE",
        ui::BG_PANEL,
    );
    draw_list(bytes, layout, state);
    rt::surface_present_buffer_slot(
        surface_handle,
        buffer_slot,
        0,
        0,
        state.width.min(BUFFER_WIDTH),
        state.height.min(BUFFER_HEIGHT),
    )
}

fn reload_catalog(package_handle: rt::Handle, state: &mut AppState) -> rt::Result<()> {
    state.entry_count = 0;
    state.selected_index = 0;
    state.scroll_offset = 0;
    let mut latest = [0u8; 24];
    let mut category = [0u8; MAX_CATEGORY_BYTES];
    let mut summary = [0u8; MAX_SUMMARY_BYTES];
    for index in 0..MAX_ENTRIES {
        let Some(entry) =
            rt::package_catalog(package_handle, index, &mut latest, &mut category, &mut summary)?
        else {
            break;
        };
        state.entries[state.entry_count] = CatalogEntry {
            service_id: entry.service_id,
            repo_index: entry.repo_index,
            installed: entry.installed,
            active: entry.active,
            rollback: entry.rollback_available,
            latest_version: latest,
            latest_version_len: entry.latest_version_len,
            category,
            category_len: entry.category_len,
            summary,
            summary_len: entry.summary_len,
        };
        state.entry_count += 1;
    }
    let entry_count = state.entry_count;
    set_statusf(state, format_args!("catalog loaded: {} entries", entry_count));
    Ok(())
}

fn handle_pointer_down(
    package_handle: rt::Handle,
    state: &mut AppState,
    x: i32,
    y: i32,
) -> rt::Result<bool> {
    let layout = compute_layout(state);
    if y >= layout.sync_y0 && y < layout.sync_y1 && x >= layout.sync_x0 && x < layout.sync_x1 {
        sync_repositories(package_handle, state);
        return Ok(true);
    }
    if y >= layout.install_y0 && y < layout.install_y1 && x >= layout.install_x0 && x < layout.install_x1 {
        if let Some(entry) = selected_entry(state) {
            apply_selected_package_action(package_handle, state, entry, PackageAction::InstallOrUpdate);
            return Ok(true);
        }
    }
    if y >= layout.remove_y0 && y < layout.remove_y1 && x >= layout.remove_x0 && x < layout.remove_x1 {
        if let Some(entry) = selected_entry(state) {
            apply_selected_package_action(package_handle, state, entry, PackageAction::Remove);
            return Ok(true);
        }
    }

    let visible_rows = layout.visible_rows();
    if x >= layout.left_x + 8 && x < layout.left_x + layout.left_w - 8 && y >= layout.list_rows_y {
        let row = ((y - layout.list_rows_y) / ROW_HEIGHT) as usize;
        let entry_index = state.scroll_offset + row;
        if row < visible_rows && entry_index < state.entry_count {
            state.selected_index = entry_index;
            return Ok(true);
        }
    }
    Ok(false)
}

fn handle_key_down(package_handle: rt::Handle, state: &mut AppState, key: u32) -> rt::Result<bool> {
    match key {
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
            let step = visible_row_count(state.height).max(1);
            state.selected_index = state.selected_index.saturating_sub(step);
            ensure_selected_visible(state);
            return Ok(true);
        }
        KEY_PAGE_DOWN => {
            let step = visible_row_count(state.height).max(1);
            if state.entry_count > 0 {
                state.selected_index = (state.selected_index + step).min(state.entry_count - 1);
                ensure_selected_visible(state);
                return Ok(true);
            }
        }
        KEY_ENTER => {
            if let Some(entry) = selected_entry(state) {
                apply_selected_package_action(package_handle, state, entry, PackageAction::InstallOrUpdate);
                return Ok(true);
            }
        }
        KEY_BACKSPACE | KEY_DELETE => {
            if let Some(entry) = selected_entry(state).filter(|entry| entry.installed) {
                apply_selected_package_action(package_handle, state, entry, PackageAction::Remove);
                return Ok(true);
            }
        }
        KEY_R => {
            sync_repositories(package_handle, state);
            return Ok(true);
        }
        _ => {}
    }
    Ok(false)
}

#[derive(Clone, Copy)]
enum PackageAction {
    InstallOrUpdate,
    Remove,
}

fn sync_repositories(package_handle: rt::Handle, state: &mut AppState) {
    match rt::package_repository_sync(package_handle, None) {
        Ok(sync) => {
            if reload_catalog(package_handle, state).is_ok() {
                set_statusf(
                    state,
                    format_args!("sync complete: {} ok, {} failed", sync.synced, sync.failed),
                );
            } else {
                set_statusf(state, format_args!("sync complete but catalog reload failed"));
            }
        }
        Err(error) => set_statusf(state, format_args!("sync failed: {}", error_label(error))),
    }
}

fn apply_selected_package_action(
    package_handle: rt::Handle,
    state: &mut AppState,
    entry: CatalogEntry,
    action: PackageAction,
) {
    let result = match action {
        PackageAction::InstallOrUpdate => {
            if entry.installed {
                rt::package_update(package_handle, entry.service_id, None)
            } else {
                rt::package_install(package_handle, entry.service_id, None)
            }
        }
        PackageAction::Remove => rt::package_remove(package_handle, entry.service_id),
    };

    match result {
        Ok(()) => {
            if reload_catalog(package_handle, state).is_ok() {
                select_service(state, entry.service_id);
                match action {
                    PackageAction::InstallOrUpdate => {
                        if entry.installed {
                            set_statusf(
                                state,
                                format_args!("updated {}", service_label(entry.service_id)),
                            );
                        } else {
                            set_statusf(
                                state,
                                format_args!("installed {}", service_label(entry.service_id)),
                            );
                        }
                    }
                    PackageAction::Remove => {
                        set_statusf(state, format_args!("removed {}", service_label(entry.service_id)));
                    }
                }
            } else {
                set_statusf(state, format_args!("package action completed but reload failed"));
            }
        }
        Err(error) => {
            let verb = match action {
                PackageAction::InstallOrUpdate => {
                    if entry.installed { "update" } else { "install" }
                }
                PackageAction::Remove => "remove",
            };
            set_statusf(
                state,
                format_args!("{} failed: {}", verb, error_label(error)),
            );
        }
    }
}

fn draw_titlebar(bytes: &mut [u8], width: usize) {
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
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, 10, 9, ui::TEXT_PRIMARY, "SOFTWARE CENTER");
}

fn draw_header(bytes: &mut [u8], layout: Layout, state: &AppState) {
    draw_panel(
        bytes,
        layout.header_x,
        layout.header_y,
        layout.header_w,
        HEADER_HEIGHT,
        ui::BG_PANEL,
    );
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, layout.header_x + 14, layout.header_y + 12, ui::TEXT_PRIMARY, "DISCOVER AND MANAGE SOFTWARE");
    let mut summary = FixedLogBuffer::<64>::new();
    let _ = write!(
        &mut summary,
        "{} packages  {} installed",
        state.entry_count,
        installed_count(state),
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        layout.header_x + 14,
        layout.header_y + 28,
        ui::TEXT_SECONDARY,
        str::from_utf8(summary.as_bytes()).unwrap_or(""),
    );
}

fn draw_details(
    bytes: &mut [u8],
    layout: Layout,
    detail0: &str,
    detail1: &str,
    detail2: &str,
    detail3: &str,
    status: &str,
    entry: Option<CatalogEntry>,
) {
    let meta_x = layout.right_x + 12;
    let title_y = layout.detail_title_y;
    draw_text_fit(bytes, meta_x, title_y, ui::TEXT_PRIMARY, detail0, layout.detail_text_w);
    draw_text_fit(
        bytes,
        meta_x,
        title_y + 16,
        ui::TEXT_SECONDARY,
        detail1,
        layout.detail_text_w,
    );
    if let Some(entry) = entry {
        draw_chip(
            bytes,
            meta_x,
            layout.detail_chip_y,
            category_chip_label(&entry),
            ui::ACCENT_DIM,
            ui::TEXT_PRIMARY,
        );
        if entry.installed {
            draw_chip(bytes, meta_x + 102, layout.detail_chip_y, "INSTALLED", ui::STATUS_OK, ui::BG_PANEL);
        }
        if entry.active {
            draw_chip(bytes, meta_x + 188, layout.detail_chip_y, "ACTIVE", ui::ACCENT, ui::BG_PANEL);
        }
    }
    draw_text_fit(
        bytes,
        meta_x,
        layout.detail_body_y,
        ui::TEXT_SECONDARY,
        detail2,
        layout.detail_text_w,
    );
    draw_text_fit(
        bytes,
        meta_x,
        layout.detail_body_y + 14,
        ui::TEXT_SECONDARY,
        detail3,
        layout.detail_text_w,
    );
    draw_status_bar(bytes, layout.right_x + 12, layout.status_y, layout.right_w - 24, status);
}

fn draw_list(bytes: &mut [u8], layout: Layout, state: &AppState) {
    let visible_rows = layout.visible_rows();
    for row in 0..visible_rows {
        let entry_index = state.scroll_offset + row;
        if entry_index >= state.entry_count {
            break;
        }
        let entry = state.entries[entry_index];
        let row_y = layout.list_rows_y as usize + row * ROW_HEIGHT as usize;
        let selected = entry_index == state.selected_index;
        fill_rect(
            bytes,
            (layout.left_x + 8) as usize,
            row_y,
            (layout.left_w - 16).max(0) as usize,
            (ROW_HEIGHT - 4).max(0) as usize,
            if selected { ui::ACCENT_DIM } else { ui::BG_WINDOW },
        );
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            layout.left_x + 14,
            row_y as i32 + 4,
            if selected {
                ui::TEXT_PRIMARY
            } else {
                ui::TEXT_SECONDARY
            },
            service_title(entry.service_id),
        );
        let mut meta = FixedLogBuffer::<96>::new();
        let _ = write!(
            &mut meta,
            "v{}  {}  r{}  {}{}{}",
            text_or_dash(&entry.latest_version[..entry.latest_version_len]),
            text_or_dash(&entry.category[..entry.category_len]),
            entry.repo_index,
            if entry.installed { "I" } else { "-" },
            if entry.active { "A" } else { "-" },
            if entry.rollback { "R" } else { "-" },
        );
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            layout.left_x + 14,
            row_y as i32 + 16,
            ui::TEXT_MUTED,
            str::from_utf8(meta.as_bytes()).unwrap_or(""),
        );
    }
}

fn draw_button(
    bytes: &mut [u8],
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: u32,
    label: &str,
    text_color: u32,
) {
    fill_rect(
        bytes,
        x0.max(0) as usize,
        y0.max(0) as usize,
        (x1 - x0).max(0) as usize,
        (y1 - y0).max(0) as usize,
        color,
    );
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, x0 + 8, y0 + 7, text_color, label);
}

fn draw_panel(bytes: &mut [u8], x: i32, y: i32, width: i32, height: i32, color: u32) {
    fill_rect(
        bytes,
        x.max(0) as usize,
        y.max(0) as usize,
        width.max(0) as usize,
        height.max(0) as usize,
        color,
    );
}

fn draw_chip(bytes: &mut [u8], x: i32, y: i32, label: &str, color: u32, text: u32) {
    let width = (label.len() as i32 * 8 + 12).min(128);
    fill_rect(bytes, x.max(0) as usize, y.max(0) as usize, width.max(0) as usize, 16, color);
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, x + 6, y + 4, text, label);
}

fn draw_text_fit(bytes: &mut [u8], x: i32, y: i32, color: u32, text: &str, width: i32) {
    let max_chars = (width.max(8) as usize / 8).max(1);
    let mut buffer = FixedLogBuffer::<128>::new();
    let text_bytes = text.as_bytes();
    if text_bytes.len() <= max_chars {
        let _ = buffer.write_str(text);
    } else if max_chars <= 1 {
        let _ = buffer.write_str(".");
    } else if max_chars == 2 {
        let _ = buffer.write_str("..");
    } else {
        let visible = max_chars.saturating_sub(3);
        let slice = &text_bytes[..visible.min(text_bytes.len())];
        let clipped = str::from_utf8(slice).unwrap_or("?");
        let _ = buffer.write_str(clipped);
        let _ = buffer.write_str("...");
    }
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        x,
        y,
        color,
        str::from_utf8(buffer.as_bytes()).unwrap_or(""),
    );
}

fn draw_status_bar(bytes: &mut [u8], x: i32, y: i32, width: i32, status: &str) {
    fill_rect(
        bytes,
        x.max(0) as usize,
        y.max(0) as usize,
        width.max(0) as usize,
        STATUS_BAR_HEIGHT as usize,
        ui::BG_WINDOW,
    );
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, x + 8, y + 8, ui::TEXT_MUTED, status);
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

fn selected_entry(state: &AppState) -> Option<CatalogEntry> {
    state.entries[..state.entry_count].get(state.selected_index).copied()
}

fn installed_count(state: &AppState) -> usize {
    state.entries[..state.entry_count]
        .iter()
        .filter(|entry| entry.installed)
        .count()
}

fn select_service(state: &mut AppState, service_id: ServiceId) {
    if let Some(index) = state.entries[..state.entry_count]
        .iter()
        .position(|entry| entry.service_id == service_id)
    {
        state.selected_index = index;
        ensure_selected_visible(state);
    }
}

fn visible_row_count(height: u32) -> usize {
    compute_layout_for_height(height).visible_rows()
}

fn ensure_selected_visible(state: &mut AppState) {
    let visible = visible_row_count(state.height).max(1);
    if state.selected_index < state.scroll_offset {
        state.scroll_offset = state.selected_index;
    } else if state.selected_index >= state.scroll_offset + visible {
        state.scroll_offset = state.selected_index + 1 - visible;
    }
}

fn clamp_view(state: &mut AppState) {
    let visible = visible_row_count(state.height).max(1);
    let max_scroll = state.entry_count.saturating_sub(visible);
    if state.scroll_offset > max_scroll {
        state.scroll_offset = max_scroll;
    }
    if state.selected_index >= state.entry_count && state.entry_count != 0 {
        state.selected_index = state.entry_count - 1;
    }
}

fn scroll_up(state: &mut AppState, amount: usize) {
    state.scroll_offset = state.scroll_offset.saturating_sub(amount.max(1));
    if state.selected_index < state.scroll_offset {
        state.selected_index = state.scroll_offset;
    }
}

fn scroll_down(state: &mut AppState, amount: usize) {
    let visible = visible_row_count(state.height).max(1);
    let max_scroll = state.entry_count.saturating_sub(visible);
    state.scroll_offset = (state.scroll_offset + amount.max(1)).min(max_scroll);
    if state.selected_index < state.scroll_offset {
        state.selected_index = state.scroll_offset;
    }
}

fn set_statusf(state: &mut AppState, args: core::fmt::Arguments<'_>) {
    let mut buffer = FixedLogBuffer::<MAX_STATUS_BYTES>::new();
    let _ = buffer.write_fmt(args);
    state.status_len = buffer.as_bytes().len().min(state.status.len());
    state.status[..state.status_len].copy_from_slice(&buffer.as_bytes()[..state.status_len]);
}

fn error_label(error: rt::Error) -> &'static str {
    match error {
        rt::Error::NotFound => "not found",
        rt::Error::PermissionDenied => "denied",
        rt::Error::Busy => "busy",
        rt::Error::NotInitialized => "not ready",
        rt::Error::InvalidArgument => "invalid",
        rt::Error::InvalidCall => "verification failed",
        rt::Error::Unsupported => "unsupported",
        rt::Error::BufferTooSmall => "buffer too small",
        rt::Error::CapacityExceeded => "capacity exceeded",
        rt::Error::QueueEmpty => "timeout",
        rt::Error::Unknown(_) => "unknown",
    }
}

fn category_chip_label(entry: &CatalogEntry) -> &str {
    let category = text_or_dash(&entry.category[..entry.category_len]);
    if category == "-" {
        "SYSTEM"
    } else {
        category
    }
}

fn trust_badge(value: rt::PackageTrustState) -> &'static str {
    match value {
        rt::PackageTrustState::BootTrusted => "boot-trusted",
        rt::PackageTrustState::Unverified => "unverified",
        rt::PackageTrustState::DigestPinned => "digest-pinned",
        rt::PackageTrustState::VerificationFailed => "verification-failed",
    }
}

fn channel_label(value: rt::PackageChannel) -> &'static str {
    match value {
        rt::PackageChannel::Stable => "stable",
        rt::PackageChannel::Beta => "beta",
        rt::PackageChannel::Canary => "canary",
    }
}

fn ring_label(value: rt::PackageRing) -> &'static str {
    match value {
        rt::PackageRing::Production => "production",
        rt::PackageRing::Preview => "preview",
        rt::PackageRing::Testing => "testing",
    }
}

fn action_label(entry: Option<CatalogEntry>) -> &'static str {
    match entry {
        Some(entry) if entry.installed => "UPDATE",
        Some(_) => "INSTALL",
        None => "INSTALL",
    }
}

fn text_or_dash(bytes: &[u8]) -> &str {
    if bytes.is_empty() {
        "-"
    } else {
        str::from_utf8(bytes).unwrap_or("?")
    }
}

fn service_title(service_id: ServiceId) -> &'static str {
    match service_id {
        ServiceId::Announce => "Announce",
        ServiceId::Runtime => "Runtime Tools",
        ServiceId::Developer => "Developer SDK",
        _ => service_label(service_id),
    }
}

fn service_label(service_id: ServiceId) -> &'static str {
    match service_id {
        ServiceId::RootManager => "root-manager",
        ServiceId::Storage => "storage-service",
        ServiceId::Console => "console-service",
        ServiceId::Config => "config-service",
        ServiceId::Log => "log-service",
        ServiceId::Status => "status-service",
        ServiceId::Shell => "shell-service",
        ServiceId::Package => "package-service",
        ServiceId::Announce => "announce-service",
        ServiceId::Network => "network-service",
        ServiceId::Graphics => "graphics-service",
        ServiceId::Session => "session-service",
        ServiceId::DesktopShell => "desktop-shell-service",
        ServiceId::Terminal => "terminal-service",
        ServiceId::Audio => "audio-service",
        ServiceId::Runtime => "runtime-service",
        ServiceId::Developer => "developer-service",
        ServiceId::Clipboard => "clipboard-service",
    }
}

fn compute_layout(state: &AppState) -> Layout {
    compute_layout_for_dims(
        state.width.min(BUFFER_WIDTH) as i32,
        state.height.min(BUFFER_HEIGHT) as i32,
        selected_entry(state).map(|entry| service_title(entry.service_id)).unwrap_or("Select a package"),
    )
}

fn compute_layout_for_height(height: u32) -> Layout {
    compute_layout_for_dims(BUFFER_WIDTH as i32, height.min(BUFFER_HEIGHT) as i32, "Select a package")
}

fn compute_layout_for_dims(width: i32, height: i32, selected_title: &str) -> Layout {
    let content_top = ui::TITLEBAR_HEIGHT as i32 + OUTER_PAD;
    let header_x = OUTER_PAD;
    let header_y = content_top;
    let header_w = width - OUTER_PAD * 2;
    let body_y = header_y + HEADER_HEIGHT + CONTENT_GAP;
    let body_h = height - body_y - OUTER_PAD;
    let mut left_w = ((header_w - CONTENT_GAP) * 38) / 100;
    left_w = left_w.clamp(300, 388.min(header_w - 220));
    let right_w = header_w - CONTENT_GAP - left_w;
    let left_x = OUTER_PAD;
    let left_y = body_y;
    let right_x = left_x + left_w + CONTENT_GAP;
    let right_y = body_y;
    let detail_title_y = right_y + 40;
    let install_x0 = right_x + right_w - ACTION_BUTTON_WIDTH - 12;
    let install_x1 = install_x0 + ACTION_BUTTON_WIDTH;
    let remove_x0 = install_x0;
    let remove_x1 = install_x1;
    let install_y0 = detail_title_y - 2;
    let install_y1 = install_y0 + BUTTON_HEIGHT;
    let remove_y0 = install_y1 + ACTION_BUTTON_GAP;
    let remove_y1 = remove_y0 + BUTTON_HEIGHT;
    let detail_text_w = (install_x0 - (right_x + 12) - 12).max(64);
    let detail_chip_y = detail_title_y + 34;
    let detail_body_y = detail_chip_y + 26;
    let sync_y0 = header_y + 18;
    let sync_y1 = sync_y0 + BUTTON_HEIGHT;
    let sync_x1 = header_x + header_w - 14;
    let sync_x0 = sync_x1 - 88;
    let list_rows_y = left_y + PANEL_TITLE_HEIGHT + 8;
    let status_y = right_y + body_h - STATUS_BAR_HEIGHT - 12;
    let _ = selected_title;
    Layout {
        header_x,
        header_y,
        header_w,
        left_x,
        left_y,
        left_w,
        left_h: body_h,
        right_x,
        right_y,
        right_w,
        right_h: body_h,
        list_rows_y,
        list_rows_h: body_h - PANEL_TITLE_HEIGHT - 16,
        sync_x0,
        sync_x1,
        sync_y0,
        sync_y1,
        install_x0,
        install_x1,
        install_y0,
        install_y1,
        remove_x0,
        remove_x1,
        remove_y0,
        remove_y1,
        detail_title_y,
        detail_body_y,
        detail_chip_y,
        detail_text_w,
        status_y,
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

fn pointer_action_from_word(value: u64) -> Option<AppPointerAction> {
    Some(match value as u32 {
        x if x == AppPointerAction::Down as u32 => AppPointerAction::Down,
        x if x == AppPointerAction::Move as u32 => AppPointerAction::Move,
        x if x == AppPointerAction::Up as u32 => AppPointerAction::Up,
        x if x == AppPointerAction::Scroll as u32 => AppPointerAction::Scroll,
        _ => return None,
    })
}

fn key_action_from_word(value: u64) -> Option<AppKeyAction> {
    Some(match value as u32 {
        x if x == AppKeyAction::Down as u32 => AppKeyAction::Down,
        x if x == AppKeyAction::Up as u32 => AppKeyAction::Up,
        _ => return None,
    })
}

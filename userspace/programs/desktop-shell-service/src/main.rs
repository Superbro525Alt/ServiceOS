#![no_std]
#![no_main]

mod input;
mod logging;
mod render;
mod requests;
mod windows;

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{
    ControlTag, DesktopAppId, DesktopDragMode, RawMessage, ServiceId, ServiceImageId,
};

const SESSION_ID: u32 = 1;
const APP_COUNT: usize = 5;
const APP_PAGE_SIZE: usize = 4;
const WINDOW_PAGE_SIZE: usize = 2;
const WORKSPACE_COUNT: u32 = 4;
const STATUS_REFRESH_TICKS: u64 = 100;
const TOPBAR_HEIGHT: u32 = 42;
const LAUNCHER_WIDTH: u32 = 250;
const LAUNCHER_HEIGHT: u32 = 144;
const PANEL_MARGIN: u32 = 20;
const STATUS_PANEL_WIDTH: u32 = 280;
const STATUS_PANEL_HEIGHT: u32 = 160;
const WINDOW_MIN_WIDTH: u32 = 280;
const WINDOW_MIN_HEIGHT: u32 = 160;
const RESIZE_GRIP_SIZE: i32 = 20;
const CURSOR_SIZE: u32 = 14;
const CURSOR_Z_ORDER: u32 = 4_096;
const NOTIFICATION_TIMEOUT_TICKS: u64 = 300;
const MAX_NOTIFICATION_BYTES: usize = 96;
const NOTIFICATION_HISTORY_MAX: usize = 12;
const NOTIFICATION_HISTORY_TEXT_MAX: usize = 64;
const CLIPBOARD_HISTORY_LINES: usize = 5;
const PALETTE_QUERY_MAX: usize = 32;
const OVERLAY_RESULT_MAX: usize = 6;
const SWITCHER_WIDTH: u32 = 280;
const SWITCHER_HEIGHT: u32 = 132;
const PALETTE_WIDTH: u32 = 360;
const PALETTE_HEIGHT: u32 = 188;
const HISTORY_WIDTH: u32 = 360;
const HISTORY_HEIGHT: u32 = 188;
const MOD_SHIFT: u32 = 1 << 0;
const MOD_ALT: u32 = 1 << 1;
const MOD_CTRL: u32 = 1 << 2;
const KEY_ESC: u32 = 1;
const KEY_TAB: u32 = 15;
const KEY_BACKSPACE: u32 = 14;
const KEY_ENTER: u32 = 28;
const KEY_SPACE: u32 = 57;
const KEY_V: u32 = 47;
const KEY_N: u32 = 49;
const KEY_UP: u32 = 103;
const KEY_DOWN: u32 = 108;
const KEY_LEFT_ALT: u32 = 56;
const KEY_RIGHT_ALT: u32 = 100;
const KEY_F4: u32 = 62;
const KEY_1: u32 = 2;
const KEY_2: u32 = 3;
const KEY_3: u32 = 4;
const KEY_4: u32 = 5;
const KEY_5: u32 = 6;

#[derive(Clone, Copy)]
struct Chrome {
    desktop_handle: rt::Handle,
    topbar_handle: rt::Handle,
    launcher_handle: rt::Handle,
    status_handle: rt::Handle,
    switcher_handle: rt::Handle,
    palette_handle: rt::Handle,
    notifications_handle: rt::Handle,
    clipboard_handle: rt::Handle,
    cursor_handle: rt::Handle,
    output_width: u32,
    output_height: u32,
}

#[derive(Clone, Copy)]
struct WindowState {
    surface_id: u32,
    surface_handle: rt::Handle,
    control_handle: rt::Handle,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    z_order: u32,
    minimized: bool,
    maximized: bool,
    restore_x: i32,
    restore_y: i32,
    restore_width: u32,
    restore_height: u32,
}

impl WindowState {
    const fn empty() -> Self {
        Self {
            surface_id: 0,
            surface_handle: rt::INVALID_HANDLE,
            control_handle: rt::INVALID_HANDLE,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            z_order: 0,
            minimized: false,
            maximized: false,
            restore_x: 0,
            restore_y: 0,
            restore_width: 0,
            restore_height: 0,
        }
    }

    fn visible(&self) -> bool {
        self.surface_id != 0 && !self.minimized
    }
}

#[derive(Clone, Copy)]
struct AppSlot {
    app_id: DesktopAppId,
    image_id: ServiceImageId,
    task_handle: rt::Handle,
    window: WindowState,
    running: bool,
    workspace_id: u32,
    launch_count: u32,
}

impl AppSlot {
    const fn new(app_id: DesktopAppId, image_id: ServiceImageId) -> Self {
        Self {
            app_id,
            image_id,
            task_handle: rt::INVALID_HANDLE,
            window: WindowState::empty(),
            running: false,
            workspace_id: 1,
            launch_count: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DesktopStatusSnapshot {
    running_apps: u32,
    focused_app: Option<DesktopAppId>,
    active_workspace: u32,
    heartbeat_count: u64,
    heartbeat_tick: u64,
    ipv4_address: u32,
    notification_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OverlayMode {
    None,
    Switcher,
    CommandPalette,
    Notifications,
    ClipboardHistory,
}

#[derive(Clone, Copy)]
struct NotificationEntry {
    occupied: bool,
    sequence: u32,
    source_app: Option<DesktopAppId>,
    actionable: bool,
    text_len: usize,
    text: [u8; NOTIFICATION_HISTORY_TEXT_MAX],
}

impl NotificationEntry {
    const fn empty() -> Self {
        Self {
            occupied: false,
            sequence: 0,
            source_app: None,
            actionable: false,
            text_len: 0,
            text: [0; NOTIFICATION_HISTORY_TEXT_MAX],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DragState {
    Move {
        app_id: DesktopAppId,
        grab_offset_x: i32,
        grab_offset_y: i32,
    },
    Resize {
        app_id: DesktopAppId,
        edges: ResizeEdges,
        origin_pointer_x: i32,
        origin_pointer_y: i32,
        start_x: i32,
        start_y: i32,
        start_width: u32,
        start_height: u32,
    },
}

impl DragState {
    fn mode(self) -> DesktopDragMode {
        match self {
            Self::Move { .. } => DesktopDragMode::Move,
            Self::Resize { .. } => DesktopDragMode::Resize,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ResizeEdges(u8);

impl ResizeEdges {
    const NONE: Self = Self(0);
    const LEFT: Self = Self(1 << 0);
    const RIGHT: Self = Self(1 << 1);
    const TOP: Self = Self(1 << 2);
    const BOTTOM: Self = Self(1 << 3);

    fn contains(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl core::ops::BitOrAssign for ResizeEdges {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

enum HitTarget {
    Background,
    Launcher(DesktopAppId),
    WindowContent(DesktopAppId),
    WindowMove {
        app_id: DesktopAppId,
        grab_offset_x: i32,
        grab_offset_y: i32,
    },
    WindowResize {
        app_id: DesktopAppId,
        edges: ResizeEdges,
    },
    WindowClose(DesktopAppId),
    WindowMinimize(DesktopAppId),
    WindowMaximize(DesktopAppId),
}

#[derive(Clone, Copy)]
struct ContentCapture {
    app_id: DesktopAppId,
    button: u32,
}

struct DesktopState {
    bootstrap: rt::Handle,
    log_handle: rt::Handle,
    graphics_handle: rt::Handle,
    session_handle: rt::Handle,
    network_handle: rt::Handle,
    system_status_handle: rt::Handle,
    clipboard_service_handle: rt::Handle,
    chrome: Chrome,
    apps: [AppSlot; APP_COUNT],
    focused_app: Option<DesktopAppId>,
    active_workspace: u32,
    recent_focus: [DesktopAppId; APP_COUNT],
    recent_focus_len: usize,
    next_status_refresh: u64,
    last_status_snapshot: Option<DesktopStatusSnapshot>,
    next_z_order: u32,
    pointer_x: i32,
    pointer_y: i32,
    drag_state: Option<DragState>,
    content_capture: Option<ContentCapture>,
    notification: [u8; MAX_NOTIFICATION_BYTES],
    notification_len: usize,
    notification_deadline: u64,
    notification_history: [NotificationEntry; NOTIFICATION_HISTORY_MAX],
    notification_history_len: usize,
    next_notification_sequence: u32,
    overlay_mode: OverlayMode,
    overlay_selection: usize,
    palette_query: [u8; PALETTE_QUERY_MAX],
    palette_query_len: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PaletteAction {
    Launch(DesktopAppId),
    ShowNotifications,
    ShowClipboardHistory,
    SwitchWorkspace(u32),
    FocusNext,
}

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xfe01;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 1 {
        return 0xfe02;
    }

    let log_handle = startup.handles[0];
    let graphics_handle = match rt::lookup_service(bootstrap, ServiceId::Graphics) {
        Ok(handle) => handle,
        Err(_) => return 0xfe03,
    };
    let session_handle = match rt::lookup_service(bootstrap, ServiceId::Session) {
        Ok(handle) => handle,
        Err(_) => return 0xfe04,
    };
    let network_handle = match rt::lookup_service(bootstrap, ServiceId::Network) {
        Ok(handle) => handle,
        Err(_) => return 0xfe05,
    };
    let system_status_handle = match rt::lookup_service(bootstrap, ServiceId::Status) {
        Ok(handle) => handle,
        Err(_) => return 0xfe06,
    };
    let clipboard_service_handle =
        rt::lookup_service(bootstrap, ServiceId::Clipboard).unwrap_or(rt::INVALID_HANDLE);

    let output = match rt::graphics_output_status(graphics_handle, 0) {
        Ok(Some(output)) => output,
        _ => return 0xfe07,
    };
    let chrome = match create_chrome(graphics_handle, output.width, output.height) {
        Ok(chrome) => chrome,
        Err(_) => return 0xfe08,
    };

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xfe09,
    };
    if rt::register_service(bootstrap, ServiceId::DesktopShell, public.second).is_err() {
        return 0xfe0a;
    }
    let _ = rt::handle_close(public.second);

    let mut state = DesktopState {
        bootstrap,
        log_handle,
        graphics_handle,
        session_handle,
        network_handle,
        system_status_handle,
        clipboard_service_handle,
        chrome,
        apps: [
            AppSlot::new(DesktopAppId::Settings, ServiceImageId::SettingsApp),
            AppSlot::new(DesktopAppId::Files, ServiceImageId::FilesApp),
            AppSlot::new(DesktopAppId::Monitor, ServiceImageId::MonitorApp),
            AppSlot::new(DesktopAppId::Terminal, ServiceImageId::TerminalApp),
            AppSlot::new(DesktopAppId::SoftwareCenter, ServiceImageId::SoftwareCenterApp),
        ],
        focused_app: None,
        active_workspace: 1,
        recent_focus: [DesktopAppId::Settings; APP_COUNT],
        recent_focus_len: 0,
        next_status_refresh: 0,
        last_status_snapshot: None,
        next_z_order: 10,
        pointer_x: (output.width / 2) as i32,
        pointer_y: (output.height / 2) as i32,
        drag_state: None,
        content_capture: None,
        notification: [0; MAX_NOTIFICATION_BYTES],
        notification_len: 0,
        notification_deadline: 0,
        notification_history: [NotificationEntry::empty(); NOTIFICATION_HISTORY_MAX],
        notification_history_len: 0,
        next_notification_sequence: 1,
        overlay_mode: OverlayMode::None,
        overlay_selection: 0,
        palette_query: [0; PALETTE_QUERY_MAX],
        palette_query_len: 0,
    };

    if render::render_desktop(&mut state).is_err() {
        return 0xfe0b;
    }
    if render::sync_cursor(&state).is_err() {
        return 0xfe14;
    }
    if show_chrome(&state.chrome).is_err() {
        return 0xfe0c;
    }
    let _ = logging::emit_log(
        state.log_handle,
        rt::LogSeverity::Info,
        rt::LogEvent::DesktopReady,
        SESSION_ID as u64,
        output.width as u64,
    );

    loop {
        let mut did_work = false;
        match requests::poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xfe0d,
        }

        loop {
            let mut request = RawMessage::empty(0);
            match rt::channel_receive_nonblocking(public.first, &mut request) {
                Ok(()) => {
                    did_work = true;
                    if requests::handle_request(&mut state, &request).is_err() {
                        return 0xfe0e;
                    }
                }
                Err(rt::Error::QueueEmpty) => break,
                Err(_) => return 0xfe0f,
            }
        }

        if windows::refresh_apps(&mut state).is_err() {
            return 0xfe10;
        }

        let now = match rt::monotonic_now() {
            Ok(now) => now,
            Err(_) => return 0xfe11,
        };
        if state.notification_len != 0 && now >= state.notification_deadline {
            state.notification_len = 0;
            if render::render_desktop(&mut state).is_err() {
                return 0xfe15;
            }
        }
        if now >= state.next_status_refresh {
            if render::refresh_desktop_status(&mut state).is_err() {
                return 0xfe12;
            }
            state.next_status_refresh = now.saturating_add(STATUS_REFRESH_TICKS);
        }

        if did_work {
            continue;
        }

        if rt::yield_current().is_err() {
            return 0xfe13;
        }
    }
}

fn create_chrome(
    graphics_handle: rt::Handle,
    output_width: u32,
    output_height: u32,
) -> rt::Result<Chrome> {
    let (_, desktop_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        0,
        0,
        output_width,
        output_height,
        0,
        ui::BG_DESKTOP,
        false,
    )?;
    let (_, topbar_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        0,
        0,
        output_width,
        TOPBAR_HEIGHT,
        1,
        ui::BG_PANEL,
        false,
    )?;
    let (_, launcher_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        PANEL_MARGIN as i32,
        (TOPBAR_HEIGHT + PANEL_MARGIN) as i32,
        LAUNCHER_WIDTH,
        264,
        2,
        ui::BG_PANEL,
        false,
    )?;
    let (_, status_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        (output_width.saturating_sub(STATUS_PANEL_WIDTH + PANEL_MARGIN)) as i32,
        (TOPBAR_HEIGHT + PANEL_MARGIN) as i32,
        STATUS_PANEL_WIDTH,
        STATUS_PANEL_HEIGHT,
        2,
        ui::BG_PANEL,
        false,
    )?;
    let (_, switcher_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        ((output_width.saturating_sub(SWITCHER_WIDTH)) / 2) as i32,
        ((output_height.saturating_sub(SWITCHER_HEIGHT)) / 2) as i32,
        SWITCHER_WIDTH,
        SWITCHER_HEIGHT,
        CURSOR_Z_ORDER - 3,
        ui::BG_PANEL,
        false,
    )?;
    let (_, palette_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        ((output_width.saturating_sub(PALETTE_WIDTH)) / 2) as i32,
        72,
        PALETTE_WIDTH,
        PALETTE_HEIGHT,
        CURSOR_Z_ORDER - 3,
        ui::BG_PANEL,
        false,
    )?;
    let (_, notifications_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        (output_width.saturating_sub(HISTORY_WIDTH + PANEL_MARGIN)) as i32,
        (TOPBAR_HEIGHT + PANEL_MARGIN + STATUS_PANEL_HEIGHT + 12) as i32,
        HISTORY_WIDTH,
        HISTORY_HEIGHT,
        CURSOR_Z_ORDER - 3,
        ui::BG_PANEL,
        false,
    )?;
    let (_, clipboard_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        (output_width.saturating_sub(HISTORY_WIDTH + PANEL_MARGIN)) as i32,
        (TOPBAR_HEIGHT + PANEL_MARGIN) as i32,
        HISTORY_WIDTH,
        HISTORY_HEIGHT,
        CURSOR_Z_ORDER - 3,
        ui::BG_PANEL,
        false,
    )?;
    let (_, cursor_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        (output_width / 2) as i32,
        (output_height / 2) as i32,
        CURSOR_SIZE,
        CURSOR_SIZE,
        CURSOR_Z_ORDER,
        0,
        false,
    )?;
    ui::render_cursor(cursor_handle, CURSOR_SIZE)?;

    Ok(Chrome {
        desktop_handle,
        topbar_handle,
        launcher_handle,
        status_handle,
        switcher_handle,
        palette_handle,
        notifications_handle,
        clipboard_handle,
        cursor_handle,
        output_width,
        output_height,
    })
}

fn show_chrome(chrome: &Chrome) -> rt::Result<()> {
    rt::surface_set_visibility(chrome.desktop_handle, true)?;
    rt::surface_set_visibility(chrome.topbar_handle, true)?;
    rt::surface_set_visibility(chrome.launcher_handle, true)?;
    rt::surface_set_visibility(chrome.status_handle, true)?;
    rt::surface_set_visibility(chrome.switcher_handle, false)?;
    rt::surface_set_visibility(chrome.palette_handle, false)?;
    rt::surface_set_visibility(chrome.notifications_handle, false)?;
    rt::surface_set_visibility(chrome.clipboard_handle, false)?;
    rt::surface_set_visibility(chrome.cursor_handle, true)?;
    Ok(())
}

fn palette_matches(state: &DesktopState, results: &mut [PaletteAction; OVERLAY_RESULT_MAX]) -> usize {
    let query = core::str::from_utf8(&state.palette_query[..state.palette_query_len]).unwrap_or("");
    let actions = [
        PaletteAction::Launch(DesktopAppId::Settings),
        PaletteAction::Launch(DesktopAppId::Files),
        PaletteAction::Launch(DesktopAppId::Monitor),
        PaletteAction::Launch(DesktopAppId::Terminal),
        PaletteAction::Launch(DesktopAppId::SoftwareCenter),
        PaletteAction::ShowNotifications,
        PaletteAction::ShowClipboardHistory,
        PaletteAction::SwitchWorkspace(1),
        PaletteAction::SwitchWorkspace(2),
        PaletteAction::SwitchWorkspace(3),
        PaletteAction::SwitchWorkspace(4),
        PaletteAction::FocusNext,
    ];
    let mut ranked = [(PaletteAction::ShowNotifications, 0u32); 12];
    let mut ranked_len = 0usize;
    for action in actions {
        let label = palette_action_label(action);
        let matches = query.is_empty() || contains_case_fold(label, query);
        if !matches {
            continue;
        }
        let mut score = 1u32;
        if query.is_empty() {
            score = score.saturating_add(1);
        }
        if starts_with_case_fold(label, query) {
            score = score.saturating_add(32);
        }
        if let PaletteAction::Launch(app_id) = action {
            if let Some(index) = windows::app_slot_index(&state.apps, app_id) {
                score = score.saturating_add(state.apps[index].launch_count.saturating_mul(2));
                if state.focused_app == Some(app_id) {
                    score = score.saturating_add(24);
                }
                if state.apps[index].running {
                    score = score.saturating_add(8);
                }
            }
            if let Some(position) = state.recent_focus[..state.recent_focus_len]
                .iter()
                .position(|candidate| *candidate == app_id)
            {
                score = score.saturating_add((APP_COUNT - position) as u32 * 6);
            }
        }
        ranked[ranked_len] = (action, score);
        ranked_len += 1;
    }

    let mut index = 1usize;
    while index < ranked_len {
        let current = ranked[index];
        let mut scan = index;
        while scan > 0 && ranked[scan - 1].1 < current.1 {
            ranked[scan] = ranked[scan - 1];
            scan -= 1;
        }
        ranked[scan] = current;
        index += 1;
    }

    let count = ranked_len.min(OVERLAY_RESULT_MAX);
    for index in 0..count {
        results[index] = ranked[index].0;
    }
    count
}

fn palette_action_label(action: PaletteAction) -> &'static str {
    match action {
        PaletteAction::Launch(DesktopAppId::Settings) => "Open Settings",
        PaletteAction::Launch(DesktopAppId::Files) => "Open Files",
        PaletteAction::Launch(DesktopAppId::Monitor) => "Open Monitor",
        PaletteAction::Launch(DesktopAppId::Terminal) => "Open Terminal",
        PaletteAction::Launch(DesktopAppId::SoftwareCenter) => "Open Software Center",
        PaletteAction::ShowNotifications => "Show Notification History",
        PaletteAction::ShowClipboardHistory => "Show Clipboard History",
        PaletteAction::SwitchWorkspace(1) => "Switch to Workspace 1",
        PaletteAction::SwitchWorkspace(2) => "Switch to Workspace 2",
        PaletteAction::SwitchWorkspace(3) => "Switch to Workspace 3",
        PaletteAction::SwitchWorkspace(4) => "Switch to Workspace 4",
        PaletteAction::FocusNext => "Focus Next Window",
        PaletteAction::SwitchWorkspace(_) => "Switch Workspace",
    }
}

fn contains_case_fold(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    if needle.len() > haystack.len() {
        return false;
    }
    for start in 0..=haystack.len() - needle.len() {
        if haystack[start..start + needle.len()]
            .iter()
            .zip(needle.iter())
            .all(|(left, right)| left.to_ascii_lowercase() == right.to_ascii_lowercase())
        {
            return true;
        }
    }
    false
}

fn starts_with_case_fold(haystack: &str, needle: &str) -> bool {
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    if needle.len() > haystack.len() {
        return false;
    }
    haystack[..needle.len()]
        .iter()
        .zip(needle.iter())
        .all(|(left, right)| left.to_ascii_lowercase() == right.to_ascii_lowercase())
}

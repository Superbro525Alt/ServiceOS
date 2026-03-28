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
const APP_COUNT: usize = 4;
const WINDOW_PAGE_SIZE: usize = 2;
const STATUS_REFRESH_TICKS: u64 = 100;
const TOPBAR_HEIGHT: u32 = 42;
const LAUNCHER_WIDTH: u32 = 250;
const PANEL_MARGIN: u32 = 20;
const STATUS_PANEL_WIDTH: u32 = 280;
const STATUS_PANEL_HEIGHT: u32 = 160;
const WINDOW_MIN_WIDTH: u32 = 280;
const WINDOW_MIN_HEIGHT: u32 = 160;
const RESIZE_GRIP_SIZE: i32 = 20;
const CURSOR_SIZE: u32 = 14;
const CURSOR_Z_ORDER: u32 = 4_096;
const MOD_ALT: u32 = 1 << 1;
const KEY_TAB: u32 = 15;
const KEY_F4: u32 = 62;

#[derive(Clone, Copy)]
struct Chrome {
    desktop_handle: rt::Handle,
    topbar_handle: rt::Handle,
    launcher_handle: rt::Handle,
    status_handle: rt::Handle,
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
}

impl AppSlot {
    const fn new(app_id: DesktopAppId, image_id: ServiceImageId) -> Self {
        Self {
            app_id,
            image_id,
            task_handle: rt::INVALID_HANDLE,
            window: WindowState::empty(),
            running: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DesktopStatusSnapshot {
    running_apps: u32,
    focused_app: Option<DesktopAppId>,
    heartbeat_count: u64,
    heartbeat_tick: u64,
    ipv4_address: u32,
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
    chrome: Chrome,
    apps: [AppSlot; APP_COUNT],
    focused_app: Option<DesktopAppId>,
    next_status_refresh: u64,
    last_status_snapshot: Option<DesktopStatusSnapshot>,
    next_z_order: u32,
    pointer_x: i32,
    pointer_y: i32,
    drag_state: Option<DragState>,
    content_capture: Option<ContentCapture>,
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
        chrome,
        apps: [
            AppSlot::new(DesktopAppId::Settings, ServiceImageId::SettingsApp),
            AppSlot::new(DesktopAppId::Files, ServiceImageId::FilesApp),
            AppSlot::new(DesktopAppId::Monitor, ServiceImageId::MonitorApp),
            AppSlot::new(DesktopAppId::Terminal, ServiceImageId::TerminalApp),
        ],
        focused_app: None,
        next_status_refresh: 0,
        last_status_snapshot: None,
        next_z_order: 10,
        pointer_x: (output.width / 2) as i32,
        pointer_y: (output.height / 2) as i32,
        drag_state: None,
        content_capture: None,
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
    rt::surface_set_visibility(chrome.cursor_handle, true)?;
    Ok(())
}

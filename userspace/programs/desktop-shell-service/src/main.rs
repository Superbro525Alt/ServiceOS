#![no_std]
#![no_main]

use core::{char, fmt::Write, str};

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{
    ControlTag, DesktopAppId, DesktopDragMode, DesktopInputAction, DesktopStatus, DesktopTag,
    DesktopWindowAction, FixedLogBuffer, LifecycleEvent, LogDomain, LogEvent, LogSeverity,
    RawMessage, ServiceId, ServiceImageId, StartupHandle,
};

const SESSION_ID: u32 = 1;
const APP_COUNT: usize = 3;
const WINDOW_PAGE_SIZE: usize = 2;
const STATUS_REFRESH_TICKS: u64 = 100;
const TOPBAR_HEIGHT: u32 = 42;
const LAUNCHER_WIDTH: u32 = 250;
const PANEL_MARGIN: u32 = 20;
const STATUS_PANEL_WIDTH: u32 = 280;
const STATUS_PANEL_HEIGHT: u32 = 160;
const LAUNCHER_ITEM_START_Y: i32 = 54;
const LAUNCHER_ITEM_STEP: i32 = 18;
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

    if render_desktop(&mut state).is_err() {
        return 0xfe0b;
    }
    if sync_cursor(&state).is_err() {
        return 0xfe14;
    }
    if show_chrome(&state.chrome).is_err() {
        return 0xfe0c;
    }
    let _ = emit_log(
        state.log_handle,
        LogSeverity::Info,
        LogEvent::DesktopReady,
        SESSION_ID as u64,
        output.width as u64,
    );

    let _ = launch_or_focus_app(&mut state, DesktopAppId::Monitor);

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xfe0d,
        }

        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                if handle_request(&mut state, &request).is_err() {
                    return 0xfe0e;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xfe0f,
        }

        if refresh_apps(&mut state).is_err() {
            return 0xfe10;
        }

        let now = match rt::monotonic_now() {
            Ok(now) => now,
            Err(_) => return 0xfe11,
        };
        if now >= state.next_status_refresh {
            if refresh_desktop_status(&mut state).is_err() {
                return 0xfe12;
            }
            state.next_status_refresh = now.saturating_add(STATUS_REFRESH_TICKS);
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

fn handle_request(state: &mut DesktopState, request: &RawMessage) -> rt::Result<()> {
    match request.tag {
        x if x == DesktopTag::StatusRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(DesktopTag::StatusReply as u32);
            reply.word_count = 7;
            reply.words[0] = DesktopStatus::Ok as u32 as u64;
            reply.words[1] = SESSION_ID as u64;
            reply.words[2] = state.focused_app.map(|app| app as u32 as u64).unwrap_or(0);
            reply.words[3] = running_app_count(&state.apps) as u64;
            reply.words[4] = focused_surface_id(state) as u64;
            reply.words[5] = state.drag_state.map(|drag| drag.mode()).unwrap_or(DesktopDragMode::None)
                as u32 as u64;
            reply.words[6] = pack_i32_pair(state.pointer_x, state.pointer_y);
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == DesktopTag::ListAppsRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(DesktopTag::ListAppsReply as u32);
            reply.word_count = 2 + (state.apps.len() as u32 * 4);
            reply.words[0] = DesktopStatus::Ok as u32 as u64;
            reply.words[1] = state.apps.len() as u64;
            for (index, slot) in state.apps.iter().copied().enumerate() {
                let base = 2 + index * 4;
                reply.words[base] = slot.app_id as u32 as u64;
                reply.words[base + 1] = u64::from(slot.running);
                reply.words[base + 2] = u64::from(state.focused_app == Some(slot.app_id));
                reply.words[base + 3] = slot.window.surface_id as u64;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == DesktopTag::LaunchAppRequest as u32 => {
            if request.word_count < 1 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(DesktopTag::LaunchAppReply as u32);
            reply.word_count = 2;
            match desktop_app_from_word(request.words[0]) {
                Some(app_id) => reply_for_surface(&mut reply, launch_or_focus_app(state, app_id)),
                None => reply.words[0] = DesktopStatus::NotFound as u32 as u64,
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == DesktopTag::FocusAppRequest as u32 => {
            if request.word_count < 1 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(DesktopTag::FocusAppReply as u32);
            reply.word_count = 2;
            match desktop_app_from_word(request.words[0]) {
                Some(app_id) => reply_for_surface(&mut reply, focus_app(state, app_id)),
                None => reply.words[0] = DesktopStatus::NotFound as u32 as u64,
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == DesktopTag::ListWindowsRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let start = request.words[0] as usize;
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(DesktopTag::ListWindowsReply as u32);
            reply.word_count = 3;
            reply.words[0] = DesktopStatus::Ok as u32 as u64;
            encode_window_page(state, start, &mut reply);
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == DesktopTag::WindowActionRequest as u32 => {
            if request.word_count < 4 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(DesktopTag::WindowActionReply as u32);
            reply.word_count = 2;
            let action = desktop_window_action_from_word(request.words[0]);
            let app_id = desktop_app_from_word(request.words[1]);
            let result = match action {
                Some(DesktopWindowAction::Focus) => app_id
                    .ok_or(rt::Error::NotFound)
                    .and_then(|app| focus_app(state, app)),
                Some(DesktopWindowAction::Close) => app_id
                    .ok_or(rt::Error::NotFound)
                    .and_then(|app| close_app(state, app).map(|_| 0)),
                Some(DesktopWindowAction::Minimize) => app_id
                    .ok_or(rt::Error::NotFound)
                    .and_then(|app| minimize_app(state, app)),
                Some(DesktopWindowAction::Restore) => app_id
                    .ok_or(rt::Error::NotFound)
                    .and_then(|app| restore_app(state, app)),
                Some(DesktopWindowAction::Move) => app_id.ok_or(rt::Error::NotFound).and_then(|app| {
                    move_app(
                        state,
                        app,
                        request.words[2] as i64 as i32,
                        request.words[3] as i64 as i32,
                    )
                }),
                Some(DesktopWindowAction::Resize) => app_id.ok_or(rt::Error::NotFound).and_then(|app| {
                    resize_app(
                        state,
                        app,
                        request.words[2] as u32,
                        request.words[3] as u32,
                    )
                }),
                Some(DesktopWindowAction::Maximize) => app_id
                    .ok_or(rt::Error::NotFound)
                    .and_then(|app| maximize_app(state, app)),
                Some(DesktopWindowAction::FocusNext) => focus_next_app(state),
                None => Err(rt::Error::NotFound),
            };
            reply_for_surface(&mut reply, result);
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == DesktopTag::InputRequest as u32 => {
            if request.word_count < 3 || request.handle_count < 1 {
                return Ok(());
            }
            let action = desktop_input_action_from_word(request.words[0]);
            let x = request.words[1] as i64 as i32;
            let y = request.words[2] as i64 as i32;
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(DesktopTag::InputReply as u32);
            reply.word_count = 2;
            let result = match action {
                Some(action) => handle_input(state, action, x, y),
                None => Err(rt::Error::NotFound),
            };
            reply_for_surface(&mut reply, result);
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        _ => {}
    }

    Ok(())
}

fn reply_for_surface(reply: &mut RawMessage, result: rt::Result<u32>) {
    match result {
        Ok(surface_id) => {
            reply.words[0] = DesktopStatus::Ok as u32 as u64;
            reply.words[1] = surface_id as u64;
        }
        Err(rt::Error::PermissionDenied) => reply.words[0] = DesktopStatus::Denied as u32 as u64,
        Err(rt::Error::NotFound) => reply.words[0] = DesktopStatus::NotFound as u32 as u64,
        Err(_) => reply.words[0] = DesktopStatus::Busy as u32 as u64,
    }
}

fn launch_or_focus_app(state: &mut DesktopState, app_id: DesktopAppId) -> rt::Result<u32> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if state.apps[index].running {
        if state.apps[index].window.minimized {
            return restore_app(state, app_id);
        }
        return focus_app(state, app_id);
    }

    let (x, y, width, height, fill_rgb) = initial_window_layout(state.chrome.output_width, app_id);
    let z_order = allocate_z_order(state);
    let (surface_id, surface_handle) = rt::graphics_surface_create(
        state.graphics_handle,
        SESSION_ID,
        x,
        y,
        width,
        height,
        z_order,
        fill_rgb,
        false,
    )?;
    let surface_transfer = rt::handle_duplicate(
        surface_handle,
        rt::rights::SEND | rt::rights::RECEIVE | rt::rights::DUPLICATE | rt::rights::TRANSFER,
    )?;
    let control = rt::channel_create()?;

    let task_handle = rt::manager_launch_program_with_payload(
        state.bootstrap,
        state.apps[index].image_id,
        &[surface_id as u64, width as u64, height as u64, 1],
        &[
            StartupHandle {
                handle: surface_transfer,
                rights: rt::rights::SEND
                    | rt::rights::RECEIVE
                    | rt::rights::DUPLICATE
                    | rt::rights::TRANSFER,
            },
            StartupHandle {
                handle: control.second,
                rights: rt::rights::SEND
                    | rt::rights::RECEIVE
                    | rt::rights::DUPLICATE
                    | rt::rights::TRANSFER,
            },
        ],
    )?;
    let _ = rt::handle_close(surface_transfer);
    let _ = rt::handle_close(control.second);

    state.apps[index].task_handle = task_handle;
    state.apps[index].window = WindowState {
        surface_id,
        surface_handle,
        control_handle: control.first,
        x,
        y,
        width,
        height,
        z_order,
        minimized: false,
        maximized: false,
        restore_x: x,
        restore_y: y,
        restore_width: width,
        restore_height: height,
    };
    state.apps[index].running = true;
    sync_window_surface(&state.apps[index])?;
    let _ = rt::app_control_resize(control.first, width, height);
    let surface_id = focus_app(state, app_id)?;
    let _ = emit_log(
        state.log_handle,
        LogSeverity::Info,
        LogEvent::DesktopAppLaunched,
        app_id as u32 as u64,
        surface_id as u64,
    );
    Ok(surface_id)
}

fn focus_app(state: &mut DesktopState, app_id: DesktopAppId) -> rt::Result<u32> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if !state.apps[index].running || state.apps[index].window.surface_id == 0 {
        return Err(rt::Error::NotFound);
    }
    if state.apps[index].window.minimized {
        state.apps[index].window.minimized = false;
        sync_window_surface(&state.apps[index])?;
    }

    if let Some(previous) = state.focused_app {
        if previous != app_id {
            if let Some(previous_index) = app_slot_index(&state.apps, previous) {
                let previous_control = state.apps[previous_index].window.control_handle;
                if previous_control != rt::INVALID_HANDLE {
                    let _ = rt::app_control_focus(previous_control, false);
                }
            }
        }
    }

    state.apps[index].window.z_order = allocate_z_order(state);
    apply_window_geometry(&state.apps[index])?;
    let control_handle = state.apps[index].window.control_handle;
    if control_handle != rt::INVALID_HANDLE {
        let _ = rt::app_control_focus(control_handle, true);
    }
    let surface_id = state.apps[index].window.surface_id;
    let _ = rt::session_focus(state.session_handle, SESSION_ID, surface_id)?;
    state.focused_app = Some(app_id);
    let _ = emit_log(
        state.log_handle,
        LogSeverity::Info,
        LogEvent::DesktopFocusChanged,
        app_id as u32 as u64,
        surface_id as u64,
    );
    render_desktop(state)?;
    Ok(surface_id)
}

fn minimize_app(state: &mut DesktopState, app_id: DesktopAppId) -> rt::Result<u32> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if !state.apps[index].running {
        return Err(rt::Error::NotFound);
    }
    state.apps[index].window.minimized = true;
    sync_window_surface(&state.apps[index])?;
    if state.focused_app == Some(app_id) {
        state.focused_app = None;
        let _ = rt::session_focus(state.session_handle, SESSION_ID, 0);
        let _ = focus_next_visible_without_cycle(state);
    }
    let _ = emit_text_log(
        "desktop",
        format_args!("window minimized app={}", app_title(app_id)),
    );
    render_desktop(state)?;
    Ok(state.apps[index].window.surface_id)
}

fn restore_app(state: &mut DesktopState, app_id: DesktopAppId) -> rt::Result<u32> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if !state.apps[index].running {
        return Err(rt::Error::NotFound);
    }
    if state.apps[index].window.maximized {
        let restore_x = state.apps[index].window.restore_x;
        let restore_y = state.apps[index].window.restore_y;
        let restore_width = state.apps[index].window.restore_width.max(WINDOW_MIN_WIDTH);
        let restore_height = state.apps[index].window.restore_height.max(WINDOW_MIN_HEIGHT);
        state.apps[index].window.x =
            clamp_window_x(state.chrome.output_width, restore_width, restore_x);
        state.apps[index].window.y =
            clamp_window_y(state.chrome.output_height, restore_height, restore_y);
        state.apps[index].window.width = restore_width;
        state.apps[index].window.height = restore_height;
        state.apps[index].window.maximized = false;
        apply_window_geometry(&state.apps[index])?;
        if state.apps[index].window.control_handle != rt::INVALID_HANDLE {
            let _ = rt::app_control_resize(
                state.apps[index].window.control_handle,
                restore_width,
                restore_height,
            );
        }
    }
    state.apps[index].window.minimized = false;
    sync_window_surface(&state.apps[index])?;
    focus_app(state, app_id)
}

fn maximize_app(state: &mut DesktopState, app_id: DesktopAppId) -> rt::Result<u32> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if !state.apps[index].running {
        return Err(rt::Error::NotFound);
    }
    if state.apps[index].window.maximized {
        return restore_app(state, app_id);
    }

    state.apps[index].window.restore_x = state.apps[index].window.x;
    state.apps[index].window.restore_y = state.apps[index].window.y;
    state.apps[index].window.restore_width = state.apps[index].window.width;
    state.apps[index].window.restore_height = state.apps[index].window.height;
    state.apps[index].window.maximized = true;
    state.apps[index].window.minimized = false;
    state.apps[index].window.x = PANEL_MARGIN as i32;
    state.apps[index].window.y = (TOPBAR_HEIGHT + PANEL_MARGIN) as i32;
    state.apps[index].window.width = state
        .chrome
        .output_width
        .saturating_sub(PANEL_MARGIN * 2);
    state.apps[index].window.height = state
        .chrome
        .output_height
        .saturating_sub(TOPBAR_HEIGHT + PANEL_MARGIN * 2);
    apply_window_geometry(&state.apps[index])?;
    if state.apps[index].window.control_handle != rt::INVALID_HANDLE {
        let _ = rt::app_control_resize(
            state.apps[index].window.control_handle,
            state.apps[index].window.width,
            state.apps[index].window.height,
        );
    }
    render_desktop(state)?;
    focus_app(state, app_id)
}

fn move_app(state: &mut DesktopState, app_id: DesktopAppId, x: i32, y: i32) -> rt::Result<u32> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if !state.apps[index].running {
        return Err(rt::Error::NotFound);
    }
    state.apps[index].window.maximized = false;
    state.apps[index].window.x = clamp_window_x(state.chrome.output_width, state.apps[index].window.width, x);
    state.apps[index].window.y = clamp_window_y(state.chrome.output_height, state.apps[index].window.height, y);
    apply_window_geometry(&state.apps[index])?;
    render_desktop(state)?;
    let _ = emit_text_log(
        "desktop",
        format_args!(
            "window moved app={} x={} y={}",
            app_title(app_id),
            state.apps[index].window.x,
            state.apps[index].window.y
        ),
    );
    Ok(state.apps[index].window.surface_id)
}

fn resize_app(
    state: &mut DesktopState,
    app_id: DesktopAppId,
    width: u32,
    height: u32,
) -> rt::Result<u32> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if !state.apps[index].running {
        return Err(rt::Error::NotFound);
    }
    state.apps[index].window.maximized = false;
    let width = width.clamp(WINDOW_MIN_WIDTH, state.chrome.output_width.saturating_sub(PANEL_MARGIN));
    let height = height.clamp(
        WINDOW_MIN_HEIGHT,
        state.chrome
            .output_height
            .saturating_sub(TOPBAR_HEIGHT + PANEL_MARGIN),
    );
    state.apps[index].window.width = width;
    state.apps[index].window.height = height;
    state.apps[index].window.x =
        clamp_window_x(state.chrome.output_width, width, state.apps[index].window.x);
    state.apps[index].window.y =
        clamp_window_y(state.chrome.output_height, height, state.apps[index].window.y);
    apply_window_geometry(&state.apps[index])?;
    let control_handle = state.apps[index].window.control_handle;
    if control_handle != rt::INVALID_HANDLE {
        let _ = rt::app_control_resize(control_handle, width, height);
    }
    render_desktop(state)?;
    let _ = emit_text_log(
        "desktop",
        format_args!("window resized app={} size={}x{}", app_title(app_id), width, height),
    );
    Ok(state.apps[index].window.surface_id)
}

fn close_app(state: &mut DesktopState, app_id: DesktopAppId) -> rt::Result<()> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if !state.apps[index].running {
        return Err(rt::Error::NotFound);
    }
    let control_handle = state.apps[index].window.control_handle;
    if control_handle != rt::INVALID_HANDLE {
        let _ = rt::app_control_close(control_handle);
    }
    let _ = emit_text_log("desktop", format_args!("window close requested app={}", app_title(app_id)));
    Ok(())
}

fn refresh_apps(state: &mut DesktopState) -> rt::Result<()> {
    let mut changed = false;
    for slot in &mut state.apps {
        if !slot.running || slot.task_handle == rt::INVALID_HANDLE {
            continue;
        }
        let status = rt::task_status(slot.task_handle)?;
        if status.state != rt::TaskStateCode::Exited {
            continue;
        }
        let exited_app = slot.app_id;
        let exited_surface = slot.window.surface_id;
        let exit_code = status.exit_code;
        if slot.window.surface_handle != rt::INVALID_HANDLE {
            let _ = rt::surface_close(slot.window.surface_handle);
            let _ = rt::handle_close(slot.window.surface_handle);
        }
        if slot.window.control_handle != rt::INVALID_HANDLE {
            let _ = rt::handle_close(slot.window.control_handle);
        }
        let _ = rt::handle_close(slot.task_handle);
        slot.task_handle = rt::INVALID_HANDLE;
        slot.window = WindowState::empty();
        slot.running = false;
        if state.focused_app == Some(exited_app) {
            state.focused_app = None;
            let _ = rt::session_focus(state.session_handle, SESSION_ID, 0);
        }
        let _ = emit_log(
            state.log_handle,
            LogSeverity::Warn,
            LogEvent::DesktopAppExited,
            exited_app as u32 as u64,
            exit_code,
        );
        let _ = exited_surface;
        changed = true;
    }
    if changed {
        let _ = focus_next_visible_without_cycle(state);
        render_desktop(state)?;
    }
    Ok(())
}

fn render_desktop(state: &mut DesktopState) -> rt::Result<()> {
    let status_snapshot = sample_desktop_status(state);
    rt::surface_set_fill(state.chrome.desktop_handle, ui::BG_DESKTOP)?;
    rt::surface_clear_scene(state.chrome.desktop_handle)?;

    let mut running_buf = FixedLogBuffer::<32>::new();
    let _ = write!(&mut running_buf, "RUNNING {}", status_snapshot.running_apps);
    let running_text = str::from_utf8(running_buf.as_bytes()).unwrap_or("RUNNING ?");

    let mut focus_buf = FixedLogBuffer::<40>::new();
    let _ = write!(
        &mut focus_buf,
        "FOCUS {}",
        status_snapshot.focused_app.map(app_title).unwrap_or("NONE")
    );
    let focus_text = str::from_utf8(focus_buf.as_bytes()).unwrap_or("FOCUS ?");

    let mut network_buf = FixedLogBuffer::<48>::new();
    write_network_status(&mut network_buf, status_snapshot.ipv4_address);
    let network_text = str::from_utf8(network_buf.as_bytes()).unwrap_or("NET OFFLINE");

    ui::render_panel(
        state.chrome.topbar_handle,
        state.chrome.output_width,
        TOPBAR_HEIGHT,
        "SERVICEOS DESKTOP",
        &[running_text, focus_text, network_text],
    )?;

    let launcher_lines = [
        "LAUNCHER",
        launcher_line(state.apps[0]),
        launcher_line(state.apps[1]),
        launcher_line(state.apps[2]),
        "CLICK ITEMS OR USE SHELL",
        "DRAG TITLEBAR / GRIP",
    ];
    ui::render_panel(
        state.chrome.launcher_handle,
        LAUNCHER_WIDTH,
        264,
        "LAUNCHER",
        &launcher_lines,
    )?;

    let mut hb_buf = FixedLogBuffer::<32>::new();
    let _ = write!(&mut hb_buf, "HEARTBEAT {}", status_snapshot.heartbeat_count);
    let heartbeat_text = str::from_utf8(hb_buf.as_bytes()).unwrap_or("HEARTBEAT ?");

    let mut tick_buf = FixedLogBuffer::<32>::new();
    let _ = write!(&mut tick_buf, "LAST TICK {}", status_snapshot.heartbeat_tick);
    let tick_text = str::from_utf8(tick_buf.as_bytes()).unwrap_or("LAST TICK ?");

    ui::render_status_panel(
        state.chrome.status_handle,
        STATUS_PANEL_WIDTH,
        STATUS_PANEL_HEIGHT,
        "SYSTEM STATUS",
        &[
            (network_text, ui::TEXT_PRIMARY),
            (heartbeat_text, ui::STATUS_OK),
            (tick_text, ui::TEXT_SECONDARY),
            (focus_text, ui::TEXT_SECONDARY),
            ("POINTER READY", ui::TEXT_MUTED),
        ],
    )?;

    state.last_status_snapshot = Some(status_snapshot);
    Ok(())
}

fn refresh_desktop_status(state: &mut DesktopState) -> rt::Result<()> {
    let snapshot = sample_desktop_status(state);
    if state.last_status_snapshot == Some(snapshot) {
        return Ok(());
    }
    render_desktop(state)
}

fn sample_desktop_status(state: &DesktopState) -> DesktopStatusSnapshot {
    let (heartbeat_count, heartbeat_tick) =
        rt::status_snapshot(state.system_status_handle).unwrap_or((0, 0));
    let ipv4_address = rt::network_interface_status(state.network_handle, 0)
        .ok()
        .flatten()
        .map(|info| info.address)
        .unwrap_or(0);
    DesktopStatusSnapshot {
        running_apps: running_app_count(&state.apps) as u32,
        focused_app: state.focused_app,
        heartbeat_count,
        heartbeat_tick,
        ipv4_address,
    }
}

fn handle_input(
    state: &mut DesktopState,
    action: DesktopInputAction,
    x: i32,
    y: i32,
) -> rt::Result<u32> {
    let result = match action {
        DesktopInputAction::PointerDown => {
            state.pointer_x = x;
            state.pointer_y = y;
            handle_pointer_down(state, x, y)
        }
        DesktopInputAction::PointerMove => {
            state.pointer_x = x;
            state.pointer_y = y;
            handle_pointer_move(state, x, y)
        }
        DesktopInputAction::PointerUp => {
            state.pointer_x = x;
            state.pointer_y = y;
            let surface_id = handle_pointer_up(state, x, y)?;
            state.drag_state = None;
            state.content_capture = None;
            Ok(surface_id)
        }
        DesktopInputAction::Click => {
            state.pointer_x = x;
            state.pointer_y = y;
            let _ = handle_pointer_down(state, x, y)?;
            state.drag_state = None;
            state.content_capture = None;
            Ok(focused_surface_id(state))
        }
        DesktopInputAction::KeyDown => {
            handle_key_input(state, rt::AppKeyAction::Down, x as u32, y as u32)
        }
        DesktopInputAction::KeyUp => {
            handle_key_input(state, rt::AppKeyAction::Up, x as u32, y as u32)
        }
        DesktopInputAction::TextInput => handle_text_input(state, x as u32),
    }?;
    sync_cursor(state)?;
    match action {
        DesktopInputAction::PointerMove => {
            if state.drag_state.is_some() || state.content_capture.is_some() {
                render_desktop(state)?;
            }
        }
        _ => render_desktop(state)?,
    }
    Ok(result)
}

fn sync_cursor(state: &DesktopState) -> rt::Result<()> {
    rt::surface_set_geometry(
        state.chrome.cursor_handle,
        state.pointer_x,
        state.pointer_y,
        CURSOR_SIZE,
        CURSOR_SIZE,
        CURSOR_Z_ORDER,
    )
}

fn handle_pointer_down(state: &mut DesktopState, x: i32, y: i32) -> rt::Result<u32> {
    match hit_test(state, x, y) {
        HitTarget::Background => {
            state.drag_state = None;
            state.content_capture = None;
            Ok(focused_surface_id(state))
        }
        HitTarget::Launcher(app_id) => launch_or_focus_app(state, app_id),
        HitTarget::WindowContent(app_id) => {
            state.drag_state = None;
            state.content_capture = Some(ContentCapture { app_id, button: 1 });
            let surface_id = focus_app(state, app_id)?;
            let (local_x, local_y) = app_local_coords(state, app_id, x, y)?;
            dispatch_pointer_to_app(
                state,
                app_id,
                rt::AppPointerAction::Down,
                local_x,
                local_y,
                1,
            )?;
            Ok(surface_id)
        }
        HitTarget::WindowMove {
            app_id,
            grab_offset_x,
            grab_offset_y,
        } => {
            state.content_capture = None;
            let surface_id = focus_app(state, app_id)?;
            state.drag_state = Some(DragState::Move {
                app_id,
                grab_offset_x,
                grab_offset_y,
            });
            Ok(surface_id)
        }
        HitTarget::WindowResize {
            app_id,
            edges,
        } => {
            state.content_capture = None;
            let surface_id = focus_app(state, app_id)?;
            let index = app_slot_index(&state.apps, app_id).ok_or(rt::Error::NotFound)?;
            state.drag_state = Some(DragState::Resize {
                app_id,
                edges,
                origin_pointer_x: x,
                origin_pointer_y: y,
                start_x: state.apps[index].window.x,
                start_y: state.apps[index].window.y,
                start_width: state.apps[index].window.width,
                start_height: state.apps[index].window.height,
            });
            Ok(surface_id)
        }
        HitTarget::WindowClose(app_id) => {
            state.content_capture = None;
            close_app(state, app_id)?;
            Ok(focused_surface_id(state))
        }
        HitTarget::WindowMaximize(app_id) => {
            state.content_capture = None;
            maximize_app(state, app_id)
        }
        HitTarget::WindowMinimize(app_id) => minimize_app(state, app_id),
    }
}

fn handle_pointer_move(state: &mut DesktopState, x: i32, y: i32) -> rt::Result<u32> {
    match state.drag_state {
        Some(DragState::Move {
            app_id,
            grab_offset_x,
            grab_offset_y,
        }) => move_app(state, app_id, x - grab_offset_x, y - grab_offset_y),
        Some(DragState::Resize {
            app_id,
            edges,
            origin_pointer_x,
            origin_pointer_y,
            start_x,
            start_y,
            start_width,
            start_height,
        }) => resize_drag(
            state,
            app_id,
            edges,
            origin_pointer_x,
            origin_pointer_y,
            start_x,
            start_y,
            start_width,
            start_height,
            x,
            y,
        ),
        None => {
            if let Some(capture) = state.content_capture {
                let (local_x, local_y) = app_local_coords(state, capture.app_id, x, y)?;
                dispatch_pointer_to_app(
                    state,
                    capture.app_id,
                    rt::AppPointerAction::Move,
                    local_x,
                    local_y,
                    capture.button,
                )?;
            }
            Ok(focused_surface_id(state))
        }
    }
}

fn handle_pointer_up(state: &mut DesktopState, x: i32, y: i32) -> rt::Result<u32> {
    if let Some(capture) = state.content_capture {
        let (local_x, local_y) = app_local_coords(state, capture.app_id, x, y)?;
        dispatch_pointer_to_app(
            state,
            capture.app_id,
            rt::AppPointerAction::Up,
            local_x,
            local_y,
            capture.button,
        )?;
    }
    Ok(focused_surface_id(state))
}

fn handle_key_input(
    state: &mut DesktopState,
    action: rt::AppKeyAction,
    key_code: u32,
    modifiers: u32,
) -> rt::Result<u32> {
    if action == rt::AppKeyAction::Down && modifiers & MOD_ALT != 0 {
        if key_code == KEY_TAB {
            return focus_next_app(state);
        }
        if key_code == KEY_F4 {
            if let Some(app_id) = state.focused_app {
                close_app(state, app_id)?;
                return Ok(focused_surface_id(state));
            }
        }
    }

    let Some(app_id) = state.focused_app else {
        return Ok(0);
    };
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Ok(0);
    };
    let control = state.apps[index].window.control_handle;
    if control == rt::INVALID_HANDLE {
        return Ok(0);
    }
    rt::app_control_key(control, action, key_code)?;
    let _ = emit_log(
        state.log_handle,
        LogSeverity::Debug,
        LogEvent::InputKeyDelivered,
        app_id as u32 as u64,
        key_code as u64,
    );
    Ok(state.apps[index].window.surface_id)
}

fn handle_text_input(state: &mut DesktopState, scalar: u32) -> rt::Result<u32> {
    let Some(ch) = char::from_u32(scalar) else {
        return Ok(focused_surface_id(state));
    };
    let Some(app_id) = state.focused_app else {
        return Ok(0);
    };
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Ok(0);
    };
    let control = state.apps[index].window.control_handle;
    if control == rt::INVALID_HANDLE {
        return Ok(0);
    }
    rt::app_control_text(control, ch)?;
    Ok(state.apps[index].window.surface_id)
}

fn sort_app_ids_by_z(state: &DesktopState, values: &mut [DesktopAppId]) {
    let mut index = 1usize;
    while index < values.len() {
        let current = values[index];
        let current_z = app_slot_index(&state.apps, current)
            .map(|slot_index| state.apps[slot_index].window.z_order)
            .unwrap_or(0);
        let mut scan = index;
        while scan > 0 {
            let prev = values[scan - 1];
            let prev_z = app_slot_index(&state.apps, prev)
                .map(|slot_index| state.apps[slot_index].window.z_order)
                .unwrap_or(0);
            if prev_z <= current_z {
                break;
            }
            values[scan] = prev;
            scan -= 1;
        }
        values[scan] = current;
        index += 1;
    }
}

fn hit_test(state: &DesktopState, x: i32, y: i32) -> HitTarget {
    if let Some(app_id) = launcher_hit_app(state, x, y) {
        return HitTarget::Launcher(app_id);
    }

    let mut order = [DesktopAppId::Settings; APP_COUNT];
    let mut count = 0usize;
    for slot in state.apps.iter().copied() {
        if slot.running && slot.window.visible() {
            order[count] = slot.app_id;
            count += 1;
        }
    }
    sort_app_ids_by_z(state, &mut order[..count]);

    for app_id in order[..count].iter().copied().rev() {
        let index = app_slot_index(&state.apps, app_id).unwrap();
        let window = state.apps[index].window;
        if x < window.x
            || y < window.y
            || x >= window.x + window.width as i32
            || y >= window.y + window.height as i32
        {
            continue;
        }

        let local_x = x - window.x;
        let local_y = y - window.y;
        if local_y < ui::TITLEBAR_HEIGHT as i32 {
            let close_left =
                window.width as i32 - ui::WINDOW_BUTTON_RIGHT_MARGIN - ui::WINDOW_BUTTON_SIZE as i32;
            let minimize_left = close_left - ui::WINDOW_BUTTON_GAP - ui::WINDOW_BUTTON_SIZE as i32;
            let maximize_left = minimize_left - ui::WINDOW_BUTTON_GAP - ui::WINDOW_BUTTON_SIZE as i32;
            if local_x >= close_left && local_x < close_left + ui::WINDOW_BUTTON_SIZE as i32 {
                return HitTarget::WindowClose(app_id);
            }
            if local_x >= minimize_left
                && local_x < minimize_left + ui::WINDOW_BUTTON_SIZE as i32
            {
                return HitTarget::WindowMinimize(app_id);
            }
            if local_x >= maximize_left
                && local_x < maximize_left + ui::WINDOW_BUTTON_SIZE as i32
            {
                return HitTarget::WindowMaximize(app_id);
            }
        }

        let resize_edges = resize_hit_edges(&window, local_x, local_y);
        if !resize_edges.is_empty() && !window.maximized {
            return HitTarget::WindowResize {
                app_id,
                edges: resize_edges,
            };
        }

        if local_y < ui::TITLEBAR_HEIGHT as i32 {
            return HitTarget::WindowMove {
                app_id,
                grab_offset_x: local_x,
                grab_offset_y: local_y,
            };
        }

        return HitTarget::WindowContent(app_id);
    }

    HitTarget::Background
}

fn launcher_hit_app(state: &DesktopState, x: i32, y: i32) -> Option<DesktopAppId> {
    let launcher_x = PANEL_MARGIN as i32;
    let launcher_y = (TOPBAR_HEIGHT + PANEL_MARGIN) as i32;
    if x < launcher_x
        || y < launcher_y
        || x >= launcher_x + LAUNCHER_WIDTH as i32
        || y >= launcher_y + 264
    {
        return None;
    }

    let local_y = y - launcher_y;
    let row = (local_y - LAUNCHER_ITEM_START_Y) / LAUNCHER_ITEM_STEP;
    match row {
        0 => Some(state.apps[0].app_id),
        1 => Some(state.apps[1].app_id),
        2 => Some(state.apps[2].app_id),
        _ => None,
    }
}

fn focus_next_app(state: &mut DesktopState) -> rt::Result<u32> {
    let mut candidates = [DesktopAppId::Settings; APP_COUNT];
    let mut count = 0usize;
    for slot in state.apps.iter().copied() {
        if slot.running && slot.window.visible() {
            candidates[count] = slot.app_id;
            count += 1;
        }
    }
    if count == 0 {
        return Err(rt::Error::NotFound);
    }
    sort_app_ids_by_z(state, &mut candidates[..count]);

    let next = if let Some(current) = state.focused_app {
        let current_index = candidates[..count]
            .iter()
            .position(|candidate| *candidate == current)
            .unwrap_or(usize::MAX);
        if current_index == usize::MAX {
            candidates[count - 1]
        } else {
            candidates[(current_index + 1) % count]
        }
    } else {
        candidates[count - 1]
    };
    focus_app(state, next)
}

fn focus_next_visible_without_cycle(state: &mut DesktopState) -> rt::Result<u32> {
    let mut best: Option<(u32, DesktopAppId)> = None;
    for slot in state.apps.iter().copied() {
        if !slot.running || !slot.window.visible() {
            continue;
        }
        match best {
            Some((z_order, _)) if z_order >= slot.window.z_order => {}
            _ => best = Some((slot.window.z_order, slot.app_id)),
        }
    }
    match best {
        Some((_, app_id)) => focus_app(state, app_id),
        None => Ok(0),
    }
}

fn encode_window_page(state: &DesktopState, start: usize, reply: &mut RawMessage) {
    let mut windows = [WindowState::empty(); APP_COUNT];
    let mut app_ids = [DesktopAppId::Settings; APP_COUNT];
    let mut total = 0usize;
    for slot in state.apps.iter().copied() {
        if !slot.running || slot.window.surface_id == 0 {
            continue;
        }
        windows[total] = slot.window;
        app_ids[total] = slot.app_id;
        total += 1;
    }
    for index in 0..total {
        let mut best = index;
        for candidate in index + 1..total {
            if windows[candidate].z_order < windows[best].z_order {
                best = candidate;
            }
        }
        windows.swap(index, best);
        app_ids.swap(index, best);
    }

    let mut returned = 0usize;
    for index in start..total.min(start + WINDOW_PAGE_SIZE) {
        let base = 3 + returned * 5;
        let app_id = app_ids[index];
        let window = windows[index];
        reply.words[base] = app_id as u32 as u64;
        reply.words[base + 1] = window.surface_id as u64;
        reply.words[base + 2] = pack_window_flags(
            window.z_order,
            state.focused_app == Some(app_id),
            window.minimized,
            window.visible(),
        );
        reply.words[base + 3] = pack_i32_pair(window.x, window.y);
        reply.words[base + 4] = pack_u32_pair(window.width, window.height);
        returned += 1;
    }
    reply.words[1] = returned as u64;
    reply.words[2] = if start + returned >= total {
        u32::MAX as u64
    } else {
        (start + returned) as u64
    };
    reply.word_count = (3 + returned * 5) as u32;
}

fn apply_window_geometry(slot: &AppSlot) -> rt::Result<()> {
    rt::surface_set_geometry(
        slot.window.surface_handle,
        slot.window.x,
        slot.window.y,
        slot.window.width,
        slot.window.height,
        slot.window.z_order,
    )
}

fn sync_window_surface(slot: &AppSlot) -> rt::Result<()> {
    apply_window_geometry(slot)?;
    rt::surface_set_visibility(slot.window.surface_handle, slot.window.visible())
}

fn allocate_z_order(state: &mut DesktopState) -> u32 {
    let z_order = state.next_z_order;
    state.next_z_order = state.next_z_order.saturating_add(1);
    z_order
}

fn focused_surface_id(state: &DesktopState) -> u32 {
    state
        .focused_app
        .and_then(|app_id| app_slot_index(&state.apps, app_id))
        .map(|index| state.apps[index].window.surface_id)
        .unwrap_or(0)
}

fn initial_window_layout(output_width: u32, app_id: DesktopAppId) -> (i32, i32, u32, u32, u32) {
    match app_id {
        DesktopAppId::Settings => (292, 92, 420, 240, ui::BG_WINDOW),
        DesktopAppId::Files => (336, 168, 560, 276, ui::BG_WINDOW_ALT),
        DesktopAppId::Monitor => (
            output_width.saturating_sub(500 + PANEL_MARGIN) as i32,
            108,
            480,
            240,
            ui::BG_WINDOW,
        ),
    }
}

fn clamp_window_x(output_width: u32, width: u32, requested: i32) -> i32 {
    let max_x = output_width.saturating_sub(width + PANEL_MARGIN) as i32;
    requested.clamp(PANEL_MARGIN as i32, max_x.max(PANEL_MARGIN as i32))
}

fn clamp_window_y(output_height: u32, height: u32, requested: i32) -> i32 {
    let min_y = (TOPBAR_HEIGHT + PANEL_MARGIN) as i32;
    let max_y = output_height
        .saturating_sub(height + PANEL_MARGIN)
        .max(TOPBAR_HEIGHT + PANEL_MARGIN) as i32;
    requested.clamp(min_y, max_y)
}

fn write_network_status(buffer: &mut FixedLogBuffer<48>, ipv4_address: u32) {
    if ipv4_address == 0 {
        let _ = write!(buffer, "NET OFFLINE");
        return;
    }
    let _ = write!(
        buffer,
        "NET {}.{}.{}.{}",
        (ipv4_address >> 24) & 0xff,
        (ipv4_address >> 16) & 0xff,
        (ipv4_address >> 8) & 0xff,
        ipv4_address & 0xff,
    );
}

fn launcher_line(slot: AppSlot) -> &'static str {
    match (slot.app_id, slot.running, slot.window.minimized) {
        (DesktopAppId::Settings, true, false) => "SETTINGS RUN",
        (DesktopAppId::Settings, true, true) => "SETTINGS MIN",
        (DesktopAppId::Settings, false, _) => "SETTINGS OFF",
        (DesktopAppId::Files, true, false) => "FILES RUN",
        (DesktopAppId::Files, true, true) => "FILES MIN",
        (DesktopAppId::Files, false, _) => "FILES OFF",
        (DesktopAppId::Monitor, true, false) => "MONITOR RUN",
        (DesktopAppId::Monitor, true, true) => "MONITOR MIN",
        (DesktopAppId::Monitor, false, _) => "MONITOR OFF",
    }
}

fn running_app_count(apps: &[AppSlot; APP_COUNT]) -> usize {
    apps.iter().filter(|slot| slot.running).count()
}

fn app_slot_index(apps: &[AppSlot; APP_COUNT], app_id: DesktopAppId) -> Option<usize> {
    apps.iter().position(|slot| slot.app_id == app_id)
}

fn desktop_app_from_word(value: u64) -> Option<DesktopAppId> {
    match value as u32 {
        x if x == DesktopAppId::Settings as u32 => Some(DesktopAppId::Settings),
        x if x == DesktopAppId::Files as u32 => Some(DesktopAppId::Files),
        x if x == DesktopAppId::Monitor as u32 => Some(DesktopAppId::Monitor),
        _ => None,
    }
}

fn desktop_window_action_from_word(value: u64) -> Option<DesktopWindowAction> {
    match value as u32 {
        x if x == DesktopWindowAction::Focus as u32 => Some(DesktopWindowAction::Focus),
        x if x == DesktopWindowAction::Close as u32 => Some(DesktopWindowAction::Close),
        x if x == DesktopWindowAction::Minimize as u32 => Some(DesktopWindowAction::Minimize),
        x if x == DesktopWindowAction::Restore as u32 => Some(DesktopWindowAction::Restore),
        x if x == DesktopWindowAction::Move as u32 => Some(DesktopWindowAction::Move),
        x if x == DesktopWindowAction::Resize as u32 => Some(DesktopWindowAction::Resize),
        x if x == DesktopWindowAction::FocusNext as u32 => Some(DesktopWindowAction::FocusNext),
        x if x == DesktopWindowAction::Maximize as u32 => Some(DesktopWindowAction::Maximize),
        _ => None,
    }
}

fn desktop_input_action_from_word(value: u64) -> Option<DesktopInputAction> {
    match value as u32 {
        x if x == DesktopInputAction::PointerDown as u32 => Some(DesktopInputAction::PointerDown),
        x if x == DesktopInputAction::PointerMove as u32 => Some(DesktopInputAction::PointerMove),
        x if x == DesktopInputAction::PointerUp as u32 => Some(DesktopInputAction::PointerUp),
        x if x == DesktopInputAction::Click as u32 => Some(DesktopInputAction::Click),
        x if x == DesktopInputAction::KeyDown as u32 => Some(DesktopInputAction::KeyDown),
        x if x == DesktopInputAction::KeyUp as u32 => Some(DesktopInputAction::KeyUp),
        x if x == DesktopInputAction::TextInput as u32 => Some(DesktopInputAction::TextInput),
        _ => None,
    }
}

fn app_title(app_id: DesktopAppId) -> &'static str {
    match app_id {
        DesktopAppId::Settings => "SETTINGS",
        DesktopAppId::Files => "FILES",
        DesktopAppId::Monitor => "MONITOR",
    }
}

fn pack_window_flags(z_order: u32, focused: bool, minimized: bool, visible: bool) -> u64 {
    let mut flags = (z_order as u64) << 32;
    if focused {
        flags |= 0x1;
    }
    if minimized {
        flags |= 0x2;
    }
    if visible {
        flags |= 0x4;
    }
    flags
}

fn pack_i32_pair(first: i32, second: i32) -> u64 {
    (first as u32 as u64) | ((second as u32 as u64) << 32)
}

fn pack_u32_pair(first: u32, second: u32) -> u64 {
    first as u64 | ((second as u64) << 32)
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

fn emit_log(
    log_handle: rt::Handle,
    severity: LogSeverity,
    event: LogEvent,
    arg0: u64,
    arg1: u64,
) -> rt::Result<()> {
    rt::send_log_record(
        log_handle,
        ServiceId::DesktopShell,
        severity,
        LogDomain::Desktop,
        event,
        arg0,
        arg1,
    )
}

fn emit_text_log(domain: &str, args: core::fmt::Arguments<'_>) -> rt::Result<()> {
    rt::write_logf(domain, args)
}

fn dispatch_pointer_to_app(
    state: &DesktopState,
    app_id: DesktopAppId,
    action: rt::AppPointerAction,
    local_x: i32,
    local_y: i32,
    button: u32,
) -> rt::Result<()> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    let control = state.apps[index].window.control_handle;
    if control == rt::INVALID_HANDLE {
        return Err(rt::Error::NotFound);
    }
    rt::app_control_pointer(control, action, local_x, local_y, button)
}

fn app_local_coords(
    state: &DesktopState,
    app_id: DesktopAppId,
    x: i32,
    y: i32,
) -> rt::Result<(i32, i32)> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    let window = state.apps[index].window;
    let max_x = (window.width.saturating_sub(1)) as i32;
    let max_y = (window.height.saturating_sub(1)) as i32;
    Ok(((x - window.x).clamp(0, max_x), (y - window.y).clamp(0, max_y)))
}

fn resize_hit_edges(window: &WindowState, local_x: i32, local_y: i32) -> ResizeEdges {
    let mut edges = ResizeEdges::NONE;
    if local_x <= ui::WINDOW_BORDER_THICKNESS {
        edges |= ResizeEdges::LEFT;
    }
    if local_x >= window.width as i32 - RESIZE_GRIP_SIZE {
        edges |= ResizeEdges::RIGHT;
    }
    if local_y <= ui::WINDOW_BORDER_THICKNESS {
        edges |= ResizeEdges::TOP;
    }
    if local_y >= window.height as i32 - RESIZE_GRIP_SIZE {
        edges |= ResizeEdges::BOTTOM;
    }
    edges
}

#[allow(clippy::too_many_arguments)]
fn resize_drag(
    state: &mut DesktopState,
    app_id: DesktopAppId,
    edges: ResizeEdges,
    origin_pointer_x: i32,
    origin_pointer_y: i32,
    start_x: i32,
    start_y: i32,
    start_width: u32,
    start_height: u32,
    x: i32,
    y: i32,
) -> rt::Result<u32> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    let delta_x = x - origin_pointer_x;
    let delta_y = y - origin_pointer_y;

    let mut new_x = start_x;
    let mut new_y = start_y;
    let mut new_width = start_width as i32;
    let mut new_height = start_height as i32;

    if edges.contains(ResizeEdges::LEFT) {
        new_x = start_x + delta_x;
        new_width = start_width as i32 - delta_x;
    }
    if edges.contains(ResizeEdges::RIGHT) {
        new_width = start_width as i32 + delta_x;
    }
    if edges.contains(ResizeEdges::TOP) {
        new_y = start_y + delta_y;
        new_height = start_height as i32 - delta_y;
    }
    if edges.contains(ResizeEdges::BOTTOM) {
        new_height = start_height as i32 + delta_y;
    }

    if new_width < WINDOW_MIN_WIDTH as i32 {
        if edges.contains(ResizeEdges::LEFT) {
            new_x -= WINDOW_MIN_WIDTH as i32 - new_width;
        }
        new_width = WINDOW_MIN_WIDTH as i32;
    }
    if new_height < WINDOW_MIN_HEIGHT as i32 {
        if edges.contains(ResizeEdges::TOP) {
            new_y -= WINDOW_MIN_HEIGHT as i32 - new_height;
        }
        new_height = WINDOW_MIN_HEIGHT as i32;
    }

    state.apps[index].window.maximized = false;
    state.apps[index].window.x =
        clamp_window_x(state.chrome.output_width, new_width as u32, new_x);
    state.apps[index].window.y =
        clamp_window_y(state.chrome.output_height, new_height as u32, new_y);
    state.apps[index].window.width = new_width as u32;
    state.apps[index].window.height = new_height as u32;
    apply_window_geometry(&state.apps[index])?;
    if state.apps[index].window.control_handle != rt::INVALID_HANDLE {
        let _ = rt::app_control_resize(
            state.apps[index].window.control_handle,
            state.apps[index].window.width,
            state.apps[index].window.height,
        );
    }
    Ok(state.apps[index].window.surface_id)
}

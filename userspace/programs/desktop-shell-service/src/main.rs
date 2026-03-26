#![no_std]
#![no_main]

use core::{fmt::Write, str};

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{
    ControlTag, DesktopAppId, DesktopStatus, DesktopTag, FixedLogBuffer, LifecycleEvent,
    LogDomain, LogEvent, LogSeverity, RawMessage, ServiceId, ServiceImageId, StartupHandle,
};

const SESSION_ID: u32 = 1;
const APP_COUNT: usize = 3;
const STATUS_REFRESH_TICKS: u64 = 25;
const TOPBAR_HEIGHT: u32 = 42;
const LAUNCHER_WIDTH: u32 = 250;
const PANEL_MARGIN: u32 = 20;
const STATUS_PANEL_WIDTH: u32 = 280;
const STATUS_PANEL_HEIGHT: u32 = 144;

#[derive(Clone, Copy)]
struct Chrome {
    desktop_handle: rt::Handle,
    topbar_handle: rt::Handle,
    launcher_handle: rt::Handle,
    status_handle: rt::Handle,
    output_width: u32,
    output_height: u32,
}

#[derive(Clone, Copy)]
struct AppSlot {
    app_id: DesktopAppId,
    image_id: ServiceImageId,
    task_handle: rt::Handle,
    surface_id: u32,
    running: bool,
}

impl AppSlot {
    const fn new(app_id: DesktopAppId, image_id: ServiceImageId) -> Self {
        Self {
            app_id,
            image_id,
            task_handle: rt::INVALID_HANDLE,
            surface_id: 0,
            running: false,
        }
    }
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
    last_status_refresh: u64,
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
        last_status_refresh: 0,
    };

    if render_desktop(&mut state).is_err() {
        return 0xfe0b;
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
            Err(_) => return 0xfe0c,
        }

        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                if handle_request(&mut state, &request).is_err() {
                    return 0xfe0d;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xfe0e,
        }

        if refresh_apps(&mut state).is_err() {
            return 0xfe0f;
        }

        let now = match rt::monotonic_now() {
            Ok(now) => now,
            Err(_) => return 0xfe10,
        };
        if now.saturating_sub(state.last_status_refresh) >= STATUS_REFRESH_TICKS {
            if render_desktop(&mut state).is_err() {
                return 0xfe11;
            }
            state.last_status_refresh = now;
        }

        if rt::yield_current().is_err() {
            return 0xfe12;
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
        true,
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
        true,
    )?;
    let (_, launcher_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        PANEL_MARGIN as i32,
        (TOPBAR_HEIGHT + PANEL_MARGIN) as i32,
        LAUNCHER_WIDTH,
        248,
        2,
        ui::BG_PANEL,
        true,
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
        true,
    )?;

    Ok(Chrome {
        desktop_handle,
        topbar_handle,
        launcher_handle,
        status_handle,
        output_width,
        output_height,
    })
}

fn handle_request(state: &mut DesktopState, request: &RawMessage) -> rt::Result<()> {
    match request.tag {
        x if x == DesktopTag::StatusRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(DesktopTag::StatusReply as u32);
            reply.word_count = 4;
            reply.words[0] = DesktopStatus::Ok as u32 as u64;
            reply.words[1] = SESSION_ID as u64;
            reply.words[2] = state.focused_app.map(|app| app as u32 as u64).unwrap_or(0);
            reply.words[3] = running_app_count(&state.apps) as u64;
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
                reply.words[base + 3] = slot.surface_id as u64;
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
                Some(app_id) => match launch_or_focus_app(state, app_id) {
                    Ok(surface_id) => {
                        reply.words[0] = DesktopStatus::Ok as u32 as u64;
                        reply.words[1] = surface_id as u64;
                    }
                    Err(rt::Error::PermissionDenied) => {
                        reply.words[0] = DesktopStatus::Denied as u32 as u64;
                    }
                    Err(rt::Error::NotFound) => {
                        reply.words[0] = DesktopStatus::NotFound as u32 as u64;
                    }
                    Err(_) => {
                        reply.words[0] = DesktopStatus::Busy as u32 as u64;
                    }
                },
                None => {
                    reply.words[0] = DesktopStatus::NotFound as u32 as u64;
                }
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
                Some(app_id) => match focus_app(state, app_id) {
                    Ok(surface_id) => {
                        reply.words[0] = DesktopStatus::Ok as u32 as u64;
                        reply.words[1] = surface_id as u64;
                    }
                    Err(rt::Error::NotFound) => {
                        reply.words[0] = DesktopStatus::NotFound as u32 as u64;
                    }
                    Err(_) => {
                        reply.words[0] = DesktopStatus::Busy as u32 as u64;
                    }
                },
                None => {
                    reply.words[0] = DesktopStatus::NotFound as u32 as u64;
                }
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        _ => {}
    }

    Ok(())
}

fn launch_or_focus_app(state: &mut DesktopState, app_id: DesktopAppId) -> rt::Result<u32> {
    let Some(slot_index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if state.apps[slot_index].running {
        return focus_app(state, app_id);
    }

    let (x, y, width, height, z_order, fill_rgb) = app_layout(state.chrome.output_width, app_id);
    let (surface_id, surface_handle) = rt::graphics_surface_create(
        state.graphics_handle,
        SESSION_ID,
        x,
        y,
        width,
        height,
        z_order,
        fill_rgb,
        true,
    )?;
    let _ = ui::render_window(
        surface_handle,
        width,
        height,
        fill_rgb,
        ui::ACCENT_DIM,
        app_title(app_id),
        &["LAUNCHING", "PLEASE WAIT"],
    );

    let task_handle = rt::manager_launch_program_with_payload(
        state.bootstrap,
        state.apps[slot_index].image_id,
        &[surface_id as u64, width as u64, height as u64],
        &[StartupHandle {
            handle: surface_handle,
            rights: rt::rights::SEND
                | rt::rights::RECEIVE
                | rt::rights::DUPLICATE
                | rt::rights::TRANSFER,
        }],
    )?;

    state.apps[slot_index].task_handle = task_handle;
    state.apps[slot_index].surface_id = surface_id;
    state.apps[slot_index].running = true;
    let _ = focus_app(state, app_id)?;
    let _ = emit_log(
        state.log_handle,
        LogSeverity::Info,
        LogEvent::DesktopAppLaunched,
        app_id as u32 as u64,
        surface_id as u64,
    );
    render_desktop(state)?;
    Ok(surface_id)
}

fn focus_app(state: &mut DesktopState, app_id: DesktopAppId) -> rt::Result<u32> {
    let Some(slot_index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if !state.apps[slot_index].running || state.apps[slot_index].surface_id == 0 {
        return Err(rt::Error::NotFound);
    }
    let surface_id = state.apps[slot_index].surface_id;
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
        let exited_surface = slot.surface_id;
        let exit_code = status.exit_code;
        let _ = rt::handle_close(slot.task_handle);
        slot.task_handle = rt::INVALID_HANDLE;
        slot.surface_id = 0;
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
        render_desktop(state)?;
    }
    Ok(())
}

fn render_desktop(state: &mut DesktopState) -> rt::Result<()> {
    rt::surface_set_fill(state.chrome.desktop_handle, ui::BG_DESKTOP)?;
    rt::surface_clear_scene(state.chrome.desktop_handle)?;

    let running_count = running_app_count(&state.apps);
    let mut running_buf = FixedLogBuffer::<32>::new();
    let _ = write!(&mut running_buf, "RUNNING {}", running_count);
    let running_text = str::from_utf8(running_buf.as_bytes()).unwrap_or("RUNNING ?");

    let mut focus_buf = FixedLogBuffer::<32>::new();
    let _ = write!(
        &mut focus_buf,
        "FOCUS {}",
        state.focused_app.map(app_title).unwrap_or("NONE")
    );
    let focus_text = str::from_utf8(focus_buf.as_bytes()).unwrap_or("FOCUS ?");

    let mut network_buf = FixedLogBuffer::<48>::new();
    write_network_status(&mut network_buf, state.network_handle);
    let network_text = str::from_utf8(network_buf.as_bytes()).unwrap_or("NET OFFLINE");
    ui::render_panel(
        state.chrome.topbar_handle,
        state.chrome.output_width,
        TOPBAR_HEIGHT,
        "SERVICEOS DESKTOP",
        &[running_text, focus_text, network_text],
    )?;

    let mut app_lines = [
        "LAUNCHER",
        "SETTINGS OFF",
        "FILES OFF",
        "MONITOR OFF",
        "SERIAL: DESKTOP LAUNCH",
        "SERIAL: DESKTOP FOCUS",
    ];
    for (index, slot) in state.apps.iter().copied().enumerate() {
        app_lines[index + 1] = app_state_label(slot.app_id, slot.running);
    }
    ui::render_panel(
        state.chrome.launcher_handle,
        LAUNCHER_WIDTH,
        248,
        "LAUNCHER",
        &app_lines,
    )?;

    let (heartbeat_count, heartbeat_tick) =
        rt::status_snapshot(state.system_status_handle).unwrap_or((0, 0));
    let mut hb_buf = FixedLogBuffer::<32>::new();
    let _ = write!(&mut hb_buf, "HEARTBEAT {}", heartbeat_count);
    let heartbeat_text = str::from_utf8(hb_buf.as_bytes()).unwrap_or("HEARTBEAT ?");

    let mut tick_buf = FixedLogBuffer::<32>::new();
    let _ = write!(&mut tick_buf, "LAST TICK {}", heartbeat_tick);
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
        ],
    )?;

    Ok(())
}

fn write_network_status(buffer: &mut FixedLogBuffer<48>, network_handle: rt::Handle) {
    let Ok(Some(info)) = rt::network_interface_status(network_handle, 0) else {
        let _ = write!(buffer, "NET OFFLINE");
        return;
    };
    let _ = write!(
        buffer,
        "NET {}.{}.{}.{}",
        (info.address >> 24) & 0xff,
        (info.address >> 16) & 0xff,
        (info.address >> 8) & 0xff,
        info.address & 0xff,
    );
}

fn app_layout(output_width: u32, app_id: DesktopAppId) -> (i32, i32, u32, u32, u32, u32) {
    match app_id {
        DesktopAppId::Settings => (290, 84, 400, 220, 10, ui::BG_WINDOW),
        DesktopAppId::Files => (290, 324, 540, 240, 11, ui::BG_WINDOW_ALT),
        DesktopAppId::Monitor => (
            (output_width.saturating_sub(480 + PANEL_MARGIN)) as i32,
            84,
            460,
            220,
            12,
            ui::BG_WINDOW,
        ),
    }
}

fn app_slot_index(apps: &[AppSlot; APP_COUNT], app_id: DesktopAppId) -> Option<usize> {
    apps.iter().position(|slot| slot.app_id == app_id)
}

fn running_app_count(apps: &[AppSlot; APP_COUNT]) -> usize {
    apps.iter().filter(|slot| slot.running).count()
}

fn desktop_app_from_word(value: u64) -> Option<DesktopAppId> {
    match value as u32 {
        x if x == DesktopAppId::Settings as u32 => Some(DesktopAppId::Settings),
        x if x == DesktopAppId::Files as u32 => Some(DesktopAppId::Files),
        x if x == DesktopAppId::Monitor as u32 => Some(DesktopAppId::Monitor),
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

fn app_state_label(app_id: DesktopAppId, running: bool) -> &'static str {
    match (app_id, running) {
        (DesktopAppId::Settings, true) => "SETTINGS RUN",
        (DesktopAppId::Settings, false) => "SETTINGS OFF",
        (DesktopAppId::Files, true) => "FILES RUN",
        (DesktopAppId::Files, false) => "FILES OFF",
        (DesktopAppId::Monitor, true) => "MONITOR RUN",
        (DesktopAppId::Monitor, false) => "MONITOR OFF",
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

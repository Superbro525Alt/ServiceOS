#![no_std]
#![no_main]

mod chrome;
mod input;
mod logging;
mod palette;
mod render;
mod requests;
mod state;
mod windows;

pub(crate) use palette::{palette_action_label, palette_matches};
pub(crate) use state::*;

use serviceos_userspace_runtime as rt;
use rt::{ControlTag, DesktopAppId, RawMessage, ServiceId, ServiceImageId};

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
    let chrome = match chrome::create_chrome(graphics_handle, output.width, output.height) {
        Ok(chrome) => chrome,
        Err(_) => return 0xfe08,
    };
    let palette_buffers = match serviceos_desktop_ui::SurfaceBuffers::<PALETTE_BUFFER_SLOTS>::new(
        chrome.palette_handle,
        PALETTE_WIDTH,
        PALETTE_HEIGHT,
        PALETTE_WIDTH,
        PALETTE_BUFFER_BYTES,
    ) {
        Ok(buffers) => buffers,
        Err(_) => return 0xfe17,
    };
    let palette_presenter = serviceos_desktop_ui::FirstPresentSurface::new(chrome.palette_handle);

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
        palette_buffers,
        palette_presenter,
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
        next_app_refresh: 0,
        next_status_refresh: 0,
        last_status_snapshot: None,
        pending_shell_refresh: false,
        pending_focus_refresh: false,
        pending_app_launch: None,
        next_z_order: 10,
        pointer_x: (output.width / 2) as i32,
        pointer_y: (output.height / 2) as i32,
        drag_state: None,
        content_capture: None,
        pending_resize: None,
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
    if chrome::show_chrome(&state.chrome).is_err() {
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
        let mut pending_pointer_move: Option<(i32, i32, i32)> = None;
        let mut request_budget = MAX_DESKTOP_REQUESTS_PER_TURN;
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
                    if let Some(move_request) = requests::coalescible_pointer_move(&request) {
                        pending_pointer_move = Some(move_request);
                        continue;
                    }
                    if let Some((x, y, detail)) = pending_pointer_move.take() {
                        if requests::dispatch_input_request(
                            &mut state,
                            rt::DesktopInputAction::PointerMove,
                            x,
                            y,
                            detail,
                            None,
                        )
                        .is_err()
                        {
                            return 0xfe0e;
                        }
                    }
                    if requests::handle_request(&mut state, &request).is_err() {
                        return 0xfe0e;
                    }
                    request_budget = request_budget.saturating_sub(1);
                    if request_budget == 0 {
                        break;
                    }
                }
                Err(rt::Error::QueueEmpty) => break,
                Err(_) => return 0xfe0f,
            }
        }

        if let Some((x, y, detail)) = pending_pointer_move.take() {
            if requests::dispatch_input_request(
                &mut state,
                rt::DesktopInputAction::PointerMove,
                x,
                y,
                detail,
                None,
            )
            .is_err()
            {
                return 0xfe0e;
            }
        }

        if windows::flush_pending_resize(&mut state).is_err() {
            return 0xfe16;
        }

        let now = match rt::monotonic_now() {
            Ok(now) => now,
            Err(_) => return 0xfe11,
        };
        if !did_work {
            if let Some(app_id) = state.pending_app_launch.take() {
                if windows::launch_or_focus_app(&mut state, app_id).is_err() {
                    return 0xfe19;
                }
                continue;
            }
            if state.pending_shell_refresh {
                if render::render_desktop(&mut state).is_err() {
                    return 0xfe18;
                }
                state.pending_shell_refresh = false;
                state.pending_focus_refresh = false;
            } else if state.pending_focus_refresh {
                if render::render_focus_chrome(&mut state).is_err() {
                    return 0xfe18;
                }
                state.pending_focus_refresh = false;
            }
            if now >= state.next_app_refresh {
                if windows::refresh_apps(&mut state).is_err() {
                    return 0xfe10;
                }
                state.next_app_refresh = now.saturating_add(APP_REFRESH_TICKS);
            }
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
        }

        if did_work {
            continue;
        }

        if rt::yield_current().is_err() {
            return 0xfe13;
        }
    }
}

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

mod access;
mod actions;
mod chrome;
mod crash;
mod input;
mod launcher_docs;
mod logging;
mod login;
mod media;
mod palette;
mod palette_docs;
mod render;
mod requests;
mod state;
mod switcher;
mod windows;

pub(crate) use palette::{palette_action_label, palette_matches};
pub(crate) use palette_docs::PaletteEntry;
pub(crate) use state::*;

use rt::{ControlTag, DesktopAppId, RawMessage, ServiceId, ServiceImageId};
use serviceos_userspace_runtime as rt;

rt::entry!(main);

const IDLE_WAIT_TICKS: u64 = 2;

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
    let audio_service_handle =
        rt::lookup_service(bootstrap, ServiceId::Audio).unwrap_or(rt::INVALID_HANDLE);
    let storage_handle =
        rt::lookup_service(bootstrap, ServiceId::Storage).unwrap_or(rt::INVALID_HANDLE);
    let access_store_dir = access::ensure_access_store_dir(storage_handle);
    let access_settings = access::load_access_settings(access_store_dir);

    let output = match rt::graphics_output_status(graphics_handle, 0) {
        Ok(Some(output)) => output,
        _ => return 0xfe07,
    };
    let chrome = match chrome::create_chrome(
        graphics_handle,
        output.width,
        output.height,
        access_settings.high_contrast,
    ) {
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
        audio_service_handle,
        chrome,
        palette_buffers,
        palette_presenter,
        apps: [
            AppSlot::new(DesktopAppId::Settings, ServiceImageId::SettingsApp),
            AppSlot::new(DesktopAppId::Files, ServiceImageId::FilesApp),
            AppSlot::new(DesktopAppId::Media, ServiceImageId::MediaApp),
            AppSlot::new(DesktopAppId::Monitor, ServiceImageId::MonitorApp),
            AppSlot::new(DesktopAppId::Terminal, ServiceImageId::TerminalApp),
            AppSlot::new(
                DesktopAppId::SoftwareCenter,
                ServiceImageId::SoftwareCenterApp,
            ),
        ],
        focused_app: None,
        active_workspace: 1,
        recent_focus: [DesktopAppId::Settings; APP_COUNT],
        recent_focus_len: 0,
        next_app_refresh: 0,
        next_status_refresh: 0,
        last_status_snapshot: None,
        pending_shell_refresh: rt::PendingFlag::new(),
        pending_focus_refresh: rt::PendingFlag::new(),
        pending_app_launch: rt::PendingValue::new(),
        next_z_order: 10,
        pointer_x: (output.width / 2) as i32,
        pointer_y: (output.height / 2) as i32,
        drag_state: None,
        drag_snap_zone: windows::SnapZone::None,
        content_capture: None,
        content_drag: None,
        pending_resize: None,
        notification: [0; MAX_NOTIFICATION_BYTES],
        notification_len: 0,
        notification_deadline: 0,
        notification_history: [NotificationEntry::empty(); NOTIFICATION_HISTORY_MAX],
        notification_history_len: 0,
        next_notification_sequence: 1,
        overlay_mode: OverlayMode::None,
        overlay_selection: 0,
        switcher_selection: 0,
        palette_query: [0; PALETTE_QUERY_MAX],
        palette_query_len: 0,
        login: login::LoginState::new(),
        shell_client: rt::INVALID_HANDLE,
        login_endpoint: rt::INVALID_HANDLE,
        storage_handle,
        doc_hits: [crate::palette_docs::DocHit {
            path: [0; crate::palette_docs::DOC_PATH_MAX],
            path_len: 0,
            kind: 0,
            line: 0,
        }; crate::palette_docs::DOC_HITS_MAX],
        doc_hits_len: 0,
        launcher_docs: [crate::palette_docs::DocHit {
            path: [0; crate::palette_docs::DOC_PATH_MAX],
            path_len: 0,
            kind: 0,
            line: 0,
        }; crate::launcher_docs::LAUNCHER_DOCS_MAX],
        launcher_docs_len: 0,
        launcher_docs_rendered: 0,
        next_launcher_docs_refresh: 0,
        master_volume: media::MASTER_VOLUME_DEFAULT,
        master_muted: false,
        pending_media_refresh: rt::PendingFlag::new(),
        animations: [None; windows::ANIM_QUEUE_MAX],
        shadow_surface_id: 0,
        shadow_surface_handle: rt::INVALID_HANDLE,
        shadow_width: 0,
        shadow_height: 0,
        access: access_settings,
        access_store_dir,
        corner_dwell: access::CornerDwell::new(),
        show_desktop_active: false,
        show_desktop_restore_mask: 0,
        zoom_applied: false,
        zoom_last_fx: -1,
        zoom_last_fy: -1,
        zoom_last_index: 0,
    };

    // Panel document section: seed from the persistent files-app recent
    // ring before the first render so a returning session shows its
    // recent documents immediately. Fresh boots (no ring) stay app-only.
    launcher_docs::refresh_launcher_docs(&mut state);

    if render::render_desktop(&mut state).is_err() {
        return 0xfe0b;
    }
    if render::sync_cursor(&state).is_err() {
        return 0xfe14;
    }
    if access::sync_zoom(&mut state).is_err() {
        return 0xfe1d;
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
                        // E2E: a fresher pointer sample replacing a pending one
                        // is the shell-visible "lost" counter (gated).
                        if pending_pointer_move.is_some() {
                            crate::input::e2e::note_coalesced_drop();
                        }
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
        windows::step_animations(&mut state, now);
        if access::sync_zoom(&mut state).is_err() {
            return 0xfe1d;
        }
        if !did_work {
            let corner_now = access::corner_at(
                state.pointer_x,
                state.pointer_y,
                output.width as i32,
                output.height as i32,
            );
            if let Some(corner) = state.corner_dwell.update(corner_now, now) {
                if input::fire_corner_action(&mut state, corner).is_err() {
                    return 0xfe1c;
                }
            }
            if let Some(app_id) = state.pending_app_launch.take() {
                if windows::launch_or_focus_app(&mut state, app_id).is_err() {
                    return 0xfe19;
                }
                continue;
            }
            if state.pending_shell_refresh.take() {
                if render::render_desktop(&mut state).is_err() {
                    return 0xfe18;
                }
                state.pending_focus_refresh.clear();
            } else if state.pending_focus_refresh.take() {
                if render::render_focus_chrome(&mut state).is_err() {
                    return 0xfe18;
                }
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
            if state
                .content_drag
                .as_ref()
                .is_some_and(|drag| drag.expired(now))
            {
                state.content_drag = None;
                if render::render_desktop(&mut state).is_err() {
                    return 0xfe1b;
                }
            }
            if now >= state.next_status_refresh {
                if render::refresh_desktop_status(&mut state).is_err() {
                    return 0xfe12;
                }
                state.next_status_refresh = now.saturating_add(STATUS_REFRESH_TICKS);
                if state.overlay_mode == OverlayMode::Media {
                    if render::render_overlays_only(&mut state).is_err() {
                        return 0xfe1a;
                    }
                }
            }
            if now >= state.next_launcher_docs_refresh {
                state.next_launcher_docs_refresh = now.saturating_add(LAUNCHER_DOCS_REFRESH_TICKS);
                if launcher_docs::refresh_launcher_docs(&mut state)
                    && render::render_desktop(&mut state).is_err()
                {
                    return 0xfe1e;
                }
            }
        }

        if did_work {
            continue;
        }

        let mut waited = RawMessage::empty(0);
        match rt::channel_receive_blocking_timeout(public.first, &mut waited, IDLE_WAIT_TICKS) {
            Ok(()) => {
                if let Some((x, y, detail)) = requests::coalescible_pointer_move(&waited) {
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
                } else if requests::handle_request(&mut state, &waited).is_err() {
                    return 0xfe0e;
                }
                continue;
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xfe13,
        }
    }
}

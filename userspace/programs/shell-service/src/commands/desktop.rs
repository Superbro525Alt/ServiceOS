use serviceos_userspace_runtime as rt;
use rt::{DesktopAppId, DesktopAppInfo, DesktopWindowInfo, ServiceId};

use crate::util::{
    desktop_app_name, desktop_drag_name, parse_desktop_app_name, write_session_linef,
    MAX_DESKTOP_APPS, MAX_DESKTOP_WINDOWS,
};

pub(crate) fn cmd_desktop<'a, I>(
    bootstrap: rt::Handle,
    session: rt::Handle,
    mut parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("status") => cmd_desktop_status(bootstrap, session),
        Some("apps") => cmd_desktop_apps(bootstrap, session),
        Some("windows") => cmd_desktop_windows(bootstrap, session),
        Some("launch") => match parts.next().and_then(parse_desktop_app_name) {
            Some(app_id) => cmd_desktop_launch(bootstrap, session, app_id),
            None => write_session_linef(
                session,
                format_args!("usage: desktop launch <settings|files|monitor>"),
            ),
        },
        Some("focus") => match parts.next().and_then(parse_desktop_app_name) {
            Some(app_id) => cmd_desktop_focus(bootstrap, session, app_id),
            None => write_session_linef(
                session,
                format_args!("usage: desktop focus <settings|files|monitor>"),
            ),
        },
        Some("next") => cmd_desktop_next(bootstrap, session),
        Some("close") => match parts.next().and_then(parse_desktop_app_name) {
            Some(app_id) => cmd_desktop_close(bootstrap, session, app_id),
            None => write_session_linef(
                session,
                format_args!("usage: desktop close <settings|files|monitor>"),
            ),
        },
        Some("minimize") => match parts.next().and_then(parse_desktop_app_name) {
            Some(app_id) => cmd_desktop_minimize(bootstrap, session, app_id),
            None => write_session_linef(
                session,
                format_args!("usage: desktop minimize <settings|files|monitor>"),
            ),
        },
        Some("restore") => match parts.next().and_then(parse_desktop_app_name) {
            Some(app_id) => cmd_desktop_restore(bootstrap, session, app_id),
            None => write_session_linef(
                session,
                format_args!("usage: desktop restore <settings|files|monitor>"),
            ),
        },
        Some("maximize") => match parts.next().and_then(parse_desktop_app_name) {
            Some(app_id) => cmd_desktop_maximize(bootstrap, session, app_id),
            None => write_session_linef(
                session,
                format_args!("usage: desktop maximize <settings|files|monitor>"),
            ),
        },
        Some("move") => match (
            parts.next().and_then(parse_desktop_app_name),
            parts.next().and_then(|value| value.parse::<i32>().ok()),
            parts.next().and_then(|value| value.parse::<i32>().ok()),
        ) {
            (Some(app_id), Some(x), Some(y)) => cmd_desktop_move(bootstrap, session, app_id, x, y),
            _ => write_session_linef(
                session,
                format_args!("usage: desktop move <settings|files|monitor> <x> <y>"),
            ),
        },
        Some("resize") => match (
            parts.next().and_then(parse_desktop_app_name),
            parts.next().and_then(|value| value.parse::<u32>().ok()),
            parts.next().and_then(|value| value.parse::<u32>().ok()),
        ) {
            (Some(app_id), Some(width), Some(height)) => {
                cmd_desktop_resize(bootstrap, session, app_id, width, height)
            }
            _ => write_session_linef(
                session,
                format_args!("usage: desktop resize <settings|files|monitor> <width> <height>"),
            ),
        },
        Some("click") => match (
            parts.next().and_then(|value| value.parse::<i32>().ok()),
            parts.next().and_then(|value| value.parse::<i32>().ok()),
        ) {
            (Some(x), Some(y)) => cmd_desktop_click(bootstrap, session, x, y),
            _ => write_session_linef(session, format_args!("usage: desktop click <x> <y>")),
        },
        _ => write_session_linef(
            session,
            format_args!(
                "usage: desktop <status|apps|windows|launch|focus|next|close|minimize|restore|maximize|move|resize|click> ..."
            ),
        ),
    }
}

fn cmd_desktop_status(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let status = rt::desktop_status(desktop_handle)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!(
            "session={} focused-app={} focused-surface={} running-apps={} drag={} pointer=({}, {})",
            status.session_id,
            status.focused_app.map(desktop_app_name).unwrap_or("none"),
            status.focused_surface,
            status.running_apps,
            desktop_drag_name(status.drag_mode),
            status.pointer_x,
            status.pointer_y,
        ),
    )
}

fn cmd_desktop_apps(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let mut apps = [DesktopAppInfo {
        app_id: DesktopAppId::Settings,
        running: false,
        focused: false,
        surface_id: 0,
    }; MAX_DESKTOP_APPS];
    let count = rt::desktop_list_apps(desktop_handle, &mut apps)?;
    let _ = rt::handle_close(desktop_handle);
    if count == 0 {
        return write_session_linef(session, format_args!("no desktop apps"));
    }
    for app in apps.iter().copied().take(count) {
        write_session_linef(
            session,
            format_args!(
                "{:<10} running={} focused={} surface={}",
                desktop_app_name(app.app_id),
                app.running,
                app.focused,
                app.surface_id,
            ),
        )?;
    }
    Ok(())
}

fn cmd_desktop_windows(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let mut windows = [DesktopWindowInfo {
        app_id: DesktopAppId::Settings,
        surface_id: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        z_order: 0,
        focused: false,
        minimized: false,
        visible: false,
    }; MAX_DESKTOP_WINDOWS];
    let count = rt::desktop_list_windows(desktop_handle, &mut windows)?;
    let _ = rt::handle_close(desktop_handle);
    if count == 0 {
        return write_session_linef(session, format_args!("no desktop windows"));
    }
    for window in windows.iter().copied().take(count) {
        write_session_linef(
            session,
            format_args!(
                "{:<10} surface={} pos=({}, {}) size={}x{} z={} focused={} minimized={} visible={}",
                desktop_app_name(window.app_id),
                window.surface_id,
                window.x,
                window.y,
                window.width,
                window.height,
                window.z_order,
                window.focused,
                window.minimized,
                window.visible,
            ),
        )?;
    }
    Ok(())
}

fn cmd_desktop_launch(
    bootstrap: rt::Handle,
    session: rt::Handle,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_launch_app(desktop_handle, app_id)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!("launched {} on surface {}", desktop_app_name(app_id), surface_id),
    )
}

fn cmd_desktop_focus(
    bootstrap: rt::Handle,
    session: rt::Handle,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_focus_app(desktop_handle, app_id)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!("focused {} on surface {}", desktop_app_name(app_id), surface_id),
    )
}

fn cmd_desktop_next(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_focus_next(desktop_handle)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!("focused next visible window on surface {}", surface_id),
    )
}

fn cmd_desktop_close(
    bootstrap: rt::Handle,
    session: rt::Handle,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    rt::desktop_close_app(desktop_handle, app_id)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(session, format_args!("closed {}", desktop_app_name(app_id)))
}

fn cmd_desktop_minimize(
    bootstrap: rt::Handle,
    session: rt::Handle,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_minimize_app(desktop_handle, app_id)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!("minimized {} on surface {}", desktop_app_name(app_id), surface_id),
    )
}

fn cmd_desktop_restore(
    bootstrap: rt::Handle,
    session: rt::Handle,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_restore_app(desktop_handle, app_id)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!("restored {} on surface {}", desktop_app_name(app_id), surface_id),
    )
}

fn cmd_desktop_maximize(
    bootstrap: rt::Handle,
    session: rt::Handle,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_maximize_app(desktop_handle, app_id)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!("maximized {} on surface {}", desktop_app_name(app_id), surface_id),
    )
}

fn cmd_desktop_move(
    bootstrap: rt::Handle,
    session: rt::Handle,
    app_id: DesktopAppId,
    x: i32,
    y: i32,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_move_app(desktop_handle, app_id, x, y)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!(
            "moved {} to ({}, {}) on surface {}",
            desktop_app_name(app_id),
            x,
            y,
            surface_id,
        ),
    )
}

fn cmd_desktop_resize(
    bootstrap: rt::Handle,
    session: rt::Handle,
    app_id: DesktopAppId,
    width: u32,
    height: u32,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_resize_app(desktop_handle, app_id, width, height)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!(
            "resized {} to {}x{} on surface {}",
            desktop_app_name(app_id),
            width,
            height,
            surface_id,
        ),
    )
}

fn cmd_desktop_click(
    bootstrap: rt::Handle,
    session: rt::Handle,
    x: i32,
    y: i32,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_pointer_click(desktop_handle, x, y)?;
    let _ = rt::handle_close(desktop_handle);
    write_session_linef(
        session,
        format_args!("desktop click at ({}, {}) targeted surface {}", x, y, surface_id),
    )
}

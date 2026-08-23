use core::fmt::Write;

use rt::{
    DesktopAppId, DesktopAppInfo, DesktopWindowInfo, DesktopWorkspaceAction, FixedLogBuffer,
    ServiceId,
};
use serviceos_userspace_runtime as rt;

use crate::util::{
    MAX_DESKTOP_APPS, MAX_DESKTOP_WINDOWS, ShellOutput, desktop_app_name, desktop_drag_name,
    parse_desktop_app_name, write_output_linef,
};

pub(crate) fn cmd_desktop<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    mut parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("status") => cmd_desktop_status(bootstrap, output),
        Some("apps") => cmd_desktop_apps(bootstrap, output),
        Some("windows") => cmd_desktop_windows(bootstrap, output),
        Some("workspace") => cmd_desktop_workspace(bootstrap, output, parts),
        Some("notifications") => cmd_desktop_notifications(bootstrap, output, parts),
        Some("launch") => match parts.next().and_then(parse_desktop_app_name) {
            Some(app_id) => cmd_desktop_launch(bootstrap, output, app_id),
            None => write_output_linef(
                output,
                format_args!("usage: desktop launch <settings|files|monitor|terminal>"),
            ),
        },
        Some("focus") => match parts.next().and_then(parse_desktop_app_name) {
            Some(app_id) => cmd_desktop_focus(bootstrap, output, app_id),
            None => write_output_linef(
                output,
                format_args!("usage: desktop focus <settings|files|monitor|terminal>"),
            ),
        },
        Some("next") => cmd_desktop_next(bootstrap, output),
        Some("close") => match parts.next().and_then(parse_desktop_app_name) {
            Some(app_id) => cmd_desktop_close(bootstrap, output, app_id),
            None => write_output_linef(
                output,
                format_args!("usage: desktop close <settings|files|monitor|terminal>"),
            ),
        },
        Some("minimize") => match parts.next().and_then(parse_desktop_app_name) {
            Some(app_id) => cmd_desktop_minimize(bootstrap, output, app_id),
            None => write_output_linef(
                output,
                format_args!("usage: desktop minimize <settings|files|monitor|terminal>"),
            ),
        },
        Some("restore") => match parts.next().and_then(parse_desktop_app_name) {
            Some(app_id) => cmd_desktop_restore(bootstrap, output, app_id),
            None => write_output_linef(
                output,
                format_args!("usage: desktop restore <settings|files|monitor|terminal>"),
            ),
        },
        Some("maximize") => match parts.next().and_then(parse_desktop_app_name) {
            Some(app_id) => cmd_desktop_maximize(bootstrap, output, app_id),
            None => write_output_linef(
                output,
                format_args!("usage: desktop maximize <settings|files|monitor|terminal>"),
            ),
        },
        Some("move") => match (
            parts.next().and_then(parse_desktop_app_name),
            parts.next().and_then(|value| value.parse::<i32>().ok()),
            parts.next().and_then(|value| value.parse::<i32>().ok()),
        ) {
            (Some(app_id), Some(x), Some(y)) => cmd_desktop_move(bootstrap, output, app_id, x, y),
            _ => write_output_linef(
                output,
                format_args!("usage: desktop move <settings|files|monitor|terminal> <x> <y>"),
            ),
        },
        Some("resize") => match (
            parts.next().and_then(parse_desktop_app_name),
            parts.next().and_then(|value| value.parse::<u32>().ok()),
            parts.next().and_then(|value| value.parse::<u32>().ok()),
        ) {
            (Some(app_id), Some(width), Some(height)) => {
                cmd_desktop_resize(bootstrap, output, app_id, width, height)
            }
            _ => write_output_linef(
                output,
                format_args!(
                    "usage: desktop resize <settings|files|monitor|terminal> <width> <height>"
                ),
            ),
        },
        Some("click") => match (
            parts.next().and_then(|value| value.parse::<i32>().ok()),
            parts.next().and_then(|value| value.parse::<i32>().ok()),
        ) {
            (Some(x), Some(y)) => cmd_desktop_click(bootstrap, output, x, y),
            _ => write_output_linef(output, format_args!("usage: desktop click <x> <y>")),
        },
        Some("notify") => {
            let mut message = FixedLogBuffer::<128>::new();
            let mut wrote_any = false;
            for part in parts {
                if wrote_any {
                    let _ = message.write_str(" ");
                }
                let _ = message.write_str(part);
                wrote_any = true;
            }
            let message = core::str::from_utf8(message.as_bytes()).unwrap_or("");
            if message.is_empty() {
                write_output_linef(output, format_args!("usage: desktop notify <text>"))
            } else {
                cmd_desktop_notify(bootstrap, output, &message)
            }
        }
        Some("open") => match parts.next() {
            Some(path) => cmd_desktop_open(bootstrap, output, path),
            None => write_output_linef(output, format_args!("usage: desktop open <path>")),
        },
        _ => write_output_linef(
            output,
            format_args!(
                "usage: desktop <status|apps|windows|workspace|notifications|launch|focus|next|close|minimize|restore|maximize|move|resize|click|notify|open> ..."
            ),
        ),
    }
}

fn cmd_desktop_status(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let status = rt::desktop_status(desktop_handle)?;
    let _ = rt::handle_close(desktop_handle);
    write_output_linef(
        output,
        format_args!(
            "session={} focused-app={} focused-surface={} running-apps={} workspace={}/{} notifications={} drag={} pointer=({}, {})",
            status.session_id,
            status.focused_app.map(desktop_app_name).unwrap_or("none"),
            status.focused_surface,
            status.running_apps,
            status.active_workspace,
            status.workspace_count,
            status.notification_count,
            desktop_drag_name(status.drag_mode),
            status.pointer_x,
            status.pointer_y,
        ),
    )
}

fn cmd_desktop_apps(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
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
        return write_output_linef(output, format_args!("no desktop apps"));
    }
    for app in apps.iter().copied().take(count) {
        write_output_linef(
            output,
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

fn cmd_desktop_windows(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
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
        return write_output_linef(output, format_args!("no desktop windows"));
    }
    for window in windows.iter().copied().take(count) {
        write_output_linef(
            output,
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
    output: ShellOutput,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_launch_app(desktop_handle, app_id)?;
    let _ = rt::handle_close(desktop_handle);
    write_output_linef(
        output,
        format_args!(
            "launched {} on surface {}",
            desktop_app_name(app_id),
            surface_id
        ),
    )
}

fn cmd_desktop_focus(
    bootstrap: rt::Handle,
    output: ShellOutput,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_focus_app(desktop_handle, app_id)?;
    let _ = rt::handle_close(desktop_handle);
    write_output_linef(
        output,
        format_args!(
            "focused {} on surface {}",
            desktop_app_name(app_id),
            surface_id
        ),
    )
}

fn cmd_desktop_next(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_focus_next(desktop_handle)?;
    let _ = rt::handle_close(desktop_handle);
    write_output_linef(
        output,
        format_args!("focused next visible window on surface {}", surface_id),
    )
}

fn cmd_desktop_notify(bootstrap: rt::Handle, output: ShellOutput, text: &str) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    rt::desktop_notify(desktop_handle, text)?;
    let _ = rt::handle_close(desktop_handle);
    write_output_linef(output, format_args!("notification posted"))
}

fn cmd_desktop_open(bootstrap: rt::Handle, output: ShellOutput, path: &str) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_open_path(desktop_handle, path)?;
    let _ = rt::handle_close(desktop_handle);
    write_output_linef(
        output,
        format_args!("opened {} via files on surface {}", path, surface_id),
    )
}

fn cmd_desktop_workspace<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    mut parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let result = match parts.next() {
        None | Some("status") => {
            rt::desktop_workspace_action(desktop_handle, DesktopWorkspaceAction::Status, 0)
        }
        Some("switch") => match parts.next().and_then(|value| value.parse::<u32>().ok()) {
            Some(workspace_id) => rt::desktop_workspace_action(
                desktop_handle,
                DesktopWorkspaceAction::Switch,
                workspace_id,
            ),
            None => {
                let _ = rt::handle_close(desktop_handle);
                return write_output_linef(
                    output,
                    format_args!("usage: desktop workspace switch <1-4>"),
                );
            }
        },
        Some("move") => match parts.next().and_then(|value| value.parse::<u32>().ok()) {
            Some(workspace_id) => rt::desktop_workspace_action(
                desktop_handle,
                DesktopWorkspaceAction::MoveFocused,
                workspace_id,
            ),
            None => {
                let _ = rt::handle_close(desktop_handle);
                return write_output_linef(
                    output,
                    format_args!("usage: desktop workspace move <1-4>"),
                );
            }
        },
        _ => {
            let _ = rt::handle_close(desktop_handle);
            return write_output_linef(
                output,
                format_args!("usage: desktop workspace <status|switch|move> [id]"),
            );
        }
    }?;
    let _ = rt::handle_close(desktop_handle);
    write_output_linef(
        output,
        format_args!(
            "workspace={}/{} focused-surface={}",
            result.active_workspace, result.workspace_count, result.focused_surface,
        ),
    )
}

fn cmd_desktop_notifications<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    mut parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    let requested = parts
        .next()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(6);
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let mut any = false;
    for index in 0..requested {
        match rt::desktop_notification_history(desktop_handle, index as u32) {
            Ok(entry) => {
                any = true;
                let text = core::str::from_utf8(&entry.text[..entry.text_len as usize])
                    .unwrap_or("NOTICE");
                write_output_linef(
                    output,
                    format_args!(
                        "#{:<2} app={} actionable={} {}",
                        entry.sequence,
                        entry.source_app.map(desktop_app_name).unwrap_or("shell"),
                        entry.actionable,
                        text,
                    ),
                )?;
            }
            Err(rt::Error::NotFound) => break,
            Err(error) => {
                let _ = rt::handle_close(desktop_handle);
                return Err(error);
            }
        }
    }
    let _ = rt::handle_close(desktop_handle);
    if !any {
        return write_output_linef(output, format_args!("no desktop notifications"));
    }
    Ok(())
}

fn cmd_desktop_close(
    bootstrap: rt::Handle,
    output: ShellOutput,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    rt::desktop_close_app(desktop_handle, app_id)?;
    let _ = rt::handle_close(desktop_handle);
    write_output_linef(output, format_args!("closed {}", desktop_app_name(app_id)))
}

fn cmd_desktop_minimize(
    bootstrap: rt::Handle,
    output: ShellOutput,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_minimize_app(desktop_handle, app_id)?;
    let _ = rt::handle_close(desktop_handle);
    write_output_linef(
        output,
        format_args!(
            "minimized {} on surface {}",
            desktop_app_name(app_id),
            surface_id
        ),
    )
}

fn cmd_desktop_restore(
    bootstrap: rt::Handle,
    output: ShellOutput,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_restore_app(desktop_handle, app_id)?;
    let _ = rt::handle_close(desktop_handle);
    write_output_linef(
        output,
        format_args!(
            "restored {} on surface {}",
            desktop_app_name(app_id),
            surface_id
        ),
    )
}

fn cmd_desktop_maximize(
    bootstrap: rt::Handle,
    output: ShellOutput,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_maximize_app(desktop_handle, app_id)?;
    let _ = rt::handle_close(desktop_handle);
    write_output_linef(
        output,
        format_args!(
            "maximized {} on surface {}",
            desktop_app_name(app_id),
            surface_id
        ),
    )
}

fn cmd_desktop_move(
    bootstrap: rt::Handle,
    output: ShellOutput,
    app_id: DesktopAppId,
    x: i32,
    y: i32,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_move_app(desktop_handle, app_id, x, y)?;
    let _ = rt::handle_close(desktop_handle);
    write_output_linef(
        output,
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
    output: ShellOutput,
    app_id: DesktopAppId,
    width: u32,
    height: u32,
) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_resize_app(desktop_handle, app_id, width, height)?;
    let _ = rt::handle_close(desktop_handle);
    write_output_linef(
        output,
        format_args!(
            "resized {} to {}x{} on surface {}",
            desktop_app_name(app_id),
            width,
            height,
            surface_id,
        ),
    )
}

fn cmd_desktop_click(bootstrap: rt::Handle, output: ShellOutput, x: i32, y: i32) -> rt::Result<()> {
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let surface_id = rt::desktop_pointer_click(desktop_handle, x, y)?;
    let _ = rt::handle_close(desktop_handle);
    write_output_linef(
        output,
        format_args!(
            "desktop click at ({}, {}) targeted surface {}",
            x, y, surface_id
        ),
    )
}

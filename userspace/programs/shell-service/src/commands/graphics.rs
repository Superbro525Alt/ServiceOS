use serviceos_userspace_runtime as rt;
use rt::ServiceId;

use crate::util::{
    display_backend_name, display_state_name, pixel_format_name, session_input_source_name,
    write_output_linef, ShellOutput,
};

pub(crate) fn cmd_gfx<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    mut parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("outputs") => cmd_gfx_outputs(bootstrap, output),
        Some("surfaces") => cmd_gfx_surfaces(bootstrap, output),
        Some("sessions") => cmd_gfx_sessions(bootstrap, output),
        Some("focus") => match parts.next().and_then(|value| value.parse::<u32>().ok()) {
            Some(surface_id) => cmd_gfx_focus(bootstrap, output, surface_id),
            None => write_output_linef(output, format_args!("usage: gfx focus <surface-id>")),
        },
        _ => write_output_linef(
            output,
            format_args!("usage: gfx <outputs|surfaces|sessions|focus> ..."),
        ),
    }
}

fn cmd_gfx_outputs(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let graphics_handle = rt::lookup_service(bootstrap, ServiceId::Graphics)?;
    let count = rt::graphics_output_count(graphics_handle)?;
    if count == 0 {
        let _ = rt::handle_close(graphics_handle);
        return write_output_linef(output, format_args!("no outputs"));
    }

    for index in 0..count {
        if let Some(display) = rt::graphics_output_status(graphics_handle, index)? {
            write_output_linef(
                output,
                format_args!(
                    "out{} backend={} state={} mode={}x{} stride={} format={} surfaces={} presents={}",
                    display.index,
                    display_backend_name(display.backend),
                    display_state_name(display.state),
                    display.width,
                    display.height,
                    display.stride,
                    pixel_format_name(display.pixel_format),
                    display.surface_count,
                    display.present_count,
                ),
            )?;
        }
    }

    let _ = rt::handle_close(graphics_handle);
    Ok(())
}

fn cmd_gfx_surfaces(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let graphics_handle = rt::lookup_service(bootstrap, ServiceId::Graphics)?;
    let mut surface_ids = [0u32; 8];
    let count = rt::graphics_surface_list(graphics_handle, &mut surface_ids)?;
    if count == 0 {
        let _ = rt::handle_close(graphics_handle);
        return write_output_linef(output, format_args!("no surfaces"));
    }

    for surface_id in surface_ids.iter().copied().take(count) {
        if let Some(surface) = rt::graphics_surface_status(graphics_handle, surface_id)? {
            write_output_linef(
                output,
                format_args!(
                    "surface{} session={} pos=({}, {}) size={}x{} z={} color=#{:06x} visible={}",
                    surface.surface_id,
                    surface.owner_session,
                    surface.x,
                    surface.y,
                    surface.width,
                    surface.height,
                    surface.z_order,
                    surface.fill_rgb,
                    surface.visible,
                ),
            )?;
        }
    }

    let _ = rt::handle_close(graphics_handle);
    Ok(())
}

fn cmd_gfx_sessions(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let session_handle = rt::lookup_service(bootstrap, ServiceId::Session)?;
    let mut session_ids = [0u32; 4];
    let count = rt::session_list(session_handle, &mut session_ids)?;
    if count == 0 {
        let _ = rt::handle_close(session_handle);
        return write_output_linef(output, format_args!("no sessions"));
    }

    for session_id in session_ids.iter().copied().take(count) {
        if let Some(status) = rt::session_status(session_handle, session_id)? {
            write_output_linef(
                output,
                format_args!(
                    "session{} input={} focused-surface={} surfaces={}",
                    status.session_id,
                    session_input_source_name(status.input_source),
                    status.focused_surface,
                    status.surface_count,
                ),
            )?;
        }
    }

    let _ = rt::handle_close(session_handle);
    Ok(())
}

fn cmd_gfx_focus(bootstrap: rt::Handle, output: ShellOutput, surface_id: u32) -> rt::Result<()> {
    let session_handle = rt::lookup_service(bootstrap, ServiceId::Session)?;
    let focused_surface = rt::session_focus(session_handle, 1, surface_id)?;
    let _ = rt::handle_close(session_handle);
    write_output_linef(
        output,
        format_args!("focused graphical surface {}", focused_surface),
    )
}

use core::{fmt::Write, str};

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{ConfigKey, FixedLogBuffer};

use crate::security::{
    audit_kind_name, first_actionable_runtime, image_name, policy_name, runtime_env_state_name,
    security_policy_count, PermissionSummary, RuntimeCapSummary,
};
use crate::state::*;

pub(crate) fn render(
    surface_handle: rt::Handle,
    buffer_slot: u32,
    buffer: &mut rt::MappedMemory,
    config_handle: rt::Handle,
    network_handle: rt::Handle,
    audio_handle: rt::Handle,
    runtime_handle: rt::Handle,
    security_handle: rt::Handle,
    state: &AppState,
) -> rt::Result<()> {
    let width = state.width.min(BUFFER_WIDTH) as usize;
    let height = state.height.min(BUFFER_HEIGHT) as usize;
    let bytes = &mut buffer.as_slice_mut()[..BUFFER_BYTES];

    fill_rect(bytes, 0, 0, width, height, ui::BG_WINDOW_ALT);
    fill_rect(
        bytes,
        0,
        0,
        width,
        ui::TITLEBAR_HEIGHT as usize,
        if state.focused { ui::ACCENT } else { ui::ACCENT_DIM },
    );
    fill_rect(
        bytes,
        0,
        ui::TITLEBAR_HEIGHT as usize,
        width,
        height.saturating_sub(ui::TITLEBAR_HEIGHT as usize),
        ui::BG_WINDOW_ALT,
    );
    draw_titlebar(bytes, width);
    draw_tabs(bytes, state.page);

    match state.page {
        SettingsPage::System => draw_system_page(
            bytes,
            config_handle,
            network_handle,
            audio_handle,
            state.editing_note,
            &state.note[..state.note_len],
        ),
        SettingsPage::Security => draw_security_page(
            bytes,
            runtime_handle,
            security_handle,
            state.selected_policy_index,
        )?,
    }

    rt::surface_present_buffer_slot(
        surface_handle,
        buffer_slot,
        0,
        0,
        width as u32,
        height as u32,
    )
}

fn draw_system_page(
    bytes: &mut [u8],
    config_handle: rt::Handle,
    network_handle: rt::Handle,
    audio_handle: rt::Handle,
    editing_note: bool,
    note: &[u8],
) {
    let min_level = rt::config_read(config_handle, ConfigKey::LogMinimumSeverity)
        .map(|(_, value)| value)
        .unwrap_or(0);
    let heartbeat = rt::config_read(config_handle, ConfigKey::StatusHeartbeatTicks)
        .map(|(_, value)| value)
        .unwrap_or(0);
    let interface = rt::network_interface_status(network_handle, 0).unwrap_or(None);
    let note_text = str::from_utf8(note).unwrap_or("");

    let mut line0 = FixedLogBuffer::<48>::new();
    let _ = write!(&mut line0, "SYSTEM OVERVIEW");
    let mut line1 = FixedLogBuffer::<48>::new();
    let _ = write!(&mut line1, "LOG MINIMUM {}", min_level);
    let mut line2 = FixedLogBuffer::<48>::new();
    let _ = write!(&mut line2, "HEARTBEAT {}", heartbeat);
    let mut line3 = FixedLogBuffer::<48>::new();
    if let Some(info) = interface {
        let _ = write!(
            &mut line3,
            "IP {}.{}.{}.{}",
            (info.address >> 24) & 0xff,
            (info.address >> 16) & 0xff,
            (info.address >> 8) & 0xff,
            info.address & 0xff,
        );
    } else {
        let _ = write!(&mut line3, "IP UNAVAILABLE");
    }
    let mut line4 = FixedLogBuffer::<48>::new();
    if let Some(info) = interface {
        let _ = write!(
            &mut line4,
            "GATEWAY {}.{}.{}.{}",
            (info.gateway >> 24) & 0xff,
            (info.gateway >> 16) & 0xff,
            (info.gateway >> 8) & 0xff,
            info.gateway & 0xff,
        );
    } else {
        let _ = write!(&mut line4, "GATEWAY UNAVAILABLE");
    }
    let mut audio_line = FixedLogBuffer::<48>::new();
    if let Some(endpoint) = rt::audio_service_endpoint_status(audio_handle, 0).unwrap_or(None) {
        let _ = write!(
            &mut audio_line,
            "AUDIO {} {}HZ {}",
            match endpoint.state {
                rt::AudioEndpointState::Offline => "OFFLINE",
                rt::AudioEndpointState::Idle => "IDLE",
                rt::AudioEndpointState::Active => "ACTIVE",
            },
            endpoint.current_frequency_hz,
            endpoint.play_count,
        );
    } else {
        let _ = write!(&mut audio_line, "AUDIO UNAVAILABLE");
    }

    for (index, line) in [
        str::from_utf8(line0.as_bytes()).unwrap_or("SYSTEM OVERVIEW"),
        str::from_utf8(line1.as_bytes()).unwrap_or("LOG MINIMUM ?"),
        str::from_utf8(line2.as_bytes()).unwrap_or("HEARTBEAT ?"),
        str::from_utf8(line3.as_bytes()).unwrap_or("IP ?"),
        str::from_utf8(line4.as_bytes()).unwrap_or("GATEWAY ?"),
    ]
    .into_iter()
    .enumerate()
    {
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            12,
            70 + (index as i32 * 10),
            if index == 0 { ui::TEXT_PRIMARY } else { ui::TEXT_SECONDARY },
            line,
        );
    }

    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, NOTE_FIELD_X0, 106, ui::TEXT_MUTED, "NOTE");
    fill_rect(
        bytes,
        NOTE_FIELD_X0.max(0) as usize,
        NOTE_FIELD_Y0.max(0) as usize,
        (NOTE_FIELD_X1 - NOTE_FIELD_X0).max(0) as usize,
        (NOTE_FIELD_Y1 - NOTE_FIELD_Y0).max(0) as usize,
        if editing_note { ui::ACCENT } else { ui::ACCENT_DIM },
    );
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, NOTE_FIELD_X0 + 8, NOTE_FIELD_Y0 + 7, ui::BG_PANEL, note_text);

    draw_button(
        bytes,
        AUDIO_TEST_X0,
        AUDIO_TEST_Y0,
        AUDIO_TEST_X1,
        AUDIO_TEST_Y1,
        ui::ACCENT_DIM,
        "TEST TONE",
        ui::TEXT_PRIMARY,
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        12,
        174,
        ui::TEXT_MUTED,
        str::from_utf8(audio_line.as_bytes()).unwrap_or("AUDIO"),
    );
}

fn draw_security_page(
    bytes: &mut [u8],
    runtime_handle: rt::Handle,
    security_handle: rt::Handle,
    selected_policy_index: usize,
) -> rt::Result<()> {
    let policy_count = security_policy_count(security_handle)?;
    let policy_index = if policy_count == 0 {
        0
    } else {
        selected_policy_index.min(policy_count - 1)
    };
    let policy = if policy_count == 0 {
        None
    } else {
        rt::security_policy_list(security_handle, policy_index)?
    };
    let pending_runtime = first_actionable_runtime(runtime_handle)?;
    let latest_native_audit = rt::security_audit_list(security_handle, 0)?;
    let latest_runtime_audit = if runtime_handle != rt::INVALID_HANDLE {
        rt::runtime_audit_list(runtime_handle, 0)?
    } else {
        None
    };

    let mut line0 = FixedLogBuffer::<64>::new();
    if let Some(policy) = policy {
        let name = str::from_utf8(&policy.name[..policy.name_len as usize]).unwrap_or("?");
        let _ = write!(&mut line0, "APP {} ({}/{})", name, policy_index + 1, policy_count);
    } else {
        let _ = write!(&mut line0, "APP no registered policies");
    }

    let mut line1 = FixedLogBuffer::<64>::new();
    let mut line2 = FixedLogBuffer::<64>::new();
    let mut line3 = FixedLogBuffer::<64>::new();
    if let Some(policy) = policy {
        let _ = write!(&mut line1, "POLICY {}", policy_name(policy.policy));
        let _ = write!(&mut line2, "PERMS {}", PermissionSummary(policy.permissions));
        let _ = write!(&mut line3, "SENSITIVE {}", PermissionSummary(policy.sensitive_permissions));
    } else {
        let _ = write!(&mut line1, "POLICY unavailable");
        let _ = write!(&mut line2, "PERMS -");
        let _ = write!(&mut line3, "SENSITIVE -");
    }

    let mut line4 = FixedLogBuffer::<80>::new();
    if runtime_handle == rt::INVALID_HANDLE {
        let _ = write!(&mut line4, "RUNTIME service not installed");
    } else if let Some(runtime) = pending_runtime {
        let _ = write!(
            &mut line4,
            "RUNTIME env{} {} {}",
            runtime.env_id,
            runtime_env_state_name(runtime.state),
            RuntimeCapSummary(runtime.capabilities),
        );
    } else {
        let _ = write!(&mut line4, "RUNTIME no pending or denied environments");
    }

    let mut line5 = FixedLogBuffer::<96>::new();
    if let Some(audit) = latest_runtime_audit {
        let _ = write!(
            &mut line5,
            "AUDIT runtime#{} {} env{}",
            audit.sequence,
            audit_kind_name(audit.kind),
            audit.env_id,
        );
    } else if let Some(audit) = latest_native_audit {
        let _ = write!(
            &mut line5,
            "AUDIT native#{} {} {}",
            audit.sequence,
            audit_kind_name(audit.kind),
            image_name(audit.subject_image_id),
        );
    } else {
        let _ = write!(&mut line5, "AUDIT no recent security events");
    }

    for (index, line) in [
        "SECURITY REVIEW",
        str::from_utf8(line0.as_bytes()).unwrap_or("APP"),
        str::from_utf8(line1.as_bytes()).unwrap_or("POLICY"),
        str::from_utf8(line2.as_bytes()).unwrap_or("PERMS"),
        str::from_utf8(line3.as_bytes()).unwrap_or("SENSITIVE"),
    ]
    .into_iter()
    .enumerate()
    {
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            12,
            70 + (index as i32 * 12),
            if index == 0 { ui::TEXT_PRIMARY } else { ui::TEXT_SECONDARY },
            line,
        );
    }

    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, 12, 136, ui::TEXT_MUTED, "APP POLICY");
    draw_security_buttons(bytes, policy.is_some(), pending_runtime.is_some());
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        12,
        170,
        ui::TEXT_MUTED,
        str::from_utf8(line4.as_bytes()).unwrap_or("RUNTIME"),
    );
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, 12, 200, ui::TEXT_MUTED, "RUNTIME ACTIONS");
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        12,
        226,
        ui::TEXT_MUTED,
        str::from_utf8(line5.as_bytes()).unwrap_or("AUDIT"),
    );
    Ok(())
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
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, minimize_x + 3, ui::WINDOW_BUTTON_TOP + 2, ui::BG_PANEL, "_");
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, close_x + 3, ui::WINDOW_BUTTON_TOP + 2, ui::BG_PANEL, "X");
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, 10, 9, ui::TEXT_PRIMARY, "SETTINGS");
}

fn draw_tabs(bytes: &mut [u8], page: SettingsPage) {
    draw_button(
        bytes,
        TAB_SYSTEM_X0,
        TAB_Y0,
        TAB_SYSTEM_X1,
        TAB_Y1,
        if page == SettingsPage::System { ui::ACCENT } else { ui::ACCENT_DIM },
        "SYSTEM",
        ui::TEXT_PRIMARY,
    );
    draw_button(
        bytes,
        TAB_SECURITY_X0,
        TAB_Y0,
        TAB_SECURITY_X1,
        TAB_Y1,
        if page == SettingsPage::Security { ui::ACCENT } else { ui::ACCENT_DIM },
        "SECURITY",
        ui::TEXT_PRIMARY,
    );
}

fn draw_security_buttons(bytes: &mut [u8], has_policy: bool, has_runtime: bool) {
    for (x0, x1, label, active) in [
        (SEC_PREV_X0, SEC_PREV_X1, "PREV", has_policy),
        (SEC_NEXT_X0, SEC_NEXT_X1, "NEXT", has_policy),
        (SEC_ALLOW_X0, SEC_ALLOW_X1, "ALLOW", has_policy),
        (SEC_BLOCK_X0, SEC_BLOCK_X1, "BLOCK", has_policy),
        (SEC_DEFAULT_X0, SEC_DEFAULT_X1, "DEFAULT", has_policy),
    ] {
        draw_button(
            bytes,
            x0,
            SEC_ACTION_Y0,
            x1,
            SEC_ACTION_Y1,
            if active { ui::ACCENT_DIM } else { ui::BG_PANEL },
            label,
            ui::TEXT_PRIMARY,
        );
    }
    for (x0, x1, label, active) in [
        (SEC_APPROVE_X0, SEC_APPROVE_X1, "APPROVE", has_runtime),
        (SEC_DENY_X0, SEC_DENY_X1, "DENY", has_runtime),
        (SEC_RESET_X0, SEC_RESET_X1, "RESET", has_runtime),
    ] {
        draw_button(
            bytes,
            x0,
            SEC_RUNTIME_Y0,
            x1,
            SEC_RUNTIME_Y1,
            if active { ui::ACCENT_DIM } else { ui::BG_PANEL },
            label,
            ui::TEXT_PRIMARY,
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
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, x0 + 8, y0 + 6, text_color, label);
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

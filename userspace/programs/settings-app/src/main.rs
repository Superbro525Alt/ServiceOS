#![no_std]
#![no_main]

use core::{array, char, fmt::Write, str};

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{
    AppControlTag, AppKeyAction, AppPointerAction, ConfigKey, ControlTag, FixedLogBuffer,
    LifecycleEvent, PermissionPolicyState, RawMessage,
};

const NOTE_MAX_BYTES: usize = 24;
const BUFFER_WIDTH: u32 = 1024;
const BUFFER_HEIGHT: u32 = 768;
const BUFFER_BYTES: usize = BUFFER_WIDTH as usize * BUFFER_HEIGHT as usize * 4;
const SURFACE_BUFFER_SLOTS: usize = 2;
const PIXEL_STRIDE: usize = BUFFER_WIDTH as usize;
const TAB_SYSTEM_X0: i32 = 10;
const TAB_SYSTEM_X1: i32 = 98;
const TAB_SECURITY_X0: i32 = 106;
const TAB_SECURITY_X1: i32 = 214;
const TAB_Y0: i32 = 36;
const TAB_Y1: i32 = 56;
const NOTE_FIELD_X0: i32 = 10;
const NOTE_FIELD_Y0: i32 = 114;
const NOTE_FIELD_X1: i32 = 232;
const NOTE_FIELD_Y1: i32 = 138;
const AUDIO_TEST_X0: i32 = 10;
const AUDIO_TEST_Y0: i32 = 144;
const AUDIO_TEST_X1: i32 = 118;
const AUDIO_TEST_Y1: i32 = 164;
const SEC_PREV_X0: i32 = 10;
const SEC_PREV_X1: i32 = 58;
const SEC_NEXT_X0: i32 = 66;
const SEC_NEXT_X1: i32 = 114;
const SEC_ACTION_Y0: i32 = 146;
const SEC_ACTION_Y1: i32 = 166;
const SEC_ALLOW_X0: i32 = 122;
const SEC_ALLOW_X1: i32 = 174;
const SEC_BLOCK_X0: i32 = 182;
const SEC_BLOCK_X1: i32 = 234;
const SEC_DEFAULT_X0: i32 = 242;
const SEC_DEFAULT_X1: i32 = 308;
const SEC_RUNTIME_Y0: i32 = 176;
const SEC_RUNTIME_Y1: i32 = 196;
const SEC_APPROVE_X0: i32 = 122;
const SEC_APPROVE_X1: i32 = 190;
const SEC_DENY_X0: i32 = 198;
const SEC_DENY_X1: i32 = 246;
const SEC_RESET_X0: i32 = 254;
const SEC_RESET_X1: i32 = 308;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsPage {
    System,
    Security,
}

#[derive(Clone, Copy)]
struct PendingRuntime {
    env_id: u32,
    state: rt::RuntimeEnvState,
    capabilities: u32,
}

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf001;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 6 || startup.word_count < 4 {
        return 0xf002;
    }

    let surface_handle = startup.handles[0];
    let control_handle = startup.handles[1];
    let config_handle = startup.handles[2];
    let network_handle = startup.handles[3];
    let audio_handle = startup.handles[4];
    let security_handle = startup.handles[5];
    let runtime_handle = if startup.handle_count >= 7 {
        startup.handles[6]
    } else {
        rt::INVALID_HANDLE
    };
    let mut width = startup.words[1] as u32;
    let mut height = startup.words[2] as u32;
    let mut focused = startup.words[3] != 0;
    let mut page = SettingsPage::System;
    let mut editing_note = false;
    let mut selected_policy_index = 0usize;
    let mut note = [0u8; NOTE_MAX_BYTES];
    let mut note_len = 0usize;
    let audio_stream_handle =
        rt::audio_stream_open(audio_handle, rt::AudioStreamDirection::Playback, 0)
            .unwrap_or(rt::INVALID_HANDLE);

    let mut buffer_handles = [rt::INVALID_HANDLE; SURFACE_BUFFER_SLOTS];
    let mut mapped_buffers: [Option<rt::MappedMemory>; SURFACE_BUFFER_SLOTS] =
        array::from_fn(|_| None);
    for slot in 0..SURFACE_BUFFER_SLOTS {
        let buffer_handle = match rt::memory_create(BUFFER_BYTES, true) {
            Ok(handle) => handle,
            Err(_) => return 0xf007,
        };
        if rt::surface_attach_buffer_slot(
            surface_handle,
            slot as u32,
            buffer_handle,
            BUFFER_WIDTH,
            BUFFER_HEIGHT,
            BUFFER_WIDTH,
        )
        .is_err()
        {
            let _ = rt::handle_close(buffer_handle);
            return 0xf008;
        }
        let mapped_buffer = match rt::MappedMemory::map(buffer_handle, BUFFER_BYTES, true) {
            Ok(buffer) => buffer,
            Err(_) => {
                let _ = rt::handle_close(buffer_handle);
                return 0xf009;
            }
        };
        buffer_handles[slot] = buffer_handle;
        mapped_buffers[slot] = Some(mapped_buffer);
    }
    let mut front_buffer_slot = 0usize;

    if render(
        surface_handle,
        front_buffer_slot as u32,
        mapped_buffers[front_buffer_slot].as_mut().unwrap(),
        width,
        height,
        focused,
        config_handle,
        network_handle,
        audio_handle,
        runtime_handle,
        security_handle,
        page,
        selected_policy_index,
        editing_note,
        &note[..note_len],
    )
    .is_err()
    {
        return 0xf003;
    }

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => {
                cleanup_audio(audio_stream_handle, audio_handle);
                break;
            }
            Ok(false) => {}
            Err(_) => return 0xf004,
        }
        match poll_control(
            control_handle,
            surface_handle,
            &mut mapped_buffers,
            &mut front_buffer_slot,
            &mut width,
            &mut height,
            &mut focused,
            &mut page,
            &mut editing_note,
            &mut selected_policy_index,
            &mut note,
            &mut note_len,
            config_handle,
            network_handle,
            audio_handle,
            runtime_handle,
            security_handle,
            audio_stream_handle,
        ) {
            Ok(ControlFlow::Continue) => {}
            Ok(ControlFlow::Exit) => {
                cleanup_audio(audio_stream_handle, audio_handle);
                break;
            }
            Err(_) => return 0xf006,
        }
        if rt::yield_current().is_err() {
            cleanup_audio(audio_stream_handle, audio_handle);
            return 0xf005;
        }
    }

    for handle in buffer_handles {
        if handle != rt::INVALID_HANDLE {
            let _ = rt::handle_close(handle);
        }
    }
    0
}

fn render(
    surface_handle: rt::Handle,
    buffer_slot: u32,
    buffer: &mut rt::MappedMemory,
    width: u32,
    height: u32,
    focused: bool,
    config_handle: rt::Handle,
    network_handle: rt::Handle,
    audio_handle: rt::Handle,
    runtime_handle: rt::Handle,
    security_handle: rt::Handle,
    page: SettingsPage,
    selected_policy_index: usize,
    editing_note: bool,
    note: &[u8],
) -> rt::Result<()> {
    let width = width.min(BUFFER_WIDTH) as usize;
    let height = height.min(BUFFER_HEIGHT) as usize;
    let bytes = &mut buffer.as_slice_mut()[..BUFFER_BYTES];

    fill_rect(bytes, 0, 0, width, height, ui::BG_WINDOW_ALT);
    fill_rect(
        bytes,
        0,
        0,
        width,
        ui::TITLEBAR_HEIGHT as usize,
        if focused { ui::ACCENT } else { ui::ACCENT_DIM },
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
    draw_tabs(bytes, page);

    match page {
        SettingsPage::System => draw_system_page(
            bytes,
            config_handle,
            network_handle,
            audio_handle,
            editing_note,
            note,
        ),
        SettingsPage::Security => draw_security_page(
            bytes,
            runtime_handle,
            security_handle,
            selected_policy_index,
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
            if index == 0 {
                ui::TEXT_PRIMARY
            } else {
                ui::TEXT_SECONDARY
            },
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
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        NOTE_FIELD_X0 + 8,
        NOTE_FIELD_Y0 + 7,
        ui::BG_PANEL,
        note_text,
    );

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
        let _ = write!(
            &mut line3,
            "SENSITIVE {}",
            PermissionSummary(policy.sensitive_permissions)
        );
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
            if index == 0 {
                ui::TEXT_PRIMARY
            } else {
                ui::TEXT_SECONDARY
            },
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

enum ControlFlow {
    Continue,
    Exit,
}

fn poll_control(
    control_handle: rt::Handle,
    surface_handle: rt::Handle,
    buffers: &mut [Option<rt::MappedMemory>; SURFACE_BUFFER_SLOTS],
    front_buffer_slot: &mut usize,
    width: &mut u32,
    height: &mut u32,
    focused: &mut bool,
    page: &mut SettingsPage,
    editing_note: &mut bool,
    selected_policy_index: &mut usize,
    note: &mut [u8; NOTE_MAX_BYTES],
    note_len: &mut usize,
    config_handle: rt::Handle,
    network_handle: rt::Handle,
    audio_handle: rt::Handle,
    runtime_handle: rt::Handle,
    security_handle: rt::Handle,
    audio_stream_handle: rt::Handle,
) -> rt::Result<ControlFlow> {
    let mut changed = false;
    loop {
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(control_handle, &mut message) {
            Ok(()) if message.tag == AppControlTag::FocusChanged as u32 && message.word_count > 0 => {
                *focused = message.words[0] != 0;
                changed = true;
            }
            Ok(()) if message.tag == AppControlTag::Resize as u32 && message.word_count >= 2 => {
                *width = message.words[0] as u32;
                *height = message.words[1] as u32;
                changed = true;
            }
            Ok(()) if message.tag == AppControlTag::Pointer as u32 && message.word_count >= 4 => {
                let action = app_pointer_action_from_word(message.words[0]);
                let x = message.words[1] as i64 as i32;
                let y = message.words[2] as i64 as i32;
                if matches!(action, Some(AppPointerAction::Down)) {
                    if x >= TAB_SYSTEM_X0 && x < TAB_SYSTEM_X1 && y >= TAB_Y0 && y < TAB_Y1 {
                        *page = SettingsPage::System;
                        *editing_note = false;
                        changed = true;
                    } else if x >= TAB_SECURITY_X0
                        && x < TAB_SECURITY_X1
                        && y >= TAB_Y0
                        && y < TAB_Y1
                    {
                        *page = SettingsPage::Security;
                        *editing_note = false;
                        changed = true;
                    } else if *page == SettingsPage::System
                        && x >= NOTE_FIELD_X0
                        && x < NOTE_FIELD_X1
                        && y >= NOTE_FIELD_Y0
                        && y < NOTE_FIELD_Y1
                    {
                        *editing_note = true;
                        changed = true;
                    } else if *page == SettingsPage::System
                        && x >= AUDIO_TEST_X0
                        && x < AUDIO_TEST_X1
                        && y >= AUDIO_TEST_Y0
                        && y < AUDIO_TEST_Y1
                    {
                        if audio_stream_handle != rt::INVALID_HANDLE {
                            let _ = rt::audio_stream_play_tone(audio_stream_handle, 880, 120);
                        }
                        *editing_note = false;
                        changed = true;
                    } else if *page == SettingsPage::Security {
                        *editing_note = false;
                        if x >= SEC_PREV_X0 && x < SEC_PREV_X1 && y >= SEC_ACTION_Y0 && y < SEC_ACTION_Y1 {
                            if *selected_policy_index > 0 {
                                *selected_policy_index -= 1;
                            }
                            changed = true;
                        } else if x >= SEC_NEXT_X0
                            && x < SEC_NEXT_X1
                            && y >= SEC_ACTION_Y0
                            && y < SEC_ACTION_Y1
                        {
                            let count = security_policy_count(security_handle)?;
                            if *selected_policy_index + 1 < count {
                                *selected_policy_index += 1;
                            }
                            changed = true;
                        } else if x >= SEC_ALLOW_X0
                            && x < SEC_ALLOW_X1
                            && y >= SEC_ACTION_Y0
                            && y < SEC_ACTION_Y1
                        {
                            update_policy(security_handle, *selected_policy_index, PermissionPolicyState::Allowed)?;
                            changed = true;
                        } else if x >= SEC_BLOCK_X0
                            && x < SEC_BLOCK_X1
                            && y >= SEC_ACTION_Y0
                            && y < SEC_ACTION_Y1
                        {
                            update_policy(security_handle, *selected_policy_index, PermissionPolicyState::Blocked)?;
                            changed = true;
                        } else if x >= SEC_DEFAULT_X0
                            && x < SEC_DEFAULT_X1
                            && y >= SEC_ACTION_Y0
                            && y < SEC_ACTION_Y1
                        {
                            update_policy(
                                security_handle,
                                *selected_policy_index,
                                PermissionPolicyState::DefaultAllow,
                            )?;
                            changed = true;
                        } else if x >= SEC_APPROVE_X0
                            && x < SEC_APPROVE_X1
                            && y >= SEC_RUNTIME_Y0
                            && y < SEC_RUNTIME_Y1
                        {
            if let Some(runtime) = first_actionable_runtime(runtime_handle)? {
                rt::runtime_env_decide(
                    runtime_handle,
                    runtime.env_id,
                                    PermissionPolicyState::Allowed,
                                )?;
                                changed = true;
                            }
                        } else if x >= SEC_DENY_X0
                            && x < SEC_DENY_X1
                            && y >= SEC_RUNTIME_Y0
                            && y < SEC_RUNTIME_Y1
                        {
            if let Some(runtime) = first_actionable_runtime(runtime_handle)? {
                rt::runtime_env_decide(
                    runtime_handle,
                    runtime.env_id,
                                    PermissionPolicyState::Blocked,
                                )?;
                                changed = true;
                            }
                        } else if x >= SEC_RESET_X0
                            && x < SEC_RESET_X1
                            && y >= SEC_RUNTIME_Y0
                            && y < SEC_RUNTIME_Y1
                        {
            if let Some(runtime) = first_actionable_runtime(runtime_handle)? {
                rt::runtime_env_decide(
                    runtime_handle,
                    runtime.env_id,
                                    PermissionPolicyState::DefaultAllow,
                                )?;
                                changed = true;
                            }
                        } else {
                            changed = true;
                        }
                    } else {
                        *editing_note = false;
                        changed = true;
                    }
                }
            }
            Ok(()) if message.tag == AppControlTag::Key as u32 && message.word_count >= 2 => {
                if matches!(app_key_action_from_word(message.words[0]), Some(AppKeyAction::Down)) {
                    match message.words[1] as u32 {
                        14 if *editing_note && *note_len > 0 => {
                            *note_len -= 1;
                            changed = true;
                        }
                        15 => {
                            *page = match *page {
                                SettingsPage::System => SettingsPage::Security,
                                SettingsPage::Security => SettingsPage::System,
                            };
                            *editing_note = false;
                            changed = true;
                        }
                        103 if *page == SettingsPage::Security => {
                            if *selected_policy_index > 0 {
                                *selected_policy_index -= 1;
                                changed = true;
                            }
                        }
                        108 if *page == SettingsPage::Security => {
                            let count = security_policy_count(security_handle)?;
                            if *selected_policy_index + 1 < count {
                                *selected_policy_index += 1;
                                changed = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ok(()) if message.tag == AppControlTag::Text as u32 && message.word_count > 0 => {
                if *editing_note && let Some(ch) = char::from_u32(message.words[0] as u32) {
                    if ch == '\n' {
                        *editing_note = false;
                        changed = true;
                    } else if ch.is_ascii_graphic() || ch == ' ' {
                        let mut scratch = [0u8; 4];
                        let bytes = ch.encode_utf8(&mut scratch).as_bytes();
                        if *note_len + bytes.len() <= NOTE_MAX_BYTES {
                            note[*note_len..*note_len + bytes.len()].copy_from_slice(bytes);
                            *note_len += bytes.len();
                            changed = true;
                        }
                    }
                }
            }
            Ok(()) if message.tag == AppControlTag::Close as u32 => return Ok(ControlFlow::Exit),
            Ok(()) => {}
            Err(rt::Error::QueueEmpty) => break,
            Err(error) => return Err(error),
        }
    }

    if changed {
        *front_buffer_slot = (*front_buffer_slot + 1) % SURFACE_BUFFER_SLOTS;
        render(
            surface_handle,
            *front_buffer_slot as u32,
            buffers[*front_buffer_slot].as_mut().unwrap(),
            *width,
            *height,
            *focused,
            config_handle,
            network_handle,
            audio_handle,
            runtime_handle,
            security_handle,
            *page,
            *selected_policy_index,
            *editing_note,
            &note[..*note_len],
        )?;
    }

    Ok(ControlFlow::Continue)
}

fn cleanup_audio(audio_stream_handle: rt::Handle, audio_handle: rt::Handle) {
    if audio_stream_handle != rt::INVALID_HANDLE {
        let _ = rt::audio_stream_close(audio_stream_handle);
        let _ = rt::handle_close(audio_stream_handle);
    }
    if audio_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(audio_handle);
    }
}

fn app_pointer_action_from_word(value: u64) -> Option<AppPointerAction> {
    match value as u32 {
        x if x == AppPointerAction::Down as u32 => Some(AppPointerAction::Down),
        x if x == AppPointerAction::Move as u32 => Some(AppPointerAction::Move),
        x if x == AppPointerAction::Up as u32 => Some(AppPointerAction::Up),
        _ => None,
    }
}

fn app_key_action_from_word(value: u64) -> Option<AppKeyAction> {
    match value as u32 {
        x if x == AppKeyAction::Down as u32 => Some(AppKeyAction::Down),
        x if x == AppKeyAction::Up as u32 => Some(AppKeyAction::Up),
        _ => None,
    }
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
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        minimize_x + 3,
        ui::WINDOW_BUTTON_TOP + 2,
        ui::BG_PANEL,
        "_",
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        close_x + 3,
        ui::WINDOW_BUTTON_TOP + 2,
        ui::BG_PANEL,
        "X",
    );
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, 10, 9, ui::TEXT_PRIMARY, "SETTINGS");
}

fn draw_tabs(bytes: &mut [u8], page: SettingsPage) {
    draw_button(
        bytes,
        TAB_SYSTEM_X0,
        TAB_Y0,
        TAB_SYSTEM_X1,
        TAB_Y1,
        if page == SettingsPage::System {
            ui::ACCENT
        } else {
            ui::ACCENT_DIM
        },
        "SYSTEM",
        ui::TEXT_PRIMARY,
    );
    draw_button(
        bytes,
        TAB_SECURITY_X0,
        TAB_Y0,
        TAB_SECURITY_X1,
        TAB_Y1,
        if page == SettingsPage::Security {
            ui::ACCENT
        } else {
            ui::ACCENT_DIM
        },
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

fn security_policy_count(security_handle: rt::Handle) -> rt::Result<usize> {
    let mut index = 0usize;
    while rt::security_policy_list(security_handle, index)?.is_some() {
        index += 1;
    }
    Ok(index)
}

fn update_policy(
    security_handle: rt::Handle,
    selected_policy_index: usize,
    policy: PermissionPolicyState,
) -> rt::Result<()> {
    if let Some(info) = rt::security_policy_list(security_handle, selected_policy_index)? {
        rt::security_policy_set(security_handle, info.image_id, policy)?;
    }
    Ok(())
}

fn first_actionable_runtime(runtime_handle: rt::Handle) -> rt::Result<Option<PendingRuntime>> {
    if runtime_handle == rt::INVALID_HANDLE {
        return Ok(None);
    }
    let mut envs = [rt::RuntimeEnvInfo {
        env_id: 0,
        kind: rt::RuntimeKind::Posix,
        state: rt::RuntimeEnvState::Destroyed,
        capabilities: 0,
        mount_count: 0,
        var_count: 0,
        active_runs: 0,
    }; 8];
    let count = rt::runtime_env_list(runtime_handle, &mut envs)?;
    for env in envs.into_iter().take(count) {
        if matches!(
            env.state,
            rt::RuntimeEnvState::PendingApproval | rt::RuntimeEnvState::Denied
        ) {
            return Ok(Some(PendingRuntime {
                env_id: env.env_id,
                state: env.state,
                capabilities: env.capabilities,
            }));
        }
    }
    Ok(None)
}

fn policy_name(policy: PermissionPolicyState) -> &'static str {
    match policy {
        PermissionPolicyState::DefaultAllow => "default-allow",
        PermissionPolicyState::Allowed => "allowed",
        PermissionPolicyState::Blocked => "blocked",
    }
}

fn runtime_env_state_name(state: rt::RuntimeEnvState) -> &'static str {
    match state {
        rt::RuntimeEnvState::Ready => "ready",
        rt::RuntimeEnvState::Busy => "busy",
        rt::RuntimeEnvState::Destroyed => "destroyed",
        rt::RuntimeEnvState::PendingApproval => "pending-approval",
        rt::RuntimeEnvState::Denied => "denied",
    }
}

fn audit_kind_name(kind: rt::SecurityAuditKind) -> &'static str {
    match kind {
        rt::SecurityAuditKind::PolicyChanged => "policy-changed",
        rt::SecurityAuditKind::LaunchDenied => "launch-denied",
        rt::SecurityAuditKind::RuntimeApprovalRequested => "approval-requested",
        rt::SecurityAuditKind::RuntimeApprovalChanged => "approval-changed",
    }
}

fn image_name(image_id: rt::ServiceImageId) -> &'static str {
    match image_id {
        rt::ServiceImageId::SettingsApp => "settings",
        rt::ServiceImageId::FilesApp => "files",
        rt::ServiceImageId::MonitorApp => "monitor",
        rt::ServiceImageId::TerminalApp => "terminal",
        rt::ServiceImageId::SoftwareCenterApp => "software",
        rt::ServiceImageId::SysinfoTool => "sysinfo",
        rt::ServiceImageId::PosixHostTool => "runtime-host",
        rt::ServiceImageId::CrossBuilderTool => "cross-builder",
        _ => "unknown",
    }
}

struct PermissionSummary(u32);

impl core::fmt::Display for PermissionSummary {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut first = true;
        for (name, mask) in [
            ("config", rt::app_permission::CONFIG),
            ("storage", rt::app_permission::STORAGE),
            ("status", rt::app_permission::STATUS),
            ("package", rt::app_permission::PACKAGE),
            ("network", rt::app_permission::NETWORK),
            ("audio", rt::app_permission::AUDIO),
            ("terminal", rt::app_permission::TERMINAL),
            ("clipboard", rt::app_permission::CLIPBOARD),
        ] {
            if self.0 & mask == 0 {
                continue;
            }
            if !first {
                let _ = f.write_str(",");
            }
            first = false;
            let _ = f.write_str(name);
        }
        if first {
            let _ = f.write_str("-");
        }
        Ok(())
    }
}

struct RuntimeCapSummary(u32);

impl core::fmt::Display for RuntimeCapSummary {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut first = true;
        for (name, mask) in [
            ("file-read", rt::runtime_capability::FILE_READ),
            ("terminal-io", rt::runtime_capability::TERMINAL_IO),
            ("network", rt::runtime_capability::NETWORK),
            ("graphics", rt::runtime_capability::GRAPHICS),
            ("audio", rt::runtime_capability::AUDIO),
        ] {
            if self.0 & mask == 0 {
                continue;
            }
            if !first {
                let _ = f.write_str(",");
            }
            first = false;
            let _ = f.write_str(name);
        }
        if first {
            let _ = f.write_str("-");
        }
        Ok(())
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

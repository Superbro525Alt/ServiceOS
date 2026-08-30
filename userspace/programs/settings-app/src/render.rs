use core::{fmt::Write, str};

use rt::{ConfigKey, FixedLogBuffer};
use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::netdiag;
use crate::security::{
    PermissionSummary, RuntimeCapSummary, audit_kind_name, first_actionable_runtime, image_name,
    policy_name, runtime_env_state_name, security_policy_count,
};
use crate::state::*;
use crate::wifi;

pub(crate) fn render(
    presenter: &mut ui::FirstPresentSurface,
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

    ui::draw_window_frame_rgba8888(
        bytes,
        PIXEL_STRIDE,
        width,
        height,
        state.focused,
        ui::BG_WINDOW_ALT,
        "SETTINGS",
    );
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
        SettingsPage::Network => draw_network_page(bytes, network_handle, state),
        SettingsPage::Wifi => draw_wifi_page(bytes, network_handle, state),
    }

    presenter.present(buffer_slot, width as u32, height as u32)
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
    let mut pcm_streams = [settings_pcm_stream_info(); SETTINGS_PCM_STREAMS_MAX];
    let pcm_listed = rt::audio_stream_list(audio_handle, &mut pcm_streams).unwrap_or(0);
    let pcm_active = pcm_streams[..pcm_listed]
        .iter()
        .filter(|stream| stream.state == rt::AudioStreamState::Active)
        .count();
    let _ = write!(&mut audio_line, " PCM {}/{}", pcm_active, pcm_listed);

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

    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        NOTE_FIELD_X0,
        106,
        ui::TEXT_MUTED,
        "NOTE",
    );
    ui::fill_rgba8888_rect(
        bytes,
        PIXEL_STRIDE,
        BUFFER_WIDTH as usize,
        BUFFER_HEIGHT as usize,
        NOTE_FIELD_X0.max(0) as usize,
        NOTE_FIELD_Y0.max(0) as usize,
        (NOTE_FIELD_X1 - NOTE_FIELD_X0).max(0) as usize,
        (NOTE_FIELD_Y1 - NOTE_FIELD_Y0).max(0) as usize,
        if editing_note {
            ui::ACCENT
        } else {
            ui::ACCENT_DIM
        },
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
        let _ = write!(
            &mut line0,
            "APP {} ({}/{})",
            name,
            policy_index + 1,
            policy_count
        );
    } else {
        let _ = write!(&mut line0, "APP no registered policies");
    }

    let mut line1 = FixedLogBuffer::<64>::new();
    let mut line2 = FixedLogBuffer::<64>::new();
    let mut line3 = FixedLogBuffer::<64>::new();
    if let Some(policy) = policy {
        let _ = write!(&mut line1, "POLICY {}", policy_name(policy.policy));
        let _ = write!(
            &mut line2,
            "PERMS {}",
            PermissionSummary(policy.permissions)
        );
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
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        12,
        200,
        ui::TEXT_MUTED,
        "RUNTIME ACTIONS",
    );
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

#[allow(clippy::too_many_lines)]
fn draw_network_page(bytes: &mut [u8], network_handle: rt::Handle, state: &AppState) {
    let interface = rt::network_interface_status(network_handle, 0).unwrap_or(None);

    let mut eth0 = FixedLogBuffer::<48>::new();
    match interface {
        Some(info) => {
            let _ = write!(
                &mut eth0,
                "ETH0 {} {}/{}",
                netdiag::link_state_name(info.link_state),
                netdiag::format_ipv4::<16>(info.address).as_str(),
                info.prefix_len,
            );
        }
        None => {
            let _ = write!(&mut eth0, "ETH0 UNAVAILABLE");
        }
    }

    let mut gw_dns = FixedLogBuffer::<64>::new();
    match interface {
        Some(info) if info.gateway != 0 => {
            let _ = write!(
                &mut gw_dns,
                "GW {} DNS {}",
                netdiag::format_ipv4::<16>(info.gateway).as_str(),
                netdiag::format_ipv4::<16>(info.dns_server).as_str(),
            );
        }
        _ => {
            let _ = write!(&mut gw_dns, "GW UNAVAILABLE");
        }
    }

    let mut hostname = [0u8; crate::state::HOSTNAME_EDIT_MAX_BYTES];
    let hostname_len = rt::network_hostname_get(network_handle, &mut hostname)
        .unwrap_or(0)
        .min(hostname.len());
    let hostname_text = if hostname_len > 0 {
        core::str::from_utf8(&hostname[..hostname_len]).unwrap_or("HOSTNAME UNAVAILABLE")
    } else {
        "HOSTNAME UNAVAILABLE"
    };

    let firewall = rt::network_firewall_summary(network_handle).ok();
    let mut firewall_line = FixedLogBuffer::<48>::new();
    let mut firewall_deny = FixedLogBuffer::<48>::new();
    if let Some(summary) = firewall {
        let _ = write!(
            &mut firewall_line,
            "FW RULES {} IN {}",
            summary.rule_count,
            if summary.default_inbound_allow {
                "ALLOW"
            } else {
                "DENY"
            },
        );
        let _ = write!(
            &mut firewall_deny,
            "FW DENY IN {} OUT {}",
            summary.inbound_denied_total, summary.outbound_denied_total,
        );
    } else {
        let _ = write!(&mut firewall_line, "FW UNAVAILABLE");
        let _ = write!(&mut firewall_deny, "FW DENY -");
    }

    let mut resolver_line = FixedLogBuffer::<48>::new();
    match interface {
        Some(info) => {
            let _ = write!(
                &mut resolver_line,
                "RESOLVER HIT {} MISS {}",
                info.resolver_hits, info.resolver_misses,
            );
        }
        _ => {
            let _ = write!(&mut resolver_line, "RESOLVER UNAVAILABLE");
        }
    }

    let mut ping_headline = FixedLogBuffer::<56>::new();
    let mut ping_detail = FixedLogBuffer::<56>::new();
    let mut ping_loss = FixedLogBuffer::<32>::new();
    match (state.ping_failed, state.ping_stats) {
        (false, Some(stats)) => {
            let _ = write!(
                &mut ping_headline,
                "PING {} {}/{} RX",
                str::from_utf8(&state.ping_target[..state.ping_target_len]).unwrap_or("?"),
                stats.received,
                stats.sent,
            );
            let _ = write!(
                &mut ping_detail,
                "MIN {}MS MAX {}MS AVG {}MS",
                stats.min_ms, stats.max_ms, stats.avg_ms,
            );
            let _ = write!(
                &mut ping_loss,
                "JIT {}MS LOSS {}",
                stats.jitter_ms,
                netdiag::format_loss::<16>(stats.loss_permil).as_str(),
            );
        }
        (true, _) => {
            let _ = write!(&mut ping_headline, "PING FAILED");
            let _ = write!(&mut ping_detail, "MIN - MAX - AVG -");
            let _ = write!(&mut ping_loss, "JIT - LOSS -");
        }
        (false, None) => {
            let _ = write!(&mut ping_headline, "PING PRESS RUN");
            let _ = write!(&mut ping_detail, "MIN - MAX - AVG -");
            let _ = write!(&mut ping_loss, "JIT - LOSS -");
        }
    }

    let mut neighbors = [rt::NetworkNeighborEntry {
        address: 0,
        mac: [0; 6],
    }; 3];
    let neighbor_count = rt::network_neighbor_list(network_handle, &mut neighbors).unwrap_or(0);
    let mut neighbor_line = FixedLogBuffer::<48>::new();
    if neighbor_count == 0 {
        let _ = write!(&mut neighbor_line, "NEIGHBORS UNAVAILABLE");
    } else {
        let _ = write!(&mut neighbor_line, "NEIGHBORS {}", neighbor_count);
    }

    let mut ports = [rt::NetworkListenPort {
        kind: rt::NetworkListenPortKind::Unknown,
        port: 0,
    }; 8];
    let port_count = rt::network_listen_ports(network_handle, &mut ports).unwrap_or(0);
    let mut ports_line = FixedLogBuffer::<64>::new();
    if port_count == 0 {
        let _ = write!(&mut ports_line, "PORTS UNAVAILABLE");
    } else {
        let _ = write!(&mut ports_line, "PORTS {}:", port_count);
        for port in ports.iter().take(port_count) {
            let _ = write!(
                &mut ports_line,
                " {} {}",
                netdiag::listen_port_kind_name(port.kind),
                port.port,
            );
        }
    }

    let mut peers = [rt::NetworkDiscoveryPeer {
        address: 0,
        name_len: 0,
        name: [0; 15],
        age_ms: 0,
    }; 2];
    let peer_count = rt::network_discovery_peers(network_handle, 0, &mut peers).unwrap_or(0);
    let mut peers_line = FixedLogBuffer::<64>::new();
    if peer_count == 0 {
        let _ = write!(&mut peers_line, "PEERS UNAVAILABLE");
    } else {
        let _ = write!(&mut peers_line, "PEERS {}:", peer_count);
        for peer in peers.iter().take(peer_count) {
            let _ = write!(
                &mut peers_line,
                " {} {}",
                str::from_utf8(&peer.name[..peer.name_len]).unwrap_or("?"),
                netdiag::format_ipv4::<16>(peer.address).as_str(),
            );
        }
    }

    for (index, line) in [
        "NETWORK STATUS",
        str::from_utf8(eth0.as_bytes()).unwrap_or("ETH0"),
        str::from_utf8(gw_dns.as_bytes()).unwrap_or("GW"),
        str::from_utf8(firewall_line.as_bytes()).unwrap_or("FW"),
        str::from_utf8(firewall_deny.as_bytes()).unwrap_or("FW DENY"),
        str::from_utf8(resolver_line.as_bytes()).unwrap_or("RESOLVER"),
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

    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, 12, 144, ui::TEXT_MUTED, "PING");
    draw_button(
        bytes,
        NET_PING_RUN_X0,
        NET_PING_RUN_Y0,
        NET_PING_RUN_X1,
        NET_PING_RUN_Y1,
        ui::ACCENT_DIM,
        "RUN PING",
        ui::TEXT_PRIMARY,
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        12,
        182,
        ui::TEXT_SECONDARY,
        str::from_utf8(ping_headline.as_bytes()).unwrap_or("PING"),
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        12,
        194,
        ui::TEXT_SECONDARY,
        str::from_utf8(ping_detail.as_bytes()).unwrap_or("MIN -"),
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        12,
        206,
        ui::TEXT_SECONDARY,
        str::from_utf8(ping_loss.as_bytes()).unwrap_or("JIT -"),
    );

    // Neighbor rows under the ping block.
    for (index, neighbor) in neighbors.iter().take(neighbor_count).enumerate() {
        let mut row = FixedLogBuffer::<48>::new();
        let _ = write!(
            &mut row,
            "{} {}",
            netdiag::format_ipv4::<16>(neighbor.address).as_str(),
            netdiag::format_mac::<18>(neighbor.mac).as_str(),
        );
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            12,
            230 + (index as i32 * 12),
            ui::TEXT_MUTED,
            str::from_utf8(row.as_bytes()).unwrap_or("NEIGHBOR"),
        );
    }

    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        12,
        266,
        ui::TEXT_MUTED,
        str::from_utf8(ports_line.as_bytes()).unwrap_or("PORTS"),
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        12,
        278,
        ui::TEXT_MUTED,
        str::from_utf8(peers_line.as_bytes()).unwrap_or("PEERS"),
    );

    // Hostname read/edit row last so the field draws over the row band.
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, 12, 104, ui::TEXT_MUTED, "HOSTNAME");
    let edit_text = if state.editing_hostname {
        str::from_utf8(&state.hostname_edit[..state.hostname_edit_len]).unwrap_or("")
    } else {
        hostname_text
    };
    ui::fill_rgba8888_rect(
        bytes,
        PIXEL_STRIDE,
        BUFFER_WIDTH as usize,
        BUFFER_HEIGHT as usize,
        NET_HOSTNAME_FIELD_X0.max(0) as usize,
        NET_HOSTNAME_FIELD_Y0.max(0) as usize,
        (NET_HOSTNAME_FIELD_X1 - NET_HOSTNAME_FIELD_X0).max(0) as usize,
        (NET_HOSTNAME_FIELD_Y1 - NET_HOSTNAME_FIELD_Y0).max(0) as usize,
        if state.editing_hostname {
            ui::ACCENT
        } else {
            ui::ACCENT_DIM
        },
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        NET_HOSTNAME_FIELD_X0 + 8,
        NET_HOSTNAME_FIELD_Y0 + 7,
        ui::BG_PANEL,
        edit_text,
    );
}

/// Wi-Fi page: status + scan list + join + saved networks + hints. The
/// wireless family answers Unsupported with no backend device, so every
/// block degrades to an honest unavailable line — empty is the correct
/// render today, never fabricated scan rows.
fn draw_wifi_page(bytes: &mut [u8], network_handle: rt::Handle, state: &AppState) {
    let (wlan_line, device_line, link_state, status_error) =
        match rt::network_wifi_status(network_handle) {
            Ok(status) => {
                let mut wlan = FixedLogBuffer::<64>::new();
                let _ = write!(
                    &mut wlan,
                    "WLAN {} SSID {}",
                    wifi::link_state_name(status.link_state),
                    if status.ssid_len > 0 {
                        wifi::ssid_str(&status.ssid, status.ssid_len)
                    } else {
                        "-"
                    },
                );
                (wlan, "DEVICE PRESENT", Some(status.link_state), None)
            }
            Err(error) => {
                let classified = wifi::classify(error);
                let device = if classified == WifiOpError::Unsupported {
                    "DEVICE: NO WIRELESS DEVICE"
                } else {
                    "DEVICE UNAVAILABLE"
                };
                (FixedLogBuffer::<64>::new(), device, None, Some(classified))
            }
        };

    for (index, line) in [
        "WI-FI",
        str::from_utf8(wlan_line.as_bytes()).unwrap_or("WLAN"),
        device_line,
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

    draw_button(
        bytes,
        WIFI_SCAN_BTN_X0,
        WIFI_BTN_Y0,
        WIFI_SCAN_BTN_X1,
        WIFI_BTN_Y1,
        ui::ACCENT_DIM,
        "SCAN",
        ui::TEXT_PRIMARY,
    );
    draw_button(
        bytes,
        WIFI_JOIN_BTN_X0,
        WIFI_BTN_Y0,
        WIFI_JOIN_BTN_X1,
        WIFI_BTN_Y1,
        ui::ACCENT_DIM,
        "JOIN SEL",
        ui::TEXT_PRIMARY,
    );

    // Scan header + rows.
    let scan_header: FixedLogBuffer<48> = if let Some(error) = state.wifi.scan_error {
        let mut text = FixedLogBuffer::<48>::new();
        let _ = write!(&mut text, "SCAN FAILED {}", wifi::error_name(error));
        text
    } else if state.wifi.scan_count > 0 {
        let mut text = FixedLogBuffer::<48>::new();
        let _ = write!(
            &mut text,
            "SCAN {} FOUND (SHOWING {})",
            state.wifi.scan_total, state.wifi.scan_count,
        );
        text
    } else {
        let mut text = FixedLogBuffer::<48>::new();
        let _ = write!(&mut text, "SCAN PRESS SCAN");
        text
    };
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        12,
        132,
        ui::TEXT_MUTED,
        str::from_utf8(scan_header.as_bytes()).unwrap_or("SCAN"),
    );
    for (index, entry) in state.wifi.scans.iter().take(state.wifi.scan_count).enumerate() {
        let row = wifi::scan_row_text::<64>(entry);
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            12,
            WIFI_SCAN_ROW_Y0 + 2 + (index as i32 * WIFI_ROW_H),
            if index == state.wifi.selected_scan {
                ui::TEXT_PRIMARY
            } else {
                ui::TEXT_MUTED
            },
            str::from_utf8(row.as_bytes()).unwrap_or("SCAN ROW"),
        );
    }

    // Saved networks.
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, 12, 170, ui::TEXT_MUTED, "SAVED");
    if state.wifi.saved_count == 0 {
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            12,
            WIFI_SAVED_ROW_Y0 + 2,
            ui::TEXT_MUTED,
            "SAVED UNAVAILABLE",
        );
    }
    for (index, record) in state.wifi.saved.iter().take(state.wifi.saved_count).enumerate() {
        let row = wifi::saved_row_text::<48>(record);
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            12,
            WIFI_SAVED_ROW_Y0 + 2 + (index as i32 * WIFI_ROW_H),
            if index == state.wifi.selected_saved {
                ui::TEXT_PRIMARY
            } else {
                ui::TEXT_MUTED
            },
            str::from_utf8(row.as_bytes()).unwrap_or("SAVED ROW"),
        );
    }

    draw_button(
        bytes,
        WIFI_ADD_BTN_X0,
        WIFI_ACTION_Y0,
        WIFI_ADD_BTN_X1,
        WIFI_ACTION_Y1,
        ui::ACCENT_DIM,
        "SAVED +",
        ui::TEXT_PRIMARY,
    );
    draw_button(
        bytes,
        WIFI_REMOVE_BTN_X0,
        WIFI_ACTION_Y0,
        WIFI_REMOVE_BTN_X1,
        WIFI_ACTION_Y1,
        ui::ACCENT_DIM,
        "SAVED -",
        ui::TEXT_PRIMARY,
    );

    if let Some(outcome) = &state.wifi.join_outcome {
        let text = wifi::join_outcome_text::<48>(outcome);
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            12,
            226,
            ui::TEXT_SECONDARY,
            str::from_utf8(text.as_bytes()).unwrap_or("JOIN"),
        );
    } else if let Some(outcome) = wifi::saved_outcome_text::<48>(
        state.wifi.saved_add_outcome,
        state.wifi.saved_remove_outcome,
    ) {
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            12,
            226,
            ui::TEXT_SECONDARY,
            str::from_utf8(outcome.as_bytes()).unwrap_or("SAVED"),
        );
    }

    // State-based troubleshooting hints.
    let hints = wifi::hint_lines(
        link_state,
        status_error,
        state.wifi.join_outcome.and_then(|outcome| outcome.err()),
        state.wifi.scan_error,
    );
    for (index, hint) in hints.iter().enumerate() {
        if hint.is_empty() {
            continue;
        }
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            12,
            240 + (index as i32 * 12),
            ui::TEXT_MUTED,
            hint,
        );
    }

    // Modal prompt overlay drawn last so it covers rows underneath.
    if let Some(prompt) = state.wifi.prompt {
        ui::fill_rgba8888_rect(
            bytes,
            PIXEL_STRIDE,
            BUFFER_WIDTH as usize,
            BUFFER_HEIGHT as usize,
            WIFI_ROW_X0.max(0) as usize,
            WIFI_SCAN_ROW_Y0.saturating_sub(10).max(0) as usize,
            (WIFI_ROW_X1 - WIFI_ROW_X0).max(0) as usize,
            (WIFI_ACTION_Y0 - (WIFI_SCAN_ROW_Y0 - 10)).max(0) as usize,
            ui::BG_PANEL,
        );
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            12,
            WIFI_SCAN_ROW_Y0.saturating_sub(4),
            ui::TEXT_PRIMARY,
            wifi::prompt_title(prompt),
        );
        ui::fill_rgba8888_rect(
            bytes,
            PIXEL_STRIDE,
            BUFFER_WIDTH as usize,
            BUFFER_HEIGHT as usize,
            12,
            (WIFI_SCAN_ROW_Y0 + 10).max(0) as usize,
            268,
            20,
            ui::ACCENT,
        );
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            20,
            WIFI_SCAN_ROW_Y0 + 16,
            ui::BG_PANEL,
            str::from_utf8(&state.wifi.prompt_edit[..state.wifi.prompt_len]).unwrap_or(""),
        );
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            12,
            WIFI_SCAN_ROW_Y0 + 36,
            ui::TEXT_MUTED,
            "ENTER COMMIT  ESC CANCEL  BKSP DELETE",
        );
    }
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
    draw_button(
        bytes,
        TAB_NETWORK_X0,
        TAB_Y0,
        TAB_NETWORK_X1,
        TAB_Y1,
        if page == SettingsPage::Network {
            ui::ACCENT
        } else {
            ui::ACCENT_DIM
        },
        "NETWORK",
        ui::TEXT_PRIMARY,
    );
    draw_button(
        bytes,
        TAB_WIFI_X0,
        TAB_Y0,
        TAB_WIFI_X1,
        TAB_Y1,
        if page == SettingsPage::Wifi {
            ui::ACCENT
        } else {
            ui::ACCENT_DIM
        },
        "WIFI",
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
    ui::fill_rgba8888_rect(
        bytes,
        PIXEL_STRIDE,
        BUFFER_WIDTH as usize,
        BUFFER_HEIGHT as usize,
        x0.max(0) as usize,
        y0.max(0) as usize,
        (x1 - x0).max(0) as usize,
        (y1 - y0).max(0) as usize,
        color,
    );
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, x0 + 8, y0 + 6, text_color, label);
}

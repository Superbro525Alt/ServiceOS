use core::fmt::Write as _;

use rt::FixedLogBuffer;
use serviceos_userspace_runtime as rt;

use crate::state::{WifiOpError, WifiPrompt, WifiUiState, WIFI_SSID_MAX_BYTES};

/// Classify a failed wrapper call for honest rendering. Unsupported is the
/// documented reply while no wireless backend device is registered.
pub(crate) fn classify(error: rt::Error) -> WifiOpError {
    match error {
        rt::Error::Unsupported => WifiOpError::Unsupported,
        rt::Error::InvalidArgument | rt::Error::BufferTooSmall => WifiOpError::Invalid,
        rt::Error::NotFound | rt::Error::Busy | rt::Error::CapacityExceeded => WifiOpError::Rejected,
        _ => WifiOpError::Transport,
    }
}

pub(crate) fn link_state_name(state: rt::WifiLinkState) -> &'static str {
    match state {
        rt::WifiLinkState::Down => "DOWN",
        rt::WifiLinkState::Scanning => "SCAN",
        rt::WifiLinkState::Authenticating => "AUTH",
        rt::WifiLinkState::Associating => "ASSOC",
        rt::WifiLinkState::Connected => "UP",
    }
}

/// Text security marker for a scan row. Open nets show OPN; unknown
/// protection still shows honestly instead of pretending open.
pub(crate) fn security_tag(security: rt::WifiSecurity) -> &'static str {
    match security {
        rt::WifiSecurity::Open => "OPN",
        rt::WifiSecurity::Wpa2 => "WPA2",
        rt::WifiSecurity::Wpa3 => "WPA3",
        rt::WifiSecurity::Unknown => "SEC?",
    }
}

/// Only open networks join without a psk sub-prompt.
pub(crate) fn join_needs_psk(security: rt::WifiSecurity) -> bool {
    !matches!(security, rt::WifiSecurity::Open)
}

pub(crate) fn prompt_title(prompt: WifiPrompt) -> &'static str {
    match prompt {
        WifiPrompt::JoinPsk => "JOIN PSK:",
        WifiPrompt::SavedSsid => "SAVED SSID:",
        WifiPrompt::SavedPsk => "SAVED PSK:",
        WifiPrompt::SavedPriority => "SAVED PRIORITY:",
    }
}

pub(crate) fn error_name(error: WifiOpError) -> &'static str {
    match error {
        WifiOpError::Unsupported => "UNSUPPORTED",
        WifiOpError::Invalid => "INVALID",
        WifiOpError::Rejected => "REFUSED",
        WifiOpError::Transport => "UNAVAILABLE",
    }
}

pub(crate) fn ssid_str<'a>(ssid: &'a [u8; WIFI_SSID_MAX_BYTES], len: usize) -> &'a str {
    core::str::from_utf8(&ssid[..len.min(WIFI_SSID_MAX_BYTES)]).unwrap_or("?")
}

pub(crate) fn scan_row_text<const N: usize>(entry: &rt::NetworkWifiScanEntry) -> FixedLogBuffer<N> {
    let mut row = FixedLogBuffer::<N>::new();
    let _ = write!(
        &mut row,
        "{:>2} {:>4}DBM {:<4} {}",
        entry.channel,
        entry.rssi,
        security_tag(entry.security),
        ssid_str(&entry.ssid, entry.ssid_len),
    );
    row
}

pub(crate) fn saved_row_text<const N: usize>(
    record: &rt::NetworkWifiSavedNetwork,
) -> FixedLogBuffer<N> {
    let mut row = FixedLogBuffer::<N>::new();
    let _ = write!(
        &mut row,
        "{} P{}",
        ssid_str(&record.ssid, record.ssid_len),
        record.priority,
    );
    row
}

pub(crate) fn join_outcome_text<const N: usize>(
    outcome: &Result<rt::WifiLinkState, WifiOpError>,
) -> FixedLogBuffer<N> {
    let mut text = FixedLogBuffer::<N>::new();
    match outcome {
        Ok(state) => {
            let _ = write!(&mut text, "JOIN OK {}", link_state_name(*state));
        }
        Err(error) => {
            let _ = write!(&mut text, "JOIN FAILED {}", error_name(*error));
        }
    }
    text
}

pub(crate) fn saved_outcome_text<const N: usize>(
    add: Option<Result<(), WifiOpError>>,
    remove: Option<Result<(), WifiOpError>>,
) -> Option<FixedLogBuffer<N>> {
    let mut text = FixedLogBuffer::<N>::new();
    match add {
        Some(Ok(())) => {
            let _ = write!(&mut text, "SAVED ADD OK");
            Some(text)
        }
        Some(Err(error)) => {
            let _ = write!(&mut text, "SAVED ADD FAILED {}", error_name(error));
            Some(text)
        }
        None => match remove {
            Some(Ok(())) => {
                let _ = write!(&mut text, "SAVED REMOVE OK");
                Some(text)
            }
            Some(Err(error)) => {
                let _ = write!(&mut text, "SAVED REMOVE FAILED {}", error_name(error));
                Some(text)
            }
            None => None,
        },
    }
}

/// Two-to-three honest troubleshooting hints derived from the current
/// status, scan error, and join outcome. Empty strings render nothing.
pub(crate) fn hint_lines(
    link_state: Option<rt::WifiLinkState>,
    status_error: Option<WifiOpError>,
    join_error: Option<WifiOpError>,
    scan_error: Option<WifiOpError>,
) -> [&'static str; 3] {
    let mut hints = ["", "", ""];
    hints[0] = match (status_error, link_state) {
        (Some(WifiOpError::Unsupported), _) | (None, None) => "HINT: NO WIRELESS DEVICE",
        (Some(_), _) => "HINT: STATUS UNAVAILABLE",
        (None, Some(state)) => match state {
            rt::WifiLinkState::Down => "HINT: LINK DOWN - CHECK DEVICE",
            rt::WifiLinkState::Scanning => "HINT: SCANNING - WAIT",
            rt::WifiLinkState::Authenticating | rt::WifiLinkState::Associating => {
                "HINT: AUTH IN PROGRESS - WAIT"
            }
            rt::WifiLinkState::Connected => "HINT: LINK UP",
        },
    };
    if join_error.is_some() {
        hints[1] = "HINT: RECHECK PSK OR SIGNAL";
    }
    if matches!(scan_error, Some(WifiOpError::Unsupported)) {
        hints[2] = "HINT: SCAN NEEDS A DEVICE";
    }
    hints
}

/// Trigger a scan and record the decoded rows plus the total found. Any
/// failure lands in `scan_error`; the list stays empty — never fabricated.
pub(crate) fn run_scan(network_handle: rt::Handle, state: &mut WifiUiState) -> bool {
    let result = rt::network_wifi_scan(network_handle, &mut state.scans);
    match result {
        Ok(total) => {
            state.scan_count = state.scans.len();
            state.scan_total = total;
            state.scan_error = None;
        }
        Err(error) => {
            state.scan_count = 0;
            state.scan_total = 0;
            state.scan_error = Some(classify(error));
        }
    }
    true
}

/// Refresh the saved-network list. Returns the records carried and the
/// total kept; the list itself is re-queried per frame for display.
pub(crate) fn run_saved_list(
    network_handle: rt::Handle,
    saved: &mut [rt::NetworkWifiSavedNetwork],
) -> Result<usize, WifiOpError> {
    rt::network_wifi_saved_list(network_handle, saved).map_err(classify)
}

/// Join the selected scan row. Secured rows first route through the psk
/// sub-prompt; open rows join directly without one.
pub(crate) fn begin_join(network_handle: rt::Handle, state: &mut WifiUiState) -> bool {
    if state.selected_scan >= state.scan_count {
        return false;
    }
    let entry = state.scans[state.selected_scan];
    if join_needs_psk(entry.security) {
        state.prompt_len = 0;
        state.prompt_edit = [0; crate::state::WIFI_EDIT_MAX_BYTES];
        state.prompt = Some(WifiPrompt::JoinPsk);
    } else {
        let ssid = ssid_str(&entry.ssid, entry.ssid_len);
        state.join_outcome = Some(
            rt::network_wifi_join(network_handle, ssid, None).map_err(classify),
        );
    }
    true
}

fn commit_join(
    network_handle: rt::Handle,
    entry: rt::NetworkWifiScanEntry,
    psk: Option<&str>,
    state: &mut WifiUiState,
) {
    let ssid = ssid_str(&entry.ssid, entry.ssid_len);
    state.join_outcome = Some(
        rt::network_wifi_join(network_handle, ssid, psk)
            .map_err(classify),
    );
}

/// Feed one typed character into the active prompt stage. Each stage has
/// its own alphabet and length cap; overflow is a silent no-op like the
/// note/hostname editors.
pub(crate) fn wifi_prompt_char(state: &mut WifiUiState, ch: char) -> bool {
    let Some(prompt) = state.prompt else {
        return false;
    };
    let (max_bytes, allow_space, digits_only) = match prompt {
        WifiPrompt::JoinPsk | WifiPrompt::SavedPsk => (crate::state::WIFI_PSK_MAX_BYTES, true, false),
        WifiPrompt::SavedSsid => (WIFI_SSID_MAX_BYTES, true, false),
        WifiPrompt::SavedPriority => (3, false, true),
    };
    let allowed = if digits_only {
        ch.is_ascii_digit()
    } else {
        ch.is_ascii_graphic() || (allow_space && ch == ' ')
    };
    if !allowed {
        return false;
    }
    if state.prompt_len >= max_bytes {
        return false;
    }
    state.prompt_edit[state.prompt_len] = ch as u8;
    state.prompt_len += 1;
    true
}

pub(crate) fn wifi_prompt_backspace(state: &mut WifiUiState) -> bool {
    if state.prompt.is_none() || state.prompt_len == 0 {
        return false;
    }
    state.prompt_len -= 1;
    true
}

/// Parse the typed priority: empty means 0; otherwise digits only, value
/// must fit u8.
pub(crate) fn parse_priority(edit: &[u8]) -> Option<u8> {
    if edit.is_empty() {
        return Some(0);
    }
    let mut value: usize = 0;
    for byte in edit {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value * 10 + (byte - b'0') as usize;
        if value > u8::MAX as usize {
            return None;
        }
    }
    Some(value as u8)
}

/// Commit the active prompt stage. Ssid/psk stages advance the saved-add
/// flow; the final priority stage (or a join psk) performs the transport
/// call. Returns whether the frame changed.
pub(crate) fn wifi_prompt_enter(network_handle: rt::Handle, state: &mut WifiUiState) -> bool {
    let Some(prompt) = state.prompt else {
        return false;
    };
    match prompt {
        WifiPrompt::JoinPsk => {
            let entry = state.scans[state.selected_scan];
            let mut psk_buf = [0u8; crate::state::WIFI_PSK_MAX_BYTES];
            psk_buf[..state.prompt_len].copy_from_slice(&state.prompt_edit[..state.prompt_len]);
            let psk = core::str::from_utf8(&psk_buf[..state.prompt_len]).ok();
            commit_join(network_handle, entry, psk, state);
            state.stop_editing();
            true
        }
        WifiPrompt::SavedSsid => {
            if state.prompt_len == 0 {
                return false;
            }
            state.add_ssid_len = state.prompt_len;
            state.add_ssid = [0; WIFI_SSID_MAX_BYTES];
            state.add_ssid[..state.prompt_len]
                .copy_from_slice(&state.prompt_edit[..state.prompt_len]);
            state.prompt_len = 0;
            state.prompt_edit = [0; crate::state::WIFI_EDIT_MAX_BYTES];
            state.prompt = Some(WifiPrompt::SavedPsk);
            true
        }
        WifiPrompt::SavedPsk => {
            if state.prompt_len == 0 {
                return false;
            }
            state.add_psk_len = state.prompt_len;
            state.add_psk = [0; crate::state::WIFI_PSK_MAX_BYTES];
            state.add_psk[..state.prompt_len].copy_from_slice(&state.prompt_edit[..state.prompt_len]);
            state.prompt_len = 0;
            state.prompt_edit = [0; crate::state::WIFI_EDIT_MAX_BYTES];
            state.prompt = Some(WifiPrompt::SavedPriority);
            true
        }
        WifiPrompt::SavedPriority => {
            let Some(priority) = parse_priority(&state.prompt_edit[..state.prompt_len]) else {
                return false;
            };
            let ssid = core::str::from_utf8(&state.add_ssid[..state.add_ssid_len]).unwrap_or("");
            let psk = core::str::from_utf8(&state.add_psk[..state.add_psk_len]).unwrap_or("");
            state.saved_add_outcome = Some(
                rt::network_wifi_saved_add(network_handle, ssid, psk, priority)
                    .map_err(classify),
            );
            state.stop_editing();
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::WIFI_EDIT_MAX_BYTES;

    fn scan_entry(security: rt::WifiSecurity) -> rt::NetworkWifiScanEntry {
        let mut entry = rt::NetworkWifiScanEntry {
            bssid: [1, 2, 3, 4, 5, 6],
            channel: 6,
            rssi: -55,
            ssid_len: 4,
            ssid: [0; WIFI_SSID_MAX_BYTES],
            security,
        };
        entry.ssid[..4].copy_from_slice(b"home");
        entry
    }

    #[test]
    fn classify_maps_unsupported_and_transport_honestly() {
        assert_eq!(classify(rt::Error::Unsupported), WifiOpError::Unsupported);
        assert_eq!(classify(rt::Error::InvalidArgument), WifiOpError::Invalid);
        assert_eq!(classify(rt::Error::BufferTooSmall), WifiOpError::Invalid);
        assert_eq!(classify(rt::Error::NotFound), WifiOpError::Rejected);
        assert_eq!(classify(rt::Error::Busy), WifiOpError::Rejected);
        assert_eq!(classify(rt::Error::CapacityExceeded), WifiOpError::Rejected);
        assert_eq!(classify(rt::Error::BrokenPipe), WifiOpError::Transport);
        assert_eq!(classify(rt::Error::Unknown(99)), WifiOpError::Transport);
        assert_eq!(classify(rt::Error::QueueEmpty), WifiOpError::Transport);
    }

    #[test]
    fn link_and_security_names_are_honest() {
        assert_eq!(link_state_name(rt::WifiLinkState::Down), "DOWN");
        assert_eq!(link_state_name(rt::WifiLinkState::Scanning), "SCAN");
        assert_eq!(link_state_name(rt::WifiLinkState::Authenticating), "AUTH");
        assert_eq!(link_state_name(rt::WifiLinkState::Associating), "ASSOC");
        assert_eq!(link_state_name(rt::WifiLinkState::Connected), "UP");
        assert_eq!(security_tag(rt::WifiSecurity::Open), "OPN");
        assert_eq!(security_tag(rt::WifiSecurity::Wpa2), "WPA2");
        assert_eq!(security_tag(rt::WifiSecurity::Wpa3), "WPA3");
        assert_eq!(security_tag(rt::WifiSecurity::Unknown), "SEC?");
    }

    #[test]
    fn scan_row_renders_channel_rssi_security_ssid() {
        let row = scan_row_text::<48>(&scan_entry(rt::WifiSecurity::Wpa2));
        assert_eq!(row.as_str(), " 6  -55DBM WPA2 home");
    }

    #[test]
    fn saved_row_renders_ssid_and_priority() {
        let mut record = rt::NetworkWifiSavedNetwork {
            ssid_len: 4,
            ssid: [0; WIFI_SSID_MAX_BYTES],
            priority: 3,
        };
        record.ssid[..4].copy_from_slice(b"home");
        assert_eq!(saved_row_text::<24>(&record).as_str(), "home P3");
    }

    #[test]
    fn outcome_text_distinguishes_ok_from_failed() {
        let ok = join_outcome_text::<32>(&Ok(rt::WifiLinkState::Connected));
        assert_eq!(ok.as_str(), "JOIN OK UP");
        let failed = join_outcome_text::<32>(&Err(WifiOpError::Unsupported));
        assert_eq!(failed.as_str(), "JOIN FAILED UNSUPPORTED");
        assert_eq!(
            saved_outcome_text::<32>(Some(Ok(())), None)
                .unwrap()
                .as_str(),
            "SAVED ADD OK"
        );
        assert_eq!(
            saved_outcome_text::<32>(Some(Err(WifiOpError::Rejected)), None)
                .unwrap()
                .as_str(),
            "SAVED ADD FAILED REFUSED"
        );
        assert_eq!(
            saved_outcome_text::<32>(None, Some(Err(WifiOpError::Transport)))
                .unwrap()
                .as_str(),
            "SAVED REMOVE FAILED UNAVAILABLE"
        );
        assert!(saved_outcome_text::<32>(None, None).is_none());
    }

    #[test]
    fn hints_are_state_based_and_honest() {
        // No backend today: status Unsupported and scan errors map to the
        // same honest no-device story.
        let hints = hint_lines(
            None,
            Some(WifiOpError::Unsupported),
            Some(WifiOpError::Unsupported),
            Some(WifiOpError::Unsupported),
        );
        assert_eq!(hints[0], "HINT: NO WIRELESS DEVICE");
        assert_eq!(hints[1], "HINT: RECHECK PSK OR SIGNAL");
        assert_eq!(hints[2], "HINT: SCAN NEEDS A DEVICE");

        let down = hint_lines(Some(rt::WifiLinkState::Down), None, None, None);
        assert_eq!(down[0], "HINT: LINK DOWN - CHECK DEVICE");

        let scanning = hint_lines(Some(rt::WifiLinkState::Scanning), None, None, None);
        assert_eq!(scanning[0], "HINT: SCANNING - WAIT");

        let auth = hint_lines(Some(rt::WifiLinkState::Authenticating), None, None, None);
        assert_eq!(auth[0], "HINT: AUTH IN PROGRESS - WAIT");

        let up = hint_lines(Some(rt::WifiLinkState::Connected), None, None, None);
        assert_eq!(up[0], "HINT: LINK UP");

        let other = hint_lines(None, Some(WifiOpError::Transport), None, None);
        assert_eq!(other[0], "HINT: STATUS UNAVAILABLE");

        let quiet = hint_lines(None, Some(WifiOpError::Unsupported), None, None);
        assert_eq!(quiet, ["HINT: NO WIRELESS DEVICE", "", ""]);
    }

    #[test]
    fn prompt_chars_follow_stage_rules() {
        let mut state = WifiUiState::new();

        // No prompt: typing is a no-op.
        assert!(!wifi_prompt_char(&mut state, 'a'));

        state.prompt = Some(WifiPrompt::SavedPriority);
        assert!(wifi_prompt_char(&mut state, '3'));
        assert!(!wifi_prompt_char(&mut state, 'x'));
        assert!(!wifi_prompt_char(&mut state, ' '));
        assert_eq!(state.prompt_len, 1);

        state.prompt = Some(WifiPrompt::SavedSsid);
        assert!(wifi_prompt_char(&mut state, ' '));
        assert_eq!(state.prompt_edit[..2], *b"3 ");

        state.prompt = Some(WifiPrompt::JoinPsk);
        state.prompt_len = 0;
        state.prompt_edit = [0; WIFI_EDIT_MAX_BYTES];
        for ch in b"pass word 12" {
            assert!(wifi_prompt_char(&mut state, *ch as char));
        }
        assert_eq!(state.prompt_len, 12);
        assert_eq!(&state.prompt_edit[..12], b"pass word 12");

        // Cap: psk never exceeds 64 bytes.
        state.prompt_len = WIFI_EDIT_MAX_BYTES;
        assert!(!wifi_prompt_char(&mut state, 'z'));
    }

    #[test]
    fn prompt_backspace_trims_and_ignores_empty() {
        let mut state = WifiUiState::new();
        assert!(!wifi_prompt_backspace(&mut state));
        state.prompt = Some(WifiPrompt::JoinPsk);
        assert!(wifi_prompt_char(&mut state, 'a'));
        assert!(wifi_prompt_backspace(&mut state));
        assert_eq!(state.prompt_len, 0);
        assert!(!wifi_prompt_backspace(&mut state));
    }

    #[test]
    fn priority_parse_empty_means_zero_and_rejects_overflow() {
        assert_eq!(parse_priority(b""), Some(0));
        assert_eq!(parse_priority(b"3"), Some(3));
        assert_eq!(parse_priority(b"007"), Some(7));
        assert_eq!(parse_priority(b"255"), Some(255));
        assert_eq!(parse_priority(b"256"), None);
        assert_eq!(parse_priority(b"x"), None);
        assert_eq!(parse_priority(b"1x"), None);
    }

    #[test]
    fn join_prompt_enter_attempts_join_and_closes() {
        let mut state = WifiUiState::new();
        state.scans[0] = scan_entry(rt::WifiSecurity::Wpa2);
        state.scan_count = 1;
        state.selected_scan = 0;
        state.prompt = Some(WifiPrompt::JoinPsk);
        for ch in b"passphrase1" {
            let _ = wifi_prompt_char(&mut state, *ch as char);
        }
        // INVALID_HANDLE: transport fails but the attempt is recorded
        // honestly and the prompt closes.
        assert!(wifi_prompt_enter(rt::INVALID_HANDLE, &mut state));
        assert!(state.prompt.is_none());
        assert_eq!(state.prompt_len, 0);
        assert!(state.join_outcome.is_some());
    }

    #[test]
    fn saved_add_prompt_walks_three_stages() {
        let mut state = WifiUiState::new();
        state.prompt = Some(WifiPrompt::SavedSsid);

        // Empty ssid stays in the stage.
        assert!(!wifi_prompt_enter(rt::INVALID_HANDLE, &mut state));
        assert_eq!(state.prompt, Some(WifiPrompt::SavedSsid));

        for ch in "cafe".chars() {
            assert!(wifi_prompt_char(&mut state, ch));
        }
        assert!(wifi_prompt_enter(rt::INVALID_HANDLE, &mut state));
        assert_eq!(state.prompt, Some(WifiPrompt::SavedPsk));
        assert_eq!(state.add_ssid_len, 4);
        assert_eq!(&state.add_ssid[..4], b"cafe");
        assert_eq!(state.prompt_len, 0);

        for ch in b"password12" {
            assert!(wifi_prompt_char(&mut state, *ch as char));
        }
        assert!(wifi_prompt_enter(rt::INVALID_HANDLE, &mut state));
        assert_eq!(state.prompt, Some(WifiPrompt::SavedPriority));
        assert_eq!(state.add_psk_len, 10);

        // Priority commit: digits parsed, transport attempted, prompt ends.
        assert!(wifi_prompt_char(&mut state, '2'));
        assert!(wifi_prompt_enter(rt::INVALID_HANDLE, &mut state));
        assert!(state.prompt.is_none());
        assert!(state.saved_add_outcome.is_some());
        assert_eq!(state.add_ssid_len, 0);
        assert_eq!(state.add_psk_len, 0);
    }

    #[test]
    fn saved_add_priority_rejects_non_digits_and_overflow() {
        let mut state = WifiUiState::new();
        state.prompt = Some(WifiPrompt::SavedPriority);
        state.add_ssid_len = 4;
        state.add_ssid[..4].copy_from_slice(b"cafe");
        state.add_psk_len = 10;
        state.add_psk[..10].copy_from_slice(b"password12");

        // Digits-only alphabet: letters and space never enter the buffer.
        assert!(wifi_prompt_char(&mut state, '2'));
        assert!(!wifi_prompt_char(&mut state, 'x'));
        assert!(!wifi_prompt_char(&mut state, ' '));
        assert!(wifi_prompt_char(&mut state, '5'));
        assert!(wifi_prompt_char(&mut state, '6'));

        // 256 overflows u8: enter refuses and the stage stays open.
        assert!(!wifi_prompt_enter(rt::INVALID_HANDLE, &mut state));
        assert_eq!(state.prompt, Some(WifiPrompt::SavedPriority));
        assert!(state.saved_add_outcome.is_none());

        // Back to a valid priority: enter commits (transport fails on the
        // invalid handle) and the prompt closes.
        assert!(wifi_prompt_backspace(&mut state));
        assert_eq!(state.prompt_len, 2);
        assert!(wifi_prompt_enter(rt::INVALID_HANDLE, &mut state));
        assert!(state.prompt.is_none());
        assert!(state.saved_add_outcome.is_some());
    }

    #[test]
    fn begin_join_routes_open_direct_and_secured_to_prompt() {
        let mut state = WifiUiState::new();

        // No selection: no-op.
        assert!(!begin_join(rt::INVALID_HANDLE, &mut state));

        state.scans[0] = scan_entry(rt::WifiSecurity::Open);
        state.scan_count = 1;
        // Open network joins directly (no prompt); INVALID_HANDLE transport
        // fails but the attempt is recorded honestly.
        assert!(begin_join(rt::INVALID_HANDLE, &mut state));
        assert!(state.join_outcome.is_some());
        assert!(state.prompt.is_none());

        state.scans[1] = scan_entry(rt::WifiSecurity::Wpa3);
        state.scan_count = 2;
        state.selected_scan = 1;
        state.join_outcome = None;
        assert!(begin_join(rt::INVALID_HANDLE, &mut state));
        assert_eq!(state.prompt, Some(WifiPrompt::JoinPsk));
        assert_eq!(state.prompt_len, 0);
    }
}

use serviceos_userspace_runtime as rt;

pub(crate) const NOTE_MAX_BYTES: usize = 24;
pub(crate) const SETTINGS_PCM_STREAMS_MAX: usize = 4;
pub(crate) const BUFFER_WIDTH: u32 = 1024;
pub(crate) const BUFFER_HEIGHT: u32 = 768;
pub(crate) const BUFFER_BYTES: usize = BUFFER_WIDTH as usize * BUFFER_HEIGHT as usize * 4;
pub(crate) const SURFACE_BUFFER_SLOTS: usize = 2;
pub(crate) const PIXEL_STRIDE: usize = BUFFER_WIDTH as usize;
pub(crate) const TAB_SYSTEM_X0: i32 = 10;
pub(crate) const TAB_SYSTEM_X1: i32 = 98;
pub(crate) const TAB_SECURITY_X0: i32 = 106;
pub(crate) const TAB_SECURITY_X1: i32 = 214;
pub(crate) const TAB_NETWORK_X0: i32 = 222;
pub(crate) const TAB_NETWORK_X1: i32 = 322;
pub(crate) const TAB_Y0: i32 = 36;
pub(crate) const TAB_Y1: i32 = 56;
pub(crate) const NOTE_FIELD_X0: i32 = 10;
pub(crate) const NOTE_FIELD_Y0: i32 = 114;
pub(crate) const NOTE_FIELD_X1: i32 = 232;
pub(crate) const NOTE_FIELD_Y1: i32 = 138;
pub(crate) const AUDIO_TEST_X0: i32 = 10;
pub(crate) const AUDIO_TEST_Y0: i32 = 144;
pub(crate) const AUDIO_TEST_X1: i32 = 118;
pub(crate) const AUDIO_TEST_Y1: i32 = 164;
pub(crate) const SEC_PREV_X0: i32 = 10;
pub(crate) const SEC_PREV_X1: i32 = 58;
pub(crate) const SEC_NEXT_X0: i32 = 66;
pub(crate) const SEC_NEXT_X1: i32 = 114;
pub(crate) const SEC_ACTION_Y0: i32 = 146;
pub(crate) const SEC_ACTION_Y1: i32 = 166;
pub(crate) const SEC_ALLOW_X0: i32 = 122;
pub(crate) const SEC_ALLOW_X1: i32 = 174;
pub(crate) const SEC_BLOCK_X0: i32 = 182;
pub(crate) const SEC_BLOCK_X1: i32 = 234;
pub(crate) const SEC_DEFAULT_X0: i32 = 242;
pub(crate) const SEC_DEFAULT_X1: i32 = 308;
pub(crate) const SEC_RUNTIME_Y0: i32 = 176;
pub(crate) const SEC_RUNTIME_Y1: i32 = 196;
pub(crate) const SEC_APPROVE_X0: i32 = 122;
pub(crate) const SEC_APPROVE_X1: i32 = 190;
pub(crate) const SEC_DENY_X0: i32 = 198;
pub(crate) const SEC_DENY_X1: i32 = 246;
pub(crate) const SEC_RESET_X0: i32 = 254;
pub(crate) const SEC_RESET_X1: i32 = 308;
pub(crate) const NET_HOSTNAME_FIELD_X0: i32 = 100;
pub(crate) const NET_HOSTNAME_FIELD_Y0: i32 = 98;
pub(crate) const NET_HOSTNAME_FIELD_X1: i32 = 236;
pub(crate) const NET_HOSTNAME_FIELD_Y1: i32 = 118;
pub(crate) const NET_PING_RUN_X0: i32 = 100;
pub(crate) const NET_PING_RUN_Y0: i32 = 152;
pub(crate) const NET_PING_RUN_X1: i32 = 190;
pub(crate) const NET_PING_RUN_Y1: i32 = 170;
pub(crate) const TAB_WIFI_X0: i32 = 330;
pub(crate) const TAB_WIFI_X1: i32 = 398;
pub(crate) const TAB_BACKUP_X0: i32 = 406;
pub(crate) const TAB_BACKUP_X1: i32 = 486;
pub(crate) const BACKUP_BTN_Y0: i32 = 196;
pub(crate) const BACKUP_BTN_Y1: i32 = 212;
pub(crate) const BACKUP_EXPORT_BTN_X0: i32 = 10;
pub(crate) const BACKUP_EXPORT_BTN_X1: i32 = 86;
pub(crate) const BACKUP_RESTORE_BTN_X0: i32 = 94;
pub(crate) const BACKUP_RESTORE_BTN_X1: i32 = 170;
pub(crate) const BACKUP_DELETE_BTN_X0: i32 = 178;
pub(crate) const BACKUP_DELETE_BTN_X1: i32 = 254;
pub(crate) const BACKUP_LIST_Y0: i32 = 230;
/// Snapshot row pitch on the Backup page list (render steps 10px per row).
pub(crate) const BACKUP_ROW_H: i32 = 10;
/// Prompt confirm/cancel buttons (rendered only while a prompt is up).
pub(crate) const BACKUP_PROMPT_BTN_Y0: i32 = 336;
pub(crate) const BACKUP_PROMPT_BTN_Y1: i32 = 352;
pub(crate) const BACKUP_CONFIRM_BTN_X0: i32 = 10;
pub(crate) const BACKUP_CONFIRM_BTN_X1: i32 = 86;
pub(crate) const BACKUP_CANCEL_BTN_X0: i32 = 94;
pub(crate) const BACKUP_CANCEL_BTN_X1: i32 = 170;
pub(crate) const WIFI_BTN_Y0: i32 = 106;
pub(crate) const WIFI_BTN_Y1: i32 = 122;
pub(crate) const WIFI_SCAN_BTN_X0: i32 = 10;
pub(crate) const WIFI_SCAN_BTN_X1: i32 = 86;
pub(crate) const WIFI_JOIN_BTN_X0: i32 = 94;
pub(crate) const WIFI_JOIN_BTN_X1: i32 = 170;
pub(crate) const WIFI_ROW_X0: i32 = 10;
pub(crate) const WIFI_ROW_X1: i32 = 310;
pub(crate) const WIFI_ROW_H: i32 = 12;
pub(crate) const WIFI_SCAN_ROW_Y0: i32 = 140;
pub(crate) const WIFI_SAVED_ROW_Y0: i32 = 178;
pub(crate) const WIFI_ADD_BTN_X0: i32 = 10;
pub(crate) const WIFI_ADD_BTN_X1: i32 = 86;
pub(crate) const WIFI_REMOVE_BTN_X0: i32 = 94;
pub(crate) const WIFI_REMOVE_BTN_X1: i32 = 186;
pub(crate) const WIFI_ACTION_Y0: i32 = 202;
pub(crate) const WIFI_ACTION_Y1: i32 = 218;
/// Mirrors the network-service MAX_HOSTNAME_BYTES decode buffer.
pub(crate) const HOSTNAME_EDIT_MAX_BYTES: usize = 48;
pub(crate) const PING_TARGET_MAX_BYTES: usize = 16;
pub(crate) const PING_PROBE_COUNT: usize = 4;
/// Reply entry caps the network-service documents for the wireless family.
pub(crate) const WIFI_SCAN_ROWS: usize = rt::NETWORK_WIFI_SCAN_REPLY_ENTRIES_MAX;
pub(crate) const WIFI_SAVED_ROWS: usize = rt::NETWORK_WIFI_SAVED_REPLY_ENTRIES_MAX;
pub(crate) const WIFI_SSID_MAX_BYTES: usize = rt::NETWORK_WIFI_SSID_BYTES_MAX;
pub(crate) const WIFI_PSK_MAX_BYTES: usize = rt::NETWORK_WIFI_PSK_BYTES_MAX;
pub(crate) const WIFI_EDIT_MAX_BYTES: usize = WIFI_PSK_MAX_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsPage {
    System,
    Security,
    Network,
    Wifi,
    Backup,
}

#[derive(Clone, Copy)]
pub(crate) struct PendingRuntime {
    pub(crate) env_id: u32,
    pub(crate) state: rt::RuntimeEnvState,
    pub(crate) capabilities: u32,
}

pub(crate) struct AppState {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) focused: bool,
    pub(crate) page: SettingsPage,
    pub(crate) editing_note: bool,
    pub(crate) editing_hostname: bool,
    pub(crate) selected_policy_index: usize,
    pub(crate) note: [u8; NOTE_MAX_BYTES],
    pub(crate) note_len: usize,
    pub(crate) hostname_edit: [u8; HOSTNAME_EDIT_MAX_BYTES],
    pub(crate) hostname_edit_len: usize,
    pub(crate) ping_stats: Option<rt::NetworkDiagPingStats>,
    pub(crate) ping_failed: bool,
    pub(crate) ping_target: [u8; PING_TARGET_MAX_BYTES],
    pub(crate) ping_target_len: usize,
    pub(crate) wifi: WifiUiState,
    pub(crate) backup: crate::backup::BackupUiState,
}

impl SettingsPage {
    pub(crate) fn next(self) -> Self {
        match self {
            SettingsPage::System => SettingsPage::Security,
            SettingsPage::Security => SettingsPage::Network,
            SettingsPage::Network => SettingsPage::Wifi,
            SettingsPage::Wifi => SettingsPage::Backup,
            SettingsPage::Backup => SettingsPage::System,
        }
    }
}

/// Modal prompt stages on the Wi-Fi page. Join psk for a secured scan row,
/// then the three-stage saved-network add flow (ssid, psk, priority).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WifiPrompt {
    JoinPsk,
    SavedSsid,
    SavedPsk,
    SavedPriority,
}

/// Honest classification of a failed wireless wrapper call. Unsupported is
/// the expected reply on boots with no wireless backend device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WifiOpError {
    Unsupported,
    Invalid,
    Rejected,
    Transport,
}

pub(crate) struct WifiUiState {
    pub(crate) scans: [rt::NetworkWifiScanEntry; WIFI_SCAN_ROWS],
    pub(crate) scan_count: usize,
    pub(crate) scan_total: usize,
    pub(crate) scan_error: Option<WifiOpError>,
    pub(crate) selected_scan: usize,
    pub(crate) join_outcome: Option<Result<rt::WifiLinkState, WifiOpError>>,
    pub(crate) saved: [rt::NetworkWifiSavedNetwork; WIFI_SAVED_ROWS],
    pub(crate) saved_count: usize,
    pub(crate) saved_total: usize,
    pub(crate) saved_add_outcome: Option<Result<(), WifiOpError>>,
    pub(crate) saved_remove_outcome: Option<Result<(), WifiOpError>>,
    pub(crate) selected_saved: usize,
    pub(crate) prompt: Option<WifiPrompt>,
    pub(crate) prompt_edit: [u8; WIFI_EDIT_MAX_BYTES],
    pub(crate) prompt_len: usize,
    pub(crate) add_ssid: [u8; WIFI_SSID_MAX_BYTES],
    pub(crate) add_ssid_len: usize,
    pub(crate) add_psk: [u8; WIFI_PSK_MAX_BYTES],
    pub(crate) add_psk_len: usize,
}

impl WifiUiState {
    pub(crate) fn new() -> Self {
        Self {
            scans: [rt::NetworkWifiScanEntry {
                bssid: [0; 6],
                channel: 0,
                rssi: 0,
                ssid_len: 0,
                ssid: [0; WIFI_SSID_MAX_BYTES],
                security: rt::WifiSecurity::Unknown,
            }; WIFI_SCAN_ROWS],
            scan_count: 0,
            scan_total: 0,
            scan_error: None,
            selected_scan: 0,
            join_outcome: None,
            saved: [rt::NetworkWifiSavedNetwork {
                ssid_len: 0,
                ssid: [0; WIFI_SSID_MAX_BYTES],
                priority: 0,
            }; WIFI_SAVED_ROWS],
            saved_count: 0,
            saved_total: 0,
            saved_add_outcome: None,
            saved_remove_outcome: None,
            selected_saved: 0,
            prompt: None,
            prompt_edit: [0; WIFI_EDIT_MAX_BYTES],
            prompt_len: 0,
            add_ssid: [0; WIFI_SSID_MAX_BYTES],
            add_ssid_len: 0,
            add_psk: [0; WIFI_PSK_MAX_BYTES],
            add_psk_len: 0,
        }
    }

    /// Close the modal prompt and discard any half-typed stage data.
    pub(crate) fn stop_editing(&mut self) {
        self.prompt = None;
        self.prompt_len = 0;
        self.prompt_edit = [0; WIFI_EDIT_MAX_BYTES];
        self.add_ssid_len = 0;
        self.add_ssid = [0; WIFI_SSID_MAX_BYTES];
        self.add_psk_len = 0;
        self.add_psk = [0; WIFI_PSK_MAX_BYTES];
    }
}

pub(crate) fn settings_pcm_stream_info() -> rt::AudioStreamInfo {
    rt::AudioStreamInfo {
        slot: 0,
        direction: rt::AudioStreamDirection::Playback,
        state: rt::AudioStreamState::Closed,
        session_id: 0,
        endpoint_index: 0,
        frequency_hz: 0,
        remaining_ticks: 0,
    }
}

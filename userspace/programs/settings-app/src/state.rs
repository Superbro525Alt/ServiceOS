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
/// Mirrors the network-service MAX_HOSTNAME_BYTES decode buffer.
pub(crate) const HOSTNAME_EDIT_MAX_BYTES: usize = 48;
pub(crate) const PING_TARGET_MAX_BYTES: usize = 16;
pub(crate) const PING_PROBE_COUNT: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsPage {
    System,
    Security,
    Network,
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
}

impl SettingsPage {
    pub(crate) fn next(self) -> Self {
        match self {
            SettingsPage::System => SettingsPage::Security,
            SettingsPage::Security => SettingsPage::Network,
            SettingsPage::Network => SettingsPage::System,
        }
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

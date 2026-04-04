use serviceos_userspace_runtime as rt;

pub(crate) const NOTE_MAX_BYTES: usize = 24;
pub(crate) const BUFFER_WIDTH: u32 = 1024;
pub(crate) const BUFFER_HEIGHT: u32 = 768;
pub(crate) const BUFFER_BYTES: usize = BUFFER_WIDTH as usize * BUFFER_HEIGHT as usize * 4;
pub(crate) const SURFACE_BUFFER_SLOTS: usize = 2;
pub(crate) const PIXEL_STRIDE: usize = BUFFER_WIDTH as usize;
pub(crate) const TAB_SYSTEM_X0: i32 = 10;
pub(crate) const TAB_SYSTEM_X1: i32 = 98;
pub(crate) const TAB_SECURITY_X0: i32 = 106;
pub(crate) const TAB_SECURITY_X1: i32 = 214;
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsPage {
    System,
    Security,
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
    pub(crate) selected_policy_index: usize,
    pub(crate) note: [u8; NOTE_MAX_BYTES],
    pub(crate) note_len: usize,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioEndpointBackend {
    Unknown = 0,
    PcSpeaker = 1,
    VirtioSound = 2,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioEndpointDirection {
    Output = 1,
    Input = 2,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioEndpointState {
    Offline = 0,
    Idle = 1,
    Active = 2,
}

pub mod audio_capability {
    pub const PLAYBACK: u32 = 1 << 0;
    pub const CAPTURE: u32 = 1 << 1;
    pub const TONE: u32 = 1 << 2;
    pub const PCM: u32 = 1 << 3;
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioSampleFormat {
    U8 = 1,
    S16Le = 2,
    S32Le = 3,
    F32Le = 4,
}

pub mod audio_stream_write_flag {
    pub const BLOCKING: u64 = 1 << 0;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioEndpointInfo {
    pub backend: u32,
    pub direction: u32,
    pub state: u32,
    pub capabilities: u32,
    pub nominal_rate_hz: u32,
    pub channels: u32,
    pub min_frequency_hz: u32,
    pub max_frequency_hz: u32,
    pub current_frequency_hz: u32,
    pub reserved: u32,
    pub play_count: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioToneRequest {
    pub frequency_hz: u32,
    pub duration_ticks: u32,
    pub volume: u16,
    pub flags: u16,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioTag {
    EndpointListRequest = 0x880,
    EndpointListReply = 0x881,
    EndpointStatusRequest = 0x882,
    EndpointStatusReply = 0x883,
    StreamOpenRequest = 0x884,
    StreamOpenReply = 0x885,
    StreamListRequest = 0x886,
    StreamListReply = 0x887,
    StreamStatusRequest = 0x888,
    StreamStatusReply = 0x889,
    StreamPlayToneRequest = 0x88a,
    StreamPlayToneReply = 0x88b,
    StreamCloseRequest = 0x88c,
    StreamCloseReply = 0x88d,
    StreamConfigureRequest = 0x88e,
    StreamConfigureReply = 0x88f,
    StreamWriteRequest = 0x890,
    StreamWriteReply = 0x891,
    StreamDrainRequest = 0x892,
    StreamDrainReply = 0x893,
    StreamSetVolumeRequest = 0x894,
    StreamSetVolumeReply = 0x895,
    EndpointVolumeSetRequest = 0x896,
    EndpointVolumeSetReply = 0x897,
    /// Reserved groundwork: capture-stream open negotiation. Services must
    /// answer these with `AudioStatus::Unsupported` until capture lands.
    CaptureOpenRequest = 0x898,
    CaptureOpenReply = 0x899,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioStatus {
    Ok = 0,
    NotFound = 1,
    Busy = 2,
    Unsupported = 3,
    Denied = 4,
    CapacityExceeded = 5,
    Closed = 6,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioStreamDirection {
    Playback = 1,
    Capture = 2,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioStreamState {
    Idle = 1,
    Active = 2,
    Closed = 3,
    Failed = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockDeviceBackend {
    Unknown = 0,
    VirtioPci = 1,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockDeviceInfo {
    pub backend: u32,
    pub writable: u32,
    pub block_size: u32,
    pub reserved: u32,
    pub block_count: u64,
    pub read_ops: u64,
    pub write_ops: u64,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayOutputBackend {
    Unknown = 0,
    BootFramebuffer = 1,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayOutputState {
    Disconnected = 0,
    Connected = 1,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayPixelFormat {
    Unknown = 0,
    Xrgb8888 = 1,
    Bgrx8888 = 2,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DisplayOutputInfo {
    pub backend: u32,
    pub state: u32,
    pub pixel_format: u32,
    pub reserved: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub bytes_per_pixel: u32,
    pub byte_len: u64,
    pub present_count: u64,
}

pub const INPUT_SOURCE_FLAG_NONBLOCK: u32 = 1 << 0;

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputSourceBackend {
    Unknown = 0,
    VirtioPci = 1,
}

pub mod input_capability {
    pub const POINTER: u32 = 1 << 0;
    pub const KEYBOARD: u32 = 1 << 1;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputSourceInfo {
    pub backend: u32,
    pub capabilities: u32,
    pub device_count: u32,
    pub pending_events: u32,
}

pub mod input_device_class {
    pub const KEYBOARD: u32 = 1;
    /// Relative pointer (mouse-style).
    pub const POINTER: u32 = 2;
    /// Absolute pointer (tablet-style).
    pub const TABLET: u32 = 3;
}

pub mod input_role_flag {
    /// This instance is the positional authority feeding pointer motion.
    pub const POSITIONAL_AUTHORITY: u32 = 1 << 0;
    /// This instance was demoted by the single-positional-stream policy:
    /// motion is suppressed but buttons and wheel still route.
    pub const SCROLL_ONLY: u32 = 1 << 1;
}

/// One enumerated physical input instance behind a source object.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputDeviceInfo {
    /// Stable per-instance id assigned at bring-up (1-based); matches the
    /// `source_id` tag carried on every `InputEventInfo` it emits.
    pub source_id: u32,
    /// One of `input_device_class`.
    pub class: u32,
    /// Bitmask of `input_role_flag` from the role-unification pass.
    pub role_flags: u32,
    /// 1 while the instance participates in event routing, 0 once marked
    /// absent after removal.
    pub present: u32,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputEventKind {
    PointerMotion = 1,
    PointerButton = 2,
    Key = 3,
    PointerDelta = 4,
    PointerScroll = 5,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputButton {
    Left = 1,
    Right = 2,
    Middle = 3,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputEventInfo {
    pub kind: u32,
    pub code: u32,
    pub value0: i32,
    pub value1: i32,
    /// Originating device instance (`InputDeviceInfo.source_id`); 0 when the
    /// backend cannot attribute the event.
    pub source_id: u32,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphicsTag {
    OutputListRequest = 0x900,
    OutputListReply = 0x901,
    OutputStatusRequest = 0x902,
    OutputStatusReply = 0x903,
    SurfaceCreateRequest = 0x904,
    SurfaceCreateReply = 0x905,
    SurfaceListRequest = 0x906,
    SurfaceListReply = 0x907,
    SurfaceStatusRequest = 0x908,
    SurfaceStatusReply = 0x909,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphicsStatus {
    Ok = 0,
    NotFound = 1,
    Busy = 2,
    Denied = 3,
    CapacityExceeded = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceTag {
    SetGeometryRequest = 0x920,
    SetGeometryReply = 0x921,
    SetFillRequest = 0x922,
    SetFillReply = 0x923,
    SetVisibilityRequest = 0x924,
    SetVisibilityReply = 0x925,
    CloseRequest = 0x926,
    ClearSceneRequest = 0x927,
    ClearSceneReply = 0x928,
    SetRectRequest = 0x929,
    SetRectReply = 0x92a,
    SetLabelRequest = 0x92b,
    SetLabelReply = 0x92c,
    AttachBufferRequest = 0x92d,
    AttachBufferReply = 0x92e,
    PresentBufferRequest = 0x92f,
    PresentBufferReply = 0x930,
    ReleaseBufferRequest = 0x931,
    ReleaseBufferReply = 0x932,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionTag {
    ListRequest = 0x980,
    ListReply = 0x981,
    StatusRequest = 0x982,
    StatusReply = 0x983,
    FocusRequest = 0x984,
    FocusReply = 0x985,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionStatus {
    Ok = 0,
    NotFound = 1,
    Busy = 2,
    Denied = 3,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionInputSource {
    None = 0,
    ServiceControl = 1,
    Hardware = 2,
}

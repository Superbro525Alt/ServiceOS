#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopTag {
    StatusRequest = 0xa00,
    StatusReply = 0xa01,
    ListAppsRequest = 0xa02,
    ListAppsReply = 0xa03,
    LaunchAppRequest = 0xa04,
    LaunchAppReply = 0xa05,
    FocusAppRequest = 0xa06,
    FocusAppReply = 0xa07,
    ListWindowsRequest = 0xa08,
    ListWindowsReply = 0xa09,
    WindowActionRequest = 0xa0a,
    WindowActionReply = 0xa0b,
    InputRequest = 0xa0c,
    InputReply = 0xa0d,
    NotifyRequest = 0xa0e,
    NotifyReply = 0xa0f,
    NotificationHistoryRequest = 0xa10,
    NotificationHistoryReply = 0xa11,
    WorkspaceRequest = 0xa12,
    WorkspaceReply = 0xa13,
    OpenPathRequest = 0xa14,
    OpenPathReply = 0xa15,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopStatus {
    Ok = 0,
    NotFound = 1,
    Busy = 2,
    Denied = 3,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum DesktopAppId {
    Settings = 1,
    Files = 2,
    Monitor = 3,
    Terminal = 4,
    SoftwareCenter = 5,
    Media = 6,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopWindowAction {
    Focus = 1,
    Close = 2,
    Minimize = 3,
    Restore = 4,
    Move = 5,
    Resize = 6,
    FocusNext = 7,
    Maximize = 8,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopInputAction {
    PointerDown = 1,
    PointerMove = 2,
    PointerUp = 3,
    Click = 4,
    KeyDown = 5,
    KeyUp = 6,
    TextInput = 7,
    PointerScroll = 8,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopDragMode {
    None = 0,
    Move = 1,
    Resize = 2,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopWorkspaceAction {
    Status = 1,
    Switch = 2,
    MoveFocused = 3,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppControlTag {
    FocusChanged = 0xac0,
    Resize = 0xac1,
    Close = 0xac2,
    Pointer = 0xac3,
    Key = 0xac4,
    Text = 0xac5,
    OpenPath = 0xac6,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppPointerAction {
    Down = 1,
    Move = 2,
    Up = 3,
    Scroll = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppKeyAction {
    Down = 1,
    Up = 2,
}

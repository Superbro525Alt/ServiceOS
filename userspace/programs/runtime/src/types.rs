use crate::{
    DesktopAppId, DesktopDragMode, DisplayOutputBackend, DisplayOutputState, DisplayPixelFormat,
    LogDomain, LogEvent, LogSeverity, ManagerServicePhase, PacketInterfaceBackend,
    PacketInterfaceLinkState, ServiceId, SessionInputSource,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogRecord {
    pub sequence: u64,
    pub source: ServiceId,
    pub severity: LogSeverity,
    pub domain: LogDomain,
    pub event: LogEvent,
    pub arg0: u64,
    pub arg1: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManagerServiceInfo {
    pub service_id: ServiceId,
    pub phase: ManagerServicePhase,
    pub attempts: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageListEntry {
    pub service_id: ServiceId,
    pub installed: bool,
    pub active: bool,
    pub rollback_available: bool,
    pub repository_versions: u32,
    pub installed_version_len: usize,
    pub active_version_len: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageInfo {
    pub installed: bool,
    pub active: bool,
    pub rollback_available: bool,
    pub repository_versions: u32,
    pub installed_version_len: usize,
    pub active_version_len: usize,
    pub rollback_version_len: usize,
    pub latest_version_len: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkInterfaceStatusInfo {
    pub index: u32,
    pub backend: PacketInterfaceBackend,
    pub link_state: PacketInterfaceLinkState,
    pub mtu: u32,
    pub address: u32,
    pub prefix_len: u8,
    pub gateway: u32,
    pub mac: [u8; 6],
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub dropped_packets: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GraphicsOutputStatusInfo {
    pub index: u32,
    pub backend: DisplayOutputBackend,
    pub state: DisplayOutputState,
    pub pixel_format: DisplayPixelFormat,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub bytes_per_pixel: u32,
    pub byte_len: u64,
    pub present_count: u64,
    pub surface_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GraphicsSurfaceStatusInfo {
    pub surface_id: u32,
    pub output_index: u32,
    pub owner_session: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub z_order: u32,
    pub fill_rgb: u32,
    pub visible: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionStatusInfo {
    pub session_id: u32,
    pub input_source: SessionInputSource,
    pub focused_surface: u32,
    pub surface_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DesktopShellStatusInfo {
    pub session_id: u32,
    pub focused_app: Option<DesktopAppId>,
    pub running_apps: u32,
    pub focused_surface: u32,
    pub drag_mode: DesktopDragMode,
    pub pointer_x: i32,
    pub pointer_y: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DesktopAppInfo {
    pub app_id: DesktopAppId,
    pub running: bool,
    pub focused: bool,
    pub surface_id: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DesktopWindowInfo {
    pub app_id: DesktopAppId,
    pub surface_id: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub z_order: u32,
    pub focused: bool,
    pub minimized: bool,
    pub visible: bool,
}

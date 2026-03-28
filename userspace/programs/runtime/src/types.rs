use crate::{
    AudioEndpointBackend, AudioEndpointDirection, AudioEndpointState, AudioStreamDirection,
    AudioStreamState, DesktopAppId, DesktopDragMode, DisplayOutputBackend, DisplayOutputState,
    DeveloperArtifactFormat, DeveloperJobState, DeveloperTarget, DeveloperToolchainState,
    DisplayPixelFormat, LogDomain, LogEvent, LogSeverity, ManagerServicePhase,
    NetworkConfigMode, NetworkConfigState, NetworkSocketKind, NetworkSocketState,
    PacketInterfaceBackend, PacketInterfaceLinkState, RuntimeEnvState, RuntimeKind,
    RuntimeRunState, RuntimeWorkloadKind, ServiceId, SessionInputSource,
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
    pub config_mode: NetworkConfigMode,
    pub config_state: NetworkConfigState,
    pub address: u32,
    pub prefix_len: u8,
    pub gateway: u32,
    pub dns_server: u32,
    pub mac: [u8; 6],
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub dropped_packets: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkSocketInfo {
    pub slot: u32,
    pub kind: NetworkSocketKind,
    pub state: NetworkSocketState,
    pub remote_address: u32,
    pub remote_port: u16,
    pub local_port: u16,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioEndpointStatusInfo {
    pub index: u32,
    pub backend: AudioEndpointBackend,
    pub direction: AudioEndpointDirection,
    pub state: AudioEndpointState,
    pub capabilities: u32,
    pub nominal_rate_hz: u32,
    pub channels: u32,
    pub min_frequency_hz: u32,
    pub max_frequency_hz: u32,
    pub current_frequency_hz: u32,
    pub play_count: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioStreamInfo {
    pub slot: u32,
    pub direction: AudioStreamDirection,
    pub state: AudioStreamState,
    pub session_id: u32,
    pub endpoint_index: u32,
    pub frequency_hz: u32,
    pub remaining_ticks: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeEnvInfo {
    pub env_id: u32,
    pub kind: RuntimeKind,
    pub state: RuntimeEnvState,
    pub capabilities: u32,
    pub mount_count: u32,
    pub var_count: u32,
    pub active_runs: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeRunInfo {
    pub run_id: u32,
    pub env_id: u32,
    pub workload: RuntimeWorkloadKind,
    pub state: RuntimeRunState,
    pub exit_code: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeveloperToolchainInfo {
    pub toolchain_id: u32,
    pub target: DeveloperTarget,
    pub state: DeveloperToolchainState,
    pub format: DeveloperArtifactFormat,
    pub name_len: u32,
    pub name: [u8; 64],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeveloperWorkspaceInfo {
    pub workspace_id: u32,
    pub target_mask: u32,
    pub name_len: u32,
    pub name: [u8; 64],
    pub source_path_len: u32,
    pub source_path: [u8; 96],
    pub toolchains: [u32; 4],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeveloperJobInfo {
    pub job_id: u32,
    pub workspace_id: u32,
    pub target: DeveloperTarget,
    pub state: DeveloperJobState,
    pub format: DeveloperArtifactFormat,
    pub artifact_size: usize,
    pub artifact_name_len: u32,
    pub artifact_name: [u8; 64],
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

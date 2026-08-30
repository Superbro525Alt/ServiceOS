use crate::{
    AudioEndpointBackend, AudioEndpointDirection, AudioEndpointState, AudioStreamDirection,
    AudioStreamState, DesktopAppId, DesktopDragMode, DeveloperArtifactFormat, DeveloperJobState,
    DeveloperTarget, DeveloperToolchainState, DisplayOutputBackend, DisplayOutputState,
    DisplayPixelFormat, LogDomain, LogEvent, LogSeverity, ManagerAvailability, ManagerServicePhase,
    ManagerStartupMode, NetworkConfigMode, NetworkConfigState, NetworkSocketKind,
    NetworkSocketState, PackageChannel, PackageMaintenanceAction, PackageRepositorySyncState,
    PackageRepositoryTrustMode, PackageRing, PackageTrustState, PacketInterfaceBackend,
    PacketInterfaceLinkState, PermissionPolicyState, RuntimeEnvState, RuntimeKind, RuntimeRunState,
    RuntimeWorkloadKind, SecurityAuditKind, ServiceId, SessionInputSource, WifiLinkState,
    WifiSecurity,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogRecord {
    pub sequence: u64,
    pub tick: u64,
    pub source: ServiceId,
    pub severity: LogSeverity,
    pub domain: LogDomain,
    pub event: LogEvent,
    pub arg0: u64,
    pub arg1: u64,
    pub arg2: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManagerServiceInfo {
    pub service_id: ServiceId,
    pub phase: ManagerServicePhase,
    pub attempts: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManagerServiceStatusInfo {
    pub status: crate::ManagerStatus,
    pub phase: ManagerServicePhase,
    pub attempts: u32,
    pub last_exit: u64,
    pub startup: ManagerStartupMode,
    pub availability: ManagerAvailability,
    pub blocked_dependency: ServiceId,
    pub last_start_tick: u64,
    pub last_ready_tick: u64,
    pub next_restart_tick: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManagerServiceTemplateInfo {
    pub startup: ManagerStartupMode,
    pub availability: ManagerAvailability,
    pub ready_timeout_ticks: u32,
    pub restart_limit: u32,
    pub restart_backoff_ticks: u32,
    pub grant_count: u32,
    pub lookup_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManagerGraphStatusInfo {
    pub degraded_boot: bool,
    pub blocked_services: u32,
    pub degraded_services: u32,
    pub service_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManagerServiceLookupInfo {
    pub target: ServiceId,
    pub rights: u64,
    pub policy: crate::ManagerLookupPolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StatusServiceInfo {
    pub service_id: ServiceId,
    pub phase: crate::ManagerServicePhase,
    pub health: crate::StatusHealth,
    pub detail_kind: u32,
    pub detail0: u64,
    pub detail1: u64,
    pub updated_tick: u64,
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
pub struct PackageCatalogEntry {
    pub service_id: ServiceId,
    pub repo_index: u32,
    pub installed: bool,
    pub active: bool,
    pub rollback_available: bool,
    pub category_len: usize,
    pub summary_len: usize,
    pub latest_version_len: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageRepositoryInfo {
    pub repo_index: u32,
    pub package_count: u32,
    pub trust_mode: PackageRepositoryTrustMode,
    pub sync_state: PackageRepositorySyncState,
    pub channel: PackageChannel,
    pub ring: PackageRing,
    pub enabled: bool,
    pub pinned_digest: u64,
    pub last_digest: u64,
    pub name_len: usize,
    pub url_len: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageRepositorySyncInfo {
    pub synced: u32,
    pub failed: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageProvenanceInfo {
    pub repo_index: u32,
    pub trust_state: PackageTrustState,
    pub signed_key_fingerprint: u64,
    pub channel: PackageChannel,
    pub ring: PackageRing,
    pub installed: bool,
    pub active: bool,
    pub rollback_available: bool,
    pub installed_version_len: usize,
    pub active_version_len: usize,
    pub rollback_version_len: usize,
    pub latest_version_len: usize,
    pub source_len: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackagePolicyInfo {
    pub channel: PackageChannel,
    pub ring: PackageRing,
    pub pinned_version_len: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageMaintenanceInfo {
    pub action: PackageMaintenanceAction,
    pub repaired_entries: u32,
    pub garbage_collected_entries: u32,
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
    /// Resolver cache hits; zero from services predating the trailing word.
    pub resolver_hits: u32,
    /// Resolver cache misses; zero from services predating the trailing word.
    pub resolver_misses: u32,
}

/// Aggregate of one DiagPingStatsRequest: N sequential ICMP probes folded
/// into min/max/avg/jitter plus permil loss.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkDiagPingStats {
    pub resolved_address: u32,
    pub sent: u32,
    pub received: u32,
    pub min_ms: u64,
    pub max_ms: u64,
    pub avg_ms: u64,
    pub jitter_ms: u64,
    pub loss_permil: u64,
}

/// One ARP-snooped neighbor from the interface-observed table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkNeighborEntry {
    pub address: u32,
    pub mac: [u8; 6],
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkListenPortKind {
    Unknown = 0,
    TcpListener = 1,
    UdpClient = 2,
    UdpInternal = 3,
}

/// One locally bound port from the network-service self port-scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkListenPort {
    pub kind: NetworkListenPortKind,
    pub port: u16,
}

/// One discovery beacon peer announced within the requested window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkDiscoveryPeer {
    pub address: u32,
    pub name_len: usize,
    pub name: [u8; 15],
    pub age_ms: u32,
}

/// One wireless scan result as decoded by network-service (backend-absent
/// boots never populate these; the shape pins the pure scan-record fields).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkWifiScanEntry {
    pub bssid: [u8; 6],
    pub channel: u8,
    pub rssi: i8,
    pub ssid_len: usize,
    pub ssid: [u8; 32],
    pub security: WifiSecurity,
}

/// One remembered wireless network. PSK octets never leave network-service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkWifiSavedNetwork {
    pub ssid_len: usize,
    pub ssid: [u8; 32],
    pub priority: u8,
}

/// Wireless status echo from WifiStatusRequest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkWifiStatus {
    pub link_state: WifiLinkState,
    /// True only when a WirelessBackend device is registered with the kernel;
    /// always false today.
    pub backend_present: bool,
    pub ssid_len: usize,
    pub ssid: [u8; 32],
}

/// Read-only firewall snapshot from FirewallRulesGetRequest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkFirewallSummary {
    pub rule_count: u32,
    pub default_inbound_allow: bool,
    pub inbound_denied_total: u32,
    pub outbound_denied_total: u32,
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
pub struct SecurityAppPolicyInfo {
    pub image_id: crate::ServiceImageId,
    pub permissions: u32,
    pub policy: PermissionPolicyState,
    pub sensitive_permissions: u32,
    pub name_len: u32,
    pub name: [u8; 64],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SecurityAuditInfo {
    pub sequence: u32,
    pub kind: SecurityAuditKind,
    pub subject_image_id: crate::ServiceImageId,
    pub policy: PermissionPolicyState,
    pub detail: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeAuditInfo {
    pub sequence: u32,
    pub kind: SecurityAuditKind,
    pub env_id: u32,
    pub capabilities: u32,
    pub detail: u64,
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
    pub attached_buffer_count: u32,
    pub active_buffer_slot: Option<u32>,
    pub active_buffer_width: u32,
    pub active_buffer_height: u32,
    pub active_buffer_stride_pixels: u32,
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
    pub active_workspace: u32,
    pub workspace_count: u32,
    pub notification_count: u32,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DesktopNotificationInfo {
    pub sequence: u32,
    pub source_app: Option<DesktopAppId>,
    pub actionable: bool,
    pub text_len: u32,
    pub text: [u8; 64],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DesktopWorkspaceInfo {
    pub active_workspace: u32,
    pub workspace_count: u32,
    pub focused_surface: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClipboardHistoryEntry {
    pub index: u32,
    pub active: bool,
    pub len: u32,
    pub bytes: [u8; 64],
}

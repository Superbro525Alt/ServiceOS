use crate::{
    AudioEndpointBackend, AudioEndpointDirection, AudioEndpointState, AudioStatus,
    AudioStreamDirection, AudioStreamState, DesktopAppId, DesktopDragMode, DesktopStatus,
    DeveloperArtifactFormat, DeveloperJobState, DeveloperStatus, DeveloperTarget,
    DeveloperToolchainState, DisplayOutputBackend, DisplayOutputState, DisplayPixelFormat, Error,
    GraphicsStatus, LogDomain, LogEvent, LogSeverity, ManagerAvailability, ManagerLookupPolicy,
    ManagerServicePhase, ManagerStartupMode, ManagerStatus, NetworkConfigMode, NetworkConfigState,
    NetworkSocketKind, NetworkSocketState, NetworkStatus, PackageStatus, PacketInterfaceBackend,
    PacketInterfaceLinkState, RuntimeEnvState, RuntimeKind, RuntimeRunState, RuntimeStatus,
    RuntimeWorkloadKind, SecurityAuditKind, ServiceId, SessionInputSource, SessionStatus,
};

pub(crate) fn service_id_from_word(value: u64) -> ServiceId {
    match value as u32 {
        x if x == ServiceId::Storage as u32 => ServiceId::Storage,
        x if x == ServiceId::Console as u32 => ServiceId::Console,
        x if x == ServiceId::Config as u32 => ServiceId::Config,
        x if x == ServiceId::Log as u32 => ServiceId::Log,
        x if x == ServiceId::Status as u32 => ServiceId::Status,
        x if x == ServiceId::Shell as u32 => ServiceId::Shell,
        x if x == ServiceId::Package as u32 => ServiceId::Package,
        x if x == ServiceId::Announce as u32 => ServiceId::Announce,
        x if x == ServiceId::Network as u32 => ServiceId::Network,
        x if x == ServiceId::Graphics as u32 => ServiceId::Graphics,
        x if x == ServiceId::Session as u32 => ServiceId::Session,
        x if x == ServiceId::DesktopShell as u32 => ServiceId::DesktopShell,
        x if x == ServiceId::Terminal as u32 => ServiceId::Terminal,
        x if x == ServiceId::Audio as u32 => ServiceId::Audio,
        x if x == ServiceId::Runtime as u32 => ServiceId::Runtime,
        x if x == ServiceId::Developer as u32 => ServiceId::Developer,
        x if x == ServiceId::Clipboard as u32 => ServiceId::Clipboard,
        x if x == ServiceId::Security as u32 => ServiceId::Security,
        _ => ServiceId::RootManager,
    }
}

pub(crate) fn severity_from_word(value: u64) -> LogSeverity {
    match value as u32 {
        x if x == LogSeverity::Trace as u32 => LogSeverity::Trace,
        x if x == LogSeverity::Debug as u32 => LogSeverity::Debug,
        x if x == LogSeverity::Warn as u32 => LogSeverity::Warn,
        x if x == LogSeverity::Error as u32 => LogSeverity::Error,
        _ => LogSeverity::Info,
    }
}

pub(crate) fn domain_from_word(value: u64) -> LogDomain {
    match value as u32 {
        x if x == LogDomain::Bootstrap as u32 => LogDomain::Bootstrap,
        x if x == LogDomain::ServiceManager as u32 => LogDomain::ServiceManager,
        x if x == LogDomain::Storage as u32 => LogDomain::Storage,
        x if x == LogDomain::Log as u32 => LogDomain::Log,
        x if x == LogDomain::Config as u32 => LogDomain::Config,
        x if x == LogDomain::Console as u32 => LogDomain::Console,
        x if x == LogDomain::Status as u32 => LogDomain::Status,
        x if x == LogDomain::Ipc as u32 => LogDomain::Ipc,
        x if x == LogDomain::Shell as u32 => LogDomain::Shell,
        x if x == LogDomain::Package as u32 => LogDomain::Package,
        x if x == LogDomain::Network as u32 => LogDomain::Network,
        x if x == LogDomain::Graphics as u32 => LogDomain::Graphics,
        x if x == LogDomain::Session as u32 => LogDomain::Session,
        x if x == LogDomain::Desktop as u32 => LogDomain::Desktop,
        x if x == LogDomain::App as u32 => LogDomain::App,
        x if x == LogDomain::Audio as u32 => LogDomain::Audio,
        x if x == LogDomain::Runtime as u32 => LogDomain::Runtime,
        x if x == LogDomain::Developer as u32 => LogDomain::Developer,
        x if x == LogDomain::Security as u32 => LogDomain::Security,
        x if x == LogDomain::Kernel as u32 => LogDomain::Kernel,
        _ => LogDomain::Service,
    }
}

pub(crate) fn event_from_word(value: u64) -> LogEvent {
    match value as u32 {
        x if x == LogEvent::ServiceStarted as u32 => LogEvent::ServiceStarted,
        x if x == LogEvent::ServiceReady as u32 => LogEvent::ServiceReady,
        x if x == LogEvent::ServiceFailed as u32 => LogEvent::ServiceFailed,
        x if x == LogEvent::ServiceRestarting as u32 => LogEvent::ServiceRestarting,
        x if x == LogEvent::ConfigLoaded as u32 => LogEvent::ConfigLoaded,
        x if x == LogEvent::ConfigRead as u32 => LogEvent::ConfigRead,
        x if x == LogEvent::ConsoleWrite as u32 => LogEvent::ConsoleWrite,
        x if x == LogEvent::StatusStarted as u32 => LogEvent::StatusStarted,
        x if x == LogEvent::StatusHeartbeat as u32 => LogEvent::StatusHeartbeat,
        x if x == LogEvent::StorageMounted as u32 => LogEvent::StorageMounted,
        x if x == LogEvent::ManifestLoaded as u32 => LogEvent::ManifestLoaded,
        x if x == LogEvent::ResourceOpened as u32 => LogEvent::ResourceOpened,
        x if x == LogEvent::SessionOpened as u32 => LogEvent::SessionOpened,
        x if x == LogEvent::ShellCommand as u32 => LogEvent::ShellCommand,
        x if x == LogEvent::ToolLaunched as u32 => LogEvent::ToolLaunched,
        x if x == LogEvent::PackageCatalogLoaded as u32 => LogEvent::PackageCatalogLoaded,
        x if x == LogEvent::PackageInstalled as u32 => LogEvent::PackageInstalled,
        x if x == LogEvent::PackageUpdated as u32 => LogEvent::PackageUpdated,
        x if x == LogEvent::PackageRemoved as u32 => LogEvent::PackageRemoved,
        x if x == LogEvent::PackageRolledBack as u32 => LogEvent::PackageRolledBack,
        x if x == LogEvent::PackageActivationFailed as u32 => LogEvent::PackageActivationFailed,
        x if x == LogEvent::NetworkInterfaceReady as u32 => LogEvent::NetworkInterfaceReady,
        x if x == LogEvent::NetworkAddressConfigured as u32 => LogEvent::NetworkAddressConfigured,
        x if x == LogEvent::NetworkResolveCompleted as u32 => LogEvent::NetworkResolveCompleted,
        x if x == LogEvent::NetworkProbeCompleted as u32 => LogEvent::NetworkProbeCompleted,
        x if x == LogEvent::NetworkLinkChanged as u32 => LogEvent::NetworkLinkChanged,
        x if x == LogEvent::DisplayOutputReady as u32 => LogEvent::DisplayOutputReady,
        x if x == LogEvent::SurfaceCreated as u32 => LogEvent::SurfaceCreated,
        x if x == LogEvent::SurfaceUpdated as u32 => LogEvent::SurfaceUpdated,
        x if x == LogEvent::CompositorPresented as u32 => LogEvent::CompositorPresented,
        x if x == LogEvent::SessionReady as u32 => LogEvent::SessionReady,
        x if x == LogEvent::SessionFocusChanged as u32 => LogEvent::SessionFocusChanged,
        x if x == LogEvent::DesktopReady as u32 => LogEvent::DesktopReady,
        x if x == LogEvent::DesktopAppLaunched as u32 => LogEvent::DesktopAppLaunched,
        x if x == LogEvent::DesktopAppExited as u32 => LogEvent::DesktopAppExited,
        x if x == LogEvent::DesktopFocusChanged as u32 => LogEvent::DesktopFocusChanged,
        x if x == LogEvent::AppRendered as u32 => LogEvent::AppRendered,
        x if x == LogEvent::InputSourceReady as u32 => LogEvent::InputSourceReady,
        x if x == LogEvent::InputKeyDelivered as u32 => LogEvent::InputKeyDelivered,
        x if x == LogEvent::NetworkLeaseChanged as u32 => LogEvent::NetworkLeaseChanged,
        x if x == LogEvent::NetworkSocketOpened as u32 => LogEvent::NetworkSocketOpened,
        x if x == LogEvent::NetworkSocketClosed as u32 => LogEvent::NetworkSocketClosed,
        x if x == LogEvent::TerminalSessionOpened as u32 => LogEvent::TerminalSessionOpened,
        x if x == LogEvent::TerminalSessionClosed as u32 => LogEvent::TerminalSessionClosed,
        x if x == LogEvent::AudioEndpointReady as u32 => LogEvent::AudioEndpointReady,
        x if x == LogEvent::AudioStreamOpened as u32 => LogEvent::AudioStreamOpened,
        x if x == LogEvent::AudioStreamStarted as u32 => LogEvent::AudioStreamStarted,
        x if x == LogEvent::AudioStreamStopped as u32 => LogEvent::AudioStreamStopped,
        x if x == LogEvent::AudioStreamClosed as u32 => LogEvent::AudioStreamClosed,
        x if x == LogEvent::RuntimeEnvironmentCreated as u32 => LogEvent::RuntimeEnvironmentCreated,
        x if x == LogEvent::RuntimeEnvironmentDestroyed as u32 => {
            LogEvent::RuntimeEnvironmentDestroyed
        }
        x if x == LogEvent::RuntimeLaunchStarted as u32 => LogEvent::RuntimeLaunchStarted,
        x if x == LogEvent::RuntimeLaunchExited as u32 => LogEvent::RuntimeLaunchExited,
        x if x == LogEvent::RuntimeMappedRead as u32 => LogEvent::RuntimeMappedRead,
        x if x == LogEvent::DeveloperCatalogLoaded as u32 => LogEvent::DeveloperCatalogLoaded,
        x if x == LogEvent::DeveloperBuildStarted as u32 => LogEvent::DeveloperBuildStarted,
        x if x == LogEvent::DeveloperBuildFinished as u32 => LogEvent::DeveloperBuildFinished,
        x if x == LogEvent::DeveloperBuildFailed as u32 => LogEvent::DeveloperBuildFailed,
        x if x == LogEvent::DeveloperArtifactOpened as u32 => LogEvent::DeveloperArtifactOpened,
        x if x == LogEvent::SecurityPolicyChanged as u32 => LogEvent::SecurityPolicyChanged,
        x if x == LogEvent::SecurityLaunchDenied as u32 => LogEvent::SecurityLaunchDenied,
        x if x == LogEvent::RuntimeApprovalPending as u32 => LogEvent::RuntimeApprovalPending,
        x if x == LogEvent::RuntimeApprovalChanged as u32 => LogEvent::RuntimeApprovalChanged,
        x if x == LogEvent::KernelTrap as u32 => LogEvent::KernelTrap,
        _ => LogEvent::LookupGranted,
    }
}

pub(crate) fn manager_phase_from_word(value: u64) -> ManagerServicePhase {
    match value as u32 {
        x if x == ManagerServicePhase::Dormant as u32 => ManagerServicePhase::Dormant,
        x if x == ManagerServicePhase::WaitingDependencies as u32 => {
            ManagerServicePhase::WaitingDependencies
        }
        x if x == ManagerServicePhase::Starting as u32 => ManagerServicePhase::Starting,
        x if x == ManagerServicePhase::Ready as u32 => ManagerServicePhase::Ready,
        x if x == ManagerServicePhase::Backoff as u32 => ManagerServicePhase::Backoff,
        x if x == ManagerServicePhase::Degraded as u32 => ManagerServicePhase::Degraded,
        _ => ManagerServicePhase::Exited,
    }
}

pub(crate) fn manager_status_from_word(value: u64) -> ManagerStatus {
    match value as u32 {
        x if x == ManagerStatus::Ok as u32 => ManagerStatus::Ok,
        x if x == ManagerStatus::Denied as u32 => ManagerStatus::Denied,
        x if x == ManagerStatus::NotFound as u32 => ManagerStatus::NotFound,
        x if x == ManagerStatus::Busy as u32 => ManagerStatus::Busy,
        x if x == ManagerStatus::Failed as u32 => ManagerStatus::Failed,
        _ => ManagerStatus::Busy,
    }
}

pub(crate) fn manager_startup_from_word(value: u64) -> ManagerStartupMode {
    match value as u32 {
        x if x == ManagerStartupMode::OnDemand as u32 => ManagerStartupMode::OnDemand,
        _ => ManagerStartupMode::Eager,
    }
}

pub(crate) fn manager_availability_from_word(value: u64) -> ManagerAvailability {
    match value as u32 {
        x if x == ManagerAvailability::Optional as u32 => ManagerAvailability::Optional,
        _ => ManagerAvailability::Required,
    }
}

pub(crate) fn manager_lookup_policy_from_word(value: u64) -> ManagerLookupPolicy {
    match value as u32 {
        x if x == ManagerLookupPolicy::Revoked as u32 => ManagerLookupPolicy::Revoked,
        _ => ManagerLookupPolicy::Default,
    }
}

pub(crate) fn security_audit_kind_from_word(value: u64) -> SecurityAuditKind {
    match value as u32 {
        x if x == SecurityAuditKind::LaunchDenied as u32 => SecurityAuditKind::LaunchDenied,
        x if x == SecurityAuditKind::RuntimeApprovalRequested as u32 => {
            SecurityAuditKind::RuntimeApprovalRequested
        }
        x if x == SecurityAuditKind::RuntimeApprovalChanged as u32 => {
            SecurityAuditKind::RuntimeApprovalChanged
        }
        _ => SecurityAuditKind::PolicyChanged,
    }
}

pub(crate) fn package_status_from_word(value: u64) -> PackageStatus {
    match value as u32 {
        x if x == PackageStatus::Ok as u32 => PackageStatus::Ok,
        x if x == PackageStatus::NotFound as u32 => PackageStatus::NotFound,
        x if x == PackageStatus::AlreadyInstalled as u32 => PackageStatus::AlreadyInstalled,
        x if x == PackageStatus::NotInstalled as u32 => PackageStatus::NotInstalled,
        x if x == PackageStatus::Busy as u32 => PackageStatus::Busy,
        x if x == PackageStatus::Denied as u32 => PackageStatus::Denied,
        x if x == PackageStatus::IntegrityFailed as u32 => PackageStatus::IntegrityFailed,
        x if x == PackageStatus::End as u32 => PackageStatus::End,
        x if x == PackageStatus::NoChange as u32 => PackageStatus::NoChange,
        x if x == PackageStatus::NoRollback as u32 => PackageStatus::NoRollback,
        x if x == PackageStatus::Unsupported as u32 => PackageStatus::Unsupported,
        x if x == PackageStatus::Offline as u32 => PackageStatus::Offline,
        x if x == PackageStatus::Interrupted as u32 => PackageStatus::Interrupted,
        x if x == PackageStatus::VerificationFailed as u32 => PackageStatus::VerificationFailed,
        _ => PackageStatus::Busy,
    }
}

pub(crate) fn package_status_error(status: PackageStatus) -> Error {
    match status {
        PackageStatus::NotFound => Error::NotFound,
        PackageStatus::AlreadyInstalled
        | PackageStatus::Busy
        | PackageStatus::NoChange
        | PackageStatus::Offline
        | PackageStatus::Interrupted => Error::Busy,
        PackageStatus::NotInstalled
        | PackageStatus::NoRollback
        | PackageStatus::End
        | PackageStatus::Unsupported => Error::InvalidArgument,
        PackageStatus::Denied => Error::PermissionDenied,
        PackageStatus::IntegrityFailed | PackageStatus::VerificationFailed => Error::InvalidCall,
        PackageStatus::Ok => Error::InvalidArgument,
    }
}

pub(crate) fn network_status_from_word(value: u64) -> NetworkStatus {
    match value as u32 {
        x if x == NetworkStatus::Ok as u32 => NetworkStatus::Ok,
        x if x == NetworkStatus::NotFound as u32 => NetworkStatus::NotFound,
        x if x == NetworkStatus::Busy as u32 => NetworkStatus::Busy,
        x if x == NetworkStatus::InvalidTarget as u32 => NetworkStatus::InvalidTarget,
        x if x == NetworkStatus::Timeout as u32 => NetworkStatus::Timeout,
        x if x == NetworkStatus::End as u32 => NetworkStatus::End,
        x if x == NetworkStatus::Unsupported as u32 => NetworkStatus::Unsupported,
        x if x == NetworkStatus::Denied as u32 => NetworkStatus::Denied,
        x if x == NetworkStatus::CapacityExceeded as u32 => NetworkStatus::CapacityExceeded,
        x if x == NetworkStatus::Closed as u32 => NetworkStatus::Closed,
        _ => NetworkStatus::Busy,
    }
}

pub(crate) fn network_status_error(status: NetworkStatus) -> Error {
    match status {
        NetworkStatus::Ok => Error::InvalidArgument,
        NetworkStatus::NotFound | NetworkStatus::InvalidTarget => Error::NotFound,
        NetworkStatus::Busy => Error::Busy,
        NetworkStatus::Timeout => Error::QueueEmpty,
        NetworkStatus::End => Error::NotFound,
        NetworkStatus::Unsupported => Error::Unsupported,
        NetworkStatus::Denied => Error::PermissionDenied,
        NetworkStatus::CapacityExceeded => Error::CapacityExceeded,
        NetworkStatus::Closed => Error::NotFound,
    }
}

pub(crate) fn audio_status_from_word(value: u64) -> AudioStatus {
    match value as u32 {
        x if x == AudioStatus::Ok as u32 => AudioStatus::Ok,
        x if x == AudioStatus::NotFound as u32 => AudioStatus::NotFound,
        x if x == AudioStatus::Busy as u32 => AudioStatus::Busy,
        x if x == AudioStatus::Unsupported as u32 => AudioStatus::Unsupported,
        x if x == AudioStatus::Denied as u32 => AudioStatus::Denied,
        x if x == AudioStatus::CapacityExceeded as u32 => AudioStatus::CapacityExceeded,
        x if x == AudioStatus::Closed as u32 => AudioStatus::Closed,
        _ => AudioStatus::Busy,
    }
}

pub(crate) fn audio_status_error(status: AudioStatus) -> Error {
    match status {
        AudioStatus::Ok => Error::InvalidArgument,
        AudioStatus::NotFound | AudioStatus::Closed => Error::NotFound,
        AudioStatus::Busy => Error::Busy,
        AudioStatus::Unsupported => Error::Unsupported,
        AudioStatus::Denied => Error::PermissionDenied,
        AudioStatus::CapacityExceeded => Error::CapacityExceeded,
    }
}

pub(crate) fn audio_endpoint_backend_from_word(value: u64) -> AudioEndpointBackend {
    match value as u32 {
        x if x == AudioEndpointBackend::PcSpeaker as u32 => AudioEndpointBackend::PcSpeaker,
        _ => AudioEndpointBackend::Unknown,
    }
}

pub(crate) fn audio_endpoint_direction_from_word(value: u64) -> AudioEndpointDirection {
    match value as u32 {
        x if x == AudioEndpointDirection::Input as u32 => AudioEndpointDirection::Input,
        _ => AudioEndpointDirection::Output,
    }
}

pub(crate) fn audio_endpoint_state_from_word(value: u64) -> AudioEndpointState {
    match value as u32 {
        x if x == AudioEndpointState::Offline as u32 => AudioEndpointState::Offline,
        x if x == AudioEndpointState::Active as u32 => AudioEndpointState::Active,
        _ => AudioEndpointState::Idle,
    }
}

pub(crate) fn audio_stream_direction_from_word(value: u64) -> AudioStreamDirection {
    match value as u32 {
        x if x == AudioStreamDirection::Capture as u32 => AudioStreamDirection::Capture,
        _ => AudioStreamDirection::Playback,
    }
}

pub(crate) fn audio_stream_state_from_word(value: u64) -> AudioStreamState {
    match value as u32 {
        x if x == AudioStreamState::Active as u32 => AudioStreamState::Active,
        x if x == AudioStreamState::Closed as u32 => AudioStreamState::Closed,
        x if x == AudioStreamState::Failed as u32 => AudioStreamState::Failed,
        _ => AudioStreamState::Idle,
    }
}

pub(crate) fn runtime_status_from_word(value: u64) -> RuntimeStatus {
    match value as u32 {
        x if x == RuntimeStatus::Ok as u32 => RuntimeStatus::Ok,
        x if x == RuntimeStatus::NotFound as u32 => RuntimeStatus::NotFound,
        x if x == RuntimeStatus::Busy as u32 => RuntimeStatus::Busy,
        x if x == RuntimeStatus::Denied as u32 => RuntimeStatus::Denied,
        x if x == RuntimeStatus::InvalidPath as u32 => RuntimeStatus::InvalidPath,
        x if x == RuntimeStatus::Unsupported as u32 => RuntimeStatus::Unsupported,
        x if x == RuntimeStatus::Closed as u32 => RuntimeStatus::Closed,
        x if x == RuntimeStatus::PendingApproval as u32 => RuntimeStatus::PendingApproval,
        _ => RuntimeStatus::Busy,
    }
}

pub(crate) fn runtime_status_error(status: RuntimeStatus) -> Error {
    match status {
        RuntimeStatus::Ok => Error::InvalidArgument,
        RuntimeStatus::NotFound | RuntimeStatus::Closed => Error::NotFound,
        RuntimeStatus::Busy => Error::Busy,
        RuntimeStatus::Denied => Error::PermissionDenied,
        RuntimeStatus::InvalidPath => Error::InvalidArgument,
        RuntimeStatus::Unsupported => Error::Unsupported,
        RuntimeStatus::PendingApproval => Error::Busy,
    }
}

pub(crate) fn runtime_kind_from_word(value: u64) -> RuntimeKind {
    match value as u32 {
        x if x == RuntimeKind::Windows as u32 => RuntimeKind::Windows,
        _ => RuntimeKind::Posix,
    }
}

pub(crate) fn runtime_env_state_from_word(value: u64) -> RuntimeEnvState {
    match value as u32 {
        x if x == RuntimeEnvState::Busy as u32 => RuntimeEnvState::Busy,
        x if x == RuntimeEnvState::Destroyed as u32 => RuntimeEnvState::Destroyed,
        x if x == RuntimeEnvState::PendingApproval as u32 => RuntimeEnvState::PendingApproval,
        x if x == RuntimeEnvState::Denied as u32 => RuntimeEnvState::Denied,
        _ => RuntimeEnvState::Ready,
    }
}

pub(crate) fn runtime_run_state_from_word(value: u64) -> RuntimeRunState {
    match value as u32 {
        x if x == RuntimeRunState::Running as u32 => RuntimeRunState::Running,
        x if x == RuntimeRunState::Exited as u32 => RuntimeRunState::Exited,
        x if x == RuntimeRunState::Failed as u32 => RuntimeRunState::Failed,
        _ => RuntimeRunState::Launching,
    }
}

pub(crate) fn runtime_workload_kind_from_word(value: u64) -> RuntimeWorkloadKind {
    match value as u32 {
        x if x == RuntimeWorkloadKind::Env as u32 => RuntimeWorkloadKind::Env,
        x if x == RuntimeWorkloadKind::Mounts as u32 => RuntimeWorkloadKind::Mounts,
        x if x == RuntimeWorkloadKind::Cat as u32 => RuntimeWorkloadKind::Cat,
        _ => RuntimeWorkloadKind::Inspect,
    }
}

pub(crate) fn developer_status_from_word(value: u64) -> DeveloperStatus {
    match value as u32 {
        x if x == DeveloperStatus::Ok as u32 => DeveloperStatus::Ok,
        x if x == DeveloperStatus::NotFound as u32 => DeveloperStatus::NotFound,
        x if x == DeveloperStatus::Busy as u32 => DeveloperStatus::Busy,
        x if x == DeveloperStatus::Denied as u32 => DeveloperStatus::Denied,
        x if x == DeveloperStatus::Unsupported as u32 => DeveloperStatus::Unsupported,
        _ => DeveloperStatus::Busy,
    }
}

pub(crate) fn developer_status_error(status: DeveloperStatus) -> Error {
    match status {
        DeveloperStatus::Ok => Error::InvalidArgument,
        DeveloperStatus::NotFound => Error::NotFound,
        DeveloperStatus::Busy => Error::Busy,
        DeveloperStatus::Denied => Error::PermissionDenied,
        DeveloperStatus::Unsupported => Error::Unsupported,
    }
}

pub(crate) fn developer_target_from_word(value: u64) -> DeveloperTarget {
    match value as u32 {
        x if x == DeveloperTarget::LinuxX64 as u32 => DeveloperTarget::LinuxX64,
        x if x == DeveloperTarget::WindowsX64 as u32 => DeveloperTarget::WindowsX64,
        x if x == DeveloperTarget::MacosX64 as u32 => DeveloperTarget::MacosX64,
        _ => DeveloperTarget::NativeX64,
    }
}

pub(crate) fn developer_toolchain_state_from_word(value: u64) -> DeveloperToolchainState {
    match value as u32 {
        x if x == DeveloperToolchainState::RemoteOnly as u32 => DeveloperToolchainState::RemoteOnly,
        _ => DeveloperToolchainState::Installed,
    }
}

pub(crate) fn developer_artifact_format_from_word(value: u64) -> DeveloperArtifactFormat {
    match value as u32 {
        x if x == DeveloperArtifactFormat::Elf64 as u32 => DeveloperArtifactFormat::Elf64,
        x if x == DeveloperArtifactFormat::Pe32Plus as u32 => DeveloperArtifactFormat::Pe32Plus,
        x if x == DeveloperArtifactFormat::MachO64 as u32 => DeveloperArtifactFormat::MachO64,
        _ => DeveloperArtifactFormat::ServiceOsFlat,
    }
}

pub(crate) fn developer_job_state_from_word(value: u64) -> DeveloperJobState {
    match value as u32 {
        x if x == DeveloperJobState::Running as u32 => DeveloperJobState::Running,
        x if x == DeveloperJobState::Succeeded as u32 => DeveloperJobState::Succeeded,
        x if x == DeveloperJobState::Failed as u32 => DeveloperJobState::Failed,
        x if x == DeveloperJobState::Unsupported as u32 => DeveloperJobState::Unsupported,
        _ => DeveloperJobState::Queued,
    }
}

pub(crate) fn network_config_mode_from_word(value: u64) -> NetworkConfigMode {
    match value as u32 {
        x if x == NetworkConfigMode::Dynamic as u32 => NetworkConfigMode::Dynamic,
        _ => NetworkConfigMode::Static,
    }
}

pub(crate) fn network_config_state_from_word(value: u64) -> NetworkConfigState {
    match value as u32 {
        x if x == NetworkConfigState::Pending as u32 => NetworkConfigState::Pending,
        x if x == NetworkConfigState::FallbackStatic as u32 => NetworkConfigState::FallbackStatic,
        x if x == NetworkConfigState::Failed as u32 => NetworkConfigState::Failed,
        _ => NetworkConfigState::Configured,
    }
}

pub(crate) fn network_socket_kind_from_word(_value: u64) -> NetworkSocketKind {
    NetworkSocketKind::TcpStream
}

pub(crate) fn network_socket_state_from_word(value: u64) -> NetworkSocketState {
    match value as u32 {
        x if x == NetworkSocketState::Connecting as u32 => NetworkSocketState::Connecting,
        x if x == NetworkSocketState::Established as u32 => NetworkSocketState::Established,
        x if x == NetworkSocketState::Closing as u32 => NetworkSocketState::Closing,
        x if x == NetworkSocketState::Failed as u32 => NetworkSocketState::Failed,
        _ => NetworkSocketState::Closed,
    }
}

pub(crate) fn graphics_status_from_word(value: u64) -> GraphicsStatus {
    match value as u32 {
        x if x == GraphicsStatus::Ok as u32 => GraphicsStatus::Ok,
        x if x == GraphicsStatus::NotFound as u32 => GraphicsStatus::NotFound,
        x if x == GraphicsStatus::Busy as u32 => GraphicsStatus::Busy,
        x if x == GraphicsStatus::Denied as u32 => GraphicsStatus::Denied,
        x if x == GraphicsStatus::CapacityExceeded as u32 => GraphicsStatus::CapacityExceeded,
        _ => GraphicsStatus::Busy,
    }
}

pub(crate) fn graphics_status_error(status: GraphicsStatus) -> Error {
    match status {
        GraphicsStatus::Ok => Error::InvalidArgument,
        GraphicsStatus::NotFound => Error::NotFound,
        GraphicsStatus::Busy => Error::Busy,
        GraphicsStatus::Denied => Error::PermissionDenied,
        GraphicsStatus::CapacityExceeded => Error::CapacityExceeded,
    }
}

pub(crate) fn session_status_from_word(value: u64) -> SessionStatus {
    match value as u32 {
        x if x == SessionStatus::Ok as u32 => SessionStatus::Ok,
        x if x == SessionStatus::NotFound as u32 => SessionStatus::NotFound,
        x if x == SessionStatus::Busy as u32 => SessionStatus::Busy,
        x if x == SessionStatus::Denied as u32 => SessionStatus::Denied,
        _ => SessionStatus::Busy,
    }
}

pub(crate) fn session_status_error(status: SessionStatus) -> Error {
    match status {
        SessionStatus::Ok => Error::InvalidArgument,
        SessionStatus::NotFound => Error::NotFound,
        SessionStatus::Busy => Error::Busy,
        SessionStatus::Denied => Error::PermissionDenied,
    }
}

pub(crate) fn desktop_status_from_word(value: u64) -> DesktopStatus {
    match value as u32 {
        x if x == DesktopStatus::Ok as u32 => DesktopStatus::Ok,
        x if x == DesktopStatus::NotFound as u32 => DesktopStatus::NotFound,
        x if x == DesktopStatus::Busy as u32 => DesktopStatus::Busy,
        x if x == DesktopStatus::Denied as u32 => DesktopStatus::Denied,
        _ => DesktopStatus::Busy,
    }
}

pub(crate) fn desktop_status_error(status: DesktopStatus) -> Error {
    match status {
        DesktopStatus::Ok => Error::InvalidArgument,
        DesktopStatus::NotFound => Error::NotFound,
        DesktopStatus::Busy => Error::Busy,
        DesktopStatus::Denied => Error::PermissionDenied,
    }
}

pub(crate) fn desktop_app_id_from_word(value: u64) -> core::result::Result<DesktopAppId, ()> {
    match value as u32 {
        x if x == DesktopAppId::Settings as u32 => Ok(DesktopAppId::Settings),
        x if x == DesktopAppId::Files as u32 => Ok(DesktopAppId::Files),
        x if x == DesktopAppId::Monitor as u32 => Ok(DesktopAppId::Monitor),
        x if x == DesktopAppId::Terminal as u32 => Ok(DesktopAppId::Terminal),
        x if x == DesktopAppId::SoftwareCenter as u32 => Ok(DesktopAppId::SoftwareCenter),
        _ => Err(()),
    }
}

pub(crate) fn desktop_drag_mode_from_word(value: u64) -> DesktopDragMode {
    match value as u32 {
        x if x == DesktopDragMode::Move as u32 => DesktopDragMode::Move,
        x if x == DesktopDragMode::Resize as u32 => DesktopDragMode::Resize,
        _ => DesktopDragMode::None,
    }
}

pub(crate) fn unpack_i32_pair(value: u64) -> (i32, i32) {
    (value as u32 as i32, (value >> 32) as u32 as i32)
}

pub(crate) fn unpack_u32_pair(value: u64) -> (u32, u32) {
    (value as u32, (value >> 32) as u32)
}

pub(crate) fn display_backend_from_word(value: u64) -> DisplayOutputBackend {
    match value as u32 {
        x if x == DisplayOutputBackend::BootFramebuffer as u32 => {
            DisplayOutputBackend::BootFramebuffer
        }
        _ => DisplayOutputBackend::Unknown,
    }
}

pub(crate) fn display_state_from_word(value: u64) -> DisplayOutputState {
    match value as u32 {
        x if x == DisplayOutputState::Connected as u32 => DisplayOutputState::Connected,
        _ => DisplayOutputState::Disconnected,
    }
}

pub(crate) fn display_pixel_format_from_word(value: u64) -> DisplayPixelFormat {
    match value as u32 {
        x if x == DisplayPixelFormat::Xrgb8888 as u32 => DisplayPixelFormat::Xrgb8888,
        x if x == DisplayPixelFormat::Bgrx8888 as u32 => DisplayPixelFormat::Bgrx8888,
        _ => DisplayPixelFormat::Unknown,
    }
}

pub(crate) fn session_input_source_from_word(value: u64) -> SessionInputSource {
    match value as u32 {
        x if x == SessionInputSource::ServiceControl as u32 => SessionInputSource::ServiceControl,
        _ => SessionInputSource::None,
    }
}

pub(crate) fn packet_backend_from_word(value: u64) -> PacketInterfaceBackend {
    match value as u32 {
        x if x == PacketInterfaceBackend::VirtioPci as u32 => PacketInterfaceBackend::VirtioPci,
        _ => PacketInterfaceBackend::Unknown,
    }
}

pub(crate) fn packet_link_state_from_word(value: u64) -> PacketInterfaceLinkState {
    match value as u32 {
        x if x == PacketInterfaceLinkState::Up as u32 => PacketInterfaceLinkState::Up,
        _ => PacketInterfaceLinkState::Down,
    }
}

pub(crate) fn unpack_mac(word: u64) -> [u8; 6] {
    [
        (word & 0xff) as u8,
        ((word >> 8) & 0xff) as u8,
        ((word >> 16) & 0xff) as u8,
        ((word >> 24) & 0xff) as u8,
        ((word >> 32) & 0xff) as u8,
        ((word >> 40) & 0xff) as u8,
    ]
}

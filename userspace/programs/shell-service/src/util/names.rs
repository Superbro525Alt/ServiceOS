use serviceos_userspace_runtime as rt;
use rt::{
    ConfigKey, DesktopAppId, DesktopDragMode, LogDomain, LogEvent, LogSeverity,
    ManagerServicePhase, ServiceId,
};

pub(crate) fn parse_service_name(name: &str) -> Option<ServiceId> {
    match name {
        "root-manager" => Some(ServiceId::RootManager),
        "storage" | "storage-service" => Some(ServiceId::Storage),
        "console" | "console-service" => Some(ServiceId::Console),
        "config" | "config-service" => Some(ServiceId::Config),
        "log" | "log-service" => Some(ServiceId::Log),
        "status" | "status-service" => Some(ServiceId::Status),
        "shell" | "shell-service" => Some(ServiceId::Shell),
        "package" | "package-service" => Some(ServiceId::Package),
        "announce" | "announce-service" => Some(ServiceId::Announce),
        "network" | "network-service" => Some(ServiceId::Network),
        "graphics" | "graphics-service" => Some(ServiceId::Graphics),
        "session" | "session-service" => Some(ServiceId::Session),
        "desktop" | "desktop-shell" | "desktop-shell-service" => Some(ServiceId::DesktopShell),
        "audio" | "audio-service" => Some(ServiceId::Audio),
        "runtime" | "runtime-service" => Some(ServiceId::Runtime),
        "developer" | "developer-service" => Some(ServiceId::Developer),
        "security" | "security-service" => Some(ServiceId::Security),
        _ => None,
    }
}

pub(crate) fn parse_desktop_app_name(name: &str) -> Option<DesktopAppId> {
    match name {
        "settings" => Some(DesktopAppId::Settings),
        "files" => Some(DesktopAppId::Files),
        "monitor" => Some(DesktopAppId::Monitor),
        "terminal" => Some(DesktopAppId::Terminal),
        "software" | "software-center" | "store" => Some(DesktopAppId::SoftwareCenter),
        _ => None,
    }
}

pub(crate) fn service_name(service_id: ServiceId) -> &'static str {
    match service_id {
        ServiceId::RootManager => "root-manager",
        ServiceId::Storage => "storage-service",
        ServiceId::Console => "console-service",
        ServiceId::Config => "config-service",
        ServiceId::Log => "log-service",
        ServiceId::Status => "status-service",
        ServiceId::Shell => "shell-service",
        ServiceId::Package => "package-service",
        ServiceId::Announce => "announce-service",
        ServiceId::Network => "network-service",
        ServiceId::Graphics => "graphics-service",
        ServiceId::Session => "session-service",
        ServiceId::DesktopShell => "desktop-shell-service",
        ServiceId::Terminal => "terminal-service",
        ServiceId::Audio => "audio-service",
        ServiceId::Runtime => "runtime-service",
        ServiceId::Developer => "developer-service",
        ServiceId::Clipboard => "clipboard-service",
        ServiceId::Security => "security-service",
    }
}

pub(crate) fn desktop_app_name(app_id: DesktopAppId) -> &'static str {
    match app_id {
        DesktopAppId::Settings => "settings",
        DesktopAppId::Files => "files",
        DesktopAppId::Monitor => "monitor",
        DesktopAppId::Terminal => "terminal",
        DesktopAppId::SoftwareCenter => "software",
    }
}

pub(crate) fn desktop_drag_name(mode: DesktopDragMode) -> &'static str {
    match mode {
        DesktopDragMode::None => "none",
        DesktopDragMode::Move => "move",
        DesktopDragMode::Resize => "resize",
    }
}

pub(crate) fn phase_name(phase: ManagerServicePhase) -> &'static str {
    match phase {
        ManagerServicePhase::Dormant => "dormant",
        ManagerServicePhase::WaitingDependencies => "waiting-deps",
        ManagerServicePhase::Starting => "starting",
        ManagerServicePhase::Ready => "ready",
        ManagerServicePhase::Backoff => "backoff",
        ManagerServicePhase::Degraded => "degraded",
        ManagerServicePhase::Exited => "exited",
    }
}

pub(crate) fn startup_name(startup: rt::ManagerStartupMode) -> &'static str {
    match startup {
        rt::ManagerStartupMode::Eager => "eager",
        rt::ManagerStartupMode::OnDemand => "on-demand",
    }
}

pub(crate) fn availability_name(availability: rt::ManagerAvailability) -> &'static str {
    match availability {
        rt::ManagerAvailability::Required => "required",
        rt::ManagerAvailability::Optional => "optional",
    }
}

pub(crate) fn manager_status_name(status: rt::ManagerStatus) -> &'static str {
    match status {
        rt::ManagerStatus::Ok => "ok",
        rt::ManagerStatus::Denied => "denied",
        rt::ManagerStatus::NotFound => "not-found",
        rt::ManagerStatus::Busy => "busy",
        rt::ManagerStatus::Failed => "failed",
    }
}

pub(crate) fn config_key_name(key: ConfigKey) -> &'static str {
    match key {
        ConfigKey::LogMinimumSeverity => "log.minimum_severity",
        ConfigKey::StatusHeartbeatTicks => "status.heartbeat_ticks",
        ConfigKey::StatusConsoleMirror => "status.console_mirror",
        ConfigKey::StatusHeartbeatLogPeriod => "status.heartbeat_log_period",
        ConfigKey::NetworkIpv4Address => "network.ipv4_address",
        ConfigKey::NetworkIpv4PrefixLength => "network.ipv4_prefix_length",
        ConfigKey::NetworkIpv4Gateway => "network.ipv4_gateway",
        ConfigKey::NetworkProbeTimeoutTicks => "network.probe_timeout_ticks",
        ConfigKey::NetworkDynamicIpv4 => "network.dynamic_ipv4",
        ConfigKey::NetworkDnsServer => "network.dns_server",
        ConfigKey::NetworkDnsQueryTimeoutTicks => "network.dns_query_timeout_ticks",
        ConfigKey::NetworkDhcpAcquireTimeoutTicks => "network.dhcp_acquire_timeout_ticks",
        ConfigKey::NetworkTcpConnectTimeoutTicks => "network.tcp_connect_timeout_ticks",
        ConfigKey::NetworkTcpIdleTimeoutTicks => "network.tcp_idle_timeout_ticks",
    }
}

pub(crate) fn severity_name(severity: LogSeverity) -> &'static str {
    match severity {
        LogSeverity::Trace => "trace",
        LogSeverity::Debug => "debug",
        LogSeverity::Info => "info",
        LogSeverity::Warn => "warn",
        LogSeverity::Error => "error",
    }
}

pub(crate) fn domain_name(domain: LogDomain) -> &'static str {
    match domain {
        LogDomain::Bootstrap => "bootstrap",
        LogDomain::ServiceManager => "service-manager",
        LogDomain::Service => "service",
        LogDomain::Storage => "storage",
        LogDomain::Log => "log",
        LogDomain::Config => "config",
        LogDomain::Console => "console",
        LogDomain::Status => "status",
        LogDomain::Ipc => "ipc",
        LogDomain::Shell => "shell",
        LogDomain::Package => "package",
        LogDomain::Network => "network",
        LogDomain::Graphics => "graphics",
        LogDomain::Session => "session",
        LogDomain::Desktop => "desktop",
        LogDomain::App => "app",
        LogDomain::Audio => "audio",
        LogDomain::Runtime => "runtime",
        LogDomain::Developer => "developer",
        LogDomain::Security => "security",
        LogDomain::Kernel => "kernel",
    }
}

pub(crate) fn event_name(event: LogEvent) -> &'static str {
    match event {
        LogEvent::ServiceStarted => "service-started",
        LogEvent::ServiceReady => "service-ready",
        LogEvent::ServiceFailed => "service-failed",
        LogEvent::ServiceRestarting => "service-restarting",
        LogEvent::ConfigLoaded => "config-loaded",
        LogEvent::ConfigRead => "config-read",
        LogEvent::ConsoleWrite => "console-write",
        LogEvent::StatusStarted => "status-started",
        LogEvent::StatusHeartbeat => "status-heartbeat",
        LogEvent::LookupGranted => "lookup-granted",
        LogEvent::StorageMounted => "storage-mounted",
        LogEvent::ManifestLoaded => "manifest-loaded",
        LogEvent::ResourceOpened => "resource-opened",
        LogEvent::SessionOpened => "session-opened",
        LogEvent::ShellCommand => "shell-command",
        LogEvent::ToolLaunched => "tool-launched",
        LogEvent::PackageCatalogLoaded => "package-catalog-loaded",
        LogEvent::PackageInstalled => "package-installed",
        LogEvent::PackageUpdated => "package-updated",
        LogEvent::PackageRemoved => "package-removed",
        LogEvent::PackageRolledBack => "package-rolled-back",
        LogEvent::PackageActivationFailed => "package-activation-failed",
        LogEvent::PackageRepositoryAdded => "package-repository-added",
        LogEvent::PackageRepositorySynced => "package-repository-synced",
        LogEvent::PackageRepositorySyncFailed => "package-repository-sync-failed",
        LogEvent::PackageRepairCompleted => "package-repair-completed",
        LogEvent::PackageGarbageCollected => "package-garbage-collected",
        LogEvent::NetworkInterfaceReady => "network-interface-ready",
        LogEvent::NetworkAddressConfigured => "network-address-configured",
        LogEvent::NetworkResolveCompleted => "network-resolve-completed",
        LogEvent::NetworkProbeCompleted => "network-probe-completed",
        LogEvent::NetworkLinkChanged => "network-link-changed",
        LogEvent::NetworkLeaseChanged => "network-lease-changed",
        LogEvent::NetworkSocketOpened => "network-socket-opened",
        LogEvent::NetworkSocketClosed => "network-socket-closed",
        LogEvent::DisplayOutputReady => "display-output-ready",
        LogEvent::SurfaceCreated => "surface-created",
        LogEvent::SurfaceUpdated => "surface-updated",
        LogEvent::CompositorPresented => "compositor-presented",
        LogEvent::SessionReady => "session-ready",
        LogEvent::SessionFocusChanged => "session-focus-changed",
        LogEvent::DesktopReady => "desktop-ready",
        LogEvent::DesktopAppLaunched => "desktop-app-launched",
        LogEvent::DesktopAppExited => "desktop-app-exited",
        LogEvent::DesktopFocusChanged => "desktop-focus-changed",
        LogEvent::AppRendered => "app-rendered",
        LogEvent::InputSourceReady => "input-source-ready",
        LogEvent::InputKeyDelivered => "input-key-delivered",
        LogEvent::TerminalSessionOpened => "terminal-session-opened",
        LogEvent::TerminalSessionClosed => "terminal-session-closed",
        LogEvent::AudioEndpointReady => "audio-endpoint-ready",
        LogEvent::AudioStreamOpened => "audio-stream-opened",
        LogEvent::AudioStreamStarted => "audio-stream-started",
        LogEvent::AudioStreamStopped => "audio-stream-stopped",
        LogEvent::AudioStreamClosed => "audio-stream-closed",
        LogEvent::RuntimeEnvironmentCreated => "runtime-environment-created",
        LogEvent::RuntimeEnvironmentDestroyed => "runtime-environment-destroyed",
        LogEvent::RuntimeLaunchStarted => "runtime-launch-started",
        LogEvent::RuntimeLaunchExited => "runtime-launch-exited",
        LogEvent::RuntimeMappedRead => "runtime-mapped-read",
        LogEvent::DeveloperCatalogLoaded => "developer-catalog-loaded",
        LogEvent::DeveloperBuildStarted => "developer-build-started",
        LogEvent::DeveloperBuildFinished => "developer-build-finished",
        LogEvent::DeveloperBuildFailed => "developer-build-failed",
        LogEvent::DeveloperArtifactOpened => "developer-artifact-opened",
        LogEvent::SecurityPolicyChanged => "security-policy-changed",
        LogEvent::SecurityLaunchDenied => "security-launch-denied",
        LogEvent::RuntimeApprovalPending => "runtime-approval-pending",
        LogEvent::RuntimeApprovalChanged => "runtime-approval-changed",
        LogEvent::KernelTrap => "kernel-trap",
    }
}

pub(crate) fn link_state_name(state: rt::PacketInterfaceLinkState) -> &'static str {
    match state {
        rt::PacketInterfaceLinkState::Up => "up",
        rt::PacketInterfaceLinkState::Down => "down",
    }
}

pub(crate) fn network_config_mode_name(mode: rt::NetworkConfigMode) -> &'static str {
    match mode {
        rt::NetworkConfigMode::Static => "static",
        rt::NetworkConfigMode::Dynamic => "dynamic",
    }
}

pub(crate) fn network_config_state_name(state: rt::NetworkConfigState) -> &'static str {
    match state {
        rt::NetworkConfigState::Pending => "pending",
        rt::NetworkConfigState::Configured => "configured",
        rt::NetworkConfigState::FallbackStatic => "fallback-static",
        rt::NetworkConfigState::Failed => "failed",
    }
}

pub(crate) fn network_socket_state_name(state: rt::NetworkSocketState) -> &'static str {
    match state {
        rt::NetworkSocketState::Connecting => "connecting",
        rt::NetworkSocketState::Established => "established",
        rt::NetworkSocketState::Closing => "closing",
        rt::NetworkSocketState::Closed => "closed",
        rt::NetworkSocketState::Failed => "failed",
    }
}

pub(crate) fn display_backend_name(backend: rt::DisplayOutputBackend) -> &'static str {
    match backend {
        rt::DisplayOutputBackend::BootFramebuffer => "boot-framebuffer",
        rt::DisplayOutputBackend::Unknown => "unknown",
    }
}

pub(crate) fn display_state_name(state: rt::DisplayOutputState) -> &'static str {
    match state {
        rt::DisplayOutputState::Connected => "connected",
        rt::DisplayOutputState::Disconnected => "disconnected",
    }
}

pub(crate) fn pixel_format_name(format: rt::DisplayPixelFormat) -> &'static str {
    match format {
        rt::DisplayPixelFormat::Xrgb8888 => "xrgb8888",
        rt::DisplayPixelFormat::Bgrx8888 => "bgrx8888",
        rt::DisplayPixelFormat::Unknown => "unknown",
    }
}

pub(crate) fn session_input_source_name(source: rt::SessionInputSource) -> &'static str {
    match source {
        rt::SessionInputSource::ServiceControl => "service-control",
        rt::SessionInputSource::Hardware => "hardware",
        rt::SessionInputSource::None => "none",
    }
}

pub fn error_name(error: rt::Error) -> &'static str {
    match error {
        rt::Error::Unsupported => "unsupported",
        rt::Error::InvalidCall => "invalid-call",
        rt::Error::PermissionDenied => "permission-denied",
        rt::Error::NotInitialized => "not-initialized",
        rt::Error::InvalidArgument => "invalid-argument",
        rt::Error::BufferTooSmall => "buffer-too-small",
        rt::Error::QueueEmpty => "timeout",
        rt::Error::NotFound => "not-found",
        rt::Error::Busy => "busy",
        rt::Error::CapacityExceeded => "capacity-exceeded",
        rt::Error::Unknown(_) => "unknown",
    }
}

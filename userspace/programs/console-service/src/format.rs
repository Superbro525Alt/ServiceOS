use core::fmt::Write;

use rt::{LifecycleEvent, LogDomain, LogEvent, LogSeverity, ServiceId};
use serviceos_userspace_runtime as rt;

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
        _ => LogEvent::LookupGranted,
    }
}

pub(crate) fn lifecycle_event_from_word(value: u64) -> LifecycleEvent {
    match value as u32 {
        x if x == LifecycleEvent::Starting as u32 => LifecycleEvent::Starting,
        x if x == LifecycleEvent::Ready as u32 => LifecycleEvent::Ready,
        x if x == LifecycleEvent::Failed as u32 => LifecycleEvent::Failed,
        x if x == LifecycleEvent::Stopped as u32 => LifecycleEvent::Stopped,
        _ => LifecycleEvent::Restarting,
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

pub(crate) fn format_ipv4(value: u32) -> FixedValueText {
    FixedValueText::ipv4(value)
}

pub(crate) fn format_mac(value: [u8; 6]) -> FixedValueText {
    FixedValueText::mac(value)
}

pub(crate) fn unpack_mac(value: u64) -> [u8; 6] {
    [
        (value & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        ((value >> 16) & 0xff) as u8,
        ((value >> 24) & 0xff) as u8,
        ((value >> 32) & 0xff) as u8,
        ((value >> 40) & 0xff) as u8,
    ]
}

pub(crate) struct FixedValueText {
    bytes: [u8; 32],
    len: usize,
}

impl FixedValueText {
    fn ipv4(value: u32) -> Self {
        let mut text = Self {
            bytes: [0; 32],
            len: 0,
        };
        let _ = write!(
            &mut text,
            "{}.{}.{}.{}",
            (value >> 24) & 0xff,
            (value >> 16) & 0xff,
            (value >> 8) & 0xff,
            value & 0xff,
        );
        text
    }

    fn mac(value: [u8; 6]) -> Self {
        let mut text = Self {
            bytes: [0; 32],
            len: 0,
        };
        let _ = write!(
            &mut text,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            value[0], value[1], value[2], value[3], value[4], value[5],
        );
        text
    }
}

impl core::fmt::Display for FixedValueText {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let text = core::str::from_utf8(&self.bytes[..self.len]).map_err(|_| core::fmt::Error)?;
        f.write_str(text)
    }
}

impl Write for FixedValueText {
    fn write_str(&mut self, value: &str) -> core::fmt::Result {
        let bytes = value.as_bytes();
        let remaining = self.bytes.len().saturating_sub(self.len);
        let copy_len = remaining.min(bytes.len());
        self.bytes[self.len..self.len + copy_len].copy_from_slice(&bytes[..copy_len]);
        self.len += copy_len;
        Ok(())
    }
}

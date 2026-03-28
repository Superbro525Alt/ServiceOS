use core::fmt::Write;

use serviceos_userspace_runtime as rt;
use rt::{
    ConfigKey, DesktopAppId, DesktopDragMode, FixedLogBuffer, LogDomain, LogEvent, LogSeverity,
    ManagerServicePhase, ServiceId,
};

pub(crate) const MAX_LISTED_SERVICES: usize = 12;
pub(crate) const MAX_STORAGE_PATH: usize = 96;
pub(crate) const MAX_CAT_CHUNK: usize = 96;
pub(crate) const MAX_VERSION_BYTES: usize = 24;
pub(crate) const MAX_DESKTOP_APPS: usize = 8;
pub(crate) const MAX_DESKTOP_WINDOWS: usize = 8;
pub(crate) const MAX_SESSION_WRITE_BYTES: usize = (rt::IPC_MAX_WORDS - 1) * 8;

pub(crate) const HELP_TEXT: &str = "\
help: show this command list\r\n\
services: list managed services\r\n\
service <name>: show one service state\r\n\
restart <name>: request a service restart\r\n\
logs [count]: show recent structured logs\r\n\
config: show core configuration values\r\n\
store ls [prefix]: list boot-store paths\r\n\
cat <path>: print a text resource\r\n\
status: show system heartbeat status\r\n\
net ifaces: show network interfaces\r\n\
net route: show the default route\r\n\
net resolve <name>: resolve a host or literal\r\n\
net ping <name|ip>: run an ICMP reachability probe\r\n\
gfx outputs: show graphics outputs\r\n\
gfx surfaces: show compositor surfaces\r\n\
gfx sessions: show graphical sessions\r\n\
gfx focus <surface-id>: change focused session surface\r\n\
desktop status: show desktop shell status\r\n\
desktop apps: list desktop app state\r\n\
desktop windows: list desktop window state\r\n\
desktop launch <settings|files|monitor>: launch a desktop app\r\n\
desktop focus <settings|files|monitor>: focus a desktop app\r\n\
desktop next: focus the next visible window\r\n\
desktop close <settings|files|monitor>: close a desktop app window\r\n\
desktop minimize <settings|files|monitor>: minimize a desktop app window\r\n\
desktop restore <settings|files|monitor>: restore a minimized app window\r\n\
desktop maximize <settings|files|monitor>: maximize or restore a window\r\n\
desktop move <settings|files|monitor> <x> <y>: move a window\r\n\
desktop resize <settings|files|monitor> <width> <height>: resize a window\r\n\
desktop click <x> <y>: inject a pointer click into the desktop session\r\n\
pkg list: list repository packages\r\n\
pkg info <name>: inspect one package\r\n\
pkg install <name> [version]: activate a package\r\n\
pkg update <name> [version]: switch to a newer package version\r\n\
pkg remove <name>: deactivate a package\r\n\
pkg rollback <name>: restore the prior active version\r\n\
pkg history <name>: show current and rollback versions\r\n\
run sysinfo: launch a transient tool\r\n";

pub(crate) fn emit_shell_log(
    bootstrap: rt::Handle,
    severity: LogSeverity,
    event: LogEvent,
    arg0: u64,
    arg1: u64,
) -> rt::Result<()> {
    let log_handle = rt::lookup_service(bootstrap, ServiceId::Log)?;
    let result = rt::send_log_record(
        log_handle,
        ServiceId::Shell,
        severity,
        LogDomain::Shell,
        event,
        arg0,
        arg1,
    );
    let _ = rt::handle_close(log_handle);
    result
}

pub(crate) fn write_session_linef(
    session: rt::Handle,
    args: core::fmt::Arguments<'_>,
) -> rt::Result<()> {
    let mut buffer = FixedLogBuffer::<256>::new();
    let _ = buffer.write_fmt(args);
    let _ = buffer.write_str("\r\n");
    let text = core::str::from_utf8(buffer.as_bytes()).map_err(|_| rt::Error::InvalidArgument)?;
    rt::console_session_write(session, text)
}

pub(crate) fn write_session_text(session: rt::Handle, text: &str) -> rt::Result<()> {
    let bytes = text.as_bytes();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let end = (offset + MAX_SESSION_WRITE_BYTES).min(bytes.len());
        let chunk =
            core::str::from_utf8(&bytes[offset..end]).map_err(|_| rt::Error::InvalidArgument)?;
        rt::console_session_write(session, chunk)?;
        offset = end;
    }
    Ok(())
}

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
        _ => None,
    }
}

pub(crate) fn parse_desktop_app_name(name: &str) -> Option<DesktopAppId> {
    match name {
        "settings" => Some(DesktopAppId::Settings),
        "files" => Some(DesktopAppId::Files),
        "monitor" => Some(DesktopAppId::Monitor),
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
    }
}

pub(crate) fn desktop_app_name(app_id: DesktopAppId) -> &'static str {
    match app_id {
        DesktopAppId::Settings => "settings",
        DesktopAppId::Files => "files",
        DesktopAppId::Monitor => "monitor",
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
        ManagerServicePhase::Starting => "starting",
        ManagerServicePhase::Ready => "ready",
        ManagerServicePhase::Exited => "exited",
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
        LogEvent::NetworkInterfaceReady => "network-interface-ready",
        LogEvent::NetworkAddressConfigured => "network-address-configured",
        LogEvent::NetworkResolveCompleted => "network-resolve-completed",
        LogEvent::NetworkProbeCompleted => "network-probe-completed",
        LogEvent::NetworkLinkChanged => "network-link-changed",
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
    }
}

pub(crate) fn write_log_record(session: rt::Handle, record: rt::LogRecord) -> rt::Result<()> {
    match record.event {
        LogEvent::ConfigLoaded => write_session_linef(
            session,
            format_args!(
                "#{} {} {} {}/{} minimum-severity={}",
                record.sequence,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                record.arg0,
            ),
        ),
        LogEvent::NetworkInterfaceReady => write_session_linef(
            session,
            format_args!(
                "#{} {} {} {}/{} iface={} mac={}",
                record.sequence,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                record.arg0,
                format_mac(unpack_mac(record.arg1)),
            ),
        ),
        LogEvent::NetworkAddressConfigured => write_session_linef(
            session,
            format_args!(
                "#{} {} {} {}/{} addr={} gateway={}",
                record.sequence,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                format_ipv4(record.arg0 as u32),
                format_ipv4(record.arg1 as u32),
            ),
        ),
        LogEvent::NetworkResolveCompleted => write_session_linef(
            session,
            format_args!(
                "#{} {} {} {}/{} addr={} count={}",
                record.sequence,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                format_ipv4(record.arg0 as u32),
                record.arg1,
            ),
        ),
        LogEvent::NetworkProbeCompleted => write_session_linef(
            session,
            format_args!(
                "#{} {} {} {}/{} addr={} elapsed-ms={}",
                record.sequence,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                format_ipv4(record.arg0 as u32),
                record.arg1,
            ),
        ),
        LogEvent::DisplayOutputReady => write_session_linef(
            session,
            format_args!(
                "#{} {} {} {}/{} {}x{}",
                record.sequence,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                record.arg0,
                record.arg1,
            ),
        ),
        LogEvent::SurfaceCreated | LogEvent::SessionReady | LogEvent::SessionFocusChanged => {
            write_session_linef(
                session,
                format_args!(
                    "#{} {} {} {}/{} {} {}",
                    record.sequence,
                    severity_name(record.severity),
                    service_name(record.source),
                    domain_name(record.domain),
                    event_name(record.event),
                    record.arg0,
                    record.arg1,
                ),
            )
        }
        _ => write_session_linef(
            session,
            format_args!(
                "#{} {} {} {}/{} {} {}",
                record.sequence,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                record.arg0,
                record.arg1,
            ),
        ),
    }
}

pub(crate) fn config_value_text(key: ConfigKey, value: u64) -> FixedValueText {
    match key {
        ConfigKey::NetworkIpv4Address | ConfigKey::NetworkIpv4Gateway => {
            FixedValueText::ipv4(value as u32)
        }
        _ => FixedValueText::unsigned(value),
    }
}

pub(crate) fn link_state_name(state: rt::PacketInterfaceLinkState) -> &'static str {
    match state {
        rt::PacketInterfaceLinkState::Up => "up",
        rt::PacketInterfaceLinkState::Down => "down",
    }
}

pub(crate) fn format_ipv4(value: u32) -> FixedValueText {
    FixedValueText::ipv4(value)
}

pub(crate) fn format_mac(value: [u8; 6]) -> FixedValueText {
    FixedValueText::mac(value)
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

fn unpack_mac(value: u64) -> [u8; 6] {
    [
        (value & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        ((value >> 16) & 0xff) as u8,
        ((value >> 24) & 0xff) as u8,
        ((value >> 32) & 0xff) as u8,
        ((value >> 40) & 0xff) as u8,
    ]
}

pub(crate) fn error_name(error: rt::Error) -> &'static str {
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

pub(crate) struct FixedValueText {
    bytes: [u8; 32],
    len: usize,
}

impl FixedValueText {
    pub(crate) fn unsigned(value: u64) -> Self {
        let mut text = Self {
            bytes: [0; 32],
            len: 0,
        };
        let _ = write!(&mut text, "{value}");
        text
    }

    pub(crate) fn ipv4(value: u32) -> Self {
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

    pub(crate) fn mac(value: [u8; 6]) -> Self {
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

pub(crate) fn printable_version(value: &str) -> &str {
    if value.is_empty() {
        "-"
    } else {
        value
    }
}

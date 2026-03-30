#![no_std]

use core::{
    arch::asm,
    fmt::{self, Write},
};

pub use serviceos_abi::{
    AppControlTag, AppKeyAction, AppPointerAction, AudioEndpointBackend, AudioEndpointDirection,
    AudioEndpointInfo, AudioEndpointState, AudioStatus, AudioStreamDirection, AudioStreamState,
    AudioTag, AudioToneRequest, BootstrapPlatform, ClipboardStatus, ClipboardTag, ConfigKey,
    ConfigTag, ConfigValueKind,
    ConsoleTag, ControlTag, DesktopAppId, DesktopDragMode, DesktopInputAction, DesktopStatus,
    DesktopTag, DesktopWindowAction, DisplayOutputBackend, DisplayOutputInfo, DisplayOutputState,
    DisplayPixelFormat, GraphicsStatus, GraphicsTag, Handle, HandlePair, IPC_FLAG_NONBLOCK,
    IPC_MAX_HANDLES, IPC_MAX_WORDS, INPUT_SOURCE_FLAG_NONBLOCK, INVALID_HANDLE, InputButton,
    InputEventInfo, InputEventKind, InputSourceBackend, InputSourceInfo, LifecycleEvent,
    LogDomain, LogEvent, LogQueryStatus, LogSeverity, LogTag, LookupStatus, ManagerAction,
    ManagerServicePhase, ManagerStatus, ManagerTag, NetworkConfigMode, NetworkConfigState,
    NetworkSocketKind, NetworkSocketState, NetworkSocketTag, NetworkStatus, NetworkTag,
    PACKET_INTERFACE_FLAG_NONBLOCK, PacketInterfaceBackend, PacketInterfaceInfo,
    PacketInterfaceLinkState, PackageStatus, PackageTag, RawMessage, ServiceId, ServiceImageId,
    DeveloperArtifactFormat, DeveloperJobState, DeveloperStatus, DeveloperTag, DeveloperTarget,
    DeveloperToolchainState, RuntimeEnvState, RuntimeKind, RuntimeRunState, RuntimeStatus,
    RuntimeTag, RuntimeWorkloadKind, SessionInputSource, SessionStatus, SessionTag, StatusTag,
    StorageEntryKind, StorageStatus, StorageTag, SurfaceTag, SyscallErrorCode, SyscallNumber,
    TaskStateCode, TaskStatus, TerminalStatus, TerminalTag,
};
pub use serviceos_abi::{audio_capability, input_capability, rights, runtime_capability};

mod app_control;
mod audio;
mod bootstrap;
mod clipboard;
mod config;
mod developer;
mod compat;
mod console;
mod desktop;
mod devices;
mod graphics;
mod glyphs;
mod ipc;
mod kernel;
mod log_service;
mod manager;
mod memory;
mod network;
mod package;
mod relay;
mod session;
mod status;
mod storage;
mod terminal;
mod types;

pub use app_control::*;
pub use audio::*;
pub use bootstrap::*;
pub use clipboard::*;
pub use config::*;
pub use developer::*;
pub use compat::*;
pub use console::*;
pub use desktop::*;
pub use devices::*;
pub use graphics::*;
pub use glyphs::*;
pub use ipc::*;
pub use kernel::*;
pub use log_service::*;
pub use manager::*;
pub use memory::*;
pub use network::*;
pub use package::*;
pub use relay::*;
pub use session::*;
pub use status::*;
pub use storage::*;
pub use terminal::*;
pub use types::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Unsupported,
    InvalidCall,
    PermissionDenied,
    NotInitialized,
    InvalidArgument,
    BufferTooSmall,
    QueueEmpty,
    NotFound,
    Busy,
    CapacityExceeded,
    Unknown(u64),
}

impl Error {
    fn from_code(code: u64) -> Self {
        match code {
            x if x == SyscallErrorCode::Unsupported as u64 => Self::Unsupported,
            x if x == SyscallErrorCode::InvalidCall as u64 => Self::InvalidCall,
            x if x == SyscallErrorCode::PermissionDenied as u64 => Self::PermissionDenied,
            x if x == SyscallErrorCode::NotInitialized as u64 => Self::NotInitialized,
            x if x == SyscallErrorCode::InvalidArgument as u64 => Self::InvalidArgument,
            x if x == SyscallErrorCode::BufferTooSmall as u64 => Self::BufferTooSmall,
            x if x == SyscallErrorCode::QueueEmpty as u64 => Self::QueueEmpty,
            x if x == SyscallErrorCode::NotFound as u64 => Self::NotFound,
            x if x == SyscallErrorCode::Busy as u64 => Self::Busy,
            x if x == SyscallErrorCode::CapacityExceeded as u64 => Self::CapacityExceeded,
            other => Self::Unknown(other),
        }
    }
}

pub type Result<T> = core::result::Result<T, Error>;

pub fn write_log(domain: &str, message: &str) -> Result<()> {
    let mut buffer = FixedLogBuffer::<192>::new();
    let _ = write!(&mut buffer, "{domain}: {message}");
    debug_log(buffer.as_bytes())
}

pub fn write_logf(domain: &str, args: fmt::Arguments<'_>) -> Result<()> {
    let mut buffer = FixedLogBuffer::<192>::new();
    let _ = write!(&mut buffer, "{domain}: ");
    let _ = buffer.write_fmt(args);
    debug_log(buffer.as_bytes())
}

pub struct FixedLogBuffer<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> FixedLogBuffer<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_bytes()).unwrap_or("")
    }
}

impl<const N: usize> Write for FixedLogBuffer<N> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let bytes = value.as_bytes();
        let remaining = N.saturating_sub(self.len);
        let copy_len = remaining.min(bytes.len());
        self.bytes[self.len..self.len + copy_len].copy_from_slice(&bytes[..copy_len]);
        self.len += copy_len;
        Ok(())
    }
}

#[macro_export]
macro_rules! entry {
    ($path:path) => {
        #[panic_handler]
        fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
            let _ = $crate::write_log("panic", "userspace panic");
            $crate::thread_exit(0xffff_ffff_ffff_ff00)
        }

        #[unsafe(no_mangle)]
        #[unsafe(link_section = ".text.start")]
        pub extern "C" fn _start() -> ! {
            let code: u64 = $path();
            $crate::thread_exit(code)
        }
    };
}

fn syscall0(number: SyscallNumber) -> Result<u64> {
    let (value, error) = raw_syscall(number as u32 as u64, 0, 0, 0, 0, 0, 0);
    decode_result(value, error)
}

fn syscall1(number: SyscallNumber, arg0: u64) -> Result<u64> {
    let (value, error) = raw_syscall(number as u32 as u64, arg0, 0, 0, 0, 0, 0);
    decode_result(value, error)
}

fn syscall2(number: SyscallNumber, arg0: u64, arg1: u64) -> Result<u64> {
    let (value, error) = raw_syscall(number as u32 as u64, arg0, arg1, 0, 0, 0, 0);
    decode_result(value, error)
}

fn syscall3(number: SyscallNumber, arg0: u64, arg1: u64, arg2: u64) -> Result<u64> {
    let (value, error) = raw_syscall(number as u32 as u64, arg0, arg1, arg2, 0, 0, 0);
    decode_result(value, error)
}

fn syscall4(number: SyscallNumber, arg0: u64, arg1: u64, arg2: u64, arg3: u64) -> Result<u64> {
    let (value, error) = raw_syscall(number as u32 as u64, arg0, arg1, arg2, arg3, 0, 0);
    decode_result(value, error)
}

pub fn pack_bytes(source: &[u8], words: &mut [u64]) -> Result<u32> {
    let required_words = source.len().div_ceil(8);
    if required_words > words.len() {
        return Err(Error::BufferTooSmall);
    }
    for (index, chunk) in source.chunks(8).enumerate() {
        let mut bytes = [0u8; 8];
        bytes[..chunk.len()].copy_from_slice(chunk);
        words[index] = u64::from_le_bytes(bytes);
    }
    Ok(required_words as u32)
}

pub fn unpack_bytes(words: &[u64], len: usize, destination: &mut [u8]) -> Result<()> {
    if len > destination.len() || len > words.len() * 8 {
        return Err(Error::BufferTooSmall);
    }

    let mut copied = 0usize;
    for word in words.iter().copied() {
        if copied >= len {
            break;
        }
        let bytes = word.to_le_bytes();
        let chunk = (len - copied).min(bytes.len());
        destination[copied..copied + chunk].copy_from_slice(&bytes[..chunk]);
        copied += chunk;
    }
    Ok(())
}

fn service_id_from_word(value: u64) -> ServiceId {
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
        _ => ServiceId::RootManager,
    }
}

fn severity_from_word(value: u64) -> LogSeverity {
    match value as u32 {
        x if x == LogSeverity::Trace as u32 => LogSeverity::Trace,
        x if x == LogSeverity::Debug as u32 => LogSeverity::Debug,
        x if x == LogSeverity::Warn as u32 => LogSeverity::Warn,
        x if x == LogSeverity::Error as u32 => LogSeverity::Error,
        _ => LogSeverity::Info,
    }
}

fn domain_from_word(value: u64) -> LogDomain {
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
        _ => LogDomain::Service,
    }
}

fn event_from_word(value: u64) -> LogEvent {
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
        _ => LogEvent::LookupGranted,
    }
}

fn manager_phase_from_word(value: u64) -> ManagerServicePhase {
    match value as u32 {
        x if x == ManagerServicePhase::Dormant as u32 => ManagerServicePhase::Dormant,
        x if x == ManagerServicePhase::Starting as u32 => ManagerServicePhase::Starting,
        x if x == ManagerServicePhase::Exited as u32 => ManagerServicePhase::Exited,
        _ => ManagerServicePhase::Ready,
    }
}

fn manager_status_from_word(value: u64) -> ManagerStatus {
    match value as u32 {
        x if x == ManagerStatus::Ok as u32 => ManagerStatus::Ok,
        x if x == ManagerStatus::Denied as u32 => ManagerStatus::Denied,
        x if x == ManagerStatus::NotFound as u32 => ManagerStatus::NotFound,
        x if x == ManagerStatus::Busy as u32 => ManagerStatus::Busy,
        x if x == ManagerStatus::Failed as u32 => ManagerStatus::Failed,
        _ => ManagerStatus::Busy,
    }
}

fn package_status_from_word(value: u64) -> PackageStatus {
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
        _ => PackageStatus::Busy,
    }
}

fn package_status_error(status: PackageStatus) -> Error {
    match status {
        PackageStatus::NotFound => Error::NotFound,
        PackageStatus::AlreadyInstalled | PackageStatus::Busy | PackageStatus::NoChange => {
            Error::Busy
        }
        PackageStatus::NotInstalled | PackageStatus::NoRollback | PackageStatus::End => {
            Error::InvalidArgument
        }
        PackageStatus::Denied => Error::PermissionDenied,
        PackageStatus::IntegrityFailed => Error::InvalidCall,
        PackageStatus::Ok => Error::InvalidArgument,
    }
}

fn network_status_from_word(value: u64) -> NetworkStatus {
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

fn network_status_error(status: NetworkStatus) -> Error {
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

fn audio_status_from_word(value: u64) -> AudioStatus {
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

fn audio_status_error(status: AudioStatus) -> Error {
    match status {
        AudioStatus::Ok => Error::InvalidArgument,
        AudioStatus::NotFound | AudioStatus::Closed => Error::NotFound,
        AudioStatus::Busy => Error::Busy,
        AudioStatus::Unsupported => Error::Unsupported,
        AudioStatus::Denied => Error::PermissionDenied,
        AudioStatus::CapacityExceeded => Error::CapacityExceeded,
    }
}

fn audio_endpoint_backend_from_word(value: u64) -> AudioEndpointBackend {
    match value as u32 {
        x if x == AudioEndpointBackend::PcSpeaker as u32 => AudioEndpointBackend::PcSpeaker,
        _ => AudioEndpointBackend::Unknown,
    }
}

fn audio_endpoint_direction_from_word(value: u64) -> AudioEndpointDirection {
    match value as u32 {
        x if x == AudioEndpointDirection::Input as u32 => AudioEndpointDirection::Input,
        _ => AudioEndpointDirection::Output,
    }
}

fn audio_endpoint_state_from_word(value: u64) -> AudioEndpointState {
    match value as u32 {
        x if x == AudioEndpointState::Offline as u32 => AudioEndpointState::Offline,
        x if x == AudioEndpointState::Active as u32 => AudioEndpointState::Active,
        _ => AudioEndpointState::Idle,
    }
}

fn audio_stream_direction_from_word(value: u64) -> AudioStreamDirection {
    match value as u32 {
        x if x == AudioStreamDirection::Capture as u32 => AudioStreamDirection::Capture,
        _ => AudioStreamDirection::Playback,
    }
}

fn audio_stream_state_from_word(value: u64) -> AudioStreamState {
    match value as u32 {
        x if x == AudioStreamState::Active as u32 => AudioStreamState::Active,
        x if x == AudioStreamState::Closed as u32 => AudioStreamState::Closed,
        x if x == AudioStreamState::Failed as u32 => AudioStreamState::Failed,
        _ => AudioStreamState::Idle,
    }
}

fn runtime_status_from_word(value: u64) -> RuntimeStatus {
    match value as u32 {
        x if x == RuntimeStatus::Ok as u32 => RuntimeStatus::Ok,
        x if x == RuntimeStatus::NotFound as u32 => RuntimeStatus::NotFound,
        x if x == RuntimeStatus::Busy as u32 => RuntimeStatus::Busy,
        x if x == RuntimeStatus::Denied as u32 => RuntimeStatus::Denied,
        x if x == RuntimeStatus::InvalidPath as u32 => RuntimeStatus::InvalidPath,
        x if x == RuntimeStatus::Unsupported as u32 => RuntimeStatus::Unsupported,
        x if x == RuntimeStatus::Closed as u32 => RuntimeStatus::Closed,
        _ => RuntimeStatus::Busy,
    }
}

fn runtime_status_error(status: RuntimeStatus) -> Error {
    match status {
        RuntimeStatus::Ok => Error::InvalidArgument,
        RuntimeStatus::NotFound | RuntimeStatus::Closed => Error::NotFound,
        RuntimeStatus::Busy => Error::Busy,
        RuntimeStatus::Denied => Error::PermissionDenied,
        RuntimeStatus::InvalidPath => Error::InvalidArgument,
        RuntimeStatus::Unsupported => Error::Unsupported,
    }
}

fn runtime_kind_from_word(value: u64) -> RuntimeKind {
    match value as u32 {
        x if x == RuntimeKind::Windows as u32 => RuntimeKind::Windows,
        _ => RuntimeKind::Posix,
    }
}

fn runtime_env_state_from_word(value: u64) -> RuntimeEnvState {
    match value as u32 {
        x if x == RuntimeEnvState::Busy as u32 => RuntimeEnvState::Busy,
        x if x == RuntimeEnvState::Destroyed as u32 => RuntimeEnvState::Destroyed,
        _ => RuntimeEnvState::Ready,
    }
}

fn runtime_run_state_from_word(value: u64) -> RuntimeRunState {
    match value as u32 {
        x if x == RuntimeRunState::Running as u32 => RuntimeRunState::Running,
        x if x == RuntimeRunState::Exited as u32 => RuntimeRunState::Exited,
        x if x == RuntimeRunState::Failed as u32 => RuntimeRunState::Failed,
        _ => RuntimeRunState::Launching,
    }
}

fn runtime_workload_kind_from_word(value: u64) -> RuntimeWorkloadKind {
    match value as u32 {
        x if x == RuntimeWorkloadKind::Env as u32 => RuntimeWorkloadKind::Env,
        x if x == RuntimeWorkloadKind::Mounts as u32 => RuntimeWorkloadKind::Mounts,
        x if x == RuntimeWorkloadKind::Cat as u32 => RuntimeWorkloadKind::Cat,
        _ => RuntimeWorkloadKind::Inspect,
    }
}

fn developer_status_from_word(value: u64) -> DeveloperStatus {
    match value as u32 {
        x if x == DeveloperStatus::Ok as u32 => DeveloperStatus::Ok,
        x if x == DeveloperStatus::NotFound as u32 => DeveloperStatus::NotFound,
        x if x == DeveloperStatus::Busy as u32 => DeveloperStatus::Busy,
        x if x == DeveloperStatus::Denied as u32 => DeveloperStatus::Denied,
        x if x == DeveloperStatus::Unsupported as u32 => DeveloperStatus::Unsupported,
        _ => DeveloperStatus::Busy,
    }
}

fn developer_status_error(status: DeveloperStatus) -> Error {
    match status {
        DeveloperStatus::Ok => Error::InvalidArgument,
        DeveloperStatus::NotFound => Error::NotFound,
        DeveloperStatus::Busy => Error::Busy,
        DeveloperStatus::Denied => Error::PermissionDenied,
        DeveloperStatus::Unsupported => Error::Unsupported,
    }
}

fn developer_target_from_word(value: u64) -> DeveloperTarget {
    match value as u32 {
        x if x == DeveloperTarget::LinuxX64 as u32 => DeveloperTarget::LinuxX64,
        x if x == DeveloperTarget::WindowsX64 as u32 => DeveloperTarget::WindowsX64,
        x if x == DeveloperTarget::MacosX64 as u32 => DeveloperTarget::MacosX64,
        _ => DeveloperTarget::NativeX64,
    }
}

fn developer_toolchain_state_from_word(value: u64) -> DeveloperToolchainState {
    match value as u32 {
        x if x == DeveloperToolchainState::RemoteOnly as u32 => DeveloperToolchainState::RemoteOnly,
        _ => DeveloperToolchainState::Installed,
    }
}

fn developer_artifact_format_from_word(value: u64) -> DeveloperArtifactFormat {
    match value as u32 {
        x if x == DeveloperArtifactFormat::Elf64 as u32 => DeveloperArtifactFormat::Elf64,
        x if x == DeveloperArtifactFormat::Pe32Plus as u32 => DeveloperArtifactFormat::Pe32Plus,
        x if x == DeveloperArtifactFormat::MachO64 as u32 => DeveloperArtifactFormat::MachO64,
        _ => DeveloperArtifactFormat::ServiceOsFlat,
    }
}

fn developer_job_state_from_word(value: u64) -> DeveloperJobState {
    match value as u32 {
        x if x == DeveloperJobState::Running as u32 => DeveloperJobState::Running,
        x if x == DeveloperJobState::Succeeded as u32 => DeveloperJobState::Succeeded,
        x if x == DeveloperJobState::Failed as u32 => DeveloperJobState::Failed,
        x if x == DeveloperJobState::Unsupported as u32 => DeveloperJobState::Unsupported,
        _ => DeveloperJobState::Queued,
    }
}

fn network_config_mode_from_word(value: u64) -> NetworkConfigMode {
    match value as u32 {
        x if x == NetworkConfigMode::Dynamic as u32 => NetworkConfigMode::Dynamic,
        _ => NetworkConfigMode::Static,
    }
}

fn network_config_state_from_word(value: u64) -> NetworkConfigState {
    match value as u32 {
        x if x == NetworkConfigState::Pending as u32 => NetworkConfigState::Pending,
        x if x == NetworkConfigState::FallbackStatic as u32 => NetworkConfigState::FallbackStatic,
        x if x == NetworkConfigState::Failed as u32 => NetworkConfigState::Failed,
        _ => NetworkConfigState::Configured,
    }
}

fn network_socket_kind_from_word(value: u64) -> NetworkSocketKind {
    match value as u32 {
        _ => NetworkSocketKind::TcpStream,
    }
}

fn network_socket_state_from_word(value: u64) -> NetworkSocketState {
    match value as u32 {
        x if x == NetworkSocketState::Connecting as u32 => NetworkSocketState::Connecting,
        x if x == NetworkSocketState::Established as u32 => NetworkSocketState::Established,
        x if x == NetworkSocketState::Closing as u32 => NetworkSocketState::Closing,
        x if x == NetworkSocketState::Failed as u32 => NetworkSocketState::Failed,
        _ => NetworkSocketState::Closed,
    }
}

fn graphics_status_from_word(value: u64) -> GraphicsStatus {
    match value as u32 {
        x if x == GraphicsStatus::Ok as u32 => GraphicsStatus::Ok,
        x if x == GraphicsStatus::NotFound as u32 => GraphicsStatus::NotFound,
        x if x == GraphicsStatus::Busy as u32 => GraphicsStatus::Busy,
        x if x == GraphicsStatus::Denied as u32 => GraphicsStatus::Denied,
        x if x == GraphicsStatus::CapacityExceeded as u32 => GraphicsStatus::CapacityExceeded,
        _ => GraphicsStatus::Busy,
    }
}

fn graphics_status_error(status: GraphicsStatus) -> Error {
    match status {
        GraphicsStatus::Ok => Error::InvalidArgument,
        GraphicsStatus::NotFound => Error::NotFound,
        GraphicsStatus::Busy => Error::Busy,
        GraphicsStatus::Denied => Error::PermissionDenied,
        GraphicsStatus::CapacityExceeded => Error::CapacityExceeded,
    }
}

fn session_status_from_word(value: u64) -> SessionStatus {
    match value as u32 {
        x if x == SessionStatus::Ok as u32 => SessionStatus::Ok,
        x if x == SessionStatus::NotFound as u32 => SessionStatus::NotFound,
        x if x == SessionStatus::Busy as u32 => SessionStatus::Busy,
        x if x == SessionStatus::Denied as u32 => SessionStatus::Denied,
        _ => SessionStatus::Busy,
    }
}

fn session_status_error(status: SessionStatus) -> Error {
    match status {
        SessionStatus::Ok => Error::InvalidArgument,
        SessionStatus::NotFound => Error::NotFound,
        SessionStatus::Busy => Error::Busy,
        SessionStatus::Denied => Error::PermissionDenied,
    }
}

fn desktop_status_from_word(value: u64) -> DesktopStatus {
    match value as u32 {
        x if x == DesktopStatus::Ok as u32 => DesktopStatus::Ok,
        x if x == DesktopStatus::NotFound as u32 => DesktopStatus::NotFound,
        x if x == DesktopStatus::Busy as u32 => DesktopStatus::Busy,
        x if x == DesktopStatus::Denied as u32 => DesktopStatus::Denied,
        _ => DesktopStatus::Busy,
    }
}

fn desktop_status_error(status: DesktopStatus) -> Error {
    match status {
        DesktopStatus::Ok => Error::InvalidArgument,
        DesktopStatus::NotFound => Error::NotFound,
        DesktopStatus::Busy => Error::Busy,
        DesktopStatus::Denied => Error::PermissionDenied,
    }
}

fn desktop_app_id_from_word(value: u64) -> core::result::Result<DesktopAppId, ()> {
    match value as u32 {
        x if x == DesktopAppId::Settings as u32 => Ok(DesktopAppId::Settings),
        x if x == DesktopAppId::Files as u32 => Ok(DesktopAppId::Files),
        x if x == DesktopAppId::Monitor as u32 => Ok(DesktopAppId::Monitor),
        x if x == DesktopAppId::Terminal as u32 => Ok(DesktopAppId::Terminal),
        _ => Err(()),
    }
}

fn desktop_drag_mode_from_word(value: u64) -> DesktopDragMode {
    match value as u32 {
        x if x == DesktopDragMode::Move as u32 => DesktopDragMode::Move,
        x if x == DesktopDragMode::Resize as u32 => DesktopDragMode::Resize,
        _ => DesktopDragMode::None,
    }
}

fn unpack_i32_pair(value: u64) -> (i32, i32) {
    (value as u32 as i32, (value >> 32) as u32 as i32)
}

fn unpack_u32_pair(value: u64) -> (u32, u32) {
    (value as u32, (value >> 32) as u32)
}

fn display_backend_from_word(value: u64) -> DisplayOutputBackend {
    match value as u32 {
        x if x == DisplayOutputBackend::BootFramebuffer as u32 => {
            DisplayOutputBackend::BootFramebuffer
        }
        _ => DisplayOutputBackend::Unknown,
    }
}

fn display_state_from_word(value: u64) -> DisplayOutputState {
    match value as u32 {
        x if x == DisplayOutputState::Connected as u32 => DisplayOutputState::Connected,
        _ => DisplayOutputState::Disconnected,
    }
}

fn display_pixel_format_from_word(value: u64) -> DisplayPixelFormat {
    match value as u32 {
        x if x == DisplayPixelFormat::Xrgb8888 as u32 => DisplayPixelFormat::Xrgb8888,
        x if x == DisplayPixelFormat::Bgrx8888 as u32 => DisplayPixelFormat::Bgrx8888,
        _ => DisplayPixelFormat::Unknown,
    }
}

fn session_input_source_from_word(value: u64) -> SessionInputSource {
    match value as u32 {
        x if x == SessionInputSource::ServiceControl as u32 => SessionInputSource::ServiceControl,
        _ => SessionInputSource::None,
    }
}

fn packet_backend_from_word(value: u64) -> PacketInterfaceBackend {
    match value as u32 {
        x if x == PacketInterfaceBackend::VirtioPci as u32 => PacketInterfaceBackend::VirtioPci,
        _ => PacketInterfaceBackend::Unknown,
    }
}

fn packet_link_state_from_word(value: u64) -> PacketInterfaceLinkState {
    match value as u32 {
        x if x == PacketInterfaceLinkState::Up as u32 => PacketInterfaceLinkState::Up,
        _ => PacketInterfaceLinkState::Down,
    }
}

fn unpack_mac(word: u64) -> [u8; 6] {
    [
        (word & 0xff) as u8,
        ((word >> 8) & 0xff) as u8,
        ((word >> 16) & 0xff) as u8,
        ((word >> 24) & 0xff) as u8,
        ((word >> 32) & 0xff) as u8,
        ((word >> 40) & 0xff) as u8,
    ]
}

fn decode_result(value: u64, error: u64) -> Result<u64> {
    if error == 0 {
        Ok(value)
    } else {
        Err(Error::from_code(error))
    }
}

#[cfg(target_arch = "x86_64")]
fn raw_syscall(
    number: u64,
    arg0: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
) -> (u64, u64) {
    let value: u64;
    let error: u64;
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") number => value,
            in("rdi") arg0,
            in("rsi") arg1,
            inlateout("rdx") arg2 => error,
            in("r10") arg3,
            in("r8") arg4,
            in("r9") arg5,
        );
    }
    (value, error)
}

#[cfg(target_arch = "aarch64")]
fn raw_syscall(
    number: u64,
    arg0: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
) -> (u64, u64) {
    let mut value = arg0;
    let mut error = arg1;
    unsafe {
        asm!(
            "svc #0",
            in("x8") number,
            inlateout("x0") value,
            inlateout("x1") error,
            in("x2") arg2,
            in("x3") arg3,
            in("x4") arg4,
            in("x5") arg5,
            options(nostack),
        );
    }
    (value, error)
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn raw_syscall(
    _number: u64,
    _arg0: u64,
    _arg1: u64,
    _arg2: u64,
    _arg3: u64,
    _arg4: u64,
    _arg5: u64,
) -> (u64, u64) {
    (0, SyscallErrorCode::Unsupported as u64)
}

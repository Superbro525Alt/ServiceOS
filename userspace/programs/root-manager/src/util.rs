use serviceos_bundle::ServiceManifest;
use serviceos_userspace_runtime as rt;
use rt::{
    LogDomain, LogEvent, LogSeverity, ManagerAction, ManagerAvailability, ManagerServicePhase,
    ManagerStartupMode, ServiceId, ServiceImageId,
};

use crate::state::{BootstrapResources, ServicePhase, ServiceSlot, MAX_SERVICE_SLOTS};

pub(crate) fn emit_manager_event(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    severity: LogSeverity,
    event: LogEvent,
    target: ServiceId,
    detail: u64,
) -> rt::Result<()> {
    if let Some(log_index) = find_slot_index_checked(slots, service_count, ServiceId::Log) {
        let log_handle = slots[log_index].public_handle;
        if log_handle != rt::INVALID_HANDLE {
            return rt::send_log_record(
                log_handle,
                ServiceId::RootManager,
                severity,
                LogDomain::ServiceManager,
                event,
                target as u32 as u64,
                detail,
            );
        }
    }
    fallback_manager_event(severity, event, target, detail)
}

pub(crate) fn fallback_manager_event(
    severity: LogSeverity,
    event: LogEvent,
    target: ServiceId,
    detail: u64,
) -> rt::Result<()> {
    rt::write_logf(
        "service-manager",
        format_args!(
            "level={} event={} target={} detail={}",
            severity_name(severity),
            event_name(event),
            service_name(target),
            detail,
        ),
    )
}

pub(crate) fn bootstrap_resource_for(
    service_id: ServiceId,
    bootstrap_resources: BootstrapResources,
) -> Option<(rt::Handle, usize, u64)> {
    match service_id {
        ServiceId::Storage => Some((
            bootstrap_resources.bootstore.handle,
            bootstrap_resources.bootstore.len,
            bootstrap_resources.bootstore.rights,
        )),
        ServiceId::Network => bootstrap_resources
            .network
            .map(|resource| (resource.handle, resource.len, resource.rights)),
        ServiceId::Graphics => bootstrap_resources
            .display
            .map(|resource| (resource.handle, resource.len, resource.rights)),
        ServiceId::Session => bootstrap_resources
            .input
            .map(|resource| (resource.handle, resource.len, resource.rights)),
        ServiceId::Audio => bootstrap_resources
            .audio
            .map(|resource| (resource.handle, resource.len, resource.rights)),
        _ => None,
    }
}

pub(crate) fn service_index_path(platform: rt::BootstrapPlatform) -> &'static str {
    match platform {
        rt::BootstrapPlatform::Raspi5 => "services/index.raspi5.txt",
        _ => "services/index.txt",
    }
}

pub(crate) fn dependencies_ready(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    index: usize,
) -> bool {
    slots[index].manifest.dependencies[..slots[index].manifest.dependency_count]
        .iter()
        .copied()
        .all(|dependency| {
            if dependency == ServiceId::RootManager {
                return true;
            }
            find_slot_index_checked(slots, service_count, dependency)
                .map(|slot| slots[slot].phase == ServicePhase::Ready)
                .unwrap_or(false)
        })
}

pub(crate) fn first_unready_dependency(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    index: usize,
) -> Option<ServiceId> {
    slots[index].manifest.dependencies[..slots[index].manifest.dependency_count]
        .iter()
        .copied()
        .find(|dependency| {
            if *dependency == ServiceId::RootManager {
                return false;
            }
            find_slot_index_checked(slots, service_count, *dependency)
                .map(|slot| slots[slot].phase != ServicePhase::Ready)
                .unwrap_or(true)
        })
}

pub(crate) fn service_startup_mode(manifest: ServiceManifest) -> ManagerStartupMode {
    match manifest.startup {
        serviceos_bundle::ServiceStartupMode::Eager => ManagerStartupMode::Eager,
        serviceos_bundle::ServiceStartupMode::OnDemand => ManagerStartupMode::OnDemand,
    }
}

pub(crate) fn service_availability(manifest: ServiceManifest) -> ManagerAvailability {
    match manifest.availability {
        serviceos_bundle::ServiceAvailability::Required => ManagerAvailability::Required,
        serviceos_bundle::ServiceAvailability::Optional => ManagerAvailability::Optional,
    }
}

pub(crate) fn lookup_rights(slot: &ServiceSlot, requested: ServiceId) -> Option<u64> {
    slot.manifest.lookups[..slot.manifest.lookup_count]
        .iter()
        .enumerate()
        .find(|(index, entry)| {
            entry.target == requested && (slot.revoked_lookup_mask & (1u64 << *index)) == 0
        })
        .map(|(_, entry)| entry.rights)
}

pub(crate) fn set_lookup_policy(
    slot: &mut ServiceSlot,
    target: ServiceId,
    policy: rt::ManagerLookupPolicy,
) -> bool {
    let Some(index) = slot.manifest.lookups[..slot.manifest.lookup_count]
        .iter()
        .position(|entry| entry.target == target)
    else {
        return false;
    };

    match policy {
        rt::ManagerLookupPolicy::Default => {
            slot.revoked_lookup_mask &= !(1u64 << index);
        }
        rt::ManagerLookupPolicy::Revoked => {
            slot.revoked_lookup_mask |= 1u64 << index;
        }
    }
    true
}

pub(crate) fn allocate_slot(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: &mut usize,
) -> rt::Result<usize> {
    if let Some(index) = (0..*service_count).find(|index| !slots[*index].occupied) {
        return Ok(index);
    }
    if *service_count == slots.len() {
        return Err(rt::Error::CapacityExceeded);
    }
    let index = *service_count;
    *service_count += 1;
    Ok(index)
}

pub(crate) fn compact_service_slots(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: &mut usize,
) {
    while *service_count > 0 && !slots[*service_count - 1].occupied {
        *service_count -= 1;
    }
}

pub(crate) fn occupied_service_count(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
) -> usize {
    slots[..service_count]
        .iter()
        .filter(|slot| slot.occupied)
        .count()
}

pub(crate) fn ready_service_count(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
) -> usize {
    slots[..service_count]
        .iter()
        .filter(|slot| slot.occupied && slot.phase == ServicePhase::Ready)
        .count()
}

pub(crate) fn close_slot_handles(slot: &mut ServiceSlot) {
    close_if_valid(&mut slot.task_handle);
    close_if_valid(&mut slot.control_handle);
    close_if_valid(&mut slot.public_handle);
}

fn close_if_valid(handle: &mut rt::Handle) {
    if *handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(*handle);
        *handle = rt::INVALID_HANDLE;
    }
}

pub(crate) fn find_slot_index(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    service_id: ServiceId,
) -> rt::Result<usize> {
    find_slot_index_checked(slots, service_count, service_id).ok_or(rt::Error::NotFound)
}

pub(crate) fn find_slot_index_checked(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    service_id: ServiceId,
) -> Option<usize> {
    (0..service_count)
        .find(|index| slots[*index].occupied && slots[*index].manifest.service_id == service_id)
}

pub(crate) fn unpack_bytes(words: &[u64], len: usize, destination: &mut [u8]) -> rt::Result<()> {
    if len > destination.len() || len > words.len() * 8 {
        return Err(rt::Error::BufferTooSmall);
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

pub(crate) fn image_id_from_word(value: u64) -> ServiceImageId {
    match value as u32 {
        x if x == ServiceImageId::StorageService as u32 => ServiceImageId::StorageService,
        x if x == ServiceImageId::ConsoleService as u32 => ServiceImageId::ConsoleService,
        x if x == ServiceImageId::ConfigService as u32 => ServiceImageId::ConfigService,
        x if x == ServiceImageId::LogService as u32 => ServiceImageId::LogService,
        x if x == ServiceImageId::StatusService as u32 => ServiceImageId::StatusService,
        x if x == ServiceImageId::ShellService as u32 => ServiceImageId::ShellService,
        x if x == ServiceImageId::SysinfoTool as u32 => ServiceImageId::SysinfoTool,
        x if x == ServiceImageId::PackageService as u32 => ServiceImageId::PackageService,
        x if x == ServiceImageId::AnnounceService as u32 => ServiceImageId::AnnounceService,
        x if x == ServiceImageId::NetworkService as u32 => ServiceImageId::NetworkService,
        x if x == ServiceImageId::GraphicsService as u32 => ServiceImageId::GraphicsService,
        x if x == ServiceImageId::SessionService as u32 => ServiceImageId::SessionService,
        x if x == ServiceImageId::DesktopShellService as u32 => {
            ServiceImageId::DesktopShellService
        }
        x if x == ServiceImageId::SettingsApp as u32 => ServiceImageId::SettingsApp,
        x if x == ServiceImageId::FilesApp as u32 => ServiceImageId::FilesApp,
        x if x == ServiceImageId::MonitorApp as u32 => ServiceImageId::MonitorApp,
        x if x == ServiceImageId::TerminalService as u32 => ServiceImageId::TerminalService,
        x if x == ServiceImageId::TerminalApp as u32 => ServiceImageId::TerminalApp,
        x if x == ServiceImageId::AudioService as u32 => ServiceImageId::AudioService,
        x if x == ServiceImageId::RuntimeService as u32 => ServiceImageId::RuntimeService,
        x if x == ServiceImageId::PosixHostTool as u32 => ServiceImageId::PosixHostTool,
        x if x == ServiceImageId::DeveloperService as u32 => ServiceImageId::DeveloperService,
        x if x == ServiceImageId::CrossBuilderTool as u32 => ServiceImageId::CrossBuilderTool,
        x if x == ServiceImageId::ClipboardService as u32 => ServiceImageId::ClipboardService,
        x if x == ServiceImageId::SoftwareCenterApp as u32 => ServiceImageId::SoftwareCenterApp,
        x if x == ServiceImageId::SecurityService as u32 => ServiceImageId::SecurityService,
        _ => ServiceImageId::RootManager,
    }
}

pub(crate) fn manager_action_from_word(value: u64) -> ManagerAction {
    match value as u32 {
        x if x == ManagerAction::Restart as u32 => ManagerAction::Restart,
        _ => ManagerAction::Restart,
    }
}

pub(crate) fn encode_phase(phase: ServicePhase, attempts: u32) -> u64 {
    manager_phase(phase) as u32 as u64 | ((attempts as u64) << 32)
}

pub(crate) fn manager_phase(phase: ServicePhase) -> ManagerServicePhase {
    match phase {
        ServicePhase::Dormant => ManagerServicePhase::Dormant,
        ServicePhase::WaitingDependencies => ManagerServicePhase::WaitingDependencies,
        ServicePhase::Starting => ManagerServicePhase::Starting,
        ServicePhase::Ready => ManagerServicePhase::Ready,
        ServicePhase::Backoff => ManagerServicePhase::Backoff,
        ServicePhase::Degraded => ManagerServicePhase::Degraded,
        ServicePhase::Exited => ManagerServicePhase::Exited,
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
    }
}

pub(crate) fn fallback_log(message: &str) {
    let _ = rt::write_log("service-manager", message);
}

pub(crate) fn fallback_logf(args: core::fmt::Arguments<'_>) -> rt::Result<()> {
    rt::write_logf("service-manager", args)
}

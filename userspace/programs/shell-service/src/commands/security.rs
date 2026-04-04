use core::str;

use serviceos_userspace_runtime as rt;
use rt::{PermissionPolicyState, ServiceId, ServiceImageId};

use crate::util::{parse_service_name, ShellOutput, write_output_linef};

const MAX_SECURITY_AUDIT: usize = 8;

pub(crate) fn cmd_security<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    mut parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("apps") => cmd_security_apps(bootstrap, output),
        Some("app") => match parts.next() {
            Some(name) => {
                let Some(image_id) = parse_image_name(name) else {
                    return write_output_linef(output, format_args!("unknown app: {}", name));
                };
                match parts.next() {
                    Some("allow") => cmd_security_app_set(bootstrap, output, image_id, PermissionPolicyState::Allowed),
                    Some("block") => cmd_security_app_set(bootstrap, output, image_id, PermissionPolicyState::Blocked),
                    Some("default") => cmd_security_app_set(bootstrap, output, image_id, PermissionPolicyState::DefaultAllow),
                    None => cmd_security_app_info(bootstrap, output, image_id),
                    _ => write_output_linef(output, format_args!("usage: security app <name> [allow|block|default]")),
                }
            }
            None => write_output_linef(output, format_args!("usage: security app <name> [allow|block|default]")),
        },
        Some("runtimes") => cmd_security_runtimes(bootstrap, output),
        Some("runtime") => match parts.next().and_then(|v| v.parse::<u32>().ok()) {
            Some(env_id) => match parts.next() {
                Some("approve") => cmd_security_runtime_set(bootstrap, output, env_id, PermissionPolicyState::Allowed),
                Some("deny") => cmd_security_runtime_set(bootstrap, output, env_id, PermissionPolicyState::Blocked),
                Some("reset") => cmd_security_runtime_set(bootstrap, output, env_id, PermissionPolicyState::DefaultAllow),
                None => cmd_security_runtime_info(bootstrap, output, env_id),
                _ => write_output_linef(output, format_args!("usage: security runtime <env-id> [approve|deny|reset]")),
            },
            None => write_output_linef(output, format_args!("usage: security runtime <env-id> [approve|deny|reset]")),
        },
        Some("repos") => cmd_security_repos(bootstrap, output),
        Some("package") => match parts.next().and_then(parse_service_name) {
            Some(service_id) => cmd_security_package(bootstrap, output, service_id),
            None => write_output_linef(output, format_args!("usage: security package <name>")),
        },
        Some("workspace") => match parts.next().and_then(|v| v.parse::<u32>().ok()) {
            Some(id) => cmd_security_workspace(bootstrap, output, id),
            None => write_output_linef(output, format_args!("usage: security workspace <id>")),
        },
        Some("audit") => {
            let count = parts.next().and_then(|v| v.parse::<usize>().ok()).unwrap_or(MAX_SECURITY_AUDIT);
            cmd_security_audit(bootstrap, output, count)
        }
        _ => write_output_linef(
            output,
            format_args!("usage: security <apps|app|runtimes|runtime|repos|package|workspace|audit> ..."),
        ),
    }
}

fn cmd_security_apps(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let security = rt::security_lookup(bootstrap)?;
    let mut index = 0usize;
    let mut any = false;
    while let Some(info) = rt::security_policy_list(security, index)? {
        any = true;
        let name = str::from_utf8(&info.name[..info.name_len as usize]).unwrap_or("?");
        write_output_linef(
            output,
            format_args!(
                "{} policy={} perms={} sensitive={}",
                name,
                policy_name(info.policy),
                permission_summary(info.permissions),
                permission_summary(info.sensitive_permissions),
            ),
        )?;
        index += 1;
    }
    let _ = rt::handle_close(security);
    if any { Ok(()) } else { write_output_linef(output, format_args!("no security policies")) }
}

fn cmd_security_app_info(
    bootstrap: rt::Handle,
    output: ShellOutput,
    image_id: ServiceImageId,
) -> rt::Result<()> {
    let security = rt::security_lookup(bootstrap)?;
    let info = rt::security_policy_info(security, image_id)?;
    let _ = rt::handle_close(security);
    let name = str::from_utf8(&info.name[..info.name_len as usize]).unwrap_or("?");
    write_output_linef(output, format_args!("{}", name))?;
    write_output_linef(output, format_args!("  policy={}", policy_name(info.policy)))?;
    write_output_linef(output, format_args!("  perms={}", permission_summary(info.permissions)))?;
    write_output_linef(
        output,
        format_args!("  sensitive={}", permission_summary(info.sensitive_permissions)),
    )
}

fn cmd_security_app_set(
    bootstrap: rt::Handle,
    output: ShellOutput,
    image_id: ServiceImageId,
    policy: PermissionPolicyState,
) -> rt::Result<()> {
    let security = rt::security_lookup(bootstrap)?;
    rt::security_policy_set(security, image_id, policy)?;
    let _ = rt::handle_close(security);
    write_output_linef(output, format_args!("app policy updated"))
}

fn cmd_security_runtimes(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let runtime = rt::lookup_service(bootstrap, ServiceId::Runtime)?;
    let mut envs = [rt::RuntimeEnvInfo {
        env_id: 0,
        kind: rt::RuntimeKind::Posix,
        state: rt::RuntimeEnvState::Destroyed,
        capabilities: 0,
        mount_count: 0,
        var_count: 0,
        active_runs: 0,
    }; 8];
    let count = rt::runtime_env_list(runtime, &mut envs)?;
    let _ = rt::handle_close(runtime);
    if count == 0 {
        return write_output_linef(output, format_args!("no runtime environments"));
    }
    for env in envs.iter().take(count).copied() {
        write_output_linef(
            output,
            format_args!(
                "env{} kind={} state={} caps={}",
                env.env_id,
                runtime_kind_name(env.kind),
                runtime_env_state_name(env.state),
                runtime_cap_summary(env.capabilities),
            ),
        )?;
    }
    Ok(())
}

fn cmd_security_runtime_info(
    bootstrap: rt::Handle,
    output: ShellOutput,
    env_id: u32,
) -> rt::Result<()> {
    let runtime = rt::lookup_service(bootstrap, ServiceId::Runtime)?;
    let env = rt::runtime_env_status(runtime, env_id)?;
    let _ = rt::handle_close(runtime);
    write_output_linef(output, format_args!("env{}", env.env_id))?;
    write_output_linef(output, format_args!("  kind={}", runtime_kind_name(env.kind)))?;
    write_output_linef(output, format_args!("  state={}", runtime_env_state_name(env.state)))?;
    write_output_linef(output, format_args!("  caps={}", runtime_cap_summary(env.capabilities)))
}

fn cmd_security_runtime_set(
    bootstrap: rt::Handle,
    output: ShellOutput,
    env_id: u32,
    policy: PermissionPolicyState,
) -> rt::Result<()> {
    let runtime = rt::lookup_service(bootstrap, ServiceId::Runtime)?;
    rt::runtime_env_decide(runtime, env_id, policy)?;
    let _ = rt::handle_close(runtime);
    write_output_linef(output, format_args!("runtime policy updated"))
}

fn cmd_security_repos(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let package = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut name = [0u8; 24];
    let mut url = [0u8; 88];
    let mut index = 0usize;
    while let Some(repo) = rt::package_repository_list(package, index, &mut name, &mut url)? {
        write_output_linef(
            output,
            format_args!(
                "#{} {} trust={} sync={} digest={:016x}",
                repo.repo_index,
                str::from_utf8(&name[..repo.name_len]).unwrap_or("?"),
                repo_trust_name(repo.trust_mode),
                repo_sync_name(repo.sync_state),
                repo.last_digest,
            ),
        )?;
        index += 1;
    }
    let _ = rt::handle_close(package);
    Ok(())
}

fn cmd_security_package(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
) -> rt::Result<()> {
    let package = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut installed = [0u8; 24];
    let mut active = [0u8; 24];
    let mut rollback = [0u8; 24];
    let mut latest = [0u8; 24];
    let mut source = [0u8; 96];
    let info = rt::package_provenance(package, service_id, &mut installed, &mut active, &mut rollback, &mut latest, &mut source)?;
    let _ = rt::handle_close(package);
    write_output_linef(
        output,
        format_args!(
            "{} trust={} channel={} ring={} source={}",
            crate::util::service_name(service_id),
            package_trust_name(info.trust_state),
            channel_name(info.channel),
            ring_name(info.ring),
            str::from_utf8(&source[..info.source_len]).unwrap_or("?"),
        ),
    )
}

fn cmd_security_workspace(bootstrap: rt::Handle, output: ShellOutput, workspace_id: u32) -> rt::Result<()> {
    let developer = rt::lookup_service(bootstrap, ServiceId::Developer)?;
    let mut name = [0u8; 64];
    let mut source = [0u8; 96];
    let info = rt::developer_workspace_status(developer, workspace_id, &mut name, &mut source)?;
    let _ = rt::handle_close(developer);
    let name = str::from_utf8(&name[..info.name_len as usize]).unwrap_or("?");
    let source = str::from_utf8(&source[..info.source_path_len as usize]).unwrap_or("?");
    write_output_linef(output, format_args!("{}", name))?;
    write_output_linef(output, format_args!("  source={}", source))?;
    write_output_linef(output, format_args!("  review=package-delivered workspace metadata"))?;
    write_output_linef(output, format_args!("  build-authority=read source, emit artifact, no ambient network"))
}

fn cmd_security_audit(bootstrap: rt::Handle, output: ShellOutput, count: usize) -> rt::Result<()> {
    let security = rt::security_lookup(bootstrap)?;
    for index in 0..count {
        let Some(entry) = rt::security_audit_list(security, index)? else { break };
        write_output_linef(
            output,
            format_args!(
                "native#{} kind={} image={} policy={} detail={:#x}",
                entry.sequence,
                audit_kind_name(entry.kind),
                image_name(entry.subject_image_id),
                policy_name(entry.policy),
                entry.detail,
            ),
        )?;
    }
    let _ = rt::handle_close(security);

    let runtime = match rt::lookup_service(bootstrap, ServiceId::Runtime) {
        Ok(handle) => handle,
        Err(_) => return Ok(()),
    };
    for index in 0..count {
        let Some(entry) = rt::runtime_audit_list(runtime, index)? else { break };
        write_output_linef(
            output,
            format_args!(
                "runtime#{} kind={} env={} caps={} detail={:#x}",
                entry.sequence,
                audit_kind_name(entry.kind),
                entry.env_id,
                runtime_cap_summary(entry.capabilities),
                entry.detail,
            ),
        )?;
    }
    let _ = rt::handle_close(runtime);
    Ok(())
}

fn parse_image_name(name: &str) -> Option<ServiceImageId> {
    match name {
        "settings" => Some(ServiceImageId::SettingsApp),
        "files" => Some(ServiceImageId::FilesApp),
        "monitor" => Some(ServiceImageId::MonitorApp),
        "terminal" => Some(ServiceImageId::TerminalApp),
        "software" | "store" => Some(ServiceImageId::SoftwareCenterApp),
        "sysinfo" => Some(ServiceImageId::SysinfoTool),
        "runtime-host" => Some(ServiceImageId::PosixHostTool),
        "cross-builder" => Some(ServiceImageId::CrossBuilderTool),
        _ => None,
    }
}

fn image_name(image_id: ServiceImageId) -> &'static str {
    match image_id {
        ServiceImageId::SettingsApp => "settings",
        ServiceImageId::FilesApp => "files",
        ServiceImageId::MonitorApp => "monitor",
        ServiceImageId::TerminalApp => "terminal",
        ServiceImageId::SoftwareCenterApp => "software",
        ServiceImageId::SysinfoTool => "sysinfo",
        ServiceImageId::PosixHostTool => "runtime-host",
        ServiceImageId::CrossBuilderTool => "cross-builder",
        ServiceImageId::SecurityService => "security-service",
        _ => "unknown",
    }
}

fn policy_name(policy: PermissionPolicyState) -> &'static str {
    match policy {
        PermissionPolicyState::Allowed => "allowed",
        PermissionPolicyState::Blocked => "blocked",
        PermissionPolicyState::DefaultAllow => "default",
    }
}

fn permission_summary(bits: u32) -> PermissionSummary {
    PermissionSummary(bits)
}

struct PermissionSummary(u32);

impl core::fmt::Display for PermissionSummary {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut first = true;
        for (name, bit) in [
            ("config", rt::app_permission::CONFIG),
            ("storage", rt::app_permission::STORAGE),
            ("status", rt::app_permission::STATUS),
            ("package", rt::app_permission::PACKAGE),
            ("network", rt::app_permission::NETWORK),
            ("audio", rt::app_permission::AUDIO),
            ("terminal", rt::app_permission::TERMINAL),
            ("clipboard", rt::app_permission::CLIPBOARD),
        ] {
            if self.0 & bit == 0 {
                continue;
            }
            if !first {
                f.write_str(",")?;
            }
            first = false;
            f.write_str(name)?;
        }
        if first {
            f.write_str("-")?;
        }
        Ok(())
    }
}

fn runtime_cap_summary(bits: u32) -> RuntimeCapSummary {
    RuntimeCapSummary(bits)
}

struct RuntimeCapSummary(u32);

impl core::fmt::Display for RuntimeCapSummary {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut first = true;
        for (name, bit) in [
            ("file-read", rt::runtime_capability::FILE_READ),
            ("terminal-io", rt::runtime_capability::TERMINAL_IO),
            ("network", rt::runtime_capability::NETWORK),
            ("graphics", rt::runtime_capability::GRAPHICS),
            ("audio", rt::runtime_capability::AUDIO),
        ] {
            if self.0 & bit == 0 {
                continue;
            }
            if !first {
                f.write_str(",")?;
            }
            first = false;
            f.write_str(name)?;
        }
        if first {
            f.write_str("-")?;
        }
        Ok(())
    }
}

fn runtime_kind_name(kind: rt::RuntimeKind) -> &'static str {
    match kind {
        rt::RuntimeKind::Windows => "windows",
        rt::RuntimeKind::Posix => "posix",
    }
}

fn runtime_env_state_name(state: rt::RuntimeEnvState) -> &'static str {
    match state {
        rt::RuntimeEnvState::Ready => "ready",
        rt::RuntimeEnvState::Busy => "busy",
        rt::RuntimeEnvState::Destroyed => "destroyed",
        rt::RuntimeEnvState::PendingApproval => "pending-approval",
        rt::RuntimeEnvState::Denied => "denied",
    }
}

fn audit_kind_name(kind: rt::SecurityAuditKind) -> &'static str {
    match kind {
        rt::SecurityAuditKind::PolicyChanged => "policy-changed",
        rt::SecurityAuditKind::LaunchDenied => "launch-denied",
        rt::SecurityAuditKind::RuntimeApprovalRequested => "runtime-approval-requested",
        rt::SecurityAuditKind::RuntimeApprovalChanged => "runtime-approval-changed",
    }
}

fn package_trust_name(value: rt::PackageTrustState) -> &'static str {
    match value {
        rt::PackageTrustState::BootTrusted => "boot-trusted",
        rt::PackageTrustState::DigestPinned => "digest-pinned",
        rt::PackageTrustState::Unverified => "unverified",
        rt::PackageTrustState::VerificationFailed => "verification-failed",
    }
}

fn repo_trust_name(value: rt::PackageRepositoryTrustMode) -> &'static str {
    match value {
        rt::PackageRepositoryTrustMode::Boot => "boot",
        rt::PackageRepositoryTrustMode::Unsigned => "unsigned",
        rt::PackageRepositoryTrustMode::PinnedDigest => "pinned-digest",
    }
}

fn repo_sync_name(value: rt::PackageRepositorySyncState) -> &'static str {
    match value {
        rt::PackageRepositorySyncState::Idle => "idle",
        rt::PackageRepositorySyncState::Ready => "ready",
        rt::PackageRepositorySyncState::Offline => "offline",
        rt::PackageRepositorySyncState::Failed => "failed",
    }
}

fn channel_name(value: rt::PackageChannel) -> &'static str {
    match value {
        rt::PackageChannel::Stable => "stable",
        rt::PackageChannel::Beta => "beta",
        rt::PackageChannel::Canary => "canary",
    }
}

fn ring_name(value: rt::PackageRing) -> &'static str {
    match value {
        rt::PackageRing::Production => "production",
        rt::PackageRing::Preview => "preview",
        rt::PackageRing::Testing => "testing",
    }
}

use crate::{
    channel_create, channel_receive_blocking, channel_send, handle_close, rights, Error, Handle,
    PermissionPolicyState, RawMessage, Result, SecurityAuditInfo,
    SecurityAuditKind, SecurityAppPolicyInfo, SecurityStatus, SecurityTag, ServiceId, ServiceImageId,
};

pub fn security_lookup(bootstrap: Handle) -> Result<Handle> {
    crate::lookup_service(bootstrap, ServiceId::Security)
}

pub fn security_policy_list(
    security_handle: Handle,
    index: usize,
) -> Result<Option<SecurityAppPolicyInfo>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SecurityTag::PolicyListRequest as u32);
    request.word_count = 1;
    request.words[0] = index as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(security_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SecurityTag::PolicyListReply as u32 || response.word_count < 6 {
        return Err(Error::InvalidArgument);
    }
    match security_status_from_word(response.words[0]) {
        SecurityStatus::Ok => {
            let name_len = response.words[5] as usize;
            if name_len > 64 {
                return Err(Error::BufferTooSmall);
            }
            let mut name = [0u8; 64];
            crate::unpack_bytes(&response.words[6..response.word_count as usize], name_len, &mut name)?;
            Ok(Some(SecurityAppPolicyInfo {
                image_id: image_id_from_word(response.words[1]),
                permissions: response.words[2] as u32,
                policy: permission_policy_state_from_word(response.words[3]),
                sensitive_permissions: response.words[4] as u32,
                name_len: name_len as u32,
                name,
            }))
        }
        SecurityStatus::NotFound => Ok(None),
        SecurityStatus::Denied => Err(Error::PermissionDenied),
        SecurityStatus::Busy => Err(Error::Busy),
    }
}

pub fn security_policy_info(
    security_handle: Handle,
    image_id: ServiceImageId,
) -> Result<SecurityAppPolicyInfo> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SecurityTag::PolicyInfoRequest as u32);
    request.word_count = 1;
    request.words[0] = image_id as u32 as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(security_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SecurityTag::PolicyInfoReply as u32 || response.word_count < 6 {
        return Err(Error::InvalidArgument);
    }
    match security_status_from_word(response.words[0]) {
        SecurityStatus::Ok => {
            let name_len = response.words[5] as usize;
            if name_len > 64 {
                return Err(Error::BufferTooSmall);
            }
            let mut name = [0u8; 64];
            crate::unpack_bytes(&response.words[6..response.word_count as usize], name_len, &mut name)?;
            Ok(SecurityAppPolicyInfo {
                image_id: image_id_from_word(response.words[1]),
                permissions: response.words[2] as u32,
                policy: permission_policy_state_from_word(response.words[3]),
                sensitive_permissions: response.words[4] as u32,
                name_len: name_len as u32,
                name,
            })
        }
        SecurityStatus::NotFound => Err(Error::NotFound),
        SecurityStatus::Denied => Err(Error::PermissionDenied),
        SecurityStatus::Busy => Err(Error::Busy),
    }
}

pub fn security_policy_set(
    security_handle: Handle,
    image_id: ServiceImageId,
    policy: PermissionPolicyState,
) -> Result<()> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SecurityTag::PolicySetRequest as u32);
    request.word_count = 2;
    request.words[0] = image_id as u32 as u64;
    request.words[1] = policy as u32 as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(security_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SecurityTag::PolicySetReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match security_status_from_word(response.words[0]) {
        SecurityStatus::Ok => Ok(()),
        SecurityStatus::NotFound => Err(Error::NotFound),
        SecurityStatus::Denied => Err(Error::PermissionDenied),
        SecurityStatus::Busy => Err(Error::Busy),
    }
}

pub fn security_audit_list(
    security_handle: Handle,
    index: usize,
) -> Result<Option<SecurityAuditInfo>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SecurityTag::AuditListRequest as u32);
    request.word_count = 1;
    request.words[0] = index as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(security_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SecurityTag::AuditListReply as u32 || response.word_count < 5 {
        return Err(Error::InvalidArgument);
    }
    match security_status_from_word(response.words[0]) {
        SecurityStatus::Ok => Ok(Some(SecurityAuditInfo {
            sequence: response.words[1] as u32,
            kind: security_audit_kind_from_word(response.words[2]),
            subject_image_id: image_id_from_word(response.words[3]),
            policy: permission_policy_state_from_word(response.words[4]),
            detail: if response.word_count > 5 { response.words[5] } else { 0 },
        })),
        SecurityStatus::NotFound => Ok(None),
        SecurityStatus::Denied => Err(Error::PermissionDenied),
        SecurityStatus::Busy => Err(Error::Busy),
    }
}

fn security_status_from_word(value: u64) -> SecurityStatus {
    match value as u32 {
        x if x == SecurityStatus::NotFound as u32 => SecurityStatus::NotFound,
        x if x == SecurityStatus::Busy as u32 => SecurityStatus::Busy,
        x if x == SecurityStatus::Denied as u32 => SecurityStatus::Denied,
        _ => SecurityStatus::Ok,
    }
}

fn permission_policy_state_from_word(value: u64) -> PermissionPolicyState {
    match value as u32 {
        x if x == PermissionPolicyState::Allowed as u32 => PermissionPolicyState::Allowed,
        x if x == PermissionPolicyState::Blocked as u32 => PermissionPolicyState::Blocked,
        _ => PermissionPolicyState::DefaultAllow,
    }
}

fn security_audit_kind_from_word(value: u64) -> SecurityAuditKind {
    match value as u32 {
        x if x == SecurityAuditKind::LaunchDenied as u32 => SecurityAuditKind::LaunchDenied,
        x if x == SecurityAuditKind::RuntimeApprovalRequested as u32 => SecurityAuditKind::RuntimeApprovalRequested,
        x if x == SecurityAuditKind::RuntimeApprovalChanged as u32 => SecurityAuditKind::RuntimeApprovalChanged,
        _ => SecurityAuditKind::PolicyChanged,
    }
}

fn image_id_from_word(value: u64) -> ServiceImageId {
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
        x if x == ServiceImageId::DesktopShellService as u32 => ServiceImageId::DesktopShellService,
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

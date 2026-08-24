#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

use core::{fmt::Write, str};

use rt::{
    ControlTag, LifecycleEvent, LogDomain, LogEvent, LogSeverity, PermissionPolicyState,
    RawMessage, SecurityAuditKind, SecurityStatus, SecurityTag, ServiceId, ServiceImageId,
    app_permission,
};
use serviceos_userspace_runtime as rt;

const MAX_POLICIES: usize = 8;
const MAX_AUDIT: usize = 24;
const MAX_STATE_BYTES: usize = 256;
const STATE_DIR: &str = "state";
const SECURITY_DIR: &str = "security";
const POLICY_FILE: &str = "launch-policy.cfg";

#[derive(Clone, Copy)]
struct PolicyEntry {
    image_id: ServiceImageId,
    name: &'static str,
    permissions: u32,
    sensitive: u32,
}

const POLICIES: [PolicyEntry; MAX_POLICIES] = [
    PolicyEntry {
        image_id: ServiceImageId::SettingsApp,
        name: "settings",
        permissions: app_permission::CONFIG | app_permission::NETWORK | app_permission::AUDIO,
        sensitive: app_permission::NETWORK | app_permission::AUDIO,
    },
    PolicyEntry {
        image_id: ServiceImageId::FilesApp,
        name: "files",
        permissions: app_permission::STORAGE,
        sensitive: app_permission::STORAGE,
    },
    PolicyEntry {
        image_id: ServiceImageId::MonitorApp,
        name: "monitor",
        permissions: app_permission::STATUS | app_permission::NETWORK,
        sensitive: app_permission::NETWORK,
    },
    PolicyEntry {
        image_id: ServiceImageId::TerminalApp,
        name: "terminal",
        permissions: app_permission::TERMINAL | app_permission::CLIPBOARD,
        sensitive: app_permission::CLIPBOARD,
    },
    PolicyEntry {
        image_id: ServiceImageId::SoftwareCenterApp,
        name: "software",
        permissions: app_permission::PACKAGE,
        sensitive: app_permission::PACKAGE,
    },
    PolicyEntry {
        image_id: ServiceImageId::SysinfoTool,
        name: "sysinfo",
        permissions: app_permission::TERMINAL,
        sensitive: 0,
    },
    PolicyEntry {
        image_id: ServiceImageId::PosixHostTool,
        name: "runtime-host",
        permissions: app_permission::TERMINAL | app_permission::STORAGE,
        sensitive: app_permission::STORAGE,
    },
    PolicyEntry {
        image_id: ServiceImageId::CrossBuilderTool,
        name: "cross-builder",
        permissions: app_permission::STORAGE | app_permission::TERMINAL,
        sensitive: app_permission::STORAGE,
    },
];

#[derive(Clone, Copy)]
struct AuditSlot {
    occupied: bool,
    sequence: u32,
    kind: SecurityAuditKind,
    subject: ServiceImageId,
    policy: PermissionPolicyState,
    detail: u64,
}

impl AuditSlot {
    const fn empty() -> Self {
        Self {
            occupied: false,
            sequence: 0,
            kind: SecurityAuditKind::PolicyChanged,
            subject: ServiceImageId::RootManager,
            policy: PermissionPolicyState::DefaultAllow,
            detail: 0,
        }
    }
}

#[cfg(not(test))]
rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xfd21;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 1 {
        return 0xfd22;
    }

    let log_handle = startup.handles[0];
    let storage_handle = match rt::lookup_service(bootstrap, ServiceId::Storage) {
        Ok(handle) => handle,
        Err(_) => return 0xfd23,
    };

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xfd24,
    };
    if rt::register_service(bootstrap, ServiceId::Security, public.second).is_err() {
        return 0xfd25;
    }
    let _ = rt::handle_close(public.second);

    let mut policy_states = [PermissionPolicyState::DefaultAllow; MAX_POLICIES];
    let mut audits = [AuditSlot::empty(); MAX_AUDIT];
    let mut next_sequence = 1u32;
    let _ = load_policy_state(storage_handle, &mut policy_states);

    loop {
        let mut did_work = false;
        if poll_lifecycle(bootstrap).unwrap_or(false) {
            let _ = rt::handle_close(storage_handle);
            return 0;
        }

        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                did_work = true;
                if handle_request(
                    storage_handle,
                    log_handle,
                    &mut policy_states,
                    &mut audits,
                    &mut next_sequence,
                    &request,
                )
                .is_err()
                {
                    return 0xfd26;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xfd27,
        }

        if !did_work && rt::yield_current().is_err() {
            return 0xfd28;
        }
    }
}

fn handle_request(
    storage_handle: rt::Handle,
    log_handle: rt::Handle,
    policy_states: &mut [PermissionPolicyState; MAX_POLICIES],
    audits: &mut [AuditSlot; MAX_AUDIT],
    next_sequence: &mut u32,
    request: &RawMessage,
) -> rt::Result<()> {
    match request.tag {
        x if x == SecurityTag::PolicyListRequest as u32 => {
            if request.handle_count < 1 || request.word_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let index = request.words[0] as usize;
            let mut reply = RawMessage::empty(SecurityTag::PolicyListReply as u32);
            reply.word_count = 6;
            if let Some(entry) = POLICIES.get(index).copied() {
                encode_policy_reply(&mut reply, entry, policy_states[index])?;
            } else {
                reply.words[0] = SecurityStatus::NotFound as u32 as u64;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == SecurityTag::PolicyInfoRequest as u32 => {
            if request.handle_count < 1 || request.word_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let image_id = image_id_from_word(request.words[0]);
            let mut reply = RawMessage::empty(SecurityTag::PolicyInfoReply as u32);
            reply.word_count = 6;
            if let Some(index) = policy_index(image_id) {
                encode_policy_reply(&mut reply, POLICIES[index], policy_states[index])?;
            } else {
                reply.words[0] = SecurityStatus::NotFound as u32 as u64;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == SecurityTag::PolicySetRequest as u32 => {
            if request.handle_count < 1 || request.word_count < 2 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let image_id = image_id_from_word(request.words[0]);
            let policy = policy_from_word(request.words[1]);
            let mut reply = RawMessage::empty(SecurityTag::PolicySetReply as u32);
            reply.word_count = 1;
            if let Some(index) = policy_index(image_id) {
                policy_states[index] = policy;
                let persist_ok = persist_policy_state(storage_handle, policy_states).is_ok();
                record_audit(
                    audits,
                    next_sequence,
                    SecurityAuditKind::PolicyChanged,
                    image_id,
                    policy,
                    0,
                );
                let _ = emit_log(
                    log_handle,
                    if persist_ok {
                        LogSeverity::Info
                    } else {
                        LogSeverity::Warn
                    },
                    LogEvent::SecurityPolicyChanged,
                    image_id as u32 as u64,
                    (policy as u32 as u64) | ((u64::from(persist_ok)) << 32),
                );
                reply.words[0] = SecurityStatus::Ok as u32 as u64;
            } else {
                reply.words[0] = SecurityStatus::NotFound as u32 as u64;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == SecurityTag::AuditListRequest as u32 => {
            if request.handle_count < 1 || request.word_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let index = request.words[0] as usize;
            let filter = if request.word_count >= 2 {
                Some(request.words[1])
            } else {
                None
            };
            let mut reply = RawMessage::empty(SecurityTag::AuditListReply as u32);
            reply.word_count = 6;
            if let Some(entry) = select_audit(audits, index, filter) {
                reply.words[0] = SecurityStatus::Ok as u32 as u64;
                reply.words[1] = entry.sequence as u64;
                reply.words[2] = entry.kind as u32 as u64;
                reply.words[3] = entry.subject as u32 as u64;
                reply.words[4] = entry.policy as u32 as u64;
                reply.words[5] = entry.detail;
            } else {
                reply.words[0] = SecurityStatus::NotFound as u32 as u64;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        _ => {}
    }
    Ok(())
}

fn encode_policy_reply(
    reply: &mut RawMessage,
    entry: PolicyEntry,
    policy: PermissionPolicyState,
) -> rt::Result<()> {
    reply.words[0] = SecurityStatus::Ok as u32 as u64;
    reply.words[1] = entry.image_id as u32 as u64;
    reply.words[2] = entry.permissions as u64;
    reply.words[3] = policy as u32 as u64;
    reply.words[4] = entry.sensitive as u64;
    reply.words[5] = entry.name.len() as u64;
    reply.word_count += rt::pack_bytes(entry.name.as_bytes(), &mut reply.words[6..])?;
    Ok(())
}

fn emit_log(
    log_handle: rt::Handle,
    severity: LogSeverity,
    event: LogEvent,
    arg0: u64,
    arg1: u64,
) -> rt::Result<()> {
    rt::send_log_record(
        log_handle,
        ServiceId::Security,
        severity,
        LogDomain::Security,
        event,
        arg0,
        arg1,
    )
}

fn audit_kind_from_word(value: u64) -> Option<SecurityAuditKind> {
    match value as u32 {
        x if x == SecurityAuditKind::PolicyChanged as u32 => Some(SecurityAuditKind::PolicyChanged),
        x if x == SecurityAuditKind::LaunchDenied as u32 => Some(SecurityAuditKind::LaunchDenied),
        x if x == SecurityAuditKind::RuntimeApprovalRequested as u32 => {
            Some(SecurityAuditKind::RuntimeApprovalRequested)
        }
        x if x == SecurityAuditKind::RuntimeApprovalChanged as u32 => {
            Some(SecurityAuditKind::RuntimeApprovalChanged)
        }
        _ => None,
    }
}

fn select_audit(
    audits: &[AuditSlot; MAX_AUDIT],
    index: usize,
    filter: Option<u64>,
) -> Option<AuditSlot> {
    let matches = |entry: &AuditSlot| match filter {
        None => true,
        Some(word) => audit_kind_from_word(word).is_some_and(|expected| expected == entry.kind),
    };
    audits
        .iter()
        .filter(|entry| entry.occupied && matches(entry))
        .nth(index)
        .copied()
}

fn record_audit(
    audits: &mut [AuditSlot; MAX_AUDIT],
    next_sequence: &mut u32,
    kind: SecurityAuditKind,
    subject: ServiceImageId,
    policy: PermissionPolicyState,
    detail: u64,
) {
    let index = audits.iter().position(|entry| !entry.occupied).unwrap_or(0);
    audits[index] = AuditSlot {
        occupied: true,
        sequence: *next_sequence,
        kind,
        subject,
        policy,
        detail,
    };
    *next_sequence = next_sequence.saturating_add(1);
}

fn policy_index(image_id: ServiceImageId) -> Option<usize> {
    POLICIES.iter().position(|entry| entry.image_id == image_id)
}

fn policy_from_word(value: u64) -> PermissionPolicyState {
    match value as u32 {
        x if x == PermissionPolicyState::Allowed as u32 => PermissionPolicyState::Allowed,
        x if x == PermissionPolicyState::Blocked as u32 => PermissionPolicyState::Blocked,
        _ => PermissionPolicyState::DefaultAllow,
    }
}

fn image_id_from_word(value: u64) -> ServiceImageId {
    match value as u32 {
        x if x == ServiceImageId::SettingsApp as u32 => ServiceImageId::SettingsApp,
        x if x == ServiceImageId::FilesApp as u32 => ServiceImageId::FilesApp,
        x if x == ServiceImageId::MonitorApp as u32 => ServiceImageId::MonitorApp,
        x if x == ServiceImageId::TerminalApp as u32 => ServiceImageId::TerminalApp,
        x if x == ServiceImageId::SoftwareCenterApp as u32 => ServiceImageId::SoftwareCenterApp,
        x if x == ServiceImageId::SysinfoTool as u32 => ServiceImageId::SysinfoTool,
        x if x == ServiceImageId::PosixHostTool as u32 => ServiceImageId::PosixHostTool,
        x if x == ServiceImageId::CrossBuilderTool as u32 => ServiceImageId::CrossBuilderTool,
        x if x == ServiceImageId::SecurityService as u32 => ServiceImageId::SecurityService,
        _ => ServiceImageId::RootManager,
    }
}

fn load_policy_state(
    storage_handle: rt::Handle,
    policy_states: &mut [PermissionPolicyState; MAX_POLICIES],
) -> rt::Result<()> {
    let path = "state/security/launch-policy.cfg";
    let (blob, len) = rt::storage_open(storage_handle, path)?;
    let mut bytes = [0u8; MAX_STATE_BYTES];
    let capacity = bytes.len();
    let loaded = rt::storage_read_all(blob, &mut bytes, len.min(capacity))?;
    let _ = rt::storage_blob_close(blob);
    let text = str::from_utf8(&bytes[..loaded]).map_err(|_| rt::Error::InvalidArgument)?;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Some((id_text, policy_text)) = line.split_once('=') else {
            continue;
        };
        let Ok(image_raw) = id_text.parse::<u32>() else {
            continue;
        };
        let Ok(policy_raw) = policy_text.parse::<u32>() else {
            continue;
        };
        let image = image_id_from_word(image_raw as u64);
        if let Some(index) = policy_index(image) {
            policy_states[index] = policy_from_word(policy_raw as u64);
        }
    }
    Ok(())
}

fn persist_policy_state(
    storage_handle: rt::Handle,
    policy_states: &[PermissionPolicyState; MAX_POLICIES],
) -> rt::Result<()> {
    ensure_state_dirs(storage_handle)?;
    let security_dir = ensure_directory(storage_handle, "state", SECURITY_DIR)?;
    let (file, _) = rt::storage_directory_open_file(security_dir, POLICY_FILE, true, true)?;
    let mut bytes = [0u8; MAX_STATE_BYTES];
    let mut buffer = rt::FixedLogBuffer::<MAX_STATE_BYTES>::new();
    for (index, state) in policy_states.iter().copied().enumerate() {
        let _ = writeln!(
            &mut buffer,
            "{}={}",
            POLICIES[index].image_id as u32, state as u32
        );
    }
    let total = buffer.as_bytes().len().min(bytes.len());
    bytes[..total].copy_from_slice(&buffer.as_bytes()[..total]);
    let mut offset = 0usize;
    while offset < total {
        let chunk_len = (total - offset).min((rt::IPC_MAX_WORDS - 3) * 8);
        let _ = rt::storage_write(file, offset, total, &bytes[offset..offset + chunk_len])?;
        offset += chunk_len;
    }
    let _ = rt::storage_blob_close(file);
    let _ = rt::handle_close(security_dir);
    Ok(())
}

fn ensure_state_dirs(storage_handle: rt::Handle) -> rt::Result<()> {
    let state = ensure_directory(storage_handle, "", STATE_DIR)?;
    let _ = rt::handle_close(state);
    Ok(())
}

fn ensure_directory(
    storage_handle: rt::Handle,
    parent: &str,
    name: &str,
) -> rt::Result<rt::Handle> {
    let mut path = rt::FixedLogBuffer::<64>::new();
    if !parent.is_empty() {
        let _ = write!(&mut path, "{}/{}", parent, name);
    } else {
        let _ = write!(&mut path, "{}", name);
    }
    if let Ok(handle) = rt::storage_open_directory(
        storage_handle,
        core::str::from_utf8(path.as_bytes()).map_err(|_| rt::Error::InvalidArgument)?,
        true,
    ) {
        return Ok(handle);
    }

    let parent_handle = rt::storage_open_directory(storage_handle, parent, true)?;
    rt::storage_directory_create(parent_handle, name, rt::StorageEntryKind::Directory)?;
    let _ = rt::handle_close(parent_handle);
    rt::storage_open_directory(
        storage_handle,
        core::str::from_utf8(path.as_bytes()).map_err(|_| rt::Error::InvalidArgument)?,
        true,
    )
}

fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
    let mut message = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut message) {
        Ok(()) if message.tag == ControlTag::Lifecycle as u32 && message.word_count > 0 => {
            Ok(matches!(
                lifecycle_event_from_word(message.words[0]),
                LifecycleEvent::Restarting | LifecycleEvent::Stopped
            ))
        }
        Ok(()) => Ok(false),
        Err(rt::Error::QueueEmpty) => Ok(false),
        Err(error) => Err(error),
    }
}

fn lifecycle_event_from_word(value: u64) -> LifecycleEvent {
    match value as u32 {
        x if x == LifecycleEvent::Starting as u32 => LifecycleEvent::Starting,
        x if x == LifecycleEvent::Ready as u32 => LifecycleEvent::Ready,
        x if x == LifecycleEvent::Failed as u32 => LifecycleEvent::Failed,
        x if x == LifecycleEvent::Stopped as u32 => LifecycleEvent::Stopped,
        _ => LifecycleEvent::Restarting,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(kind: SecurityAuditKind, sequence: u32) -> AuditSlot {
        let mut entry = AuditSlot::empty();
        entry.occupied = true;
        entry.kind = kind;
        entry.sequence = sequence;
        entry
    }

    #[test]
    fn audit_roundtrip_preserves_entries() {
        let mut audits = [AuditSlot::empty(); MAX_AUDIT];
        let mut next = 1u32;
        record_audit(
            &mut audits,
            &mut next,
            SecurityAuditKind::PolicyChanged,
            ServiceImageId::SettingsApp,
            PermissionPolicyState::Blocked,
            0,
        );
        record_audit(
            &mut audits,
            &mut next,
            SecurityAuditKind::LaunchDenied,
            ServiceImageId::TerminalApp,
            PermissionPolicyState::DefaultAllow,
            42,
        );
        let first = select_audit(&audits, 0, None).expect("first entry");
        assert!(matches!(first.kind, SecurityAuditKind::PolicyChanged));
        assert_eq!(first.sequence, 1);
        assert!(matches!(first.subject, ServiceImageId::SettingsApp));
        assert!(matches!(first.policy, PermissionPolicyState::Blocked));
        let second = select_audit(&audits, 1, None).expect("second entry");
        assert!(matches!(second.kind, SecurityAuditKind::LaunchDenied));
        assert_eq!(second.detail, 42);
        assert!(select_audit(&audits, 2, None).is_none());
        assert_eq!(next, 3);
    }

    #[test]
    fn audit_kind_filter_skips_non_matching_entries() {
        let mut audits = [AuditSlot::empty(); MAX_AUDIT];
        audits[0] = slot(SecurityAuditKind::LaunchDenied, 1);
        audits[2] = slot(SecurityAuditKind::RuntimeApprovalRequested, 2);
        audits[3] = slot(SecurityAuditKind::RuntimeApprovalChanged, 3);
        let changed_word = SecurityAuditKind::RuntimeApprovalChanged as u32 as u64;
        let filtered = select_audit(&audits, 0, Some(changed_word)).expect("filtered entry");
        assert_eq!(filtered.sequence, 3);
        assert!(select_audit(&audits, 1, Some(changed_word)).is_none());
        assert!(select_audit(&audits, 0, Some(9999)).is_none());
        assert_eq!(
            select_audit(&audits, 0, None).expect("unfiltered").sequence,
            1
        );
    }

    #[test]
    fn audit_kind_from_word_decodes_discriminants() {
        for kind in [
            SecurityAuditKind::PolicyChanged,
            SecurityAuditKind::LaunchDenied,
            SecurityAuditKind::RuntimeApprovalRequested,
            SecurityAuditKind::RuntimeApprovalChanged,
        ] {
            assert!(audit_kind_from_word(kind as u32 as u64).is_some());
        }
        assert!(audit_kind_from_word(9999).is_none());
    }

    #[test]
    fn audit_wraparound_overwrites_oldest_slot() {
        let mut audits = [AuditSlot::empty(); MAX_AUDIT];
        let mut next = 1u32;
        for _ in 0..=MAX_AUDIT {
            record_audit(
                &mut audits,
                &mut next,
                SecurityAuditKind::PolicyChanged,
                ServiceImageId::RootManager,
                PermissionPolicyState::DefaultAllow,
                0,
            );
        }
        assert!(audits.iter().all(|entry| entry.occupied));
        assert_eq!(audits[0].sequence, MAX_AUDIT as u32 + 1);
    }
}

//! Per-env-kind policy defaults (roadmap row 204 residual). Operators can
//! configure a default decision per environment kind — `Ask` (today's
//! pending-approval card flow), `AllowAll` (auto-approve the full sensitive
//! mask), or `DenyAll` (auto-deny) — that runtime-service applies at the
//! point a decision would be awaited: environment creation and the boot-time
//! rehydrate of persisted `PendingApproval` records. Policy-derived verdicts
//! never enter the pending-approval queue, so the desktop prompt card is not
//! created for them (the card mirrors genuinely-pending envs only).
//!
//! State lives in the cross-reboot envstore (`environments.cfg`, additive
//! `policy <kind> <default>` section lines; see `envstore.rs`). The default
//! table is all-`Ask`, which reproduces today's behavior byte-for-byte and
//! writes nothing to the store.
//!
//! Additive audit contract: policy-derived verdicts record the ordinary
//! `RuntimeApprovalChanged` audit with the granted mask in the detail word's
//! low 32 bits and bit 63 set as the POLICY-DERIVED marker, so history
//! distinguishes them from operator decisions without moving any existing
//! field.

use rt::{
    LogEvent, LogSeverity, PermissionPolicyState, RawMessage, RuntimeKind, RuntimeStatus,
    SecurityAuditKind,
};
use serviceos_userspace_runtime as rt;

use crate::{
    consts::{ENV_POLICY_GET_REPLY_TAG, ENV_POLICY_SET_REPLY_TAG, MAX_AUDIT, MAX_ENVS},
    envstore,
    types::{AuditSlot, EnvSlot},
    util::{emit_log, now_tick, sensitive_capabilities},
};

/// Additive detail-word marker (bit 31): the audit record was derived from
/// the per-kind policy default, not an operator decision. The AuditListReply
/// packs the detail as `policy | detail << 32`, so the marker lives at the
/// top of the 32-bit detail window that survives the reply (the granted mask
/// occupies bits 0..4 and is untouched).
pub(crate) const AUDIT_DETAIL_POLICY_DERIVED: u64 = 1 << 31;

/// Per-env-kind default decision. Wire/store word values are pinned:
/// 0 = ask, 1 = allow-all, 2 = deny-all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum EnvPolicyDefault {
    Ask,
    AllowAll,
    DenyAll,
}

impl EnvPolicyDefault {
    pub(crate) const fn word(self) -> u64 {
        match self {
            EnvPolicyDefault::Ask => 0,
            EnvPolicyDefault::AllowAll => 1,
            EnvPolicyDefault::DenyAll => 2,
        }
    }

    pub(crate) fn from_word(word: u64) -> Option<Self> {
        match word {
            0 => Some(EnvPolicyDefault::Ask),
            1 => Some(EnvPolicyDefault::AllowAll),
            2 => Some(EnvPolicyDefault::DenyAll),
            _ => None,
        }
    }

    /// The PermissionPolicyState an enforced decision audited with (mirrors
    /// the operator decision the default stands in for).
    pub(crate) const fn audit_state(self) -> PermissionPolicyState {
        match self {
            EnvPolicyDefault::Ask => PermissionPolicyState::DefaultAllow,
            EnvPolicyDefault::AllowAll => PermissionPolicyState::Allowed,
            EnvPolicyDefault::DenyAll => PermissionPolicyState::Blocked,
        }
    }
}

/// The policy table: one default per env kind, indexed by kind word - 1
/// (posix = 0, windows = 1). Defaults to all-`Ask`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct PolicyTable {
    defaults: [EnvPolicyDefault; 2],
}

impl PolicyTable {
    pub(crate) const fn new() -> Self {
        Self {
            defaults: [EnvPolicyDefault::Ask; 2],
        }
    }

    fn index(kind: RuntimeKind) -> Option<usize> {
        match kind {
            RuntimeKind::Posix => Some(0),
            RuntimeKind::Windows => Some(1),
        }
    }

    pub(crate) fn default_for(&self, kind: RuntimeKind) -> EnvPolicyDefault {
        Self::index(kind).map_or(EnvPolicyDefault::Ask, |index| self.defaults[index])
    }

    /// Sets the default for `kind`; returns false for an out-of-range kind
    /// (never expected on the pinned two-kind wire, kept total).
    pub(crate) fn set(&mut self, kind: RuntimeKind, default: EnvPolicyDefault) -> bool {
        match Self::index(kind) {
            Some(index) => {
                self.defaults[index] = default;
                true
            }
            None => false,
        }
    }

    /// True while every kind is still `Ask` (nothing to persist).
    pub(crate) fn is_all_ask(&self) -> bool {
        self.defaults
            .iter()
            .all(|default| matches!(default, EnvPolicyDefault::Ask))
    }

    /// Kinds in wire order, for codec iteration.
    pub(crate) fn kinds() -> [RuntimeKind; 2] {
        [RuntimeKind::Posix, RuntimeKind::Windows]
    }
}

/// Applies the kind default to one environment at the decision-await point.
/// Returns `Some(default)` when the policy resolved a would-be pending
/// approval (the caller audits a policy-derived verdict); `None` leaves the
/// env exactly as the pre-policy flow built it.
pub(crate) fn enforce_at_await(
    env: &mut EnvSlot,
    default: EnvPolicyDefault,
) -> Option<EnvPolicyDefault> {
    let pending = sensitive_capabilities(env.capabilities) & !env.granted_caps;
    if pending == 0 || matches!(default, EnvPolicyDefault::Ask) {
        return None;
    }
    match default {
        EnvPolicyDefault::Ask => None,
        EnvPolicyDefault::AllowAll => {
            // Auto-approve with the full sensitive mask (the same verdict an
            // unmasked operator `Allowed` decision produces).
            env.granted_caps |= pending;
            env.state = rt::RuntimeEnvState::Ready;
            env.sandbox.apply_granted_mask(env.granted_caps);
            env.updated_tick = now_tick();
            Some(default)
        }
        EnvPolicyDefault::DenyAll => {
            // Auto-deny: the same verdict an operator `Blocked` decision
            // produces (grants revoked, state Denied).
            env.granted_caps = 0;
            env.state = rt::RuntimeEnvState::Denied;
            env.sandbox.apply_granted_mask(0);
            env.updated_tick = now_tick();
            Some(default)
        }
    }
}

/// Boot-time enforcement over rehydrated records: a persisted
/// `PendingApproval` env whose kind now carries a non-Ask default is resolved
/// before anything can surface a prompt card for it. Returns true when any
/// record changed (the caller persists when the store is writable). Audits
/// are recorded so the boot-local history discloses the policy-derived
/// verdicts.
pub(crate) fn enforce_rehydrated(
    envs: &mut [EnvSlot; MAX_ENVS],
    audits: &mut [AuditSlot; MAX_AUDIT],
    next_audit_sequence: &mut u32,
    policy: &PolicyTable,
) -> bool {
    let mut changed = false;
    for (env_id, env) in envs.iter_mut().enumerate() {
        if !env.occupied || env.state != rt::RuntimeEnvState::PendingApproval {
            continue;
        }
        if let Some(default) = enforce_at_await(env, policy.default_for(env.kind)) {
            changed = true;
            record_policy_audit(
                audits,
                next_audit_sequence,
                env_id as u32,
                env.capabilities,
                env.granted_caps,
                default,
            );
        }
    }
    changed
}

/// Records the policy-derived verdict in the audit trail: the ordinary
/// approval-changed record with the granted mask plus the additive
/// POLICY-DERIVED detail marker.
pub(crate) fn record_policy_audit(
    audits: &mut [AuditSlot; MAX_AUDIT],
    next_audit_sequence: &mut u32,
    env_id: u32,
    capabilities: u32,
    granted: u32,
    default: EnvPolicyDefault,
) {
    crate::protocol::envs::record_audit(
        audits,
        next_audit_sequence,
        SecurityAuditKind::RuntimeApprovalChanged,
        env_id,
        capabilities,
        default.audit_state(),
        granted as u64 | AUDIT_DETAIL_POLICY_DERIVED,
    );
}

/// `ENV_POLICY_GET_REQUEST`: words[0] = kind. Reply words:
/// [status, kind echo, default word].
pub(crate) fn handle_env_policy_get(policy: &PolicyTable, message: &RawMessage) {
    if message.handle_count < 1 || message.word_count < 1 {
        return;
    }
    let reply_handle = message.handles[0];
    let mut reply = RawMessage::empty(ENV_POLICY_GET_REPLY_TAG);
    reply.word_count = 3;
    match kind_from_word(message.words[0]) {
        Some(kind) => {
            reply.words[0] = RuntimeStatus::Ok as u32 as u64;
            reply.words[1] = message.words[0];
            reply.words[2] = policy.default_for(kind).word();
        }
        None => reply.words[0] = RuntimeStatus::Unsupported as u32 as u64,
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
}

/// `ENV_POLICY_SET_REQUEST`: words[0] = kind, words[1] = default word.
/// Reply words: [status]. Mutating (write-through via the caller's durable
/// snapshot), and audited as `PolicyChanged` so history shows the change.
pub(crate) fn handle_env_policy_set(
    storage_handle: rt::Handle,
    envstore_writable: bool,
    envs: &[EnvSlot; MAX_ENVS],
    policy: &mut PolicyTable,
    audits: &mut [AuditSlot; MAX_AUDIT],
    next_audit_sequence: &mut u32,
    message: &RawMessage,
) {
    if message.handle_count < 1 || message.word_count < 2 {
        return;
    }
    let reply_handle = message.handles[0];
    let mut reply = RawMessage::empty(ENV_POLICY_SET_REPLY_TAG);
    reply.word_count = 1;
    match (
        kind_from_word(message.words[0]),
        EnvPolicyDefault::from_word(message.words[1]),
    ) {
        (Some(kind), Some(default)) => {
            policy.set(kind, default);
            crate::protocol::envs::record_audit(
                audits,
                next_audit_sequence,
                SecurityAuditKind::PolicyChanged,
                0,
                kind as u32,
                PermissionPolicyState::DefaultAllow,
                default.word(),
            );
            if envstore_writable {
                envstore::persist_envs(storage_handle, envs, policy);
            }
            reply.words[0] = RuntimeStatus::Ok as u32 as u64;
        }
        _ => reply.words[0] = RuntimeStatus::Unsupported as u32 as u64,
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
}

fn kind_from_word(word: u64) -> Option<RuntimeKind> {
    match word as u32 {
        x if x == RuntimeKind::Posix as u32 => Some(RuntimeKind::Posix),
        x if x == RuntimeKind::Windows as u32 => Some(RuntimeKind::Windows),
        _ => None,
    }
}

/// Boot log line for a configured (non-default) policy table; nothing is
/// logged while every kind is `Ask`, keeping fresh boots byte-identical.
pub(crate) fn log_configured(log_handle: rt::Handle, policy: &PolicyTable) {
    if policy.is_all_ask() {
        return;
    }
    let _ = emit_log(
        log_handle,
        LogSeverity::Info,
        LogEvent::SecurityPolicyChanged,
        0,
        policy.default_for(RuntimeKind::Posix).word()
            | (policy.default_for(RuntimeKind::Windows).word() << 8),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::SandboxProfile;
    use rt::runtime_capability;

    fn pending_env(capabilities: u32) -> EnvSlot {
        let mut env = EnvSlot::empty();
        env.occupied = true;
        env.kind = RuntimeKind::Posix;
        env.capabilities = capabilities;
        env.state = rt::RuntimeEnvState::PendingApproval;
        env.sandbox = SandboxProfile::from_masks(capabilities, 0);
        env
    }

    #[test]
    fn default_table_is_all_ask() {
        let policy = PolicyTable::new();
        assert!(policy.is_all_ask());
        assert!(matches!(
            policy.default_for(RuntimeKind::Posix),
            EnvPolicyDefault::Ask
        ));
        assert!(matches!(
            policy.default_for(RuntimeKind::Windows),
            EnvPolicyDefault::Ask
        ));
    }

    #[test]
    fn policy_words_roundtrip_and_reject_unknowns() {
        for default in [
            EnvPolicyDefault::Ask,
            EnvPolicyDefault::AllowAll,
            EnvPolicyDefault::DenyAll,
        ] {
            assert_eq!(EnvPolicyDefault::from_word(default.word()), Some(default));
        }
        assert_eq!(EnvPolicyDefault::from_word(3), None);
        assert_eq!(EnvPolicyDefault::from_word(u64::MAX), None);
    }

    #[test]
    fn table_set_is_kind_scoped() {
        let mut policy = PolicyTable::new();
        assert!(policy.set(RuntimeKind::Posix, EnvPolicyDefault::AllowAll));
        assert!(matches!(
            policy.default_for(RuntimeKind::Posix),
            EnvPolicyDefault::AllowAll
        ));
        assert!(matches!(
            policy.default_for(RuntimeKind::Windows),
            EnvPolicyDefault::Ask
        ));
        assert!(!policy.is_all_ask());
        assert!(policy.set(RuntimeKind::Posix, EnvPolicyDefault::Ask));
        assert!(policy.is_all_ask());
    }

    #[test]
    fn allow_all_policy_auto_approves_full_sensitive_mask() {
        let caps = runtime_capability::FILE_READ
            | runtime_capability::NETWORK
            | runtime_capability::GRAPHICS
            | runtime_capability::AUDIO;
        let mut env = pending_env(caps);
        let enforced = enforce_at_await(&mut env, EnvPolicyDefault::AllowAll);
        assert_eq!(enforced, Some(EnvPolicyDefault::AllowAll));
        assert!(matches!(env.state, rt::RuntimeEnvState::Ready));
        assert_eq!(env.granted_caps, caps & sensitive_capabilities(caps));
        assert!(
            env.sandbox
                .class_granted(crate::sandbox::DeviceClass::Network)
        );
        assert!(!env.sandbox.has_pending_classes());
    }

    #[test]
    fn deny_all_policy_denies_and_revokes() {
        let mut env = pending_env(runtime_capability::NETWORK | runtime_capability::AUDIO);
        env.granted_caps = runtime_capability::NETWORK;
        let enforced = enforce_at_await(&mut env, EnvPolicyDefault::DenyAll);
        assert_eq!(enforced, Some(EnvPolicyDefault::DenyAll));
        assert!(matches!(env.state, rt::RuntimeEnvState::Denied));
        assert_eq!(env.granted_caps, 0);
    }

    #[test]
    fn ask_policy_leaves_the_env_untouched() {
        let mut env = pending_env(runtime_capability::NETWORK);
        let before = env;
        assert_eq!(enforce_at_await(&mut env, EnvPolicyDefault::Ask), None);
        assert_eq!(env.state, before.state);
        assert_eq!(env.granted_caps, before.granted_caps);
    }

    #[test]
    fn policy_is_inert_without_pending_sensitive_caps() {
        // Nothing sensitive requested: no decision was ever awaited, so an
        // AllowAll default must not touch the env (it is Ready already).
        let mut env = pending_env(runtime_capability::FILE_READ);
        env.state = rt::RuntimeEnvState::Ready;
        assert_eq!(enforce_at_await(&mut env, EnvPolicyDefault::AllowAll), None);
        assert!(matches!(env.state, rt::RuntimeEnvState::Ready));

        // A Ready env with grants already in place is also untouched.
        let mut granted = pending_env(runtime_capability::NETWORK);
        granted.state = rt::RuntimeEnvState::Ready;
        granted.granted_caps = runtime_capability::NETWORK;
        assert_eq!(enforce_at_await(&mut env, EnvPolicyDefault::DenyAll), None);
        assert!(matches!(granted.state, rt::RuntimeEnvState::Ready));
    }

    #[test]
    fn rehydrate_enforcement_resolves_pending_records_and_audits() {
        let mut policy = PolicyTable::new();
        policy.set(RuntimeKind::Posix, EnvPolicyDefault::AllowAll);
        policy.set(RuntimeKind::Windows, EnvPolicyDefault::DenyAll);
        let mut envs = [EnvSlot::empty(); MAX_ENVS];
        envs[1] = pending_env(runtime_capability::NETWORK);
        envs[1].kind = RuntimeKind::Posix;
        envs[2] = pending_env(runtime_capability::GRAPHICS);
        envs[2].kind = RuntimeKind::Windows;
        // Ask-kind env stays pending; non-pending envs untouched.
        envs[3] = pending_env(runtime_capability::AUDIO);
        envs[3].kind = RuntimeKind::Posix;
        envs[3].state = rt::RuntimeEnvState::Ready;

        let mut audits = [AuditSlot::empty(); MAX_AUDIT];
        let mut sequence = 1u32;
        assert!(enforce_rehydrated(
            &mut envs,
            &mut audits,
            &mut sequence,
            &policy
        ));
        assert!(matches!(envs[1].state, rt::RuntimeEnvState::Ready));
        assert_eq!(envs[1].granted_caps, runtime_capability::NETWORK);
        assert!(matches!(envs[2].state, rt::RuntimeEnvState::Denied));
        assert!(matches!(envs[3].state, rt::RuntimeEnvState::Ready));
        assert_eq!(sequence, 3);
        assert_eq!(audits[0].env_id, 1);
        assert_eq!(
            audits[0].detail,
            runtime_capability::NETWORK as u64 | AUDIT_DETAIL_POLICY_DERIVED
        );
        assert_eq!(audits[1].env_id, 2);
        assert_eq!(audits[1].detail, AUDIT_DETAIL_POLICY_DERIVED);
    }

    #[test]
    fn rehydrate_enforcement_is_a_noop_under_ask() {
        let mut envs = [EnvSlot::empty(); MAX_ENVS];
        envs[0] = pending_env(runtime_capability::NETWORK);
        let mut audits = [AuditSlot::empty(); MAX_AUDIT];
        let mut sequence = 1u32;
        assert!(!enforce_rehydrated(
            &mut envs,
            &mut audits,
            &mut sequence,
            &PolicyTable::new()
        ));
        assert!(matches!(
            envs[0].state,
            rt::RuntimeEnvState::PendingApproval
        ));
        assert_eq!(sequence, 1);
    }

    #[test]
    fn policy_get_wire_shape_and_unknown_kind() {
        let policy = PolicyTable::new();
        let mut request = RawMessage::empty(crate::consts::ENV_POLICY_GET_REQUEST_TAG);
        request.word_count = 1;
        request.words[0] = RuntimeKind::Posix as u32 as u64;
        // handle_count < 1: silently dropped (house shape guard).
        handle_env_policy_get(&policy, &request);

        request.handle_count = 1;
        request.handles[0] = 9;
        // A bad handle makes the transport fail before decode; the handler
        // must not panic. (No assertion beyond "returned".)
        handle_env_policy_get(&policy, &request);
    }
}

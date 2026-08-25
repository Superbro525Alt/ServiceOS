use core::fmt;

use rt::{PermissionPolicyState, RuntimeEnvState, SecurityAuditKind, ServiceId, ServiceImageId};
use serviceos_userspace_runtime as rt;

use crate::util::{ShellOutput, write_output_linef};

const AUDIT_SCAN_DEPTH: usize = 8;
const RUNTIME_ENV_SCAN_DEPTH: usize = 8;
const MAX_EXPLANATION_LINE: usize = 128;

pub(crate) enum DenialSubject<'a> {
    App {
        name: &'a str,
    },
    StoredImage {
        path: &'a str,
    },
    RuntimeLaunch {
        env_id: u32,
        workload: &'a str,
        path: Option<&'a str>,
    },
}

#[derive(Default)]
pub(crate) struct DenialObservation {
    pub app_policy: Option<PermissionPolicyState>,
    pub env_state: Option<RuntimeEnvState>,
    pub pending_approvals: u32,
    pub audit_launch_denied: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DenialReasonClass {
    AppBlockedByPolicy,
    RuntimePendingApproval,
    RuntimeEnvDenied,
    LaunchAuditDenied,
    Unattributed,
}

impl DenialReasonClass {
    pub(crate) fn class_name(self) -> &'static str {
        match self {
            DenialReasonClass::AppBlockedByPolicy => "app-policy-blocked",
            DenialReasonClass::RuntimePendingApproval => "runtime-pending-approval",
            DenialReasonClass::RuntimeEnvDenied => "runtime-env-denied",
            DenialReasonClass::LaunchAuditDenied => "launch-audit-denied",
            DenialReasonClass::Unattributed => "unattributed",
        }
    }
}

pub(crate) struct DenialExplanation {
    pub class: DenialReasonClass,
    pub reason: &'static str,
    pub missing_authority: &'static str,
    pub review_surface: &'static str,
}

/// Classify why a launch was denied from operator-visible observations only
/// (policy state, runtime environment states, approval queue depth, and the
/// security audit trail). Precedence: an explicit block wins, then a pending
/// runtime approval, then an explicitly denied environment, then a corroborating
/// launch-denied audit record, then an unattributed fallback.
pub(crate) fn classify_denial(
    subject: &DenialSubject<'_>,
    observation: &DenialObservation,
) -> DenialExplanation {
    match subject {
        DenialSubject::App { .. } | DenialSubject::StoredImage { .. } => {
            if observation.app_policy == Some(PermissionPolicyState::Blocked) {
                DenialExplanation {
                    class: DenialReasonClass::AppBlockedByPolicy,
                    reason: "operator security policy blocks this image",
                    missing_authority: "explicit allow decision for this image",
                    review_surface: "security apps",
                }
            } else if observation.env_state == Some(RuntimeEnvState::PendingApproval)
                || observation.pending_approvals > 0
            {
                // Permissions handoff: a launch denial while runtime approvals
                // are outstanding points at the approval queue instead of a
                // bare denial, so the operator can go decide.
                DenialExplanation {
                    class: DenialReasonClass::RuntimePendingApproval,
                    reason: "a runtime environment awaits an operator approval decision",
                    missing_authority: "approval decision for the pending runtime environment",
                    review_surface: "runtime envs",
                }
            } else if observation.audit_launch_denied {
                DenialExplanation {
                    class: DenialReasonClass::LaunchAuditDenied,
                    reason: "manager recorded a launch denial for this workload",
                    missing_authority: "launch authority not granted by current policy",
                    review_surface: "security audit",
                }
            } else {
                DenialExplanation {
                    class: DenialReasonClass::Unattributed,
                    reason: "no matching policy or audit record explains the denial",
                    missing_authority: "unknown; no denial record found",
                    review_surface: "security audit",
                }
            }
        }
        DenialSubject::RuntimeLaunch { .. } => {
            if observation.env_state == Some(RuntimeEnvState::PendingApproval)
                || observation.pending_approvals > 0
            {
                DenialExplanation {
                    class: DenialReasonClass::RuntimePendingApproval,
                    reason: "runtime environment is waiting for an operator approval decision",
                    missing_authority: "approval decision for this runtime environment",
                    review_surface: "runtime envs",
                }
            } else if observation.env_state == Some(RuntimeEnvState::Denied) {
                DenialExplanation {
                    class: DenialReasonClass::RuntimeEnvDenied,
                    reason: "runtime environment authority is set to denied",
                    missing_authority: "approved runtime authority for this environment",
                    review_surface: "security runtimes",
                }
            } else if observation.audit_launch_denied || observation.pending_approvals > 0 {
                DenialExplanation {
                    class: DenialReasonClass::LaunchAuditDenied,
                    reason: "manager recorded a launch denial for this workload",
                    missing_authority: "launch authority not granted by current policy",
                    review_surface: "security audit",
                }
            } else {
                DenialExplanation {
                    class: DenialReasonClass::Unattributed,
                    reason: "no matching runtime state or audit record explains the denial",
                    missing_authority: "unknown; no denial record found",
                    review_surface: "security audit",
                }
            }
        }
    }
}

struct LineWriter<'a> {
    buffer: &'a mut [u8],
    len: usize,
}

impl fmt::Write for LineWriter<'_> {
    fn write_str(&mut self, piece: &str) -> fmt::Result {
        let bytes = piece.as_bytes();
        let remaining = self.buffer.len() - self.len;
        let take = bytes.len().min(remaining);
        self.buffer[self.len..self.len + take].copy_from_slice(&bytes[..take]);
        self.len += take;
        Ok(())
    }
}

fn subject_text(subject: &DenialSubject<'_>) -> heapless_string::String {
    let mut text = heapless_string::String::new();
    match subject {
        DenialSubject::App { name } => {
            let _ = fmt::write(&mut text, format_args!("app {}", name));
        }
        DenialSubject::StoredImage { path } => {
            let _ = fmt::write(&mut text, format_args!("stored image {}", path));
        }
        DenialSubject::RuntimeLaunch {
            env_id,
            workload,
            path,
        } => match path {
            Some(path) => {
                let _ = fmt::write(
                    &mut text,
                    format_args!("runtime workload env{} {} {}", env_id, workload, path),
                );
            }
            None => {
                let _ = fmt::write(
                    &mut text,
                    format_args!("runtime workload env{} {}", env_id, workload),
                );
            }
        },
    }
    text
}

fn next_action_text(
    subject: &DenialSubject<'_>,
    explanation: &DenialExplanation,
) -> heapless_string::String {
    let mut text = heapless_string::String::new();
    match (explanation.class, subject) {
        (DenialReasonClass::AppBlockedByPolicy, DenialSubject::App { name }) => {
            let _ = fmt::write(&mut text, format_args!("security app {} allow", name));
        }
        (DenialReasonClass::AppBlockedByPolicy, _) => {
            text.push_str("security apps");
        }
        (
            DenialReasonClass::RuntimePendingApproval,
            DenialSubject::RuntimeLaunch { env_id, .. },
        ) => {
            let _ = fmt::write(
                &mut text,
                format_args!("security runtime {} approve (queue: runtime envs)", env_id),
            );
        }
        (DenialReasonClass::RuntimePendingApproval, _) => {
            text.push_str("review the queue: runtime envs; decide via security runtime <id> approve");
        }
        (DenialReasonClass::RuntimeEnvDenied, DenialSubject::RuntimeLaunch { env_id, .. }) => {
            let _ = fmt::write(&mut text, format_args!("security runtime {} reset", env_id));
        }
        (DenialReasonClass::RuntimeEnvDenied, _) => {
            text.push_str("security runtimes");
        }
        (_, DenialSubject::App { name }) => {
            let _ = fmt::write(&mut text, format_args!("security app {}", name));
        }
        (_, DenialSubject::StoredImage { .. }) | (_, DenialSubject::RuntimeLaunch { .. }) => {
            text.push_str("security audit 8");
        }
    }
    text
}

mod heapless_string {
    use core::fmt;

    pub(crate) struct String {
        bytes: [u8; 96],
        len: usize,
    }

    impl String {
        pub(crate) const fn new() -> Self {
            Self {
                bytes: [0; 96],
                len: 0,
            }
        }

        pub(crate) fn push_str(&mut self, piece: &str) {
            let bytes = piece.as_bytes();
            let remaining = self.bytes.len() - self.len;
            let take = bytes.len().min(remaining);
            self.bytes[self.len..self.len + take].copy_from_slice(&bytes[..take]);
            self.len += take;
        }

        pub(crate) fn as_str(&self) -> &str {
            match core::str::from_utf8(&self.bytes[..self.len]) {
                Ok(text) => text,
                Err(_) => "?",
            }
        }
    }

    impl fmt::Write for String {
        fn write_str(&mut self, piece: &str) -> fmt::Result {
            self.push_str(piece);
            Ok(())
        }
    }
}

/// Render a structured multi-line denial explanation instead of bare denial
/// text. Lines: denied subject, reason class, human reason, missing authority,
/// the review surface to inspect, and a concrete next action.
pub(crate) fn render_denial_explanation(
    output: ShellOutput,
    subject: &DenialSubject<'_>,
    explanation: &DenialExplanation,
) -> rt::Result<()> {
    let subject_text = subject_text(subject);
    let next_action = next_action_text(subject, explanation);
    write_output_linef(output, format_args!("denied: {}", subject_text.as_str()))?;
    emit_explanation_line(output, "reason-class:", explanation.class.class_name())?;
    emit_explanation_line(output, "reason:", explanation.reason)?;
    emit_explanation_line(output, "missing-authority:", explanation.missing_authority)?;
    emit_explanation_line(output, "review-surface:", explanation.review_surface)?;
    emit_explanation_line(output, "next-action:", next_action.as_str())
}

fn emit_explanation_line(output: ShellOutput, label: &str, value: &str) -> rt::Result<()> {
    let mut buffer = [0u8; MAX_EXPLANATION_LINE];
    let len = {
        let mut writer = LineWriter {
            buffer: &mut buffer,
            len: 0,
        };
        let _ = fmt::write(&mut writer, format_args!("{} {}", label, value));
        writer.len
    };
    let text = core::str::from_utf8(&buffer[..len]).unwrap_or(label);
    write_output_linef(output, format_args!("{}", text))
}

/// Gather the observations a native/stored-image denial explanation needs.
pub(crate) fn observe_native_denial(
    bootstrap: rt::Handle,
    image_id: Option<ServiceImageId>,
) -> DenialObservation {
    let mut observation = DenialObservation::default();
    if let Some(image_id) = image_id {
        observation.app_policy = read_app_policy(bootstrap, image_id);
        observation.audit_launch_denied |= audit_has_launch_denied_for_app(bootstrap, image_id);
    }
    observation.pending_approvals = count_pending_runtime_approvals(bootstrap);
    observation
}

/// Depth of the runtime-service pending-approval queue; drives the
/// permissions-review handoff in launch denial explanations.
fn count_pending_runtime_approvals(bootstrap: rt::Handle) -> u32 {
    let Ok(runtime) = rt::lookup_service(bootstrap, ServiceId::Runtime) else {
        return 0;
    };
    let mut envs = [rt::RuntimeEnvInfo {
        env_id: 0,
        kind: rt::RuntimeKind::Posix,
        state: RuntimeEnvState::Destroyed,
        capabilities: 0,
        mount_count: 0,
        var_count: 0,
        active_runs: 0,
    }; RUNTIME_ENV_SCAN_DEPTH];
    let count = rt::runtime_env_list(runtime, &mut envs).unwrap_or(0);
    let pending = envs
        .iter()
        .take(count)
        .filter(|env| env.state == RuntimeEnvState::PendingApproval)
        .count() as u32;
    let _ = rt::handle_close(runtime);
    pending
}

/// Gather the observations a runtime-workload denial explanation needs.
pub(crate) fn observe_runtime_denial(bootstrap: rt::Handle, env_id: u32) -> DenialObservation {
    let mut observation = DenialObservation::default();
    let runtime = match rt::lookup_service(bootstrap, ServiceId::Runtime) {
        Ok(handle) => handle,
        Err(_) => return observation,
    };
    let mut envs = [rt::RuntimeEnvInfo {
        env_id: 0,
        kind: rt::RuntimeKind::Posix,
        state: RuntimeEnvState::Destroyed,
        capabilities: 0,
        mount_count: 0,
        var_count: 0,
        active_runs: 0,
    }; 8];
    let count = rt::runtime_env_list(runtime, &mut envs).unwrap_or(0);
    for env in envs.iter().take(count).copied() {
        if env.env_id == env_id {
            observation.env_state = Some(env.state);
        }
        if env.state == RuntimeEnvState::PendingApproval {
            observation.pending_approvals += 1;
        }
    }
    let _ = rt::handle_close(runtime);
    observation.audit_launch_denied = audit_has_runtime_launch_denied(bootstrap, env_id);
    observation
}

fn read_app_policy(
    bootstrap: rt::Handle,
    image_id: ServiceImageId,
) -> Option<PermissionPolicyState> {
    let security = rt::security_lookup(bootstrap).ok()?;
    let policy = rt::security_policy_info(security, image_id)
        .ok()
        .map(|info| info.policy);
    let _ = rt::handle_close(security);
    policy
}

fn audit_has_launch_denied_for_app(bootstrap: rt::Handle, image_id: ServiceImageId) -> bool {
    let Some(security) = rt::security_lookup(bootstrap).ok() else {
        return false;
    };
    let mut found = false;
    for index in 0..AUDIT_SCAN_DEPTH {
        let Some(entry) = rt::security_audit_list(security, index).unwrap_or(None) else {
            break;
        };
        if entry.kind == SecurityAuditKind::LaunchDenied && entry.subject_image_id == image_id {
            found = true;
            break;
        }
    }
    let _ = rt::handle_close(security);
    found
}

fn audit_has_runtime_launch_denied(bootstrap: rt::Handle, env_id: u32) -> bool {
    let Ok(runtime) = rt::lookup_service(bootstrap, ServiceId::Runtime) else {
        return false;
    };
    let mut found = false;
    for index in 0..AUDIT_SCAN_DEPTH {
        let Some(entry) = rt::runtime_audit_list(runtime, index).unwrap_or(None) else {
            break;
        };
        if entry.kind == SecurityAuditKind::LaunchDenied && entry.env_id == env_id {
            found = true;
            break;
        }
    }
    let _ = rt::handle_close(runtime);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(
        app_policy: Option<PermissionPolicyState>,
        env_state: Option<RuntimeEnvState>,
        pending_approvals: u32,
        audit_launch_denied: bool,
    ) -> DenialObservation {
        DenialObservation {
            app_policy,
            env_state,
            pending_approvals,
            audit_launch_denied,
        }
    }

    fn app_subject<'a>() -> DenialSubject<'a> {
        DenialSubject::App { name: "terminal" }
    }

    fn runtime_subject<'a>() -> DenialSubject<'a> {
        DenialSubject::RuntimeLaunch {
            env_id: 2,
            workload: "cat",
            path: Some("/etc/hostname"),
        }
    }

    #[test]
    fn blocked_app_policy_maps_to_app_blocked_class() {
        let explained = classify_denial(
            &app_subject(),
            &observation(Some(PermissionPolicyState::Blocked), None, 0, false),
        );
        assert_eq!(explained.class, DenialReasonClass::AppBlockedByPolicy);
        assert_eq!(explained.class.class_name(), "app-policy-blocked");
        assert_eq!(explained.review_surface, "security apps");
    }

    #[test]
    fn explicit_allow_beats_audit_corroboration() {
        let explained = classify_denial(
            &app_subject(),
            &observation(Some(PermissionPolicyState::Allowed), None, 0, true),
        );
        assert_eq!(explained.class, DenialReasonClass::LaunchAuditDenied);
    }

    #[test]
    fn default_allow_without_records_is_unattributed() {
        let explained = classify_denial(
            &app_subject(),
            &observation(Some(PermissionPolicyState::DefaultAllow), None, 0, false),
        );
        assert_eq!(explained.class, DenialReasonClass::Unattributed);
        assert_eq!(explained.review_surface, "security audit");
    }

    #[test]
    fn pending_approval_env_maps_to_pending_class_with_queue_surface() {
        let explained = classify_denial(
            &runtime_subject(),
            &observation(None, Some(RuntimeEnvState::PendingApproval), 1, false),
        );
        assert_eq!(explained.class, DenialReasonClass::RuntimePendingApproval);
        assert_eq!(explained.review_surface, "runtime envs");
        assert!(explained.missing_authority.contains("approval"));
    }

    #[test]
    fn queue_depth_alone_triggers_pending_class() {
        let explained = classify_denial(
            &runtime_subject(),
            &observation(None, Some(RuntimeEnvState::Ready), 2, false),
        );
        assert_eq!(explained.class, DenialReasonClass::RuntimePendingApproval);
    }

    #[test]
    fn pending_beats_denied_environment_state() {
        let explained = classify_denial(
            &runtime_subject(),
            &observation(None, Some(RuntimeEnvState::PendingApproval), 1, true),
        );
        assert_eq!(explained.class, DenialReasonClass::RuntimePendingApproval);
    }

    #[test]
    fn denied_environment_maps_to_env_denied_class() {
        let explained = classify_denial(
            &runtime_subject(),
            &observation(None, Some(RuntimeEnvState::Denied), 0, false),
        );
        assert_eq!(explained.class, DenialReasonClass::RuntimeEnvDenied);
        assert_eq!(explained.class.class_name(), "runtime-env-denied");
    }

    #[test]
    fn runtime_without_signals_is_unattributed() {
        let explained = classify_denial(
            &runtime_subject(),
            &observation(None, Some(RuntimeEnvState::Ready), 0, false),
        );
        assert_eq!(explained.class, DenialReasonClass::Unattributed);
    }

    #[test]
    fn stored_image_relies_on_audit_corroboration() {
        let subject = DenialSubject::StoredImage { path: "demo.img" };
        let explained = classify_denial(&subject, &observation(None, None, 0, true));
        assert_eq!(explained.class, DenialReasonClass::LaunchAuditDenied);
    }

    #[test]
    fn pending_approvals_hand_app_denial_to_review_queue() {
        let subject = app_subject();
        let explained = classify_denial(
            &subject,
            &observation(Some(PermissionPolicyState::DefaultAllow), None, 1, false),
        );
        assert_eq!(explained.class, DenialReasonClass::RuntimePendingApproval);
        assert_eq!(explained.review_surface, "runtime envs");
    }

    #[test]
    fn pending_approvals_hand_stored_image_denial_to_review_queue() {
        let subject = DenialSubject::StoredImage { path: "pkg.img" };
        let explained = classify_denial(&subject, &observation(None, None, 3, false));
        assert_eq!(explained.class, DenialReasonClass::RuntimePendingApproval);
        assert!(explained.missing_authority.contains("approval"));
    }

    #[test]
    fn explicit_block_still_wins_over_pending_approvals() {
        let explained = classify_denial(
            &app_subject(),
            &observation(Some(PermissionPolicyState::Blocked), None, 2, false),
        );
        assert_eq!(explained.class, DenialReasonClass::AppBlockedByPolicy);
    }

    #[test]
    fn app_pending_next_action_points_at_queue_query_and_decision_surface() {
        let subject = app_subject();
        let explained = classify_denial(
            &subject,
            &observation(Some(PermissionPolicyState::DefaultAllow), None, 2, false),
        );
        let next_action = next_action_text(&subject, &explained).as_str().to_string();
        assert!(next_action.contains("runtime envs"));
        assert!(next_action.contains("security runtime"));
        assert!(next_action.contains("approve"));
    }

    #[test]
    fn rendering_emits_structured_lines_for_blocked_app() {
        let subject = app_subject();
        let explained = classify_denial(
            &subject,
            &observation(Some(PermissionPolicyState::Blocked), None, 0, false),
        );
        let subject_text = subject_text(&subject).as_str().to_string();
        let next_action = next_action_text(&subject, &explained).as_str().to_string();
        assert_eq!(subject_text, "app terminal");
        assert_eq!(next_action, "security app terminal allow");
    }

    #[test]
    fn rendering_names_queue_in_runtime_next_action() {
        let subject = runtime_subject();
        let explained = classify_denial(
            &subject,
            &observation(None, Some(RuntimeEnvState::PendingApproval), 1, false),
        );
        let subject_text = subject_text(&subject).as_str().to_string();
        let next_action = next_action_text(&subject, &explained).as_str().to_string();
        assert_eq!(subject_text, "runtime workload env2 cat /etc/hostname");
        assert_eq!(
            next_action,
            "security runtime 2 approve (queue: runtime envs)"
        );
    }

    #[test]
    fn rendering_covers_every_reason_class() {
        let cases = [
            (DenialReasonClass::AppBlockedByPolicy, "app-policy-blocked"),
            (
                DenialReasonClass::RuntimePendingApproval,
                "runtime-pending-approval",
            ),
            (DenialReasonClass::RuntimeEnvDenied, "runtime-env-denied"),
            (DenialReasonClass::LaunchAuditDenied, "launch-audit-denied"),
            (DenialReasonClass::Unattributed, "unattributed"),
        ];
        for (class, name) in cases {
            assert_eq!(class.class_name(), name);
        }
    }
}

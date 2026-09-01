//! Runtime-approval prompt cards: desktop-side mirror of the runtime-service
//! PendingApproval contract. Pure state machine lives here so host tests cover
//! dedup, expiry, capability decoding, and decision routing without handles.

use core::fmt::Write;

use serviceos_userspace_runtime as rt;
use serviceos_userspace_runtime::FixedLogBuffer;

use crate::render::render_desktop;
use crate::{DesktopState, OverlayMode};

use crate::state::{APPROVAL_CARDS_MAX, APPROVAL_PROMPT_TIMEOUT_TICKS, KEY_A, KEY_D};

pub(crate) const APPROVAL_CAPS_TEXT_MAX: usize = 48;
pub(crate) const APPROVAL_NOTICE_TEXT_MAX: usize = 64;

#[derive(Clone, Copy)]
pub(crate) struct ApprovalCard {
    pub(crate) occupied: bool,
    pub(crate) env_id: u32,
    pub(crate) kind: rt::RuntimeKind,
    pub(crate) capabilities: u32,
    pub(crate) first_seen: u64,
    pub(crate) surfaced: bool,
    /// Partial-grant checklist state (additive): when active the card renders
    /// a per-capability selection instead of the A/D strip.
    pub(crate) partial_edit: bool,
    pub(crate) partial_mask: u32,
    pub(crate) partial_cursor: usize,
}

impl ApprovalCard {
    pub(crate) const fn empty() -> Self {
        Self {
            occupied: false,
            env_id: 0,
            kind: rt::RuntimeKind::Posix,
            capabilities: 0,
            first_seen: 0,
            surfaced: false,
            partial_edit: false,
            partial_mask: 0,
            partial_cursor: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ApprovalState {
    pub(crate) cards: [ApprovalCard; APPROVAL_CARDS_MAX],
    pub(crate) len: usize,
}

impl ApprovalState {
    pub(crate) const fn new() -> Self {
        Self {
            cards: [ApprovalCard::empty(); APPROVAL_CARDS_MAX],
            len: 0,
        }
    }

    pub(crate) fn card_index(&self, env_id: u32) -> Option<usize> {
        (0..self.len)
            .find(|index| self.cards[*index].occupied && self.cards[*index].env_id == env_id)
    }

    pub(crate) fn first_card(&self) -> Option<&ApprovalCard> {
        (0..self.len)
            .map(|index| &self.cards[index])
            .find(|card| card.occupied)
    }

    pub(crate) fn pending_beyond_first(&self) -> usize {
        self.len.saturating_sub(1)
    }
}

pub(crate) fn runtime_kind_name(kind: rt::RuntimeKind) -> &'static str {
    match kind {
        rt::RuntimeKind::Posix => "posix",
        rt::RuntimeKind::Windows => "windows",
    }
}

/// The decodable capability classes, in the stable order the shell and
/// settings surfaces render them (shell-service CapabilitySummary list).
pub(crate) const CAPABILITY_CLASSES: [(&str, u32); 5] = [
    ("file-read", rt::runtime_capability::FILE_READ),
    ("terminal-io", rt::runtime_capability::TERMINAL_IO),
    ("network", rt::runtime_capability::NETWORK),
    ("graphics", rt::runtime_capability::GRAPHICS),
    ("audio", rt::runtime_capability::AUDIO),
];

/// Capability mask decoded with the same class names the shell and settings
/// surfaces use (shell-service commands/runtime.rs CapabilitySummary list).
pub(crate) fn capability_names(capabilities: u32) -> FixedLogBuffer<APPROVAL_CAPS_TEXT_MAX> {
    let mut text = FixedLogBuffer::<APPROVAL_CAPS_TEXT_MAX>::new();
    let mut wrote = false;
    for (name, mask) in CAPABILITY_CLASSES {
        if capabilities & mask == 0 {
            continue;
        }
        if wrote {
            let _ = write!(text, ",");
        }
        let _ = write!(text, "{name}");
        wrote = true;
    }
    if !wrote {
        let _ = write!(text, "none");
    }
    text
}

/// Inserts cards for envs in PendingApproval not already tracked; drops cards
/// whose env is no longer pending. Returns true when a brand-new card was
/// inserted (the caller may force the overlay open).
pub(crate) fn sync_pending_cards(
    set: &mut ApprovalState,
    envs: &[rt::RuntimeEnvInfo],
    count: usize,
    now: u64,
) -> bool {
    let mut inserted = false;
    for env in envs.iter().take(count) {
        if env.state != rt::RuntimeEnvState::PendingApproval {
            continue;
        }
        if set.card_index(env.env_id).is_some() {
            continue;
        }
        if set.len >= APPROVAL_CARDS_MAX {
            continue;
        }
        set.cards[set.len] = ApprovalCard {
            occupied: true,
            env_id: env.env_id,
            kind: env.kind,
            capabilities: env.capabilities,
            first_seen: now,
            surfaced: true,
            partial_edit: false,
            partial_mask: 0,
            partial_cursor: 0,
        };
        set.len += 1;
        inserted = true;
    }
    remove_resolved_cards(set, envs, count);
    inserted
}

fn remove_resolved_cards(set: &mut ApprovalState, envs: &[rt::RuntimeEnvInfo], count: usize) {
    let mut index = 0usize;
    while index < set.len {
        let env_id = set.cards[index].env_id;
        let still_pending = envs
            .iter()
            .take(count)
            .any(|env| env.state == rt::RuntimeEnvState::PendingApproval && env.env_id == env_id);
        if still_pending {
            index += 1;
            continue;
        }
        remove_card_at(set, index);
    }
}

fn remove_card_at(set: &mut ApprovalState, index: usize) {
    let limit = set.len.min(APPROVAL_CARDS_MAX);
    for slot in index..limit - 1 {
        set.cards[slot] = set.cards[slot + 1];
    }
    set.cards[limit - 1] = ApprovalCard::empty();
    set.len = limit - 1;
}

/// Expires cards past the bounded prompt window; expired env ids are written
/// into `out` (return count) so the caller can record honest history entries.
pub(crate) fn expire_stale_cards(
    set: &mut ApprovalState,
    now: u64,
    out: &mut [u32; APPROVAL_CARDS_MAX],
) -> usize {
    let mut expired = 0usize;
    let mut index = 0usize;
    while index < set.len {
        let card = set.cards[index];
        if now.saturating_sub(card.first_seen) >= APPROVAL_PROMPT_TIMEOUT_TICKS {
            if expired < out.len() {
                out[expired] = card.env_id;
            }
            expired += 1;
            remove_card_at(set, index);
            continue;
        }
        index += 1;
    }
    expired
}

/// A/D decision keys map onto the exact decision contract the settings
/// security page drives (Allowed / Blocked). Anything else is not a decision.
pub(crate) fn decision_policy(key_code: u32) -> Option<rt::PermissionPolicyState> {
    match key_code {
        KEY_A => Some(rt::PermissionPolicyState::Allowed),
        KEY_D => Some(rt::PermissionPolicyState::Blocked),
        _ => None,
    }
}

/// Capability class at `index` of the requested-capability checklist
/// (mask 0 when the index is beyond the decodable set).
pub(crate) fn partial_class_at(capabilities: u32, index: usize) -> Option<(&'static str, u32)> {
    CAPABILITY_CLASSES
        .iter()
        .filter(|(_, mask)| capabilities & mask != 0)
        .nth(index)
        .copied()
}

pub(crate) fn partial_class_count(capabilities: u32) -> usize {
    CAPABILITY_CLASSES
        .iter()
        .filter(|(_, mask)| capabilities & mask != 0)
        .count()
}

/// Enters the partial-grant checklist on the first card with every requested
/// capability preselected (an inverse of deny: deselect what must not run).
pub(crate) fn begin_partial_edit(set: &mut ApprovalState) -> bool {
    let Some(card) = set.cards.first_mut() else {
        return false;
    };
    if !card.occupied || card.partial_edit {
        return false;
    }
    card.partial_edit = true;
    card.partial_mask = card.capabilities;
    card.partial_cursor = 0;
    true
}

pub(crate) fn partial_editing(set: &ApprovalState) -> bool {
    set.cards
        .first()
        .is_some_and(|card| card.occupied && card.partial_edit)
}

/// Leaves the checklist without deciding (overlay close / re-sync).
pub(crate) fn cancel_partial_edit(set: &mut ApprovalState) {
    if let Some(card) = set.cards.first_mut() {
        card.partial_edit = false;
        card.partial_mask = 0;
        card.partial_cursor = 0;
    }
}

pub(crate) fn partial_toggle_all(set: &mut ApprovalState) {
    let Some(card) = set.cards.first_mut() else {
        return;
    };
    if !card.occupied {
        return;
    }
    if card.partial_mask == card.capabilities {
        card.partial_mask = 0;
    } else {
        card.partial_mask = card.capabilities;
    }
}

pub(crate) fn partial_move_cursor(set: &mut ApprovalState, delta: i32) {
    let Some(card) = set.cards.first_mut() else {
        return;
    };
    if !card.occupied {
        return;
    }
    let count = partial_class_count(card.capabilities);
    if count == 0 {
        return;
    }
    let next = card.partial_cursor as i32 + delta;
    card.partial_cursor = next.clamp(0, count as i32 - 1) as usize;
}

pub(crate) fn partial_toggle_at(set: &mut ApprovalState, index: usize) {
    let Some(card) = set.cards.first_mut() else {
        return;
    };
    if !card.occupied {
        return;
    }
    let Some((_, mask)) = partial_class_at(card.capabilities, index) else {
        return;
    };
    card.partial_mask ^= mask;
}

pub(crate) fn partial_toggle_cursor(set: &mut ApprovalState) {
    let cursor = set
        .cards
        .first()
        .filter(|card| card.occupied)
        .map(|card| card.partial_cursor)
        .unwrap_or(0);
    partial_toggle_at(set, cursor);
}

pub(crate) fn partial_selection(set: &ApprovalState) -> (u32, u32) {
    match set.cards.first() {
        Some(card) if card.occupied => (card.env_id, card.partial_mask & card.capabilities),
        _ => (0, 0),
    }
}

/// Checklist row for the renderer: `[x] name` with the cursor bracket.
pub(crate) fn partial_row_text(
    card: &ApprovalCard,
    index: usize,
) -> Option<FixedLogBuffer<APPROVAL_CAPS_TEXT_MAX>> {
    let (name, mask) = partial_class_at(card.capabilities, index)?;
    let mut text = FixedLogBuffer::<APPROVAL_CAPS_TEXT_MAX>::new();
    let marker = if card.partial_mask & mask != 0 {
        'x'
    } else {
        ' '
    };
    let cursor = if card.partial_cursor == index {
        '>'
    } else {
        ' '
    };
    let _ = write!(text, "{cursor}[{marker}] {name}");
    Some(text)
}

pub(crate) fn partial_notice_text(
    env_id: u32,
    granted: u32,
    outcome: rt::Result<()>,
) -> FixedLogBuffer<APPROVAL_NOTICE_TEXT_MAX> {
    let mut text = FixedLogBuffer::<APPROVAL_NOTICE_TEXT_MAX>::new();
    match outcome {
        Ok(()) => {
            let names = capability_names(granted);
            let _ = write!(text, "env {env_id} partial grant: {}", names.as_str());
        }
        Err(_) => {
            let _ = write!(text, "runtime decision failed (env {env_id})");
        }
    }
    text
}

/// Pure key vocabulary for the checklist so host tests cover routing without
/// handles: A toggles all, digits/up-down+space toggle rows, Enter submits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PartialKeyAction {
    ToggleAll,
    MoveCursor(i32),
    ToggleCursor,
    ToggleIndex(usize),
    Submit,
    Deny,
}

pub(crate) fn partial_key_action(key_code: u32) -> Option<PartialKeyAction> {
    match key_code {
        KEY_A => Some(PartialKeyAction::ToggleAll),
        crate::state::KEY_UP => Some(PartialKeyAction::MoveCursor(-1)),
        crate::state::KEY_DOWN => Some(PartialKeyAction::MoveCursor(1)),
        crate::state::KEY_SPACE => Some(PartialKeyAction::ToggleCursor),
        crate::state::KEY_1 => Some(PartialKeyAction::ToggleIndex(0)),
        crate::state::KEY_2 => Some(PartialKeyAction::ToggleIndex(1)),
        crate::state::KEY_3 => Some(PartialKeyAction::ToggleIndex(2)),
        crate::state::KEY_4 => Some(PartialKeyAction::ToggleIndex(3)),
        crate::state::KEY_5 => Some(PartialKeyAction::ToggleIndex(4)),
        crate::state::KEY_ENTER => Some(PartialKeyAction::Submit),
        KEY_D => Some(PartialKeyAction::Deny),
        _ => None,
    }
}

pub(crate) fn decision_notice_text(
    env_id: u32,
    policy: rt::PermissionPolicyState,
    outcome: rt::Result<()>,
) -> FixedLogBuffer<APPROVAL_NOTICE_TEXT_MAX> {
    let mut text = FixedLogBuffer::<APPROVAL_NOTICE_TEXT_MAX>::new();
    match (policy, outcome) {
        (rt::PermissionPolicyState::Allowed, Ok(())) => {
            let _ = write!(text, "runtime env {env_id} approved");
        }
        (rt::PermissionPolicyState::Blocked, Ok(())) => {
            let _ = write!(text, "runtime env {env_id} denied");
        }
        (_, Err(_)) => {
            let _ = write!(text, "runtime decision failed (env {env_id})");
        }
        (_, Ok(())) => {
            let _ = write!(text, "runtime env {env_id} decided");
        }
    }
    text
}

pub(crate) fn expiry_notice_text(env_id: u32) -> FixedLogBuffer<APPROVAL_NOTICE_TEXT_MAX> {
    let mut text = FixedLogBuffer::<APPROVAL_NOTICE_TEXT_MAX>::new();
    let _ = write!(text, "approval prompt expired (env {env_id}); see settings");
    text
}

/// Marks the visible card "later": no re-nag until a new env id appears.
/// Also leaves any partial-grant checklist so a reopened card starts clean.
pub(crate) fn note_overlay_closed(set: &mut ApprovalState) {
    for index in 0..set.len {
        if set.cards[index].occupied && set.cards[index].surfaced {
            set.cards[index].surfaced = false;
            set.cards[index].partial_edit = false;
            set.cards[index].partial_mask = 0;
            set.cards[index].partial_cursor = 0;
            return;
        }
    }
}

pub(crate) fn remove_card(set: &mut ApprovalState, env_id: u32) {
    if let Some(index) = set.card_index(env_id) {
        remove_card_at(set, index);
    }
}

/// Polls the runtime env list for PendingApproval environments — the same
/// filtered query the settings security page runs — and mirrors them into
/// bounded prompt cards. Runtime-less platforms (no lookup grant) no-op.
pub(crate) fn refresh_runtime_approvals(state: &mut DesktopState, now: u64) -> rt::Result<()> {
    if state.runtime_handle == rt::INVALID_HANDLE {
        return Ok(());
    }
    let mut envs = [rt::RuntimeEnvInfo {
        env_id: 0,
        kind: rt::RuntimeKind::Posix,
        state: rt::RuntimeEnvState::Destroyed,
        capabilities: 0,
        mount_count: 0,
        var_count: 0,
        active_runs: 0,
    }; 8];
    let count = match rt::runtime_env_list(state.runtime_handle, &mut envs) {
        Ok(count) => count,
        Err(_) => return Ok(()),
    };
    let inserted = sync_pending_cards(&mut state.approvals, &envs, count, now);
    let mut expired = [0u32; APPROVAL_CARDS_MAX];
    let expired_count = expire_stale_cards(&mut state.approvals, now, &mut expired);
    for index in 0..expired_count.min(expired.len()) {
        let text = expiry_notice_text(expired[index]);
        let _ =
            crate::windows::post_notification(state, None, false, false, text.as_str().as_bytes());
    }
    if inserted {
        if matches!(
            state.overlay_mode,
            OverlayMode::None | OverlayMode::Approval
        ) {
            state.overlay_mode = OverlayMode::Approval;
        }
        if state.overlay_mode == OverlayMode::Approval {
            render_desktop(state)?;
        }
    }
    Ok(())
}

/// Drives the existing decision contract (EnvDecisionRequest) for the first
/// pending card. Failure keeps the card pending; success records the outcome
/// in the notification history and closes the prompt when none remain.
pub(crate) fn decide_first_card(
    state: &mut DesktopState,
    policy: rt::PermissionPolicyState,
) -> rt::Result<()> {
    let Some(card) = state.approvals.first_card().copied() else {
        return Ok(());
    };
    let outcome = if state.runtime_handle == rt::INVALID_HANDLE {
        Err(rt::Error::PermissionDenied)
    } else {
        rt::runtime_env_decide(state.runtime_handle, card.env_id, policy)
    };
    if outcome.is_ok() {
        remove_card(&mut state.approvals, card.env_id);
        if state.approvals.len == 0 && state.overlay_mode == OverlayMode::Approval {
            state.overlay_mode = OverlayMode::None;
        }
    }
    let text = decision_notice_text(card.env_id, policy, outcome);
    let _ = crate::windows::post_notification(state, None, false, false, text.as_str().as_bytes());
    Ok(())
}

/// Submits the checklist mask through the masked variant of the decision
/// contract (EnvDecisionRequest word 2 = allowed mask). runtime-service
/// grants requested ∩ sensitive; a proper subset keeps the env — and this
/// card — pending, so the card leaves edit mode but stays visible for a
/// follow-up approve-all/deny. A mask covering every requested capability is
/// the full-approve shape and closes the card like A does.
pub(crate) fn decide_first_card_masked(
    state: &mut DesktopState,
    allowed_mask: u32,
) -> rt::Result<()> {
    let Some(card) = state.approvals.first_card().copied() else {
        return Ok(());
    };
    let mask = allowed_mask & card.capabilities;
    let outcome = if state.runtime_handle == rt::INVALID_HANDLE {
        Err(rt::Error::PermissionDenied)
    } else {
        rt::runtime_env_decide_with_mask(
            state.runtime_handle,
            card.env_id,
            rt::PermissionPolicyState::Allowed,
            Some(mask),
        )
    };
    if outcome.is_ok() {
        if mask == card.capabilities {
            remove_card(&mut state.approvals, card.env_id);
            if state.approvals.len == 0 && state.overlay_mode == OverlayMode::Approval {
                state.overlay_mode = OverlayMode::None;
            }
        } else {
            cancel_partial_edit(&mut state.approvals);
        }
    }
    let text = if mask == card.capabilities {
        decision_notice_text(card.env_id, rt::PermissionPolicyState::Allowed, outcome)
    } else {
        partial_notice_text(card.env_id, mask, outcome)
    };
    let _ = crate::windows::post_notification(state, None, false, false, text.as_str().as_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_env(env_id: u32, capabilities: u32) -> rt::RuntimeEnvInfo {
        rt::RuntimeEnvInfo {
            env_id,
            kind: rt::RuntimeKind::Posix,
            state: rt::RuntimeEnvState::PendingApproval,
            capabilities,
            mount_count: 0,
            var_count: 0,
            active_runs: 0,
        }
    }

    fn ready_env(env_id: u32) -> rt::RuntimeEnvInfo {
        rt::RuntimeEnvInfo {
            state: rt::RuntimeEnvState::Ready,
            ..pending_env(env_id, 0)
        }
    }

    #[test]
    fn capability_names_decode_mask() {
        let names =
            capability_names(rt::runtime_capability::FILE_READ | rt::runtime_capability::NETWORK);
        assert_eq!(names.as_str(), "file-read,network");
        assert_eq!(capability_names(0).as_str(), "none");
        assert_eq!(
            capability_names(rt::runtime_capability::AUDIO).as_str(),
            "audio"
        );
    }

    #[test]
    fn new_pending_envs_insert_cards_and_dedup_by_env_id() {
        let mut set = ApprovalState::new();
        let envs = [
            pending_env(3, 0x1),
            pending_env(3, 0x1),
            pending_env(4, 0x4),
        ];
        assert!(sync_pending_cards(&mut set, &envs, 3, 100));
        assert_eq!(set.len, 2);
        assert!(set.card_index(3).is_some());
        assert!(set.card_index(4).is_some());
        assert!(!sync_pending_cards(&mut set, &envs, 3, 110));
        assert_eq!(set.len, 2);
    }

    #[test]
    fn card_capacity_is_bounded() {
        let mut set = ApprovalState::new();
        let envs = [
            pending_env(1, 0),
            pending_env(2, 0),
            pending_env(3, 0),
            pending_env(4, 0),
            pending_env(5, 0),
            pending_env(6, 0),
        ];
        assert!(sync_pending_cards(&mut set, &envs, 6, 0));
        assert_eq!(set.len, crate::state::APPROVAL_CARDS_MAX);
    }

    #[test]
    fn resolved_envs_drop_their_cards() {
        let mut set = ApprovalState::new();
        let envs = [pending_env(3, 0), pending_env(4, 0)];
        let _ = sync_pending_cards(&mut set, &envs, 2, 0);
        let resolved = [ready_env(3), pending_env(4, 0)];
        assert!(!sync_pending_cards(&mut set, &resolved, 2, 10));
        assert_eq!(set.len, 1);
        assert!(set.card_index(3).is_none());
        assert!(set.card_index(4).is_some());
    }

    #[test]
    fn stale_cards_expire_after_bounded_window() {
        let mut set = ApprovalState::new();
        let envs = [pending_env(7, 0), pending_env(8, 0)];
        let _ = sync_pending_cards(&mut set, &envs, 2, 100);
        let mut expired = [0u32; APPROVAL_CARDS_MAX];
        assert_eq!(
            expire_stale_cards(
                &mut set,
                100 + APPROVAL_PROMPT_TIMEOUT_TICKS - 1,
                &mut expired
            ),
            0
        );
        assert_eq!(
            expire_stale_cards(&mut set, 100 + APPROVAL_PROMPT_TIMEOUT_TICKS, &mut expired),
            2
        );
        assert_eq!(expired[0], 7);
        assert_eq!(expired[1], 8);
        assert_eq!(set.len, 0);
    }

    #[test]
    fn decision_keys_route_to_settings_contract_policies() {
        assert_eq!(
            decision_policy(KEY_A),
            Some(rt::PermissionPolicyState::Allowed)
        );
        assert_eq!(
            decision_policy(KEY_D),
            Some(rt::PermissionPolicyState::Blocked)
        );
        assert_eq!(decision_policy(crate::state::KEY_ESC), None);
        assert_eq!(decision_policy(crate::state::KEY_ENTER), None);
    }

    #[test]
    fn decision_notices_record_outcomes_honestly() {
        let approved = decision_notice_text(5, rt::PermissionPolicyState::Allowed, Ok(()));
        assert_eq!(approved.as_str(), "runtime env 5 approved");
        let denied = decision_notice_text(5, rt::PermissionPolicyState::Blocked, Ok(()));
        assert_eq!(denied.as_str(), "runtime env 5 denied");
        let failed =
            decision_notice_text(5, rt::PermissionPolicyState::Allowed, Err(rt::Error::Busy));
        assert_eq!(failed.as_str(), "runtime decision failed (env 5)");
        assert_eq!(
            expiry_notice_text(9).as_str(),
            "approval prompt expired (env 9); see settings"
        );
    }

    #[test]
    fn closing_the_overlay_marks_card_surfaced_false_without_dropping_it() {
        let mut set = ApprovalState::new();
        let envs = [pending_env(2, 0)];
        let _ = sync_pending_cards(&mut set, &envs, 1, 0);
        note_overlay_closed(&mut set);
        assert_eq!(set.len, 1);
        assert!(!set.cards[0].surfaced);
        assert!(!sync_pending_cards(&mut set, &envs, 1, 50));
        assert_eq!(set.len, 1);
    }

    #[test]
    fn kind_names_stay_lower_case() {
        assert_eq!(runtime_kind_name(rt::RuntimeKind::Posix), "posix");
        assert_eq!(runtime_kind_name(rt::RuntimeKind::Windows), "windows");
    }

    #[test]
    fn partial_edit_starts_all_selected_and_clamps_to_requested() {
        let mut set = ApprovalState::new();
        let envs = [pending_env(
            5,
            rt::runtime_capability::NETWORK | rt::runtime_capability::GRAPHICS,
        )];
        let _ = sync_pending_cards(&mut set, &envs, 1, 0);
        assert!(begin_partial_edit(&mut set));
        let (env_id, mask) = partial_selection(&set);
        assert_eq!(env_id, 5);
        assert_eq!(
            mask,
            rt::runtime_capability::NETWORK | rt::runtime_capability::GRAPHICS
        );
        assert!(partial_editing(&set));
        // No card: begin/editing are inert.
        let mut empty = ApprovalState::new();
        assert!(!begin_partial_edit(&mut empty));
        assert!(!partial_editing(&empty));
        assert_eq!(partial_selection(&empty), (0, 0));
    }

    #[test]
    fn partial_toggles_and_cursor_move_stay_in_bounds() {
        let caps = rt::runtime_capability::FILE_READ
            | rt::runtime_capability::TERMINAL_IO
            | rt::runtime_capability::NETWORK;
        let mut set = ApprovalState::new();
        let envs = [pending_env(2, caps)];
        let _ = sync_pending_cards(&mut set, &envs, 1, 0);
        begin_partial_edit(&mut set);

        partial_move_cursor(&mut set, -1);
        assert_eq!(set.cards[0].partial_cursor, 0);
        partial_move_cursor(&mut set, 1);
        assert_eq!(set.cards[0].partial_cursor, 1);
        partial_toggle_cursor(&mut set);
        assert_eq!(
            set.cards[0].partial_mask & rt::runtime_capability::TERMINAL_IO,
            0
        );
        // Digits toggle rows directly; unknown digits are inert.
        partial_toggle_at(&mut set, 0);
        partial_toggle_at(&mut set, 9);
        assert_eq!(set.cards[0].partial_mask, rt::runtime_capability::NETWORK);
        // A is a toggle-all: not-all -> all, all -> none.
        partial_toggle_all(&mut set);
        assert_eq!(set.cards[0].partial_mask, caps);
        partial_toggle_all(&mut set);
        assert_eq!(set.cards[0].partial_mask, 0);
        partial_move_cursor(&mut set, 5);
        assert_eq!(set.cards[0].partial_cursor, 2);
    }

    #[test]
    fn partial_key_actions_cover_the_checklist_vocabulary() {
        use crate::state::{KEY_1, KEY_5, KEY_DOWN, KEY_ENTER, KEY_SPACE, KEY_UP};
        assert!(matches!(
            partial_key_action(KEY_A),
            Some(PartialKeyAction::ToggleAll)
        ));
        assert!(matches!(
            partial_key_action(KEY_D),
            Some(PartialKeyAction::Deny)
        ));
        assert!(matches!(
            partial_key_action(KEY_UP),
            Some(PartialKeyAction::MoveCursor(-1))
        ));
        assert!(matches!(
            partial_key_action(KEY_DOWN),
            Some(PartialKeyAction::MoveCursor(1))
        ));
        assert!(matches!(
            partial_key_action(KEY_SPACE),
            Some(PartialKeyAction::ToggleCursor)
        ));
        assert!(matches!(
            partial_key_action(KEY_1),
            Some(PartialKeyAction::ToggleIndex(0))
        ));
        assert!(matches!(
            partial_key_action(KEY_5),
            Some(PartialKeyAction::ToggleIndex(4))
        ));
        assert!(matches!(
            partial_key_action(KEY_ENTER),
            Some(PartialKeyAction::Submit)
        ));
        assert_eq!(partial_key_action(crate::state::KEY_ESC), None);
        assert_eq!(partial_key_action(crate::state::KEY_P), None);
    }

    #[test]
    fn partial_rows_render_marker_and_cursor() {
        let caps = rt::runtime_capability::NETWORK | rt::runtime_capability::GRAPHICS;
        let mut set = ApprovalState::new();
        let envs = [pending_env(1, caps)];
        let _ = sync_pending_cards(&mut set, &envs, 1, 0);
        begin_partial_edit(&mut set);
        let card = set.cards[0];
        let first = partial_row_text(&card, 0).expect("row 0");
        assert_eq!(first.as_str(), ">[x] network");
        let second = partial_row_text(&card, 1).expect("row 1");
        assert_eq!(second.as_str(), " [x] graphics");
        assert!(partial_row_text(&card, 2).is_none());
        partial_toggle_at(&mut set, 1);
        partial_move_cursor(&mut set, 1);
        let card = set.cards[0];
        let second = partial_row_text(&card, 1).expect("row 1");
        assert_eq!(second.as_str(), ">[ ] graphics");
    }

    #[test]
    fn partial_notice_decodes_granted_subset_or_failure() {
        let granted = rt::runtime_capability::NETWORK;
        let ok = partial_notice_text(4, granted, Ok(()));
        assert_eq!(ok.as_str(), "env 4 partial grant: network");
        let failed = partial_notice_text(4, granted, Err(rt::Error::Busy));
        assert_eq!(failed.as_str(), "runtime decision failed (env 4)");
    }

    #[test]
    fn closing_the_overlay_also_cancels_the_partial_checklist() {
        let mut set = ApprovalState::new();
        let envs = [pending_env(2, 0x1)];
        let _ = sync_pending_cards(&mut set, &envs, 1, 0);
        begin_partial_edit(&mut set);
        note_overlay_closed(&mut set);
        assert_eq!(set.len, 1);
        assert!(!set.cards[0].surfaced);
        assert!(!set.cards[0].partial_edit);
        assert_eq!(set.cards[0].partial_mask, 0);
    }

    #[test]
    fn partial_class_list_matches_capability_names_order() {
        assert_eq!(partial_class_count(u32::MAX), CAPABILITY_CLASSES.len());
        assert_eq!(partial_class_count(0), 0);
        let (name, mask) =
            partial_class_at(rt::runtime_capability::AUDIO, 0).expect("audio is row 0");
        assert_eq!(name, "audio");
        assert_eq!(mask, rt::runtime_capability::AUDIO);
        assert!(partial_class_at(rt::runtime_capability::AUDIO, 1).is_none());
    }
}

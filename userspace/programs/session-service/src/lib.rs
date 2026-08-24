//! Session registry, handoff state machine, and isolation policy for the
//! session service. Pure logic, shared between the `no_std` service binary
//! and host unit tests.

#![cfg_attr(not(test), no_std)]

pub const MAX_SESSIONS: usize = 4;
pub const MAX_MEMBERS: usize = 16;
pub const BOOTSTRAP_SESSION_ID: u32 = 1;
pub const PRIMARY_SEAT: u32 = 0;
/// Sentinel used on the wire for "no seat bound".
pub const NO_SEAT: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionKind {
    Graphical = 0,
    Operator = 1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SwitchError {
    UnknownTarget,
    AlreadyActive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FocusError {
    UnknownSession,
    InactiveSession,
    ForeignSurface,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisterError {
    CapacityExceeded,
    IdInUse,
    SeatInUse,
}

/// Ordered stages applied by a session handoff.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandoffStage {
    /// Kernel input route detached from the outgoing session.
    InputDetach = 0,
    /// Outgoing session focus torn down (focused surface cleared).
    FocusTeardown = 1,
    /// Seat ownership moved to the incoming session.
    SeatTransfer = 2,
    /// Incoming session becomes the active routing target.
    Activation = 3,
}
pub const HANDOFF_STAGE_COUNT: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HandoffRecord {
    pub from: u32,
    pub to: u32,
    pub transferred_seat: Option<u32>,
    pub completed: [bool; HANDOFF_STAGE_COUNT],
}

impl HandoffRecord {
    fn new(from: u32, to: u32, transferred_seat: Option<u32>) -> Self {
        Self {
            from,
            to,
            transferred_seat,
            completed: [false; HANDOFF_STAGE_COUNT],
        }
    }

    pub fn stage_done(&self, stage: HandoffStage) -> bool {
        self.completed[stage as usize]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionSnapshot {
    pub id: u32,
    pub seat: Option<u32>,
    pub kind: SessionKind,
    pub active: bool,
    pub focused_surface: u32,
    pub surface_count_hint: u32,
    pub member_count: usize,
}

/// Input-routing verdict produced by the isolation policy each turn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputDecision {
    /// Forward kernel input toward the desktop route of this session.
    RouteToDesktop(u32),
    /// Drop: session is registered but not active (isolation).
    DropInactive(u32),
    /// Drop: a handoff is mid-teardown; neither side may receive input.
    DropHandoff,
    /// Drop: no sessions registered at all.
    DropNoSession,
}

struct SessionEntry {
    id: u32,
    seat: Option<u32>,
    kind: SessionKind,
    focused_surface: u32,
    surface_count_hint: u32,
    members: [u32; MAX_MEMBERS],
    member_count: usize,
}

impl SessionEntry {
    fn new(id: u32, seat: Option<u32>, kind: SessionKind) -> Self {
        Self {
            id,
            seat,
            kind,
            focused_surface: 0,
            surface_count_hint: 0,
            members: [0; MAX_MEMBERS],
            member_count: 0,
        }
    }

    fn owns_surface(&self, surface_id: u32) -> bool {
        self.members[..self.member_count].contains(&surface_id)
    }

    fn add_member(&mut self, surface_id: u32) {
        if surface_id == 0 || self.owns_surface(surface_id) {
            return;
        }
        if self.member_count < MAX_MEMBERS {
            self.members[self.member_count] = surface_id;
            self.member_count += 1;
        }
    }

    fn snapshot(&self, active_id: u32) -> SessionSnapshot {
        SessionSnapshot {
            id: self.id,
            seat: self.seat,
            kind: self.kind,
            active: self.id == active_id,
            focused_surface: self.focused_surface,
            surface_count_hint: self.surface_count_hint,
            member_count: self.member_count,
        }
    }
}

pub struct SessionRegistry {
    entries: [Option<SessionEntry>; MAX_SESSIONS],
    active_id: u32,
    handoff_active: bool,
    handoff_from: u32,
    handoff_to: u32,
}

impl SessionRegistry {
    /// Registry with only the bootstrap graphical session (seat 0) active:
    /// exactly the pre-S8 single-session behavior.
    pub fn boot() -> Self {
        let mut registry = Self {
            entries: Default::default(),
            active_id: 0,
            handoff_active: false,
            handoff_from: 0,
            handoff_to: 0,
        };
        let _ = registry.insert(
            BOOTSTRAP_SESSION_ID,
            Some(PRIMARY_SEAT),
            SessionKind::Graphical,
        );
        registry.active_id = BOOTSTRAP_SESSION_ID;
        registry
    }

    /// Empty registry (host tests / future headless boot).
    pub fn empty() -> Self {
        Self {
            entries: Default::default(),
            active_id: 0,
            handoff_active: false,
            handoff_from: 0,
            handoff_to: 0,
        }
    }

    fn insert(
        &mut self,
        id: u32,
        seat: Option<u32>,
        kind: SessionKind,
    ) -> Result<u32, RegisterError> {
        let slot = self
            .entries
            .iter()
            .position(|entry| entry.is_none())
            .ok_or(RegisterError::CapacityExceeded)?;
        if self.find(id).is_some() {
            return Err(RegisterError::IdInUse);
        }
        if seat.is_some() && self.seat_owner(seat.unwrap_or(0)).is_some() {
            return Err(RegisterError::SeatInUse);
        }
        self.entries[slot] = Some(SessionEntry::new(id, seat, kind));
        Ok(id)
    }

    /// Register a new session. `requested_id == 0` allocates the next free id;
    /// `seat_word == NO_SEAT` registers without a seat binding.
    pub fn register(
        &mut self,
        requested_id: u32,
        seat_word: u32,
        kind: SessionKind,
    ) -> Result<u32, RegisterError> {
        let id = if requested_id == 0 {
            self.next_free_id()
        } else {
            requested_id
        };
        let seat = if seat_word == NO_SEAT {
            None
        } else {
            Some(seat_word)
        };
        self.insert(id, seat, kind)
    }

    fn next_free_id(&self) -> u32 {
        (1..).find(|id| self.find(*id).is_none()).unwrap_or(1)
    }

    fn find(&self, id: u32) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| matches!(entry, Some(e) if e.id == id))
    }

    fn entry_mut(&mut self, id: u32) -> Option<&mut SessionEntry> {
        let slot = self.find(id)?;
        self.entries[slot].as_mut()
    }

    fn entry(&self, id: u32) -> Option<&SessionEntry> {
        let slot = self.find(id)?;
        self.entries[slot].as_ref()
    }

    pub fn seat_owner(&self, seat: u32) -> Option<u32> {
        self.entries.iter().flatten().find_map(|entry| {
            if entry.seat == Some(seat) {
                Some(entry.id)
            } else {
                None
            }
        })
    }

    pub fn current(&self) -> Option<(u32, Option<u32>)> {
        let entry = self.entry(self.active_id)?;
        Some((entry.id, entry.seat))
    }

    pub fn current_id(&self) -> u32 {
        self.active_id
    }

    pub fn list_ids(&self, ids: &mut [u32]) -> usize {
        let mut count = 0;
        for entry in self.entries.iter().flatten() {
            if count < ids.len() {
                ids[count] = entry.id;
                count += 1;
            }
        }
        count
    }

    pub fn status(&self, id: u32) -> Option<SessionSnapshot> {
        self.entry(id).map(|entry| entry.snapshot(self.active_id))
    }

    /// True when this session is the sole allowed input/focus target.
    pub fn isolation_allows(&self, id: u32) -> bool {
        !self.handoff_active && id != 0 && id == self.active_id
    }

    /// Isolation-policy verdict for kernel input this turn.
    pub fn classify_input(&self) -> InputDecision {
        if self.handoff_active {
            return InputDecision::DropHandoff;
        }
        match self.entry(self.active_id) {
            Some(entry) => InputDecision::RouteToDesktop(entry.id),
            None => InputDecision::DropNoSession,
        }
    }

    /// Begin a handoff: input routes detach immediately; focus teardown and
    /// activation are finished by [`complete_switch`].
    pub fn begin_switch(&mut self, target: u32) -> Result<HandoffRecord, SwitchError> {
        if self.handoff_active {
            return Err(SwitchError::AlreadyActive);
        }
        if target == self.active_id {
            return Err(SwitchError::AlreadyActive);
        }
        if self.entry(target).is_none() {
            return Err(SwitchError::UnknownTarget);
        }
        let from = self.active_id;
        self.handoff_active = true;
        self.handoff_from = from;
        self.handoff_to = target;
        // Stage 1: detach input routes from the outgoing session.
        Ok(HandoffRecord::new(from, target, None))
    }

    /// Finish a handoff begun by [`begin_switch`]: tear down outgoing focus,
    /// transfer seat ownership, activate the incoming session.
    pub fn complete_switch(&mut self, mut record: HandoffRecord) -> HandoffRecord {
        if !self.handoff_active {
            return record;
        }
        record.completed[HandoffStage::InputDetach as usize] = true;
        // Stage 2: clean focus teardown on the outgoing session.
        if let Some(outgoing) = self.entry_mut(record.from) {
            outgoing.focused_surface = 0;
        }
        record.completed[HandoffStage::FocusTeardown as usize] = true;
        // Stage 3: seat ownership moves with the active role.
        let transferred_seat = {
            let from_seat = self.entry(record.from).and_then(|e| e.seat);
            let to_has_seat = self
                .entry(record.to)
                .map(|e| e.seat.is_some())
                .unwrap_or(false);
            if let (Some(seat), false) = (from_seat, to_has_seat) {
                if let Some(outgoing) = self.entry_mut(record.from) {
                    outgoing.seat = None;
                }
                if let Some(incoming) = self.entry_mut(record.to) {
                    incoming.seat = Some(seat);
                }
                Some(seat)
            } else {
                None
            }
        };
        record.transferred_seat = transferred_seat;
        record.completed[HandoffStage::SeatTransfer as usize] = true;
        // Stage 4: activation.
        self.active_id = record.to;
        self.handoff_active = false;
        self.handoff_from = 0;
        self.handoff_to = 0;
        record.completed[HandoffStage::Activation as usize] = true;
        record
    }

    /// Atomic convenience wrapper: begin + complete in one call.
    pub fn switch_active(&mut self, target: u32) -> Result<HandoffRecord, SwitchError> {
        let record = self.begin_switch(target)?;
        Ok(self.complete_switch(record))
    }

    /// Focus request against the isolation policy: only the active session
    /// accepts focus, and only on surfaces it owns.
    pub fn focus_request(&mut self, session_id: u32, surface_id: u32) -> Result<u32, FocusError> {
        if self.entry(session_id).is_none() {
            return Err(FocusError::UnknownSession);
        }
        if !self.isolation_allows(session_id) {
            return Err(FocusError::InactiveSession);
        }
        if surface_id != 0 {
            if let Some(owner) = self.surface_owner(surface_id) {
                if owner != session_id {
                    return Err(FocusError::ForeignSurface);
                }
            }
        }
        let entry = self
            .entry_mut(session_id)
            .ok_or(FocusError::UnknownSession)?;
        entry.focused_surface = surface_id;
        if surface_id != 0 {
            entry.add_member(surface_id);
            entry.surface_count_hint = entry.surface_count_hint.max(1);
        }
        Ok(surface_id)
    }

    pub fn surface_owner(&self, surface_id: u32) -> Option<u32> {
        self.entries
            .iter()
            .flatten()
            .find_map(|entry| entry.owns_surface(surface_id).then_some(entry.id))
    }

    pub fn handoff_pending(&self) -> bool {
        self.handoff_active
    }

    #[cfg(test)]
    pub(crate) fn handoff_window(&self) -> (u32, u32) {
        (self.handoff_from, self.handoff_to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_SEAT_WORD: u32 = NO_SEAT;

    #[test]
    fn boot_matches_legacy_single_session() {
        let registry = SessionRegistry::boot();
        assert_eq!(
            registry.current(),
            Some((BOOTSTRAP_SESSION_ID, Some(PRIMARY_SEAT)))
        );
        assert_eq!(
            registry.seat_owner(PRIMARY_SEAT),
            Some(BOOTSTRAP_SESSION_ID)
        );
        let mut ids = [0u32; MAX_SESSIONS];
        assert_eq!(registry.list_ids(&mut ids), 1);
        assert_eq!(ids[0], BOOTSTRAP_SESSION_ID);
        assert_eq!(
            registry.classify_input(),
            InputDecision::RouteToDesktop(BOOTSTRAP_SESSION_ID)
        );
    }

    #[test]
    fn register_allocates_ids_and_enforces_unique_ids_and_seats() {
        let mut registry = SessionRegistry::boot();
        let second = registry
            .register(0, NO_SEAT_WORD, SessionKind::Operator)
            .expect("auto id");
        assert_eq!(second, 2);
        assert_eq!(
            registry.register(2, NO_SEAT_WORD, SessionKind::Operator),
            Err(RegisterError::IdInUse)
        );
        assert_eq!(
            registry.register(3, PRIMARY_SEAT, SessionKind::Graphical),
            Err(RegisterError::SeatInUse)
        );
        let third = registry
            .register(7, NO_SEAT_WORD, SessionKind::Graphical)
            .expect("explicit id");
        assert_eq!(third, 7);
        let fourth = registry
            .register(9, NO_SEAT_WORD, SessionKind::Operator)
            .expect("last free slot");
        assert_eq!(fourth, 9);
        assert_eq!(
            registry.register(11, NO_SEAT_WORD, SessionKind::Operator),
            Err(RegisterError::CapacityExceeded)
        );
    }

    #[test]
    fn handoff_follows_stage_order_and_transfers_seat() {
        let mut registry = SessionRegistry::boot();
        registry
            .register(0, NO_SEAT_WORD, SessionKind::Operator)
            .unwrap();

        // Mid-handoff: routes detached but activation pending.
        let mut record = registry.begin_switch(2).expect("begin");
        assert!(registry.handoff_pending());
        assert_eq!(registry.handoff_window(), (1, 2));
        assert!(!record.stage_done(HandoffStage::Activation));
        assert!(!registry.isolation_allows(1));
        assert!(!registry.isolation_allows(2));

        record = registry.complete_switch(record);

        for stage in [
            HandoffStage::InputDetach,
            HandoffStage::FocusTeardown,
            HandoffStage::SeatTransfer,
            HandoffStage::Activation,
        ] {
            assert!(record.stage_done(stage), "stage {stage:?} missing");
        }
        assert!(!registry.handoff_pending());
        assert_eq!(registry.current_id(), 2);
        assert_eq!(registry.current(), Some((2, Some(PRIMARY_SEAT))));
        assert_eq!(
            registry.status(1).map(|s| (s.seat, s.active)),
            Some((None, false))
        );
        assert_eq!(registry.classify_input(), InputDecision::RouteToDesktop(2));
    }

    #[test]
    fn handoff_tears_down_outgoing_focus() {
        let mut registry = SessionRegistry::boot();
        registry
            .register(0, NO_SEAT_WORD, SessionKind::Operator)
            .unwrap();
        registry.focus_request(1, 41).expect("focus own surface");
        assert_eq!(registry.status(1).map(|s| s.focused_surface), Some(41));

        registry
            .switch_active(2)
            .expect("switch to operator session");

        let snapshot = registry.status(1).expect("session 1 snapshot");
        assert_eq!(snapshot.focused_surface, 0);
        assert_eq!(snapshot.member_count, 1);
    }

    #[test]
    fn switch_errors_are_distinct() {
        let mut registry = SessionRegistry::boot();
        assert_eq!(registry.switch_active(9), Err(SwitchError::UnknownTarget));
        assert_eq!(registry.switch_active(1), Err(SwitchError::AlreadyActive));
    }

    #[test]
    fn handoff_round_trip_restores_original_session() {
        let mut registry = SessionRegistry::boot();
        registry
            .register(0, NO_SEAT_WORD, SessionKind::Operator)
            .unwrap();

        let out = registry.switch_active(2).expect("outbound");
        assert_eq!(out.from, 1);
        assert_eq!(out.to, 2);
        let back = registry.switch_active(1).expect("return");
        assert_eq!(back.transferred_seat, Some(PRIMARY_SEAT));

        assert_eq!(registry.current(), Some((1, Some(PRIMARY_SEAT))));
        assert_eq!(registry.status(2).map(|s| s.seat), Some(None));
    }

    #[test]
    fn isolation_focus_matrix() {
        let mut registry = SessionRegistry::boot();
        registry
            .register(0, NO_SEAT_WORD, SessionKind::Operator)
            .unwrap();

        // Active session may focus its own surface; membership recorded.
        assert_eq!(registry.focus_request(1, 10), Ok(10));
        assert_eq!(registry.surface_owner(10), Some(1));

        // Unknown session.
        assert_eq!(
            registry.focus_request(42, 10),
            Err(FocusError::UnknownSession)
        );

        // Inactive session denied even on unowned surface.
        assert_eq!(
            registry.focus_request(2, 20),
            Err(FocusError::InactiveSession)
        );

        // Foreign surface owned by another session is denied while active...
        registry.switch_active(2).expect("activate session 2");
        assert_eq!(registry.surface_owner(10), Some(1));
        assert_eq!(
            registry.focus_request(2, 10),
            Err(FocusError::ForeignSurface)
        );

        // ...and the now-inactive owner cannot re-focus it either.
        assert_eq!(
            registry.focus_request(1, 10),
            Err(FocusError::InactiveSession)
        );

        // New active session can build its own membership.
        assert_eq!(registry.focus_request(2, 20), Ok(20));
        assert_eq!(registry.surface_owner(20), Some(2));
        assert_eq!(registry.status(2).map(|s| s.focused_surface), Some(20));
    }

    #[test]
    fn isolation_input_matrix() {
        let registry = SessionRegistry::empty();
        assert_eq!(registry.classify_input(), InputDecision::DropNoSession);
        assert!(!registry.isolation_allows(1));

        let mut registry = SessionRegistry::boot();
        registry
            .register(0, NO_SEAT_WORD, SessionKind::Operator)
            .unwrap();

        // Active route.
        assert_eq!(registry.classify_input(), InputDecision::RouteToDesktop(1));
        assert!(registry.isolation_allows(1));
        assert!(!registry.isolation_allows(2));

        // Mid-handoff: nobody receives input.
        let record = registry.begin_switch(2).unwrap();
        assert_eq!(registry.classify_input(), InputDecision::DropHandoff);
        assert!(!registry.isolation_allows(1));
        assert!(!registry.isolation_allows(2));

        // Post-activation: only the incoming session routes.
        registry.complete_switch(record);
        assert_eq!(registry.classify_input(), InputDecision::RouteToDesktop(2));
        assert!(!registry.isolation_allows(1));
        assert!(registry.isolation_allows(2));
    }

    #[test]
    fn zero_surface_focus_clears_without_membership() {
        let mut registry = SessionRegistry::boot();
        registry.focus_request(1, 30).unwrap();
        assert_eq!(registry.focus_request(1, 0), Ok(0));
        let snapshot = registry.status(1).unwrap();
        assert_eq!(snapshot.focused_surface, 0);
        assert_eq!(snapshot.member_count, 1);
        assert_eq!(snapshot.surface_count_hint, 1);
    }
}

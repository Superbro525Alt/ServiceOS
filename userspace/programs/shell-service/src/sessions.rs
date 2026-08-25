//! Operator session registry for the shell service.
//!
//! The shell hosts several independent operator sessions instead of one
//! serial REPL: every session is keyed by its source (`SessionKey`) and owns
//! its own command-history ring and optional account identity (ownership
//! policy). Two key kinds exist today:
//!
//! - `Console`: the classic serial console session opened against
//!   console-service at boot (one per shell process).
//! - `Client`: an operator session reached over the shell public channel
//!   (see `shell_tag` in the crate root), keyed by the server-side endpoint
//!   handle so multiple graphical/front-end clients never share state.
//!
//! Everything here is pure bookkeeping so host tests cover keying, history
//! rings, and ownership binding without a kernel.

use core::cell::UnsafeCell;

pub const MAX_OPERATOR_SESSIONS: usize = 4;
pub const MAX_HISTORY_ENTRIES: usize = 16;
pub const HISTORY_LINE_BYTES: usize = 128;
pub const MAX_OWNER_NAME: usize = 32;

/// Identity of one operator session's source. Encode/decode round-trips
/// through a `u64` so it can sit in the single-threaded "active session"
/// slot alongside command execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionKey {
    /// Serial console session; payload is the console endpoint handle.
    Console(u32),
    /// Public-channel client session; payload is the server endpoint handle.
    Client(u32),
}

impl SessionKey {
    pub const fn encode(self) -> u64 {
        let (kind, value) = match self {
            SessionKey::Console(value) => (1u64, value),
            SessionKey::Client(value) => (2u64, value),
        };
        (kind << 32) | value as u64
    }

    pub const fn decode(word: u64) -> Option<Self> {
        let kind = word >> 32;
        let value = (word & 0xffff_ffff) as u32;
        match kind {
            1 => Some(SessionKey::Console(value)),
            2 => Some(SessionKey::Client(value)),
            _ => None,
        }
    }

    pub const fn kind_name(self) -> &'static str {
        match self {
            SessionKey::Console(_) => "console",
            SessionKey::Client(_) => "client",
        }
    }
}

/// Account identity bound to a session by the ownership policy. Absent until
/// a successful login; activation of account-service is manual, so unowned
/// sessions are a normal, fully functional state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Owner {
    pub account_id: u32,
    pub capabilities: u64,
    pub name_len: usize,
    pub name: [u8; MAX_OWNER_NAME],
}

impl Owner {
    pub fn none_named(name: &str, account_id: u32, capabilities: u64) -> Option<Self> {
        let bytes = name.as_bytes();
        if bytes.len() > MAX_OWNER_NAME {
            return None;
        }
        let mut storage = [0u8; MAX_OWNER_NAME];
        storage[..bytes.len()].copy_from_slice(bytes);
        Some(Self {
            account_id,
            capabilities,
            name_len: bytes.len(),
            name: storage,
        })
    }

    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("")
    }
}

/// Per-session command history ring: bounded, deduplicates consecutive
/// duplicates, and serves entries newest-first by order index.
#[derive(Clone, Copy)]
pub struct HistoryRing {
    entries: [[u8; HISTORY_LINE_BYTES]; MAX_HISTORY_ENTRIES],
    lens: [usize; MAX_HISTORY_ENTRIES],
    count: usize,
    head: usize,
}

impl HistoryRing {
    pub const fn new() -> Self {
        Self {
            entries: [[0; HISTORY_LINE_BYTES]; MAX_HISTORY_ENTRIES],
            lens: [0; MAX_HISTORY_ENTRIES],
            count: 0,
            head: 0,
        }
    }

    /// Record one submitted command line. Consecutive duplicates collapse so
    /// arrow-key resubmission does not flood the ring.
    pub fn push(&mut self, line: &[u8]) {
        let line = if line.len() > HISTORY_LINE_BYTES {
            &line[..HISTORY_LINE_BYTES]
        } else {
            line
        };
        if line.is_empty() {
            return;
        }
        if let Some(latest) = self.latest_slot_and_len() {
            let (slot, len) = latest;
            if len == line.len() && &self.entries[slot][..len] == line {
                return;
            }
        }
        let slot = self.head;
        self.entries[slot][..line.len()].copy_from_slice(line);
        self.lens[slot] = line.len();
        self.head = (self.head + 1) % MAX_HISTORY_ENTRIES;
        if self.count < MAX_HISTORY_ENTRIES {
            self.count += 1;
        }
    }

    fn latest_slot_and_len(&self) -> Option<(usize, usize)> {
        if self.count == 0 {
            return None;
        }
        let slot = self.slot(self.count - 1);
        Some((slot, self.lens[slot]))
    }

    fn slot(&self, order: usize) -> usize {
        (self.head + MAX_HISTORY_ENTRIES - self.count + order) % MAX_HISTORY_ENTRIES
    }

    /// Copy the entry `order` steps from the oldest into `out`, returning its
    /// length (None when out of range).
    pub fn entry(&self, order: usize, out: &mut [u8]) -> Option<usize> {
        if order >= self.count {
            return None;
        }
        let slot = self.slot(order);
        let len = self.lens[slot].min(out.len());
        out[..len].copy_from_slice(&self.entries[slot][..len]);
        Some(len)
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// One operator session: independent context keyed by its source.
#[derive(Clone, Copy)]
pub struct OperatorSession {
    pub id: u32,
    pub key: SessionKey,
    pub history: HistoryRing,
    pub owner: Option<Owner>,
    /// Peer endpoint used for outbound output (clients only).
    pub peer: u32,
    pub occupied: bool,
}

impl OperatorSession {
    pub const fn empty(id: u32, key: SessionKey) -> Self {
        Self {
            id,
            key,
            history: HistoryRing::new(),
            owner: None,
            peer: 0,
            occupied: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnsureError {
    TableFull,
}

/// Fixed-size registry. Slots are keyed; re-ensuring an existing key returns
/// the same row so client reconnects do not lose context mid-flight.
pub struct SessionTable {
    slots: [Option<OperatorSession>; MAX_OPERATOR_SESSIONS],
    next_id: u32,
}

impl SessionTable {
    pub const fn new() -> Self {
        Self {
            slots: [None; MAX_OPERATOR_SESSIONS],
            next_id: 1,
        }
    }

    pub fn contains(&self, key: SessionKey) -> bool {
        self.slots
            .iter()
            .any(|slot| slot.is_some_and(|s| s.occupied && s.key == key))
    }

    pub fn position(&self, key: SessionKey) -> Option<usize> {
        self.slots
            .iter()
            .position(|slot| slot.is_some_and(|s| s.occupied && s.key == key))
    }

    /// Create or fetch the row for `key`, returning its session id.
    pub fn ensure(&mut self, key: SessionKey, peer: u32) -> Result<u32, EnsureError> {
        if let Some(index) = self.position(key) {
            let slot = self.slots[index].as_mut().expect("position implies some");
            slot.peer = peer;
            return Ok(slot.id);
        }
        let free = self
            .slots
            .iter_mut()
            .position(|slot| slot.is_none_or(|s| !s.occupied))
            .ok_or(EnsureError::TableFull)?;
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.slots[free] = Some(OperatorSession {
            id,
            key,
            history: HistoryRing::new(),
            owner: None,
            peer,
            occupied: true,
        });
        Ok(id)
    }

    pub fn remove(&mut self, key: SessionKey) -> bool {
        match self.position(key) {
            Some(index) => {
                self.slots[index] = None;
                true
            }
            None => false,
        }
    }

    pub fn count(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.as_ref().is_some_and(|s| s.occupied))
            .count()
    }

    pub fn record_history(&mut self, key: SessionKey, line: &[u8]) {
        if let Some(index) = self.position(key) {
            self.slots[index]
                .as_mut()
                .expect("position implies some")
                .history
                .push(line);
        }
    }

    pub fn bind_owner(&mut self, key: SessionKey, owner: Owner) -> bool {
        match self.position(key) {
            Some(index) => {
                self.slots[index]
                    .as_mut()
                    .expect("position implies some")
                    .owner = Some(owner);
                true
            }
            None => false,
        }
    }

    pub fn unbind_owner(&mut self, key: SessionKey) -> bool {
        match self.position(key) {
            Some(index) => {
                self.slots[index]
                    .as_mut()
                    .expect("position implies some")
                    .owner = None;
                true
            }
            None => false,
        }
    }

    pub fn owner_of(&self, key: SessionKey) -> Option<Owner> {
        let index = self.position(key)?;
        self.slots[index].and_then(|slot| slot.owner)
    }

    pub fn history_len(&self, key: SessionKey) -> usize {
        match self.position(key) {
            Some(index) => self.slots[index]
                .as_ref()
                .map(|slot| slot.history.len())
                .unwrap_or(0),
            None => 0,
        }
    }

    pub fn history_entry(&self, key: SessionKey, order: usize, out: &mut [u8]) -> Option<usize> {
        let index = self.position(key)?;
        self.slots[index]
            .as_ref()
            .and_then(|slot| slot.history.entry(order, out))
    }

    pub fn peer_of(&self, key: SessionKey) -> Option<u32> {
        let index = self.position(key)?;
        self.slots[index].as_ref().map(|slot| slot.peer)
    }

    /// Visit rows oldest-registration-first (slot order) for listings.
    pub fn for_each<F: FnMut(&OperatorSession)>(&self, mut visit: F) {
        for slot in &self.slots {
            if let Some(session) = slot {
                if session.occupied {
                    visit(session);
                }
            }
        }
    }
}

impl Default for SessionTable {
    fn default() -> Self {
        Self::new()
    }
}

struct ActiveSlot(UnsafeCell<u64>);
unsafe impl Sync for ActiveSlot {}
static ACTIVE_KEY: ActiveSlot = ActiveSlot(UnsafeCell::new(0));

struct TableSlot(UnsafeCell<SessionTable>);
unsafe impl Sync for TableSlot {}
static OPERATOR_TABLE: TableSlot = TableSlot(UnsafeCell::new(SessionTable::new()));

/// Mark which operator session subsequent command execution belongs to.
pub fn set_active_key(word: u64) {
    // SAFETY: the shell task is strictly single-threaded (pending-line slot
    // precedent); no concurrent access is possible.
    unsafe {
        *ACTIVE_KEY.0.get() = word;
    }
}

pub fn active_key() -> Option<SessionKey> {
    // SAFETY: see `set_active_key`.
    let word = unsafe { *ACTIVE_KEY.0.get() };
    SessionKey::decode(word)
}

fn table() -> &'static mut SessionTable {
    // SAFETY: see `set_active_key`.
    unsafe { &mut *OPERATOR_TABLE.0.get() }
}

/// Registry entry point used by the main loop: create-or-fetch on demand.
pub fn ensure_session(key: SessionKey, peer: u32) -> Result<u32, EnsureError> {
    table().ensure(key, peer)
}

pub fn drop_session(key: SessionKey) -> bool {
    table().remove(key)
}

pub fn session_count() -> usize {
    table().count()
}

pub fn record_history(key: SessionKey, line: &str) {
    table().record_history(key, line.as_bytes());
}

pub fn bind_owner(key: SessionKey, owner: Owner) -> bool {
    table().bind_owner(key, owner)
}

pub fn unbind_owner(key: SessionKey) -> bool {
    table().unbind_owner(key)
}

pub fn owner_of(key: SessionKey) -> Option<Owner> {
    table().owner_of(key)
}

pub fn history_len(key: SessionKey) -> usize {
    table().history_len(key)
}

pub fn history_entry(key: SessionKey, order: usize, out: &mut [u8]) -> Option<usize> {
    table().history_entry(key, order, out)
}

pub fn for_each<F: FnMut(&OperatorSession)>(visit: F) {
    table().for_each(visit);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_roundtrip_through_words() {
        for key in [
            SessionKey::Console(7),
            SessionKey::Client(0xdead_beef_u32 as u32),
        ] {
            assert_eq!(SessionKey::decode(key.encode()), Some(key));
        }
        assert_eq!(SessionKey::decode(0), None);
        assert_eq!(SessionKey::decode(3 << 32 | 1), None);
        assert_ne!(
            SessionKey::Console(5).encode(),
            SessionKey::Client(5).encode()
        );
    }

    #[test]
    fn ensure_reuses_rows_by_key_and_mints_distinct_ids() {
        let mut table = SessionTable::new();
        let first = table.ensure(SessionKey::Console(10), 0).unwrap();
        let again = table.ensure(SessionKey::Console(10), 0).unwrap();
        assert_eq!(first, again);
        let client = table.ensure(SessionKey::Client(11), 11).unwrap();
        assert_ne!(first, client);
        assert_eq!(table.count(), 2);
        assert!(table.contains(SessionKey::Client(11)));
        assert_eq!(table.peer_of(SessionKey::Client(11)), Some(11));
    }

    #[test]
    fn capacity_is_bounded_and_remove_frees_a_slot() {
        let mut table = SessionTable::new();
        for index in 0..MAX_OPERATOR_SESSIONS as u32 {
            assert!(table.ensure(SessionKey::Client(index), index).is_ok());
        }
        assert_eq!(
            table.ensure(SessionKey::Client(99), 99),
            Err(EnsureError::TableFull)
        );
        assert!(table.remove(SessionKey::Client(2)));
        assert!(table.ensure(SessionKey::Client(99), 99).is_ok());
        assert!(!table.remove(SessionKey::Client(12345)));
    }

    #[test]
    fn history_ring_is_oldest_first_and_wraps() {
        let mut ring = HistoryRing::new();
        assert!(ring.is_empty());
        ring.push(b"one");
        ring.push(b"two");
        let mut buffer = [0u8; HISTORY_LINE_BYTES];
        assert_eq!(ring.entry(0, &mut buffer), Some(3));
        assert_eq!(&buffer[..3], b"one", "order 0 is the oldest entry");
        assert_eq!(ring.entry(1, &mut buffer), Some(3));
        assert_eq!(&buffer[..3], b"two");
        assert_eq!(ring.entry(2, &mut buffer), None);

        for index in 0..MAX_HISTORY_ENTRIES + 4 {
            let line = format!("cmd{index}");
            ring.push(line.as_bytes());
        }
        assert_eq!(ring.len(), MAX_HISTORY_ENTRIES);
        let newest_order = MAX_HISTORY_ENTRIES - 1;
        assert_eq!(
            ring.entry(newest_order, &mut buffer),
            Some(5),
            "last order index is the newest entry"
        );
        assert_eq!(&buffer[..5], b"cmd19");
        assert_eq!(ring.entry(0, &mut buffer), Some(4));
        assert_eq!(
            &buffer[..4],
            b"cmd4",
            "oldest retained entry wraps past evicted rows"
        );
    }

    #[test]
    fn history_collapses_consecutive_duplicates_but_keeps_repeats_later() {
        let mut ring = HistoryRing::new();
        ring.push(b"ls");
        ring.push(b"ls");
        assert_eq!(ring.len(), 1);
        ring.push(b"help");
        ring.push(b"ls");
        assert_eq!(ring.len(), 3);
        let mut buffer = [0u8; HISTORY_LINE_BYTES];
        assert_eq!(ring.entry(2, &mut buffer), Some(2));
        assert_eq!(&buffer[..2], b"ls");
    }

    #[test]
    fn history_truncates_overlong_lines_instead_of_panicking() {
        let mut ring = HistoryRing::new();
        let long = [b'a'; HISTORY_LINE_BYTES + 50];
        ring.push(&long);
        let mut buffer = [0u8; HISTORY_LINE_BYTES];
        assert_eq!(ring.entry(0, &mut buffer), Some(HISTORY_LINE_BYTES));
        ring.push(b"");
        assert_eq!(ring.len(), 1, "empty pushes are ignored");
    }

    #[test]
    fn owner_binding_scopes_to_one_session() {
        let mut table = SessionTable::new();
        let console = SessionKey::Console(1);
        let client = SessionKey::Client(2);
        table.ensure(console, 0).unwrap();
        table.ensure(client, 2).unwrap();

        assert!(table.owner_of(console).is_none(), "sessions start unowned");
        let owner = Owner::none_named("paul", 3, 0b101).unwrap();
        assert!(table.bind_owner(console, owner));
        assert_eq!(table.owner_of(console), Some(owner));
        assert_eq!(table.owner_of(client), None);
        assert_eq!(owner.name(), "paul");
        assert!(table.unbind_owner(console));
        assert!(table.owner_of(console).is_none());
        assert!(!table.bind_owner(SessionKey::Client(404), owner));
    }

    #[test]
    fn history_and_listing_helpers_scope_by_key() {
        let mut table = SessionTable::new();
        let console = SessionKey::Console(9);
        let client = SessionKey::Client(10);
        table.ensure(console, 0).unwrap();
        table.ensure(client, 10).unwrap();
        table.record_history(console, b"services".as_slice());
        assert_eq!(table.history_len(console), 1);
        assert_eq!(table.history_len(client), 0);

        let mut seen = 0usize;
        table.for_each(|session| {
            seen += 1;
            assert!(session.id >= 1);
        });
        assert_eq!(seen, 2);
    }
}

//! Peripheral device registry, attach/detach event log, and the printer
//! class stub for ServiceOS.
//!
//! Honest scope note: manual-activation services receive no kernel device
//! handles at spawn (device transports are injected only into graph
//! services), so this v0 registry is populated through its own ATTACH
//! contract — clients that hold real transports report descriptors built
//! from the kernel's enumeration vocabulary (input device classes from the
//! shared ABI, block/display backend words) and the service classifies and
//! tracks them. With no registrants the registry honestly reports zero
//! devices. The printer class ships as a full query shape with an explicit
//! `Unimplemented` status; no print pipeline exists.

#![cfg_attr(not(test), no_std)]

pub const MAX_DEVICES: usize = 12;
pub const MAX_EVENTS: usize = 16;
/// Events carried per EVENTS reply (IPC word budget: 16 words total).
pub const MAX_EVENTS_PER_REPLY: usize = 3;
pub const PRINTER_QUEUE_CAPACITY: u32 = 8;

/// Wire tags for the peripheral service's control channel. Requests carry a
/// reply channel as handles[0]; replies are status-first
/// (`PeripheralError::to_code`, 0 = Ok) followed by op-specific words.
/// Published in-crate following account-service/power-service precedent so
/// no ABI edit is needed; range 0x260+ is unused elsewhere.
pub mod peripheral_tag {
    pub const STATUS_REQUEST: u32 = 0x260;
    pub const STATUS_REPLY: u32 = 0x261;
    pub const ATTACH_REQUEST: u32 = 0x262;
    pub const ATTACH_REPLY: u32 = 0x263;
    pub const DETACH_REQUEST: u32 = 0x264;
    pub const DETACH_REPLY: u32 = 0x265;
    pub const LIST_REQUEST: u32 = 0x266;
    pub const LIST_REPLY: u32 = 0x267;
    pub const EVENTS_REQUEST: u32 = 0x268;
    pub const EVENTS_REPLY: u32 = 0x269;
    pub const PRINTER_QUERY_REQUEST: u32 = 0x26A;
    pub const PRINTER_QUERY_REPLY: u32 = 0x26B;
}

/// Transport family hints reported by clients on ATTACH. The input family
/// resolves subclasses via the kernel's `input_device_class` vocabulary; the
/// other families are their own known classes.
pub mod device_family {
    pub const INPUT: u32 = 0;
    pub const BLOCK: u32 = 1;
    pub const DISPLAY: u32 = 2;
    pub const AUDIO: u32 = 3;
    pub const PRINTER: u32 = 4;
}

/// Known device classes. Input subclasses mirror the shared ABI's
/// `input_device_class` values (keyboard=1, pointer=2, tablet=3); the rest
/// number contiguously from 4.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceClass {
    Unknown = 0,
    Keyboard = 1,
    Pointer = 2,
    Tablet = 3,
    Block = 4,
    Display = 5,
    Audio = 6,
    Printer = 7,
}

impl DeviceClass {
    pub const fn from_word(value: u64) -> Self {
        match value {
            x if x == DeviceClass::Keyboard as u64 => DeviceClass::Keyboard,
            x if x == DeviceClass::Pointer as u64 => DeviceClass::Pointer,
            x if x == DeviceClass::Tablet as u64 => DeviceClass::Tablet,
            x if x == DeviceClass::Block as u64 => DeviceClass::Block,
            x if x == DeviceClass::Display as u64 => DeviceClass::Display,
            x if x == DeviceClass::Audio as u64 => DeviceClass::Audio,
            x if x == DeviceClass::Printer as u64 => DeviceClass::Printer,
            _ => DeviceClass::Unknown,
        }
    }

    /// Map a reported transport family plus detail word onto a known class.
    /// Input details use the kernel enumeration classes; unknown inputs stay
    /// honestly Unknown rather than being guessed.
    pub const fn classify(family: u32, detail: u32) -> Self {
        match family {
            x if x == device_family::INPUT => match detail {
                1 => DeviceClass::Keyboard,
                2 => DeviceClass::Pointer,
                3 => DeviceClass::Tablet,
                _ => DeviceClass::Unknown,
            },
            x if x == device_family::BLOCK => DeviceClass::Block,
            x if x == device_family::DISPLAY => DeviceClass::Display,
            x if x == device_family::AUDIO => DeviceClass::Audio,
            x if x == device_family::PRINTER => DeviceClass::Printer,
            _ => DeviceClass::Unknown,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            DeviceClass::Unknown => "unknown",
            DeviceClass::Keyboard => "keyboard",
            DeviceClass::Pointer => "pointer",
            DeviceClass::Tablet => "tablet",
            DeviceClass::Block => "block",
            DeviceClass::Display => "display",
            DeviceClass::Audio => "audio",
            DeviceClass::Printer => "printer",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeripheralError {
    InvalidArgument = 1,
    NotFound = 2,
    CapacityExceeded = 3,
}

impl PeripheralError {
    pub const fn to_code(self) -> u64 {
        self as u64
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventKind {
    Attach = 1,
    Detach = 2,
}

impl EventKind {
    pub const fn from_word(value: u64) -> Self {
        match value {
            x if x == EventKind::Detach as u64 => EventKind::Detach,
            _ => EventKind::Attach,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceRecord {
    pub id: u32,
    pub class: DeviceClass,
    pub backend: u32,
    pub flags: u32,
    pub meta: u32,
}

/// One packed LIST-reply word:
/// id[0..16] class[16..24] backend[24..32] flags[32..48] meta[48..64].
pub const fn pack_device_record(record: &DeviceRecord) -> u64 {
    (record.id as u64 & 0xffff)
        | ((record.class as u64 & 0xff) << 16)
        | ((record.backend as u64 & 0xff) << 24)
        | ((record.flags as u64 & 0xffff) << 32)
        | ((record.meta as u64 & 0xffff) << 48)
}

pub const fn unpack_device_record(word: u64) -> DeviceRecord {
    DeviceRecord {
        id: (word & 0xffff) as u32,
        class: DeviceClass::from_word((word >> 16) & 0xff),
        backend: ((word >> 24) & 0xff) as u32,
        flags: ((word >> 32) & 0xffff) as u32,
        meta: ((word >> 48) & 0xffff) as u32,
    }
}

/// Unpack an EVENTS-reply detail word:
/// kind[40..48] device_id[16..40] class[0..8].
pub const fn unpack_event_detail(word: u64) -> (u64, u32, u64) {
    (
        (word >> 40) & 0xff,
        ((word >> 16) & 0xff_ffff) as u32,
        word & 0xff,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventRecord {
    pub seq: u64,
    pub tick: u64,
    pub kind: EventKind,
    pub device_id: u32,
    pub class: DeviceClass,
}

/// Bounded ring of the most recent attach/detach events with monotonic
/// sequence numbers and lifetime totals.
pub struct EventLog {
    slots: [Option<EventRecord>; MAX_EVENTS],
    next: usize,
    count: usize,
    next_seq: u64,
    attach_total: u64,
    detach_total: u64,
}

impl EventLog {
    pub const fn new() -> Self {
        Self {
            slots: [None; MAX_EVENTS],
            next: 0,
            count: 0,
            next_seq: 1,
            attach_total: 0,
            detach_total: 0,
        }
    }

    pub fn record(&mut self, kind: EventKind, device_id: u32, class: DeviceClass, tick: u64) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        match kind {
            EventKind::Attach => self.attach_total += 1,
            EventKind::Detach => self.detach_total += 1,
        }
        self.slots[self.next] = Some(EventRecord {
            seq,
            tick,
            kind,
            device_id,
            class,
        });
        self.next = (self.next + 1) % MAX_EVENTS;
        self.count = (self.count + 1).min(MAX_EVENTS);
        seq
    }

    /// Most recent `n` events, newest last (chronological order).
    pub fn last_n<F: FnMut(&EventRecord)>(&self, n: usize, mut visit: F) {
        let usable = n.min(self.count);
        let start = (self.next + MAX_EVENTS - usable) % MAX_EVENTS;
        for offset in 0..usable {
            let slot = (start + offset) % MAX_EVENTS;
            if let Some(record) = self.slots[slot] {
                visit(&record);
            }
        }
    }

    pub const fn len(&self) -> usize {
        self.count
    }

    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub const fn attach_total(&self) -> u64 {
        self.attach_total
    }

    pub const fn detach_total(&self) -> u64 {
        self.detach_total
    }

    pub const fn next_seq(&self) -> u64 {
        self.next_seq
    }
}

/// Fixed-capacity registry of attached devices. Ids are monotonic and never
/// reused within a boot.
pub struct DeviceRegistry {
    slots: [Option<DeviceRecord>; MAX_DEVICES],
    next_id: u32,
}

impl DeviceRegistry {
    pub const fn new() -> Self {
        Self {
            slots: [None; MAX_DEVICES],
            next_id: 1,
        }
    }

    pub fn attach(
        &mut self,
        class: DeviceClass,
        backend: u32,
        flags: u32,
        meta: u32,
    ) -> Result<DeviceRecord, PeripheralError> {
        if class == DeviceClass::Unknown {
            return Err(PeripheralError::InvalidArgument);
        }
        let free = self
            .slots
            .iter_mut()
            .position(|slot| slot.is_none())
            .ok_or(PeripheralError::CapacityExceeded)?;
        let record = DeviceRecord {
            id: self.next_id,
            class,
            backend,
            flags,
            meta,
        };
        self.next_id += 1;
        self.slots[free] = Some(record);
        Ok(record)
    }

    pub fn detach(&mut self, id: u32) -> Result<DeviceRecord, PeripheralError> {
        let position = self
            .slots
            .iter()
            .position(|slot| slot.is_some_and(|record| record.id == id))
            .ok_or(PeripheralError::NotFound)?;
        let record = self.slots[position].take().expect("position implies some");
        Ok(record)
    }

    pub fn get(&self, id: u32) -> Option<DeviceRecord> {
        self.slots
            .iter()
            .flatten()
            .find(|record| record.id == id)
            .copied()
    }

    /// Visit records oldest-attached first, optionally filtered to one class
    /// (`None` visits everything).
    pub fn for_each<F: FnMut(&DeviceRecord)>(&self, filter: Option<DeviceClass>, mut visit: F) {
        for slot in &self.slots {
            if let Some(record) = slot {
                if filter.is_none_or(|wanted| wanted == record.class) {
                    visit(record);
                }
            }
        }
    }

    pub fn count_matching(&self, filter: Option<DeviceClass>) -> usize {
        let mut total = 0usize;
        self.for_each(filter, |_| total += 1);
        total
    }
}

/// Honest v0 printer class: full query shape, explicitly Unimplemented.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrinterStatus {
    Unimplemented = 2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrinterReport {
    pub status: PrinterStatus,
    pub queue_depth: u32,
    pub queue_capacity: u32,
}

pub const fn printer_report() -> PrinterReport {
    PrinterReport {
        status: PrinterStatus::Unimplemented,
        queue_depth: 0,
        queue_capacity: PRINTER_QUEUE_CAPACITY,
    }
}

/// Aggregate service state owned by the running binary and exercised by host
/// tests through the protocol handler.
pub struct PeripheralServiceState {
    pub registry: DeviceRegistry,
    pub events: EventLog,
}

impl PeripheralServiceState {
    pub const fn new() -> Self {
        Self {
            registry: DeviceRegistry::new(),
            events: EventLog::new(),
        }
    }

    /// Attach one device and log the event at `tick`.
    pub fn attach(
        &mut self,
        family: u32,
        detail: u32,
        backend: u32,
        flags: u32,
        meta: u32,
        tick: u64,
    ) -> Result<DeviceRecord, PeripheralError> {
        let class = DeviceClass::classify(family, detail);
        let record = self.registry.attach(class, backend, flags, meta)?;
        self.events.record(EventKind::Attach, record.id, class, tick);
        Ok(record)
    }

    /// Detach one device by id and log the event at `tick`.
    pub fn detach(&mut self, id: u32, tick: u64) -> Result<DeviceRecord, PeripheralError> {
        let record = self.registry.detach(id)?;
        self.events
            .record(EventKind::Detach, record.id, record.class, tick);
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use device_family::{AUDIO, BLOCK, DISPLAY, INPUT, PRINTER};

    #[test]
    fn classify_maps_kernel_input_enumeration_classes() {
        // Kernel input_device_class vocabulary: keyboard=1 pointer=2 tablet=3.
        assert_eq!(DeviceClass::classify(INPUT, 1), DeviceClass::Keyboard);
        assert_eq!(DeviceClass::classify(INPUT, 2), DeviceClass::Pointer);
        assert_eq!(DeviceClass::classify(INPUT, 3), DeviceClass::Tablet);
        // Unknown input detail stays honestly unknown, not guessed.
        assert_eq!(DeviceClass::classify(INPUT, 9), DeviceClass::Unknown);
        assert_eq!(DeviceClass::classify(77, 1), DeviceClass::Unknown);

        assert_eq!(DeviceClass::classify(BLOCK, 0), DeviceClass::Block);
        assert_eq!(DeviceClass::classify(DISPLAY, 1), DeviceClass::Display);
        assert_eq!(DeviceClass::classify(AUDIO, 0), DeviceClass::Audio);
        assert_eq!(DeviceClass::classify(PRINTER, 0), DeviceClass::Printer);
    }

    #[test]
    fn class_words_roundtrip_through_from_word() {
        for expected in [
            DeviceClass::Unknown,
            DeviceClass::Keyboard,
            DeviceClass::Pointer,
            DeviceClass::Tablet,
            DeviceClass::Block,
            DeviceClass::Display,
            DeviceClass::Audio,
            DeviceClass::Printer,
        ] {
            assert_eq!(DeviceClass::from_word(expected as u64), expected);
        }
        assert_eq!(DeviceClass::from_word(200), DeviceClass::Unknown);
    }

    #[test]
    fn registry_attach_detach_roundtrip_with_monotonic_ids() {
        let mut registry = DeviceRegistry::new();
        let first = registry
            .attach(DeviceClass::Keyboard, 1, 0, 1)
            .expect("attach");
        let second = registry
            .attach(DeviceClass::Block, 1, 0, 4)
            .expect("attach");
        assert_eq!(first.id, 1);
        assert_eq!(second.id, 2);
        assert_eq!(registry.count_matching(None), 2);
        assert_eq!(registry.count_matching(Some(DeviceClass::Block)), 1);

        let removed = registry.detach(first.id).expect("detach");
        assert_eq!(removed.class, DeviceClass::Keyboard);
        assert_eq!(registry.detach(first.id), Err(PeripheralError::NotFound));
        assert_eq!(registry.get(first.id), None);
        assert_eq!(registry.get(second.id), Some(second));

        // Ids keep climbing after a free slot appears.
        let third = registry.attach(DeviceClass::Display, 1, 0, 0).expect("attach");
        assert_eq!(third.id, 3);
    }

    #[test]
    fn registry_rejects_unknown_class_and_reports_capacity() {
        let mut registry = DeviceRegistry::new();
        assert_eq!(
            registry.attach(DeviceClass::Unknown, 0, 0, 0),
            Err(PeripheralError::InvalidArgument)
        );
        let mut ids = [0u32; MAX_DEVICES];
        for (index, slot) in ids.iter_mut().enumerate() {
            *slot = registry
                .attach(DeviceClass::Audio, 1, index as u32, 0)
                .expect("slot")
                .id;
        }
        assert_eq!(
            registry.attach(DeviceClass::Printer, 0, 0, 0),
            Err(PeripheralError::CapacityExceeded)
        );
        // Freeing one slot admits exactly one more device.
        registry.detach(ids[0]).expect("detach");
        assert!(registry.attach(DeviceClass::Printer, 0, 0, 0).is_ok());
    }

    #[test]
    fn event_log_orders_newest_last_and_retains_bounded_window() {
        let mut log = EventLog::new();
        assert!(log.is_empty());
        for step in 0..(MAX_EVENTS + 4) {
            let kind = if step % 2 == 0 {
                EventKind::Attach
            } else {
                EventKind::Detach
            };
            let seq = log.record(kind, (step + 1) as u32, DeviceClass::Pointer, step as u64 * 10);
            assert_eq!(seq, step as u64 + 1);
        }
        assert_eq!(log.len(), MAX_EVENTS);
        assert_eq!(log.attach_total(), 10);
        assert_eq!(log.detach_total(), 10);
        assert_eq!(log.next_seq(), MAX_EVENTS as u64 + 5);

        let mut seen = [0u64; MAX_EVENTS];
        let mut index = 0usize;
        log.last_n(MAX_EVENTS, |record| {
            seen[index] = record.seq;
            index += 1;
        });
        // Oldest surviving entries were overwritten; window is strictly
        // ascending and ends at the newest seq.
        assert_eq!(seen[0], 5);
        assert_eq!(seen[MAX_EVENTS - 1], MAX_EVENTS as u64 + 4);
        for pair in seen.windows(2) {
            assert!(pair[0] < pair[1]);
        }

        let mut tail = [0u64; 2];
        let mut cursor = 0usize;
        log.last_n(2, |record| {
            tail[cursor] = record.seq;
            cursor += 1;
        });
        assert_eq!(tail, [MAX_EVENTS as u64 + 3, MAX_EVENTS as u64 + 4]);

        let empty = EventLog::new();
        empty.last_n(4, |_| panic!("no events expected"));
        assert_eq!(empty.len(), 0);
    }

    #[test]
    fn device_record_packing_roundtrips_all_fields() {
        let record = DeviceRecord {
            id: 0xabcd,
            class: DeviceClass::Printer,
            backend: 0x7f,
            flags: 0x1234,
            meta: 0xbeef,
        };
        let word = pack_device_record(&record);
        assert_eq!(word, 0xbeef_1234_7f_07_abcd);
        assert_eq!(unpack_device_record(word), record);
    }

    #[test]
    fn state_attach_detach_writes_events_and_printer_stays_unimplemented() {
        let mut state = PeripheralServiceState::new();
        let attached = state
            .attach(device_family::INPUT, 1, 1, 0, 2, 100)
            .expect("attach");
        assert_eq!(attached.class, DeviceClass::Keyboard);
        assert_eq!(state.events.attach_total(), 1);
        assert_eq!(state.events.len(), 1);

        state.detach(attached.id, 150).expect("detach");
        assert_eq!(state.events.detach_total(), 1);
        assert_eq!(state.events.len(), 2);
        assert_eq!(state.registry.count_matching(None), 0);
        let mut kinds = [0u64; 2];
        let mut index = 0usize;
        state.events.last_n(2, |event| {
            kinds[index] = event.kind as u64;
            index += 1;
        });
        assert_eq!(kinds, [EventKind::Attach as u64, EventKind::Detach as u64]);

        assert_eq!(
            state.detach(999, 200),
            Err(PeripheralError::NotFound),
        );

        // Printer stub: full query shape, honest status.
        let report = printer_report();
        assert_eq!(report.status, PrinterStatus::Unimplemented);
        assert_eq!(report.queue_depth, 0);
        assert_eq!(report.queue_capacity, PRINTER_QUEUE_CAPACITY);
    }
}

//! Power policy, suspend groundwork, and battery/thermal/device health
//! reporting for ServiceOS.
//!
//! Honest scope note: QEMU's ACPI exposes no usable battery device by
//! default, S3 suspend is not reliably implementable under QEMU TCG, and
//! userspace currently has no port-IO or physical-memory access path, so the
//! battery probes (ACPI table walk, PM-port status sample) ship as pure,
//! host-tested logic that reports graceful absence states at runtime until a
//! kernel ACPI/fw-cfg snapshot contract exists. Thermal sensors are likewise
//! unavailable in v0; the periodic health snapshot (uptime ticks plus
//! inter-sample delta, with explicit `unavailable` markers) is the honest
//! "device health" baseline until wall-clock and kernel MemoryStats
//! contracts are exposed through the ABI.

#![cfg_attr(not(test), no_std)]

pub const MAX_INHIBITS: usize = 8;
pub const MAX_LISTENERS: usize = 4;
pub const OWNER_WORDS: usize = 2;

/// Wire tags for the power service's own control channel. Requests carry a
/// reply channel as handles[0]; replies are status-first (`PowerError::
/// to_code`, 0 = Ok) followed by op-specific words.
pub mod power_tag {
    pub const STATUS_REQUEST: u32 = 0x250;
    pub const STATUS_REPLY: u32 = 0x251;
    pub const INHIBIT_ACQUIRE_REQUEST: u32 = 0x252;
    pub const INHIBIT_ACQUIRE_REPLY: u32 = 0x253;
    pub const INHIBIT_RELEASE_REQUEST: u32 = 0x254;
    pub const INHIBIT_RELEASE_REPLY: u32 = 0x255;
    pub const LISTENER_ADD_REQUEST: u32 = 0x256;
    pub const LISTENER_ADD_REPLY: u32 = 0x257;
    pub const LISTENER_REMOVE_REQUEST: u32 = 0x258;
    pub const LISTENER_REMOVE_REPLY: u32 = 0x259;
    /// Operator-issued dry-run: broadcasts prepare-for-suspend to listeners.
    /// No actual sleep is performed.
    pub const SUSPEND_PREPARE_REQUEST: u32 = 0x25A;
    pub const SUSPEND_PREPARE_REPLY: u32 = 0x25B;
    pub const HEALTH_SNAPSHOT_REQUEST: u32 = 0x25C;
    pub const HEALTH_SNAPSHOT_REPLY: u32 = 0x25D;
    /// Broadcast-only tag pushed to registered listener channels.
    pub const SUSPEND_PREPARE_EVENT: u32 = 0x25E;
}

/// Sleep policy flags: auto-suspend stays ALLOW while no inhibitor holds a
/// cookie, and flips to INHIBIT for as long as at least one is held.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SleepState {
    Allow = 0,
    Inhibited = 1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerError {
    InvalidArgument,
    CapacityExceeded,
    UnknownCookie,
    UnknownListener,
}

impl PowerError {
    /// Wire status code: 0 = Ok, errors count up from 1.
    pub fn to_code(self) -> u32 {
        match self {
            PowerError::InvalidArgument => 1,
            PowerError::CapacityExceeded => 2,
            PowerError::UnknownCookie => 3,
            PowerError::UnknownListener => 4,
        }
    }
}

/// Refcounted inhibit registry backing the SLEEP_INHIBIT/ALLOW policy flags:
/// each acquire takes one slot (and a cookie); auto-suspend is gated on
/// `inhibit_count() == 0`.
#[derive(Clone, Copy)]
pub struct InhibitEntry {
    pub cookie: u64,
    pub owner: [u64; OWNER_WORDS],
}

#[derive(Clone, Copy)]
pub struct PowerPolicy {
    inhibits: [Option<InhibitEntry>; MAX_INHIBITS],
    next_cookie: u64,
}

impl PowerPolicy {
    pub fn new() -> Self {
        Self {
            inhibits: [None; MAX_INHIBITS],
            next_cookie: 1,
        }
    }

    pub fn acquire(&mut self, owner: [u64; OWNER_WORDS]) -> Result<u64, PowerError> {
        let slot = self
            .inhibits
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(PowerError::CapacityExceeded)?;
        let cookie = self.next_cookie;
        self.next_cookie += 1;
        *slot = Some(InhibitEntry { cookie, owner });
        Ok(cookie)
    }

    pub fn release(&mut self, cookie: u64) -> Result<(), PowerError> {
        let slot = self
            .inhibits
            .iter_mut()
            .find(|slot| matches!(slot, Some(entry) if entry.cookie == cookie))
            .ok_or(PowerError::UnknownCookie)?;
        *slot = None;
        Ok(())
    }

    pub fn inhibit_count(&self) -> usize {
        self.inhibits.iter().filter(|slot| slot.is_some()).count()
    }

    pub fn sleep_state(&self) -> SleepState {
        if self.inhibit_count() > 0 {
            SleepState::Inhibited
        } else {
            SleepState::Allow
        }
    }
}

impl Default for PowerPolicy {
    fn default() -> Self {
        Self::new()
    }
}

/// Registered prepare-for-suspend listeners. Handles are duplicates owned by
/// the service; cookies let holders unregister.
#[derive(Clone, Copy)]
pub struct ListenerSlot {
    pub cookie: u64,
    pub handle: u64,
}

#[derive(Clone, Copy)]
pub struct ListenerTable {
    slots: [Option<ListenerSlot>; MAX_LISTENERS],
    next_cookie: u64,
}

impl ListenerTable {
    pub fn new() -> Self {
        Self {
            slots: [None; MAX_LISTENERS],
            next_cookie: 1,
        }
    }

    pub fn add(&mut self, handle: u64) -> Result<u64, PowerError> {
        if handle == 0 {
            return Err(PowerError::InvalidArgument);
        }
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(PowerError::CapacityExceeded)?;
        let cookie = self.next_cookie;
        self.next_cookie += 1;
        *slot = Some(ListenerSlot { cookie, handle });
        Ok(cookie)
    }

    /// Remove a listener, returning its duplicated handle so the caller can
    /// close it.
    pub fn remove(&mut self, cookie: u64) -> Result<u64, PowerError> {
        let position = self
            .slots
            .iter()
            .position(|slot| matches!(slot, Some(entry) if entry.cookie == cookie))
            .ok_or(PowerError::UnknownListener)?;
        let entry = self.slots[position].take();
        Ok(entry.map_or(0, |listener| listener.handle))
    }

    /// Live listener slots in registration order.
    pub fn slots(&self) -> [Option<ListenerSlot>; MAX_LISTENERS] {
        self.slots
    }
}

impl Default for ListenerTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Delivery plan for one prepare-for-suspend broadcast: the sequence number
/// stamped into every event and the live targets to attempt.
#[derive(Clone, Copy)]
pub struct BroadcastPlan {
    pub sequence: u64,
    pub targets: [Option<ListenerSlot>; MAX_LISTENERS],
    pub count: usize,
}

impl ListenerTable {
    pub fn plan_broadcast(&self, sequence: u64) -> BroadcastPlan {
        let mut targets = [None; MAX_LISTENERS];
        let mut count = 0usize;
        for slot in self.slots.iter().flatten() {
            targets[count] = Some(*slot);
            count += 1;
        }
        BroadcastPlan {
            sequence,
            targets,
            count,
        }
    }
}

/// Monotonic broadcast sequence for prepare-for-suspend events.
pub fn next_event_sequence(sequence: u64) -> u64 {
    sequence.wrapping_add(1)
}

/// How the current battery report was produced. `NotAvailable` is the
/// graceful absence state used whenever the platform transport (kernel ACPI
/// snapshot or PM-port sampler) has not been wired up yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProbeEvidence {
    NotAvailable = 0,
    AcpiTableWalk = 1,
    PmPortStatus = 2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Presence {
    Unknown = 0,
    Absent = 1,
    Present = 2,
}

/// Battery/power-source report. Detail codes document exactly why a state
/// was reached: 1 = no userspace transport for ACPI tables or PM ports yet,
/// 2 = ACPI walk found no PNP0C0A battery device, 3 = DSDT declares a
/// PNP0C0A battery device (control methods not evaluated), 4 = PM status
/// sample reports no battery, 5 = PM status sample asserts the battery bit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BatteryReport {
    pub evidence: ProbeEvidence,
    pub presence: Presence,
    pub detail_code: u8,
}

impl BatteryReport {
    pub fn unavailable() -> Self {
        Self {
            evidence: ProbeEvidence::NotAvailable,
            presence: Presence::Unknown,
            detail_code: 1,
        }
    }
}

/// AML-less ACPI table walk helpers over raw byte slices. Composition
/// (turning physical table addresses into slices) stays with the transport
/// side because userspace cannot map physical memory today.

pub mod acpi {
    pub const RSDP_SIGNATURE: &[u8; 8] = b"RSD PTR ";
    pub const ROOT_XSDT: &[u8; 4] = b"XSDT";
    pub const ROOT_RSDT: &[u8; 4] = b"RSDT";
    pub const MAX_ROOT_ENTRIES: usize = 32;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct RootTable {
        pub signature: [u8; 4],
        pub entries: [u64; MAX_ROOT_ENTRIES],
        pub count: usize,
    }

    /// Encode a 7-character PNP id ("PNP0C0A") into the 4 compressed EISAID
    /// bytes AML embeds after the 0x0C DWORD prefix.
    pub fn eisaid_bytes(id: &[u8; 7]) -> [u8; 4] {
        fn letter(byte: u8) -> u64 {
            byte.wrapping_sub(0x40) as u64 & 0x7F
        }
        fn nibble(byte: u8) -> u64 {
            match byte {
                b'A'..=b'F' => (byte - b'A' + 10) as u64,
                _ => byte.wrapping_sub(b'0') as u64 & 0xF,
            }
        }
        let c0 = letter(id[0]);
        let c1 = letter(id[1]);
        let c2 = letter(id[2]);
        [
            ((c0 << 2) | (c1 >> 3)) as u8,
            (((c1 & 0x7) << 5) | c2) as u8,
            ((nibble(id[3]) << 4) | nibble(id[4])) as u8,
            ((nibble(id[5]) << 4) | nibble(id[6])) as u8,
        ]
    }

    /// Scan a memory region for a valid RSDP (16-byte aligned, signature +
    /// revision-1 checksum mandatory, extended checksum when revision >= 2).
    pub fn find_rsdp(region: &[u8]) -> Option<usize> {
        if region.len() < 20 {
            return None;
        }
        let mut offset = 0usize;
        while offset + 20 <= region.len() {
            if region[offset..offset + 8] == *RSDP_SIGNATURE
                && checksum(&region[offset..offset + 20])
            {
                let revision = region[offset + 15];
                if revision < 2 {
                    return Some(offset);
                }
                if offset + 36 <= region.len() && checksum(&region[offset..offset + 36]) {
                    return Some(offset);
                }
            }
            offset += 16;
        }
        None
    }

    /// Root table kind implied by an RSDP: XSDT when revision >= 2 carries a
    /// non-zero XSDT pointer, RSDT otherwise.
    pub fn rsdp_root_signature(rsdp: &[u8]) -> Option<[u8; 4]> {
        if rsdp.len() < 20 || rsdp[0..8] != *RSDP_SIGNATURE {
            return None;
        }
        if rsdp[15] >= 2 && rsdp.len() >= 36 {
            let xsdt = u64::from_le_bytes(rsdp[24..32].try_into().ok()?);
            if xsdt != 0 {
                return Some(*ROOT_XSDT);
            }
        }
        Some(*ROOT_RSDT)
    }

    /// Parse an XSDT/RSDT body into its table-entry list (physical
    /// addresses; u64 entries for XSDT, u32 for RSDT).
    pub fn parse_root_table(bytes: &[u8]) -> Option<RootTable> {
        if bytes.len() < 36 {
            return None;
        }
        let mut signature = [0u8; 4];
        signature.copy_from_slice(&bytes[0..4]);
        let is_xsdt = signature == *ROOT_XSDT;
        if !is_xsdt && signature != *ROOT_RSDT {
            return None;
        }
        let length = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        if length < 36 || length > bytes.len() {
            return None;
        }
        let entry_size = if is_xsdt { 8 } else { 4 };
        let mut entries = [0u64; MAX_ROOT_ENTRIES];
        let mut count = 0usize;
        let mut offset = 36usize;
        while offset + entry_size <= length && count < MAX_ROOT_ENTRIES {
            let end = offset + entry_size;
            entries[count] = if is_xsdt {
                u64::from_le_bytes(bytes[offset..end].try_into().ok()?)
            } else {
                u32::from_le_bytes(bytes[offset..end].try_into().ok()?) as u64
            };
            count += 1;
            offset = end;
        }
        Some(RootTable {
            signature,
            entries,
            count,
        })
    }

    /// Search compiled AML for a device-id declaration (the 4 compressed
    /// EISAID bytes). This is the honest heuristic behind the ACPI battery
    /// probe: it proves a PNP0C0A device node exists without evaluating any
    /// control method.
    pub fn scan_aml_for_eisaid(aml: &[u8], id: &[u8; 7]) -> bool {
        let pattern = eisaid_bytes(id);
        aml.windows(4).any(|window| window == pattern)
    }

    fn checksum(bytes: &[u8]) -> bool {
        bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) == 0
    }
}

/// ACPI-walk battery probe: feed the DSDT body when a kernel snapshot
/// contract exists, otherwise `None` produces the graceful absence state.
pub fn acpi_battery_report(dsdt: Option<&[u8]>) -> BatteryReport {
    match dsdt {
        Some(dsdt) if acpi::scan_aml_for_eisaid(dsdt, b"PNP0C0A") => BatteryReport {
            evidence: ProbeEvidence::AcpiTableWalk,
            presence: Presence::Present,
            detail_code: 3,
        },
        Some(_) => BatteryReport {
            evidence: ProbeEvidence::AcpiTableWalk,
            presence: Presence::Absent,
            detail_code: 2,
        },
        None => BatteryReport::unavailable(),
    }
}

/// Secondary probe over a sampled piix4-style PM status byte (IO base 0x600
/// region). The battery bit position is platform-defined, so the caller
/// supplies the mask from its platform table; `None` (no sampling transport
/// in userspace today) degrades to the absence state.
pub fn pm_port_battery_report(status_byte: Option<u8>, battery_mask: u8) -> BatteryReport {
    match status_byte {
        Some(byte) if byte & battery_mask != 0 => BatteryReport {
            evidence: ProbeEvidence::PmPortStatus,
            presence: Presence::Present,
            detail_code: 5,
        },
        Some(_) => BatteryReport {
            evidence: ProbeEvidence::PmPortStatus,
            presence: Presence::Absent,
            detail_code: 4,
        },
        None => BatteryReport::unavailable(),
    }
}

/// One periodic health sample. `drift_estimate_ppm` is honestly `None` in v0:
/// userspace sees only the monotonic tick counter, and no PIT/RTC wall-clock
/// contract exists to diff against. `memory_pressure_percent` is honestly
/// `None` in v0: kernel `MemoryStats` are not exposed through the ABI yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HealthSnapshot {
    pub now_ticks: u64,
    pub prev_ticks: Option<u64>,
    pub tick_delta: u64,
    pub drift_estimate_ppm: Option<i64>,
    pub memory_pressure_percent: Option<u8>,
}

impl HealthSnapshot {
    pub fn flags(&self) -> u64 {
        let mut flags = 0u64;
        if self.prev_ticks.is_some() {
            flags |= 1;
        }
        if self.drift_estimate_ppm.is_some() {
            flags |= 2;
        }
        if self.memory_pressure_percent.is_some() {
            flags |= 4;
        }
        flags
    }
}

/// Build the health snapshot from the previous sample tick and the monotonic
/// counter now. Without a previous sample the delta equals uptime.
pub fn health_snapshot(prev: Option<u64>, now: u64) -> HealthSnapshot {
    let tick_delta = match prev {
        Some(prev) if prev <= now => now - prev,
        _ => now,
    };
    HealthSnapshot {
        now_ticks: now,
        prev_ticks: prev,
        tick_delta,
        drift_estimate_ppm: None,
        memory_pressure_percent: None,
    }
}

fn write_text(out: &mut [u8], position: &mut usize, text: &[u8]) -> bool {
    if *position + text.len() > out.len() {
        return false;
    }
    out[*position..*position + text.len()].copy_from_slice(text);
    *position += text.len();
    true
}

fn write_number(out: &mut [u8], position: &mut usize, value: u64) -> bool {
    let mut scratch = [0u8; 20];
    let mut count = 0usize;
    let mut rest = value;
    loop {
        scratch[count] = b'0' + (rest % 10) as u8;
        rest /= 10;
        count += 1;
        if rest == 0 {
            break;
        }
    }
    if *position + count > out.len() {
        return false;
    }
    for index in (0..count).rev() {
        out[*position] = scratch[index];
        *position += 1;
    }
    true
}

/// Human-readable status block (ASCII lines, NUL-free) covering policy
/// state, the honest battery report, and the latest health snapshot.
/// Returns the written length, or None when `out` is too small.
pub fn format_status_text(
    policy: &PowerPolicy,
    battery: &BatteryReport,
    health: &HealthSnapshot,
    out: &mut [u8],
) -> Option<usize> {
    let mut position = 0usize;
    let state = match policy.sleep_state() {
        SleepState::Inhibited => "power: state=inhibited inhibits=",
        SleepState::Allow => "power: state=allow inhibits=",
    };
    if !write_text(out, &mut position, state.as_bytes()) {
        return None;
    }
    if !write_number(out, &mut position, policy.inhibit_count() as u64) {
        return None;
    }
    if !write_text(out, &mut position, b"\npower: battery evidence=") {
        return None;
    }
    if !write_number(out, &mut position, battery.evidence as u64) {
        return None;
    }
    if !write_text(out, &mut position, b" presence=") {
        return None;
    }
    if !write_number(out, &mut position, battery.presence as u64) {
        return None;
    }
    if !write_text(out, &mut position, b" detail=") {
        return None;
    }
    if !write_number(out, &mut position, battery.detail_code as u64) {
        return None;
    }
    if !write_text(out, &mut position, b"\npower: health uptime-ticks=") {
        return None;
    }
    if !write_number(out, &mut position, health.now_ticks) {
        return None;
    }
    if !write_text(out, &mut position, b" delta=") {
        return None;
    }
    if !write_number(out, &mut position, health.tick_delta) {
        return None;
    }
    if !write_text(
        out,
        &mut position,
        b" drift=unavailable mem-pressure=unavailable\n",
    ) {
        return None;
    }
    Some(position)
}

/// Pack `bytes` into wire words (big-endian, zero padded). Returns the
/// number of words written.
pub fn pack_words(out: &mut [u64], bytes: &[u8]) -> usize {
    let words = bytes.len().div_ceil(8);
    for (index, word) in out.iter_mut().enumerate().take(words) {
        let mut value = 0u64;
        for slot in 0..8 {
            let position = index * 8 + slot;
            let byte = if position < bytes.len() {
                bytes[position]
            } else {
                0
            };
            value |= (byte as u64) << (56 - slot * 8);
        }
        *word = value;
    }
    words
}

/// Unpack `byte_len` bytes previously written by `pack_words` into `out`.
pub fn unpack_words(words: &[u64], byte_len: usize, out: &mut [u8]) -> Result<(), PowerError> {
    if byte_len > out.len() || byte_len > words.len() * 8 {
        return Err(PowerError::InvalidArgument);
    }
    for position in 0..byte_len {
        let word = words[position / 8];
        let shift = 56 - (position % 8) * 8;
        out[position] = ((word >> shift) & 0xFF) as u8;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eisaid_matches_known_pnp0501_encoding() {
        // "PNP0501" appears in real DSDTs as 41 D0 05 01.
        assert_eq!(acpi::eisaid_bytes(b"PNP0501"), [0x41, 0xD0, 0x05, 0x01]);
    }

    #[test]
    fn battery_eisaid_pattern_is_scanned_in_aml() {
        let pattern = acpi::eisaid_bytes(b"PNP0C0A");
        assert_eq!(pattern, [0x41, 0xD0, 0x0C, 0x0A]);
        let mut aml = [0u8; 16];
        assert!(!acpi::scan_aml_for_eisaid(&aml, b"PNP0C0A"));
        aml[7..11].copy_from_slice(&[0x5B, 0x82, 0x41, pattern[0]]);
        assert!(!acpi::scan_aml_for_eisaid(&aml, b"PNP0C0A"));
        aml[9..13].copy_from_slice(&pattern);
        assert!(acpi::scan_aml_for_eisaid(&aml, b"PNP0C0A"));
    }

    fn build_rsdp(revision: u8, xsdt_address: u64) -> [u8; 36] {
        // Layout: sig[0..8], checksum[8], oemid[9..15], revision[15],
        // rsdt[16..20], length[20..24], xsdt[24..32], xchecksum[32].
        let mut rsdp = [0u8; 36];
        rsdp[0..8].copy_from_slice(acpi::RSDP_SIGNATURE);
        rsdp[15] = revision;
        if revision >= 2 {
            rsdp[24..32].copy_from_slice(&xsdt_address.to_le_bytes());
        }
        let mut sum = rsdp[..20]
            .iter()
            .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
        rsdp[8] = sum.wrapping_neg();
        if revision >= 2 {
            sum = rsdp[..32]
                .iter()
                .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
            rsdp[32] = sum.wrapping_neg();
        }
        rsdp
    }

    #[test]
    fn find_rsdp_validates_checksums_and_alignment() {
        let mut region = [0u8; 128];
        assert_eq!(acpi::find_rsdp(&region), None);
        let rsdp = build_rsdp(2, 0xdead_beef);
        region[48..84].copy_from_slice(&rsdp);
        assert_eq!(acpi::find_rsdp(&region), Some(48));

        let mut corrupted = region;
        corrupted[60] ^= 0xFF;
        assert_eq!(acpi::find_rsdp(&corrupted), None);
    }

    #[test]
    fn find_rsdp_rejects_bad_signature() {
        let mut region = [0u8; 64];
        let mut rsdp = build_rsdp(0, 0);
        rsdp[3] = b'X';
        let sum = rsdp[..20]
            .iter()
            .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
        rsdp[8] = sum.wrapping_neg();
        region[16..36].copy_from_slice(&rsdp[..20]);
        assert_eq!(acpi::find_rsdp(&region), None);
    }

    #[test]
    fn rsdp_root_signature_selects_xsdt_or_rsdt() {
        let legacy = build_rsdp(0, 0);
        assert_eq!(acpi::rsdp_root_signature(&legacy), Some(*acpi::ROOT_RSDT));
        let modern = build_rsdp(2, 0x1000);
        assert_eq!(acpi::rsdp_root_signature(&modern), Some(*acpi::ROOT_XSDT));
        let modern_without_xsdt = build_rsdp(2, 0);
        assert_eq!(
            acpi::rsdp_root_signature(&modern_without_xsdt),
            Some(*acpi::ROOT_RSDT)
        );
    }

    #[test]
    fn parse_root_table_reads_xsdt_u64_entries() {
        let mut table = [0u8; 52];
        table[0..4].copy_from_slice(acpi::ROOT_XSDT);
        table[4..6].copy_from_slice(&52u16.to_le_bytes());
        table[36..44].copy_from_slice(&0xAA00u64.to_le_bytes());
        table[44..52].copy_from_slice(&0xBB00u64.to_le_bytes());
        let root = acpi::parse_root_table(&table).expect("root");
        assert_eq!(root.count, 2);
        assert_eq!(root.entries[0], 0xAA00);
        assert_eq!(root.entries[1], 0xBB00);
    }

    #[test]
    fn parse_root_table_reads_rsdt_u32_entries_and_caps_count() {
        let mut table = [0u8; acpi::MAX_ROOT_ENTRIES * 4 + 40];
        table[0..4].copy_from_slice(acpi::ROOT_RSDT);
        let length = table.len() as u16;
        table[4..6].copy_from_slice(&length.to_le_bytes());
        for index in 0..acpi::MAX_ROOT_ENTRIES {
            let offset = 36 + index * 4;
            let value = (0x1000u32 + index as u32).to_le_bytes();
            table[offset..offset + 4].copy_from_slice(&value);
        }
        let root = acpi::parse_root_table(&table).expect("root");
        assert_eq!(root.count, acpi::MAX_ROOT_ENTRIES);

        // A length claiming more entries than the cap still parses only the
        // capped prefix rather than overrunning.
        let mut longer = [0u8; acpi::MAX_ROOT_ENTRIES * 4 + 44];
        longer[0..4].copy_from_slice(acpi::ROOT_RSDT);
        let longer_length = longer.len() as u16;
        longer[4..6].copy_from_slice(&longer_length.to_le_bytes());
        let capped = acpi::parse_root_table(&longer).expect("root");
        assert_eq!(capped.count, acpi::MAX_ROOT_ENTRIES);
    }

    #[test]
    fn parse_root_table_rejects_foreign_signatures_and_short_bodies() {
        assert_eq!(acpi::parse_root_table(&[0u8; 35]), None);
        let mut table = [0u8; 36];
        table[0..4].copy_from_slice(b"FACP");
        assert_eq!(acpi::parse_root_table(&table), None);
    }

    #[test]
    fn acpi_battery_report_covers_all_three_states() {
        assert_eq!(
            acpi_battery_report(None),
            BatteryReport {
                evidence: ProbeEvidence::NotAvailable,
                presence: Presence::Unknown,
                detail_code: 1,
            }
        );
        let absent_aml = [0x10u8, 0x20, 0x30, 0x40];
        assert_eq!(
            acpi_battery_report(Some(&absent_aml)),
            BatteryReport {
                evidence: ProbeEvidence::AcpiTableWalk,
                presence: Presence::Absent,
                detail_code: 2,
            }
        );
        let pattern = acpi::eisaid_bytes(b"PNP0C0A");
        let present_aml = [pattern[0], pattern[1], pattern[2], pattern[3]];
        assert_eq!(
            acpi_battery_report(Some(&present_aml)),
            BatteryReport {
                evidence: ProbeEvidence::AcpiTableWalk,
                presence: Presence::Present,
                detail_code: 3,
            }
        );
    }

    #[test]
    fn pm_port_probe_reports_masked_status_or_absence() {
        assert_eq!(
            pm_port_battery_report(None, 0x10),
            BatteryReport::unavailable()
        );
        assert_eq!(
            pm_port_battery_report(Some(0xEF), 0x10).presence,
            Presence::Absent
        );
        assert_eq!(
            pm_port_battery_report(Some(0x1F), 0x10).presence,
            Presence::Present
        );
    }

    #[test]
    fn event_sequence_wraps_monotonically() {
        assert_eq!(next_event_sequence(0), 1);
        assert_eq!(next_event_sequence(u64::MAX), 0);
    }

    #[test]
    fn health_snapshot_handles_gaps_and_backwards_ticks() {
        let first = health_snapshot(None, 900);
        assert_eq!(first.prev_ticks, None);
        assert_eq!(first.tick_delta, 900);
        let normal = health_snapshot(Some(900), 1200);
        assert_eq!(normal.tick_delta, 300);
        let backwards = health_snapshot(Some(5000), 1200);
        assert_eq!(backwards.tick_delta, 1200);
    }

    #[test]
    fn pack_unpack_words_roundtrip() {
        let mut words = [0u64; OWNER_WORDS];
        assert_eq!(pack_words(&mut words, b"shell"), 1);
        let mut out = [0u8; OWNER_WORDS * 8];
        unpack_words(&words, 5, &mut out).expect("unpack");
        assert_eq!(&out[..5], b"shell");
        assert_eq!(
            unpack_words(&words, OWNER_WORDS * 8 + 1, &mut out),
            Err(PowerError::InvalidArgument)
        );
    }
}

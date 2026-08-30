//! Raspberry Pi 5 VideoCore mailbox driver (property-tags channel).
//!
//! Implements the ARM side of the Broadcom mailbox property-channel protocol
//! used to negotiate firmware allocations such as the display framebuffer.
//! Wire format follows the published Broadcom/Raspberry Pi specification:
//!
//! - envelope: `[total-bytes][request/response-code][tags...][END]` in little-
//!   endian u32 words, staged in storage aligned to 16 bytes (the smallest
//!   legal envelope, one reserved-empty tag, spans 28 bytes),
//! - each tag: `[id][value-buffer-bytes][request-resp-code][values...]`;
//!   on response the second word carries the *actual* returned byte count and
//!   bit 31 of the third word marks completion (lower bits must be zero).
//!
//! Hardware reachability: the register interface follows `brcm,bcm2835-mbox`
//! (READ 0x00, STATUS 0x18, WRITE 0x20; STATUS bit31 = FULL, bit30 = EMPTY).
//! The MMIO base is discovered from the device tree rather than pinned to a
//! BCM2712 constant because the Pi 5 moved peripherals relative to older
//! boards. NOTHING HERE HAS EXECUTED AGAINST REAL VIDEOCORE SILICON YET —
//! QEMU (including `virt`) exposes no raspi5 mailbox, so hardware-facing paths
//! are implemented per spec and marked UNTESTED WITHOUT HARDWARE.
// #[allow(dead_code)]: full spec surface awaiting physical VC hardware;
// exercised via tests/protocol.rs golden-wire coverage until then.
#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, Ordering};

/// Channel number of the property-tags (ARM-to-VideoCore) mailbox.
pub const CHANNEL_PROPERTY_TAGS_ARM_TO_VC: u32 = 8;

/// Request-phase marker written into the envelope's code word.
const REQUEST_CODE: u32 = 0;
/// Success marker the firmware writes into the response-code word.
pub const RESPONSE_CODE_OK: u32 = 0x8000_0000;
/// Failure marker the firmware writes into the response-code word.
pub const RESPONSE_CODE_ERROR: u32 = 0x8000_0001;
/// Bit set by the firmware inside each tag's request/resp word on completion.
pub const TAG_RESPONSE_BIT: u32 = 0x8000_0000;
/// Terminator tag.
const TAG_END: u32 = 0;

/// Firmware revision query (GET_FIRMWARE_REVISION).
pub const TAG_GET_FIRMWARE_REVISION: u32 = 0x0000_0001;
/// Allocate contiguous framebuffer memory (ALLOCATE_BUFFER; align arg).
pub const TAG_ALLOCATE_BUFFER: u32 = 0x0004_0001;
/// Set the scanout-backed display size (SET_PHYSICAL_WIDTH_HEIGHT).
pub const TAG_SET_PHYSICAL_WIDTH_HEIGHT: u32 = 0x0004_8003;
/// Set the rendered surface size (SET_VIRTUAL_WIDTH_HEIGHT).
pub const TAG_SET_VIRTUAL_WIDTH_HEIGHT: u32 = 0x0004_8004;
/// Set the pixel depth in bits (SET_DEPTH).
pub const TAG_SET_DEPTH: u32 = 0x0004_8005;
/// Set the virtual-surface visible origin (SET_VIRTUAL_OFFSET).
pub const TAG_SET_VIRTUAL_OFFSET: u32 = 0x0004_8009;
/// Query the allocated row pitch in bytes (GET_PITCH).
pub const TAG_GET_PITCH: u32 = 0x0004_0008;

/// Envelope storage alignment demanded by the mailbox hardware (bytes).
pub const BUFFER_MIN_ALIGN_BYTES: usize = 16;
/// Word capacity of the statically staged envelope (256-byte budget).
pub const MAX_PROPERTY_WORDS: usize = 64;

/// Mailbox STATUS FULL flag — TX blocked while set.
const STATUS_FULL: u32 = 0x8000_0000;
/// Mailbox STATUS EMPTY flag — RX has data while clear.
const STATUS_EMPTY: u32 = 0x4000_0000;

/// Register offsets shared across the `brcm,bcm2835-mbox` family.
mod regs {
    /// Read slot.
    pub const READ: usize = 0x00;
    /// Status flags.
    pub const STATUS: usize = 0x18;
    /// Write slot.
    pub const WRITE: usize = 0x20;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MailboxError {
    /// Envelope exceeds [`MAX_PROPERTY_WORDS`] or violates wire format while building.
    InvalidRequest,
    /// Staged buffers must live below 4 GiB for the legacy mailbox descriptor.
    AddressAbove4GiB,
    /// Envelope storage weaker than [`BUFFER_MIN_ALIGN_BYTES`].
    MisalignedBuffer,
    /// Firmware never drained the write slot or never completed the exchange.
    Timeout,
    /// Firmware answered the envelope-level request code with an error.
    DeviceRejected(u32),
    /// A tag's completion word lacks the response bit or reports an error code.
    TagRejected(u32, u32),
    /// Response envelope truncated, over-long, or inconsistent.
    MalformedResponse,
    /// A response tag claims more bytes than the request reserved.
    ResponseLargerThanRequest(u32),
}

impl core::fmt::Display for MailboxError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MailboxStatus {
    pub implemented: bool,
    pub discovered: bool,
}

static MAILBOX_DISCOVERED: AtomicBool = AtomicBool::new(false);
static mut MAILBOX_BASE: u64 = 0;

/// Record the DTB-resolved mailbox register range. Returns false for an empty
/// range; discovery state feeds [`status`] and the framebuffer bring-up flow.
pub fn discover(range_base: u64, range_span: usize) -> bool {
    if range_span == 0 {
        return false;
    }
    unsafe {
        MAILBOX_BASE = range_base;
    }
    MAILBOX_DISCOVERED.store(true, Ordering::Release);
    true
}

/// Register base recorded by [`discover`], if any.
pub fn base() -> Option<u64> {
    if !MAILBOX_DISCOVERED.load(Ordering::Acquire) {
        return None;
    }
    Some(unsafe { MAILBOX_BASE })
}

pub fn status() -> MailboxStatus {
    MailboxStatus {
        implemented: true,
        discovered: MAILBOX_DISCOVERED.load(Ordering::Acquire),
    }
}

// ---------------------------------------------------------------------------
// Bus-address translation helpers (documented assumption)
//
// Legacy VideoCore addressing aliases SDRAM twice: the uncached-peripheral
// view starts at 0x4000_0000 and the L2-differing view at 0xC000_0000. Both
// normalize by masking the top two bits. ASSUMPTION (UNVERIFIED ON BCM2712
// SILICON): Pi 5 firmware keeps honoring these aliases for ALLOCATE_BUFFER
// responses, matching long-standing Pi-family behavior.
// ---------------------------------------------------------------------------

/// VideoCore bus address where the plain-DRAM alias window begins.
pub const VC_BUS_SDRAM_ALIAS: u64 = 0x4000_0000;
/// Top-two-bit mask covering both legacy SDRAM alias windows.
pub const VC_BUS_ALIAS_MASK: u64 = 0xC000_0000;

/// Translate an ARM physical address below 4 GiB into its VideoCore bus
/// address using the plain DRAM alias window. Returns [`None`] above the
/// legacy 32-bit window the mailbox descriptor can express.
pub fn physical_to_bus(address: u64) -> Option<u64> {
    if address >= 1 << 32 {
        return None;
    }
    Some((address & !VC_BUS_ALIAS_MASK) | VC_BUS_SDRAM_ALIAS)
}

/// Translate a VideoCore bus address back into an ARM physical address.
/// Accepts either SDRAM alias window; rejects peripheral-window encodings.
pub fn bus_to_physical(bus_address: u64) -> Option<u64> {
    match bus_address & VC_BUS_ALIAS_MASK {
        VC_BUS_SDRAM_ALIAS | 0xC000_0000 => Some(bus_address & !VC_BUS_ALIAS_MASK),
        _ => None,
    }
}

/// Geometry rules for turning validated ALLOCATE_BUFFER + GET_PITCH payloads
/// into boot-framebuffer facts. Kept beside the protocol code because its
/// rejection rules encode mailbox-contract assumptions (stride covers a full
/// row, allocation backs every scanout byte). Hardware-agnostic by design.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameGeometry {
    pub physical_base: u64,
    pub width: usize,
    pub height: usize,
    pub stride_bytes: usize,
    pub bytes_per_pixel: usize,
    pub byte_len: usize,
}

/// Combine validated allocation + pitch payloads into [`FrameGeometry`].
pub fn assemble_geometry(
    bus_address: u64,
    allocated_bytes: u32,
    width: usize,
    height: usize,
    pitch_bytes: usize,
    bytes_per_pixel: usize,
) -> Option<FrameGeometry> {
    if width == 0
        || height == 0
        || bytes_per_pixel == 0
        || pitch_bytes < width.checked_mul(bytes_per_pixel)?
    {
        return None;
    }
    let physical_base = bus_to_physical(bus_address)?;
    let byte_len = pitch_bytes.checked_mul(height)?;
    if byte_len > allocated_bytes as usize {
        return None;
    }
    Some(FrameGeometry {
        physical_base,
        width,
        height,
        stride_bytes: pitch_bytes,
        bytes_per_pixel,
        byte_len,
    })
}

// ---------------------------------------------------------------------------
// Pure request builder
// ---------------------------------------------------------------------------

/// In-place builder over a fixed word arena. Produces exactly the wire layout
/// the firmware expects, reserving the terminator slot up front.
#[derive(Debug)]
pub struct PropertyRequestWriter<'a> {
    words: &'a mut [u32],
    cursor: usize,
}

impl<'a> PropertyRequestWriter<'a> {
    /// Begin an empty envelope inside `arena` (capacity ≤ [`MAX_PROPERTY_WORDS`]).
    pub fn new(arena: &'a mut [u32]) -> Result<Self, MailboxError> {
        if arena.len() < 3 || arena.len() > MAX_PROPERTY_WORDS {
            return Err(MailboxError::InvalidRequest);
        }
        arena[0] = 0;
        arena[1] = REQUEST_CODE;
        Ok(Self {
            words: arena,
            cursor: 2,
        })
    }

    /// Append a tag reserving `value_buffer_bytes` of response space. The
    /// request materializes the *full* reserved slot (supplied `values` first,
    /// zero padding behind them) so any conforming firmware reply fits inside
    /// staged words instead of trampling the following tag or the terminator
    /// — the classic mailbox integration trap this driver refuses to ship.
    pub fn push_tag(
        &mut self,
        tag_id: u32,
        values: &[u32],
        value_buffer_bytes: u32,
    ) -> Result<(), MailboxError> {
        let reserved_words = value_buffer_bytes.div_ceil(4) as usize;
        if value_buffer_bytes == 0
            || values.len() > reserved_words
            || self.cursor + 3 + reserved_words + 1 > self.words.len()
        {
            return Err(MailboxError::InvalidRequest);
        }
        self.words[self.cursor] = tag_id;
        self.words[self.cursor + 1] = value_buffer_bytes;
        self.words[self.cursor + 2] = REQUEST_CODE;
        let payload = &mut self.words[self.cursor + 3..self.cursor + 3 + reserved_words];
        payload[..values.len()].copy_from_slice(values);
        payload[values.len()..].fill(0);
        self.cursor += 3 + reserved_words;
        Ok(())
    }

    /// Terminate the envelope, stamp the byte size, return the finalized view.
    pub fn finish(self) -> &'a [u32] {
        self.words[self.cursor] = TAG_END;
        let total_bytes = (self.cursor + 1) * 4;
        self.words[0] = total_bytes as u32;
        &self.words[..self.cursor + 1]
    }
}

// ---------------------------------------------------------------------------
// Pure response parsing / validation
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TagValues<'a> {
    pub tag_id: u32,
    pub values: &'a [u32],
}

/// Walk helper shared by validation and lookup: yields `(word_index, value_words)`
/// for every tag until END.
fn tag_entries(envelope: &[u32]) -> impl Iterator<Item = (usize, usize)> + '_ {
    let mut index = 2usize;
    core::iter::from_fn(move || {
        while index + 3 <= envelope.len() {
            if envelope[index] == TAG_END {
                return None;
            }
            let value_words = envelope[index + 1].div_ceil(4) as usize;
            let entry = (index, value_words);
            index += 3 + value_words;
            return Some(entry);
        }
        None
    })
}

pub(crate) fn locate_request_tag(request: &[u32], tag_id: u32) -> Option<usize> {
    tag_entries(request)
        .map(|(index, _)| index)
        .find(|&index| request[index] == tag_id)
}

/// Validate an exchanged (in-place mutated) envelope against the request we
/// submitted. Checks overall success, per-tag completion/error bits, envelope
/// coherence up to its terminator, and the spec's response≤request byte-count
/// rule per tag.
pub fn validate_exchange(request: &[u32], response: &[u32]) -> Result<(), MailboxError> {
    if request.is_empty()
        || request.len() != response.len()
        || response[0] as usize != response.len() * 4
    {
        return Err(MailboxError::MalformedResponse);
    }
    match response[1] {
        RESPONSE_CODE_OK => {}
        other => return Err(MailboxError::DeviceRejected(other)),
    }

    let mut cursor = 2usize;
    loop {
        if cursor >= response.len() {
            return Err(MailboxError::MalformedResponse);
        }
        if response[cursor] == TAG_END {
            break;
        }
        if cursor + 3 > response.len() {
            return Err(MailboxError::MalformedResponse);
        }
        let tag_id = response[cursor];
        let value_bytes = response[cursor + 1];
        let code = response[cursor + 2];
        let value_words = value_bytes.div_ceil(4) as usize;

        let request_index =
            locate_request_tag(request, tag_id).ok_or(MailboxError::MalformedResponse)?;
        if value_bytes > request[request_index + 1] {
            return Err(MailboxError::ResponseLargerThanRequest(tag_id));
        }
        if code & TAG_RESPONSE_BIT == 0 || code & !TAG_RESPONSE_BIT != 0 {
            return Err(MailboxError::TagRejected(tag_id, code));
        }
        cursor += 3 + value_words;
        if cursor > response.len() {
            return Err(MailboxError::MalformedResponse);
        }
    }

    // Everything past the terminator must stay zero.
    if response[cursor..].iter().any(|word| *word != TAG_END) {
        return Err(MailboxError::MalformedResponse);
    }
    Ok(())
}

/// Fetch decoded values for `tag_id` out of an already-validated response.
/// The returned slice carries the actual returned byte count rounded up.
pub fn tag_values<'a>(response: &'a [u32], tag_id: u32) -> Option<TagValues<'a>> {
    let (index, words) = tag_entries(response).find(|&(index, _)| response[index] == tag_id)?;
    let end = (index + 3 + words).min(response.len());
    Some(TagValues {
        tag_id,
        values: &response[index + 3..end],
    })
}

// ---------------------------------------------------------------------------
// Register-level transport
// ---------------------------------------------------------------------------

fn read_reg(base: u64, offset: usize) -> u32 {
    unsafe { ((base + offset as u64) as *const u32).read_volatile() }
}

fn write_reg(base: u64, offset: usize, value: u32) {
    unsafe {
        ((base + offset as u64) as *mut u32).write_volatile(value);
    }
}

fn poll_status_clear(base: u64, mask: u32, deadline: u64, now: impl Fn() -> u64) -> bool {
    loop {
        if read_reg(base, regs::STATUS) & mask == 0 {
            return true;
        }
        if now() >= deadline {
            return false;
        }
    }
}

fn poll_status_set(base: u64, mask: u32, deadline: u64, now: impl Fn() -> u64) -> bool {
    loop {
        if read_reg(base, regs::STATUS) & mask != 0 {
            return true;
        }
        if now() >= deadline {
            return false;
        }
    }
}

/// Drive one property-channel transaction against `base`. `envelope` must be
/// built with [`PropertyRequestWriter`], live in storage aligned to
/// [`BUFFER_MIN_ALIGN_BYTES`], and sit below 4 GiB. The kernel identity-maps
/// its own image (see the platform boot flow), so the staged buffer's virtual
/// address equals its physical address — the assumption the descriptor build
/// relies on. On success the same slice holds the in-place firmware response;
/// run it back through [`validate_exchange`] against the original request view.
///
/// UNTESTED WITHOUT HARDWARE: written per spec, never executed on a Pi 5.
pub fn call_property_channel(
    base: u64,
    envelope: &mut [u32],
    cycle_frequency_hz: u64,
    now: impl Fn() -> u64,
) -> Result<(), MailboxError> {
    if envelope.len() < 3 || envelope.len() > MAX_PROPERTY_WORDS {
        return Err(MailboxError::InvalidRequest);
    }
    let envelope_pointer = envelope.as_ptr() as u64;
    if envelope_pointer % BUFFER_MIN_ALIGN_BYTES as u64 != 0 {
        return Err(MailboxError::MisalignedBuffer);
    }
    if envelope_pointer >= 1 << 32 {
        return Err(MailboxError::AddressAbove4GiB);
    }

    // Descriptor: 28-bit buffer bus address plus 4-bit channel nibble.
    let descriptor =
        ((envelope_pointer as u32) | CHANNEL_PROPERTY_TAGS_ARM_TO_VC) & !VC_BUS_ALIAS_MASK as u32;

    // Deadline spans roughly a quarter second regardless of counter rate.
    let deadline = now().saturating_add(cycle_frequency_hz.max(1) / 4);

    if !poll_status_clear(base, STATUS_FULL, deadline, &now) {
        return Err(MailboxError::Timeout);
    }
    write_reg(base, regs::WRITE, descriptor);

    // Drain completions tagged with our channel; foreign traffic is consumed.
    loop {
        if !poll_status_set(base, STATUS_EMPTY, deadline, &now) {
            return Err(MailboxError::Timeout);
        }
        let answer = read_reg(base, regs::READ);
        if answer & 0xF == CHANNEL_PROPERTY_TAGS_ARM_TO_VC {
            return Ok(());
        }
        if now() >= deadline {
            return Err(MailboxError::Timeout);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    /// Largest-tagged golden request: full negotiation prelude used by the
    /// framebuffer flow — exact wire words pinned so firmware-visible layout
    /// cannot drift silently.
    #[test]
    fn allocate_buffer_request_matches_golden_layout() {
        let mut arena = [0u32; MAX_PROPERTY_WORDS];
        let mut writer = PropertyRequestWriter::new(&mut arena).unwrap();

        writer
            .push_tag(TAG_SET_PHYSICAL_WIDTH_HEIGHT, &[1920, 1080], 8)
            .unwrap();
        writer
            .push_tag(TAG_SET_VIRTUAL_WIDTH_HEIGHT, &[1920, 1080], 8)
            .unwrap();
        writer.push_tag(TAG_SET_DEPTH, &[32], 4).unwrap();
        writer
            .push_tag(TAG_ALLOCATE_BUFFER, &[BUFFER_MIN_ALIGN_BYTES as u32], 8)
            .unwrap();

        let finished = writer.finish();
        // hdr(2) + tags(3+h each; DEPTH/ALLOCATE pad their reserve slots).
        assert_eq!(finished.len(), 2 + 5 + 5 + 4 + 5 + 1);
        assert_eq!(
            finished,
            &[
                (2 + 5 + 5 + 4 + 5 + 1) * 4, // envelope bytes
                REQUEST_CODE,                // 0
                TAG_SET_PHYSICAL_WIDTH_HEIGHT,
                8,
                0,
                1920,
                1080,
                TAG_SET_VIRTUAL_WIDTH_HEIGHT,
                8,
                0,
                1920,
                1080,
                TAG_SET_DEPTH,
                4,
                0,
                32,
                TAG_ALLOCATE_BUFFER,
                8,
                0,
                BUFFER_MIN_ALIGN_BYTES as u32,
                0,
                TAG_END, //
            ][..]
        );
    }

    #[test]
    fn minimal_firmware_revision_envelope_spans_twenty_eight_bytes() {
        // GET_FIRMWARE_REVISION reserves 4 response bytes but requests none:
        // the reserved slot still materializes (zero word), landing the full
        // probe on the canonical 28-byte envelope size.
        let mut arena = [0u32; MAX_PROPERTY_WORDS];
        let mut writer = PropertyRequestWriter::new(&mut arena).unwrap();
        writer.push_tag(TAG_GET_FIRMWARE_REVISION, &[], 4).unwrap();
        let finished = writer.finish();
        assert_eq!(finished.len(), 7); // hdr(2) + tag(3) + padded slot(1) + END(1)
        assert_eq!(finished, &[28, 0, TAG_GET_FIRMWARE_REVISION, 4, 0, 0, 0]);
    }

    #[test]
    fn builder_rejects_zero_reservation_overflow_and_undersized_reservations() {
        let mut tiny = [0u32; 2];
        let probe = PropertyRequestWriter::new(&mut tiny);
        assert!(matches!(probe, Err(MailboxError::InvalidRequest)));

        let mut arena = [0u32; MAX_PROPERTY_WORDS];
        let mut writer = PropertyRequestWriter::new(&mut arena).unwrap();
        // Zero-byte reservation is meaningless on the wire.
        assert_eq!(
            writer.push_tag(TAG_SET_DEPTH, &[32], 0),
            Err(MailboxError::InvalidRequest)
        );
        // Values beyond the requested slot are rejected outright.
        assert_eq!(
            writer.push_tag(TAG_SET_DEPTH, &[32, 99], 4),
            Err(MailboxError::InvalidRequest)
        );
    }

    fn sample_request() -> Vec<u32> {
        let mut arena = [0u32; MAX_PROPERTY_WORDS];
        let mut writer = PropertyRequestWriter::new(&mut arena).unwrap();
        writer
            .push_tag(TAG_SET_PHYSICAL_WIDTH_HEIGHT, &[1280, 720], 8)
            .unwrap();
        writer
            .push_tag(TAG_ALLOCATE_BUFFER, &[BUFFER_MIN_ALIGN_BYTES as u32], 8)
            .unwrap();
        writer.finish().to_vec()
    }

    fn complete_response(request: &[u32], bus_addr: u64, fb_bytes: u32) -> Vec<u32> {
        let mut response = request.to_vec();
        response[1] = RESPONSE_CODE_OK;
        // SET_PHYSICAL completes without echoing payload changes beyond bit31.
        let phys = locate_request_tag(&response, TAG_SET_PHYSICAL_WIDTH_HEIGHT).unwrap();
        response[phys + 1] = 8;
        response[phys + 2] = TAG_RESPONSE_BIT;
        // ALLOCATE_BUFFER returns bus address + byte length.
        let alloc = locate_request_tag(&response, TAG_ALLOCATE_BUFFER).unwrap();
        response[alloc + 1] = 8;
        response[alloc + 2] = TAG_RESPONSE_BIT;
        response[alloc + 3] = bus_addr as u32;
        response[alloc + 4] = fb_bytes;
        response
    }

    #[test]
    fn roundtrip_build_then_parse_canonical_success() {
        let request = sample_request();
        let response = complete_response(&request, VC_BUS_SDRAM_ALIAS | 0x00F3_0000, 0x0033_6000);

        assert_eq!(validate_exchange(&request, &response), Ok(()));

        let allocation = tag_values(&response, TAG_ALLOCATE_BUFFER).expect("allocation present");
        assert_eq!(allocation.values.len(), 2);
        let frame_base = allocation.values[0] as u64;
        let frame_bytes = allocation.values[1];

        // Documented-assumption translation closes the loop.
        let physical = bus_to_physical(frame_base).expect("alias window");
        assert_eq!(physical, 0x00F3_0000);
        assert_eq!(
            physical_to_bus(physical),
            Some(VC_BUS_SDRAM_ALIAS | 0x00F3_0000)
        );
        assert_eq!(frame_bytes, 0x0033_6000);

        let geometry =
            tag_values(&response, TAG_SET_PHYSICAL_WIDTH_HEIGHT).expect("geometry present");
        assert_eq!(geometry.values, &[1280, 720]);
    }

    #[test]
    fn validation_rejects_error_code_missing_bit_and_oversize_and_truncation() {
        let request = sample_request();

        let mut rejected = request.clone();
        rejected[1] = RESPONSE_CODE_ERROR;
        assert_eq!(
            validate_exchange(&request, &rejected),
            Err(MailboxError::DeviceRejected(RESPONSE_CODE_ERROR))
        );

        // Missing completion bit on the allocate tag.
        let mut missing_bit = complete_response(&request, 0x4000_0000, 16);
        let alloc = locate_request_tag(&missing_bit, TAG_ALLOCATE_BUFFER).unwrap();
        missing_bit[alloc + 2] &= !TAG_RESPONSE_BIT;
        assert_eq!(
            validate_exchange(&request, &missing_bit),
            Err(MailboxError::TagRejected(TAG_ALLOCATE_BUFFER, 0))
        );

        // Response claiming more bytes than the request reserved.
        let mut oversized = complete_response(&request, 0x4000_0000, 16);
        let alloc = locate_request_tag(&oversized, TAG_ALLOCATE_BUFFER).unwrap();
        oversized[alloc + 1] = 64;
        assert_eq!(
            validate_exchange(&request, &oversized),
            Err(MailboxError::ResponseLargerThanRequest(TAG_ALLOCATE_BUFFER))
        );

        // Truncated envelope fails coherence.
        let truncated = request.clone().into_iter().take(5).collect::<Vec<_>>();
        assert_eq!(
            validate_exchange(&request, &truncated),
            Err(MailboxError::MalformedResponse)
        );
    }

    #[test]
    fn unknown_response_tags_fail_validation() {
        let request = sample_request();
        let response = complete_response(&request, 0x4000_0000, 16);
        // Response carries a tag the request never made: blank the request's
        // allocate-tag id out, leaving the id unmatched during validation.
        let alloc = locate_request_tag(&request, TAG_ALLOCATE_BUFFER).unwrap();
        let mut hole = request.clone();
        hole[alloc] = 0xFFFF_FFFF;
        assert_eq!(
            validate_exchange(&hole, &response),
            Err(MailboxError::MalformedResponse)
        );
    }

    #[test]
    fn bus_translation_roundtrip_and_window_rejection() {
        assert_eq!(physical_to_bus(0x00F3_0000), Some(0x40F3_0000));
        assert_eq!(
            bus_to_physical(0xC000_0000 | 0x00F3_0000),
            Some(0x00F3_0000)
        );
        // Peripheral windows do not map into SDRAM.
        assert_eq!(bus_to_physical(0x0001_3880), None);
        assert_eq!(bus_to_physical(0x8000_0000 | 0x1000), None);
        // Above the 32-bit legacy window nothing translates.
        assert_eq!(physical_to_bus(0x1_0000_0000), None);
    }
}

//! Pure MSI / MSI-X helpers: PCI capability parsing, MSI-X table entry
//! encoding, and x86 MSI message address/data word layout.
//!
//! Hardware-neutral by design: parsing takes a dword-fetch closure over PCI
//! configuration space (port I/O, ECAM, or synthetic test backends), and the
//! message encoders return raw words the caller delivers through the
//! platform's own write path. Unit tests cover the layout against golden
//! values from the PCI Local Bus spec (MSI-X capability, message table) and
//! the Intel SDM (MSI address/data format).

/// PCI capability ID for MSI-X.
pub const MSI_X_CAP_ID: u8 = 0x11;

/// Message Control: MSI-X Enable bit.
pub const MSI_X_MSG_CTRL_ENABLE: u16 = 1 << 15;
/// Message Control: Function Mask bit (masks all vectors).
pub const MSI_X_MSG_CTRL_FUNCTION_MASK: u16 = 1 << 14;
/// Message Control: table size field mask (bits 10:0, N-1 encoded).
pub const MSI_X_MSG_CTRL_TABLE_SIZE_MASK: u16 = 0x7ff;

/// Size in bytes of one MSI-X table entry.
pub const MSIX_TABLE_ENTRY_SIZE: u32 = 16;
/// Size in bytes of one MSI message table entry field (32-bit words).
pub const MSIX_TABLE_ENTRY_WORDS: u32 = 4;

/// Byte offset of the message address field within one table entry.
pub const MSIX_ENTRY_ADDRESS_LOWER_OFFSET: u32 = 0;
/// Byte offset of the message upper address field within one table entry.
pub const MSIX_ENTRY_ADDRESS_UPPER_OFFSET: u32 = 4;
/// Byte offset of the message data field within one table entry.
pub const MSIX_ENTRY_DATA_OFFSET: u32 = 8;
/// Byte offset of the vector-control field within one table entry.
pub const MSIX_ENTRY_VECTOR_CONTROL_OFFSET: u32 = 12;

/// Fixed base of the x86 MSI message address (Local APIC MMIO base bits).
pub const MSI_MESSAGE_ADDRESS_BASE: u32 = 0xfee0_0000;

/// Bit within the MSI address word: redirect-hint enable.
pub const MSI_ADDRESS_REDIRECT_HINT: u32 = 1 << 3;
/// Bit within the MSI address word: destination-mode logical.
pub const MSI_ADDRESS_DESTINATION_LOGICAL: u32 = 1 << 2;
/// Bit mask of the destination APIC id field (bits 19:12).
pub const MSI_ADDRESS_DESTINATION_MASK: u32 = 0xff << 12;

/// Bit within the MSI data word: level trigger (1) vs edge (0).
pub const MSI_DATA_LEVEL_TRIGGER: u32 = 1 << 15;
/// Bit within the MSI data word: level-assert (1) vs de-assert (0).
pub const MSI_DATA_LEVEL_ASSERT: u32 = 1 << 14;
/// Bit mask of the delivery vector field (bits 7:0).
pub const MSI_DATA_VECTOR_MASK: u32 = 0xff;

/// Parsed MSI-X capability structure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MsixCapability {
    /// Table size minus one, as encoded in Message Control bits 10:0.
    pub table_size_encoded: u16,
    /// MSI-X Enable bit state at parse time.
    pub enabled: bool,
    /// Function Mask bit state at parse time.
    pub function_masked: bool,
    /// BAR index (BIR) holding the MSI-X table.
    pub table_bir: u8,
    /// Byte offset of the MSI-X table within that BAR.
    pub table_offset: u32,
    /// BAR index (BIR) holding the pending-bit array.
    pub pba_bir: u8,
    /// Byte offset of the pending-bit array within that BAR.
    pub pba_offset: u32,
}

impl MsixCapability {
    /// Number of table vectors (encoded size + 1).
    pub fn table_size(&self) -> u16 {
        self.table_size_encoded + 1
    }

    /// Re-encode a Message Control word carrying this capability's table
    /// size with the given enable / function-mask bits.
    pub fn message_control(&self, enabled: bool, function_masked: bool) -> u16 {
        let mut control = self.table_size_encoded & MSI_X_MSG_CTRL_TABLE_SIZE_MASK;
        if function_masked {
            control |= MSI_X_MSG_CTRL_FUNCTION_MASK;
        }
        if enabled {
            control |= MSI_X_MSG_CTRL_ENABLE;
        }
        control
    }
}

/// Parse the MSI-X capability structure starting at `cap_offset` (which must
/// be dword-aligned; MSI-X capabilities in config space always are).
///
/// `read_word(offset)` fetches one 32-bit config-space word. Returns `None`
/// when the offset is misaligned.
pub fn parse_msix_capability(
    read_word: impl Fn(u8) -> u32,
    cap_offset: u8,
) -> Option<MsixCapability> {
    if cap_offset & 0x3 != 0 {
        return None;
    }

    let header = read_word(cap_offset);
    let control = (header >> 16) as u16;

    let table = read_word(cap_offset + 4);
    let pba = read_word(cap_offset + 8);

    Some(MsixCapability {
        table_size_encoded: control & MSI_X_MSG_CTRL_TABLE_SIZE_MASK,
        enabled: control & MSI_X_MSG_CTRL_ENABLE != 0,
        function_masked: control & MSI_X_MSG_CTRL_FUNCTION_MASK != 0,
        table_bir: (table & 0x7) as u8,
        table_offset: table & !0x7,
        pba_bir: (pba & 0x7) as u8,
        pba_offset: pba & !0x7,
    })
}

/// Encode the low dword of an MSI message address targeting
/// `destination_apic_id` in physical destination mode.
///
/// Redirect hint and logical destination mode are opt-in so v0 callers get
/// the simple physical, no-redirect form.
pub fn msi_message_address_lower(
    destination_apic_id: u8,
    redirect_hint: bool,
    logical_destination: bool,
) -> u32 {
    let mut address = MSI_MESSAGE_ADDRESS_BASE;
    address |= u32::from(destination_apic_id) << 12;
    if redirect_hint {
        address |= MSI_ADDRESS_REDIRECT_HINT;
    }
    if logical_destination {
        address |= MSI_ADDRESS_DESTINATION_LOGICAL;
    }
    address
}

/// Encode the high dword of an MSI message address. x86 APIC addressing
/// keeps all destinations below 4 GiB, so this is always zero.
pub const fn msi_message_address_upper() -> u32 {
    0
}

/// Encode an MSI message data word delivering `vector` as a fixed,
/// edge-triggered interrupt (the only form the LAPIC auto-EOIs cleanly for
/// this driver shape).
pub fn msi_message_data_edge_fixed(vector: u8) -> u32 {
    u32::from(vector) & MSI_DATA_VECTOR_MASK
}

/// Byte offset of MSI-X table entry `index` within the mapped table.
pub fn msix_table_entry_offset(index: u16) -> u32 {
    u32::from(index) * MSIX_TABLE_ENTRY_SIZE
}

/// One encoded MSI-X table entry (message address, data, vector control).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MsixTableEntry {
    pub address_lower: u32,
    pub address_upper: u32,
    pub data: u32,
    /// Bit 0 set = vector masked.
    pub vector_control: u32,
}

impl MsixTableEntry {
    /// Build an entry delivering `vector` to `destination_apic_id`, masked
    /// or unmasked. Fixed delivery mode, edge trigger.
    pub fn new_edge_fixed(destination_apic_id: u8, vector: u8, masked: bool) -> Self {
        Self {
            address_lower: msi_message_address_lower(destination_apic_id, false, false),
            address_upper: msi_message_address_upper(),
            data: msi_message_data_edge_fixed(vector),
            vector_control: u32::from(masked),
        }
    }

    /// Decode an entry from four words read in table order.
    pub fn from_words(words: [u32; 4]) -> Self {
        Self {
            address_lower: words[0],
            address_upper: words[1],
            data: words[2],
            vector_control: words[3],
        }
    }

    /// Encode into four words in table order.
    pub fn to_words(&self) -> [u32; 4] {
        [
            self.address_lower,
            self.address_upper,
            self.data,
            self.vector_control,
        ]
    }

    pub fn masked(&self) -> bool {
        self.vector_control & 0x1 != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic config-space word fetcher from raw dwords.
    fn reader(words: &[(u8, u32)]) -> impl Fn(u8) -> u32 + '_ {
        move |offset: u8| -> u32 {
            for (word_offset, value) in words {
                if *word_offset == offset {
                    return *value;
                }
            }
            0xffff_ffff
        }
    }

    fn golden_capability_words() -> [(u8, u32); 3] {
        // Typical QEMU virtio-net-pci modern shape: MSI-X cap with 4 vectors
        // (size encoded 3), enabled clear, function mask clear, table in
        // BAR 1 at 0x2000, PBA in BAR 1 at 0x3000.
        [
            (0xa0, 0x0003_0011),
            (0xa4, 0x0000_2001),
            (0xa8, 0x0000_3001),
        ]
    }

    #[test]
    fn parse_msix_capability_golden() {
        let parse =
            parse_msix_capability(reader(&golden_capability_words()), 0xa0).expect("cap parse");

        assert_eq!(parse.table_size_encoded, 3);
        assert_eq!(parse.table_size(), 4);
        assert!(!parse.enabled);
        assert!(!parse.function_masked);
        assert_eq!(parse.table_bir, 1);
        assert_eq!(parse.table_offset, 0x2000);
        assert_eq!(parse.pba_bir, 1);
        assert_eq!(parse.pba_offset, 0x3000);
    }

    #[test]
    fn parse_msix_capability_reads_enable_and_mask_bits() {
        let words = [
            (0x50, 0xc004_0011), // enable + function mask, size encoded 4
            (0x54, 0x0000_0003), // BAR 3, offset 0
            (0x58, 0x0000_0007), // BAR 7 (invalid), offset 0
        ];
        let parse = parse_msix_capability(reader(&words), 0x50).expect("cap parse");
        assert!(parse.enabled);
        assert!(parse.function_masked);
        assert_eq!(parse.table_size(), 5);
        assert_eq!(parse.table_bir, 3);
        assert_eq!(parse.table_offset, 0);
        assert_eq!(parse.pba_bir, 7);
    }

    #[test]
    fn parse_msix_capability_rejects_misaligned_offset() {
        let words = [(0xa2, 0x0003_0011)];
        assert!(parse_msix_capability(reader(&words), 0xa2).is_none());
    }

    #[test]
    fn message_control_reencode_preserves_size_and_sets_bits() {
        let parse = MsixCapability {
            table_size_encoded: 2,
            enabled: false,
            function_masked: false,
            table_bir: 1,
            table_offset: 0x2000,
            pba_bir: 1,
            pba_offset: 0x3000,
        };
        assert_eq!(parse.message_control(false, false), 0x0002);
        assert_eq!(
            parse.message_control(true, false),
            MSI_X_MSG_CTRL_ENABLE | 2
        );
        assert_eq!(
            parse.message_control(true, true),
            MSI_X_MSG_CTRL_ENABLE | MSI_X_MSG_CTRL_FUNCTION_MASK | 2
        );
    }

    #[test]
    fn msi_address_golden_layout() {
        // SDM 10.11.1: bits 31:20 = 0xFEE, bits 19:12 = destination APIC id,
        // bit 3 = redirect hint, bit 2 = destination mode.
        assert_eq!(msi_message_address_lower(0, false, false), 0xfee0_0000);
        assert_eq!(msi_message_address_lower(5, false, false), 0xfee0_5000);
        assert_eq!(
            msi_message_address_lower(0xff, true, true),
            0xfeef_f00c,
            "0xff destination in bits 19:12 + redirect hint (bit 3) + logical mode (bit 2)"
        );
        assert_eq!(
            msi_message_address_lower(0, true, false) & MSI_ADDRESS_REDIRECT_HINT,
            MSI_ADDRESS_REDIRECT_HINT
        );
        assert_eq!(msi_message_address_upper(), 0);
    }

    #[test]
    fn msi_data_golden_layout() {
        // SDM 10.11.2: bits 7:0 = vector, bit 15 = trigger level, bit 14 =
        // level assert. Edge-fixed = vector only.
        assert_eq!(msi_message_data_edge_fixed(0x50), 0x50);
        assert_eq!(msi_message_data_edge_fixed(0xff), 0xff);
    }

    #[test]
    fn msix_table_entry_roundtrip() {
        let entry = MsixTableEntry::new_edge_fixed(0, 0x50, true);
        let words = entry.to_words();
        assert_eq!(words[0], 0xfee0_0000);
        assert_eq!(words[1], 0);
        assert_eq!(words[2], 0x50);
        assert_eq!(words[3], 1);
        assert!(entry.masked());

        let decoded = MsixTableEntry::from_words(words);
        assert_eq!(decoded, entry);
        assert!(decoded.masked());

        let unmasked = MsixTableEntry::new_edge_fixed(2, 0x51, false);
        assert_eq!(unmasked.address_lower, 0xfee0_2000);
        assert_eq!(unmasked.vector_control, 0);
        assert!(!unmasked.masked());
    }

    #[test]
    fn msix_table_entry_offsets_match_stride() {
        assert_eq!(msix_table_entry_offset(0), 0);
        assert_eq!(msix_table_entry_offset(1), 16);
        assert_eq!(msix_table_entry_offset(5), 80);
        assert_eq!(MSIX_TABLE_ENTRY_WORDS, 4);
        assert_eq!(
            MSIX_ENTRY_VECTOR_CONTROL_OFFSET,
            3 * core::mem::size_of::<u32>() as u32
        );
    }
}

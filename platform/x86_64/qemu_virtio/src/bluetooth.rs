//! Platform-neutral Bluetooth BR/EDR control-plane layer (HCI-shaped).
//!
//! Pure logic only: no USB/UART transport, no probing, no IRQ paths. A
//! future Bluetooth controller backend would stage these byte shapes through
//! its own HCI transport and implement the kernel's `BluetoothBackend`
//! contract. Dependency-free so the module can be included by path in the
//! host test harness, exactly like the raspi5 mailbox driver.
//!
//! Wire surface (bounded slice of the HCI spec):
//! - commands: `[opcode u16 LE][param_total_length u8][params...]` for
//!   Reset, Write_Scan_Enable, Inquiry and Create_Connection (BR/EDR ACL),
//!   with a Class-of-Device gate restricting connectable targets to the
//!   keyboard and audio device classes;
//! - events: `[event_code u8][param_total_length u8][params...]` for
//!   Inquiry_Result, Connection_Complete, Disconnection_Complete,
//!   Command_Complete and Command_Status;
//! - a pairing state machine stub that models only the wire shapes and
//!   honestly reports `Unimplemented` for SSP phases beyond them.
//!
//! UNTESTED WITHOUT HARDWARE: nothing in this module has executed against a
//! real Bluetooth controller. What would validate it: an HCI traffic capture
//! (Reset/Inquiry/Create_Connection plus the corresponding events) replayed
//! through the builder/parser, and a real pairing exchange against a peer
//! device. Pairing beyond the wire shapes is explicitly not implemented.
#![allow(dead_code)]

/// Maximum encoded HCI command packet size (bytes).
pub const MAX_COMMAND_BYTES: usize = 64;
/// Maximum encoded HCI event packet size (bytes).
pub const MAX_EVENT_BYTES: usize = 255;

// Command opcodes: (OGF << 10) | OCF.
/// HCI_Reset (OGF 0x03, OCF 0x0003).
pub const CMD_RESET: u16 = 0x0C03;
/// Write_Scan_Enable (OGF 0x03, OCF 0x001A).
pub const CMD_WRITE_SCAN_ENABLE: u16 = 0x0C1A;
/// Inquiry (OGF 0x04, OCF 0x0001).
pub const CMD_INQUIRY: u16 = 0x0401;
/// Create_Connection (OGF 0x01, OCF 0x0005) — BR/EDR ACL.
pub const CMD_CREATE_CONNECTION: u16 = 0x0405;

/// Scan-enable flags for [`write_scan_enable`].
pub const SCAN_ENABLE_NONE: u8 = 0x00;
pub const SCAN_ENABLE_INQUIRY: u8 = 0x01;
pub const SCAN_ENABLE_PAGE: u8 = 0x02;

/// Event codes.
pub const EVENT_INQUIRY_RESULT: u8 = 0x02;
pub const EVENT_CONNECTION_COMPLETE: u8 = 0x03;
pub const EVENT_DISCONNECTION_COMPLETE: u8 = 0x05;
pub const EVENT_COMMAND_COMPLETE: u8 = 0x0E;
pub const EVENT_COMMAND_STATUS: u8 = 0x0F;

/// Link types carried by Connection_Complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkType {
    Sco,
    Acl,
}

/// HCI command/event failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HciError {
    /// Packet shorter than its header mandates.
    TooShort,
    /// Declared parameter length disagrees with the bytes present.
    BadLength,
    /// Unknown event code.
    BadEvent,
    /// Invalid command parameters (lengths, modes).
    BadCommandParams,
    /// Target device class outside the bounded connectable set.
    InvalidClass,
}

// ---------------------------------------------------------------------------
// Command builder
// ---------------------------------------------------------------------------

/// Bounded connectable device classes (mission scope: keyboard, audio).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceClass {
    Keyboard,
    Audio,
    /// Any other CoD — connect attempts are rejected by the builders.
    Other,
}

impl DeviceClass {
    /// Classifies a 3-byte Class of Device field. Major device class lives
    /// in bits 8-12 (byte 1 low nibble): 0x05 = peripheral (keyboard when
    /// minor class bits are 0x01), 0x04 = audio/video.
    pub fn from_cod(cod: [u8; 3]) -> DeviceClass {
        match cod[1] & 0x1F {
            0x05 => {
                if (cod[0] >> 2) & 0x07 == 0x01 {
                    DeviceClass::Keyboard
                } else {
                    DeviceClass::Other
                }
            }
            0x04 => DeviceClass::Audio,
            _ => DeviceClass::Other,
        }
    }

    /// The raw 3-byte CoD for the class (canonical examples: keyboard
    /// 00:05:40 → 0x0540... encoded as minor/major/service bytes).
    pub fn cod(self) -> [u8; 3] {
        match self {
            // Peripheral major (0x05), keyboard minor (0x01): byte0 =
            // minor<<2 = 0x04, byte1 = major = 0x05.
            DeviceClass::Keyboard => [0x04, 0x05, 0x00],
            // Audio/video major (0x04), uncategorized minor.
            DeviceClass::Audio => [0x00, 0x04, 0x00],
            DeviceClass::Other => [0x00, 0x00, 0x00],
        }
    }
}

/// Parameters for a BR/EDR ACL Create_Connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CreateConnectionParams {
    pub bd_addr: [u8; 6],
    /// Class of Device of the target (from Inquiry_Result).
    pub class_of_device: [u8; 3],
    /// Packet type bitmask (HCI spec; 0x0008 = DM1 as a safe default).
    pub packet_type: u16,
    /// Page scan repetition mode (0-2 per spec).
    pub page_scan_repetition_mode: u8,
    /// Valid clock offset from inquiry; `None` = invalid (0x8000 on wire).
    pub clock_offset: Option<u16>,
    /// Allow role switch on connection establishment.
    pub allow_role_switch: bool,
}

/// Incremental builder over a fixed buffer.
pub struct CommandBuilder {
    buffer: [u8; MAX_COMMAND_BYTES],
    used: usize,
}

impl CommandBuilder {
    fn empty() -> CommandBuilder {
        CommandBuilder {
            buffer: [0; MAX_COMMAND_BYTES],
            used: 0,
        }
    }

    /// The encoded command packet.
    pub fn finish(&self) -> &[u8] {
        &self.buffer[..self.used]
    }

    /// Encoded length so far.
    pub fn len(&self) -> usize {
        self.used
    }
}

/// Builds an HCI_Reset command: `03 0C 00`.
pub fn reset() -> Result<CommandBuilder, HciError> {
    encode_command(CMD_RESET, &[])
}

/// Encodes Write_Scan_Enable with the given scan-enable mask.
pub fn write_scan_enable(mask: u8) -> Result<CommandBuilder, HciError> {
    if mask & !0x03 != 0 {
        return Err(HciError::BadCommandParams);
    }
    encode_command(CMD_WRITE_SCAN_ENABLE, &[mask])
}

/// Encodes Inquiry: `01 04 05 <LAP[3]> <len> <num>`.
pub fn inquiry(
    lap: [u8; 3],
    inquiry_length: u8,
    num_responses: u8,
) -> Result<CommandBuilder, HciError> {
    if inquiry_length == 0 {
        return Err(HciError::BadCommandParams);
    }
    let mut params = [0u8; 5];
    params[..3].copy_from_slice(&lap);
    params[3] = inquiry_length;
    params[4] = num_responses;
    encode_command(CMD_INQUIRY, &params)
}

/// Encodes Create_Connection for a bounded device class (keyboard/audio
/// only; anything else is [`HciError::InvalidClass`]).
///
/// Wire shape: `05 04 0D <BD_ADDR 6> <packet_type 2 LE>
/// <page_scan_rep 1> <reserved 1> <clock_offset 2 LE> <allow_role_switch 1>`.
pub fn create_connection(params: CreateConnectionParams) -> Result<CommandBuilder, HciError> {
    if DeviceClass::from_cod(params.class_of_device) == DeviceClass::Other {
        return Err(HciError::InvalidClass);
    }
    if params.page_scan_repetition_mode > 2 {
        return Err(HciError::BadCommandParams);
    }
    let mut raw = [0u8; 13];
    raw[..6].copy_from_slice(&params.bd_addr);
    raw[6..8].copy_from_slice(&params.packet_type.to_le_bytes());
    raw[8] = params.page_scan_repetition_mode;
    raw[9] = 0; // reserved
    let clock_offset = params.clock_offset.unwrap_or(0x8000);
    raw[10..12].copy_from_slice(&clock_offset.to_le_bytes());
    raw[12] = params.allow_role_switch as u8;
    encode_command(CMD_CREATE_CONNECTION, &raw)
}

fn encode_command(opcode: u16, params: &[u8]) -> Result<CommandBuilder, HciError> {
    if params.len() > u8::MAX as usize {
        return Err(HciError::BadCommandParams);
    }
    let mut builder = CommandBuilder::empty();
    builder.buffer[..2].copy_from_slice(&opcode.to_le_bytes());
    builder.buffer[2] = params.len() as u8;
    builder.buffer[3..3 + params.len()].copy_from_slice(params);
    builder.used = 3 + params.len();
    Ok(builder)
}

// ---------------------------------------------------------------------------
// Event decode
// ---------------------------------------------------------------------------

/// Decoded HCI events (bounded set).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    /// Legacy Inquiry_Result (one response per event).
    InquiryResult {
        bd_addr: [u8; 6],
        /// Page scan repetition mode (0-2).
        page_scan_repetition_mode: u8,
        class_of_device: [u8; 3],
        /// Clock offset (bit 15 = validity on the wire).
        clock_offset: u16,
    },
    ConnectionComplete {
        /// HCI status (0 = success).
        status: u8,
        /// Connection handle.
        handle: u16,
        bd_addr: [u8; 6],
        link_type: LinkType,
        /// Encryption enable as reported.
        encryption: u8,
    },
    DisconnectionComplete {
        status: u8,
        handle: u16,
        /// HCI reason code for the disconnect.
        reason: u8,
    },
    CommandComplete {
        /// Number of command packets the controller can accept now.
        num_packets: u8,
        opcode: u16,
        status: u8,
    },
    CommandStatus {
        status: u8,
        num_packets: u8,
        opcode: u16,
    },
}

/// Decodes one event packet: `[event_code u8][param_total_length u8]
/// [params...]`. Length and code are validated against the bounded set.
pub fn decode_event(packet: &[u8]) -> Result<Event, HciError> {
    if packet.len() < 2 {
        return Err(HciError::TooShort);
    }
    let code = packet[0];
    let param_len = packet[1] as usize;
    if packet.len() - 2 != param_len {
        return Err(HciError::BadLength);
    }
    let params = &packet[2..];
    match code {
        EVENT_INQUIRY_RESULT => {
            // Legacy format carries exactly one response per event.
            if params.len() != 14 {
                return Err(HciError::BadLength);
            }
            if params[0] != 1 {
                return Err(HciError::BadEvent);
            }
            let mut bd_addr = [0u8; 6];
            bd_addr.copy_from_slice(&params[1..7]);
            let mut class_of_device = [0u8; 3];
            class_of_device.copy_from_slice(&params[9..12]);
            let clock_offset = u16::from_le_bytes([params[12], params[13]]);
            Ok(Event::InquiryResult {
                bd_addr,
                page_scan_repetition_mode: params[7],
                class_of_device,
                clock_offset,
            })
        }
        EVENT_CONNECTION_COMPLETE => {
            if params.len() != 11 {
                return Err(HciError::BadLength);
            }
            let mut bd_addr = [0u8; 6];
            bd_addr.copy_from_slice(&params[3..9]);
            let link_type = match params[9] {
                0 => LinkType::Sco,
                1 => LinkType::Acl,
                _ => return Err(HciError::BadEvent),
            };
            Ok(Event::ConnectionComplete {
                status: params[0],
                handle: u16::from_le_bytes([params[1], params[2]]),
                bd_addr,
                link_type,
                encryption: params[10],
            })
        }
        EVENT_DISCONNECTION_COMPLETE => {
            if params.len() != 4 {
                return Err(HciError::BadLength);
            }
            Ok(Event::DisconnectionComplete {
                status: params[0],
                handle: u16::from_le_bytes([params[1], params[2]]),
                reason: params[3],
            })
        }
        EVENT_COMMAND_COMPLETE => {
            if params.len() < 4 {
                return Err(HciError::BadLength);
            }
            Ok(Event::CommandComplete {
                num_packets: params[0],
                opcode: u16::from_le_bytes([params[1], params[2]]),
                status: params[3],
            })
        }
        EVENT_COMMAND_STATUS => {
            if params.len() < 4 {
                return Err(HciError::BadLength);
            }
            Ok(Event::CommandStatus {
                status: params[0],
                num_packets: params[1],
                opcode: u16::from_le_bytes([params[2], params[3]]),
            })
        }
        _ => Err(HciError::BadEvent),
    }
}

// ---------------------------------------------------------------------------
// Pairing state machine stub (honest Unimplemented for SSP)
// ---------------------------------------------------------------------------

/// Pairing phases actually modeled by the stub.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingState {
    /// No pairing activity on the link.
    NotPaired,
    /// Link key exchange in flight (the only wire shape this stub drives).
    LinkKeyExchange,
    /// Link key stored; the link is paired.
    Paired,
}

/// Pairing outcomes, including the honest refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingOutcome {
    /// Link key exchange started.
    Started,
    /// Link key accepted; pairing complete.
    KeyStored,
    /// Secure Simple Pairing (IO capability exchange, user confirmation,
    /// OOB) is beyond the wire shapes this stub models. UNIMPLEMENTED BY
    /// DESIGN — see module header.
    Unimplemented,
    /// Request does not fit the current phase.
    InvalidState,
}

/// Pairing state machine stub.
///
/// Models only the legacy link-key wire shapes. Every Secure Simple Pairing
/// phase (IO capability exchange, numeric comparison, passkey entry, OOB)
/// returns [`PairingOutcome::Unimplemented`] and leaves the state machine
/// untouched. UNTESTED WITHOUT HARDWARE: would need a real pairing capture
/// plus a peer implementing the same shapes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PairingStub {
    state: Option<PairingState>,
}

impl PairingStub {
    /// Starts at [`PairingState::NotPaired`].
    pub fn new() -> PairingStub {
        PairingStub {
            state: Some(PairingState::NotPaired),
        }
    }

    /// Current phase.
    pub fn state(&self) -> PairingState {
        self.state.unwrap_or(PairingState::NotPaired)
    }

    /// Begins the (legacy) link key exchange after an ACL connection.
    pub fn begin_link_key_exchange(&mut self) -> Result<PairingOutcome, PairingOutcome> {
        match self.state() {
            PairingState::NotPaired => {
                self.state = Some(PairingState::LinkKeyExchange);
                Ok(PairingOutcome::Started)
            }
            _ => Err(PairingOutcome::InvalidState),
        }
    }

    /// SSP phase — deliberately unimplemented beyond the wire shapes.
    pub fn on_ssp_io_capability_request(&mut self) -> Result<(), PairingOutcome> {
        Err(PairingOutcome::Unimplemented)
    }

    /// Records the link key, completing pairing.
    pub fn on_link_key_stored(&mut self) -> Result<PairingOutcome, PairingOutcome> {
        match self.state() {
            PairingState::LinkKeyExchange => {
                self.state = Some(PairingState::Paired);
                Ok(PairingOutcome::KeyStored)
            }
            _ => Err(PairingOutcome::InvalidState),
        }
    }
}

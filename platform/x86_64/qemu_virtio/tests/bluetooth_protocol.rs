//! Protocol-conformance harness for the Bluetooth pure protocol layer.
//!
//! Lives outside the lib test harness because `serviceos-platform-qemu-virtio`
//! sets `[lib] test = false`. This target includes the production
//! `bluetooth.rs` by path — it is dependency-free by design — so the real
//! HCI command builder, event decoder and pairing stub run on the host under
//! std's default allocator.

#[path = "../src/bluetooth.rs"]
mod bluetooth;

use bluetooth::{
    CMD_CREATE_CONNECTION, CMD_INQUIRY, CMD_RESET, CMD_WRITE_SCAN_ENABLE, CreateConnectionParams,
    DeviceClass, EVENT_COMMAND_COMPLETE, EVENT_COMMAND_STATUS, EVENT_CONNECTION_COMPLETE,
    EVENT_DISCONNECTION_COMPLETE, EVENT_INQUIRY_RESULT, Event, HciError, LinkType, PairingOutcome,
    PairingState, PairingStub, SCAN_ENABLE_INQUIRY, SCAN_ENABLE_NONE, SCAN_ENABLE_PAGE,
    create_connection, decode_event, inquiry, reset, write_scan_enable,
};

// ---------------------------------------------------------------------------
// Command builder (golden wire bytes)
// ---------------------------------------------------------------------------

#[test]
fn reset_emits_golden_packet() {
    let builder = reset().expect("reset always valid");
    assert_eq!(builder.finish(), &[0x03, 0x0C, 0x00]);
    assert_eq!(builder.len(), 3);
}

#[test]
fn write_scan_enable_emits_golden_packet_and_rejects_bad_masks() {
    let builder = write_scan_enable(SCAN_ENABLE_INQUIRY | SCAN_ENABLE_PAGE).expect("valid mask");
    assert_eq!(builder.finish(), &[0x1A, 0x0C, 0x01, 0x03]);
    assert_eq!(
        write_scan_enable(SCAN_ENABLE_NONE)
            .expect("zero valid")
            .len(),
        4
    );
    assert!(matches!(
        write_scan_enable(0x04),
        Err(HciError::BadCommandParams)
    ));
    assert!(matches!(
        write_scan_enable(0xFF),
        Err(HciError::BadCommandParams)
    ));
}

#[test]
fn inquiry_emits_golden_packet_and_rejects_bad_params() {
    // GIAC LAP 0x9E8B33 encoded low-octet-first.
    let builder = inquiry([0x33, 0x8B, 0x9E], 8, 0).expect("valid inquiry");
    assert_eq!(
        builder.finish(),
        &[0x01, 0x04, 0x05, 0x33, 0x8B, 0x9E, 0x08, 0x00]
    );
    assert!(matches!(
        inquiry([0x33, 0x8B, 0x9E], 0, 0),
        Err(HciError::BadCommandParams)
    ));
}

#[test]
fn create_connection_emits_golden_packet_for_bounded_classes() {
    let keyboard = CreateConnectionParams {
        bd_addr: [0x11, 0x22, 0x33, 0x44, 0x55, 0x66],
        class_of_device: DeviceClass::Keyboard.cod(),
        packet_type: 0x0008,
        page_scan_repetition_mode: 2,
        clock_offset: Some(0x1234),
        allow_role_switch: true,
    };
    let builder = create_connection(keyboard).expect("keyboard connectable");
    assert_eq!(builder.len(), 3 + 13);
    assert_eq!(
        builder.finish(),
        &[
            0x05, 0x04, 0x0D, // opcode 0x0405, 13 params
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, // BD_ADDR
            0x08, 0x00, // packet_type
            0x02, // page scan repetition mode
            0x00, // reserved
            0x34, 0x12, // clock offset
            0x01, // allow role switch
        ]
    );

    // Audio class connects too; invalid clock offset becomes 0x8000.
    let audio = CreateConnectionParams {
        bd_addr: [0xAA; 6],
        class_of_device: DeviceClass::Audio.cod(),
        packet_type: 0x0008,
        page_scan_repetition_mode: 1,
        clock_offset: None,
        allow_role_switch: false,
    };
    let builder = create_connection(audio).expect("audio connectable");
    let encoded = builder.finish();
    assert_eq!(&encoded[3..9], &[0xAA; 6]);
    assert_eq!(&encoded[13..15], &[0x00, 0x80]);
    assert_eq!(encoded[15], 0);
}

#[test]
fn create_connection_rejects_out_of_scope_classes_and_modes() {
    let other = CreateConnectionParams {
        bd_addr: [0; 6],
        class_of_device: [0x00, 0x01, 0x00], // major computer
        packet_type: 0x0008,
        page_scan_repetition_mode: 1,
        clock_offset: None,
        allow_role_switch: false,
    };
    assert!(matches!(
        create_connection(other),
        Err(HciError::InvalidClass)
    ));
    let mut bad_mode = CreateConnectionParams {
        class_of_device: DeviceClass::Keyboard.cod(),
        page_scan_repetition_mode: 3,
        ..other
    };
    bad_mode.bd_addr = [1; 6];
    assert!(matches!(
        create_connection(bad_mode),
        Err(HciError::BadCommandParams)
    ));
}

#[test]
fn device_class_classifier_covers_the_bounded_set() {
    assert_eq!(
        DeviceClass::from_cod([0x04, 0x05, 0x00]),
        DeviceClass::Keyboard
    );
    assert_eq!(
        DeviceClass::from_cod([0x00, 0x04, 0x00]),
        DeviceClass::Audio
    );
    // Peripheral major but non-keyboard minor stays Other.
    assert_eq!(
        DeviceClass::from_cod([0x08, 0x05, 0x00]),
        DeviceClass::Other
    );
    assert_eq!(
        DeviceClass::from_cod([0x00, 0x01, 0x00]),
        DeviceClass::Other
    );
    // Service-class bits in byte 2 must not leak into the major class.
    assert_eq!(
        DeviceClass::from_cod([0x04, 0x25, 0xFF]),
        DeviceClass::Keyboard
    );
}

// ---------------------------------------------------------------------------
// Event decode
// ---------------------------------------------------------------------------

#[test]
fn inquiry_result_decodes_golden_packet() {
    let packet: &[u8] = &[
        EVENT_INQUIRY_RESULT,
        14,   // param length
        0x01, // num responses
        0x11,
        0x22,
        0x33,
        0x44,
        0x55,
        0x66, // BD_ADDR
        0x02, // page scan repetition mode
        0x00, // page scan period mode (unused)
        0x04,
        0x05,
        0x00, // class of device
        0x34,
        0x12, // clock offset
    ];
    assert_eq!(
        decode_event(packet),
        Ok(Event::InquiryResult {
            bd_addr: [0x11, 0x22, 0x33, 0x44, 0x55, 0x66],
            page_scan_repetition_mode: 0x02,
            class_of_device: [0x04, 0x05, 0x00],
            clock_offset: 0x1234,
        })
    );
}

#[test]
fn connection_complete_decodes_acl_and_sco() {
    let acl: &[u8] = &[
        EVENT_CONNECTION_COMPLETE,
        11,
        0x00, // status success
        0x01,
        0x00, // handle 1
        0x11,
        0x22,
        0x33,
        0x44,
        0x55,
        0x66,
        0x01, // ACL
        0x01, // encryption enabled
    ];
    assert_eq!(
        decode_event(acl),
        Ok(Event::ConnectionComplete {
            status: 0,
            handle: 1,
            bd_addr: [0x11, 0x22, 0x33, 0x44, 0x55, 0x66],
            link_type: LinkType::Acl,
            encryption: 1,
        })
    );
    let sco: &[u8] = &[
        EVENT_CONNECTION_COMPLETE,
        11,
        0x06,
        0x02,
        0x00,
        0x99,
        0x99,
        0x99,
        0x99,
        0x99,
        0x99,
        0x00,
        0x00,
    ];
    assert_eq!(
        decode_event(sco),
        Ok(Event::ConnectionComplete {
            status: 6,
            handle: 2,
            bd_addr: [0x99; 6],
            link_type: LinkType::Sco,
            encryption: 0,
        })
    );
    // Unknown link type must be rejected, not guessed.
    let bad: &[u8] = &[
        EVENT_CONNECTION_COMPLETE,
        11,
        0x00,
        0x01,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x02,
        0x00,
    ];
    assert_eq!(decode_event(bad), Err(HciError::BadEvent));
}

#[test]
fn disconnection_complete_decodes_golden_packet() {
    let packet: &[u8] = &[
        EVENT_DISCONNECTION_COMPLETE,
        4,
        0x00, // status success
        0x01,
        0x00, // handle 1
        0x16, // reason: connection terminated by local host
    ];
    assert_eq!(
        decode_event(packet),
        Ok(Event::DisconnectionComplete {
            status: 0,
            handle: 1,
            reason: 0x16,
        })
    );
}

#[test]
fn command_complete_and_status_decode() {
    let complete: &[u8] = &[
        EVENT_COMMAND_COMPLETE,
        4,
        0x01, // one command packet allowed
        0x03,
        0x0C, // opcode Reset
        0x00, // status success
    ];
    assert_eq!(
        decode_event(complete),
        Ok(Event::CommandComplete {
            num_packets: 1,
            opcode: CMD_RESET,
            status: 0,
        })
    );
    let status: &[u8] = &[
        EVENT_COMMAND_STATUS,
        4,
        0x0C, // status: connection denied (non-zero here on purpose)
        0x01, // one command packet allowed
        0x05,
        0x04, // opcode Create_Connection
    ];
    assert_eq!(
        decode_event(status),
        Ok(Event::CommandStatus {
            status: 0x0C,
            num_packets: 1,
            opcode: CMD_CREATE_CONNECTION,
        })
    );
}

#[test]
fn event_decode_rejects_structural_faults() {
    assert_eq!(decode_event(&[]), Err(HciError::TooShort));
    assert_eq!(
        decode_event(&[EVENT_INQUIRY_RESULT]),
        Err(HciError::TooShort)
    );
    // Length field disagrees with bytes present.
    assert_eq!(
        decode_event(&[EVENT_INQUIRY_RESULT, 14, 0x01]),
        Err(HciError::BadLength)
    );
    // Extra trailing byte.
    let mut packet: Vec<u8> = vec![EVENT_DISCONNECTION_COMPLETE, 4, 0x00, 0x01, 0x00, 0x16];
    packet.push(0x00);
    assert_eq!(decode_event(&packet), Err(HciError::BadLength));
    // Unknown event code.
    assert_eq!(decode_event(&[0x3F, 0x00]), Err(HciError::BadEvent));
    // Wrong response count in legacy inquiry result.
    let bad_count: &[u8] = &[
        EVENT_INQUIRY_RESULT,
        14,
        0x02,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
    ];
    assert_eq!(decode_event(bad_count), Err(HciError::BadEvent));
}

#[test]
fn builder_and_decoder_roundtrip_command_ack_pair() {
    // Reset → CommandComplete for the Reset opcode round-trips through the
    // same byte space a real transport would carry.
    let builder = reset().expect("valid");
    let command = builder.finish();
    assert_eq!(&command[..2], &CMD_RESET.to_le_bytes()[..]);
    let ack: [u8; 6] = [
        EVENT_COMMAND_COMPLETE,
        4,
        0x01,
        command[0],
        command[1],
        0x00,
    ];
    assert_eq!(
        decode_event(&ack),
        Ok(Event::CommandComplete {
            num_packets: 1,
            opcode: CMD_RESET,
            status: 0,
        })
    );
}

#[test]
fn inquiry_scan_enable_inquiry_flow_composes() {
    // Discover → classify → connect against a keyboard peer, all in pure
    // bytes (no transport involved).
    let scan = write_scan_enable(SCAN_ENABLE_INQUIRY).expect("valid");
    assert_eq!(
        &scan.finish()[..2],
        &CMD_WRITE_SCAN_ENABLE.to_le_bytes()[..]
    );
    let ask = inquiry([0x33, 0x8B, 0x9E], 8, 0).expect("valid");
    assert_eq!(ask.len(), 3 + 5);
    let result: [u8; 16] = [
        EVENT_INQUIRY_RESULT,
        14,
        0x01,
        0x11,
        0x22,
        0x33,
        0x44,
        0x55,
        0x66,
        0x01,
        0x00,
        0x04,
        0x05,
        0x00,
        0x00,
        0x00,
    ];
    let found = decode_event(&result).expect("decodes");
    let Event::InquiryResult {
        class_of_device,
        bd_addr,
        clock_offset,
        page_scan_repetition_mode,
    } = found
    else {
        panic!("inquiry result expected");
    };
    assert_eq!(
        DeviceClass::from_cod(class_of_device),
        DeviceClass::Keyboard
    );
    let connect = create_connection(CreateConnectionParams {
        bd_addr,
        class_of_device,
        packet_type: 0x0008,
        page_scan_repetition_mode,
        clock_offset: Some(clock_offset & 0x7FFF),
        allow_role_switch: false,
    })
    .expect("keyboard peer connectable");
    assert_eq!(connect.len(), 16);
}

// ---------------------------------------------------------------------------
// Pairing state machine stub (honest Unimplemented for SSP)
// ---------------------------------------------------------------------------

#[test]
fn pairing_stub_walks_the_legacy_link_key_path() {
    let mut pairing = PairingStub::new();
    assert_eq!(pairing.state(), PairingState::NotPaired);
    assert_eq!(
        pairing.begin_link_key_exchange(),
        Ok(PairingOutcome::Started)
    );
    assert_eq!(pairing.state(), PairingState::LinkKeyExchange);
    assert_eq!(pairing.on_link_key_stored(), Ok(PairingOutcome::KeyStored));
    assert_eq!(pairing.state(), PairingState::Paired);
}

#[test]
fn pairing_stub_rejects_illegal_transitions() {
    let mut pairing = PairingStub::new();
    // Key storage before the exchange begins.
    assert_eq!(
        pairing.on_link_key_stored(),
        Err(PairingOutcome::InvalidState)
    );
    assert_eq!(pairing.state(), PairingState::NotPaired);
    pairing.begin_link_key_exchange().expect("starts");
    // Double start.
    assert_eq!(
        pairing.begin_link_key_exchange(),
        Err(PairingOutcome::InvalidState)
    );
    assert_eq!(pairing.state(), PairingState::LinkKeyExchange);
    pairing.on_link_key_stored().expect("stores");
    // Restart from Paired is refused.
    assert_eq!(
        pairing.begin_link_key_exchange(),
        Err(PairingOutcome::InvalidState)
    );
    assert_eq!(pairing.state(), PairingState::Paired);
}

#[test]
fn pairing_stub_honestly_refuses_ssp_phases() {
    let mut pairing = PairingStub::new();
    pairing.begin_link_key_exchange().expect("starts");
    // Every SSP surface refuses with Unimplemented and leaves state alone.
    assert_eq!(
        pairing.on_ssp_io_capability_request(),
        Err(PairingOutcome::Unimplemented)
    );
    assert_eq!(pairing.state(), PairingState::LinkKeyExchange);
    // From NotPaired too.
    let mut fresh = PairingStub::new();
    assert_eq!(
        fresh.on_ssp_io_capability_request(),
        Err(PairingOutcome::Unimplemented)
    );
    assert_eq!(fresh.state(), PairingState::NotPaired);
}

// Silence the unused-import lint for constants exercised only via goldens.
#[allow(unused_imports)]
use bluetooth::MAX_COMMAND_BYTES;

#[test]
fn inquiry_scan_constants_hold_their_spec_values() {
    assert_eq!(CMD_RESET, 0x0C03);
    assert_eq!(CMD_WRITE_SCAN_ENABLE, 0x0C1A);
    assert_eq!(CMD_INQUIRY, 0x0401);
    assert_eq!(CMD_CREATE_CONNECTION, 0x0405);
    assert_eq!(SCAN_ENABLE_PAGE, 0x02);
}

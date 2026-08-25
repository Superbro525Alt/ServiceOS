//! Peripheral service: device registry (classify/attach/detach over the
//! kernel's enumeration vocabulary), bounded attach/detach event log, and
//! the printer class query stub (honestly Unimplemented).
//!
//! Activation (manual, not in the default boot graph): the image is built
//! into the boot store as `services/peripheral-service/program.img` and
//! spawned on demand via the manager's stored-image launch path. The service
//! is NOT registered under a named `ServiceId`, mirroring account-service,
//! backup-service, and power-service. Launchers that pass an announcer
//! handle in startup handles[0] receive this service's public channel via
//! the launch handshake; bare launches stay reachable through whatever
//! delivery the spawner arranges.
//!
//! Honest hardware note: manual-activation services receive no kernel device
//! transports at spawn, so the v0 registry fills only through its own ATTACH
//! contract — clients that hold real input/block/display handles report the
//! descriptors and the service classifies them into known classes. With no
//! registrants it honestly reports zero devices; a transport-injection path
//! can light up live enumeration later without tag changes.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

mod protocol;

use rt::{ControlTag, LifecycleEvent, RawMessage};
use serviceos_peripheral_service::PeripheralServiceState;

use crate::protocol::{RequestScratch, handle_request};

use serviceos_userspace_runtime as rt;

const EXIT_OK: u64 = 0;
const EXIT_STARTUP: u64 = 0xfc01;
const EXIT_LOOP: u64 = 0xfc02;

#[cfg(not(test))]
rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return EXIT_STARTUP;
    }
    if startup.tag != ControlTag::Startup as u32 {
        return EXIT_STARTUP;
    }

    let mut state = PeripheralServiceState::new();
    // Registry starts empty and stays that way until a client holding real
    // transports reports descriptors over the ATTACH contract.
    let _ = rt::debug_log(b"peripheral-service ready; registry empty; printer=unimplemented");

    // Public control channel. Launch handshake (mirrors account-service's
    // wizard path): when the spawner passed a startup handle, its send-half
    // receives our public channel's send-half so the launcher can reach us;
    // handle_count == 0 launches publish nothing.
    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return EXIT_STARTUP,
    };
    let _ = public.second;
    if startup.handle_count >= 1 {
        let mut announce = RawMessage::empty(0);
        announce.word_count = 1;
        announce.words[0] = 1; // protocol version
        announce.handle_count = 1;
        announce.handles[0] = public.second;
        announce.handle_rights[0] =
            rt::rights::SEND | rt::rights::DUPLICATE | rt::rights::TRANSFER;
        let _ = rt::channel_send(startup.handles[0], &announce);
    }

    loop {
        if lifecycle_stop_requested(bootstrap) {
            let _ = rt::handle_close(public.first);
            return EXIT_OK;
        }

        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => serve(&mut state, &request),
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return EXIT_LOOP,
        }

        if rt::yield_current().is_err() {
            return EXIT_LOOP;
        }
    }
}

fn serve(state: &mut PeripheralServiceState, request: &RawMessage) {
    let reply_to = request.handles[0];
    let mut response = RawMessage::empty(0);
    let mut scratch = RequestScratch::new();
    handle_request(state, request, &mut response, &mut scratch, rt::monotonic_now().unwrap_or(0));
    if response.tag != 0 && reply_to != 0 {
        let _ = rt::channel_send(reply_to, &response);
    }
}

fn lifecycle_stop_requested(bootstrap: rt::Handle) -> bool {
    let mut lifecycle = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut lifecycle) {
        Ok(()) => {
            lifecycle.tag == ControlTag::Lifecycle as u32
                && lifecycle.word_count >= 1
                && lifecycle.words[0] == LifecycleEvent::Stopped as u32 as u64
        }
        Err(rt::Error::QueueEmpty) => false,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serviceos_peripheral_service::{
        DeviceClass, PeripheralError, device_family, peripheral_tag, printer_report,
        unpack_device_record, unpack_event_detail,
    };

    fn request(tag: u32, words: &[u64]) -> RawMessage {
        let mut message = RawMessage::empty(tag);
        message.word_count = words.len() as u32;
        message.words[..words.len()].copy_from_slice(words);
        message.handles[0] = 9;
        message.handle_count = 1;
        message
    }

    fn call(
        state: &mut PeripheralServiceState,
        tag: u32,
        words: &[u64],
    ) -> RawMessage {
        let mut response = RawMessage::empty(0);
        let mut scratch = RequestScratch::new();
        handle_request(state, &request(tag, words), &mut response, &mut scratch, 500);
        response
    }

    #[test]
    fn attach_list_events_detach_roundtrip_through_protocol() {
        let mut state = PeripheralServiceState::new();

        let attach = call(
            &mut state,
            peripheral_tag::ATTACH_REQUEST,
            &[device_family::INPUT as u64, 2, 1, 0, 3],
        );
        assert_eq!(attach.tag, peripheral_tag::ATTACH_REPLY);
        assert_eq!(attach.words[0], 0);
        assert_eq!(attach.words[1], 1);
        assert_eq!(attach.words[2], DeviceClass::Pointer as u64);
        assert_eq!(attach.word_count, 3);

        let keyboard = call(
            &mut state,
            peripheral_tag::ATTACH_REQUEST,
            &[device_family::INPUT as u64, 1, 1, 0, 0],
        );
        assert_eq!(keyboard.words[2], DeviceClass::Keyboard as u64);

        // Unfiltered list returns every record in packed form.
        let list = call(&mut state, peripheral_tag::LIST_REQUEST, &[]);
        assert_eq!(list.tag, peripheral_tag::LIST_REPLY);
        assert_eq!(list.words[1], 2);
        assert_eq!(list.words[3], 2);
        let first = unpack_device_record(list.words[4]);
        assert_eq!(first.id, 1);
        assert_eq!(first.class, DeviceClass::Pointer);
        assert_eq!(first.meta, 3);

        // Class-filtered list narrows to one class.
        let filtered = call(
            &mut state,
            peripheral_tag::LIST_REQUEST,
            &[1, DeviceClass::Keyboard as u64],
        );
        assert_eq!(filtered.words[1], 1);
        assert_eq!(filtered.words[3], 1);
        assert_eq!(
            unpack_device_record(filtered.words[4]).class,
            DeviceClass::Keyboard
        );

        // Events arrive newest-last with totals and packed detail.
        let detach = call(&mut state, peripheral_tag::DETACH_REQUEST, &[2]);
        assert_eq!(detach.words[0], 0);
        assert_eq!(detach.words[1], 2);
        let events = call(&mut state, peripheral_tag::EVENTS_REQUEST, &[3]);
        assert_eq!(events.tag, peripheral_tag::EVENTS_REPLY);
        assert_eq!(events.words[0], 0);
        assert_eq!(events.words[1], 2);
        assert_eq!(events.words[2], 1);
        assert_eq!(events.words[3], 3);
        // Layout per event: [seq][tick][detail]; three events at bases
        // 4 / 7 / 10 with the newest last.
        assert_eq!(events.words[4], 1);
        assert_eq!(events.words[7], 2);
        assert_eq!(events.words[10], 3);
        // The test harness stamps every call at tick 500.
        assert_eq!(events.words[5], 500);
        assert_eq!(events.words[8], 500);
        assert_eq!(events.words[11], 500);
        let (kind_first, device_first, class_first) =
            unpack_event_detail(events.words[6]);
        assert_eq!(
            (kind_first, device_first, class_first),
            (
                serviceos_peripheral_service::EventKind::Attach as u64,
                1,
                DeviceClass::Pointer as u64
            )
        );
        let (kind_last, device_last, _) = unpack_event_detail(events.words[12]);
        assert_eq!(
            kind_last,
            serviceos_peripheral_service::EventKind::Detach as u64
        );
        assert_eq!(device_last, 2);
        assert_eq!(events.word_count, 4 + 3 * 3);

        // Detaching an unknown id reports NotFound.
        let missing = call(&mut state, peripheral_tag::DETACH_REQUEST, &[42]);
        assert_eq!(
            missing.words[0],
            PeripheralError::NotFound.to_code()
        );
    }

    #[test]
    fn attach_capacity_and_unknown_class_report_errors() {
        let mut state = PeripheralServiceState::new();
        for index in 0..serviceos_peripheral_service::MAX_DEVICES as u64 {
            let reply = call(
                &mut state,
                peripheral_tag::ATTACH_REQUEST,
                &[device_family::BLOCK as u64, 0, 1, index, 0],
            );
            assert_eq!(reply.words[0], 0);
        }
        let overflow = call(
            &mut state,
            peripheral_tag::ATTACH_REQUEST,
            &[device_family::BLOCK as u64, 0, 1, 99, 0],
        );
        assert_eq!(overflow.words[0], PeripheralError::CapacityExceeded.to_code());

        let unknown = call(
            &mut state,
            peripheral_tag::ATTACH_REQUEST,
            &[77, 1, 0, 0, 0],
        );
        assert_eq!(unknown.words[0], PeripheralError::InvalidArgument.to_code());
    }

    #[test]
    fn status_and_printer_queries_keep_honest_shape() {
        let mut state = PeripheralServiceState::new();
        let status = call(&mut state, peripheral_tag::STATUS_REQUEST, &[]);
        assert_eq!(status.tag, peripheral_tag::STATUS_REPLY);
        assert_eq!(status.word_count, 6);
        assert_eq!(status.words[1], 0);
        assert_eq!(status.words[4], 0);
        assert_eq!(
            status.words[5],
            printer_report().status as u64
        );

        let printer = call(&mut state, peripheral_tag::PRINTER_QUERY_REQUEST, &[]);
        assert_eq!(printer.tag, peripheral_tag::PRINTER_QUERY_REPLY);
        assert_eq!(printer.words[0], 0);
        assert_eq!(printer.words[1], 2); // Unimplemented
        assert_eq!(printer.words[2], 0);
        assert_eq!(printer.words[3], 8);
        assert_eq!(printer.word_count, 5);

        // Unknown tags get no reply at all.
        let silent = call(&mut state, 0x7fff, &[]);
        assert_eq!(silent.tag, 0);
        assert_eq!(silent.word_count, 0);
    }
}

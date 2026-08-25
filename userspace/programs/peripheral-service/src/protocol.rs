//! Wire protocol handler for the peripheral service's control channel.
//! Replies are status-first (`PeripheralError::to_code`, 0 = Ok) followed by
//! op-specific words; every reply fits the 16-word IPC budget.

use serviceos_peripheral_service::{
    DeviceClass, PeripheralError, PeripheralServiceState, PrinterStatus, MAX_DEVICES,
    MAX_EVENTS_PER_REPLY, pack_device_record, peripheral_tag, printer_report,
};
use serviceos_userspace_runtime::RawMessage;

pub struct RequestScratch {
    pub devices: [u64; MAX_DEVICES],
}

impl RequestScratch {
    pub const fn new() -> Self {
        Self { devices: [0; MAX_DEVICES] }
    }
}

impl Default for RequestScratch {
    fn default() -> Self {
        Self::new()
    }
}

pub fn handle_request(
    state: &mut PeripheralServiceState,
    request: &RawMessage,
    response: &mut RawMessage,
    scratch: &mut RequestScratch,
    now_ticks: u64,
) {
    match request.tag {
        x if x == peripheral_tag::ATTACH_REQUEST => {
            response.tag = peripheral_tag::ATTACH_REPLY;
            let family = *request.words.first().unwrap_or(&u64::MAX) as u32;
            let detail = *request.words.get(1).unwrap_or(&0) as u32;
            let backend = *request.words.get(2).unwrap_or(&0) as u32;
            let flags = *request.words.get(3).unwrap_or(&0) as u32;
            let meta = *request.words.get(4).unwrap_or(&0) as u32;
            match state.attach(family, detail, backend, flags, meta, now_ticks) {
                Ok(record) => {
                    response.word_count = 3;
                    response.words[0] = 0;
                    response.words[1] = record.id as u64;
                    response.words[2] = record.class as u64;
                }
                Err(error) => fail(response, error),
            }
        }
        x if x == peripheral_tag::DETACH_REQUEST => {
            response.tag = peripheral_tag::DETACH_REPLY;
            let Some(&id) = request.words.first() else {
                return fail(response, PeripheralError::InvalidArgument);
            };
            match state.detach(id as u32, now_ticks) {
                Ok(record) => {
                    response.word_count = 3;
                    response.words[0] = 0;
                    response.words[1] = record.id as u64;
                    response.words[2] = record.class as u64;
                }
                Err(error) => fail(response, error),
            }
        }
        x if x == peripheral_tag::LIST_REQUEST => {
            response.tag = peripheral_tag::LIST_REPLY;
            // words[0]=1 filters to the class in words[1]; otherwise all.
            let filter = match request.words.first().copied().unwrap_or(0) {
                1 => Some(DeviceClass::from_word(*request.words.get(1).unwrap_or(&0))),
                _ => None,
            };
            let mut packed = 0usize;
            state.registry.for_each(filter, |record| {
                if packed < scratch.devices.len() {
                    scratch.devices[packed] = pack_device_record(record);
                    packed += 1;
                }
            });
            let total = state.registry.count_matching(filter);
            response.word_count = (4 + packed) as u32;
            response.words[0] = 0;
            response.words[1] = total as u64;
            response.words[2] = filter.map(|class| class as u64).unwrap_or(0);
            response.words[3] = packed as u64;
            for index in 0..packed {
                response.words[4 + index] = scratch.devices[index];
            }
        }
        x if x == peripheral_tag::EVENTS_REQUEST => {
            response.tag = peripheral_tag::EVENTS_REPLY;
            let wanted = request
                .words
                .first()
                .copied()
                .unwrap_or(MAX_EVENTS_PER_REPLY as u64)
                .min(MAX_EVENTS_PER_REPLY as u64) as usize;
            response.words[0] = 0;
            response.words[1] = state.events.attach_total();
            response.words[2] = state.events.detach_total();
            let mut written = 0usize;
            // Newest-last: each event takes seq, tick, and a packed
            // {kind<<40 | device_id<<16 | class} word.
            state.events.last_n(wanted, |event| {
                let base = 4 + written * 3;
                response.words[base] = event.seq;
                response.words[base + 1] = event.tick;
                response.words[base + 2] = ((event.kind as u64) << 40)
                    | ((event.device_id as u64) << 16)
                    | (event.class as u64 & 0xff);
                written += 1;
            });
            response.words[3] = written as u64;
            response.word_count = (4 + written * 3) as u32;
        }
        x if x == peripheral_tag::PRINTER_QUERY_REQUEST => {
            response.tag = peripheral_tag::PRINTER_QUERY_REPLY;
            let report = printer_report();
            response.word_count = 5;
            response.words[0] = 0;
            response.words[1] = report.status as u64;
            response.words[2] = report.queue_depth as u64;
            response.words[3] = report.queue_capacity as u64;
            response.words[4] = PrinterStatus::Unimplemented as u64;
        }
        x if x == peripheral_tag::STATUS_REQUEST => {
            response.tag = peripheral_tag::STATUS_REPLY;
            response.word_count = 6;
            response.words[0] = 0;
            response.words[1] = state.registry.count_matching(None) as u64;
            response.words[2] = state.events.attach_total();
            response.words[3] = state.events.detach_total();
            response.words[4] = state.events.next_seq().saturating_sub(1);
            response.words[5] = printer_report().status as u64;
        }
        _ => {}
    }
}

fn fail(response: &mut RawMessage, error: PeripheralError) {
    response.word_count = 1;
    response.words[0] = error.to_code();
}

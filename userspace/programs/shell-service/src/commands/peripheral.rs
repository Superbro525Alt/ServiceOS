//! Desktop-facing peripheral management surface: `peripheral` subcommands
//! drive the manually-activated peripheral-service (registry listing,
//! attach/detach event log, printer query stub).
//!
//! Reachability mirrors the account-command precedent: the image lives in
//! the boot store as `services/peripheral-service/program.img` and is
//! launched on demand through the manager's stored-image path, whose reply
//! carries the service's public channel handle. Protocol tags are the ones
//! published by `serviceos_peripheral_service::peripheral_tag`. When launch
//! fails, peripheral surfaces degrade to notices — activation is manual, so
//! absence is a normal state.

use core::cell::UnsafeCell;

use rt::{Handle, RawMessage};
use serviceos_peripheral_service::{
    DeviceClass, MAX_EVENTS_PER_REPLY, peripheral_tag, printer_report, unpack_event_detail,
};
use serviceos_userspace_runtime as rt;

use crate::util::{ShellOutput, write_output_linef};

/// Boot-store location of the peripheral-service image (manual activation).
pub const PERIPHERAL_PROGRAM_PATH: &str = "services/peripheral-service/program.img";

struct PeripheralChannel {
    handle: Handle,
    reachable: bool,
}

struct CacheSlot(UnsafeCell<PeripheralChannel>);

// SAFETY: the shell task is strictly single-threaded; see the account-cache
// precedent in commands/account.rs.
unsafe impl Sync for CacheSlot {}

static PERIPHERAL_CACHE: CacheSlot = CacheSlot(UnsafeCell::new(PeripheralChannel {
    handle: rt::INVALID_HANDLE,
    reachable: false,
}));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeripheralFlow {
    /// Stored-image launch failed or was denied.
    Unavailable,
    /// Service replied with a non-zero status code.
    Rejected(u64),
    /// Wire-level failure talking to the service.
    Transport,
}

impl PeripheralFlow {
    pub const fn message(self) -> &'static str {
        match self {
            PeripheralFlow::Unavailable => {
                "peripheral-service unavailable (not in boot store or launch denied)"
            }
            PeripheralFlow::Rejected(2) => "peripheral-service rejected: unknown device",
            PeripheralFlow::Rejected(_) => "peripheral-service rejected the request",
            PeripheralFlow::Transport => "peripheral-service transport failure",
        }
    }
}

fn cache() -> &'static mut PeripheralChannel {
    // SAFETY: single-threaded shell task.
    unsafe { &mut *PERIPHERAL_CACHE.0.get() }
}

fn ensure_peripheral_channel(bootstrap: rt::Handle) -> Option<Handle> {
    let slot = cache();
    if slot.reachable && slot.handle != rt::INVALID_HANDLE {
        return Some(slot.handle);
    }
    let handle = super::account::launch_with_announce(bootstrap, PERIPHERAL_PROGRAM_PATH, false)?;
    slot.handle = handle;
    slot.reachable = true;
    Some(handle)
}

fn call(bootstrap: rt::Handle, tag: u32, words: &[u64]) -> Result<RawMessage, PeripheralFlow> {
    let Some(handle) = ensure_peripheral_channel(bootstrap) else {
        return Err(PeripheralFlow::Unavailable);
    };
    let mut request = RawMessage::empty(tag);
    request.word_count = words.len() as u32;
    request.words[..words.len()].copy_from_slice(words);
    let response = rt::channel_call(handle, &mut request)
        .map_err(|_| PeripheralFlow::Transport)?;
    if response.word_count < 1 {
        return Err(PeripheralFlow::Transport);
    }
    if response.words[0] != 0 {
        return Err(PeripheralFlow::Rejected(response.words[0]));
    }
    Ok(response)
}

/// Class-name lookup shared with command parsing (`list keyboard`, etc.).
pub fn parse_class_name(name: &str) -> Option<u64> {
    match name {
        "unknown" => Some(DeviceClass::Unknown as u64),
        "keyboard" => Some(DeviceClass::Keyboard as u64),
        "pointer" => Some(DeviceClass::Pointer as u64),
        "tablet" => Some(DeviceClass::Tablet as u64),
        "block" => Some(DeviceClass::Block as u64),
        "display" => Some(DeviceClass::Display as u64),
        "audio" => Some(DeviceClass::Audio as u64),
        "printer" => Some(DeviceClass::Printer as u64),
        _ => None,
    }
}

pub(crate) fn cmd_peripheral(
    bootstrap: rt::Handle,
    output: ShellOutput,
    parts: core::str::SplitWhitespace<'_>,
) -> rt::Result<()> {
    let mut parts = parts;
    match parts.next() {
        None | Some("status") => cmd_status(bootstrap, output),
        Some("list") => cmd_list(bootstrap, output, parts.next()),
        Some("events") => cmd_events(bootstrap, output, parts.next()),
        Some("printer") => cmd_printer(output),
        Some(_) => write_output_linef(
            output,
            format_args!("usage: peripheral [status|list [class]|events [n]|printer]"),
        ),
    }
}

fn cmd_status(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    match call(bootstrap, peripheral_tag::STATUS_REQUEST, &[]) {
        Ok(reply) if reply.word_count >= 6 => write_output_linef(
            output,
            format_args!(
                "peripherals: devices={} attach={} detach={} events={} printer=unimplemented",
                reply.words[1],
                reply.words[2],
                reply.words[3],
                reply.words[4],
            ),
        ),
        Ok(_) => write_output_linef(output, format_args!("{}", PeripheralFlow::Transport.message())),
        Err(flow) => write_output_linef(output, format_args!("{}", flow.message())),
    }
}

fn cmd_list(bootstrap: rt::Handle, output: ShellOutput, filter: Option<&str>) -> rt::Result<()> {
    let (words, label): ([u64; 2], &str) = match filter.map(parse_class_name) {
        Some(Some(class)) => ([1, class], filter.expect("matched")),
        Some(None) => {
            return write_output_linef(
                output,
                format_args!("unknown class; try keyboard|pointer|tablet|block|display|audio|printer"),
            );
        }
        None => ([0, 0], "all"),
    };
    match call(bootstrap, peripheral_tag::LIST_REQUEST, &words) {
        Ok(reply) if reply.word_count >= 4 => {
            let total = reply.words[1];
            let count = reply.words[3] as usize;
            write_output_linef(
                output,
                format_args!("devices ({label}): {} attached", total),
            )?;
            for index in 0..count.min(12) {
                let packed = reply.words[4 + index];
                write_output_linef(
                    output,
                    format_args!(
                        "id={} class={} backend={} flags={:#x} meta={:#x}",
                        packed & 0xffff,
                        DeviceClass::from_word((packed >> 16) & 0xff).name(),
                        (packed >> 24) & 0xff,
                        (packed >> 32) & 0xffff,
                        (packed >> 48) & 0xffff,
                    ),
                )?;
            }
            Ok(())
        }
        Ok(_) => write_output_linef(output, format_args!("{}", PeripheralFlow::Transport.message())),
        Err(flow) => write_output_linef(output, format_args!("{}", flow.message())),
    }
}

fn cmd_events(bootstrap: rt::Handle, output: ShellOutput, count: Option<&str>) -> rt::Result<()> {
    let wanted = count
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(MAX_EVENTS_PER_REPLY as u64)
        .min(MAX_EVENTS_PER_REPLY as u64);
    match call(bootstrap, peripheral_tag::EVENTS_REQUEST, &[wanted]) {
        Ok(reply) if reply.word_count >= 4 => {
            let written = reply.words[3] as usize;
            write_output_linef(
                output,
                format_args!(
                    "events ({} of attach={} detach={}, newest last):",
                    written, reply.words[1], reply.words[2],
                ),
            )?;
            for index in 0..written.min(MAX_EVENTS_PER_REPLY) {
                let base = 4 + index * 3;
                let (kind, device_id, class) = unpack_event_detail(reply.words[base + 2]);
                write_output_linef(
                    output,
                    format_args!(
                        "seq={} tick={} kind={} device={} class={}",
                        reply.words[base],
                        reply.words[base + 1],
                        if kind == 2 { "detach" } else { "attach" },
                        device_id,
                        DeviceClass::from_word(class).name(),
                    ),
                )?;
            }
            Ok(())
        }
        Ok(_) => write_output_linef(output, format_args!("{}", PeripheralFlow::Transport.message())),
        Err(flow) => write_output_linef(output, format_args!("{}", flow.message())),
    }
}

fn cmd_printer(output: ShellOutput) -> rt::Result<()> {
    // Honest stub rendering straight from the shared shape; the wire query
    // would say the same thing once a printer transport exists at all.
    let report = printer_report();
    let _ = report.status as u64;
    write_output_linef(
        output,
        format_args!(
            "printer: status=unimplemented queue={}/{} (no print pipeline yet)",
            report.queue_depth, report.queue_capacity,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_names_cover_every_known_class_and_reject_junk() {
        assert_eq!(parse_class_name("keyboard"), Some(DeviceClass::Keyboard as u64));
        assert_eq!(parse_class_name("pointer"), Some(DeviceClass::Pointer as u64));
        assert_eq!(parse_class_name("tablet"), Some(DeviceClass::Tablet as u64));
        assert_eq!(parse_class_name("block"), Some(DeviceClass::Block as u64));
        assert_eq!(parse_class_name("display"), Some(DeviceClass::Display as u64));
        assert_eq!(parse_class_name("audio"), Some(DeviceClass::Audio as u64));
        assert_eq!(parse_class_name("printer"), Some(DeviceClass::Printer as u64));
        assert_eq!(parse_class_name(""), None);
        assert_eq!(parse_class_name("toaster"), None);
    }

    #[test]
    fn event_details_unpack_into_kind_device_and_class() {
        let attach = (serviceos_peripheral_service::EventKind::Attach as u64) << 40
            | (7u64 << 16)
            | DeviceClass::Pointer as u64;
        assert_eq!(
            unpack_event_detail(attach),
            (
                serviceos_peripheral_service::EventKind::Attach as u64,
                7,
                DeviceClass::Pointer as u64
            )
        );
        let detach = (serviceos_peripheral_service::EventKind::Detach as u64) << 40
            | (3u64 << 16)
            | DeviceClass::Block as u64;
        let (kind, id, class) = unpack_event_detail(detach);
        assert_eq!(kind, serviceos_peripheral_service::EventKind::Detach as u64);
        assert_eq!((id, class), (3, DeviceClass::Block as u64));
    }

    #[test]
    fn flow_messages_stay_operator_readable() {
        assert_eq!(
            PeripheralFlow::Unavailable.message(),
            "peripheral-service unavailable (not in boot store or launch denied)"
        );
        assert_eq!(
            PeripheralFlow::Rejected(2).message(),
            "peripheral-service rejected: unknown device"
        );
        assert!(PeripheralFlow::Transport.message().starts_with("peripheral"));
    }
}

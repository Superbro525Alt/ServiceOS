#![no_std]
#![no_main]

use serviceos_userspace_runtime as rt;
use rt::{ClipboardStatus, ClipboardTag, ControlTag, LifecycleEvent, RawMessage, ServiceId};

const MAX_CLIPBOARD_BYTES: usize = rt::IPC_MAX_WORDS * 8;
const HISTORY_SLOTS: usize = 8;

#[derive(Clone, Copy)]
struct HistorySlot {
    occupied: bool,
    len: usize,
    bytes: [u8; MAX_CLIPBOARD_BYTES],
}

impl HistorySlot {
    const fn empty() -> Self {
        Self {
            occupied: false,
            len: 0,
            bytes: [0; MAX_CLIPBOARD_BYTES],
        }
    }
}

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xfd01;
    }
    if startup.tag != ControlTag::Startup as u32 {
        return 0xfd02;
    }

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xfd03,
    };
    if rt::register_service(bootstrap, ServiceId::Clipboard, public.second).is_err() {
        return 0xfd04;
    }
    let _ = rt::handle_close(public.second);

    let mut bytes = [0u8; MAX_CLIPBOARD_BYTES];
    let mut len = 0usize;
    let mut history = [HistorySlot::empty(); HISTORY_SLOTS];
    let mut history_len = 0usize;

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xfd05,
        }

        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut message) {
            Ok(()) => {
                if handle_public_request(
                    &mut bytes,
                    &mut len,
                    &mut history,
                    &mut history_len,
                    &message,
                )
                .is_err()
                {
                    return 0xfd06;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xfd07,
        }

        if rt::yield_current().is_err() {
            return 0xfd08;
        }
    }
}

fn handle_public_request(
    clipboard: &mut [u8; MAX_CLIPBOARD_BYTES],
    clipboard_len: &mut usize,
    history: &mut [HistorySlot; HISTORY_SLOTS],
    history_len: &mut usize,
    message: &RawMessage,
) -> rt::Result<()> {
    match message.tag {
        x if x == ClipboardTag::ReadRequest as u32 => {
            if message.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = message.handles[0];
            let mut reply = RawMessage::empty(ClipboardTag::ReadReply as u32);
            if *clipboard_len == 0 {
                reply.word_count = 1;
                reply.words[0] = ClipboardStatus::NotFound as u32 as u64;
            } else {
                reply.word_count = 2 + pack_bytes(&clipboard[..*clipboard_len], &mut reply.words[2..])?;
                reply.words[0] = ClipboardStatus::Ok as u32 as u64;
                reply.words[1] = *clipboard_len as u64;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == ClipboardTag::WriteRequest as u32 => {
            if message.handle_count < 1 || message.word_count < 1 {
                return Ok(());
            }
            let reply_handle = message.handles[0];
            let requested = message.words[0] as usize;
            let status = if requested > clipboard.len() {
                ClipboardStatus::Denied
            } else {
                unpack_bytes(&message.words[1..message.word_count as usize], requested, clipboard)?;
                *clipboard_len = requested;
                record_history(history, history_len, &clipboard[..requested]);
                ClipboardStatus::Ok
            };
            let mut reply = RawMessage::empty(ClipboardTag::WriteReply as u32);
            reply.word_count = 1;
            reply.words[0] = status as u32 as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == ClipboardTag::HistoryRequest as u32 => {
            if message.handle_count < 1 || message.word_count < 1 {
                return Ok(());
            }
            let reply_handle = message.handles[0];
            let index = message.words[0] as usize;
            let mut reply = RawMessage::empty(ClipboardTag::HistoryReply as u32);
            if index >= *history_len || !history[index].occupied {
                reply.word_count = 1;
                reply.words[0] = ClipboardStatus::NotFound as u32 as u64;
            } else {
                let slot = history[index];
                reply.word_count = 4 + pack_bytes(&slot.bytes[..slot.len], &mut reply.words[4..])?;
                reply.words[0] = ClipboardStatus::Ok as u32 as u64;
                reply.words[1] = index as u64;
                reply.words[2] = u64::from(index == 0 && *clipboard_len != 0);
                reply.words[3] = slot.len as u64;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == ClipboardTag::ActivateRequest as u32 => {
            if message.handle_count < 1 || message.word_count < 1 {
                return Ok(());
            }
            let reply_handle = message.handles[0];
            let index = message.words[0] as usize;
            let status = if index >= *history_len || !history[index].occupied {
                ClipboardStatus::NotFound
            } else {
                let slot = history[index];
                clipboard[..slot.len].copy_from_slice(&slot.bytes[..slot.len]);
                *clipboard_len = slot.len;
                if index != 0 {
                    record_history(history, history_len, &slot.bytes[..slot.len]);
                }
                ClipboardStatus::Ok
            };
            let mut reply = RawMessage::empty(ClipboardTag::ActivateReply as u32);
            reply.word_count = 1;
            reply.words[0] = status as u32 as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        _ => {}
    }
    Ok(())
}

fn record_history(
    history: &mut [HistorySlot; HISTORY_SLOTS],
    history_len: &mut usize,
    bytes: &[u8],
) {
    if bytes.is_empty() {
        return;
    }
    if *history_len > 0 && history[0].occupied && history[0].len == bytes.len() {
        if history[0].bytes[..bytes.len()] == bytes[..] {
            return;
        }
    }
    let limit = (*history_len).min(HISTORY_SLOTS - 1);
    for index in (0..limit).rev() {
        history[index + 1] = history[index];
    }
    history[0].occupied = true;
    history[0].len = bytes.len();
    history[0].bytes[..bytes.len()].copy_from_slice(bytes);
    if *history_len < HISTORY_SLOTS {
        *history_len += 1;
    }
}

fn pack_bytes(source: &[u8], words: &mut [u64]) -> rt::Result<u32> {
    let required = source.len().div_ceil(8);
    if required > words.len() {
        return Err(rt::Error::BufferTooSmall);
    }
    for (index, chunk) in source.chunks(8).enumerate() {
        let mut bytes = [0u8; 8];
        bytes[..chunk.len()].copy_from_slice(chunk);
        words[index] = u64::from_le_bytes(bytes);
    }
    Ok(required as u32)
}

fn unpack_bytes(words: &[u64], len: usize, destination: &mut [u8]) -> rt::Result<()> {
    if len > destination.len() || len > words.len() * 8 {
        return Err(rt::Error::BufferTooSmall);
    }
    let mut copied = 0usize;
    for word in words.iter().copied() {
        if copied >= len {
            break;
        }
        let bytes = word.to_le_bytes();
        let chunk = (len - copied).min(bytes.len());
        destination[copied..copied + chunk].copy_from_slice(&bytes[..chunk]);
        copied += chunk;
    }
    Ok(())
}

fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
    let mut message = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut message) {
        Ok(()) if message.tag == ControlTag::Lifecycle as u32 && message.word_count > 0 => {
            Ok(matches!(
                lifecycle_event_from_word(message.words[0]),
                LifecycleEvent::Restarting | LifecycleEvent::Stopped
            ))
        }
        Ok(()) => Ok(false),
        Err(rt::Error::QueueEmpty) => Ok(false),
        Err(error) => Err(error),
    }
}

fn lifecycle_event_from_word(value: u64) -> LifecycleEvent {
    match value as u32 {
        x if x == LifecycleEvent::Starting as u32 => LifecycleEvent::Starting,
        x if x == LifecycleEvent::Ready as u32 => LifecycleEvent::Ready,
        x if x == LifecycleEvent::Failed as u32 => LifecycleEvent::Failed,
        x if x == LifecycleEvent::Stopped as u32 => LifecycleEvent::Stopped,
        _ => LifecycleEvent::Restarting,
    }
}

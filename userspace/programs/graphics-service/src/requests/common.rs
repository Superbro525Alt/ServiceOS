use rt::{GraphicsStatus, RawMessage, SurfaceTag};
use serviceos_userspace_runtime as rt;

use crate::types::{DamageSet, DirtyState};

pub(crate) fn reply_surface_status(
    handles: [rt::Handle; rt::IPC_MAX_HANDLES],
    handle_count: u32,
    tag: SurfaceTag,
    status: GraphicsStatus,
) {
    if handle_count == 0 {
        return;
    }
    let handle = handles[0];
    let mut reply = RawMessage::empty(tag as u32);
    reply.word_count = 1;
    reply.words[0] = status as u32 as u64;
    let _ = rt::channel_send(handle, &reply);
    let _ = rt::handle_close(handle);
}

pub(crate) fn unpack_bytes(words: &[u64], len: usize, destination: &mut [u8]) -> rt::Result<()> {
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

pub(crate) fn merge_region_dirty(
    dirty: &mut DirtyState,
    damage: crate::types::DamageRect,
    immediate: bool,
) {
    *dirty = match *dirty {
        DirtyState::Clean => DirtyState::Region {
            damages: DamageSet::empty().push(damage),
            immediate,
        },
        DirtyState::CursorOnly(existing) => DirtyState::Region {
            damages: DamageSet::empty().push(existing).push(damage),
            immediate,
        },
        DirtyState::Region {
            damages: existing,
            immediate: current,
        } => DirtyState::Region {
            damages: existing.push(damage),
            immediate: current || immediate,
        },
        DirtyState::Full { immediate: current } => DirtyState::Full {
            immediate: current || immediate,
        },
    };
}

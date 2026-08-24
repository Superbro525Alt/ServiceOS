use core::cell::UnsafeCell;

/// Single-slot stash for a command line the operator submitted (Enter) while
/// `logs follow` was streaming. The shell main loop drains and executes it
/// after the follow ends. The shell task is strictly single-threaded, so an
/// `UnsafeCell` slot needs no locking.
struct PendingLine {
    slot: UnsafeCell<PendingSlot>,
}

#[derive(Clone, Copy)]
struct PendingSlot {
    len: usize,
    bytes: [u8; MAX_PENDING_LINE],
}

const MAX_PENDING_LINE: usize = 128;

unsafe impl Sync for PendingLine {}

static PENDING_LINE: PendingLine = PendingLine {
    slot: UnsafeCell::new(PendingSlot {
        len: 0,
        bytes: [0; MAX_PENDING_LINE],
    }),
};

/// Stashes a submitted line, replacing any earlier stashed line.
pub fn stash_pending_line(line: &[u8]) {
    // SAFETY: the shell task is single-threaded; no concurrent access.
    let slot = unsafe { &mut *PENDING_LINE.slot.get() };
    let copy_len = line.len().min(MAX_PENDING_LINE);
    slot.bytes[..copy_len].copy_from_slice(&line[..copy_len]);
    slot.len = copy_len;
}

/// Takes the stashed line into `buffer`, returning its length (0 if none).
pub fn take_pending_line(buffer: &mut [u8]) -> usize {
    // SAFETY: the shell task is single-threaded; no concurrent access.
    let slot = unsafe { &mut *PENDING_LINE.slot.get() };
    let copy_len = slot.len.min(buffer.len());
    buffer[..copy_len].copy_from_slice(&slot.bytes[..copy_len]);
    slot.len = 0;
    copy_len
}

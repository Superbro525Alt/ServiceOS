use core::str;
use serviceos_userspace_runtime as rt;

use crate::state::MAX_STORAGE_PATH;

/// Recent-files ring: move-to-front dedup, fixed capacity. Pure data + codec.
pub(crate) const RECENT_MAX: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RecentRing {
    paths: [[u8; MAX_STORAGE_PATH]; RECENT_MAX],
    lens: [usize; RECENT_MAX],
    len: usize,
}

impl RecentRing {
    pub(crate) const fn empty() -> Self {
        Self {
            paths: [[0; MAX_STORAGE_PATH]; RECENT_MAX],
            lens: [0; RECENT_MAX],
            len: 0,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn get(&self, index: usize) -> Option<&[u8]> {
        if index >= self.len {
            return None;
        }
        Some(&self.paths[index][..self.lens[index]])
    }

    /// Moves `path` to the front, deduplicating an existing copy and capping
    /// at RECENT_MAX by dropping the oldest entry. Returns false only for
    /// empty, oversize, or newline-containing paths.
    pub(crate) fn record(&mut self, path: &[u8]) -> bool {
        if path.is_empty() || path.len() > MAX_STORAGE_PATH || path.contains(&b'\n') {
            return false;
        }
        if let Some(index) = (0..self.len).find(|index| self.get(*index) == Some(path)) {
            if index == 0 {
                return true;
            }
            let saved = (self.paths[index], self.lens[index]);
            shift_range(self, 0..index);
            self.paths[0] = saved.0;
            self.lens[0] = saved.1;
            return true;
        }
        let keep = self.len.min(RECENT_MAX - 1);
        shift_range(self, 0..keep);
        self.paths[0][..path.len()].copy_from_slice(path);
        self.lens[0] = path.len();
        self.len = (self.len + 1).min(RECENT_MAX);
        true
    }

    /// Newline-separated paths, newest first.
    pub(crate) fn encode(&self, buffer: &mut [u8]) -> usize {
        let mut cursor = 0usize;
        for index in 0..self.len {
            let path = &self.paths[index][..self.lens[index]];
            if cursor + path.len() + 1 > buffer.len() {
                break;
            }
            buffer[cursor..cursor + path.len()].copy_from_slice(path);
            cursor += path.len();
            buffer[cursor] = b'\n';
            cursor += 1;
        }
        cursor
    }

    pub(crate) fn decode(bytes: &[u8]) -> Self {
        let mut ring = Self::empty();
        // Decode oldest-first so re-recording reproduces the newest-first order.
        for line in bytes.split(|byte| *byte == b'\n').rev() {
            if line.is_empty() {
                continue;
            }
            ring.record(line);
        }
        ring
    }

    /// Human label: file name portion of a stored path.
    pub(crate) fn label(buffer: &mut rt::FixedLogBuffer<128>, path: &[u8]) {
        let name_start = path
            .iter()
            .rposition(|byte| *byte == b'/')
            .map(|index| index + 1)
            .unwrap_or(0);
        let name = str::from_utf8(&path[name_start..]).unwrap_or("INVALID");
        use core::fmt::Write as _;
        let _ = buffer.write_fmt(format_args!("{name}"));
    }
}

/// Shifts entries [range.start..range.end] one slot later; caller guarantees
/// room (end < RECENT_MAX).
fn shift_range(ring: &mut RecentRing, range: core::ops::Range<usize>) {
    if range.start >= range.end {
        return;
    }
    let mut index = range.end;
    while index > range.start {
        ring.paths[index] = ring.paths[index - 1];
        ring.lens[index] = ring.lens[index - 1];
        index -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(paths: &[&[u8]]) -> RecentRing {
        let mut ring = RecentRing::empty();
        for path in paths {
            assert!(ring.record(path));
        }
        ring
    }

    #[test]
    fn record_moves_to_front_with_dedup() {
        let mut ring = ring(&[b"a.txt", b"b.log", b"c.bin"]);
        assert_eq!(ring.get(0), Some(b"c.bin".as_slice()));
        assert!(ring.record(b"a.txt"));
        assert_eq!(ring.get(0), Some(b"a.txt".as_slice()));
        assert_eq!(ring.get(1), Some(b"c.bin".as_slice()));
        assert_eq!(ring.get(2), Some(b"b.log".as_slice()));
        assert_eq!(ring.len(), 3);
        // Re-record of the front entry is a stable no-op.
        assert!(ring.record(b"a.txt"));
        assert_eq!(ring.get(0), Some(b"a.txt".as_slice()));
        assert_eq!(ring.len(), 3);
    }

    #[test]
    fn capacity_caps_at_recent_max_dropping_oldest() {
        const NAMES: [&[u8]; RECENT_MAX + 2] = [
            b"f0", b"f1", b"f2", b"f3", b"f4", b"f5", b"f6", b"f7", b"f8", b"f9",
        ];
        let mut ring = RecentRing::empty();
        for name in NAMES {
            ring.record(name);
        }
        assert_eq!(ring.len(), RECENT_MAX);
        assert_eq!(ring.get(0), Some(b"f9".as_slice()));
        // Two over-capacity records dropped f0 then f1; f2 is now oldest.
        assert_eq!(ring.get(RECENT_MAX - 1), Some(b"f2".as_slice()));
        assert_eq!(ring.get(RECENT_MAX), None);
    }

    #[test]
    fn rejects_empty_oversize_and_newline_paths() {
        let mut ring = RecentRing::empty();
        assert!(!ring.record(b""));
        assert!(!ring.record(&[b'a'; MAX_STORAGE_PATH + 1]));
        assert!(!ring.record(b"home/a\nb.txt"));
        assert_eq!(ring.len(), 0);
    }

    #[test]
    fn encode_decode_roundtrip_preserves_order() {
        let source = ring(&[b"home/a.txt", b"home/b.log", b"home/c.bin"]);
        let mut buffer = [0u8; 512];
        let len = source.encode(&mut buffer);
        assert_eq!(RecentRing::decode(&buffer[..len]), source);
    }

    #[test]
    fn encode_truncates_at_buffer_edge_without_corruption() {
        let source = ring(&[b"aaaaaaaaaa", b"bbbbbbbbbb"]);
        let mut buffer = [0u8; 12];
        let len = source.encode(&mut buffer);
        assert!(len <= buffer.len());
        let decoded = RecentRing::decode(&buffer[..len]);
        assert!(decoded.len() >= 1);
        assert_eq!(decoded.get(0), Some(b"bbbbbbbbbb".as_slice()));
    }

    #[test]
    fn decode_tolerates_garbage_and_blank_lines() {
        // Encoded form is newest-first, so x.txt is newer than y.log here.
        let ring = RecentRing::decode(b"\nhome/x.txt\n\nhome/y.log\n");
        assert_eq!(ring.len(), 2);
        assert_eq!(ring.get(0), Some(b"home/x.txt".as_slice()));
        assert_eq!(RecentRing::decode(b"").len(), 0);
    }

    #[test]
    fn get_out_of_range_is_none_and_label_shows_name() {
        let source = ring(&[b"home/notes.txt"]);
        assert_eq!(source.get(1), None);
        let mut buffer = rt::FixedLogBuffer::<128>::new();
        RecentRing::label(&mut buffer, b"home/notes.txt");
        assert_eq!(str::from_utf8(buffer.as_bytes()).ok(), Some("notes.txt"));
    }
}

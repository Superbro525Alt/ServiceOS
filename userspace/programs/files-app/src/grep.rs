//! Client for the storage-service content-search contract (grep request
//! 0x523 / reply 0x524, served by `storage-service/src/index.rs`). Bounds
//! mirror the service: needles are non-empty, newline-free, at most
//! GREP_NEEDLE_MAX bytes; the service caps scanned files at
//! GREP_FILE_BYTES_MAX and returns at most GREP_RESULTS_MAX hits per scan,
//! reporting exhaustion through the reply flag word.

use rt::{RawMessage, StorageStatus, channel_call, pack_bytes, unpack_bytes};
use serviceos_userspace_runtime as rt;

pub(crate) const GREP_REQUEST_TAG: u32 = 0x523;
pub(crate) const GREP_REPLY_TAG: u32 = 0x524;
pub(crate) const GREP_NEEDLE_MAX: usize = 32;
pub(crate) const GREP_FLAG_TRUNCATED: u64 = 1 << 0;
pub(crate) const GREP_FLAG_OVERSIZE_SKIPPED: u64 = 1 << 1;

/// Request word layout: cursor, scope_len, needle_len, file_cap, result_cap.
const GREP_HEADER_WORDS: usize = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct GrepHit<const PATH_MAX: usize> {
    pub(crate) next_cursor: usize,
    pub(crate) line: u64,
    pub(crate) truncated: bool,
    pub(crate) oversize_skipped: bool,
    pub(crate) path: [u8; PATH_MAX],
    pub(crate) path_len: usize,
}

impl<const PATH_MAX: usize> GrepHit<PATH_MAX> {
    #[cfg(test)]
    pub(crate) fn path_as_bytes(&self) -> &[u8] {
        &self.path[..self.path_len.min(self.path.len())]
    }
}

/// A validated grep needle: 1..=GREP_NEEDLE_MAX bytes, no newline.
#[derive(Clone, Copy)]
pub(crate) struct GrepNeedle {
    bytes: [u8; GREP_NEEDLE_MAX],
    len: usize,
}

impl GrepNeedle {
    pub(crate) fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() || bytes.len() > GREP_NEEDLE_MAX || bytes.contains(&b'\n') {
            return None;
        }
        let mut needle = Self {
            bytes: [0; GREP_NEEDLE_MAX],
            len: bytes.len(),
        };
        needle.bytes[..bytes.len()].copy_from_slice(bytes);
        Some(needle)
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }
}

/// Packs a 0x523 request: header words plus scope bytes then needle bytes.
/// File/result caps are 0 so the service applies its contract defaults.
pub(crate) fn grep_request(
    cursor: usize,
    scope: &[u8],
    needle: &GrepNeedle,
) -> rt::Result<RawMessage> {
    let scope_words = scope.len().div_ceil(8);
    let needle_words = needle.len().div_ceil(8);
    if GREP_HEADER_WORDS + scope_words + needle_words > rt::IPC_MAX_WORDS {
        return Err(rt::Error::BufferTooSmall);
    }
    let mut request = RawMessage::empty(GREP_REQUEST_TAG);
    request.word_count = GREP_HEADER_WORDS as u32;
    request.words[0] = cursor as u64;
    request.words[1] = scope.len() as u64;
    request.words[2] = needle.len() as u64;
    request.words[3] = 0;
    request.words[4] = 0;
    request.word_count += pack_bytes(scope, &mut request.words[GREP_HEADER_WORDS..])?;
    let needle_offset = (GREP_HEADER_WORDS + scope_words) as u32;
    request.word_count += pack_bytes(
        needle.as_bytes(),
        &mut request.words[needle_offset as usize..],
    )?;
    Ok(request)
}

/// Decodes a 0x524 reply: `Some(hit)` while matches remain, `None` at the
/// End status (flags still carried for the bounds-honesty line).
pub(crate) fn grep_parse_reply<const PATH_MAX: usize>(
    reply: &RawMessage,
) -> rt::Result<Option<GrepHit<PATH_MAX>>> {
    if reply.tag != GREP_REPLY_TAG || reply.word_count < 5 {
        return Err(rt::Error::InvalidArgument);
    }
    let truncated = reply.words[3] & GREP_FLAG_TRUNCATED != 0;
    let oversize_skipped = reply.words[3] & GREP_FLAG_OVERSIZE_SKIPPED != 0;
    if reply.words[0] as u32 == StorageStatus::End as u32 {
        return Ok(None);
    }
    if reply.words[0] as u32 != StorageStatus::Ok as u32 {
        return Err(rt::Error::InvalidArgument);
    }
    let path_len = reply.words[4] as usize;
    if path_len == 0 || path_len > PATH_MAX {
        return Err(rt::Error::InvalidArgument);
    }
    let mut hit = GrepHit {
        next_cursor: reply.words[1] as usize,
        line: reply.words[2],
        truncated,
        oversize_skipped,
        path: [0; PATH_MAX],
        path_len,
    };
    unpack_bytes(
        &reply.words[5..reply.word_count as usize],
        path_len,
        &mut hit.path,
    )?;
    Ok(Some(hit))
}

pub(crate) fn storage_grep<const PATH_MAX: usize>(
    storage_handle: rt::Handle,
    cursor: usize,
    scope: &[u8],
    needle: &GrepNeedle,
) -> rt::Result<Option<GrepHit<PATH_MAX>>> {
    let mut request = grep_request(cursor, scope, needle)?;
    let response = channel_call(storage_handle, &mut request)?;
    grep_parse_reply::<PATH_MAX>(&response)
}

/// Replaces the visible rows with content hits from the current directory
/// subtree, walking the service cursor until the reply reports End or the
/// entry table is full. Truncation/oversize flags accumulate across
/// replies so the renderer can stay honest about unshown matches.
pub(crate) fn reload_content_search(
    state: &mut crate::state::ExplorerState,
    storage_handle: rt::Handle,
) -> rt::Result<()> {
    crate::navigation::reset_listing(state);
    state.content_hit_line = [0; crate::state::MAX_ENTRIES];
    state.content_truncated = false;
    state.content_oversize = false;

    let needle = match GrepNeedle::from_bytes(&state.search_query[..state.search_query_len]) {
        Some(needle) => needle,
        None => {
            state.view_mode = crate::state::ViewMode::Directory;
            return crate::navigation::reload_directory(state);
        }
    };
    let mut scope = [0u8; crate::state::MAX_STORAGE_PATH];
    scope[..state.current_path_len].copy_from_slice(&state.current_path[..state.current_path_len]);
    let scope = &scope[..state.current_path_len];

    let mut cursor = 0usize;
    while state.entry_count < state.entries.len() {
        match storage_grep::<{ crate::state::MAX_STORAGE_PATH }>(
            storage_handle,
            cursor,
            scope,
            &needle,
        )? {
            Some(hit) => {
                state.entries[state.entry_count].kind = crate::state::EntryKind::File;
                state.entries[state.entry_count].path_len = hit.path_len;
                state.entries[state.entry_count].path[..hit.path_len]
                    .copy_from_slice(&hit.path[..hit.path_len]);
                state.content_hit_line[state.entry_count] = hit.line;
                state.entry_count += 1;
                state.content_truncated |= hit.truncated;
                state.content_oversize |= hit.oversize_skipped;
                if hit.next_cursor <= cursor {
                    break;
                }
                cursor = hit.next_cursor;
            }
            None => break,
        }
    }
    crate::navigation::clamp_view(state);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::MAX_STORAGE_PATH;

    fn packed_words(bytes: &[u8]) -> Vec<u64> {
        let mut words = [0u64; rt::IPC_MAX_WORDS];
        let used = pack_bytes(bytes, &mut words).expect("pack fits");
        words[..used as usize].to_vec()
    }

    #[test]
    fn grep_request_parity_header_and_packed_payload() {
        let needle = GrepNeedle::from_bytes(b"todo").expect("valid needle");
        let request = grep_request(3, b"docs/api/", &needle).expect("packs");
        assert_eq!(request.tag, GREP_REQUEST_TAG);
        assert_eq!(request.words[0], 3);
        assert_eq!(request.words[1], 9);
        assert_eq!(request.words[2], 4);
        assert_eq!(request.words[3], 0, "file cap default");
        assert_eq!(request.words[4], 0, "result cap default");
        assert_eq!(request.word_count as usize, GREP_HEADER_WORDS + 2 + 1);

        let mut scope = [0u8; 16];
        unpack_bytes(
            &request.words[GREP_HEADER_WORDS..GREP_HEADER_WORDS + 2],
            9,
            &mut scope,
        )
        .expect("scope decodes");
        assert_eq!(&scope[..9], b"docs/api/");
        let mut decoded = [0u8; 8];
        unpack_bytes(
            &request.words[GREP_HEADER_WORDS + 2..GREP_HEADER_WORDS + 3],
            4,
            &mut decoded,
        )
        .expect("needle decodes");
        assert_eq!(&decoded[..4], b"todo");
    }

    #[test]
    fn grep_request_rejects_empty_oversize_and_path_overflow() {
        assert!(GrepNeedle::from_bytes(b"").is_none());
        assert!(GrepNeedle::from_bytes(&[b'x'; GREP_NEEDLE_MAX + 1]).is_none());
        assert!(GrepNeedle::from_bytes(b"two\nlines").is_none());
        assert!(GrepNeedle::from_bytes(&[b'x'; GREP_NEEDLE_MAX]).is_some());
        let needle = GrepNeedle::from_bytes(&[b'a'; GREP_NEEDLE_MAX]).expect("valid");
        let max_scope = [b's'; rt::IPC_MAX_WORDS * 8];
        assert_eq!(
            grep_request(0, &max_scope, &needle),
            Err(rt::Error::BufferTooSmall)
        );
    }

    #[test]
    fn grep_reply_decodes_hit_path_line_and_flags() {
        let mut reply = RawMessage::empty(GREP_REPLY_TAG);
        reply.word_count = 5;
        reply.words[0] = StorageStatus::Ok as u32 as u64;
        reply.words[1] = 4;
        reply.words[2] = 17;
        reply.words[3] = GREP_FLAG_TRUNCATED | GREP_FLAG_OVERSIZE_SKIPPED;
        reply.words[4] = b"docs/notes.md".len() as u64;
        let path_words = packed_words(b"docs/notes.md");
        reply.words[5..5 + path_words.len()].copy_from_slice(&path_words);
        reply.word_count += path_words.len() as u32;

        let hit = grep_parse_reply::<MAX_STORAGE_PATH>(&reply)
            .expect("decodes")
            .expect("hit present");
        assert_eq!(hit.next_cursor, 4);
        assert_eq!(hit.line, 17);
        assert!(hit.truncated);
        assert!(hit.oversize_skipped);
        assert_eq!(hit.path_as_bytes(), b"docs/notes.md");
    }

    #[test]
    fn grep_reply_end_carries_flags_without_hit() {
        let mut reply = RawMessage::empty(GREP_REPLY_TAG);
        reply.word_count = 5;
        reply.words[0] = StorageStatus::End as u32 as u64;
        reply.words[1] = 2;
        reply.words[3] = GREP_FLAG_TRUNCATED;
        let decoded = grep_parse_reply::<MAX_STORAGE_PATH>(&reply).expect("decodes");
        assert!(decoded.is_none());
    }

    #[test]
    fn grep_reply_rejects_wrong_tag_short_and_bad_path() {
        let mut reply = RawMessage::empty(GREP_REPLY_TAG);
        reply.word_count = 5;
        reply.words[0] = StorageStatus::Ok as u32 as u64;
        assert!(grep_parse_reply::<MAX_STORAGE_PATH>(&reply).is_err());

        reply.tag = 0x521;
        assert!(grep_parse_reply::<MAX_STORAGE_PATH>(&reply).is_err());

        reply.tag = GREP_REPLY_TAG;
        reply.word_count = 4;
        assert!(grep_parse_reply::<MAX_STORAGE_PATH>(&reply).is_err());

        reply.word_count = 5;
        reply.words[0] = StorageStatus::Ok as u32 as u64;
        reply.words[4] = 0;
        assert!(grep_parse_reply::<MAX_STORAGE_PATH>(&reply).is_err());
    }
}

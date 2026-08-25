use serviceos_userspace_runtime as rt;

use crate::{OVERLAY_RESULT_MAX, PaletteAction};

pub(crate) const DOC_HITS_MAX: usize = 4;
pub(crate) const DOC_PATH_MAX: usize = 88;
pub(crate) const DOC_ACTION_RESERVE_MAX: usize = 2;
const SEARCH_REQUEST_TAG: u32 = 0x521;
const SEARCH_REPLY_TAG: u32 = 0x522;
const GREP_REQUEST_TAG: u32 = 0x523;
const GREP_REPLY_TAG: u32 = 0x524;
const GREP_NEEDLE_MAX: usize = 32;
const STATUS_OK: u64 = 0;

const KIND_FILE: u64 = 0;
const KIND_DIRECTORY: u64 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PaletteQueryMode<'a> {
    Apps,
    Names(&'a str),
    Content(&'a str),
}

/// One document/content hit from storage-service search (0x522) or grep
/// (0x524) replies, kept as a fixed-size copyable value so it can ride in
/// palette result slots without allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DocHit {
    pub(crate) path: [u8; DOC_PATH_MAX],
    pub(crate) path_len: usize,
    pub(crate) kind: u64,
    pub(crate) line: u32,
}

impl DocHit {
    pub(crate) fn path_str(&self) -> &str {
        core::str::from_utf8(&self.path[..self.path_len.min(self.path.len())]).unwrap_or("")
    }
}

/// A command-palette result row: either a ranked app/system action or a
/// storage document hit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PaletteEntry {
    Action(PaletteAction),
    Doc(DocHit),
}

/// Leading `/` switches the palette into grep-backed content mode; any other
/// query of two or more bytes searches the name index; shorter queries stay
/// app/action-only.
pub(crate) fn palette_query_mode<'a>(query: &'a str) -> PaletteQueryMode<'a> {
    if let Some(needle) = query.strip_prefix('/') {
        return PaletteQueryMode::Content(needle);
    }
    if query.len() >= 2 {
        return PaletteQueryMode::Names(query);
    }
    PaletteQueryMode::Apps
}

/// Merges ranked app/action results with document hits so every present hit
/// ranks strictly below the app/action block while still getting guaranteed
/// slots (`DOC_ACTION_RESERVE_MAX`) even on broad queries, honoring
/// `OVERLAY_RESULT_MAX` overall.
pub(crate) fn merge_palette_entries(
    actions: &[PaletteAction],
    doc_hits: &[DocHit],
    results: &mut [PaletteEntry; OVERLAY_RESULT_MAX],
) -> usize {
    let doc_hits = &doc_hits[..doc_hits.len().min(DOC_HITS_MAX)];
    let mut out = 0usize;
    let action_cap = if doc_hits.is_empty() {
        OVERLAY_RESULT_MAX
    } else {
        OVERLAY_RESULT_MAX.saturating_sub(DOC_ACTION_RESERVE_MAX.min(doc_hits.len()))
    };
    for action in actions.iter().take(action_cap) {
        if out == OVERLAY_RESULT_MAX {
            break;
        }
        results[out] = PaletteEntry::Action(*action);
        out += 1;
    }
    for hit in doc_hits {
        if out == OVERLAY_RESULT_MAX {
            break;
        }
        results[out] = PaletteEntry::Doc(*hit);
        out += 1;
    }
    out
}

pub(crate) fn doc_kind_icon(kind: u64) -> &'static str {
    match kind {
        KIND_DIRECTORY => "[/]",
        KIND_FILE => "[]",
        _ => "[?]",
    }
}

fn decode_doc_hit(payload: &[u64], path_len: u64, kind: u64, line: u32) -> Option<DocHit> {
    let path_len = path_len as usize;
    if path_len == 0 || path_len > DOC_PATH_MAX || path_len > payload.len() * 8 {
        return None;
    }
    let mut hit = DocHit {
        path: [0; DOC_PATH_MAX],
        path_len,
        kind,
        line,
    };
    rt::unpack_bytes(payload, path_len, &mut hit.path).ok()?;
    Some(hit)
}

/// Parses one cursor reply of the storage name-search contract (tag 0x522:
/// status, next-cursor, kind, path-len, packed path). `None` covers both the
/// end-of-stream and degraded replies, which callers treat identically as
/// "stop collecting".
pub(crate) fn parse_search_reply(reply: &rt::RawMessage) -> Option<DocHit> {
    if reply.tag != SEARCH_REPLY_TAG || reply.word_count < 4 {
        return None;
    }
    if reply.words[0] != STATUS_OK {
        return None;
    }
    decode_doc_hit(&reply.words[4..], reply.words[3], reply.words[2], 0)
}

/// Parses one cursor reply of the storage grep contract (tag 0x524: status,
/// next-cursor, line, flags, path-len, packed path).
pub(crate) fn parse_grep_reply(reply: &rt::RawMessage) -> Option<DocHit> {
    if reply.tag != GREP_REPLY_TAG || reply.word_count < 5 {
        return None;
    }
    if reply.words[0] != STATUS_OK {
        return None;
    }
    decode_doc_hit(
        &reply.words[5..],
        reply.words[4],
        KIND_FILE,
        reply.words[2].min(u32::MAX as u64) as u32,
    )
}

fn build_search_request(cursor: usize, tokens: &[u8]) -> Option<rt::RawMessage> {
    if tokens.is_empty() {
        return None;
    }
    let mut request = rt::RawMessage::empty(SEARCH_REQUEST_TAG);
    request.word_count = 6;
    request.words[0] = cursor as u64;
    request.words[1] = 0;
    request.words[2] = 0;
    request.words[3] = 0;
    request.words[4] = 0;
    request.words[5] = tokens.len() as u64;
    request.word_count += rt::pack_bytes(tokens, &mut request.words[6..]).ok()?;
    Some(request)
}

fn build_grep_request(cursor: usize, needle: &[u8]) -> Option<rt::RawMessage> {
    if needle.is_empty() || needle.len() > GREP_NEEDLE_MAX {
        return None;
    }
    let mut request = rt::RawMessage::empty(GREP_REQUEST_TAG);
    request.word_count = 5;
    request.words[0] = cursor as u64;
    request.words[1] = 0;
    request.words[2] = needle.len() as u64;
    request.words[3] = 0;
    request.words[4] = 0;
    request.word_count += rt::pack_bytes(needle, &mut request.words[5..]).ok()?;
    Some(request)
}

fn collect_name_hits(
    storage: rt::Handle,
    tokens: &[u8],
    hits: &mut [DocHit; DOC_HITS_MAX],
    hits_len: &mut usize,
) {
    for cursor in 0..DOC_HITS_MAX {
        if *hits_len >= DOC_HITS_MAX {
            return;
        }
        let Some(mut request) = build_search_request(cursor, tokens) else {
            return;
        };
        match rt::channel_call(storage, &mut request) {
            Ok(reply) => match parse_search_reply(&reply) {
                Some(hit) => {
                    hits[*hits_len] = hit;
                    *hits_len += 1;
                }
                None => return,
            },
            Err(_) => return,
        }
    }
}

fn collect_content_hits(
    storage: rt::Handle,
    needle: &[u8],
    hits: &mut [DocHit; DOC_HITS_MAX],
    hits_len: &mut usize,
) {
    for cursor in 0..DOC_HITS_MAX {
        if *hits_len >= DOC_HITS_MAX {
            return;
        }
        let Some(mut request) = build_grep_request(cursor, needle) else {
            return;
        };
        match rt::channel_call(storage, &mut request) {
            Ok(reply) => match parse_grep_reply(&reply) {
                Some(hit) => {
                    hits[*hits_len] = hit;
                    *hits_len += 1;
                }
                None => return,
            },
            Err(_) => return,
        }
    }
}

/// Re-queries the storage-service index for the current palette query. Any
/// degraded condition (no storage handle, transport error, malformed or
/// end-of-stream reply, empty needle) leaves zero document hits rather than
/// stale ones.
pub(crate) fn refresh_doc_hits(state: &mut crate::DesktopState) {
    state.doc_hits_len = 0;
    if state.storage_handle == rt::INVALID_HANDLE {
        return;
    }
    let mut query_bytes = [0u8; crate::PALETTE_QUERY_MAX];
    let query_len = state.palette_query_len.min(query_bytes.len());
    query_bytes[..query_len].copy_from_slice(&state.palette_query[..query_len]);
    let Ok(query) = core::str::from_utf8(&query_bytes[..query_len]) else {
        return;
    };
    let storage = state.storage_handle;
    match palette_query_mode(query) {
        PaletteQueryMode::Apps => {}
        PaletteQueryMode::Names(tokens) => {
            collect_name_hits(
                storage,
                tokens.as_bytes(),
                &mut state.doc_hits,
                &mut state.doc_hits_len,
            );
        }
        PaletteQueryMode::Content(needle) => {
            if !needle.is_empty() {
                collect_content_hits(
                    storage,
                    needle.as_bytes(),
                    &mut state.doc_hits,
                    &mut state.doc_hits_len,
                );
            }
        }
    }
}

pub(crate) fn clear_doc_hits(state: &mut crate::DesktopState) {
    state.doc_hits_len = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PaletteAction;
    use serviceos_userspace_runtime::DesktopAppId;

    fn hit(path: &[u8], kind: u64, line: u32) -> DocHit {
        let mut value = DocHit {
            path: [0; DOC_PATH_MAX],
            path_len: path.len(),
            kind,
            line,
        };
        value.path[..path.len()].copy_from_slice(path);
        value
    }

    fn mode_of(query: &str) -> PaletteQueryMode<'_> {
        palette_query_mode(query)
    }

    #[test]
    fn short_or_empty_queries_stay_app_only() {
        assert!(matches!(mode_of(""), PaletteQueryMode::Apps));
        assert!(matches!(mode_of("t"), PaletteQueryMode::Apps));
        assert!(matches!(mode_of("te"), PaletteQueryMode::Names("te")));
        assert!(matches!(mode_of("open "), PaletteQueryMode::Names("open ")));
    }

    #[test]
    fn leading_slash_switches_to_content_mode() {
        assert!(matches!(mode_of("/wav"), PaletteQueryMode::Content("wav")));
        assert!(matches!(
            mode_of("/sample rate"),
            PaletteQueryMode::Content("sample rate")
        ));
        match mode_of("/") {
            PaletteQueryMode::Content(needle) => assert!(needle.is_empty()),
            other => panic!("bare slash must select content mode, got {other:?}"),
        }
    }

    #[test]
    fn documents_rank_below_all_app_and_action_matches() {
        let actions = [
            PaletteAction::Launch(DesktopAppId::Terminal),
            PaletteAction::ToggleMedia,
            PaletteAction::FocusNext,
        ];
        let docs = [hit(b"docs/readme.txt", KIND_FILE, 0), hit(b"notes", KIND_DIRECTORY, 0)];
        let mut results = [PaletteEntry::Action(PaletteAction::FocusNext); OVERLAY_RESULT_MAX];
        let count = merge_palette_entries(&actions, &docs, &mut results);
        assert_eq!(count, 5);
        assert_eq!(results[0], PaletteEntry::Action(actions[0]));
        assert_eq!(results[1], PaletteEntry::Action(actions[1]));
        assert_eq!(results[2], PaletteEntry::Action(actions[2]));
        assert_eq!(results[3], PaletteEntry::Doc(docs[0]));
        assert_eq!(results[4], PaletteEntry::Doc(docs[1]));
    }

    #[test]
    fn full_action_list_reserves_document_slots() {
        let actions = [
            PaletteAction::Launch(DesktopAppId::Settings),
            PaletteAction::Launch(DesktopAppId::Files),
            PaletteAction::Launch(DesktopAppId::Monitor),
            PaletteAction::Launch(DesktopAppId::Terminal),
            PaletteAction::Launch(DesktopAppId::SoftwareCenter),
            PaletteAction::ShowNotifications,
            PaletteAction::LockSession,
        ];
        let docs = [
            hit(b"a", KIND_FILE, 0),
            hit(b"b", KIND_FILE, 0),
            hit(b"c", KIND_FILE, 0),
            hit(b"d", KIND_FILE, 0),
        ];
        let mut results = [PaletteEntry::Action(PaletteAction::FocusNext); OVERLAY_RESULT_MAX];
        let count = merge_palette_entries(&actions, &docs, &mut results);
        assert_eq!(count, OVERLAY_RESULT_MAX);
        let action_rows = results[..count - 2]
            .iter()
            .filter(|entry| matches!(entry, PaletteEntry::Action(_)))
            .count();
        assert_eq!(
            action_rows,
            OVERLAY_RESULT_MAX - 2,
            "broad queries must still reserve document slots"
        );
        assert_eq!(
            results[OVERLAY_RESULT_MAX - 2],
            PaletteEntry::Doc(docs[0])
        );
        assert_eq!(
            results[OVERLAY_RESULT_MAX - 1],
            PaletteEntry::Doc(docs[1])
        );
    }

    #[test]
    fn empty_doc_list_keeps_full_action_capacity() {
        let actions = [
            PaletteAction::Launch(DesktopAppId::Settings),
            PaletteAction::LockSession,
        ];
        let mut results = [PaletteEntry::Action(PaletteAction::FocusNext); OVERLAY_RESULT_MAX];
        let count = merge_palette_entries(&actions, &[], &mut results);
        assert_eq!(count, 2);
        assert!(matches!(results[0], PaletteEntry::Action(_)));
    }

    #[test]
    fn search_reply_parses_ok_end_and_degraded_shapes() {
        // Ok reply: status, cursor, kind, path-len, packed path.
        let mut words = [0u64; rt::IPC_MAX_WORDS];
        words[0] = STATUS_OK;
        words[1] = 1;
        words[2] = KIND_FILE;
        words[3] = b"boot/notes.txt".len() as u64;
        words[4..12].copy_from_slice(&{
            let mut payload = [0u64; 8];
            assert!(rt::pack_bytes(b"boot/notes.txt", &mut payload).is_ok());
            payload
        });
        let mut reply = rt::RawMessage::empty(SEARCH_REPLY_TAG);
        reply.word_count = 4;
        reply.words = words;
        reply.word_count += 1; // one payload word used
        let parsed = parse_search_reply(&reply).expect("ok reply must parse");
        assert_eq!(parsed.path_str(), "boot/notes.txt");
        assert_eq!(parsed.kind, KIND_FILE);
        assert_eq!(parsed.line, 0);

        reply.words[0] = rt::StorageStatus::End as u64;
        assert!(parse_search_reply(&reply).is_none(), "end must yield none");

        reply.tag = 0x999;
        reply.words[0] = STATUS_OK;
        assert!(parse_search_reply(&reply).is_none(), "wrong tag is degraded");

        reply.tag = SEARCH_REPLY_TAG;
        reply.word_count = 2;
        assert!(
            parse_search_reply(&reply).is_none(),
            "truncated reply is degraded"
        );
    }

    #[test]
    fn grep_reply_carries_line_numbers_and_rejects_bad_paths() {
        let mut payload = [0u64; 4];
        assert!(rt::pack_bytes(b"state/log", &mut payload).is_ok());
        let mut reply = rt::RawMessage::empty(GREP_REPLY_TAG);
        reply.word_count = 5 + 1;
        reply.words[0] = STATUS_OK;
        reply.words[1] = 1;
        reply.words[2] = 42;
        reply.words[3] = 0;
        reply.words[4] = b"state/log".len() as u64;
        reply.words[5..9].copy_from_slice(&payload);
        let parsed = parse_grep_reply(&reply).expect("grep ok reply must parse");
        assert_eq!(parsed.path_str(), "state/log");
        assert_eq!(parsed.line, 42);

        reply.words[0] = rt::StorageStatus::End as u64;
        assert!(parse_grep_reply(&reply).is_none());

        reply.words[0] = STATUS_OK;
        reply.words[4] = (DOC_PATH_MAX + 1) as u64;
        assert!(
            parse_grep_reply(&reply).is_none(),
            "over-length paths are rejected instead of truncated"
        );
    }

    #[test]
    fn request_builders_shape_storage_contracts() {
        let request = build_search_request(3, b"note").expect("search request builds");
        assert_eq!(request.tag, SEARCH_REQUEST_TAG);
        assert_eq!(request.words[0], 3);
        assert_eq!(request.words[1], 0, "empty scope means whole subtree");
        assert_eq!(request.words[5], 4);
        assert_eq!(request.word_count, 6 + 1);

        assert!(build_search_request(0, b"").is_none());

        let request = build_grep_request(0, b"wav").expect("grep request builds");
        assert_eq!(request.tag, GREP_REQUEST_TAG);
        assert_eq!(request.words[2], 3);
        assert_eq!(request.words[3], 0, "default per-file byte cap");
        assert_eq!(request.words[4], 0, "default result cap");

        assert!(build_grep_request(0, b"").is_none(), "empty needle never sent");
        assert!(
            build_grep_request(0, &[b'x'; GREP_NEEDLE_MAX + 1]).is_none(),
            "over-length needles never sent"
        );
    }

    #[test]
    fn doc_hit_paths_roundtrip_and_guard_invalid_utf8() {
        let good = hit(b"state/files-app/ring.cfg", KIND_FILE, 0);
        assert_eq!(good.path_str(), "state/files-app/ring.cfg");
        let mut bad = hit(&[0xFF, b'a'], KIND_FILE, 0);
        bad.path[0] = 0xFF;
        assert_eq!(bad.path_str(), "");
    }
}

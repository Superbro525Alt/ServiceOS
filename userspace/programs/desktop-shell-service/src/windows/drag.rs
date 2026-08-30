use super::*;

use crate::HitTarget;

/// Reserved control prefix on the existing notify channel. A NotifyRequest
/// whose text starts with this marker is an inter-app content intent
/// (drag payload or open-with handoff), never a user-visible notification.
pub(crate) const CONTENT_INTENT_MARKER: u8 = 0x02;

/// Hint byte appended after the marker: b'0' means "undecided target"
/// (a live drag; the shell picks the target at pointer-up); b'1'..=b'9'
/// name a DesktopAppId for immediate delivery (open-with).
pub(crate) const HINT_UNDECIDED: u8 = b'0';

pub(crate) const CONTENT_PATH_MAX: usize = 96;
/// Safety net: a drag whose pointer-up never arrives expires on its own.
pub(crate) const CONTENT_DRAG_TIMEOUT_TICKS: u64 = 900;
/// Maximum notify text accepted by the desktop contract.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const CONTENT_PAYLOAD_MAX: usize = 96;
/// Bounded multi-file drag fan-out (payload budget caps practical N at 4).
pub(crate) const CONTENT_DRAG_MAX_FILES: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ContentDrag {
    pub(crate) path_len: usize,
    pub(crate) path: [u8; CONTENT_PATH_MAX],
    /// Number of files in the drag (1 = single legacy payload).
    pub(crate) count: usize,
    pub(crate) deadline: u64,
}

impl ContentDrag {
    pub(crate) fn expired(&self, now: u64) -> bool {
        now >= self.deadline
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ContentIntent {
    /// b'0' = live drag from files-app; otherwise ASCII digit of DesktopAppId.
    pub(crate) hint: u8,
    pub(crate) path_len: usize,
    pub(crate) path: [u8; CONTENT_PATH_MAX],
    /// First entry carries the path above; `count` names the fan-out
    /// (1 for legacy single-file payloads).
    pub(crate) count: usize,
}

impl ContentIntent {
    pub(crate) fn target_app(&self) -> Option<DesktopAppId> {
        if self.hint == HINT_UNDECIDED {
            return None;
        }
        app_from_hint(self.hint)
    }

    pub(crate) fn path_bytes(&self) -> &[u8] {
        &self.path[..self.path_len]
    }
}

pub(crate) fn app_from_hint(hint: u8) -> Option<DesktopAppId> {
    if !hint.is_ascii_digit() {
        return None;
    }
    match (hint - b'0') as u32 {
        x if x == DesktopAppId::Settings as u32 => Some(DesktopAppId::Settings),
        x if x == DesktopAppId::Files as u32 => Some(DesktopAppId::Files),
        x if x == DesktopAppId::Monitor as u32 => Some(DesktopAppId::Monitor),
        x if x == DesktopAppId::Terminal as u32 => Some(DesktopAppId::Terminal),
        x if x == DesktopAppId::SoftwareCenter as u32 => Some(DesktopAppId::SoftwareCenter),
        x if x == DesktopAppId::Media as u32 => Some(DesktopAppId::Media),
        _ => None,
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn hint_for_app(app_id: DesktopAppId) -> u8 {
    b'0' + app_id as u32 as u8
}

/// Parses a content-intent payload carried as notify-channel text:
/// `[marker][hint][path bytes]` (legacy single file) or, for undecided
/// multi-file drags, `[marker][b'0'][count '2'..='4'][2-digit len][path]...`
/// repeated exactly `count` times. Multi framing is only tried when the
/// count byte and the full structure validate; anything else falls back to
/// the legacy parse, keeping single-file payloads byte-compatible.
pub(crate) fn parse_content_intent(text: &[u8]) -> Option<ContentIntent> {
    if text.len() < 3 || text[0] != CONTENT_INTENT_MARKER {
        return None;
    }
    let hint = text[1];
    if hint == HINT_UNDECIDED && text.len() >= 4 && (b'2'..=b'4').contains(&text[2]) {
        if let Some((path, path_len, count)) = parse_multi_paths(&text[2..]) {
            let mut stored = [0u8; CONTENT_PATH_MAX];
            stored[..path_len].copy_from_slice(&path[..path_len]);
            return Some(ContentIntent {
                hint,
                path_len,
                path: stored,
                count,
            });
        }
    }
    let raw_path = &text[2..];
    // Whole payload must fit the notify text budget: marker + hint + path.
    if raw_path.len() > CONTENT_PAYLOAD_MAX - 2
        || !raw_path.iter().all(|byte| byte.is_ascii_graphic())
    {
        return None;
    }
    if app_from_hint(hint).is_none() && hint != HINT_UNDECIDED {
        return None;
    }
    let mut path = [0u8; CONTENT_PATH_MAX];
    path[..raw_path.len()].copy_from_slice(raw_path);
    Some(ContentIntent {
        hint,
        path_len: raw_path.len(),
        path,
        count: 1,
    })
}

/// Structural multi-path parse of `[count][len_hi][len_lo][path]...`:
/// exactly `count` segments, each length a two-digit decimal (1..=40),
/// every byte ASCII graphic, total length consumed exactly.
fn parse_multi_paths(body: &[u8]) -> Option<([u8; CONTENT_PATH_MAX], usize, usize)> {
    let count = (body[0] - b'0') as usize;
    let mut cursor = 1usize;
    let mut first_len = 0usize;
    let mut first = [0u8; CONTENT_PATH_MAX];
    for segment in 0..count {
        let len_hi = (body.get(cursor).copied()? as char).to_digit(10)?;
        let len_lo = (body.get(cursor + 1).copied()? as char).to_digit(10)?;
        let len = (len_hi * 10 + len_lo) as usize;
        if len == 0 || len > 40 {
            return None;
        }
        let path = body.get(cursor + 2..cursor + 2 + len)?;
        if !path.iter().all(|byte| byte.is_ascii_graphic()) {
            return None;
        }
        if segment == 0 {
            first_len = len;
            first[..len].copy_from_slice(path);
        }
        cursor += 2 + len;
    }
    if cursor != body.len() {
        return None;
    }
    Some((first, first_len, count))
}

/// Encodes the payload sent by files-app over the notify channel.
/// The sender crate mirrors this framing; kept here as the wire-format
/// reference exercised by unit tests.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn encode_content_intent(hint: u8, path: &[u8]) -> Option<[u8; CONTENT_PAYLOAD_MAX]> {
    let mut payload = [0u8; CONTENT_PAYLOAD_MAX];
    if 2 + path.len() > payload.len()
        || path.is_empty()
        || !path.iter().all(|byte| byte.is_ascii_graphic())
    {
        return None;
    }
    payload[0] = CONTENT_INTENT_MARKER;
    payload[1] = hint;
    payload[2..2 + path.len()].copy_from_slice(path);
    Some(payload)
}

/// Encodes a multi-file undecided drag: `[marker][b'0'][count][len][path]...`
/// with two-digit decimal lengths. Wire parity with files-app bridge.rs.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn encode_content_intent_multi(
    paths: &[&[u8]],
) -> Option<([u8; CONTENT_PAYLOAD_MAX], usize)> {
    if paths.len() < 2 || paths.len() > CONTENT_DRAG_MAX_FILES {
        return None;
    }
    let mut payload = [0u8; CONTENT_PAYLOAD_MAX];
    payload[0] = CONTENT_INTENT_MARKER;
    payload[1] = HINT_UNDECIDED;
    payload[2] = b'0' + paths.len() as u8;
    let mut len = 3usize;
    for path in paths {
        if path.is_empty() || path.len() > 40 || len + 2 + path.len() > payload.len() {
            return None;
        }
        if !path.iter().all(|byte| byte.is_ascii_graphic()) {
            return None;
        }
        payload[len] = b'0' + (path.len() / 10) as u8;
        payload[len + 1] = b'0' + (path.len() % 10) as u8;
        payload[len + 2..len + 2 + path.len()].copy_from_slice(path);
        len += 2 + path.len();
    }
    Some((payload, len))
}

/// What the shell does with an armed drag when the pointer comes up:
/// deliver to the hovered launcher icon, reveal on bare canvas, or silently
/// cancel when the pointer lands back over any window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DropDecision {
    Deliver(DesktopAppId),
    Cancel,
}

pub(crate) fn drop_decision(target: &HitTarget) -> DropDecision {
    match target {
        HitTarget::Launcher(app_id) => DropDecision::Deliver(*app_id),
        HitTarget::Background => DropDecision::Deliver(DesktopAppId::Files),
        _ => DropDecision::Cancel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent(hint: u8, path: &[u8]) -> Option<ContentIntent> {
        let payload = encode_content_intent(hint, path)?;
        parse_content_intent(&payload[..2 + path.len()])
    }

    #[test]
    fn roundtrips_undecided_drag_payload() {
        let parsed = intent(HINT_UNDECIDED, b"home/notes.txt").expect("valid payload");
        assert_eq!(parsed.hint, HINT_UNDECIDED);
        assert_eq!(parsed.path_bytes(), b"home/notes.txt");
        assert_eq!(parsed.target_app(), None);
    }

    #[test]
    fn roundtrips_explicit_open_with_hint() {
        let parsed = intent(b'4', b"home/build.log").expect("valid payload");
        assert_eq!(parsed.target_app(), Some(DesktopAppId::Terminal));
        assert_eq!(parsed.hint, b'4');
    }

    #[test]
    fn rejects_missing_marker_short_payload_and_bad_hint() {
        assert!(parse_content_intent(b"home/x.txt").is_none());
        assert!(parse_content_intent(&[CONTENT_INTENT_MARKER]).is_none());
        assert!(parse_content_intent(&[CONTENT_INTENT_MARKER, b'0']).is_none());
        assert!(intent(b'x', b"home/x.txt").is_none());
        assert!(intent(b':', b"home/x.txt").is_none());
    }

    #[test]
    fn rejects_non_graphic_and_empty_paths() {
        assert!(intent(HINT_UNDECIDED, b"").is_none());
        assert!(intent(HINT_UNDECIDED, b"home/a b.txt").is_none());
        assert!(encode_content_intent(HINT_UNDECIDED, &[0xff]).is_none());
    }

    #[test]
    fn rejects_oversize_paths_but_accepts_full_budget() {
        let max_path = [b'a'; CONTENT_PAYLOAD_MAX - 2];
        assert!(intent(HINT_UNDECIDED, &max_path).is_some());
        let oversized = [b'a'; CONTENT_PAYLOAD_MAX - 1];
        assert!(intent(HINT_UNDECIDED, &oversized).is_none());
        let mut text = [0u8; CONTENT_PAYLOAD_MAX + 1];
        text[0] = CONTENT_INTENT_MARKER;
        text[1] = HINT_UNDECIDED;
        text[2..].copy_from_slice(&oversized);
        assert!(parse_content_intent(&text).is_none());
    }

    #[test]
    fn hint_mapping_covers_every_app_and_rejects_unknown_digits() {
        for app in [
            DesktopAppId::Settings,
            DesktopAppId::Files,
            DesktopAppId::Monitor,
            DesktopAppId::Terminal,
            DesktopAppId::SoftwareCenter,
            DesktopAppId::Media,
        ] {
            assert_eq!(app_from_hint(hint_for_app(app)), Some(app));
        }
        assert_eq!(app_from_hint(b'7'), None);
        assert_eq!(app_from_hint(b'9'), None);
    }

    #[test]
    fn drop_decision_routes_launcher_canvas_and_windows() {
        assert_eq!(
            drop_decision(&HitTarget::Launcher(DesktopAppId::Terminal)),
            DropDecision::Deliver(DesktopAppId::Terminal)
        );
        assert_eq!(
            drop_decision(&HitTarget::Background),
            DropDecision::Deliver(DesktopAppId::Files)
        );
        assert_eq!(
            drop_decision(&HitTarget::LauncherDoc(0)),
            DropDecision::Cancel,
            "document rows take clicks, not drops; dropping stays a cancel"
        );
        assert_eq!(
            drop_decision(&HitTarget::WindowContent(DesktopAppId::Monitor)),
            DropDecision::Cancel
        );
        assert_eq!(
            drop_decision(&HitTarget::WindowMove {
                app_id: DesktopAppId::Files,
                grab_offset_x: 0,
                grab_offset_y: 0,
            }),
            DropDecision::Cancel
        );
    }

    #[test]
    fn drag_expires_only_after_deadline_tick() {
        let drag = ContentDrag {
            path_len: 3,
            path: [b'a'; CONTENT_PATH_MAX],
            count: 1,
            deadline: 100,
        };
        assert!(!drag.expired(99));
        assert!(drag.expired(100));
        assert!(drag.expired(150));
    }

    #[test]
    fn multi_payload_roundtrips_first_path_and_count() {
        let (payload, len) =
            encode_content_intent_multi(&[b"home/a.txt", b"home/b.txt", b"home/c.txt"])
                .expect("valid multi payload");
        let parsed = parse_content_intent(&payload[..len]).expect("parses as multi");
        assert_eq!(parsed.hint, HINT_UNDECIDED);
        assert_eq!(parsed.count, 3);
        assert_eq!(parsed.path_bytes(), b"home/a.txt");
        assert_eq!(parsed.target_app(), None);
    }

    #[test]
    fn multi_parse_rejects_broken_framing_and_falls_back_to_legacy() {
        // Count says 2 but the second segment length is not a digit:
        // multi framing fails, the bytes fall back to a legacy path.
        let text = [
            CONTENT_INTENT_MARKER,
            HINT_UNDECIDED,
            b'2',
            b'0',
            b'2',
            b'h',
            b'o',
        ];
        let parsed = parse_content_intent(&text).expect("legacy fallback");
        assert_eq!(parsed.count, 1);
        assert_eq!(parsed.path_bytes(), b"202ho");
        // Zero-length and over-length segments fail multi framing and the
        // graphic bytes fall back to a legacy single-path parse.
        let broken = [CONTENT_INTENT_MARKER, HINT_UNDECIDED, b'2', b'0', b'0'];
        let parsed = parse_content_intent(&broken).expect("legacy fallback");
        assert_eq!(parsed.count, 1);
        assert_eq!(parsed.path_bytes(), b"200");
        let mut long = [0u8; 45];
        long[0] = CONTENT_INTENT_MARKER;
        long[1] = HINT_UNDECIDED;
        long[2] = b'2';
        long[3] = b'4';
        long[4] = b'1';
        long[5..45].copy_from_slice(&[b'a'; 40]);
        let parsed = parse_content_intent(&long).expect("legacy fallback");
        assert_eq!(parsed.count, 1);
        assert_eq!(parsed.path_bytes(), &long[2..]);
        // Non-graphic tails reject outright (legacy rejects them too).
        let payload = encode_content_intent(HINT_UNDECIDED, b"203x").expect("valid");
        assert!(parse_content_intent(&payload).is_none());
    }

    #[test]
    fn single_file_payload_stays_byte_compatible() {
        let payload = encode_content_intent(HINT_UNDECIDED, b"203x").expect("valid");
        let parsed = parse_content_intent(&payload[..6]).expect("parses");
        assert_eq!(parsed.count, 1);
        assert_eq!(parsed.path_bytes(), b"203x");
    }

    #[test]
    fn open_with_hints_never_take_multi_framing() {
        let mut text = [0u8; 12];
        text[0] = CONTENT_INTENT_MARKER;
        text[1] = b'4';
        text[2] = b'2';
        text[3] = b'0';
        text[4] = b'2';
        text[5..9].copy_from_slice(b"ab12");
        let parsed = parse_content_intent(&text[..9]).expect("legacy parse");
        assert_eq!(parsed.hint, b'4');
        assert_eq!(parsed.count, 1);
        assert_eq!(parsed.path_bytes(), b"202ab12");
    }

    #[test]
    fn multi_encode_rejects_bad_counts_and_paths() {
        assert!(encode_content_intent_multi(&[b"a"]).is_none());
        let five: [&[u8]; 5] = [&[b'a'; 5][..]; 5];
        assert!(encode_content_intent_multi(&five).is_none());
        assert!(encode_content_intent_multi(&[b"a", b"",]).is_none());
        assert!(encode_content_intent_multi(&[b"a", b"has space"]).is_none());
        assert!(encode_content_intent_multi(&[&[b'a'; 41], &[b'b'; 1]]).is_none());
    }
}

use core::str;
use serviceos_userspace_runtime::DesktopAppId;

/// Association table: extension -> default app. Pure data + codec so the
/// routing policy is host-testable; persistence lives in persist.rs.
pub(crate) const ASSOC_MAX: usize = 12;
pub(crate) const EXT_MAX: usize = 8;

/// Apps offered by "open-with" cycling, in fallback order.
pub(crate) const OPEN_CANDIDATE_APPS: [DesktopAppId; 4] = [
    DesktopAppId::Files,
    DesktopAppId::Terminal,
    DesktopAppId::Monitor,
    DesktopAppId::Settings,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Association {
    ext: [u8; EXT_MAX],
    ext_len: usize,
    app: DesktopAppId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AssocTable {
    entries: [Association; ASSOC_MAX],
    len: usize,
}

impl AssocTable {
    pub(crate) const fn empty() -> Self {
        Self {
            entries: [Association {
                ext: [0; EXT_MAX],
                ext_len: 0,
                app: DesktopAppId::Files,
            }; ASSOC_MAX],
            len: 0,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn default_for(&self, ext: &[u8]) -> Option<DesktopAppId> {
        let Some((key, key_len)) = normalize_ext(ext) else {
            return None;
        };
        self.entries[..self.len]
            .iter()
            .find(|entry| entry.ext_len == key_len && entry.ext[..key_len] == key[..key_len])
            .map(|entry| entry.app)
    }

    /// Upserts the default app for `ext`. Returns false when a brand-new
    /// extension would overflow the fixed table or the ext is invalid.
    pub(crate) fn set_default(&mut self, ext: &[u8], app: DesktopAppId) -> bool {
        let Some((key, key_len)) = normalize_ext(ext) else {
            return false;
        };
        if let Some(entry) = self
            .entries
            .iter_mut()
            .take(self.len)
            .find(|entry| entry.ext_len == key_len && entry.ext[..key_len] == key[..key_len])
        {
            entry.app = app;
            return true;
        }
        if self.len >= ASSOC_MAX {
            return false;
        }
        self.entries[self.len] = Association {
            ext: key,
            ext_len: key_len,
            app,
        };
        self.len += 1;
        true
    }

    pub(crate) fn remove(&mut self, ext: &[u8]) -> bool {
        let Some((key, key_len)) = normalize_ext(ext) else {
            return false;
        };
        let Some(index) = self.entries[..self.len]
            .iter()
            .position(|entry| entry.ext_len == key_len && entry.ext[..key_len] == key[..key_len])
        else {
            return false;
        };
        self.entries[index] = self.entries[self.len - 1];
        self.len -= 1;
        true
    }

    /// Candidate apps for `ext`: stored default first (if any), then the
    /// fixed candidate list, deduplicated, capped to `out.len()`.
    pub(crate) fn candidates(&self, ext: &[u8], out: &mut [Option<DesktopAppId>]) -> usize {
        for slot in out.iter_mut() {
            *slot = None;
        }
        let mut count = 0usize;
        let mut push = |app: DesktopAppId| {
            if count < out.len() && !out[..count].contains(&Some(app)) {
                out[count] = Some(app);
                count += 1;
            }
        };
        if let Some(app) = self.default_for(ext) {
            push(app);
        }
        for app in OPEN_CANDIDATE_APPS {
            push(app);
        }
        count
    }

    /// Encodes as `ext=app;` pairs in insertion order.
    pub(crate) fn encode(&self, buffer: &mut [u8]) -> usize {
        let mut cursor = 0usize;
        for entry in &self.entries[..self.len] {
            let Some(app_byte) = hint_digit(entry.app) else {
                continue;
            };
            if cursor + entry.ext_len + 3 > buffer.len() {
                break;
            }
            buffer[cursor..cursor + entry.ext_len].copy_from_slice(&entry.ext[..entry.ext_len]);
            cursor += entry.ext_len;
            buffer[cursor] = b'=';
            buffer[cursor + 1] = app_byte;
            buffer[cursor + 2] = b';';
            cursor += 3;
        }
        cursor
    }

    pub(crate) fn decode(bytes: &[u8]) -> Self {
        let mut table = Self::empty();
        for pair in bytes.split(|byte| *byte == b';') {
            let Some(separator) = pair.iter().position(|byte| *byte == b'=') else {
                continue;
            };
            let (ext, tail) = (&pair[..separator], &pair[separator + 1..]);
            let Some((&app_byte, _)) = tail.split_first() else {
                continue;
            };
            let Some(app) = app_from_hint(app_byte) else {
                continue;
            };
            table.set_default(ext, app);
        }
        table
    }
}

pub(crate) fn hint_digit(app: DesktopAppId) -> Option<u8> {
    let value = app as u32;
    if (1..=9).contains(&value) {
        Some(b'0' + value as u8)
    } else {
        None
    }
}

fn app_from_hint(hint: u8) -> Option<DesktopAppId> {
    match (hint as char).to_digit(10) {
        Some(value) => match value as u32 {
            x if x == DesktopAppId::Files as u32 => Some(DesktopAppId::Files),
            x if x == DesktopAppId::Terminal as u32 => Some(DesktopAppId::Terminal),
            x if x == DesktopAppId::Monitor as u32 => Some(DesktopAppId::Monitor),
            x if x == DesktopAppId::Settings as u32 => Some(DesktopAppId::Settings),
            x if x == DesktopAppId::SoftwareCenter as u32 => Some(DesktopAppId::SoftwareCenter),
            _ => None,
        },
        None => None,
    }
}

/// Lowercases and strips a leading dot; requires 1..=EXT_MAX bytes of [a-z0-9].
fn normalize_ext(ext: &[u8]) -> Option<([u8; EXT_MAX], usize)> {
    let trimmed = if ext.first() == Some(&b'.') {
        &ext[1..]
    } else {
        ext
    };
    if trimmed.is_empty() || trimmed.len() > EXT_MAX {
        return None;
    }
    if !trimmed.iter().all(|byte| byte.is_ascii_alphanumeric()) {
        return None;
    }
    let mut key = [0u8; EXT_MAX];
    key[..trimmed.len()].copy_from_slice(trimmed);
    key[..trimmed.len()].make_ascii_lowercase();
    Some((key, trimmed.len()))
}

/// Extension of a storage path: text after the last dot, provided that dot
/// appears after the final separator. Trailing separators (directories),
/// dotless names, and bare leading dots yield no extension. Case is
/// preserved here; association lookups normalize it.
pub(crate) fn extension_of(path: &[u8]) -> &[u8] {
    let name_start = path
        .iter()
        .rposition(|byte| *byte == b'/')
        .map(|index| index + 1)
        .unwrap_or(0);
    let name = &path[name_start..];
    if name.ends_with(b"/") || name.is_empty() {
        return &[];
    }
    match name.iter().rposition(|byte| *byte == b'.') {
        Some(dot) if dot != 0 && dot != name.len() - 1 => &name[dot + 1..],
        _ => &[],
    }
}

/// Routing decision: explicit open-with pick wins, then the stored default,
/// then Files as the universal fallback locator.
pub(crate) fn route_app(
    ext: &[u8],
    table: &AssocTable,
    pick: Option<DesktopAppId>,
) -> DesktopAppId {
    if let Some(app) = pick {
        return app;
    }
    if let Some(app) = table.default_for(ext) {
        return app;
    }
    DesktopAppId::Files
}

pub(crate) fn app_label(app: DesktopAppId) -> &'static str {
    match app {
        DesktopAppId::Settings => "SETTINGS",
        DesktopAppId::Files => "FILES",
        DesktopAppId::Monitor => "MONITOR",
        DesktopAppId::Terminal => "TERMINAL",
        DesktopAppId::SoftwareCenter => "SOFTWARE",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_default_upserts_and_default_for_matches_case_folded_ext() {
        let mut table = AssocTable::empty();
        assert!(table.set_default(b"txt", DesktopAppId::Terminal));
        assert_eq!(table.default_for(b"txt"), Some(DesktopAppId::Terminal));
        assert_eq!(table.default_for(b"TXT"), Some(DesktopAppId::Terminal));
        assert_eq!(table.default_for(b".Txt"), Some(DesktopAppId::Terminal));
        // Upsert overwrites instead of duplicating.
        assert!(table.set_default(b"TXT", DesktopAppId::Monitor));
        assert_eq!(table.len(), 1);
        assert_eq!(table.default_for(b"txt"), Some(DesktopAppId::Monitor));
    }

    #[test]
    fn rejects_invalid_extensions() {
        let mut table = AssocTable::empty();
        assert!(!table.set_default(b"", DesktopAppId::Monitor));
        assert!(!table.set_default(b"toolongext", DesktopAppId::Monitor));
        assert!(!table.set_default(b"a.b", DesktopAppId::Monitor));
        assert!(!table.set_default(b"sp ace", DesktopAppId::Monitor));
        assert!(!table.set_default(b".", DesktopAppId::Monitor));
        assert_eq!(table.len(), 0);
        assert_eq!(table.default_for(b""), None);
    }

    #[test]
    fn remove_swaps_last_slot_into_place() {
        let mut table = AssocTable::empty();
        assert!(table.set_default(b"a", DesktopAppId::Terminal));
        assert!(table.set_default(b"b", DesktopAppId::Monitor));
        assert!(table.set_default(b"c", DesktopAppId::Settings));
        assert!(table.remove(b"b"));
        assert_eq!(table.len(), 2);
        assert_eq!(table.default_for(b"c"), Some(DesktopAppId::Settings));
        assert_eq!(table.default_for(b"b"), None);
        assert!(!table.remove(b"b"));
    }

    #[test]
    fn encode_decode_roundtrip_preserves_entries() {
        let mut table = AssocTable::empty();
        table.set_default(b"txt", DesktopAppId::Terminal);
        table.set_default(b"log", DesktopAppId::Monitor);
        table.set_default(b"cfg", DesktopAppId::Settings);
        let mut buffer = [0u8; 128];
        let len = table.encode(&mut buffer);
        assert_eq!(AssocTable::decode(&buffer[..len]), table);
    }

    #[test]
    fn decode_tolerates_garbage_and_stops_at_capacity() {
        let decoded = AssocTable::decode(b"no-separator;;=5;zz=9;x=4;");
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded.default_for(b"x"), Some(DesktopAppId::Terminal));
        // Table never grows past ASSOC_MAX even with many distinct extensions.
        let mut packed = AssocTable::empty();
        for index in 0..ASSOC_MAX + 4 {
            let mut ext = [b'a'; 3];
            ext[2] = b'a' + index as u8;
            assert_eq!(
                packed.set_default(&ext, DesktopAppId::Terminal),
                index < ASSOC_MAX
            );
        }
        assert_eq!(packed.len(), ASSOC_MAX);
    }

    #[test]
    fn candidates_put_default_first_then_fixed_order_without_duplicates() {
        let mut table = AssocTable::empty();
        let mut out = [None; 6];
        assert_eq!(
            table.candidates(b"txt", &mut out),
            OPEN_CANDIDATE_APPS.len()
        );
        assert_eq!(out[0], Some(DesktopAppId::Files));
        // Stored default jumps the queue without being repeated later.
        table.set_default(b"txt", DesktopAppId::Monitor);
        let count = table.candidates(b"TXT", &mut out);
        assert_eq!(out[0], Some(DesktopAppId::Monitor));
        assert_eq!(count, OPEN_CANDIDATE_APPS.len());
        assert_eq!(
            out[1..count]
                .iter()
                .filter(|slot| **slot == Some(DesktopAppId::Monitor))
                .count(),
            0
        );
        // Small output buffer truncates safely.
        let mut tiny = [None; 2];
        assert_eq!(table.candidates(b"txt", &mut tiny), 2);
    }

    #[test]
    fn extension_of_handles_names_dirs_and_trailing_slashes() {
        assert_eq!(extension_of(b"home/notes.txt"), b"txt");
        assert_eq!(extension_of(b"NOTES.TXT"), b"TXT");
        assert_eq!(extension_of(b"home/archive.tar.gz"), b"gz");
        assert_eq!(extension_of(b"home/noext"), b"");
        // A leading dot marks a hidden file, not an extension.
        assert_eq!(extension_of(b"home/.hidden"), b"");
        assert_eq!(extension_of(b"home/dir.d/"), b"");
        assert_eq!(extension_of(b"readme"), b"");
        assert_eq!(extension_of(b"a.b/c"), b"");
        assert_eq!(extension_of(b""), b"");
    }

    #[test]
    fn route_prefers_pick_then_default_then_files_fallback() {
        let mut table = AssocTable::empty();
        table.set_default(b"log", DesktopAppId::Monitor);
        assert_eq!(
            route_app(b"log", &table, Some(DesktopAppId::Settings)),
            DesktopAppId::Settings
        );
        assert_eq!(route_app(b"log", &table, None), DesktopAppId::Monitor);
        assert_eq!(route_app(b"unknown", &table, None), DesktopAppId::Files);
    }

    #[test]
    fn labels_and_hint_digits_cover_candidate_apps() {
        for app in OPEN_CANDIDATE_APPS {
            assert!(!app_label(app).is_empty());
            assert!(hint_digit(app).is_some());
        }
        assert_eq!(hint_digit(DesktopAppId::SoftwareCenter), Some(b'5'));
    }
}

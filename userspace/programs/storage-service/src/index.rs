use rt::{Handle, RawMessage, StorageEntryKind, StorageStatus};
use serviceos_userspace_runtime as rt;

use crate::{
    path::{find_mutable_entry, path_matches_prefix},
    state::{
        EntrySlot, MAX_BOOTSTORE_ENTRIES, MAX_MUTABLE_ENTRIES, MAX_STORAGE_PATH, MutableEntry,
    },
    util::{pack_bytes, send_reply_and_close, unpack_bytes},
};

pub(crate) const SEARCH_REQUEST_TAG: u32 = 0x521;
pub(crate) const SEARCH_REPLY_TAG: u32 = 0x522;
pub(crate) const GREP_REQUEST_TAG: u32 = 0x523;
pub(crate) const GREP_REPLY_TAG: u32 = 0x524;
pub(crate) const INDEX_TAIL_MAGIC: u64 = 0x5844_4953 | (3u64 << 32);

const INDEX_CAPACITY: usize = MAX_BOOTSTORE_ENTRIES + MAX_MUTABLE_ENTRIES;
const NAME_TOKEN_MAX: usize = 4;
const TOKEN_LEN_MAX: usize = 12;
const QUERY_TOKEN_MAX: usize = 3;
const QUERY_TOKEN_LEN_MAX: usize = 24;
const CLASS_SUBSTRING: u8 = 1;
const CLASS_PREFIX: u8 = 2;
const CLASS_EXACT: u8 = 3;
pub(crate) const GREP_FILE_BYTES_MAX: usize = 4096;
pub(crate) const GREP_RESULTS_MAX: usize = 16;
pub(crate) const GREP_NEEDLE_MAX: usize = 32;
const GREP_WINDOW_LEN_MAX: usize = GREP_NEEDLE_MAX;
const GREP_CHUNK_BYTES: usize = 512;
pub(crate) const GREP_FLAG_TRUNCATED: u64 = 1 << 0;
pub(crate) const GREP_FLAG_OVERSIZE_SKIPPED: u64 = 1 << 1;
pub(crate) const ORIGIN_BOOT: u8 = 0;
pub(crate) const ORIGIN_MUTABLE: u8 = 1;

#[derive(Clone, Copy)]
pub(crate) struct IndexEntry {
    path: [u8; MAX_STORAGE_PATH],
    path_len: usize,
    kind: StorageEntryKind,
    size: u64,
    tick: u64,
    origin: u8,
    boot_offset: u64,
}

impl IndexEntry {
    const fn empty() -> Self {
        Self {
            path: [0; MAX_STORAGE_PATH],
            path_len: 0,
            kind: StorageEntryKind::File,
            size: 0,
            tick: 0,
            origin: ORIGIN_BOOT,
            boot_offset: 0,
        }
    }

    pub(crate) fn path(&self) -> &[u8] {
        &self.path[..self.path_len]
    }

    fn boot_file(path: &[u8], data_offset: usize, data_len: usize) -> Self {
        let mut slot = Self::empty();
        slot.path[..path.len()].copy_from_slice(path);
        slot.path_len = path.len();
        slot.kind = StorageEntryKind::File;
        slot.size = data_len as u64;
        slot.origin = ORIGIN_BOOT;
        slot.boot_offset = data_offset as u64;
        slot
    }

    fn from_mutable(path: &[u8], kind: StorageEntryKind, data_len: usize, tick: u64) -> Self {
        let mut slot = Self::empty();
        slot.path[..path.len()].copy_from_slice(path);
        slot.path_len = path.len();
        slot.kind = kind;
        slot.size = data_len as u64;
        slot.tick = tick;
        slot.origin = ORIGIN_MUTABLE;
        slot
    }

    #[cfg(test)]
    fn test_entry(path: &[u8], kind: StorageEntryKind, size: u64, tick: u64, origin: u8) -> Self {
        let mut slot = Self::empty();
        slot.path[..path.len()].copy_from_slice(path);
        slot.path_len = path.len();
        slot.kind = kind;
        slot.size = size;
        slot.tick = tick;
        slot.origin = origin;
        slot
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SearchIndex {
    entries: [IndexEntry; INDEX_CAPACITY],
    count: usize,
    dirty: bool,
    built: bool,
    last_rebuild: u64,
}

impl SearchIndex {
    pub(crate) const fn new() -> Self {
        Self {
            entries: [IndexEntry::empty(); INDEX_CAPACITY],
            count: 0,
            dirty: true,
            built: false,
            last_rebuild: 0,
        }
    }

    pub(crate) fn stats(&self) -> (usize, bool, u64) {
        (self.count, self.dirty, self.last_rebuild)
    }

    pub(crate) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub(crate) fn upsert(
        &mut self,
        path: &[u8],
        kind: StorageEntryKind,
        size: u64,
        tick: u64,
        origin: u8,
    ) {
        if !self.built {
            self.dirty = true;
            return;
        }
        if let Some(slot) = self
            .entries
            .iter_mut()
            .take(self.count)
            .find(|entry| entry.path_len == path.len() && entry.path[..entry.path_len] == *path)
        {
            slot.kind = kind;
            slot.size = size;
            if tick != 0 {
                slot.tick = tick;
            }
            return;
        }
        if self.count >= INDEX_CAPACITY {
            self.dirty = true;
            return;
        }
        self.entries[self.count] = IndexEntry::from_mutable(path, kind, size as usize, tick);
        if origin == ORIGIN_BOOT {
            self.entries[self.count].origin = ORIGIN_BOOT;
        }
        self.count += 1;
    }

    #[allow(dead_code)]
    pub(crate) fn rename(&mut self, old: &[u8], new: &[u8], tick: u64) {
        let moved = self
            .entries
            .iter()
            .take(self.count)
            .find(|entry| entry.path_len == old.len() && entry.path[..entry.path_len] == *old);
        let Some(source) = moved else {
            return;
        };
        let kind = source.kind;
        let size = source.size;
        let origin = source.origin;
        let boot_offset = source.boot_offset;
        self.remove_path(old);
        if !self.built || self.count >= INDEX_CAPACITY {
            return;
        }
        let mut slot = IndexEntry::empty();
        slot.path[..new.len()].copy_from_slice(new);
        slot.path_len = new.len();
        slot.kind = kind;
        slot.size = size;
        slot.tick = tick.max(1);
        slot.origin = origin;
        slot.boot_offset = boot_offset;
        self.entries[self.count] = slot;
        self.count += 1;
    }

    pub(crate) fn remove_path(&mut self, path: &[u8]) {
        if !self.built {
            self.dirty = true;
            return;
        }
        let Some(position) = self.entries.iter().take(self.count).position(|entry| {
            entry.path_len == path.len() && entry.path[..entry.path_len] == *path
        }) else {
            return;
        };
        self.count -= 1;
        self.entries[position] = self.entries[self.count];
        self.entries[self.count] = IndexEntry::empty();
    }

    pub(crate) fn ensure_built(
        &mut self,
        boot_entries: &[EntrySlot],
        mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
        now: u64,
    ) -> bool {
        if self.built && !self.dirty {
            return false;
        }
        self.count = 0;
        for entry in boot_entries.iter() {
            if self.count >= INDEX_CAPACITY {
                break;
            }
            self.entries[self.count] = IndexEntry::boot_file(
                &entry.path[..entry.path_len],
                entry.data_offset,
                entry.data_len,
            );
            self.count += 1;
        }
        for entry in mutable_entries.iter().filter(|entry| entry.occupied) {
            if self.count >= INDEX_CAPACITY {
                break;
            }
            self.entries[self.count] = IndexEntry::from_mutable(
                &entry.path[..entry.path_len],
                entry.kind,
                entry.data_len,
                now,
            );
            self.count += 1;
        }
        self.built = true;
        self.dirty = false;
        self.last_rebuild = now;
        true
    }

    pub(crate) fn snapshot(&self) -> &[IndexEntry] {
        &self.entries[..self.count]
    }
}

pub(crate) struct NameTokens {
    lens: [u8; NAME_TOKEN_MAX],
    bytes: [[u8; TOKEN_LEN_MAX]; NAME_TOKEN_MAX],
    count: usize,
}

pub(crate) fn split_name(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|byte| *byte == b'/') {
        Some(position) => &path[position + 1..],
        None => path,
    }
}

fn fold_lower(byte: u8) -> u8 {
    byte.to_ascii_lowercase()
}

pub(crate) fn tokenize_name(name: &[u8]) -> NameTokens {
    let mut tokens = NameTokens {
        lens: [0; NAME_TOKEN_MAX],
        bytes: [[0; TOKEN_LEN_MAX]; NAME_TOKEN_MAX],
        count: 0,
    };
    let mut current: [u8; TOKEN_LEN_MAX] = [0; TOKEN_LEN_MAX];
    let mut current_len = 0usize;
    for byte in name.iter().copied().chain(core::iter::once(b'\0')) {
        let separator = byte == b'.' || byte == b'-' || byte == b'_' || byte == b' ' || byte == 0;
        if separator {
            if current_len > 0 && tokens.count < NAME_TOKEN_MAX {
                tokens.bytes[tokens.count] = current;
                tokens.lens[tokens.count] = current_len as u8;
                tokens.count += 1;
            }
            current = [0; TOKEN_LEN_MAX];
            current_len = 0;
        } else if current_len < TOKEN_LEN_MAX {
            current[current_len] = fold_lower(byte);
            current_len += 1;
        }
    }
    tokens
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

pub(crate) fn match_class(name: &[u8], tokens: &NameTokens, query: &[u8]) -> u8 {
    let mut lower_buf = [0u8; MAX_STORAGE_PATH];
    let lower_len = name.len().min(MAX_STORAGE_PATH);
    for (index, byte) in name.iter().copied().enumerate().take(lower_len) {
        lower_buf[index] = fold_lower(byte);
    }
    let lower = &lower_buf[..lower_len];
    let mut query_buf = [0u8; QUERY_TOKEN_LEN_MAX];
    let query_len = query.len().min(query_buf.len());
    for (index, byte) in query.iter().copied().enumerate().take(query_len) {
        query_buf[index] = fold_lower(byte);
    }
    let query = &query_buf[..query_len];
    let mut best = if lower == query {
        CLASS_EXACT
    } else if lower.starts_with(query) {
        CLASS_PREFIX
    } else if contains_subslice(lower, query) {
        CLASS_SUBSTRING
    } else {
        0
    };
    for index in 0..tokens.count {
        let len = tokens.lens[index] as usize;
        let token = &tokens.bytes[index][..len];
        let class = if token == query {
            CLASS_EXACT
        } else if token.starts_with(query) {
            CLASS_PREFIX
        } else if contains_subslice(token, query) {
            CLASS_SUBSTRING
        } else {
            0
        };
        if class > best {
            best = class;
        }
    }
    best
}

#[derive(Clone, Copy)]
struct Scored {
    score: u8,
    position: usize,
}

fn sort_scored(scored: &mut [Scored], snapshot: &[IndexEntry]) {
    for outer in 1..scored.len() {
        let mut cursor = outer;
        while cursor > 0 {
            let prev = scored[cursor - 1];
            let cur = scored[cursor];
            let prev_path = snapshot[prev.position].path();
            let cur_path = snapshot[cur.position].path();
            let swap = prev.score < cur.score || (prev.score == cur.score && prev_path > cur_path);
            if swap {
                scored.swap(cursor - 1, cursor);
                cursor -= 1;
            } else {
                break;
            }
        }
    }
}

fn sort_positions_by_path(positions: &mut [usize], snapshot: &[IndexEntry]) {
    for outer in 1..positions.len() {
        let mut cursor = outer;
        while cursor > 0
            && snapshot[positions[cursor - 1]].path() > snapshot[positions[cursor]].path()
        {
            positions.swap(cursor - 1, cursor);
            cursor -= 1;
        }
    }
}

pub(crate) struct SearchPlan {
    pub(crate) order: [usize; INDEX_CAPACITY],
    pub(crate) len: usize,
}

impl SearchPlan {
    fn at(&self, cursor: usize) -> Option<usize> {
        if cursor < self.len {
            Some(self.order[cursor])
        } else {
            None
        }
    }
}

pub(crate) fn plan_search(
    snapshot: &[IndexEntry],
    scope: &[u8],
    queries: &[&[u8]],
    min_size: u64,
    max_size: u64,
    since_tick: u64,
    now: u64,
) -> SearchPlan {
    let mut plan = SearchPlan {
        order: [0; INDEX_CAPACITY],
        len: 0,
    };
    let mut scored: [Scored; INDEX_CAPACITY] = [Scored {
        score: 0,
        position: 0,
    }; INDEX_CAPACITY];
    let mut matched = 0usize;
    for (position, entry) in snapshot.iter().enumerate() {
        let path = entry.path();
        if !path_matches_prefix(path, scope) {
            continue;
        }
        if entry.size < min_size || entry.size > max_size {
            continue;
        }
        if since_tick > 0 && (now < since_tick || entry.tick < now - since_tick) {
            continue;
        }
        if queries.is_empty() {
            continue;
        }
        let name = split_name(path);
        let tokens = tokenize_name(name);
        let mut total = 0u16;
        let mut all_matched = true;
        for query in queries {
            let class = match_class(name, &tokens, query);
            if class == 0 {
                all_matched = false;
                break;
            }
            total += class as u16;
        }
        if !all_matched {
            continue;
        }
        scored[matched] = Scored {
            score: total.min(u8::MAX as u16) as u8,
            position,
        };
        matched += 1;
    }
    sort_scored(&mut scored[..matched], snapshot);
    for item in scored[..matched].iter() {
        plan.order[plan.len] = item.position;
        plan.len += 1;
    }
    plan
}

pub(crate) struct GrepPlan {
    order: [usize; INDEX_CAPACITY],
    len: usize,
    oversize_skipped: usize,
}

pub(crate) fn plan_grep(snapshot: &[IndexEntry], scope: &[u8], file_cap: usize) -> GrepPlan {
    let mut plan = GrepPlan {
        order: [0; INDEX_CAPACITY],
        len: 0,
        oversize_skipped: 0,
    };
    for (position, entry) in snapshot.iter().enumerate() {
        let path = entry.path();
        if !path_matches_prefix(path, scope) || entry.kind != StorageEntryKind::File {
            continue;
        }
        if entry.size as usize > file_cap {
            plan.oversize_skipped += 1;
            continue;
        }
        plan.order[plan.len] = position;
        plan.len += 1;
    }
    sort_positions_by_path(&mut plan.order[..plan.len], snapshot);
    plan
}

pub(crate) struct StreamNeedle {
    needle: [u8; GREP_NEEDLE_MAX],
    needle_len: usize,
    window: [u8; GREP_WINDOW_LEN_MAX],
    window_len: usize,
    line_no: u64,
    last_hit_line: u64,
}

impl StreamNeedle {
    pub(crate) fn new(needle: &[u8]) -> Option<Self> {
        if needle.is_empty() || needle.len() > GREP_NEEDLE_MAX || needle.contains(&b'\n') {
            return None;
        }
        let mut stored = [0u8; GREP_NEEDLE_MAX];
        stored[..needle.len()].copy_from_slice(needle);
        Some(Self {
            needle: stored,
            needle_len: needle.len(),
            window: [0; GREP_WINDOW_LEN_MAX],
            window_len: 0,
            line_no: 1,
            last_hit_line: 0,
        })
    }

    pub(crate) fn feed(&mut self, chunk: &[u8], mut on_match: impl FnMut(u64)) {
        for byte in chunk.iter().copied() {
            if byte == b'\n' {
                self.line_no += 1;
                self.window_len = 0;
                continue;
            }
            if self.window_len < GREP_WINDOW_LEN_MAX {
                self.window[self.window_len] = byte;
                self.window_len += 1;
            } else {
                self.window.copy_within(1.., 0);
                self.window[GREP_WINDOW_LEN_MAX - 1] = byte;
            }
            if self.window_len >= self.needle_len
                && self.window[self.window_len - self.needle_len..self.window_len]
                    == self.needle[..self.needle_len]
                && self.line_no != self.last_hit_line
            {
                self.last_hit_line = self.line_no;
                on_match(self.line_no);
            }
        }
    }
}

pub(crate) struct GrepOutcome {
    hit_lines: [u64; GREP_RESULTS_MAX],
    hit_count: usize,
    cap_reached_mid_scan: bool,
}

fn scan_entry_for_matches(
    bootstore: Handle,
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    entry: &IndexEntry,
    needle: &[u8],
    content: &mut [u8],
    budget: usize,
) -> GrepOutcome {
    let mut outcome = GrepOutcome {
        hit_lines: [0; GREP_RESULTS_MAX],
        hit_count: 0,
        cap_reached_mid_scan: false,
    };
    let Some(mut stream) = StreamNeedle::new(needle) else {
        return outcome;
    };
    let mutable_slot = if entry.origin == ORIGIN_MUTABLE {
        find_mutable_entry(mutable_entries, entry.path())
    } else {
        None
    };
    if entry.origin == ORIGIN_MUTABLE && mutable_slot.is_none() {
        return outcome;
    }
    let mut collected = 0usize;
    let mut stop = false;
    let mut file_pos = 0usize;
    let total_len = entry.size as usize;
    while file_pos < total_len && !stop {
        let want = content.len().min(total_len - file_pos);
        let got = if entry.origin == ORIGIN_BOOT {
            rt::memory_read(
                bootstore,
                entry.boot_offset as usize + file_pos,
                &mut content[..want],
            )
            .ok()
            .unwrap_or(0)
        } else {
            let slot = mutable_slot.unwrap_or(0);
            let readable = mutable_entries[slot].data_len.saturating_sub(file_pos);
            let take = want.min(readable);
            if take == 0 {
                break;
            }
            rt::memory_read(
                mutable_entries[slot].data_handle,
                file_pos,
                &mut content[..take],
            )
            .ok()
            .unwrap_or(0)
        };
        if got == 0 {
            break;
        }
        stream.feed(&content[..got], |line| {
            if collected < budget && collected < GREP_RESULTS_MAX {
                outcome.hit_lines[collected] = line;
                collected += 1;
            } else {
                stop = true;
                outcome.cap_reached_mid_scan = true;
            }
        });
        file_pos += got;
    }
    outcome.hit_count = collected;
    outcome
}

pub(crate) fn handle_search_request(
    boot_entries: &[EntrySlot],
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    index: &mut SearchIndex,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 6 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let cursor = message.words[0] as usize;
    let scope_len = message.words[1] as usize;
    let min_size = message.words[2];
    let max_size = message.words[3];
    let since_tick = message.words[4];
    let query_len = message.words[5] as usize;
    let mut scope_buf = [0u8; MAX_STORAGE_PATH];
    let mut query_buf = [0u8; QUERY_TOKEN_MAX * (QUERY_TOKEN_LEN_MAX + 1)];
    let payload = &message.words[6..message.word_count as usize];
    if unpack_bytes(payload, scope_len.min(scope_buf.len()), &mut scope_buf).is_err() {
        return Ok(());
    }
    let tail = scope_len.div_ceil(8);
    if unpack_bytes(
        payload.get(tail..).unwrap_or(&[]),
        query_len.min(query_buf.len()),
        &mut query_buf,
    )
    .is_err()
    {
        return Ok(());
    }
    let scope = &scope_buf[..scope_len.min(scope_buf.len())];
    let query_raw = &query_buf[..query_len.min(query_buf.len())];
    let mut query_storage: [[u8; QUERY_TOKEN_LEN_MAX]; QUERY_TOKEN_MAX] =
        [[0; QUERY_TOKEN_LEN_MAX]; QUERY_TOKEN_MAX];
    let mut query_lens = [0usize; QUERY_TOKEN_MAX];
    let mut query_count = 0usize;
    let mut current_len = 0usize;
    for byte in query_raw.iter().copied().chain(core::iter::once(b' ')) {
        if byte == b' ' || byte == 0 {
            if current_len > 0 && query_count < QUERY_TOKEN_MAX {
                query_lens[query_count] = current_len;
                query_count += 1;
            }
            current_len = 0;
        } else if query_count < QUERY_TOKEN_MAX && current_len < QUERY_TOKEN_LEN_MAX {
            query_storage[query_count][current_len] = fold_lower(byte);
            current_len += 1;
        }
    }
    let queries_storage: [&[u8]; QUERY_TOKEN_MAX] = [
        &query_storage[0][..query_lens[0]],
        &query_storage[1][..query_lens[1]],
        &query_storage[2][..query_lens[2]],
    ];
    let queries = &queries_storage[..query_count];

    let now = rt::monotonic_now().unwrap_or(0);
    index.ensure_built(boot_entries, mutable_entries, now);
    let plan = plan_search(
        index.snapshot(),
        scope,
        queries,
        min_size,
        max_size,
        since_tick,
        now,
    );

    let mut reply = RawMessage::empty(SEARCH_REPLY_TAG);
    reply.word_count = 4;
    reply.words[0] = StorageStatus::End as u32 as u64;
    reply.words[1] = cursor as u64;
    reply.words[2] = StorageEntryKind::File as u32 as u64;
    reply.words[3] = 0;
    if let Some(position) = plan.at(cursor) {
        let entry = index.snapshot()[position];
        reply.words[0] = StorageStatus::Ok as u32 as u64;
        reply.words[1] = (cursor + 1) as u64;
        reply.words[2] = entry.kind as u32 as u64;
        reply.words[3] = entry.path().len() as u64;
        reply.word_count += pack_bytes(entry.path(), &mut reply.words[4..])?;
    }
    send_reply_and_close(reply_handle, &reply);
    Ok(())
}

pub(crate) fn handle_grep_request(
    bootstore: Handle,
    boot_entries: &[EntrySlot],
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    index: &mut SearchIndex,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 6 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let cursor = message.words[0] as usize;
    let scope_len = message.words[1] as usize;
    let needle_len = message.words[2] as usize;
    let requested_file_cap = message.words[3] as usize;
    let requested_result_cap = message.words[4] as usize;
    let mut scope_buf = [0u8; MAX_STORAGE_PATH];
    let mut needle_buf = [0u8; GREP_NEEDLE_MAX];
    let payload = &message.words[5..message.word_count as usize];
    if unpack_bytes(payload, scope_len.min(scope_buf.len()), &mut scope_buf).is_err() {
        return Ok(());
    }
    let tail = scope_len.div_ceil(8);
    if unpack_bytes(
        payload.get(tail..).unwrap_or(&[]),
        needle_len.min(needle_buf.len()),
        &mut needle_buf,
    )
    .is_err()
    {
        return Ok(());
    }
    let scope = &scope_buf[..scope_len.min(scope_buf.len())];
    let needle = &needle_buf[..needle_len.min(needle_buf.len())];
    if StreamNeedle::new(needle).is_none() {
        return Ok(());
    }
    let file_cap = if requested_file_cap == 0 {
        GREP_FILE_BYTES_MAX
    } else {
        requested_file_cap.min(GREP_FILE_BYTES_MAX)
    };
    let result_cap = if requested_result_cap == 0 {
        GREP_RESULTS_MAX
    } else {
        requested_result_cap.min(GREP_RESULTS_MAX)
    };

    let now = rt::monotonic_now().unwrap_or(0);
    index.ensure_built(boot_entries, mutable_entries, now);
    let plan = plan_grep(index.snapshot(), scope, file_cap);

    let mut flags: u64 = 0;
    if plan.oversize_skipped > 0 {
        flags |= GREP_FLAG_OVERSIZE_SKIPPED;
    }
    let mut content = [0u8; GREP_CHUNK_BYTES];
    let mut total = 0usize;
    let mut truncated = false;
    let mut files_scanned = 0usize;
    for plan_index in 0..plan.len {
        if total >= result_cap {
            break;
        }
        let position = plan.order[plan_index];
        let entry = index.snapshot()[position];
        files_scanned += 1;
        let outcome = scan_entry_for_matches(
            bootstore,
            mutable_entries,
            &entry,
            needle,
            &mut content,
            result_cap - total,
        );
        if outcome.cap_reached_mid_scan {
            truncated = true;
        }
        for line in outcome.hit_lines.into_iter().take(outcome.hit_count) {
            if total == cursor {
                let mut reply = RawMessage::empty(GREP_REPLY_TAG);
                reply.word_count = 5;
                reply.words[0] = StorageStatus::Ok as u32 as u64;
                reply.words[1] = (total + 1) as u64;
                reply.words[2] = line;
                reply.words[3] = grep_flags(flags, truncated);
                reply.words[4] = entry.path().len() as u64;
                reply.word_count += pack_bytes(entry.path(), &mut reply.words[5..])?;
                send_reply_and_close(reply_handle, &reply);
                return Ok(());
            }
            total += 1;
        }
    }
    if cap_exhausted(total, result_cap) && files_scanned < plan.len {
        truncated = true;
    }
    let mut reply = RawMessage::empty(GREP_REPLY_TAG);
    reply.word_count = 5;
    reply.words[0] = StorageStatus::End as u32 as u64;
    reply.words[1] = cursor as u64;
    reply.words[2] = 0;
    reply.words[3] = grep_flags(flags, truncated);
    reply.words[4] = 0;
    send_reply_and_close(reply_handle, &reply);
    Ok(())
}

pub(crate) fn cap_exhausted(total_matches: usize, result_cap: usize) -> bool {
    result_cap > 0 && total_matches >= result_cap
}

pub(crate) fn grep_flags(base: u64, truncated: bool) -> u64 {
    if truncated {
        base | GREP_FLAG_TRUNCATED
    } else {
        base
    }
}

pub(crate) fn index_tail_words(index: &SearchIndex) -> [u64; 4] {
    let (count, dirty, last_rebuild) = index.stats();
    [INDEX_TAIL_MAGIC, count as u64, dirty as u64, last_rebuild]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &[u8], size: u64, tick: u64) -> IndexEntry {
        IndexEntry::test_entry(path, StorageEntryKind::File, size, tick, ORIGIN_MUTABLE)
    }

    fn plan_paths(plan: &SearchPlan, snapshot: &[IndexEntry]) -> Vec<Vec<u8>> {
        (0..plan.len)
            .map(|index| snapshot[plan.order[index]].path().to_vec())
            .collect()
    }

    #[test]
    fn token_ranking_exact_then_prefix_then_substring() {
        let snapshot = [
            entry(b"data/mynote.txt", 1, 5),
            entry(b"data/notebook.md", 2, 6),
            entry(b"data/note.txt", 3, 7),
        ];
        let plan = plan_search(&snapshot, b"", &[b"note"], 0, u64::MAX, 0, 10);
        assert_eq!(
            plan_paths(&plan, &snapshot),
            vec![
                b"data/note.txt".to_vec(),
                b"data/notebook.md".to_vec(),
                b"data/mynote.txt".to_vec()
            ]
        );
    }

    #[test]
    fn and_semantics_requires_every_token() {
        let snapshot = [entry(b"data/note.txt", 1, 5), entry(b"data/notes.md", 2, 5)];
        let both = plan_search(&snapshot, b"", &[b"note", b"txt"], 0, u64::MAX, 0, 10);
        assert_eq!(
            plan_paths(&both, &snapshot),
            vec![b"data/note.txt".to_vec()]
        );
        let none = plan_search(&snapshot, b"", &[b"note", b"zzz"], 0, u64::MAX, 0, 10);
        assert_eq!(none.len, 0);
    }

    #[test]
    fn scope_size_and_recency_filters_apply() {
        let snapshot = [
            entry(b"data/keep.txt", 10, 9),
            entry(b"scratch/skip-scope.txt", 10, 9),
            entry(b"data/too-small.txt", 5, 9),
            entry(b"data/stale.txt", 10, 2),
        ];
        let plan = plan_search(&snapshot, b"data/", &[b"txt"], 8, u64::MAX, 5, 10);
        assert_eq!(
            plan_paths(&plan, &snapshot),
            vec![b"data/keep.txt".to_vec()]
        );
    }

    #[test]
    fn mutation_events_apply_without_ttl() {
        let mut index = SearchIndex::new();
        let boot: [EntrySlot; 0] = [];
        let mut mutable = [MutableEntry::empty(); MAX_MUTABLE_ENTRIES];
        assert!(index.dirty && !index.built);
        index.upsert(
            b"data/pre.txt",
            StorageEntryKind::File,
            4,
            1,
            ORIGIN_MUTABLE,
        );
        assert!(index.dirty);
        assert!(index.ensure_built(&boot, &mutable, 100));
        assert_eq!((index.count, index.built), (0, true));
        assert_eq!(index.last_rebuild, 100);

        index.upsert(
            b"data/live.txt",
            StorageEntryKind::File,
            7,
            101,
            ORIGIN_MUTABLE,
        );
        let plan = plan_search(index.snapshot(), b"", &[b"live"], 0, u64::MAX, 0, 101);
        assert_eq!(plan.len, 1);

        index.remove_path(b"data/live.txt");
        let plan = plan_search(index.snapshot(), b"", &[b"live"], 0, u64::MAX, 0, 101);
        assert_eq!(plan.len, 0);

        index.upsert(
            b"data/old.txt",
            StorageEntryKind::File,
            7,
            101,
            ORIGIN_MUTABLE,
        );
        index.rename(b"data/old.txt", b"data/new.txt", 102);
        let plan = plan_search(index.snapshot(), b"", &[b"new"], 0, u64::MAX, 0, 102);
        assert_eq!(plan.len, 1);
        assert!(!index.stats().1);
    }

    #[test]
    fn grep_plan_skips_oversize_and_orders_by_path() {
        let snapshot = [
            entry(b"z/last.txt", 20, 1),
            entry(b"huge.bin", GREP_FILE_BYTES_MAX as u64 + 1, 1),
            entry(b"a/first.txt", 30, 1),
        ];
        let plan = plan_grep(&snapshot, b"", GREP_FILE_BYTES_MAX);
        assert_eq!(plan.oversize_skipped, 1);
        assert_eq!(plan.len, 2);
        assert_eq!(snapshot[plan.order[0]].path(), b"a/first.txt");
        assert_eq!(snapshot[plan.order[1]].path(), b"z/last.txt");
    }

    #[test]
    fn stream_needle_tracks_lines_across_chunks() {
        let text = b"alpha beta\ngamma needle delta\nneedle again\nplain";
        let mut stream = StreamNeedle::new(b"needle").unwrap();
        let mut hits = Vec::new();
        for chunk in text.chunks(3) {
            stream.feed(chunk, |line| hits.push(line));
        }
        assert_eq!(hits, vec![2, 3]);
    }

    #[test]
    fn stream_needle_matches_inside_long_lines() {
        let mut line = vec![b'a'; 200];
        line.extend_from_slice(b"needle");
        line.push(b'\n');
        let mut stream = StreamNeedle::new(b"needle").unwrap();
        let mut hits = Vec::new();
        for chunk in line.chunks(7) {
            stream.feed(chunk, |hit| hits.push(hit));
        }
        assert_eq!(hits, vec![1]);
    }

    #[test]
    fn stream_needle_rejects_invalid_needles() {
        assert!(StreamNeedle::new(b"").is_none());
        assert!(StreamNeedle::new(&[b'x'; GREP_NEEDLE_MAX + 1]).is_none());
        assert!(StreamNeedle::new(b"two\nlines").is_none());
    }

    #[test]
    fn grep_bounds_helpers_flag_truncation() {
        assert!(cap_exhausted(16, 16));
        assert!(!cap_exhausted(15, 16));
        assert!(!cap_exhausted(16, 0));
        assert_eq!(
            grep_flags(GREP_FLAG_OVERSIZE_SKIPPED, true),
            GREP_FLAG_OVERSIZE_SKIPPED | GREP_FLAG_TRUNCATED
        );
        assert_eq!(grep_flags(0, false), 0);
    }

    #[test]
    fn tokenize_and_match_classes() {
        let tokens = tokenize_name(b"My-Report.Final2");
        assert_eq!(tokens.count, 3);
        assert_eq!(&tokens.bytes[0][..tokens.lens[0] as usize], b"my");
        assert_eq!(&tokens.bytes[2][..tokens.lens[2] as usize], b"final2");
        assert_eq!(
            match_class(b"note.txt", &tokenize_name(b"note.txt"), b"NOTE"),
            CLASS_EXACT
        );
        assert_eq!(
            match_class(b"notes.txt", &tokenize_name(b"notes.txt"), b"note"),
            CLASS_PREFIX
        );
        assert_eq!(
            match_class(b"mynote.txt", &tokenize_name(b"mynote.txt"), b"not"),
            CLASS_SUBSTRING
        );
        assert_eq!(
            match_class(b"other.md", &tokenize_name(b"other.md"), b"note"),
            0
        );
    }
}

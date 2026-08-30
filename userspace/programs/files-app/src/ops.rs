//! File-operation decision logic and storage-backed mutations for the
//! files app: create, delete, rename, move, and copy composed from the
//! existing storage-service contracts.

use crate::state::{EntryKind, MAX_STORAGE_PATH};
use serviceos_userspace_runtime as rt;

/// Largest payload a single `storage_write` can carry inline:
/// `(IPC_MAX_WORDS - 3) * 8` with offset/total/len words in front.
pub(crate) const WRITE_CHUNK_MAX: usize = (rt::IPC_MAX_WORDS - 3) * 8;
/// Longest entry name accepted from prompts.
pub(crate) const NAME_MAX: usize = 32;
/// Bounded attempts when suffixing colliding names.
pub(crate) const UNIQUE_NAME_ATTEMPTS: usize = 9;
/// Recursion guard for directory copies.
pub(crate) const COPY_DEPTH_MAX: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OpError {
    InvalidName,
    TooLong,
    NotFound,
    Denied,
    Exists,
    Busy,
    Transport,
}

impl From<rt::Error> for OpError {
    fn from(error: rt::Error) -> Self {
        match error {
            rt::Error::NotFound => OpError::NotFound,
            rt::Error::PermissionDenied => OpError::Denied,
            rt::Error::Busy => OpError::Busy,
            rt::Error::BufferTooSmall => OpError::TooLong,
            rt::Error::InvalidArgument => OpError::InvalidName,
            _ => OpError::Transport,
        }
    }
}

pub(crate) fn friendly_error(error: OpError) -> &'static str {
    match error {
        OpError::InvalidName => "BAD NAME (A-Z, 0-9, DOT)",
        OpError::TooLong => "NAME OR PATH TOO LONG",
        OpError::NotFound => "NOT FOUND",
        OpError::Denied => "READ-ONLY OR NO PERMISSION",
        OpError::Exists => "NAME ALREADY IN USE",
        OpError::Busy => "TARGET BUSY OR DIR NOT EMPTY",
        OpError::Transport => "STORAGE ERROR",
    }
}

pub(crate) type OpResult<T> = Result<T, OpError>;

/// Strategy selected for a move/rename request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MovePlan {
    /// Server-side rename primitive: same-directory rename, cross-directory
    /// move, and whole-subtree directory moves in one atomic request.
    Rename,
    /// Chunked copy to the target followed by source delete; used when the
    /// source/destination pair cannot pack into one rename message.
    CopyDelete,
    CopyDeleteTree,
}

impl MovePlan {
    pub(crate) fn decide(rename_supported: bool, kind: EntryKind) -> MovePlan {
        match (rename_supported, kind) {
            (true, _) => MovePlan::Rename,
            (_, EntryKind::Directory) => MovePlan::CopyDeleteTree,
            (_, EntryKind::File) | (_, EntryKind::Parent) => MovePlan::CopyDelete,
        }
    }
}

/// True when a source/destination pair fits a single rename message:
/// two length words plus both byte-packed paths must stay within
/// `IPC_MAX_WORDS`. Outside this window the copy-then-delete plan runs.
pub(crate) fn rename_packs(source_len: usize, dest_len: usize) -> bool {
    2 + source_len.div_ceil(8) + dest_len.div_ceil(8) <= rt::IPC_MAX_WORDS
}

/// Rename-specific status mapping: the storage service reports destination
/// collisions with `AlreadyExists`, which the runtime transports as `Busy`
/// (the established collision encoding from directory-create). In rename
/// context that can only mean the destination name is taken.
pub(crate) fn rename_error(error: rt::Error) -> OpError {
    match error {
        rt::Error::Busy => OpError::Exists,
        other => OpError::from(other),
    }
}

/// Chunk/step math for a byte-exact copy with bounded progress steps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CopyPlan {
    pub(crate) total_bytes: usize,
    pub(crate) chunk_bytes: usize,
    pub(crate) total_chunks: usize,
}

impl CopyPlan {
    pub(crate) fn new(total_bytes: usize, chunk_bytes: usize) -> CopyPlan {
        let chunk_bytes = chunk_bytes.max(1);
        let total_chunks = total_bytes.div_ceil(chunk_bytes);
        CopyPlan {
            total_bytes,
            chunk_bytes,
            total_chunks,
        }
    }

    /// Percent complete (0..=100), saturating and capped so the bar never
    /// exceeds its track even if extra chunks run.
    #[cfg(test)]
    pub(crate) fn progress_percent(&self, chunks_done: usize) -> u32 {
        progress_percent(chunks_done, self.total_chunks)
    }

    /// Byte range for chunk `index`, or None past the end.
    pub(crate) fn chunk_range(&self, index: usize) -> Option<(usize, usize)> {
        if index >= self.total_chunks {
            return None;
        }
        let start = index * self.chunk_bytes;
        let len = self.chunk_bytes.min(self.total_bytes - start);
        Some((start, len))
    }
}

/// Rejects names that cannot appear inside a directory listing.
pub(crate) fn validate_entry_name(name: &[u8]) -> OpResult<()> {
    if name.is_empty() {
        return Err(OpError::InvalidName);
    }
    if name.len() > NAME_MAX {
        return Err(OpError::TooLong);
    }
    if name == b"." || name == b".." || name.contains(&b'/') {
        return Err(OpError::InvalidName);
    }
    if name
        .iter()
        .any(|byte| !byte.is_ascii_graphic() && *byte != b' ')
    {
        return Err(OpError::InvalidName);
    }
    Ok(())
}

/// Joins `parent` + `name` into a namespace path; directories carry a
/// trailing slash like every directory entry in the explorer list.
pub(crate) fn compose_target(
    parent: &[u8],
    name: &[u8],
    kind: EntryKind,
) -> OpResult<(usize, [u8; MAX_STORAGE_PATH])> {
    validate_entry_name(name)?;
    let mut out = [0u8; MAX_STORAGE_PATH];
    let mut len = 0usize;
    for chunk in [parent, b"/", name] {
        if chunk == b"/" && parent.is_empty() {
            continue;
        }
        if len + chunk.len() > out.len() {
            return Err(OpError::TooLong);
        }
        out[len..len + chunk.len()].copy_from_slice(chunk);
        len += chunk.len();
    }
    if kind == EntryKind::Directory {
        if len + 1 > out.len() {
            return Err(OpError::TooLong);
        }
        out[len] = b'/';
        len += 1;
    }
    Ok((len, out))
}

fn split_stem_ext(base: &[u8]) -> (&[u8], &[u8]) {
    match base.iter().rposition(|byte| *byte == b'.') {
        Some(dot) if dot > 0 && dot + 1 < base.len() => (&base[..dot], &base[dot..]),
        _ => (base, b""),
    }
}

/// First free variant of `base` per `taken`: 0 keeps `base`, N means
/// "base (N)" (extension preserved). Err(Exists) when all variants
/// up to `UNIQUE_NAME_ATTEMPTS` collide.
pub(crate) fn next_available_name(
    base: &[u8],
    mut taken: impl FnMut(&[u8]) -> bool,
) -> OpResult<usize> {
    validate_entry_name(base)?;
    if !taken(base) {
        return Ok(0);
    }
    for variant in 2..=UNIQUE_NAME_ATTEMPTS + 1 {
        let mut scratch = [0u8; NAME_MAX];
        let len = variant_name(base, variant, &mut scratch)?;
        if !taken(&scratch[..len]) {
            return Ok(variant);
        }
    }
    Err(OpError::Exists)
}

/// Writes variant `variant` of `base` into `out` (0 = plain base,
/// N = "stem (N)" with any extension preserved), returning the length.
/// Returns TooLong when the result cannot fit `out`.
pub(crate) fn variant_name(
    base: &[u8],
    variant: usize,
    out: &mut [u8; NAME_MAX],
) -> OpResult<usize> {
    if variant == 0 {
        if base.len() > out.len() {
            return Err(OpError::TooLong);
        }
        out[..base.len()].copy_from_slice(base);
        return Ok(base.len());
    }
    let (stem, ext) = split_stem_ext(base);
    let mut digits = [0u8; 20];
    let digit_len = write_usize(variant, &mut digits);
    let total = stem.len() + 2 + digit_len + 1 + ext.len();
    if total > out.len() {
        return Err(OpError::TooLong);
    }
    let mut len = 0usize;
    out[len..len + stem.len()].copy_from_slice(stem);
    len += stem.len();
    out[len] = b' ';
    out[len + 1] = b'(';
    out[len + 2..len + 2 + digit_len].copy_from_slice(&digits[..digit_len]);
    len += 2 + digit_len;
    out[len] = b')';
    len += 1;
    out[len..len + ext.len()].copy_from_slice(ext);
    len += ext.len();
    Ok(len)
}

fn write_usize(mut value: usize, out: &mut [u8]) -> usize {
    let mut start = out.len();
    loop {
        start -= 1;
        out[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    let len = out.len() - start;
    out.copy_within(start.., 0);
    len
}

/// Maps a keyboard scancode to a prompt character (lowercase letters,
/// digits, dot, dash, equals, space); shift uppercases letters.
pub(crate) fn scancode_to_char(scancode: u32, modifiers: u32) -> Option<u8> {
    const SHIFT: u32 = crate::state::MOD_SHIFT;
    // Keyboard-order letter rows (linux/input-event-codes.h vocabulary).
    const ROWS: &[(u32, &str)] = &[
        (16, "qwertyuiop"), // 16..25
        (30, "asdfghjkl"),  // 30..38
        (44, "zxcvbnm"),    // 44..50
    ];
    let shifted = modifiers & SHIFT != 0;
    let character = match scancode {
        2..=10 => Some(b'1' + (scancode - 2) as u8),
        11 => Some(b'0'),
        12 => Some(if shifted { b'_' } else { b'-' }),
        13 => Some(b'='),
        52 => Some(b'.'),
        57 => Some(b' '),
        _ => ROWS.iter().find_map(|(first, letters)| {
            let offset = scancode.checked_sub(*first)? as usize;
            letters.as_bytes().get(offset).copied()
        }),
    };
    character
        .map(|byte| match (byte.is_ascii_lowercase(), shifted) {
            (true, true) => byte - b'a' + b'A',
            _ => byte,
        })
        .filter(|byte| byte.is_ascii_graphic() || *byte == b' ')
}

/// Appends one character to the prompt buffer, returning the new length
/// or None when the buffer is full.
pub(crate) fn prompt_push(buffer: &mut [u8], len: usize, byte: u8) -> Option<usize> {
    if len >= buffer.len() {
        return None;
    }
    buffer[len] = byte;
    Some(len + 1)
}

/// Progress tick for long copies: `chunks_done` of `total_chunks`.
#[derive(Clone, Copy)]
pub(crate) struct CopyProgress {
    pub(crate) chunks_done: usize,
    pub(crate) total_chunks: usize,
}

/// Percent complete (0..=100), saturating; an empty job counts as done.
pub(crate) fn progress_percent(chunks_done: usize, total_chunks: usize) -> u32 {
    if total_chunks == 0 {
        return 100;
    }
    let percent = (chunks_done.min(total_chunks) as u64 * 100) / total_chunks as u64;
    percent.min(100) as u32
}

pub(crate) type PathBuf = [u8; MAX_STORAGE_PATH];

fn path_text(path: &[u8]) -> OpResult<&str> {
    core::str::from_utf8(path).map_err(|_| OpError::InvalidName)
}

fn open_parent_writable(storage: rt::Handle, parent: &[u8]) -> Result<rt::Handle, OpError> {
    rt::storage_open_directory(storage, path_text(parent)?, true).map_err(OpError::from)
}

fn close_ignored(handle: rt::Handle) {
    let _ = rt::handle_close(handle);
}

/// Creates an empty file or directory named `name` inside `parent`.
pub(crate) fn create_entry(
    storage: rt::Handle,
    parent: &[u8],
    name: &[u8],
    kind: EntryKind,
) -> OpResult<()> {
    validate_entry_name(name)?;
    let entry_kind = match kind {
        EntryKind::Directory => rt::StorageEntryKind::Directory,
        _ => rt::StorageEntryKind::File,
    };
    let directory = open_parent_writable(storage, parent)?;
    let result =
        rt::storage_directory_create(directory, path_text(name)?, entry_kind);
    close_ignored(directory);
    result.map_err(OpError::from)
}

/// Deletes the file or (empty) directory `name` inside `parent`.
pub(crate) fn delete_entry(
    storage: rt::Handle,
    parent: &[u8],
    name: &[u8],
) -> OpResult<()> {
    validate_entry_name(name)?;
    let directory = open_parent_writable(storage, parent)?;
    let result = rt::storage_directory_remove(directory, path_text(name)?);
    close_ignored(directory);
    result.map_err(OpError::from)
}

/// Copies one file byte-exactly through chunked read/write contracts,
/// reporting progress per chunk. Returns bytes copied.
pub(crate) fn copy_file(
    storage: rt::Handle,
    src_path: &[u8],
    dst_parent: &[u8],
    dst_name: &[u8],
    progress: &mut dyn FnMut(CopyProgress),
) -> OpResult<usize> {
    validate_entry_name(dst_name)?;
    let (dst_len, dst_path) = compose_target(dst_parent, dst_name, EntryKind::File)?;
    let source = rt::storage_open(storage, path_text(src_path)?).map_err(OpError::from)?;
    let (src_blob, total_bytes) = source;

    // Guard against copying a file onto itself.
    if src_path.len() == dst_len && src_path[..] == dst_path[..dst_len] {
        close_ignored(src_blob);
        return Err(OpError::Exists);
    }

    let outcome =
        copy_into_new_file(storage, src_blob, total_bytes, dst_parent, dst_name, progress);
    close_ignored(src_blob);
    outcome
}

fn copy_into_new_file(
    storage: rt::Handle,
    src_blob: rt::Handle,
    total_bytes: usize,
    dst_parent: &[u8],
    dst_name: &[u8],
    progress: &mut dyn FnMut(CopyProgress),
) -> OpResult<usize> {
    let plan = CopyPlan::new(total_bytes, WRITE_CHUNK_MAX);
    let directory = open_parent_writable(storage, dst_parent)?;
    let opened =
        rt::storage_directory_open_file(directory, path_text(dst_name)?, true, true);
    close_ignored(directory);
    let (dst_blob, _) = opened.map_err(OpError::from)?;

    let mut chunk = [0u8; WRITE_CHUNK_MAX];
    let mut done = 0usize;
    let result = loop {
        match plan.chunk_range(done) {
            None => break Ok(total_bytes),
            Some((offset, len)) => {
                let read = rt::storage_read(src_blob, offset, &mut chunk[..len]);
                let written = read
                    .and_then(|_| {
                        rt::storage_write(dst_blob, offset, total_bytes, &chunk[..len])
                    });
                if let Err(error) = written {
                    break Err(OpError::from(error));
                }
                done += 1;
                progress(CopyProgress {
                    chunks_done: done,
                    total_chunks: plan.total_chunks,
                });
            }
        }
    };
    close_ignored(dst_blob);
    result
}

fn split_child<'a>(parent: &'a [u8], child: &'a [u8]) -> Option<&'a [u8]> {
    let name = child.get(parent.len()..)?;
    if name.ends_with(b"/") {
        return Some(&name[..name.len() - 1]);
    }
    Some(name)
}

/// Recursively copies a directory subtree (`src_path` ends in '/').
pub(crate) fn copy_tree(
    storage: rt::Handle,
    src_path: &[u8],
    dst_parent: &[u8],
    dst_name: &[u8],
    depth: usize,
    progress: &mut dyn FnMut(CopyProgress),
) -> OpResult<()> {
    if depth >= COPY_DEPTH_MAX {
        return Err(OpError::TooLong);
    }
    create_entry(storage, dst_parent, dst_name, EntryKind::Directory)?;
    let (dst_len, dst_root) = compose_target(dst_parent, dst_name, EntryKind::Directory)?;

    let mut cursor = 0usize;
    let mut buffer = [0u8; MAX_STORAGE_PATH];
    let mut child = [0u8; MAX_STORAGE_PATH];
    let outcome: OpResult<()> = loop {
        match rt::storage_list_directory(
            storage,
            path_text(src_path)?,
            cursor,
            &mut buffer,
        ) {
            Ok(Some((next_cursor, kind, path_len))) => {
                child[..path_len].copy_from_slice(&buffer[..path_len]);
                let child = &child[..path_len];
                let Some(name) = split_child(src_path, child) else {
                    break Ok(());
                };
                let mut owned_name = [0u8; NAME_MAX];
                if name.len() > NAME_MAX {
                    break Err(OpError::TooLong);
                }
                owned_name[..name.len()].copy_from_slice(name);
                let name = &owned_name[..name.len()];
                let copied = match kind {
                    rt::StorageEntryKind::Directory => copy_tree(
                        storage,
                        child,
                        &dst_root[..dst_len],
                        name,
                        depth + 1,
                        progress,
                    ),
                    rt::StorageEntryKind::File => copy_file(
                        storage,
                        child,
                        &dst_root[..dst_len],
                        name,
                        progress,
                    )
                    .map(|_| ()),
                };
                copied?;
                if next_cursor <= cursor {
                    break Ok(());
                }
                cursor = next_cursor;
            }
            Ok(None) => break Ok(()),
            Err(error) => break Err(OpError::from(error)),
        }
    };
    outcome
}

/// Parent/name decomposition of any namespace path (trailing slash on
/// directories is tolerated).
pub(crate) struct Segments {
    pub(crate) parent: PathBuf,
    pub(crate) parent_len: usize,
    pub(crate) name: [u8; NAME_MAX],
    pub(crate) name_len: usize,
}

pub(crate) fn split_segments(path: &[u8]) -> OpResult<Segments> {
    let trimmed = if path.ends_with(b"/") {
        &path[..path.len() - 1]
    } else {
        path
    };
    let slash = trimmed
        .iter()
        .rposition(|byte| *byte == b'/')
        .map(|index| index + 1)
        .unwrap_or(0);
    let name = &trimmed[slash..];
    validate_entry_name(name)?;
    if slash > MAX_STORAGE_PATH || slash == path.len() && path.len() > MAX_STORAGE_PATH {
        return Err(OpError::TooLong);
    }
    let mut segments = Segments {
        parent: [0u8; MAX_STORAGE_PATH],
        parent_len: slash,
        name: [0u8; NAME_MAX],
        name_len: name.len(),
    };
    segments.parent[..slash].copy_from_slice(&path[..slash]);
    segments.name[..name.len()].copy_from_slice(name);
    Ok(segments)
}

/// Deletes a directory subtree bottom-up (children first so the server's
/// non-empty guard never trips).
pub(crate) fn delete_tree(storage: rt::Handle, dir_path: &[u8], depth: usize) -> OpResult<()> {
    if depth >= COPY_DEPTH_MAX {
        return Err(OpError::TooLong);
    }
    let mut cursor = 0usize;
    let mut buffer = [0u8; MAX_STORAGE_PATH];
    let mut child = [0u8; MAX_STORAGE_PATH];
    let outcome: OpResult<()> = loop {
        match rt::storage_list_directory(
            storage,
            path_text(dir_path)?,
            cursor,
            &mut buffer,
        ) {
            Ok(Some((next_cursor, kind, path_len))) => {
                child[..path_len].copy_from_slice(&buffer[..path_len]);
                let child = &child[..path_len];
                let Some(name) = split_child(dir_path, child) else {
                    break Ok(());
                };
                let mut owned_name = [0u8; NAME_MAX];
                if name.len() > NAME_MAX {
                    break Err(OpError::TooLong);
                }
                owned_name[..name.len()].copy_from_slice(name);
                let segments = split_segments(child)?;
                let deleted = match kind {
                    rt::StorageEntryKind::Directory => delete_tree(storage, child, depth + 1),
                    rt::StorageEntryKind::File => Ok(()),
                }
                .and_then(|()| delete_entry(storage, &segments.parent[..segments.parent_len], &owned_name[..name.len()]));
                deleted?;
                if next_cursor <= cursor {
                    break Ok(());
                }
                cursor = next_cursor;
            }
            Ok(None) => break Ok(()),
            Err(error) => break Err(OpError::from(error)),
        }
    };
    outcome?;
    let segments = split_segments(dir_path)?;
    delete_entry(
        storage,
        &segments.parent[..segments.parent_len],
        &segments.name[..segments.name_len],
    )
}

/// Moves (or renames) an entry: the storage service applies the whole
/// change atomically on the wire (subtree rewrite for directories,
/// destination persistence adopted); oversized path pairs fall back to
/// chunked copy plus source deletion.
pub(crate) fn move_entry(
    storage: rt::Handle,
    kind: EntryKind,
    src_path: &[u8],
    dst_parent: &[u8],
    dst_name: &[u8],
    progress: &mut dyn FnMut(CopyProgress),
) -> OpResult<()> {
    let (dst_len, dst_path) = compose_target(dst_parent, dst_name, kind)?;
    match MovePlan::decide(rename_packs(src_path.len(), dst_len), kind) {
        MovePlan::Rename => {
            let dest = path_text(&dst_path[..dst_len])?;
            rt::storage_rename(storage, path_text(src_path)?, dest).map_err(rename_error)
        }
        MovePlan::CopyDelete => {
            copy_file(storage, src_path, dst_parent, dst_name, progress)?;
            let segments = split_segments(src_path)?;
            delete_entry(
                storage,
                &segments.parent[..segments.parent_len],
                &segments.name[..segments.name_len],
            )
        }
        MovePlan::CopyDeleteTree => {
            copy_tree(storage, src_path, dst_parent, dst_name, 0, progress)?;
            delete_tree(storage, src_path, 0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::MOD_SHIFT;

    #[test]
    fn friendly_error_covers_every_variant() {
        assert_eq!(friendly_error(OpError::Denied), "READ-ONLY OR NO PERMISSION");
        assert_eq!(friendly_error(OpError::Exists), "NAME ALREADY IN USE");
        assert_eq!(friendly_error(OpError::Busy), "TARGET BUSY OR DIR NOT EMPTY");
        assert_eq!(friendly_error(OpError::NotFound), "NOT FOUND");
        assert_eq!(friendly_error(OpError::InvalidName), "BAD NAME (A-Z, 0-9, DOT)");
        assert_eq!(friendly_error(OpError::TooLong), "NAME OR PATH TOO LONG");
        assert_eq!(friendly_error(OpError::Transport), "STORAGE ERROR");
        // Unmapped runtime statuses degrade to the generic storage error.
        assert_eq!(OpError::from(rt::Error::CapacityExceeded), OpError::Transport);
        assert_eq!(OpError::from(rt::Error::PermissionDenied), OpError::Denied);
    }

    #[test]
    fn entry_name_validation_rejects_empty_slash_dot_and_oversize() {
        assert_eq!(validate_entry_name(b"notes.txt"), Ok(()));
        assert_eq!(validate_entry_name(b"New Folder"), Ok(()));
        assert_eq!(validate_entry_name(b""), Err(OpError::InvalidName));
        assert_eq!(validate_entry_name(b"a/b"), Err(OpError::InvalidName));
        assert_eq!(validate_entry_name(b"."), Err(OpError::InvalidName));
        assert_eq!(validate_entry_name(b".."), Err(OpError::InvalidName));
        assert_eq!(
            validate_entry_name(&[b'a'; NAME_MAX + 1]),
            Err(OpError::TooLong)
        );
        assert_eq!(validate_entry_name(&[b'a'; NAME_MAX]), Ok(()));
    }

    #[test]
    fn compose_target_joins_parent_and_kind_suffix() {
        let (len, buf) = compose_target(b"home", b"notes.txt", EntryKind::File).expect("file");
        assert_eq!(&buf[..len], b"home/notes.txt");

        let (len, buf) = compose_target(b"home", b"docs", EntryKind::Directory).expect("dir");
        assert_eq!(&buf[..len], b"home/docs/");

        // Root parent has no leading slash.
        let (len, buf) = compose_target(b"", b"tmp", EntryKind::Directory).expect("root dir");
        assert_eq!(&buf[..len], b"tmp/");

        // Oversized joins are rejected, never truncated.
        let big_name = [b'a'; NAME_MAX];
        let big_parent = [b'b'; MAX_STORAGE_PATH - 2];
        assert_eq!(
            compose_target(&big_parent, &big_name, EntryKind::File),
            Err(OpError::TooLong)
        );
    }

    #[test]
    fn next_available_name_picks_lowest_free_variant_and_gives_up() {
        assert_eq!(next_available_name(b"notes.txt", |_| false).expect("free"), 0);
        let pick = next_available_name(b"notes.txt", |candidate| {
            !candidate.windows(3).any(|window| window == b"(4)")
        })
        .expect("variant 4 is free");
        assert_eq!(pick, 4);
        assert!(next_available_name(b"notes.txt", |_| true).is_err());
    }

    #[test]
    fn variant_name_keeps_extension_and_marks_folders_plainly() {
        let mut out = [0u8; NAME_MAX];
        let len = variant_name(b"notes.txt", 2, &mut out).expect("fits");
        assert_eq!(&out[..len], b"notes (2).txt");
        let len = variant_name(b"New Folder", 3, &mut out).expect("fits");
        assert_eq!(&out[..len], b"New Folder (3)");
        let len = variant_name(b"notes.txt", 0, &mut out).expect("fits");
        assert_eq!(&out[..len], b"notes.txt");
    }

    #[test]
    fn copy_plan_chunk_math_is_exact_and_bounded() {
        let plan = CopyPlan::new(0, WRITE_CHUNK_MAX);
        assert_eq!(plan.total_chunks, 0);
        assert_eq!(plan.progress_percent(0), 100);

        let plan = CopyPlan::new(WRITE_CHUNK_MAX + 1, WRITE_CHUNK_MAX);
        assert_eq!(plan.total_chunks, 2);
        assert_eq!(plan.chunk_range(0), Some((0, WRITE_CHUNK_MAX)));
        assert_eq!(plan.chunk_range(1), Some((WRITE_CHUNK_MAX, 1)));
        assert_eq!(plan.chunk_range(2), None);
        assert_eq!(plan.progress_percent(1), 50);
        assert_eq!(plan.progress_percent(9), 100);

        let plan = CopyPlan::new(300, 100);
        assert_eq!(plan.total_chunks, 3);
        assert_eq!(plan.progress_percent(2), 66);
    }

    #[test]
    fn move_plan_native_rename_covers_files_directories_and_parent_rows() {
        assert_eq!(MovePlan::decide(true, EntryKind::File), MovePlan::Rename);
        assert_eq!(
            MovePlan::decide(true, EntryKind::Directory),
            MovePlan::Rename
        );
        assert_eq!(MovePlan::decide(true, EntryKind::Parent), MovePlan::Rename);
    }

    #[test]
    fn move_plan_copy_delete_only_when_rename_cannot_pack() {
        assert_eq!(
            MovePlan::decide(false, EntryKind::File),
            MovePlan::CopyDelete
        );
        assert_eq!(
            MovePlan::decide(false, EntryKind::Parent),
            MovePlan::CopyDelete
        );
        assert_eq!(
            MovePlan::decide(false, EntryKind::Directory),
            MovePlan::CopyDeleteTree
        );
    }

    #[test]
    fn rename_packs_tracks_single_message_capacity_boundary() {
        assert!(rename_packs(0, 0));
        assert!(rename_packs(11, 14));
        assert!(rename_packs(96, 16));
        assert!(!rename_packs(96, 17));
        assert!(!rename_packs(96, 96));
    }

    #[test]
    fn rename_error_maps_collision_to_exists_and_keeps_other_mappings() {
        assert_eq!(rename_error(rt::Error::Busy), OpError::Exists);
        assert_eq!(rename_error(rt::Error::NotFound), OpError::NotFound);
        assert_eq!(
            rename_error(rt::Error::InvalidArgument),
            OpError::InvalidName
        );
        assert_eq!(rename_error(rt::Error::PermissionDenied), OpError::Denied);
        assert_eq!(rename_error(rt::Error::BufferTooSmall), OpError::TooLong);
        assert_eq!(
            rename_error(rt::Error::CapacityExceeded),
            OpError::Transport
        );
    }

    #[test]
    fn rename_wire_layout_packs_source_then_dest_with_kind_slashes() {
        let request = rt::storage_rename_request(b"home/note.txt", b"tmp/note.txt")
            .expect("file move request builds");
        assert_eq!(request.tag, rt::StorageTag::RenameRequest as u32);
        assert_eq!(request.word_count, 6);
        assert_eq!(request.words[0], 13);
        assert_eq!(request.words[1], 12);
        let mut source = [0u8; MAX_STORAGE_PATH];
        rt::unpack_bytes(&request.words[2..4], 13, &mut source).expect("source decodes");
        assert_eq!(&source[..13], b"home/note.txt");
        let mut dest = [0u8; MAX_STORAGE_PATH];
        rt::unpack_bytes(&request.words[4..6], 12, &mut dest).expect("dest decodes");
        assert_eq!(&dest[..12], b"tmp/note.txt");

        let request = rt::storage_rename_request(b"home/box/", b"tmp/box/")
            .expect("directory move request builds");
        assert_eq!(request.tag, rt::StorageTag::RenameRequest as u32);
        assert_eq!(request.word_count, 5);
        assert_eq!(request.words[0], 9);
        assert_eq!(request.words[1], 8);
        let mut dest = [0u8; MAX_STORAGE_PATH];
        rt::unpack_bytes(&request.words[4..5], 8, &mut dest).expect("dest decodes");
        assert_eq!(&dest[..8], b"tmp/box/");
    }

    #[test]
    fn scancode_map_covers_prompt_characters() {
        assert_eq!(scancode_to_char(30, 0), Some(b'a'));
        assert_eq!(scancode_to_char(30, MOD_SHIFT), Some(b'A'));
        assert_eq!(scancode_to_char(44, 0), Some(b'z'));
        assert_eq!(scancode_to_char(2, 0), Some(b'1'));
        assert_eq!(scancode_to_char(11, 0), Some(b'0'));
        assert_eq!(scancode_to_char(52, 0), Some(b'.'));
        assert_eq!(scancode_to_char(12, 0), Some(b'-'));
        assert_eq!(scancode_to_char(12, MOD_SHIFT), Some(b'_'));
        assert_eq!(scancode_to_char(57, 0), Some(b' '));
        assert_eq!(scancode_to_char(1, 0), None);
    }

    #[test]
    fn prompt_buffer_push_pop_respects_capacity() {
        let mut buffer = [0u8; NAME_MAX];
        let len = prompt_push(&mut buffer, 0, b'a').expect("push");
        assert_eq!(&buffer[..len], b"a");
        assert_eq!(prompt_push(&mut buffer, NAME_MAX, b'b'), None);
    }
}

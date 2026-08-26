//! Shell pipelines over real kernel pipe objects.
//!
//! `cmdA | cmdB` is executed by the shell, which is also where the stage
//! commands live (built-ins): there is no per-stage exec contract to attach
//! handles to — the spawn path (`ServiceSpawn`) transfers exactly one
//! bootstrap capability and stage commands are compiled into this binary,
//! not separate images. What changed is the data path: every stage boundary
//! now crosses a real kernel pipe object. The producing stage's captured
//! bytes are pushed through the pipe's writer handle with `pipe_write`
//! (blocking when the 64 KiB ring fills), the writer handle is closed so
//! readers observe EOF, and the consuming stage's input lines are drained
//! from the reader handle with `pipe_read` until that EOF arrives.
//!
//! Fallback: if the kernel does not answer the pipe syscalls (older kernel,
//! table full), [`feed_captured`] restores the previous shell-mediated
//! in-memory handoff and the caller announces the fallback loudly.
//!
//! Documented limits: at most [`MAX_PIPELINE_STAGES`] stages, no quoting or
//! redirection syntax, intermediate output clamped to [`MAX_CAPTURE_BYTES`]
//! with the truncation announced, streaming commands (`logs follow` and
//! friends) are unsuitable because the capturing shell never sees an end of
//! stream, and everything carried between stages is line-oriented UTF-8
//! text.

use core::cell::UnsafeCell;

use serviceos_userspace_runtime as rt;

use crate::util::ShellOutput;
use crate::jobs;

pub const MAX_PIPELINE_STAGES: usize = 4;
pub const MAX_CAPTURE_BYTES: usize = 2048;
pub const MAX_INPUT_LINES: usize = 64;
pub const INPUT_LINE_BYTES: usize = 128;

/// Failure modes of [`split_pipeline`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SplitError {
    EmptyLine,
    EmptyStage,
    TooManyStages,
}

/// Stage texts of one pipeline line, oldest stage first.
#[derive(Debug, PartialEq)]
pub struct PipelineStages<'a> {
    stages: [&'a str; MAX_PIPELINE_STAGES],
    pub count: usize,
}

impl<'a> PipelineStages<'a> {
    /// Stage text by position (None past the end).
    pub fn stage(&self, index: usize) -> Option<&'a str> {
        if index < self.count {
            self.stages.get(index).copied()
        } else {
            None
        }
    }
}

/// Split a command line on top-level `|` separators, trimming each stage.
/// There is no quoting: every `|` separates stages.
pub fn split_pipeline(line: &str) -> Result<PipelineStages<'_>, SplitError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err(SplitError::EmptyLine);
    }
    let mut stages = [""; MAX_PIPELINE_STAGES];
    let mut count = 0usize;
    for segment in trimmed.split('|') {
        let segment = segment.trim();
        if segment.is_empty() {
            return Err(SplitError::EmptyStage);
        }
        if count >= MAX_PIPELINE_STAGES {
            return Err(SplitError::TooManyStages);
        }
        stages[count] = segment;
        count += 1;
    }
    Ok(PipelineStages { stages, count })
}

/// Pure storage for the piped-input line list handed to consumer stages.
pub(crate) struct InputStore {
    active: bool,
    lens: [usize; MAX_INPUT_LINES],
    lines: [[u8; INPUT_LINE_BYTES]; MAX_INPUT_LINES],
}

impl InputStore {
    pub const fn new() -> Self {
        Self {
            active: false,
            lens: [0; MAX_INPUT_LINES],
            lines: [[0; INPUT_LINE_BYTES]; MAX_INPUT_LINES],
        }
    }

    /// Split `text` on newlines (tolerating `\r\n`), skipping empty lines,
    /// clamping long lines, stopping at capacity; returns stored count.
    pub fn push_text(&mut self, text: &str) -> usize {
        self.active = true;
        for raw in text.split('\n') {
            if self.len() >= MAX_INPUT_LINES {
                break;
            }
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            if line.is_empty() {
                continue;
            }
            let bytes = line.as_bytes();
            let len = bytes.len().min(INPUT_LINE_BYTES);
            let slot = self.len();
            self.lines[slot][..len].copy_from_slice(&bytes[..len]);
            self.lens[slot] = len;
        }
        self.len()
    }

    pub fn clear(&mut self) {
        *self = Self::new();
    }

    pub const fn len(&self) -> usize {
        let mut total = 0usize;
        let mut index = 0usize;
        while index < MAX_INPUT_LINES {
            if self.lens[index] > 0 {
                total += 1;
            }
            index += 1;
        }
        total
    }

    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Copy line `order` (oldest first) into `out`; returns its length.
    pub fn line(&self, order: usize, out: &mut [u8]) -> Option<usize> {
        let mut seen = 0usize;
        let mut slot = 0usize;
        while slot < MAX_INPUT_LINES {
            if self.lens[slot] > 0 {
                if seen == order {
                    let len = self.lens[slot].min(out.len());
                    out[..len].copy_from_slice(&self.lines[slot][..len]);
                    return Some(len);
                }
                seen += 1;
            }
            slot += 1;
        }
        None
    }
}

/// Value returned by [`capture_finish_scratch`]: the captured bytes plus a
/// sticky truncation flag.
pub struct CaptureOut {
    pub len: usize,
    pub buf: [u8; MAX_CAPTURE_BYTES],
    pub truncated: bool,
}

impl CaptureOut {
    pub fn as_text(&self) -> Option<&str> {
        core::str::from_utf8(&self.buf[..self.len]).ok()
    }
}

/// Pure bounded capture buffer used for intermediate pipeline stages.
pub(crate) struct CaptureBuf {
    len: usize,
    truncated: bool,
    buf: [u8; MAX_CAPTURE_BYTES],
}

impl CaptureBuf {
    pub const fn new() -> Self {
        Self {
            len: 0,
            truncated: false,
            buf: [0; MAX_CAPTURE_BYTES],
        }
    }

    pub fn reset(&mut self) {
        self.len = 0;
        self.truncated = false;
    }

    pub fn push(&mut self, bytes: &[u8]) {
        let room = MAX_CAPTURE_BYTES - self.len;
        if bytes.len() > room {
            self.buf[self.len..].copy_from_slice(&bytes[..room]);
            self.len = MAX_CAPTURE_BYTES;
            self.truncated = true;
        } else {
            self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
            self.len += bytes.len();
        }
    }

    pub fn finish(&self) -> CaptureOut {
        CaptureOut {
            len: self.len,
            buf: self.buf,
            truncated: self.truncated,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum CaptureTarget {
    Off,
    Scratch,
    Job(u32),
}

struct TargetSlot(UnsafeCell<CaptureTarget>);
unsafe impl Sync for TargetSlot {}
static CAPTURE_TARGET: TargetSlot = TargetSlot(UnsafeCell::new(CaptureTarget::Off));
static RESTORED_TARGET: TargetSlot = TargetSlot(UnsafeCell::new(CaptureTarget::Off));

struct ScratchSlot(UnsafeCell<CaptureBuf>);
unsafe impl Sync for ScratchSlot {}
static SCRATCH: ScratchSlot = ScratchSlot(UnsafeCell::new(CaptureBuf::new()));

struct InputSlot(UnsafeCell<InputStore>);
unsafe impl Sync for InputSlot {}
static INPUT: InputSlot = InputSlot(UnsafeCell::new(InputStore::new()));

fn capture_target() -> CaptureTarget {
    // SAFETY: the shell task is strictly single-threaded (sessions-slot
    // precedent); no concurrent access is possible.
    unsafe { *CAPTURE_TARGET.0.get() }
}

fn set_capture_target(target: CaptureTarget) {
    // SAFETY: see `capture_target`.
    unsafe {
        *CAPTURE_TARGET.0.get() = target;
    }
}

fn scratch() -> &'static mut CaptureBuf {
    // SAFETY: see `capture_target`.
    unsafe { &mut *SCRATCH.0.get() }
}

fn input() -> &'static mut InputStore {
    // SAFETY: see `capture_target`.
    unsafe { &mut *INPUT.0.get() }
}

/// Begin routing captured output into the per-job retained buffer.
pub fn capture_begin_job(job_id: u32) {
    // SAFETY: see `capture_target`.
    unsafe {
        *RESTORED_TARGET.0.get() = capture_target();
    }
    set_capture_target(CaptureTarget::Job(job_id));
}

/// Begin routing captured output into the one-stage scratch buffer.
pub fn capture_begin_scratch() {
    // SAFETY: see `capture_target`.
    unsafe {
        *RESTORED_TARGET.0.get() = capture_target();
    }
    scratch().reset();
    set_capture_target(CaptureTarget::Scratch);
}

/// Stop capture entirely (used after a background job finishes).
pub fn capture_end() {
    set_capture_target(CaptureTarget::Off);
}

/// Finish a scratch capture: returns the buffered bytes and restores the
/// previous capture target.
pub fn capture_finish_scratch() -> CaptureOut {
    let out = scratch().finish();
    // SAFETY: see `capture_target`.
    let restored = unsafe { *RESTORED_TARGET.0.get() };
    set_capture_target(restored);
    out
}

/// Feed captured bytes into the piped-input store; returns line count.
pub fn feed_captured(captured: &CaptureOut) -> usize {
    input().clear();
    match captured.as_text() {
        Some(text) => input().push_text(text),
        None => 0,
    }
}

/// Push the captured bytes of one stage boundary through a real kernel pipe
/// object and drain them into the piped-input store.
///
/// Sequence: create the pipe, write every byte into the writer handle
/// (looping over partial writes; a blocking write parks the shell thread on
/// the pipe's wait queue when the ring is full), close the writer so the
/// reader side reaches EOF, then read until `pipe_read` reports that EOF,
/// feeding the bytes into the line store on the way.
///
/// Returns the stored line count, or the runtime error if the kernel could
/// not service the pipe syscalls (caller falls back to [`feed_captured`]).
pub fn feed_captured_via_kernel_pipe(captured: &CaptureOut) -> rt::Result<usize> {
    let (reader, writer) = rt::pipe_create()?;

    let data = &captured.buf[..captured.len];
    let mut pushed = 0usize;
    while pushed < data.len() {
        match rt::pipe_write(writer, &data[pushed..], false) {
            Ok(0) => break,
            Ok(count) => pushed += count,
            Err(rt::Error::BrokenPipe) => break,
            Err(error) => {
                let _ = rt::handle_close(writer);
                let _ = rt::handle_close(reader);
                return Err(error);
            }
        }
    }

    // Closing the last writer handle is what flips the reader side to EOF;
    // without this the drain loop below would block forever.
    if let Err(error) = rt::handle_close(writer) {
        let _ = rt::handle_close(reader);
        return Err(error);
    }

    let mut collected = [0u8; MAX_CAPTURE_BYTES];
    let mut filled = 0usize;
    input().clear();
    loop {
        let mut chunk = [0u8; 128];
        match rt::pipe_read(reader, &mut chunk, false) {
            Ok(0) => break,
            Ok(count) => {
                let room = collected.len() - filled;
                let copy = count.min(room);
                collected[filled..filled + copy].copy_from_slice(&chunk[..copy]);
                filled += copy;
                if room == 0 {
                    break;
                }
            }
            Err(error) => {
                let _ = rt::handle_close(reader);
                return Err(error);
            }
        }
    }
    let closed = rt::handle_close(reader);

    match core::str::from_utf8(&collected[..filled]) {
        Ok(text) => {
            input().push_text(text);
            let _ = closed;
            Ok(input().len())
        }
        Err(_) => {
            let _ = closed;
            Ok(0)
        }
    }
}

/// Writer targeting whichever capture sink is armed (scratch buffer or job
/// row); writes land nowhere while capture is off.
pub(crate) fn capture_write(_handle: rt::Handle, text: &str) -> rt::Result<()> {
    match capture_target() {
        CaptureTarget::Off => Ok(()),
        CaptureTarget::Scratch => {
            scratch().push(text.as_bytes());
            Ok(())
        }
        CaptureTarget::Job(job_id) => {
            jobs::append_output(job_id, text.as_bytes());
            Ok(())
        }
    }
}

/// Output handle whose writes go to the armed capture sink.
pub fn capturing_output() -> ShellOutput {
    ShellOutput::new(0, capture_write)
}

pub fn clear_input() {
    input().clear();
}

pub fn input_active() -> bool {
    input().is_active()
}

pub fn input_count() -> usize {
    input().len()
}

pub fn input_line(order: usize, out: &mut [u8]) -> Option<usize> {
    input().line(order, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_stages_and_trims_each_segment() {
        let plan = split_pipeline("status services | count").unwrap();
        assert_eq!(plan.count, 2);
        assert_eq!(plan.stage(0), Some("status services"));
        assert_eq!(plan.stage(1), Some("count"));
        assert_eq!(plan.stage(2), None);

        let spaced = split_pipeline("  a  b  |  c | d e  ").unwrap();
        assert_eq!(spaced.count, 3);
        assert_eq!(spaced.stage(0), Some("a  b"));
        assert_eq!(spaced.stage(2), Some("d e"));
    }

    #[test]
    fn single_stage_lines_are_not_pipelines_but_still_split_cleanly() {
        let plan = split_pipeline("help").unwrap();
        assert_eq!(plan.count, 1);
        assert_eq!(plan.stage(0), Some("help"));
    }

    #[test]
    fn rejects_empty_stages_overflow_and_blank_lines() {
        assert_eq!(split_pipeline(""), Err(SplitError::EmptyLine));
        assert_eq!(split_pipeline("   "), Err(SplitError::EmptyLine));
        assert_eq!(split_pipeline("a || b"), Err(SplitError::EmptyStage));
        assert_eq!(split_pipeline("|a"), Err(SplitError::EmptyStage));
        assert_eq!(split_pipeline("a|"), Err(SplitError::EmptyStage));
        let overflow = split_pipeline("a|b|c|d|e").unwrap_err();
        assert_eq!(overflow, SplitError::TooManyStages);
        let maxed = split_pipeline("a|b|c|d").unwrap();
        assert_eq!(maxed.count, MAX_PIPELINE_STAGES);
    }

    #[test]
    fn input_store_roundtrips_lines_skipping_empties() {
        let mut store = InputStore::new();
        assert!(!store.is_active());
        assert_eq!(store.push_text("alpha\r\nbeta\n\ngamma\n"), 3);
        assert!(store.is_active());
        let mut buffer = [0u8; INPUT_LINE_BYTES];
        assert_eq!(store.line(0, &mut buffer), Some(5));
        assert_eq!(&buffer[..5], b"alpha");
        assert_eq!(store.line(2, &mut buffer), Some(5));
        assert_eq!(&buffer[..5], b"gamma");
        assert_eq!(store.line(3, &mut buffer), None);
        store.clear();
        assert_eq!(store.len(), 0);
        assert!(!store.is_active());
    }

    #[test]
    fn input_store_clamps_long_lines_and_caps_capacity() {
        let mut store = InputStore::new();
        let long = "x".repeat(INPUT_LINE_BYTES + 16);
        let mut text = long;
        for index in 0..MAX_INPUT_LINES + 4 {
            text.push_str(&format!("\nline-{index}"));
        }
        let stored = store.push_text(&text);
        assert_eq!(stored, MAX_INPUT_LINES);
        let mut buffer = [0u8; INPUT_LINE_BYTES + 8];
        let len = store.line(0, &mut buffer).unwrap();
        assert_eq!(len, INPUT_LINE_BYTES);
        assert!(buffer.iter().take(len).all(|byte| *byte == b'x'));
        assert_eq!(store.line(MAX_INPUT_LINES, &mut buffer), None);
        // Slot 0 holds the clamped long line; slots 1.. hold line-0.. so the
        // final stored line is line-<capacity - 2>.
        let tail_label = format!("line-{}", MAX_INPUT_LINES - 2);
        let tail = store.line(MAX_INPUT_LINES - 1, &mut buffer).unwrap();
        assert_eq!(&buffer[..tail], tail_label.as_bytes());
    }

    #[test]
    fn capture_buf_bounds_and_reports_truncation_once() {
        let mut capture = CaptureBuf::new();
        capture.push(b"hello ");
        capture.push(b"world");
        let out = capture.finish();
        assert_eq!(out.len, 11);
        assert!(!out.truncated);
        assert_eq!(out.as_text(), Some("hello world"));

        capture.reset();
        capture.push(&[b'y'; MAX_CAPTURE_BYTES]);
        capture.push(b"overflow");
        let out = capture.finish();
        assert_eq!(out.len, MAX_CAPTURE_BYTES);
        assert!(out.truncated);
        assert!(out.as_text().is_some());
    }
}

//! Background job table for the shell.
//!
//! Honest model: the shell is a single-threaded event loop and its built-in
//! commands are synchronous service round-trips, so `command &` does not
//! create a concurrent process — it queues a job row and returns a job id
//! immediately. The main loop drains queued jobs one per tick through the
//! ordinary execution path with output captured into the row instead of the
//! terminal. `jobs` lists id/state/pending-output bytes, `fg <id>` streams
//! the retained output into the requesting session, and a completed job
//! announces its exit status once when the session next submits a line.
//!
//! Everything here except the thin static accessors at the bottom is pure
//! bookkeeping so host tests cover the state machine without a kernel.

use core::cell::UnsafeCell;

use serviceos_userspace_runtime as rt;

use crate::util::{ShellOutput, write_output_linef};

pub const MAX_JOBS: usize = 8;
pub const JOB_CMD_BYTES: usize = 128;
pub const JOB_OUTPUT_BYTES: usize = 1024;
pub const JOB_ERROR_BYTES: usize = 24;

/// Lifecycle of one background job row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobState {
    /// Queued or executing on the shell event loop; output still accruing.
    Running,
    /// Execution finished; exit status recorded, output retained.
    Done,
}

impl JobState {
    pub const fn name(self) -> &'static str {
        match self {
            JobState::Running => "running",
            JobState::Done => "done",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpawnError {
    TableFull,
}

#[derive(Clone, Copy)]
pub struct JobRow {
    pub id: u32,
    cmd_len: usize,
    cmd: [u8; JOB_CMD_BYTES],
    pub state: JobState,
    exit_ok: bool,
    err_len: usize,
    err: [u8; JOB_ERROR_BYTES],
    out_len: usize,
    out: [u8; JOB_OUTPUT_BYTES],
    out_truncated: bool,
    truncation_reported: bool,
    done_reported: bool,
    occupied: bool,
}

impl JobRow {
    const fn empty(id: u32) -> Self {
        Self {
            id,
            cmd_len: 0,
            cmd: [0; JOB_CMD_BYTES],
            state: JobState::Running,
            exit_ok: false,
            err_len: 0,
            err: [0; JOB_ERROR_BYTES],
            out_len: 0,
            out: [0; JOB_OUTPUT_BYTES],
            out_truncated: false,
            truncation_reported: false,
            done_reported: false,
            occupied: false,
        }
    }

    /// Borrow the command text (empty string when absent).
    pub fn cmd_text<'a>(&self, buffer: &'a mut [u8]) -> &'a str {
        let len = self.cmd_len.min(buffer.len());
        buffer[..len].copy_from_slice(&self.cmd[..len]);
        core::str::from_utf8(&buffer[..len]).unwrap_or("")
    }

    /// Borrow the recorded error name (empty when the exit was ok).
    pub fn error_text<'a>(&self, buffer: &'a mut [u8]) -> &'a str {
        let len = self.err_len.min(buffer.len());
        buffer[..len].copy_from_slice(&self.err[..len]);
        core::str::from_utf8(&buffer[..len]).unwrap_or("")
    }

    pub const fn exited_ok(&self) -> bool {
        self.exit_ok
    }

    /// Bytes of retained-but-undrained output (for the jobs listing).
    pub const fn output_len(&self) -> usize {
        self.out_len
    }

    /// Human-readable exit status suffix used by `fg` and done notices.
    pub fn status_text<'a>(&self, buffer: &'a mut [u8]) -> &'a str {
        if self.exit_ok {
            "exit ok"
        } else {
            let name = self.error_text(buffer);
            if name.is_empty() {
                "exit failed"
            } else {
                return name;
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct JobTable {
    rows: [JobRow; MAX_JOBS],
    next_id: u32,
}

impl JobTable {
    pub const fn new() -> Self {
        Self {
            rows: [JobRow::empty(0); MAX_JOBS],
            next_id: 1,
        }
    }

    /// Queue one command line as a new running job; ids are monotonic and
    /// never reused. When the table is full, fully-reported done jobs are
    /// evicted oldest-id-first to make room.
    pub fn spawn(&mut self, cmd: &str) -> Result<u32, SpawnError> {
        let slot = match self.free_slot() {
            Some(slot) => slot,
            None => self
                .evict_reported_done_slot()
                .ok_or(SpawnError::TableFull)?,
        };
        let bytes = cmd.as_bytes();
        let len = bytes.len().min(JOB_CMD_BYTES);
        let id = self.next_id;
        self.rows[slot] = JobRow::empty(id);
        self.rows[slot].occupied = true;
        self.rows[slot].cmd_len = len;
        self.rows[slot].cmd[..len].copy_from_slice(&bytes[..len]);
        self.next_id += 1;
        Ok(id)
    }

    fn free_slot(&self) -> Option<usize> {
        self.rows.iter().position(|row| !row.occupied)
    }

    fn evict_reported_done_slot(&self) -> Option<usize> {
        let mut best: Option<(u32, usize)> = None;
        for (index, row) in self.rows.iter().enumerate() {
            if row.occupied && row.state == JobState::Done && row.done_reported {
                match best {
                    Some((oldest, _)) if row.id >= oldest => {}
                    _ => best = Some((row.id, index)),
                }
            }
        }
        best.map(|(_, index)| index)
    }

    fn position(&self, id: u32) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| row.occupied && row.id == id)
    }

    pub fn get(&self, id: u32) -> Option<&JobRow> {
        self.position(id).map(|index| &self.rows[index])
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut JobRow> {
        let index = self.position(id)?;
        Some(&mut self.rows[index])
    }

    pub fn remove(&mut self, id: u32) -> bool {
        match self.position(id) {
            Some(index) => {
                self.rows[index] = JobRow::empty(0);
                true
            }
            None => false,
        }
    }

    pub fn count(&self) -> usize {
        self.rows.iter().filter(|row| row.occupied).count()
    }

    /// Record completion of a job's execution.
    pub fn mark_done(&mut self, id: u32, ok: bool, error_name: &str) -> bool {
        let Some(row) = self.get_mut(id) else {
            return false;
        };
        row.state = JobState::Done;
        row.exit_ok = ok;
        let bytes = error_name.as_bytes();
        row.err_len = bytes.len().min(JOB_ERROR_BYTES);
        row.err[..row.err_len].copy_from_slice(&bytes[..row.err_len]);
        true
    }

    /// Append produced output into the row's retained buffer; overflows set
    /// a sticky truncation flag instead of failing.
    pub fn append_output(&mut self, id: u32, bytes: &[u8]) -> bool {
        let Some(row) = self.get_mut(id) else {
            return false;
        };
        let room = JOB_OUTPUT_BYTES - row.out_len;
        if bytes.len() > room {
            row.out[row.out_len..].copy_from_slice(&bytes[..room]);
            row.out_len = JOB_OUTPUT_BYTES;
            row.out_truncated = true;
        } else {
            row.out[row.out_len..row.out_len + bytes.len()].copy_from_slice(bytes);
            row.out_len += bytes.len();
        }
        true
    }

    /// Drain retained output into `out` (fg semantics). Returns the drained
    /// length plus whether previously-unreported truncation happened (the
    /// flag is cleared once reported).
    pub fn take_output(&mut self, id: u32, out: &mut [u8]) -> Option<(usize, bool)> {
        let index = self.position(id)?;
        let row = &mut self.rows[index];
        let len = row.out_len.min(out.len());
        out[..len].copy_from_slice(&row.out[..len]);
        let remaining = row.out_len - len;
        row.out.copy_within(len.., 0);
        row.out_len = remaining;
        let truncated = row.out_truncated && !row.truncation_reported;
        if truncated {
            row.truncation_reported = true;
        }
        Some((len, truncated))
    }

    /// Oldest still-running job id, for the event-loop poller.
    pub fn next_running(&self) -> Option<u32> {
        let mut best: Option<u32> = None;
        for row in &self.rows {
            if row.occupied && row.state == JobState::Running {
                match best {
                    Some(oldest) if row.id >= oldest => {}
                    _ => best = Some(row.id),
                }
            }
        }
        best
    }

    /// Visit occupied rows in slot order.
    pub fn for_each<F: FnMut(&JobRow)>(&self, mut visit: F) {
        for row in &self.rows {
            if row.occupied {
                visit(row);
            }
        }
    }

    /// Collect ids of done jobs whose exit status has not been announced.
    pub fn pending_notices(&self, ids: &mut [u32]) -> usize {
        let mut count = 0usize;
        for row in &self.rows {
            if row.occupied
                && row.state == JobState::Done
                && !row.done_reported
                && count < ids.len()
            {
                ids[count] = row.id;
                count += 1;
            }
        }
        count
    }

    pub fn mark_reported(&mut self, id: u32) -> bool {
        match self.get_mut(id) {
            Some(row) => {
                row.done_reported = true;
                true
            }
            None => false,
        }
    }
}

struct TableSlot(UnsafeCell<JobTable>);
unsafe impl Sync for TableSlot {}
static JOB_TABLE: TableSlot = TableSlot(UnsafeCell::new(JobTable::new()));

fn table() -> &'static mut JobTable {
    // SAFETY: the shell task is strictly single-threaded (sessions-slot
    // precedent); no concurrent access is possible.
    unsafe { &mut *JOB_TABLE.0.get() }
}

pub fn spawn_job(cmd: &str) -> Result<u32, SpawnError> {
    table().spawn(cmd)
}

pub fn job_state(id: u32) -> Option<JobState> {
    table().get(id).map(|row| row.state)
}

pub fn job_mark_done_ok(id: u32) -> bool {
    table().mark_done(id, true, "")
}

pub fn job_mark_done_err(id: u32, error_name: &str) -> bool {
    table().mark_done(id, false, error_name)
}

pub fn job_mark_reported(id: u32) -> bool {
    table().mark_reported(id)
}

pub(crate) fn append_output(id: u32, bytes: &[u8]) -> bool {
    table().append_output(id, bytes)
}

pub fn job_take_output(id: u32, out: &mut [u8]) -> Option<(usize, bool)> {
    table().take_output(id, out)
}

pub fn job_remove(id: u32) -> bool {
    table().remove(id)
}

pub fn next_running_job_id() -> Option<u32> {
    table().next_running()
}

/// Exit status of one job (None when the row is gone).
pub fn job_exit_ok(id: u32) -> Option<bool> {
    table().get(id).map(|row| row.exited_ok())
}

/// Copy one job's recorded error name into `out`; returns its length.
pub fn job_error_copy(id: u32, out: &mut [u8]) -> Option<usize> {
    let row = table().get(id)?;
    let len = row.err_len.min(out.len());
    out[..len].copy_from_slice(&row.err[..len]);
    Some(len)
}

pub fn job_for_each<F: FnMut(&JobRow)>(visit: F) {
    table().for_each(visit);
}

/// Copy one job's command text into `out`; returns its length.
pub fn job_cmd_copy(id: u32, out: &mut [u8]) -> Option<usize> {
    let row = table().get(id)?;
    let len = row.cmd_len.min(out.len());
    out[..len].copy_from_slice(&row.cmd[..len]);
    Some(len)
}

/// Write `[id]+ done <cmd> (<status>)` notices for every completed-but-
/// unannounced job; used by the session path so statuses surface on the
/// next prompt interaction.
pub fn flush_done_reports(output: ShellOutput) -> rt::Result<()> {
    let mut ids = [0u32; MAX_JOBS];
    let count = table().pending_notices(&mut ids);
    for &id in &ids[..count] {
        let mut cmd = [0u8; JOB_CMD_BYTES];
        let cmd_text = match table().get(id) {
            Some(row) => row.cmd_text(&mut cmd),
            None => continue,
        };
        write_output_linef(output, format_args!("[{id}]+ done {cmd_text}"))?;
        let mut err = [0u8; JOB_ERROR_BYTES];
        let (exited_ok, error_text) = match table().get(id) {
            Some(row) => (row.exited_ok(), row.error_text(&mut err)),
            None => continue,
        };
        if exited_ok {
            write_output_linef(output, format_args!("    exit ok"))?;
        } else {
            write_output_linef(output, format_args!("    exit failed: {error_text}"))?;
        }
        table().mark_reported(id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_mints_monotonic_ids_and_keeps_command_text() {
        let mut table = JobTable::new();
        let first = table.spawn("config &").unwrap();
        let second = table.spawn("logs | count").unwrap();
        assert_eq!(first, 1);
        assert_eq!(second, 2);
        let mut buffer = [0u8; JOB_CMD_BYTES];
        assert_eq!(
            table.get(second).unwrap().cmd_text(&mut buffer),
            "logs | count"
        );
        assert_eq!(table.get(first).unwrap().state, JobState::Running);
    }

    #[test]
    fn long_commands_are_clamped_into_the_row() {
        let mut table = JobTable::new();
        let cmd = "x".repeat(JOB_CMD_BYTES + 32);
        let id = table.spawn(&cmd).unwrap();
        let mut buffer = [0u8; JOB_CMD_BYTES + 8];
        let text = table.get(id).unwrap().cmd_text(&mut buffer);
        assert_eq!(text.len(), JOB_CMD_BYTES);
    }

    #[test]
    fn state_machine_runs_then_completes_with_status() {
        let mut table = JobTable::new();
        let id = table.spawn("status").unwrap();
        assert!(table.mark_done(id, false, "InvalidArgument"));
        let row = table.get(id).unwrap();
        assert_eq!(row.state, JobState::Done);
        assert!(!row.exited_ok());
        let mut err = [0u8; JOB_ERROR_BYTES];
        assert_eq!(row.error_text(&mut err), "InvalidArgument");
        assert!(table.mark_done(id, true, ""));
        assert!(table.get(id).unwrap().exited_ok());
        assert!(!table.mark_done(999, true, ""));
    }

    #[test]
    fn output_appends_bound_and_drain_reports_truncation_once() {
        let mut table = JobTable::new();
        let id = table.spawn("big").unwrap();
        // Three 400-byte appends overflow the 1024-byte row buffer partway
        // through the third; a later append has no room left at all.
        let block = [b'a'; 400];
        table.append_output(id, &block);
        table.append_output(id, &block);
        assert!(table.append_output(id, &block));
        assert!(table.append_output(id, b"tail"));
        let mut out = [0u8; JOB_OUTPUT_BYTES * 2];
        let (len, truncated) = table.take_output(id, &mut out).unwrap();
        assert_eq!(len, JOB_OUTPUT_BYTES);
        assert!(truncated, "overflow must be reported exactly once");
        assert_eq!(&out[len - 1..len], b"a", "overflowing bytes are dropped");
        let (rest, truncated_again) = table.take_output(id, &mut out[..64]).unwrap();
        assert_eq!(rest, 0);
        assert!(!truncated_again, "truncation must not re-report");
    }

    #[test]
    fn fg_of_completed_job_removes_the_row() {
        let mut table = JobTable::new();
        let id = table.spawn("cat config.cfg").unwrap();
        table.mark_done(id, true, "");
        let mut out = [0u8; JOB_OUTPUT_BYTES];
        assert!(table.take_output(id, &mut out).is_some());
        assert!(table.remove(id));
        assert!(table.get(id).is_none());
        assert!(!table.remove(id));
    }

    #[test]
    fn capacity_evicts_only_oldest_reported_done_job() {
        let mut table = JobTable::new();
        let mut ids = [0u32; MAX_JOBS];
        for index in 0..MAX_JOBS {
            ids[index] = table.spawn("job").unwrap();
        }
        assert_eq!(
            table.spawn("overflow"),
            Err(SpawnError::TableFull),
            "unreported done jobs must not be evicted"
        );
        table.mark_done(ids[0], true, "");
        assert_eq!(table.spawn("still-full"), Err(SpawnError::TableFull));
        table.mark_reported(ids[0]);
        let reclaimed = table.spawn("fits-now").unwrap();
        assert!(reclaimed > ids[MAX_JOBS - 1]);
        assert!(table.get(ids[0]).is_none(), "oldest reported-done evicted");
        assert!(table.get(ids[1]).is_some());
        assert_eq!(table.count(), MAX_JOBS);
    }

    #[test]
    fn poller_picks_oldest_running_and_notices_fire_once() {
        let mut table = JobTable::new();
        let older = table.spawn("one").unwrap();
        let newer = table.spawn("two").unwrap();
        assert_eq!(table.next_running(), Some(older));
        table.mark_done(older, true, "");
        assert_eq!(table.next_running(), Some(newer));
        table.mark_done(newer, true, "");
        assert_eq!(table.next_running(), None);

        let mut ids = [0u32; MAX_JOBS];
        assert_eq!(table.pending_notices(&mut ids), 2);
        assert_eq!(&ids[..2], &[older, newer]);
        table.mark_reported(older);
        let mut remaining = [0u32; MAX_JOBS];
        assert_eq!(table.pending_notices(&mut remaining), 1);
        assert_eq!(remaining[0], newer);
        table.mark_reported(newer);
        assert_eq!(table.pending_notices(&mut remaining), 0);
    }
}

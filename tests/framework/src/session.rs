//! SerialSession: spawn a QEMU process from a [`crate::qemu::QemuSpec`],
//! pump stdout+stderr into a shared buffer on reader threads, keep stdin
//! open for scripted injection (shell typing, 0x03 interrupts), and expose
//! witness waits, tail diagnostics, and kill-based teardown. Extends the
//! `bootlog.rs::run_bounded` pattern with an idle watchdog separate from the
//! total budget, condvar-driven wakeups, and hermetic env replay.

use std::{
    collections::VecDeque,
    io::Read,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

#[derive(Debug)]
pub enum SessionError {
    Spawn(std::io::Error),
    Io(std::io::Error),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(error) => write!(f, "failed to spawn QEMU: {error}"),
            Self::Io(error) => write!(f, "session I/O failure: {error}"),
        }
    }
}

impl std::error::Error for SessionError {}

/// Terminal conditions surfaced through frozen-API waits.
#[derive(Debug)]
pub enum WaitOutcome {
    DeadlineExceeded,
    IdleStalled(Duration),
    GuestExited(String),
}

impl std::fmt::Display for WaitOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeadlineExceeded => f.write_str("case timeout exceeded"),
            Self::IdleStalled(duration) => write!(f, "no serial output for {duration:?}"),
            Self::GuestExited(status) => write!(f, "guest exited early: {status}"),
        }
    }
}

pub const DEFAULT_TAIL_LINES: usize = 400;

struct Shared {
    text: Mutex<String>,
    lines: Mutex<VecDeque<String>>,
    last_activity: Mutex<Instant>,
    /// Some(detail) once the guest's end state is known (reap or reader EOF).
    exit_status: Mutex<Option<String>>,
    readers_alive: AtomicUsize,
    child: Mutex<Option<Child>>,
}

impl Shared {
    fn new(readers: usize) -> Arc<Self> {
        Arc::new(Self {
            text: Mutex::new(String::new()),
            lines: Mutex::new(VecDeque::new()),
            last_activity: Mutex::new(Instant::now()),
            exit_status: Mutex::new(None),
            readers_alive: AtomicUsize::new(readers),
            child: Mutex::new(None),
        })
    }
    fn push_line(&self, line: &str) {
        {
            let mut lines = locked(&self.lines);
            lines.push_back(line.to_owned());
        }
        let mut text = locked(&self.text);
        text.push_str(line);
        text.push('\n');
        drop(text);
        *locked(&self.last_activity) = Instant::now();
    }

    fn note_reader_eof(&self) {
        if self.readers_alive.fetch_sub(1, Ordering::SeqCst) == 1 {
            // Both pipes drained => the process ended; reap whatever the
            // kernel knows so waiters see a definitive verdict immediately.
            let mut child_slot = locked(&self.child);
            if let Some(child) = child_slot.as_mut() {
                if let Ok(Some(status)) = child.try_wait() {
                    *locked(&self.exit_status) = Some(exit_detail(status));
                } else {
                    *locked(&self.exit_status) =
                        Some("stream EOF with unreaped child".to_owned());
                }
            } else {
                *locked(&self.exit_status) =
                    Some("stream EOF with detached child".to_owned());
            }
        }
    }

    fn record_exit(&self, detail: String) {
        let mut slot = locked(&self.exit_status);
        if slot.is_none() {
            *slot = Some(detail);
        }
    }

    fn activity(&self) -> Instant {
        *locked(&self.last_activity)
    }
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn exit_detail(status: std::process::ExitStatus) -> String {
    format!(
        "status {status} (code {:?}, signal {:?})",
        status.code(),
        {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                status.signal()
            }
            #[cfg(not(unix))]
            {
                None::<i32>
            }
        }
    )
}

fn spawn_line_reader<R>(pipe: Option<R>, shared: Arc<Shared>) -> JoinHandle<()>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if let Some(mut pipe) = pipe {
                // Byte-level read_until instead of BufRead::lines(): guest
                // consoles emit escape sequences and firmware glyphs that are
                // not valid UTF-8, which would silently terminate a
                // `lines().map_while(ok)` pump mid-boot (observed live: witness
                // pumps starving while kill()-drain later flushed the full
                // log). Lossy conversion keeps every byte observable.
                let mut buffer: Vec<u8> = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    match pipe.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            for byte in &chunk[..n] {
                                if *byte == b'\n' {
                                    let text =
                                        String::from_utf8_lossy(&buffer).into_owned();
                                    println!("{text}");
                                    shared.push_line(text.trim_end_matches('\r'));
                                    buffer.clear();
                                } else {
                                    buffer.push(*byte);
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        }));
        if outcome.is_err() {
            eprintln!("e2e session reader panicked");
        }
        shared.note_reader_eof();
    })
}

/// A live piped serial console against one guest. Finish via [`kill`].
pub struct SerialSession {
    stdin_slot: Option<std::process::ChildStdin>,
    shared: Arc<Shared>,
    readers: Vec<JoinHandle<()>>,
    idle_timeout: Duration,
}

impl SerialSession {
    pub fn spawn(
        spec: crate::qemu::QemuSpec,
        idle_timeout: Duration,
    ) -> Result<Self, SessionError> {
        let mut command: Command = spec.into_command();
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        // stdin piping is decided by QemuSpec.stdin_piped (see into_command);
        // null-stdin matches the proven bounded-boot shape for witness-only
        // cases and keeps the injection contract intact when scripting.

        let mut child = command.spawn().map_err(SessionError::Spawn)?;
        let stdin_handle = child.stdin.take();

        let shared = Shared::new(2);
        {
            *locked(&shared.child) = Some(child);
        }
        let stdout_pipe = {
            let mut slot = locked(&shared.child);
            slot.as_mut().and_then(|c| c.stdout.take())
        };
        let stderr_pipe = {
            let mut slot = locked(&shared.child);
            slot.as_mut().and_then(|c| c.stderr.take())
        };
        let readers = vec![
            spawn_line_reader(stdout_pipe, shared.clone()),
            spawn_line_reader(stderr_pipe, shared.clone()),
        ];

        Ok(Self {
            stdin_slot: stdin_handle,
            shared,
            readers,
            idle_timeout,
        })
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            text: locked(&self.shared.text).clone(),
        }
    }

    /// Frozen API: block until `needle` appears or `deadline` elapses.
    pub fn wait_witness(&mut self, needle: &str, deadline: Instant) -> Result<(), WaitOutcome> {
        loop {
            if self.snapshot().contains(needle) {
                return Ok(());
            }
            self.await_progress(deadline)?;
        }
    }

    /// Frozen API: wait until the latest console lines look like a prompt.
    pub fn wait_prompt(&mut self, deadline: Instant) -> Result<(), WaitOutcome> {
        let pattern = crate::witness::Pattern::new(crate::witness::PROMPT_PATTERN)
            .expect("PROMPT_PATTERN constant must compile");
        loop {
            let recent = self.snapshot().last_lines(2);
            if recent.iter().any(|line| pattern.match_line(line)) {
                return Ok(());
            }
            self.await_progress(deadline)?;
        }
    }

    /// Type one newline-terminated line into the guest console. Anchor with
    /// [`wait_prompt`] first whenever the guest drives a real shell.
    pub fn send_line(&mut self, line: &str) -> Result<(), SessionError> {
        self.send_bytes(format!("{line}\n").as_bytes())
    }

    /// Raw byte injection; b"\x03" interrupts shell reads as Ctrl-C.
    pub fn send_bytes(&mut self, bytes: &[u8]) -> Result<(), SessionError> {
        use std::io::Write;
        let Some(stdin) = self.stdin_slot.as_mut() else {
            return Err(SessionError::Io(std::io::Error::other(
                "guest stdin unavailable",
            )));
        };
        stdin.write_all(bytes).map_err(SessionError::Io)?;
        stdin.flush().map_err(SessionError::Io)?;
        Ok(())
    }

    /// Block until new serial evidence appears, the guest exits, the idle
    /// watchdog fires, or `deadline` expires. Polling on a short slice rather
    /// than condvar parking avoids every lost-notification race around the
    /// snapshot/check boundary callers sit on.
    fn await_progress(&self, deadline: Instant) -> Result<(), WaitOutcome> {
        const POLL_SLICE: Duration = Duration::from_millis(10);
        // Baselines captured AFTER the caller's last predicate evaluation:
        // anything that landed in between counts as instant progress so the
        // next snapshot always sees fresh material.
        let baseline_len = locked(&self.shared.text).chars().count();
        let mut baseline_activity = self.shared.activity();
        loop {
            if let Some(detail) = locked(&self.shared.exit_status).clone() {
                return Err(WaitOutcome::GuestExited(detail));
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(WaitOutcome::DeadlineExceeded);
            }
            std::thread::sleep(POLL_SLICE);
            let current_len = locked(&self.shared.text).chars().count();
            let current_activity = self.shared.activity();
            if current_len != baseline_len || current_activity != baseline_activity {
                return Ok(());
            }
            if now.saturating_duration_since(current_activity) >= self.idle_timeout {
                return Err(WaitOutcome::IdleStalled(self.idle_timeout));
            }
            baseline_activity = current_activity;
        }
    }

    /// Reap status without teardown (cheap; idempotent).
    pub fn poll_exit(&mut self) -> bool {
        if locked(&self.shared.exit_status).is_some() {
            return true;
        }
        let mut child_slot = locked(&self.shared.child);
        match child_slot.as_mut().map(|child| child.try_wait()) {
            Some(Ok(Some(status))) => {
                self.shared.record_exit(exit_detail(status));
                true
            }
            Some(Ok(None)) | None | Some(Err(_)) => false,
        }
    }

    /// Sleep until output arrives, guest exits, the idle watchdog fires, or
    /// `deadline` expires. Evidence evaluation stays with callers that loop.
    pub fn await_signal(&self, deadline: Instant) -> Result<(), WaitOutcome> {
        self.await_progress(deadline)
    }

    /// Last `n` captured lines joined by '\n' (bounded diagnostics dump).
    pub fn tail(&self, n: usize) -> String {
        self.snapshot().last_lines(n).join("\n")
    }

    pub fn default_tail(&self) -> String {
        self.tail(DEFAULT_TAIL_LINES)
    }

    pub fn has_exited(&self) -> bool {
        locked(&self.shared.exit_status).is_some()
    }

    /// Kill the guest and return the full captured buffer. The direct child
    /// is killed and reaped; readers are abandoned rather than joined — any
    /// descendant process inheriting our pipe handles can outlive the child,
    /// and joining them would block teardown until such stragglers exit
    /// (buffer contents stay consistent: readers only ever append).
    pub fn kill(mut self) -> String {
        // Closing stdin first lets guests blocked on console reads observe
        // input EOF before termination.
        self.stdin_slot.take();
        {
            let mut child_slot = locked(&self.shared.child);
            if let Some(child) = child_slot.as_mut() {
                let _ = child.kill();
                if let Ok(status) = child.wait() {
                    self.shared.record_exit(exit_detail(status));
                }
            }
        }
        // Dropping the JoinHandles detaches the reader threads; see above.
        self.readers.clear();
        locked(&self.shared.text).clone()
    }
}

/// Value snapshot of the session buffer at one instant.
pub struct SessionSnapshot {
    text: String,
}

impl SessionSnapshot {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn contains(&self, needle: &str) -> bool {
        self.text.contains(needle)
    }

    pub fn line_count(&self) -> usize {
        self.text.lines().count()
    }

    pub fn last_lines(&self, n: usize) -> Vec<String> {
        let start = self.line_count().saturating_sub(n);
        self.text.lines().skip(start).map(str::to_owned).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qemu::QemuSpec;

    /// End-to-end session mechanics without QEMU: a shell child streams a
    /// marker line and then sleeps forever; wait_witness must observe the
    /// line while output flows, and the total-budget deadline must fire even
    /// though nothing else happens.
    #[test]
    fn synthetic_child_streams_and_deadline_fires() {
        let script = "echo witness-marker-alpha; sleep 60 & wait";
        let mut command = std::process::Command::new("/bin/sh");
        command.arg("-c").arg(script);
        let spec = QemuSpec::from_command(&command, vec![(
            "SERVICEOS_E2E_TEST_ONLY".to_owned(),
            "1".to_owned(),
        )]);
        // Scripting contract under test ⇒ opt into the injection pipe.
        let mut spec = spec;
        spec.stdin_piped = true;
        let mut session = SerialSession::spawn(spec, Duration::from_secs(5)).expect("spawn sh");

        let deadline = Instant::now() + Duration::from_secs(10);
        session
            .wait_witness("witness-marker-alpha", deadline)
            .expect("witness found in streamed output");
        assert!(session.tail(5).contains("witness-marker-alpha"));

        // stdin stays injectable per the frozen API contract.
        session.send_bytes(b"\x03").expect("send_bytes");

        let result =
            session.wait_witness("never-appears", Instant::now() + Duration::from_secs(2));
        assert!(matches!(result, Err(WaitOutcome::DeadlineExceeded)));
        let buffer = session.kill();
        assert!(buffer.contains("witness-marker-alpha"));
    }

    /// Guest-exit detection: short-lived child ends the wait early instead of
    /// spinning until the deadline.
    #[test]
    fn guest_exit_surfaces_as_wait_outcome() {
        let mut command = std::process::Command::new("/bin/sh");
        command.arg("-c").arg("echo bye; exit 7");
        let spec = QemuSpec::from_command(&command, Vec::new());
        let mut session =
            SerialSession::spawn(spec, Duration::from_millis(300)).expect("spawn sh");
        let outcome = session.wait_witness("still-waiting", Instant::now() + Duration::from_secs(5));
        assert!(matches!(outcome, Err(WaitOutcome::GuestExited(_))));
        assert!(session.has_exited());
        session.kill();
    }
}

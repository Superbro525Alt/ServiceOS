use std::{
    env,
    error::Error,
    io::{BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use crate::run::{qemu_virt_command, qemu_virtio_command};

use crate::build::BuildArtifacts;

/// Serial evidence captured from a bounded headless boot.
pub struct BootOutcome {
    /// Combined guest serial output (stdout) plus QEMU stderr.
    pub output: String,
    /// True when every requested marker was observed before shutdown.
    pub markers_seen: bool,
    /// True when the boot was killed because it exceeded the timeout
    /// without emitting every marker.
    pub timed_out: bool,
    /// True when the guest process exited on its own (status success).
    pub exited_cleanly: bool,
    /// Human-readable exit detail for diagnostics when the guest quit
    /// before emitting every marker.
    pub exit_detail: String,
}

fn boot_timeout() -> Duration {
    let secs = env::var("SERVICEOS_BOOT_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(240);
    Duration::from_secs(secs)
}

/// Run a qemu-virtio boot fully headless with piped serial output. Returns
/// as soon as every marker appears (guest is killed cleanly) or when the
/// bounded timeout expires. The guest currently never powers QEMU off by
/// itself, so marker-driven early exit keeps validation wall-time short.
pub fn bounded_qemu_virtio_boot(
    disk_image: &Path,
    data_image: &Path,
    markers: &[String],
) -> Result<BootOutcome, Box<dyn Error>> {
    // SAFETY: single-threaded xtask process; no other threads read env yet.
    unsafe {
        env::set_var("QEMU_HEADLESS", "1");
        // Throwaway firmware vars copy: killed boots must never poison the
        // shared interactive OVMF_VARS.fd.
        let vars_path = crate::build::workspace_root()
            .join("target")
            .join("ovmf")
            .join("OVMF_VARS-bounded.fd");
        env::set_var("SERVICEOS_OVMF_VARS", &vars_path);
    }
    let command = qemu_virtio_command(data_image, disk_image)?;
    run_bounded(command, markers)
}

/// Bounded headless boot of the aarch64 `virt` platform.
pub fn bounded_qemu_virt_boot(
    artifacts: &BuildArtifacts,
    markers: &[String],
) -> Result<BootOutcome, Box<dyn Error>> {
    // SAFETY: single-threaded xtask process; no other threads read env yet.
    unsafe {
        env::set_var("QEMU_HEADLESS", "1");
    }
    let command = qemu_virt_command(artifacts)?;
    run_bounded(command, markers)
}

fn run_bounded(mut command: Command, markers: &[String]) -> Result<BootOutcome, Box<dyn Error>> {
    let deadline = Instant::now() + boot_timeout();
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to launch bounded QEMU boot: {}", error))?;

    let buffer: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout_reader = spawn_line_reader(stdout_pipe, buffer.clone());
    let stderr_reader = spawn_line_reader(stderr_pipe, buffer.clone());

    let mut outcome = BootOutcome {
        output: String::new(),
        markers_seen: false,
        timed_out: false,
        exited_cleanly: false,
        exit_detail: String::new(),
    };
    loop {
        if let Some(status) = child.try_wait()? {
            outcome.exited_cleanly = status.success();
            outcome.exit_detail = format!(
                "qemu exited early: status {} (code {:?}, signal {:?})",
                status,
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
            );
            break;
        }
        {
            let buffered = buffer.lock().expect("boot log mutex poisoned");
            if !markers.is_empty() && markers.iter().all(|marker| buffered.contains(marker)) {
                outcome.markers_seen = true;
                break;
            }
        }
        if Instant::now() >= deadline {
            outcome.timed_out = true;
            break;
        }
        thread::sleep(Duration::from_millis(250));
    }

    if outcome.markers_seen || outcome.timed_out {
        let _ = child.kill();
        let _ = child.wait();
    }
    let _ = stdout_reader.join();
    let _ = stderr_reader.join();
    outcome.output = Arc::try_unwrap(buffer)
        .map(|handle| handle.into_inner().expect("boot log mutex poisoned"))
        .unwrap_or_default();

    Ok(outcome)
}
fn spawn_line_reader<R>(pipe: Option<R>, sink: Arc<Mutex<String>>) -> thread::JoinHandle<()>
where
    R: std::io::Read + Send + 'static,
{
    thread::spawn(move || {
        let Some(pipe) = pipe else {
            return;
        };
        let reader = BufReader::new(pipe);
        for line in reader.lines().map_while(Result::ok) {
            println!("{line}");
            let mut buffered = sink.lock().expect("boot log mutex poisoned");
            buffered.push_str(&line);
            buffered.push('\n');
        }
    })
}

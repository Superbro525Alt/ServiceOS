//! QemuSpec capture + builder routing. The canonical argv assembly stays in
//! `xtask-core::run` (zero-diff guarantee, docs/test-plan.md §2.3); this
//! module wraps it with an explicit environment layer so concurrent or
//! sequential spawns replay a captured environment verbatim instead of
//! depending on ambient process state.

use std::{
    ffi::OsString,
    path::PathBuf,
    process::{Command, Stdio},
};

pub use xtask_core::run::{
    find_qemu_aarch64_binary, find_qemu_binary, find_qemu_riscv64_binary,
    find_ovmf_code, find_ovmf_vars_template,
};

/// The full launcher state for one guest: program, exact args, and the env
/// deltas (KEY=VALUE) that must be in place for this instance.
#[derive(Debug, Clone)]
pub struct QemuSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(String, String)>,
    /// Witness-only boots reproduce the proven bounded-boot shape
    /// (`stdin(null)`); scripted T3+ boots flip this to keep the injection
    /// pipe open.
    pub stdin_piped: bool,
}

impl QemuSpec {
    pub fn from_command(command: &Command, env: Vec<(String, String)>) -> Self {
        Self {
            program: PathBuf::from(command.get_program().to_os_string()),
            args: command.get_args().map(OsString::from).collect(),
            env,
            stdin_piped: false,
        }
    }

    /// Rebuild a fresh Command replaying program/args/env deterministically.
    pub fn into_command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        for (key, value) in &self.env {
            // SAFETY-free: Command env mutation is process-local.
            command.env(key, value);
        }
        if self.stdin_piped {
            command.stdin(Stdio::piped());
        } else {
            command.stdin(Stdio::null());
        }
        command
    }

    /// Additive WP3 input-injection path: route the HMP monitor through the
    /// same stdio pair as the serial console (`-serial mon:stdio`) so scripts
    /// can `raw:`-send `Ctrl-A c` + sendkey sequences to the guest devices.
    /// Rewrites only the serial transport pair; every other arg stays
    /// byte-identical (plan §2.3 zero-diff guarantee for default boots).
    pub fn enable_serial_monitor_mux(&mut self) -> Result<(), String> {
        let snapshot = self.args.clone();
        let mut replaced = false;
        for window in 1..snapshot.len() {
            if snapshot[window - 1].to_string_lossy() == "-serial"
                && snapshot[window].to_string_lossy() == "stdio"
            {
                self.args[window] = OsString::from("mon:stdio");
                replaced = true;
            }
        }
        if !replaced {
            return Err("serial mux failed: no `-serial stdio` pair present".to_owned());
        }
        Ok(())
    }
}

/// Environment keys the builders read from process-global state; snapshot +
/// restore them around guard windows and pass them forward explicitly.
pub const BUILDER_ENV_KEYS: [&str; 11] = [
    "QEMU_HEADLESS",
    "SERVICEOS_OVMF_VARS",
    "QEMU_ACCEL",
    "QEMU_AUDIODEV",
    "SERVICEOS_AUDIO",
    "SERVICEOS_SMP",
    "SERVICEOS_GL",
    "OVMF_CODE",
    "OVMF_VARS",
    "QEMU_EXTRA_ARGS",
    "SERVICEOS_BOOT_MODE",
];

/// Set process-global env for the duration of a scope, restoring prior
/// values on drop. Requires single-threaded callers; parallel scheduling is
/// WP4 work and must replace ambient-env reads with per-Command plumbing.
pub struct EnvGuard {
    restored: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    pub fn apply(pairs: &[(String, String)]) -> Self {
        let mut restored: Vec<(String, Option<String>)> = Vec::new();
        for (key, _) in pairs {
            let previous = std::env::var(key).ok();
            restored.push((key.clone(), previous));
        }
        // SAFETY: single-threaded xtask runner; no reader threads exist yet
        // (readers only spawn when a session spawns, after this guard ends).
        unsafe {
            for (key, value) in pairs {
                std::env::set_var(key, value);
            }
        }
        Self { restored }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: see EnvGuard::apply.
        unsafe {
            for (key, value) in self.restored.drain(..) {
                match value {
                    Some(previous) => std::env::set_var(key, previous),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

/// Reason strings compatible with validate.rs's SKIPPED reporting.
pub fn missing_emulator_reason(platform: &str) -> Option<&'static str> {
    match platform {
        "qemu-virtio" | "qemu-isa" => find_qemu_binary()
            .is_none()
            .then_some("SKIPPED (qemu-system-x86_64 not installed)"),
        "virt" => find_qemu_aarch64_binary()
            .is_none()
            .then_some("SKIPPED (qemu-system-aarch64 not installed)"),
        "riscv64-virt" => find_qemu_riscv64_binary()
            .is_none()
            .then_some("SKIPPED (qemu-system-riscv64 not installed)"),
        _ => None,
    }
}

/// Route a platform to its canonical builder. `paths.disk_image` /
/// `paths.data_image` come from [`crate::isolation::stage_case_images`] so
/// no case ever points at dev images under `target/<platform>-image/`.
pub fn spec_for(
    platform: &str,
    artifacts: &xtask_core::build::BuildArtifacts,
    paths: &crate::isolation::SlotPaths,
) -> Result<QemuSpec, Box<dyn std::error::Error>> {
    let env = current_builder_env();
    let spec = match platform {
        "qemu-virtio" => {
            let data_image = data_image_for(platform, paths);
            let command = xtask_core::run::qemu_virtio_command(&data_image, &paths.disk_image)?;
            QemuSpec::from_command(&command, env)
        }
        "virt" => {
            let command = xtask_core::run::qemu_virt_command(artifacts)?;
            QemuSpec::from_command(&command, env)
        }
        "qemu-isa" => {
            let command = xtask_core::run::qemu_isa_command(artifacts)?;
            QemuSpec::from_command(&command, env)
        }
        "riscv64-virt" => {
            let command = xtask_core::run::qemu_riscv_virt_command(artifacts)?;
            QemuSpec::from_command(&command, env)
        }
        other => return Err(format!("platform {other:?} has no emulator boot path").into()),
    };
    Ok(spec)
}

fn data_image_for(platform: &str, paths: &crate::isolation::SlotPaths) -> PathBuf {
    if platform == "qemu-virtio" || platform == "virt" {
        if let Some(data) = &paths.data_image {
            return data.clone();
        }
    }
    // riscv64-virt / qemu-isa attach no data drive; any placeholder must not
    // exist so accidental reuse fails loudly instead of silently booting.
    PathBuf::from("/nonexistent-serviceos-data.img")
}

pub fn current_builder_env() -> Vec<(String, String)> {
    BUILDER_ENV_KEYS
        .iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| ((*key).to_owned(), value)))
        .collect()
}

/// Directories searched by binary finders, exposed for diagnostics only.
pub fn describe_emulators() -> Vec<(&'static str, Option<PathBuf>)> {
    vec![
        ("x86_64", find_qemu_binary()),
        ("aarch64", find_qemu_aarch64_binary()),
        ("riscv64", find_qemu_riscv64_binary()),
    ]
}

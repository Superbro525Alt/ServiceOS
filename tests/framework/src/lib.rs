//! Host-side end-to-end runner framework for ServiceOS (`serviceos-e2e`).
//!
//! Public API frozen by docs/test-plan.md §5 (WP1); WP2/WP3 build on these
//! types without changing their shape:
//!
//! ```text
//! e2e::CaseDef;            e2e::load_cases(root) -> Vec<CaseDef>;
//! e2e::SerialSession::{spawn,wait_witness,send_line,send_bytes,wait_prompt,tail};
//! e2e::run_case(&CaseDef, &RunCtx) -> CaseResult;   // RunCtx{stage_root, jobs, builds}
//! e2e::aggregate(vec<CaseResult>) -> ExitCode;      // codes per §4
//! ```

pub mod case;
pub mod isolation;
pub mod qemu;
pub mod report;
pub mod script;
pub mod session;
pub mod witness;

use std::{
    collections::BTreeMap,
    error::Error,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use xtask_core::{
    build::{build_for_platform, BuildArtifacts},
    image::create_platform_image,
    platform::PlatformSpec,
};

pub use case::{load_cases, CaseDef, WitnessMode};
pub use report::{aggregate, print_summary_table, write_tap, CaseResult, Outcome};
pub use script::{run_script, SerialScript};
pub use session::{SerialSession, WaitOutcome};
pub use witness::Pattern;

/// Default per-case wall-clock budget when neither the case file nor
/// `SERVICEOS_BOOT_TIMEOUT_SECS` specifies one (matches bootlog.rs).
pub const DEFAULT_CASE_TIMEOUT_SECS: u64 = 240;

/// No-output watchdog default documented for API consumers; the runner
/// currently derives its per-case value from the case budget (see
/// `execute_row`), with per-phase calibration planned for WP4.
pub const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 180;

/// raspi5 staged-bundle contents asserted by the build-only smoke case.
pub const RASPI_STAGED_FILES: [&str; 3] = ["config.txt", "kernel8.img", "serviceos/bootstore.bin"];

/// Everything a case execution needs beyond its own definition.
pub struct RunCtx {
    pub workspace_root: PathBuf,
    /// Root for per-case/slot staging (`target/e2e/...`).
    pub stage_root: PathBuf,
    /// Concurrency cap; scheduling is strict-sequential until WP4 lands the
    /// semaphore worker pool that consumes this value.
    pub jobs: usize,
    pub timeout_override: Option<u64>,
    pub release: bool,
    /// Lazily hydrated builds keyed by platform name.
    pub builds: BTreeMap<String, PlatformBuild>,
    /// Platform for the currently executing [`run_case`] row.
    pub current_platform: String,
}

/// A built platform plus its freshly staged dev image, shared by all cases
/// targeting the platform within this invocation.
pub struct PlatformBuild {
    pub artifacts: BuildArtifacts,
    pub image: PathBuf,
    pub release: bool,
}

impl RunCtx {
    pub fn new(workspace_root: PathBuf, jobs: usize, release: bool) -> Self {
        let stage_root = workspace_root.join("target").join("e2e");
        Self {
            workspace_root,
            stage_root,
            jobs,
            timeout_override: None,
            release,
            builds: BTreeMap::new(),
            current_platform: String::new(),
        }
    }

    /// Resolve + build + image exactly once per platform.
    pub fn ensure_build(
        &mut self,
        platform: &str,
        env_build: &[(String, String)],
    ) -> Result<(), Box<dyn Error>> {
        // Guest option_env! gates (SERVICEOS_E2E_*) make the built image a
        // function of the sorted flag tuple, so builds are cached per
        // platform+tuple; differing tuples rebuild inside the same target dir.
        let mut flags: Vec<&(String, String)> = env_build.iter().collect();
        flags.sort();
        let cache_key = format!(
            "{platform}::{}",
            flags
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        if self.builds.contains_key(&cache_key) {
            return Ok(());
        }
        if !flags.is_empty() {
            println!(
                "=== e2e gate env: {} ===",
                flags
                    .iter()
                    .map(|(key, value)| format!("{key}={value}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        let spec = PlatformSpec::resolve(platform)?;
        // The option_env! reads live inside guest crates compiled by the cargo
        // invocations below; the guard windows them so nothing else observes
        // ambient drift (single-threaded scheduling per §4).
        let gate_pairs: Vec<(String, String)> = flags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let _gate_guard = qemu::EnvGuard::apply(&gate_pairs);
        invalidate_guest_build_for_gates(
            &self.workspace_root,
            spec.userspace_rust_target(),
            &cache_key,
        )?;
        println!("=== e2e build: {platform} ===");
        let artifacts = build_for_platform(spec, self.release)?;
        drop(_gate_guard);
        let image = create_platform_image(&artifacts)?;
        println!(
            "=== e2e build complete: {platform} at {} ===",
            image.display()
        );
        self.builds.insert(
            cache_key,
            PlatformBuild {
                artifacts,
                image,
                release: self.release,
            },
        );
        Ok(())
    }

    /// Current-platform artifacts for a case's flag tuple, building once.
    fn artifacts_for_case(
        &mut self,
        case: &CaseDef,
        platform: &str,
    ) -> Result<&PlatformBuild, Box<dyn Error>> {
        let mut flags: Vec<(String, String)> = case.env_build.clone();
        flags.sort();
        let cache_key = format!(
            "{platform}::{}",
            flags
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        if !self.builds.contains_key(&cache_key) {
            self.ensure_build(platform, &case.env_build)?;
        }
        Ok(&self.builds[&cache_key])
    }

    /// Budget priority: `--timeout-secs` > case file > env default.
    pub fn case_timeout(&self, case: &CaseDef) -> Duration {
        let secs = self
            .timeout_override
            .or(case.timeout_secs)
            .unwrap_or_else(|| {
                std::env::var("SERVICEOS_BOOT_TIMEOUT_SECS")
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(DEFAULT_CASE_TIMEOUT_SECS)
            });
        Duration::from_secs(secs.max(1))
    }
}

/// Execute one (case, [`RunCtx::current_platform`]) row end-to-end. Guest
/// misbehavior becomes data on the result row; only harness breakdowns land
/// in `Err` and surface as InfraFailed rows upstream.
pub fn run_case(case: &CaseDef, ctx: &mut RunCtx) -> CaseResult {
    let started = Instant::now();
    let platform = ctx.current_platform.clone();

    // Declaratively-blocked cases (e.g. kernel-owned mechanisms) surface as
    // SKIPPED rows with the documented reason instead of vanishing from the
    // report or burning a boot on a known-impossible witness.
    if !case.blocker.is_empty() {
        return CaseResult::skipped(
            case,
            &platform,
            started,
            format!("blocked: {}", case.blocker),
        );
    }

    if !case.platforms.iter().any(|declared| declared == &platform) {
        return CaseResult::skipped(
            case,
            &platform,
            started,
            format!("excluded: case does not declare platform {platform}"),
        );
    }

    match execute_row(case, ctx, &platform, started) {
        Ok(result) => result,
        Err(error) => CaseResult::infra_failed(case, &platform, started, error.to_string()),
    }
}

fn execute_row(
    case: &CaseDef,
    ctx: &mut RunCtx,
    platform: &str,
    started: Instant,
) -> Result<CaseResult, Box<dyn Error>> {
    // raspi5 is build-only: no QEMU, no emulator prerequisite.
    if platform == "raspi5" {
        return execute_raspi_image_assertion(case, ctx, platform, started);
    }

    if let Some(reason) = qemu::missing_emulator_reason(platform) {
        return Ok(CaseResult::skipped(case, platform, started, reason.to_owned()));
    }

    ctx.ensure_build(platform, &case.env_build)?;

    // Only UEFI (qemu-virtio) boots consume a slot disk copy; the kernel-ELF /
    // Image platforms (-machine pc, aarch64 virt, riscv64-virt) take their
    // payload straight from BuildArtifacts and stage no block device, so
    // passing the bundle DIRECTORY here would make fs::copy explode
    // ("neither a regular file nor a symlink"). create_platform_image already
    // returned a fresh serviceos.img FILE for virtio (mirrors validate.rs:195).
    // The staged disk/data images are REUSED verbatim across a boot-B phase so
    // first-boot state written by boot A is visible to boot B (plan §T4
    // wizard-first-boot-chain row; pattern reused from upgrade.rs).
    let built_disk = match platform {
        "qemu-virtio" => Some(ctx.artifacts_for_case(case, platform)?.image.as_path().to_path_buf()),
        _ => None,
    };
    let paths = isolation::stage_case_images(
        &ctx.workspace_root,
        case,
        platform,
        built_disk.as_deref(),
        0,
    )?;

    // Ambient env mutations happen under a restoration guard while still
    // single-threaded; the built spec then carries an explicit snapshot so
    // later spawning ignores ambient drift entirely.
    let mut builder_env = vec![("QEMU_HEADLESS".to_owned(), "1".to_owned())];
    if let Some(vars_path) = &paths.ovmf_vars {
        builder_env.push((
            "SERVICEOS_OVMF_VARS".to_owned(),
            vars_path.display().to_string(),
        ));
    }
    // Case-declared launch-time env (e.g. the plan §2.5 audio pair) rides
    // the same guard window so the spec snapshots it; restored on drop.
    for (key, value) in &case.qemu_env {
        builder_env.push((key.clone(), value.clone()));
    }
    let _guard = qemu::EnvGuard::apply(&builder_env);
    let artifacts = ctx.artifacts_for_case(case, platform)?;
    let mut spec = qemu::spec_for(platform, &artifacts.artifacts, &paths)?;
    drop(_guard);

    // Scripted cases opt into the injection pipe. Input-injection cases may
    // additionally demand the muxed HMP monitor on the same stdio pair.
    let scripted = case.serial_script.is_some();
    spec.stdin_piped = scripted;
    if case.monitor_mux {
        if let Err(error) = spec.enable_serial_monitor_mux() {
            return Ok(CaseResult::infra_failed(case, platform, started, error));
        }
    }

    println!(
        "=== e2e boot: {} @ {} (slot {}) ===",
        case.name,
        platform,
        paths.dir.display()
    );

    // ---- Boot A ----
    let deadline_a = Instant::now() + ctx.case_timeout(case);
    let verdict_a = run_single_boot(case, &spec, deadline_a);
    let boot_b_demanded = !case.boot_b_witnesses.is_empty();
    match verdict_a {
        BootOutcome::Passed { .. } => {
            if !boot_b_demanded {
                return Ok(CaseResult::passed(case, platform, started));
            }
            // §6.2: kills only happen after expected witnesses observed; the
            // same staged disk + data volume then carry first-boot state into
            // boot B.
            println!(
                "=== e2e boot B: {} @ {} (slot {}, reused volume) ===",
                case.name, platform, paths.dir.display()
            );
            let deadline_b = Instant::now() + ctx.case_timeout(case);
            let boot_b = second_boot_verdict(case, &spec, deadline_b);
            return Ok(match boot_b {
                BootOutcome::Passed { .. } => CaseResult::passed(case, platform, started),
                BootOutcome::Failed(reason, Some(text)) => {
                    CaseResult::failed(case, platform, started, reason, tail_of(&text))
                }
                BootOutcome::Failed(reason, None) => {
                    CaseResult::failed(case, platform, started, reason, String::new())
                }
                BootOutcome::Infra(reason) => {
                    CaseResult::infra_failed(case, platform, started, reason)
                }
            });
        }
        BootOutcome::Failed(reason, Some(text)) => {
            return Ok(CaseResult::failed(
                case,
                platform,
                started,
                reason,
                tail_of(&text),
            ))
        }
        BootOutcome::Failed(reason, None) => {
            return Ok(CaseResult::failed(
                case,
                platform,
                started,
                reason,
                String::new(),
            ))
        }
        BootOutcome::Infra(reason) => {
            return Ok(CaseResult::infra_failed(case, platform, started, reason))
        }
    }
}

/// One serial-phase run over an already-built spec: spawn, optional script,
/// witness drive, kill. Output text is kept for failure diagnostics.
fn run_single_boot(case: &CaseDef, spec: &qemu::QemuSpec, deadline: Instant) -> BootOutcome {
    let mut session = match SerialSession::spawn(spec.clone(), budget_until(deadline)) {
        Ok(session) => session,
        Err(error) => return BootOutcome::Infra(format!("QEMU spawn failed: {error}")),
    };

    if let Some(script_path) = case.serial_script.as_deref() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if let Err(error) = run_script(script_path, &mut session, remaining) {
            let output = session.kill();
            return BootOutcome::Failed(
                format!("serial script failed: {error}"),
                Some(output),
            );
        }
    }

    let verdict = drive_witnesses(case, &mut session, deadline);
    let output = session.kill();
    match verdict {
        StepVerdict::Passed => BootOutcome::Passed { text: output },
        StepVerdict::Failed(reason) => BootOutcome::Failed(reason, Some(output)),
        StepVerdict::Infra(reason) => BootOutcome::Infra(reason),
    }
}

/// Second-boot phase of a regression chain: fresh spawn on the SAME staged
/// volume, different witness/fail_on sets, no script.
fn second_boot_verdict(case: &CaseDef, spec: &qemu::QemuSpec, deadline: Instant) -> BootOutcome {
    let mut witnesses = Vec::with_capacity(case.boot_b_witnesses.len());
    for raw in &case.boot_b_witnesses {
        match Pattern::new(raw) {
            Ok(pattern) => witnesses.push(pattern),
            Err(error) => return BootOutcome::Infra(format!("invalid boot-B pattern: {error}")),
        }
    }
    let mut fail_on = Vec::with_capacity(case.boot_b_fail_on.len());
    for raw in &case.boot_b_fail_on {
        match Pattern::new(raw) {
            Ok(pattern) => fail_on.push(pattern),
            Err(error) => return BootOutcome::Infra(format!("invalid boot-B fail_on: {error}")),
        }
    }

    let mut session = match SerialSession::spawn(spec.clone(), budget_until(deadline)) {
        Ok(session) => session,
        Err(error) => return BootOutcome::Infra(format!("QEMU spawn failed: {error}")),
    };
    let verdict = drive_pattern_sets(&witnesses, &fail_on, &mut session, deadline);
    let output = session.kill();
    match verdict {
        StepVerdict::Passed => BootOutcome::Passed { text: output },
        StepVerdict::Failed(reason) => BootOutcome::Failed(reason, Some(output)),
        StepVerdict::Infra(reason) => BootOutcome::Infra(reason),
    }
}

/// Sentinel input tracked by the userspace catalog build script
/// (rerun-if-changed over bundles_root): touching it forces a bootstore
/// regeneration even when no guest source changed.
const BOOTSTORE_INVALIDATION_SENTINEL: &str = "userspace/bundles/config/hosts.cfg";
/// Records which gate tuple produced the currently compiled guest artifacts.
const GATE_MARKER_FILE: &str = "target/userspace-programs/.e2e-gate-hash";

/// Cargo dep-info does not track `option_env!` reads on this toolchain, so
/// switching a guest gate tuple otherwise reuses binaries compiled under the
/// PREVIOUS tuple (observed live: gfx probes silent in gfx-gated builds).
/// Any recorded-tuple mismatch purges the shared guest target directory and
/// invalidates the embedded boot store chain before the rebuild below.
fn invalidate_guest_build_for_gates(
    workspace_root: &Path,
    user_target_triple: &'static str,
    cache_key: &str,
) -> Result<(), Box<dyn Error>> {
    let marker_path = workspace_root.join(GATE_MARKER_FILE);
    let previous = std::fs::read_to_string(&marker_path).ok();
    if previous.as_deref() == Some(cache_key) {
        return Ok(());
    }
    let guest_dir = workspace_root
        .join("target")
        .join("userspace-programs")
        .join(user_target_triple);
    if guest_dir.exists() {
        println!(
            "=== e2e gate switch ({cache_key}): purge stale guest artifacts ==="
        );
        std::fs::remove_dir_all(&guest_dir)?;
    } else {
        println!("=== e2e gate switch ({cache_key}): fresh guest artifacts ===");
    }
    let sentinel = workspace_root.join(BOOTSTORE_INVALIDATION_SENTINEL);
    let sentinel_meta = std::fs::metadata(&sentinel)?;
    let _needs_sentinel_to_exist = sentinel_meta.is_file();
    // set_modified keeps the file's content (bootstore manifests stay inert)
    // while advancing its mtime past the build script's last run.
    std::fs::File::options()
        .append(true)
        .open(&sentinel)?
        .set_modified(std::time::SystemTime::now())?;
    if let Some(parent) = marker_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&marker_path, cache_key)?;
    Ok(())
}

/// Budget handed to SerialSession::spawn: remaining wall clock until the
/// phase deadline (mirrors the historical per-case timeout shape).
fn budget_until(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}


fn tail_of(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let start = lines.len().saturating_sub(session::DEFAULT_TAIL_LINES);
    lines[start..].join("\n")
}

enum StepVerdict {
    Passed,
    Failed(String),
    Infra(String),
}

/// Per-phase outcome for the two-boot regression chain helper.
enum BootOutcome {
    /// Output text kept so failure rows can still dump a tail.
    Passed { #[allow(dead_code)] text: String },
    Failed(String, Option<String>),
    Infra(String),
}

fn drive_witnesses(
    case: &CaseDef,
    session: &mut SerialSession,
    deadline: Instant,
) -> StepVerdict {
    let mut witnesses = Vec::with_capacity(case.witnesses.len());
    for raw in &case.witnesses {
        match Pattern::new(raw) {
            Ok(pattern) => witnesses.push(pattern),
            Err(error) => return StepVerdict::Infra(format!("invalid witness pattern: {error}")),
        }
    }
    let mut fail_on = Vec::with_capacity(case.fail_on.len());
    for raw in &case.fail_on {
        match Pattern::new(raw) {
            Ok(pattern) => fail_on.push(pattern),
            Err(error) => return StepVerdict::Infra(format!("invalid fail_on pattern: {error}")),
        }
    }
    // mode = "suite": require the §2.6 completion line as an extra witness so
    // protocol accounting gates success alongside raw evidence patterns.
    if case.mode == WitnessMode::Suite {
        match Pattern::new("E2E SUITE DONE") {
            Ok(pattern) => witnesses.push(pattern),
            Err(_) => unreachable!("fixed constant parses"),
        }
    }
    drive_pattern_sets(&witnesses, &fail_on, session, deadline)
}

fn drive_pattern_sets(
    witnesses: &[Pattern],
    fail_on: &[Pattern],
    session: &mut SerialSession,
    deadline: Instant,
) -> StepVerdict {
    let mut satisfied = vec![false; witnesses.len()];
    loop {
        let snapshot = session.snapshot();
        let text = snapshot.text().to_owned();

        for pattern in fail_on {
            if pattern.matches(&text) {
                return StepVerdict::Failed(format!(
                    "fail_on matched early: {}",
                    pattern.raw()
                ));
            }
        }
        for (index, pattern) in witnesses.iter().enumerate() {
            if !satisfied[index]
                && pattern.satisfied(&text)
            {
                satisfied[index] = true;
                println!("    witness observed: {}", pattern.raw());
            }
        }
        if satisfied.iter().all(|hit| *hit) {
            return StepVerdict::Passed;
        }

        let missing: Vec<&str> = satisfied
            .iter()
            .zip(witnesses.iter())
            .filter(|(hit, _)| !**hit)
            .map(|(_, pattern)| pattern.raw())
            .collect();

        match session.await_signal(deadline) {
            Ok(()) => {}
            Err(signal) => {
                return StepVerdict::Failed(format!(
                    "{}; still missing: {}",
                    signal.to_string(),
                    missing.join(", ")
                ))
            }
        }
    }
}

fn execute_raspi_image_assertion(
    case: &CaseDef,
    ctx: &mut RunCtx,
    platform: &str,
    started: Instant,
) -> Result<CaseResult, Box<dyn Error>> {
    ctx.ensure_build(platform, &case.env_build)?;
    let bundle_root = ctx.artifacts_for_case(case, platform)?.image.clone();
    let mut missing = Vec::new();
    for rel in RASPI_STAGED_FILES {
        let candidate = bundle_root.join(rel);
        if !candidate.exists() {
            missing.push(rel.to_owned());
        } else {
            let size = std::fs::metadata(&candidate).map(|meta| meta.len()).unwrap_or(0);
            println!("    staged {rel}: {} bytes", size);
        }
    }
    Ok(if missing.is_empty() {
        CaseResult::passed(case, platform, started)
    } else {
        CaseResult::failed(
            case,
            platform,
            started,
            format!("staged bundle missing files: {}", missing.join(", ")),
            String::new(),
        )
    })
}

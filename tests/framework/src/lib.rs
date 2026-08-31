//! Host-side end-to-end runner framework for ServiceOS (`serviceos-e2e`).
//!
//! Public API frozen by docs/test-plan.md §5 (WP1); WP2/WP3 build on these
//! types without changing their shape:
//!
//! ```text
//! e2e::CaseDef;            e2e::load_cases(root) -> Vec<CaseDef>;
//! e2e::SerialSession::{spawn,wait_witness,send_line,send_bytes,wait_prompt,tail};
//! e2e::run_case(&CaseDef, &RunCtx, platform) -> CaseResult;  // RunCtx{stage_root, jobs, builds}
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
    sync::{Mutex, MutexGuard, TryLockError},
    time::{Duration, Instant},
};

use xtask_core::{
    build::{BuildArtifacts, build_for_platform},
    image::create_platform_image,
    platform::{PlatformSpec, RunKind},
};

pub use case::{CaseDef, WitnessMode, load_cases};
pub use report::{CaseResult, Outcome, aggregate, print_summary_table, write_tap};
pub use script::{SerialScript, run_script};
pub use session::{SerialSession, WaitOutcome};
pub use witness::Pattern;

/// Default per-case wall-clock budget when neither the case file nor
/// `SERVICEOS_BOOT_TIMEOUT_SECS` specifies one (matches bootlog.rs).
pub const DEFAULT_CASE_TIMEOUT_SECS: u64 = 240;

/// No-output watchdog default (plan §2.3: separate from the total budget).
/// Per boot phase the effective value is `SERVICEOS_IDLE_TIMEOUT_SECS` when
/// set, else the smaller of this constant and the case's total budget, so a
/// wedged console fails fast instead of squatting on a worker slot until the
/// full per-case timeout.
pub const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 180;

/// raspi5 staged-bundle contents asserted by the build-only smoke case.
pub const RASPI_STAGED_FILES: [&str; 3] = ["config.txt", "kernel8.img", "serviceos/bootstore.bin"];

/// Everything a case execution needs beyond its own definition. After the
/// serial pre-build phase (see the runner in `support/xtask/src/e2e.rs`) the
/// context is frozen and shared by reference across worker threads; rows
/// carry their platform explicitly via [`run_case`].
pub struct RunCtx {
    pub workspace_root: PathBuf,
    /// Root for per-case/slot staging (`target/e2e/...`).
    pub stage_root: PathBuf,
    /// Concurrency cap consumed by the worker pool.
    pub jobs: usize,
    pub timeout_override: Option<u64>,
    pub release: bool,
    /// Retain every stage dir (disables PASS pruning) for postmortems.
    pub keep_all: bool,
    /// Builds hydrated during the serial pre-build phase, keyed by
    /// `platform::<sorted gate tuple>`; read-only once workers start.
    pub builds: BTreeMap<String, PlatformBuild>,
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
            keep_all: false,
            builds: BTreeMap::new(),
        }
    }

    /// Resolve + build + image exactly once per platform. Serial-only: cargo
    /// invocations, the gate env guard, and the guest-artifact marker file
    /// are process-global and must never overlap (runner pre-builds every
    /// needed platform+tuple before any worker thread starts).
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
        // ambient drift (serial pre-build phase — worker threads never build).
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
        let built = create_platform_image(&artifacts)?;
        // Snapshot the build output under a per-tuple path BEFORE any other
        // tuple can build: create_platform_image writes every tuple to the
        // same fixed location, and the pre-build phase runs back-to-back, so
        // the last tuple would otherwise clobber the image content earlier
        // tuples cached (observed: gfx rows booted the input-gated image and
        // never emitted witnesses). Slot staging then copies from this
        // tuple-private snapshot, keeping boots tuple-exact.
        let image = snapshot_build_output(&self.workspace_root, &built, &cache_key)?;
        println!(
            "=== e2e build complete: {platform} tuple {cache_key:?} at {} ===",
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

    /// Current-platform artifacts for a case's flag tuple. The runner's
    /// serial pre-build phase guarantees presence; a miss here is a harness
    /// bug and surfaces as an InfraFailed row rather than a mid-flight build
    /// racing other workers' cargo invocations.
    fn artifacts_for_case(
        &self,
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
        self.builds.get(&cache_key).ok_or_else(|| {
            format!("pre-build phase did not hydrate platform {platform:?} tuple {cache_key:?}")
                .into()
        })
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

/// Process-wide serialization for TCG guest boots (host-wedge mitigation):
/// QEMU `tcg,thread=multi` instances contending for a starved host
/// intermittently wedge permanently (serial silence or network-rx stall;
/// see the 2026-09 virt investigation), so at most one TCG row runs at any
/// instant. KVM and no-emulator rows keep full parallelism. Process-global
/// because scheduling lives in the runner binary, whose worker pool calls
/// [`run_case`] concurrently.
static TCG_EXECUTION: Mutex<()> = Mutex::new(());

/// True when this platform's QEMU invocation runs under TCG on this host:
/// the aarch64 builder hardcodes `-accel tcg,thread=multi` and the
/// qemu-isa / riscv64-virt builders pass no `-accel` flag (QEMU default =
/// TCG), qemu-virtio honors `QEMU_ACCEL` / `/dev/kvm` (mirrors
/// `xtask-core::run::qemu_accel_mode`), and raspi5 never boots an emulator
/// (build-only). Unknown names count as TCG (conservative serialization).
pub fn platform_uses_tcg(platform: &str) -> bool {
    let Ok(spec) = PlatformSpec::resolve(platform) else {
        return true;
    };
    match spec.run_kind {
        RunKind::ManualDeploy => false,
        RunKind::QemuArmVirt | RunKind::QemuIsa | RunKind::QemuRiscvVirt => true,
        RunKind::QemuVirtio => accel_mode_forces_tcg(
            std::env::var("QEMU_ACCEL").ok().as_deref(),
            Path::new("/dev/kvm").exists(),
        ),
    }
}

/// Pure core of the qemu-virtio accel decision (`run.rs qemu_accel_mode`):
/// TCG iff the env override forces it, else KVM's absence.
fn accel_mode_forces_tcg(explicit: Option<&str>, kvm_available: bool) -> bool {
    match explicit {
        Some("tcg") => true,
        Some("kvm") => false,
        _ => !kvm_available,
    }
}

/// Acquire the single TCG execution slot for the whole row (staging, both
/// boots of a regression chain, teardown) when the platform is TCG. The
/// wait happens before any timer starts, so per-case budgets are untouched;
/// row elapsed still reflects the honest wall-clock wait.
fn tcg_serial_slot(platform: &str) -> Option<MutexGuard<'static, ()>> {
    if !platform_uses_tcg(platform) {
        return None;
    }
    match TCG_EXECUTION.try_lock() {
        Ok(guard) => Some(guard),
        Err(TryLockError::WouldBlock) => {
            eprintln!("    (tcg gate: waiting for the single TCG slot; {platform})");
            Some(
                TCG_EXECUTION
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            )
        }
        Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
    }
}

/// Execute one (case, platform) row end-to-end. Guest misbehavior becomes
/// data on the result row; only harness breakdowns land in `Err` and surface
/// as InfraFailed rows upstream. Safe to call from multiple worker threads:
/// the context is frozen (read-only) and every mutable interaction — env
/// capture, staging, QEMU spawn — is per-row (TCG rows additionally
/// serialize on [`TCG_EXECUTION`]).
pub fn run_case(case: &CaseDef, ctx: &RunCtx, platform: &str) -> CaseResult {
    let started = Instant::now();

    // Declaratively-blocked cases (e.g. kernel-owned mechanisms) surface as
    // SKIPPED rows with the documented reason instead of vanishing from the
    // report or burning a boot on a known-impossible witness.
    if !case.blocker.is_empty() {
        return CaseResult::skipped(
            case,
            platform,
            started,
            format!("blocked: {}", case.blocker),
        );
    }

    if !case.platforms.iter().any(|declared| declared == platform) {
        return CaseResult::skipped(
            case,
            platform,
            started,
            format!("excluded: case does not declare platform {platform}"),
        );
    }

    // Host-wedge mitigation: TCG rows hold the single TCG slot end-to-end so
    // two emulated guests never contend for the starved host concurrently.
    let _tcg_slot = tcg_serial_slot(platform);

    let result = match execute_row(case, ctx, platform, started) {
        Ok(result) => result,
        Err(error) => CaseResult::infra_failed(case, platform, started, error.to_string()),
    };
    // Plan WP4: PASSing rows shed their staged images so parallel batches
    // don't accumulate disk; failures (and --keep-all) retain everything.
    if matches!(result.outcome, Outcome::Passed) && !ctx.keep_all {
        if let Err(error) = isolation::discard_stage_dir(&ctx.workspace_root, &case.name, 0) {
            eprintln!("    note: stage prune skipped for {}: {error}", case.name);
        }
    }
    result
}

fn execute_row(
    case: &CaseDef,
    ctx: &RunCtx,
    platform: &str,
    started: Instant,
) -> Result<CaseResult, Box<dyn Error>> {
    // raspi5 is build-only: no QEMU, no emulator prerequisite.
    if platform == "raspi5" {
        return execute_raspi_image_assertion(case, ctx, platform, started);
    }

    if let Some(reason) = qemu::missing_emulator_reason(platform) {
        return Ok(CaseResult::skipped(
            case,
            platform,
            started,
            reason.to_owned(),
        ));
    }

    // Only UEFI (qemu-virtio) boots consume a slot disk copy; the kernel-ELF /
    // Image platforms (-machine pc, aarch64 virt, riscv64-virt) take their
    // payload straight from BuildArtifacts and stage no block device, so
    // passing the bundle DIRECTORY here would make fs::copy explode
    // ("neither a regular file nor a symlink"). create_platform_image already
    // returned a fresh serviceos.img FILE for virtio (mirrors validate.rs:195).
    // The staged disk/data images are REUSED verbatim across a boot-B phase so
    // first-boot state written by boot A is visible to boot B (plan §T4
    // wizard-first-boot-chain row; pattern reused from upgrade.rs).
    let artifacts = ctx.artifacts_for_case(case, platform)?;
    let built_disk = match platform {
        "qemu-virtio" => Some(artifacts.image.as_path().to_path_buf()),
        _ => None,
    };
    let paths = isolation::stage_case_images(
        &ctx.workspace_root,
        case,
        platform,
        built_disk.as_deref(),
        0,
    )?;

    // Per-row env pairs for argv assembly: hermetic headless boot, the slot's
    // own throwaway OVMF vars overlay, and any case-declared launch env
    // (e.g. the plan §2.5 audio pair). Captured under the process-wide spec
    // gate so concurrent rows never interleave ambient-env reads/writes; the
    // returned spec replays the snapshot verbatim at spawn time.
    let mut env_pairs = vec![("QEMU_HEADLESS".to_owned(), "1".to_owned())];
    if let Some(vars_path) = &paths.ovmf_vars {
        env_pairs.push((
            "SERVICEOS_OVMF_VARS".to_owned(),
            vars_path.display().to_string(),
        ));
    }
    for (key, value) in &case.qemu_env {
        env_pairs.push((key.clone(), value.clone()));
    }
    let mut spec = qemu::capture_spec(platform, &artifacts.artifacts, &paths, &env_pairs)?;

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
    let verdict_a = run_single_boot(case, &spec, deadline_a, idle_budget(case, ctx));
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
                case.name,
                platform,
                paths.dir.display()
            );
            let deadline_b = Instant::now() + ctx.case_timeout(case);
            let boot_b = second_boot_verdict(case, &spec, deadline_b, idle_budget(case, ctx));
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
            ));
        }
        BootOutcome::Failed(reason, None) => {
            return Ok(CaseResult::failed(
                case,
                platform,
                started,
                reason,
                String::new(),
            ));
        }
        BootOutcome::Infra(reason) => {
            return Ok(CaseResult::infra_failed(case, platform, started, reason));
        }
    }
}

/// One serial-phase run over an already-built spec: spawn, optional script,
/// witness drive, kill. Output text is kept for failure diagnostics.
/// `deadline` bounds the whole phase; `idle` is the no-output watchdog
/// (plan §2.3 separate knob) that trips when the console goes quiet.
fn run_single_boot(
    case: &CaseDef,
    spec: &qemu::QemuSpec,
    deadline: Instant,
    idle: Duration,
) -> BootOutcome {
    let mut session = match SerialSession::spawn(spec.clone(), idle) {
        Ok(session) => session,
        Err(error) => return BootOutcome::Infra(format!("QEMU spawn failed: {error}")),
    };

    if let Some(script_path) = case.serial_script.as_deref() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if let Err(error) = run_script(script_path, &mut session, remaining) {
            let output = session.kill();
            return BootOutcome::Failed(format!("serial script failed: {error}"), Some(output));
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
fn second_boot_verdict(
    case: &CaseDef,
    spec: &qemu::QemuSpec,
    deadline: Instant,
    idle: Duration,
) -> BootOutcome {
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

    let mut session = match SerialSession::spawn(spec.clone(), idle) {
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
        println!("=== e2e gate switch ({cache_key}): purge stale guest artifacts ===");
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

/// Copy one build output (file or directory bundle) into
/// `<workspace>/target/e2e/builds/<tuple>/`, giving each gate tuple a
/// private, stable image path for the whole invocation.
fn snapshot_build_output(
    workspace_root: &Path,
    built: &Path,
    cache_key: &str,
) -> Result<PathBuf, Box<dyn Error>> {
    let tuple_dir = workspace_root
        .join("target")
        .join("e2e")
        .join("builds")
        .join(isolation::sanitize_case_name(cache_key));
    let destination = if built.is_dir() {
        if tuple_dir.exists() {
            std::fs::remove_dir_all(&tuple_dir)?;
        }
        std::fs::create_dir_all(&tuple_dir)?;
        let into = tuple_dir.join(built.file_name().ok_or("bundle output lacks name")?);
        isolation::copy_tree(built, &into)?;
        into
    } else {
        std::fs::create_dir_all(&tuple_dir)?;
        let into = tuple_dir.join(built.file_name().ok_or("image output lacks name")?);
        std::fs::copy(built, &into)?;
        into
    };
    Ok(destination)
}

/// Idle (no-output) watchdog budget for one boot phase: the separate knob
/// from plan §2.3. Priority: case `idle_timeout_secs` (long-silent guest
/// workloads) > `SERVICEOS_IDLE_TIMEOUT_SECS` env > the smaller of the
/// constant default and the case's total budget, so a wedged console
/// releases its worker slot well before the per-case deadline.
fn idle_budget(case: &CaseDef, ctx: &RunCtx) -> Duration {
    let budget_secs = ctx.case_timeout(case).as_secs();
    let secs = case
        .idle_timeout_secs
        .or_else(|| {
            std::env::var("SERVICEOS_IDLE_TIMEOUT_SECS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|secs| *secs > 0)
        })
        .unwrap_or_else(|| DEFAULT_IDLE_TIMEOUT_SECS.min(budget_secs));
    Duration::from_secs(secs.max(1))
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
    Passed {
        #[allow(dead_code)]
        text: String,
    },
    Failed(String, Option<String>),
    Infra(String),
}

fn drive_witnesses(case: &CaseDef, session: &mut SerialSession, deadline: Instant) -> StepVerdict {
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
                return StepVerdict::Failed(format!("fail_on matched early: {}", pattern.raw()));
            }
        }
        for (index, pattern) in witnesses.iter().enumerate() {
            if !satisfied[index] && pattern.satisfied(&text) {
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
                ));
            }
        }
    }
}

fn execute_raspi_image_assertion(
    case: &CaseDef,
    ctx: &RunCtx,
    platform: &str,
    started: Instant,
) -> Result<CaseResult, Box<dyn Error>> {
    let bundle_root = ctx.artifacts_for_case(case, platform)?.image.clone();
    let mut missing = Vec::new();
    for rel in RASPI_STAGED_FILES {
        let candidate = bundle_root.join(rel);
        if !candidate.exists() {
            missing.push(rel.to_owned());
        } else {
            let size = std::fs::metadata(&candidate)
                .map(|meta| meta.len())
                .unwrap_or(0);
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

#[cfg(test)]
mod tcg_gate_tests {
    use super::*;

    #[test]
    fn tcg_platform_set_follows_the_registry() {
        assert!(platform_uses_tcg("virt"));
        assert!(platform_uses_tcg("riscv64-virt"));
        assert!(platform_uses_tcg("qemu-isa"));
        assert!(!platform_uses_tcg("raspi5"));
        assert!(platform_uses_tcg("mystery-platform"));
    }

    #[test]
    fn qemu_virtio_accel_decision_mirrors_run_rs() {
        assert!(accel_mode_forces_tcg(Some("tcg"), true));
        assert!(!accel_mode_forces_tcg(Some("kvm"), false));
        assert!(accel_mode_forces_tcg(None, false));
        assert!(!accel_mode_forces_tcg(None, true));
    }

    #[test]
    fn tcg_slot_serializes_only_tcg_platforms() {
        // KVM/no-emulator platforms never touch the gate.
        assert!(tcg_serial_slot("raspi5").is_none());
        let _gate = tcg_serial_slot("virt");
        // A second TCG row on another worker would block here; same-thread
        // reentry would deadlock, so only assert the gate is held via the
        // poisoning-free try path through a fresh handle.
        assert!(matches!(
            TCG_EXECUTION.try_lock(),
            Err(TryLockError::WouldBlock)
        ));
    }
}

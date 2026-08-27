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
    path::PathBuf,
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
    pub fn ensure_build(&mut self, platform: &str) -> Result<(), Box<dyn Error>> {
        if self.builds.contains_key(platform) {
            return Ok(());
        }
        let spec = PlatformSpec::resolve(platform)?;
        println!("=== e2e build: {platform} ===");
        let artifacts = build_for_platform(spec, self.release)?;
        let image = create_platform_image(&artifacts)?;
        println!(
            "=== e2e build complete: {platform} at {} ===",
            image.display()
        );
        self.builds.insert(
            platform.to_owned(),
            PlatformBuild {
                artifacts,
                image,
                release: self.release,
            },
        );
        Ok(())
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

    ctx.ensure_build(platform)?;

    // Only UEFI (qemu-virtio) boots consume a slot disk copy; the kernel-ELF /
    // Image platforms (-machine pc, aarch64 virt, riscv64-virt) take their
    // payload straight from BuildArtifacts and stage no block device, so
    // passing the bundle DIRECTORY here would make fs::copy explode
    // ("neither a regular file nor a symlink"). create_platform_image already
    // returned a fresh serviceos.img FILE for virtio (mirrors validate.rs:195).
    let built_disk = match platform {
        "qemu-virtio" => Some(ctx.builds[platform].image.as_path()),
        _ => None,
    };
    let paths = isolation::stage_case_images(
        &ctx.workspace_root,
        case,
        platform,
        built_disk,
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
    let artifacts = &ctx.builds[platform].artifacts;
    let mut spec = qemu::spec_for(platform, artifacts, &paths)?;
    drop(_guard);

    println!(
        "=== e2e boot: {} @ {} (slot {}) ===",
        case.name,
        platform,
        paths.dir.display()
    );

    let deadline = Instant::now() + ctx.case_timeout(case);
    // Witness-only boots mirror the proven bounded-boot launcher byte shape
    // (Stdio::null stdin) so console-driven default handling cannot diverge
    // from `cargo xtask` history; scripted cases opt into the injection pipe.
    // Loader maps `serial_script = ""` to None, so Some ⇒ keep stdin piped.
    let scripted = case.serial_script.is_some();
    spec.stdin_piped = scripted;
    let mut session = match SerialSession::spawn(spec, ctx.case_timeout(case)) {
        Ok(session) => session,
        Err(error) => {
            return Ok(CaseResult::infra_failed(
                case,
                platform,
                started,
                format!("QEMU spawn failed: {error}"),
            ))
        }
    };

    // Scripted cases (plan §2.4) type through the operator console after
    // boot; their directives anchor each send on a prior expect so bytes are
    // never dropped outside an armed readline session. Remaining wall budget
    // is shared across the script's expects; witnesses still gate the final
    // verdict afterwards so loose sequencing cannot mask a missing result.
    if let Some(script_path) = case.serial_script.as_deref() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if let Err(error) = run_script(script_path, &mut session, remaining) {
            let output = session.kill();
            return Ok(CaseResult::failed(
                case,
                platform,
                started,
                format!("serial script failed: {error}"),
                tail_of(&output),
            ));
        }
    }

    let verdict = drive_witnesses(case, &mut session, deadline);
    let output = session.kill();

    Ok(match verdict {
        StepVerdict::Passed => CaseResult::passed(case, platform, started),
        StepVerdict::Failed(reason) => {
            CaseResult::failed(case, platform, started, reason, tail_of(&output))
        }
        StepVerdict::Infra(reason) => {
            CaseResult::infra_failed(case, platform, started, format!("{reason}"))
        }
    })
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

    let mut satisfied = vec![false; witnesses.len()];
    loop {
        let snapshot = session.snapshot();
        let text = snapshot.text().to_owned();

        for pattern in &fail_on {
            if pattern.matches(&text) {
                return StepVerdict::Failed(format!(
                    "fail_on matched early: {}",
                    pattern.raw()
                ));
            }
        }
        for (index, pattern) in witnesses.iter().enumerate() {
            if !satisfied[index] && pattern.matches(&text) {
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
    ctx.ensure_build(platform)?;
    let bundle_root = &ctx.builds[platform].image;
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

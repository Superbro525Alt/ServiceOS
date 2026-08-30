//! `cargo xtask test-e2e` — end-to-end suite orchestration per
//! docs/test-plan.md §4: tier/filter selection, one serial pre-build per
//! platform+gate-tuple, tier-ordered parallel row scheduling behind the same
//! RunCtx (WP4), TAP-style reporting, and the 0/1/2 exit-code contract.

use std::{error::Error, path::PathBuf, time::Instant};

use serviceos_e2e::{self as e2e, CaseDef, CaseResult, Outcome, RunCtx};
use xtask_core::build::workspace_root;

#[derive(Debug, Default)]
struct TestE2eOptions {
    platform: Option<String>,
    tier: Option<u8>,
    filter: Option<String>,
    tag: Option<String>,
    jobs: Option<usize>,
    timeout_secs: Option<u64>,
    report: Option<PathBuf>,
    keep_all: bool,
    release: bool,
    list: bool,
}

const USAGE_TEST_E2E: &str = "usage: cargo xtask test-e2e [--platform <qemu-virtio|raspi5|virt|qemu-isa|riscv64-virt>] [--tier <1..4>]\n       [--filter <substr-or-regex>] [--tag <t>] [-j <n>] [--timeout-secs <s>] [--report <path>] [--keep-all] [--release] [--list]";

/// Sane ceiling for concurrent QEMU slots (plan §2.5: RAM-budgeted batches);
/// `-j` beyond this is refused rather than silently swapping the host.
const MAX_JOBS: usize = 8;

pub fn run_test_e2e(
    _cli_platform: &str,
    cli_release: bool,
    args: &[String],
) -> Result<(), Box<dyn Error>> {
    let options = parse_options(args);
    match execute(options, cli_release) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("test-e2e failed: {error}");
            std::process::exit(2);
        }
    }
}

fn usage_error(message: &str) -> ! {
    eprintln!("{USAGE_TEST_E2E}\n\nerror: {message}");
    std::process::exit(2);
}

fn parse_options(args: &[String]) -> TestE2eOptions {
    let mut options = TestE2eOptions::default();
    let mut index = 0usize;
    while index < args.len() {
        let flag = args[index].as_str();
        let take_value = |index: &mut usize| -> String {
            *index += 1;
            match args.get(*index) {
                Some(value) => value.clone(),
                None => usage_error(&format!("{flag} requires a value")),
            }
        };
        match flag {
            "--list" => options.list = true,
            "--release" => options.release = true,
            "--keep-all" => options.keep_all = true,
            other => {
                if let Some(value) = other.strip_prefix("--platform=") {
                    options.platform = Some(value.to_owned());
                } else if let Some(value) = other.strip_prefix("--tier=") {
                    options.tier = Some(parse_tier(value));
                } else if let Some(value) = other.strip_prefix("--filter=") {
                    options.filter = Some(value.to_owned());
                } else if let Some(value) = other.strip_prefix("--tag=") {
                    options.tag = Some(value.to_owned());
                } else if let Some(value) = other.strip_prefix("-j").filter(|rest| !rest.is_empty())
                {
                    options.jobs = Some(parse_jobs(value));
                } else if let Some(value) = other.strip_prefix("--jobs=") {
                    options.jobs = Some(parse_jobs(value));
                } else if let Some(value) = other.strip_prefix("--timeout-secs=") {
                    options.timeout_secs = Some(parse_secs(value));
                } else if let Some(value) = other.strip_prefix("--report=") {
                    options.report = Some(PathBuf::from(value));
                } else {
                    let value = take_value(&mut index);
                    match flag {
                        "--platform" => options.platform = Some(value),
                        "--tier" => options.tier = Some(parse_tier(&value)),
                        "--filter" => options.filter = Some(value),
                        "--tag" => options.tag = Some(value),
                        "-j" | "--jobs" => options.jobs = Some(parse_jobs(&value)),
                        "--timeout-secs" => options.timeout_secs = Some(parse_secs(&value)),
                        "--report" => options.report = Some(PathBuf::from(value)),
                        unknown => usage_error(&format!("unknown flag {unknown:?}")),
                    }
                }
            }
        }
        index += 1;
    }
    options
}

fn parse_tier(raw: &str) -> u8 {
    raw.parse::<u8>()
        .ok()
        .filter(|tier| (1..=4).contains(tier))
        .unwrap_or_else(|| usage_error("--tier must be an integer within 1..=4"))
}

fn parse_jobs(raw: &str) -> usize {
    raw.trim_start_matches(|c: char| c == '=')
        .parse::<usize>()
        .ok()
        .filter(|jobs| (1..=MAX_JOBS).contains(jobs))
        .unwrap_or_else(|| {
            usage_error(&format!(
                "-j/--jobs must be an integer within 1..={MAX_JOBS}"
            ))
        })
}

fn parse_secs(raw: &str) -> u64 {
    raw.trim_start_matches(|c: char| c == '=')
        .parse::<u64>()
        .ok()
        .filter(|secs| *secs > 0)
        .unwrap_or_else(|| usage_error("--timeout-secs must be positive"))
}

fn case_matches_platform(case: &CaseDef, selected: Option<&str>) -> bool {
    match selected {
        Some(platform) => case.platforms.iter().any(|declared| declared == platform),
        None => true,
    }
}

fn case_matches_tag(case: &CaseDef, tag: Option<&str>) -> bool {
    match tag {
        Some(tag) => case.tags.iter().any(|declared| declared == tag),
        None => true,
    }
}

fn case_matches_filter(case: &CaseDef, filter: Option<&str>) -> bool {
    match filter {
        None => true,
        Some(pattern) => match e2e::Pattern::new(pattern) {
            Ok(compiled) => compiled.matches(&case.name),
            Err(_) => case.name.contains(pattern),
        },
    }
}

fn execute(options: TestE2eOptions, cli_release: bool) -> Result<i32, Box<dyn Error>> {
    let release = options.release || cli_release;
    let root = workspace_root();
    let started_at = Instant::now();

    let all_cases = e2e::load_cases(&root.join("tests").join("cases"))?;
    println!(
        "loaded {} e2e case file(s) from {}",
        all_cases.len(),
        root.join("tests/cases").display()
    );

    let active: Vec<&CaseDef> = all_cases
        .iter()
        .filter(|case| options.tier.map_or(true, |max| case.tier <= max))
        .filter(|case| case_matches_platform(case, options.platform.as_deref()))
        .filter(|case| case_matches_filter(case, options.filter.as_deref()))
        .filter(|case| case_matches_tag(case, options.tag.as_deref()))
        .collect();

    if options.list {
        for case in &active {
            println!(
                "T{}  {:<34} platforms={} tags=[{}] witnesses={}",
                case.tier,
                case.name,
                case.platforms.join(","),
                case.tags.join(","),
                case.witnesses.len(),
            );
        }
        println!("total={}", active.len());
        return Ok(0);
    }

    if options.tier.is_none() {
        println!("note: T0 host unit tests are not part of test-e2e; run `cargo test --workspace`");
    }

    let jobs = options.jobs.unwrap_or(1);

    // WP3: env_build guest gates now plumb into per-tuple cached builds
    // (serviceos_e2e::RunCtx::ensure_build); the fingerprint file remains a
    // cross-invocation audit trail keyed to the selected set.
    if active.iter().any(|case| !case.env_build.is_empty()) {
        write_build_fingerprint(&active)?;
    }

    // Build rows first so the pre-build pass can walk them in the same
    // tuple-grouped order the sequential runner used (gate-switch purges of
    // the shared guest target dir stay minimal and correctly ordered).
    let mut ordered: Vec<&CaseDef> = Vec::with_capacity(active.len());
    {
        let mut tuple_order: Vec<Vec<(String, String)>> = Vec::new();
        for case in &active {
            let mut flags = case.env_build.clone();
            flags.sort();
            if !tuple_order.contains(&flags) {
                tuple_order.push(flags);
            }
        }
        for flags in &tuple_order {
            for case in active.iter().filter(|case| {
                let mut own = case.env_build.clone();
                own.sort();
                own == *flags
            }) {
                ordered.push(case);
            }
        }
    }

    let mut rows: Vec<(&CaseDef, String)> = ordered
        .iter()
        .flat_map(|case| {
            case.platforms
                .iter()
                .map(move |platform| (*case, platform.clone()))
        })
        .collect();
    // Serial pre-build phase: hydrate every needed platform+tuple build
    // before any worker thread starts (cargo, the gate env guard, and the
    // guest-artifact marker file are process-global; builds never overlap).
    let mut ctx = RunCtx::new(root.clone(), jobs, release);
    ctx.timeout_override = options.timeout_secs;
    ctx.keep_all = options.keep_all;
    for (case, platform) in &rows {
        ctx.ensure_build(platform, &case.env_build)?;
    }

    // Scheduling fairness: fast low-tier smokes grab slots first while the
    // long TCG / high-tier boots land last; case name then platform keep the
    // order deterministic run-to-run. (Build hydration above stays in
    // tuple-grouped file order; only BOOT scheduling is reordered.)
    rows.sort_by(|(case_a, platform_a), (case_b, platform_b)| {
        case_a
            .tier
            .cmp(&case_b.tier)
            .then_with(|| case_a.name.cmp(&case_b.name))
            .then_with(|| platform_a.cmp(platform_b))
    });

    // Deterministic result buffer: one slot per scheduled row, filled by
    // whichever worker finishes it; summaries re-sort by case name below so
    // completion order never leaks into the report.
    let results: Vec<std::sync::Mutex<Option<CaseResult>>> = (0..rows.len())
        .map(|_| std::sync::Mutex::new(None))
        .collect();
    let next_row = std::sync::atomic::AtomicUsize::new(0);
    let workers = jobs.min(rows.len()).max(1);
    println!(
        "\nscheduling {} row(s) across {workers} worker slot(s) (tier-ordered)",
        rows.len()
    );
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next_row.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if index >= rows.len() {
                        break;
                    }
                    let (case, platform) = &rows[index];
                    let (case, platform) = (*case, platform.as_str());
                    println!(
                        "\n=== case {}: T{} targets {:?} ===",
                        case.name, case.tier, case.platforms
                    );
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        e2e::run_case(case, &ctx, platform)
                    }));
                    let row = outcome.unwrap_or_else(|panic| {
                        CaseResult::infra_failed(
                            case,
                            platform,
                            Instant::now(),
                            format!("worker thread panicked: {panic:?}"),
                        )
                    });
                    let mut slot = results[index]
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    *slot = Some(row);
                }
            });
        }
    });
    let mut results: Vec<CaseResult> = results
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        })
        .map(|filled| filled.expect("every scheduled row produced a result"))
        .collect();
    // Deterministic summary + TAP ordering regardless of completion order.
    results.sort_by(|a, b| {
        a.case
            .cmp(&b.case)
            .then_with(|| a.platform.cmp(&b.platform))
    });

    e2e::print_summary_table(&mut std::io::stdout().lock(), &results)?;

    if let Some(report_path) = &options.report {
        e2e::write_tap(report_path, &results, started_at.elapsed())?;
        println!("wrote TAP report: {}", report_path.display());
    }

    let any_fail = results
        .iter()
        .any(|row| matches!(row.outcome, Outcome::Failed { .. }));
    let code = e2e::aggregate(&results);
    if code == 2 && !any_fail {
        println!("infrastructure failure (exit 2): see rows above");
    }
    Ok(code)
}

fn write_build_fingerprint(active: &[&CaseDef]) -> Result<(), Box<dyn Error>> {
    let path = workspace_root()
        .join("target")
        .join("e2e")
        .join("build-fingerprint.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = String::from("{\n  \"cases\": [\n");
    for (index, case) in active.iter().enumerate() {
        body.push_str(&format!(
            "    {{\"name\": \"{}\", \"env\": [",
            case.name.replace('"', "\\\"")
        ));
        for (position, (key, value)) in case.env_build.iter().enumerate() {
            if position > 0 {
                body.push_str(", ");
            }
            body.push_str(&format!("\"{key}={value}\""));
        }
        body.push_str("]}");
        if index + 1 < active.len() {
            body.push(',');
        }
        body.push('\n');
    }
    body.push_str("  ]\n}\n");
    std::fs::write(&path, body)?;
    println!("noted env fingerprint at {}", path.display());
    Ok(())
}

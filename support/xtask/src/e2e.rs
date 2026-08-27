//! `cargo xtask test-e2e` — end-to-end suite orchestration per
//! docs/test-plan.md §4: tier/filter selection, one build per platform,
//! sequential execution (parallel scheduling lands in WP4 behind the same
//! RunCtx), TAP-style reporting, and the 0/1/2 exit-code contract.

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
    release: bool,
    list: bool,
}

const USAGE_TEST_E2E: &str = "usage: cargo xtask test-e2e [--platform <qemu-virtio|raspi5|virt|qemu-isa|riscv64-virt>] [--tier <1..4>]\n       [--filter <substr-or-regex>] [--tag <t>] [-j <n>] [--timeout-secs <s>] [--report <path>] [--release] [--list]";

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
            other => {
                if let Some(value) = other.strip_prefix("--platform=") {
                    options.platform = Some(value.to_owned());
                } else if let Some(value) = other.strip_prefix("--tier=") {
                    options.tier = Some(parse_tier(value));
                } else if let Some(value) = other.strip_prefix("--filter=") {
                    options.filter = Some(value.to_owned());
                } else if let Some(value) = other.strip_prefix("--tag=") {
                    options.tag = Some(value.to_owned());
                } else if let Some(value) = other.strip_prefix("-j") {
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
        .filter(|jobs| *jobs >= 1)
        .unwrap_or_else(|| usage_error("-j/--jobs must be >= 1"))
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

    let jobs = options
        .jobs
        .unwrap_or(4)
        .clamp(1, usize::from(u16::MAX));
    if jobs > 1 {
        // Structure exists today; the semaphore worker pool that actually
        // interleaves boots is WP4 scope. Sequencing stays deterministic.
        println!(
            "note: -j {jobs} recorded in RunCtx; scheduling is sequential until WP4 lands"
        );
    }

    // WP3: env_build guest gates now plumb into per-tuple cached builds
    // (serviceos_e2e::RunCtx::ensure_build); the fingerprint file remains a
    // cross-invocation audit trail keyed to the selected set.
    if active.iter().any(|case| !case.env_build.is_empty()) {
        write_build_fingerprint(&active)?;
    }

    // Group rows by their flag tuple so gated builds are compiled contiguously
    // instead of ping-ponging rebuilds between default- and flagged-image
    // tuples across the sequential schedule. Order within a tuple keeps file
    // order; tuple order follows first appearance.
    let mut tuple_order: Vec<Vec<(String, String)>> = Vec::new();
    for case in &active {
        let mut flags = case.env_build.clone();
        flags.sort();
        if !tuple_order.contains(&flags) {
            tuple_order.push(flags);
        }
    }
    let mut ordered: Vec<&CaseDef> = Vec::with_capacity(active.len());
    for flags in &tuple_order {
        for case in active.iter().filter(|case| {
            let mut own = case.env_build.clone();
            own.sort();
            own == *flags
        }) {
            ordered.push(case);
        }
    }

    let mut ctx = RunCtx::new(root.clone(), jobs, release);
    ctx.timeout_override = options.timeout_secs;

    let mut results: Vec<CaseResult> = Vec::new();
    for case in &ordered {
        println!("\n=== case {}: T{} targets {:?} ===", case.name, case.tier, case.platforms);
        let row_platforms: Vec<String> = case.platforms.clone();
        for platform in row_platforms {
            ctx.current_platform = platform;
            results.push(e2e::run_case(case, &mut ctx));
        }
    }

    e2e::print_summary_table(&mut std::io::stdout().lock(), &results)?;

    if let Some(report_path) = &options.report {
        e2e::write_tap(
            report_path,
            &results,
            started_at.elapsed(),
        )?;
        println!("wrote TAP report: {}", report_path.display());
    }

    let any_fail = results.iter().any(|row| matches!(row.outcome, Outcome::Failed { .. }));
    let code = e2e::aggregate(&results);
    if code == 2 && !any_fail {
        println!("infrastructure failure (exit 2): see rows above");
    }
    Ok(code)
}

fn write_build_fingerprint(active: &[&CaseDef]) -> Result<(), Box<dyn Error>> {
    let path = workspace_root().join("target").join("e2e").join("build-fingerprint.json");
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

//! Result aggregation: per-row outcomes, human summary table, TAP-ish file
//! output, and the §4 exit-code contract (0 pass/skip, 1 failure, 2 infra).

use std::{
    fmt,
    io::{self, Write},
    path::Path,
    time::Duration,
};

pub const TAIL_DUMP_LINES: usize = 30;

#[derive(Debug, Clone)]
pub enum Outcome {
    Passed,
    Failed {
        reason: String,
        tail: String,
    },
    Skipped {
        reason: String,
    },
    /// Infrastructure failure not attributable to case behavior: image build
    /// problems, QEMU spawn errors, harness misconfiguration.
    InfraFailed {
        reason: String,
        tail: String,
    },
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Passed => f.write_str("PASS"),
            Self::Skipped { .. } => f.write_str("SKIP"),
            Self::Failed { .. } => f.write_str("FAIL"),
            Self::InfraFailed { .. } => f.write_str("ERR"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CaseResult {
    pub case: String,
    pub tier: u8,
    pub platform: String,
    pub outcome: Outcome,
    pub elapsed: Duration,
}

impl CaseResult {
    pub fn passed(
        case: &crate::case::CaseDef,
        platform: &str,
        started_at: std::time::Instant,
    ) -> Self {
        Self {
            case: case.name.clone(),
            tier: case.tier,
            platform: platform.to_owned(),
            outcome: Outcome::Passed,
            elapsed: started_at.elapsed(),
        }
    }

    pub fn failed(
        case: &crate::case::CaseDef,
        platform: &str,
        started_at: std::time::Instant,
        reason: String,
        tail: String,
    ) -> Self {
        Self {
            case: case.name.clone(),
            tier: case.tier,
            platform: platform.to_owned(),
            outcome: Outcome::Failed { reason, tail },
            elapsed: started_at.elapsed(),
        }
    }

    pub fn skipped(
        case: &crate::case::CaseDef,
        platform: &str,
        started_at: std::time::Instant,
        reason: String,
    ) -> Self {
        Self {
            case: case.name.clone(),
            tier: case.tier,
            platform: platform.to_owned(),
            outcome: Outcome::Skipped { reason },
            elapsed: started_at.elapsed(),
        }
    }

    pub fn infra_failed(
        case: &crate::case::CaseDef,
        platform: &str,
        started_at: std::time::Instant,
        reason: String,
    ) -> Self {
        Self {
            case: case.name.clone(),
            tier: case.tier,
            platform: platform.to_owned(),
            outcome: Outcome::InfraFailed {
                reason,
                tail: String::new(),
            },
            elapsed: started_at.elapsed(),
        }
    }
}

pub struct Summary {
    pub pass: usize,
    pub fail: usize,
    pub skip: usize,
    pub infra: usize,
    pub wall: Duration,
}

/// Exit-code contract: 0 all pass/skip; 1 assertion failures; 2 infra.
/// Infrastructure dominates when both appear (CI wants the loudest signal).
pub fn aggregate(results: &[CaseResult]) -> i32 {
    if results.iter().any(|row| matches!(row.outcome, Outcome::InfraFailed { .. })) {
        return 2;
    }
    if results.iter().any(|row| matches!(row.outcome, Outcome::Failed { .. })) {
        return 1;
    }
    0
}

pub fn summarize(results: &[CaseResult]) -> Summary {
    let mut summary = Summary {
        pass: 0,
        fail: 0,
        skip: 0,
        infra: 0,
        wall: Duration::ZERO,
    };
    for row in results {
        summary.wall += row.elapsed;
        match row.outcome {
            Outcome::Passed => summary.pass += 1,
            Outcome::Failed { .. } => summary.fail += 1,
            Outcome::Skipped { .. } => summary.skip += 1,
            Outcome::InfraFailed { .. } => summary.infra += 1,
        }
    }
    summary
}

pub fn print_summary_table(out: &mut dyn Write, results: &[CaseResult]) -> io::Result<()> {
    writeln!(out, "\n== ServiceOS E2E ==")?;
    writeln!(
        out,
        "{:<34} {:<5} {:<12} {:<6} {:>9}  detail",
        "case", "tier", "platform", "result", "elapsed"
    )?;
    for row in results {
        let detail = match &row.outcome {
            Outcome::Passed => String::new(),
            Outcome::Skipped { reason } | Outcome::Failed { reason, .. }
            | Outcome::InfraFailed { reason, .. } => reason.clone(),
        };
        writeln!(
            out,
            "{:<34} {:<5} {:<12} {:<6} {:>8.1}s  {}",
            row.case,
            row.tier,
            row.platform,
            row.outcome,
            row.elapsed.as_secs_f64(),
            first_line(&detail),
        )?;
    }
    let summary = summarize(results);
    writeln!(
        out,
        "pass={} fail={} skip={} err={}",
        summary.pass, summary.fail, summary.skip, summary.infra
    )
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

/// TAP-ish stream per docs/test-plan.md §4 including bounded tails.
pub fn write_tap(path: &Path, results: &[CaseResult], wall: Duration) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = io::BufWriter::new(std::fs::File::create(path)?);
    let total = results.len();
    for (index, row) in results.iter().enumerate() {
        let number = index + 1;
        let meta = format!("elapsed {:.1}s platform={}", row.elapsed.as_secs_f64(), row.platform);
        match &row.outcome {
            Outcome::Passed => {
                writeln!(out, "ok {number} - {} # {meta}", row.case)?;
            }
            Outcome::Skipped { reason } => {
                writeln!(out, "ok {number} - {} # SKIP {reason}; {meta}", row.case)?;
            }
            Outcome::Failed { reason, tail } => {
                writeln!(out, "not ok {number} - {} # {reason}; {meta}", row.case)?;
                write_tail_block(&mut out, tail)?;
            }
            Outcome::InfraFailed { reason, tail } => {
                writeln!(out, "not ok {number} - {} # INFRA {reason}; {meta}", row.case)?;
                write_tail_block(&mut out, tail)?;
            }
        }
    }
    let summary = summarize(results);
    writeln!(
        out,
        "1..{} # plan=pass={} fail={} skip={} duration_wall={:.1}s",
        total, summary.pass, summary.fail, summary.skip, wall.as_secs_f64()
    )?;
    Ok(())
}

fn write_tail_block<W: Write>(out: &mut W, tail: &str) -> io::Result<()> {
    let bounded: Vec<&str> = tail.lines().rev().take(TAIL_DUMP_LINES).collect::<Vec<_>>();
    if bounded.is_empty() {
        return Ok(());
    }
    writeln!(out, "TAIL_START")?;
    for line in bounded.into_iter().rev() {
        writeln!(out, "{line}")?;
    }
    writeln!(out, "TAIL_END")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::case::CaseDef;

    fn stub_def(name: &str, tier: u8) -> CaseDef {
        CaseDef {
            source_path: Path::new(format!("/tmp/fake/{name}.toml").as_str()).to_path_buf(),
            name: name.to_owned(),
            tier,
            platforms: vec!["qemu-virtio".to_owned()],
            timeout_secs: None,
            witnesses: vec!["marker".to_owned()],
            fail_on: Vec::new(),
            env_build: Vec::new(),
            qemu_env: Vec::new(),
            probes: Vec::new(),
            serial_script: None,
            data_fresh: true,
            tags: Vec::new(),
            mode: crate::case::WitnessMode::Witness,
            graph: String::new(),
        }
    }

    fn result(name: &str, outcome: Outcome) -> CaseResult {
        let def = stub_def(name, 1);
        CaseResult {
            case: def.name,
            tier: def.tier,
            platform: "qemu-virtio".to_owned(),
            outcome,
            elapsed: Duration::from_secs_f64(3.0),
        }
    }

    #[test]
    fn exit_codes_follow_plan_contract() {
        // All pass/skip => 0.
        let rows = vec![
            result("a", Outcome::Passed),
            result("b", Outcome::Skipped { reason: "no emulator".into() }),
        ];
        assert_eq!(aggregate(&rows), 0);

        // Any assertion failure => 1 even with passes present.
        let rows = vec![
            result("a", Outcome::Passed),
            result("b", Outcome::Failed { reason: "timeout".into(), tail: String::new() }),
        ];
        assert_eq!(aggregate(&rows), 1);

        // Infra outranks everything => 2.
        let rows = vec![
            result("a", Outcome::Failed { reason: "x".into(), tail: String::new() }),
            result("b", Outcome::InfraFailed { reason: "spawn died".into(), tail: String::new() }),
        ];
        assert_eq!(aggregate(&rows), 2);
    }

    #[test]
    fn tap_output_includes_numbered_rows_and_footer() {
        let rows = vec![result("good", Outcome::Passed)];
        let path = Path::new("/tmp/serviceos-e2e-tap-test.tap");
        write_tap(path, &rows, Duration::from_secs(7)).expect("tap");
        let text = std::fs::read_to_string(path).expect("read");
        assert!(text.starts_with("ok 1 - good # "));
        assert!(text.ends_with("1..1 # plan=pass=1 fail=0 skip=0 duration_wall=7.0s\n"));
        let _ = std::fs::remove_file(path);
    }
}

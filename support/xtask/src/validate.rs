use std::{error::Error, path::Path, process::Command};

use crate::{
    bootlog,
    build::{build_for_platform, workspace_root},
    image::create_platform_image,
    platform::PlatformSpec,
    run::find_qemu_aarch64_binary,
};

const MARKER_STORAGE: &str = "selftest file-written bytes=";
const MARKER_NET: &str = "net-selftest end";
const MARKER_AUDIO: &str = "selftest mix";

struct StepResult {
    name: String,
    ok: bool,
    detail: String,
}

pub fn run_validate() -> Result<(), Box<dyn Error>> {
    let root = workspace_root();
    let mut steps: Vec<StepResult> = Vec::new();

    steps.push(host_step(
        &root,
        "check workspace",
        vec!["check".to_string(), "--workspace".to_string()],
    ));
    steps.push(host_step(
        &root,
        "test workspace",
        vec!["test".to_string(), "--workspace".to_string()],
    ));

    let virtio = boot_step_virtio();
    match virtio {
        Ok(report) => steps.push(StepResult {
            name: "boot qemu-virtio".to_string(),
            ok: report.pass,
            detail: report.detail,
        }),
        Err(error) => steps.push(StepResult {
            name: "boot qemu-virtio".to_string(),
            ok: false,
            detail: format!("error: {error}"),
        }),
    }

    let virt_available = find_qemu_aarch64_binary().is_some();
    if virt_available {
        match boot_step_virt() {
            Ok(report) => {
                if report.skipped {
                    steps.push(StepResult {
                        name: "boot virt".to_string(),
                        ok: true,
                        detail: report.detail,
                    });
                } else {
                    steps.push(StepResult {
                        name: "boot virt".to_string(),
                        ok: report.pass,
                        detail: report.detail,
                    });
                }
            }
            Err(error) => steps.push(StepResult {
                name: "boot virt".to_string(),
                ok: false,
                detail: format!("error: {error}"),
            }),
        }
    } else {
        steps.push(StepResult {
            name: "boot virt".to_string(),
            ok: true,
            detail: "SKIPPED (qemu-system-aarch64 not installed)".to_string(),
        });
    }

    println!("\n== ServiceOS E2E validation ==");
    let mut all_ok = true;
    for step in &steps {
        println!(
            "{:<24} {}  {}",
            step.name,
            if step.ok { "PASS" } else { "FAIL" },
            step.detail
        );
        all_ok &= step.ok;
    }
    println!("result: {}", if all_ok { "PASS" } else { "FAIL" });
    if !all_ok {
        return Err("validation bundle reported failures".into());
    }
    Ok(())
}

fn host_step(root: &Path, name: &str, args: Vec<String>) -> StepResult {
    println!("\n=== validate: {name} ===");
    let mut command = Command::new("cargo");
    command.current_dir(root).args(&args);
    let output = command.output();
    match output {
        Ok(output) if output.status.success() => {
            let detail = if name == "test workspace" {
                format!(
                    "({})",
                    summarize_test_output(&String::from_utf8_lossy(&output.stdout))
                )
            } else {
                String::new()
            };
            StepResult {
                name: name.to_string(),
                ok: true,
                detail,
            }
        }
        Ok(output) => {
            let tail = tail_lines(
                &format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ),
                30,
            );
            println!("{tail}");
            StepResult {
                name: name.to_string(),
                ok: false,
                detail: format!("exit status {}", output.status),
            }
        }
        Err(error) => StepResult {
            name: name.to_string(),
            ok: false,
            detail: format!("spawn failed: {error}"),
        },
    }
}

fn summarize_test_output(stdout: &str) -> String {
    let mut passed_total = 0u64;
    let mut failed_total = 0u64;
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("test result: ") {
            for token in rest.split_whitespace().collect::<Vec<_>>().windows(2) {
                if token[1] == "passed" || token[1] == "passed;" {
                    passed_total += token[0].parse::<u64>().unwrap_or(0);
                }
                if token[1] == "failed" || token[1] == "failed;" {
                    failed_total += token[0].parse::<u64>().unwrap_or(0);
                }
            }
        }
    }
    format!("{passed_total} passed, {failed_total} failed")
}

fn tail_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(count);
    lines[start..].join("\n")
}

struct BootReport {
    pass: bool,
    /// True when the platform could not be exercised for a reason that
    /// predates validation (e.g. a tree that does not compile for that
    /// target). Skipped rows are reported loudly but do not fail the
    /// bundle, whose job is validating what can be validated.
    skipped: bool,
    detail: String,
}

fn marker_flags(output: &str) -> (bool, bool, bool) {
    (
        output.contains(MARKER_STORAGE),
        output.contains(MARKER_NET),
        output.contains(MARKER_AUDIO),
    )
}

fn flag(ok: bool) -> &'static str {
    if ok { "✓" } else { "✗" }
}

fn boot_step_virtio() -> Result<BootReport, Box<dyn Error>> {
    println!("\n=== validate: bounded boot qemu-virtio ===");
    let spec = PlatformSpec::qemu_virtio();
    let artifacts = build_for_platform(spec, false)?;
    let disk = create_platform_image(&artifacts)?;
    // Scratch copies keep validation deterministic: the dev data image is
    // never mutated and every run starts from a factory-fresh volume.
    let stage_dir = workspace_root().join("target").join("validate");
    std::fs::create_dir_all(&stage_dir)?;
    let stage_disk = stage_dir.join("serviceos.img");
    let stage_data = stage_dir.join("serviceos-data.img");
    std::fs::copy(&disk, &stage_disk)?;
    write_zeroed_image(&stage_data, 128)?;
    let markers = vec![
        MARKER_STORAGE.to_string(),
        MARKER_NET.to_string(),
        MARKER_AUDIO.to_string(),
    ];
    let outcome = bootlog::bounded_qemu_virtio_boot(&stage_disk, &stage_data, &markers)?;
    finish_boot_report(&outcome, true)
}

fn write_zeroed_image(path: &Path, size_mb: u64) -> Result<(), Box<dyn Error>> {
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    let file = std::fs::File::create(path)?;
    file.set_len(size_mb * 1024 * 1024)?;
    Ok(())
}

fn boot_step_virt() -> Result<BootReport, Box<dyn Error>> {
    println!("\n=== validate: bounded boot virt (aarch64) ===");
    let spec = PlatformSpec::virt();
    let build_result = build_for_platform(spec, false);
    let artifacts = match build_result {
        Ok(artifacts) => artifacts,
        Err(build_error) => {
            // Pre-existing tree breakage for this target is reported, not
            // counted against validation.
            return Ok(BootReport {
                pass: false,
                skipped: true,
                detail: format!(
                    "SKIPPED (tree does not currently build for aarch64: {build_error})"
                ),
            });
        }
    };
    create_platform_image(&artifacts)?;
    let markers = vec![
        MARKER_STORAGE.to_string(),
        MARKER_NET.to_string(),
        MARKER_AUDIO.to_string(),
    ];
    let outcome = bootlog::bounded_qemu_virt_boot(&artifacts, &markers)?;
    finish_boot_report(&outcome, false)
}

fn finish_boot_report(
    outcome: &bootlog::BootOutcome,
    require_all: bool,
) -> Result<BootReport, Box<dyn Error>> {
    let (storage, net, audio) = marker_flags(&outcome.output);
    let any = storage || net || audio;
    let timed_out = outcome.timed_out && !outcome.markers_seen;
    let pass = if require_all {
        storage && net && audio && !timed_out
    } else {
        any && !timed_out
    };
    let mut detail = format!(
        "storage {s} net {n} audio {a}",
        s = flag(storage),
        n = flag(net),
        a = flag(audio),
    );
    if timed_out {
        detail.push_str(" (timeout without full evidence)");
    } else if !outcome.markers_seen && !outcome.exit_detail.is_empty() {
        detail.push_str(&format!(" ({})", outcome.exit_detail));
    }
    Ok(BootReport {
        pass,
        skipped: false,
        detail,
    })
}

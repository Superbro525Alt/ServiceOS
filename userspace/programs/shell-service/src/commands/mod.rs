mod account;
mod audio;
mod console;
mod core;
mod deny;
mod desktop;
mod developer;
mod diagnostics;
mod graphics;
mod identity;
mod network;
mod operator;
mod package;
mod peripheral;
mod runtime;
mod security;
mod sysupdate;

use serviceos_userspace_runtime as rt;

use crate::jobs;
use crate::pipeline;
use crate::util::{
    HELP_TEXT, ShellOutput, parse_service_name, shell_output_write, write_output_linef,
};

pub(crate) fn execute_command(
    bootstrap: rt::Handle,
    output: ShellOutput,
    line: &str,
) -> rt::Result<()> {
    let mut parts = line.split_whitespace();
    let Some(command) = parts.next() else {
        return Ok(());
    };

    match command {
        "help" => shell_output_write(output, HELP_TEXT),
        "services" => core::cmd_services(bootstrap, output),
        "service" => match parts.next().and_then(parse_service_name) {
            Some(service_id) => core::cmd_service(bootstrap, output, service_id),
            None => write_output_linef(output, format_args!("usage: service <name>")),
        },
        "service-caps" => match parts.next().and_then(parse_service_name) {
            Some(service_id) => core::cmd_service_caps(bootstrap, output, service_id),
            None => write_output_linef(output, format_args!("usage: service-caps <name>")),
        },
        "service-revoke-lookup" => match (
            parts.next().and_then(parse_service_name),
            parts.next().and_then(parse_service_name),
            parts.next(),
        ) {
            (Some(service_id), Some(target), Some("revoke")) => {
                core::cmd_service_revoke_lookup(bootstrap, output, service_id, target, true)
            }
            (Some(service_id), Some(target), Some("default")) => {
                core::cmd_service_revoke_lookup(bootstrap, output, service_id, target, false)
            }
            _ => write_output_linef(
                output,
                format_args!("usage: service-revoke-lookup <service> <target> <revoke|default>"),
            ),
        },
        "restart" => match parts.next().and_then(parse_service_name) {
            Some(service_id) => core::cmd_restart(bootstrap, output, service_id),
            None => write_output_linef(output, format_args!("usage: restart <name>")),
        },
        "logs" => match parts.next() {
            Some("stream") => {
                let count = parts
                    .next()
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(12);
                core::cmd_logs_stream(bootstrap, output, count)
            }
            Some("follow") => match parts.next() {
                Some(filter) => diagnostics::cmd_logs_follow(bootstrap, output, filter),
                None => {
                    write_output_linef(output, format_args!("usage: logs follow <domain|service>"))
                }
            },
            Some("crashes") => {
                let count = parts
                    .next()
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(8);
                diagnostics::cmd_logs_crashes(bootstrap, output, count)
            }
            maybe_count => {
                let count = maybe_count
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(12);
                core::cmd_logs(bootstrap, output, count)
            }
        },
        "config" => match parts.next() {
            None => core::cmd_config(bootstrap, output),
            Some("get") => match parts.next() {
                Some(key) => core::cmd_config_get(bootstrap, output, key),
                None => write_output_linef(output, format_args!("usage: config get <key>")),
            },
            Some("set") => match (
                parts.next(),
                parts.next().and_then(|value| value.parse::<u64>().ok()),
            ) {
                (Some(key), Some(value)) => core::cmd_config_set(bootstrap, output, key, value),
                _ => write_output_linef(output, format_args!("usage: config set <key> <value>")),
            },
            _ => write_output_linef(output, format_args!("usage: config [get|set] ...")),
        },
        "store" => match parts.next() {
            Some("ls") => core::cmd_store_ls(bootstrap, output, parts.next().unwrap_or("")),
            Some("mounts") => core::cmd_store_mounts(bootstrap, output),
            Some("mkdir") => match parts.next() {
                Some(path) => core::cmd_store_mkdir(bootstrap, output, path),
                None => write_output_linef(output, format_args!("usage: store mkdir <path>")),
            },
            Some("write") => match parts.next() {
                Some(path) => core::cmd_store_write(bootstrap, output, path, parts),
                None => {
                    write_output_linef(output, format_args!("usage: store write <path> <text>"))
                }
            },
            Some("rm") => match parts.next() {
                Some(path) => core::cmd_store_rm(bootstrap, output, path),
                None => write_output_linef(output, format_args!("usage: store rm <path>")),
            },
            _ => write_output_linef(
                output,
                format_args!("usage: store <ls|mounts|mkdir|write|rm> ..."),
            ),
        },
        "cat" => match parts.next() {
            Some(path) => core::cmd_cat(bootstrap, output, path),
            None => operator::cmd_cat_input(output),
        },
        "status" => match parts.next() {
            None => core::cmd_status_snapshot(bootstrap, output),
            Some("services") => core::cmd_status_services(bootstrap, output),
            Some("health") => diagnostics::cmd_status_health(bootstrap, output),
            Some("svc") => match parts.next().and_then(parse_service_name) {
                Some(service_id) => diagnostics::cmd_status_svc(bootstrap, output, service_id),
                None => write_output_linef(output, format_args!("usage: status svc <name>")),
            },
            Some("watch") => {
                let count = parts
                    .next()
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(8);
                core::cmd_status_watch(bootstrap, output, count)
            }
            _ => write_output_linef(
                output,
                format_args!("usage: status [services|health|svc <name>|watch [count]]"),
            ),
        },
        "ps" => match parts.next() {
            Some("app") => diagnostics::cmd_ps_app(bootstrap, output, parts.next()),
            _ => write_output_linef(output, format_args!("usage: ps app [name]")),
        },
        "net" => network::cmd_net(bootstrap, output, parts),
        "audio" => audio::cmd_audio(bootstrap, output, parts),
        "gfx" => graphics::cmd_gfx(bootstrap, output, parts),
        "desktop" => desktop::cmd_desktop(bootstrap, output, parts),
        "dev" => developer::cmd_dev(bootstrap, output, parts),
        "pkg" => package::cmd_pkg(bootstrap, output, parts),
        "sysupdate" => sysupdate::cmd_sysupdate(bootstrap, output, parts),
        "runtime" => runtime::cmd_runtime(bootstrap, output, parts),
        "security" => security::cmd_security(bootstrap, output, parts),
        "run" => match parts.next() {
            Some("sysinfo") => core::cmd_run_sysinfo(bootstrap, output),
            Some("pkg") => match parts.next().and_then(parse_service_name) {
                Some(service_id) => core::cmd_run_package(bootstrap, output, service_id),
                None => write_output_linef(output, format_args!("usage: run pkg <name>")),
            },
            Some("image") => match parts.next() {
                Some(path) => {
                    if !package::gate_run_image(output, path)? {
                        return Ok(());
                    }
                    core::cmd_run_image(bootstrap, output, path)
                }
                None => write_output_linef(output, format_args!("usage: run image <path>")),
            },
            _ => write_output_linef(output, format_args!("usage: run <sysinfo|pkg|image>")),
        },
        "sessions" => operator::cmd_sessions(output),
        "history" => {
            let count = parts.next().and_then(|value| value.parse::<usize>().ok());
            operator::cmd_history(output, count)
        }
        "jobs" => operator::cmd_jobs(output),
        "fg" => match parts.next().and_then(|value| value.parse::<u32>().ok()) {
            Some(job_id) => operator::cmd_fg(output, job_id),
            None => write_output_linef(output, format_args!("usage: fg <job-id>")),
        },
        "filter" => match parts.next() {
            Some(pattern) => operator::cmd_filter(output, pattern),
            None => write_output_linef(
                output,
                format_args!("usage: filter <text> (pipeline stage)"),
            ),
        },
        "count" => operator::cmd_count(output),
        "login" => match (parts.next(), parts.next()) {
            (name, secret) => operator::cmd_login(bootstrap, output, name, secret),
        },
        "whoami" => operator::cmd_whoami(output),
        "logout" => operator::cmd_logout(bootstrap, output),
        "su" => match (parts.next(), parts.next()) {
            (name, secret) => identity::cmd_su(bootstrap, output, name, secret),
        },
        "peripheral" => peripheral::cmd_peripheral(bootstrap, output, parts),
        "console" => console::cmd_console(bootstrap, output, parts.next()),
        _ => write_output_linef(output, format_args!("unknown command: {command}")),
    }
}

/// Full line entry point: background detection first, then pipelines, then
/// the plain single-command dispatcher.
pub(crate) fn execute_line(
    bootstrap: rt::Handle,
    output: ShellOutput,
    line: &str,
) -> rt::Result<()> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    if let Some(background) = strip_background(trimmed) {
        return match jobs::spawn_job(background) {
            Ok(job_id) => write_output_linef(
                output,
                format_args!("[{job_id}] background: {background}"),
            ),
            Err(_) => write_output_linef(
                output,
                format_args!("job table full; use jobs/fg to reclaim slots"),
            ),
        };
    }
    run_sync(bootstrap, output, trimmed)
}

/// A single trailing `&` backgrounds the rest of the line (`&&` chains are
/// intentionally unsupported and stay foreground).
fn strip_background(line: &str) -> Option<&str> {
    let body = line.strip_suffix('&')?.trim_end();
    if body.is_empty() {
        None
    } else {
        Some(body)
    }
}

/// Synchronous execution with shell-mediated pipeline support.
pub(crate) fn run_sync(
    bootstrap: rt::Handle,
    output: ShellOutput,
    line: &str,
) -> rt::Result<()> {
    pipeline::clear_input();
    let plan = match pipeline::split_pipeline(line) {
        Ok(plan) => plan,
        Err(pipeline::SplitError::EmptyStage) => {
            return write_output_linef(
                output,
                format_args!("pipeline: empty stage between '|'"),
            );
        }
        Err(pipeline::SplitError::TooManyStages) => {
            return write_output_linef(
                output,
                format_args!(
                    "pipeline: too many stages (max {})",
                    pipeline::MAX_PIPELINE_STAGES
                ),
            );
        }
        Err(pipeline::SplitError::EmptyLine) => return Ok(()),
    };
    if plan.count == 1 {
        return execute_command(bootstrap, output, plan.stage(0).unwrap_or(""));
    }
    for index in 0..plan.count - 1 {
        let Some(stage) = plan.stage(index) else {
            break;
        };
        pipeline::capture_begin_scratch();
        let result = execute_command(bootstrap, pipeline::capturing_output(), stage);
        let captured = pipeline::capture_finish_scratch();
        result?;
        match pipeline::feed_captured_via_kernel_pipe(&captured) {
            Ok(_) => {}
            Err(pipe_error) => {
                // Loud fallback: without kernel pipes this boundary would be
                // a plain in-memory handoff again.
                write_output_linef(
                    output,
                    format_args!(
                        "pipeline: kernel pipe unavailable ({pipe_error:?}); mediated fallback"
                    ),
                )?;
                pipeline::feed_captured(&captured);
            }
        }
        if captured.truncated {
            write_output_linef(
                output,
                format_args!(
                    "pipeline: stage {} output truncated at {} bytes",
                    index + 1,
                    pipeline::MAX_CAPTURE_BYTES
                ),
            )?;
        }
    }
    match plan.stage(plan.count - 1) {
        Some(last) => execute_command(bootstrap, output, last),
        None => Ok(()),
    }
}

/// Event-loop poller: executes at most one queued background job per call,
/// capturing its output into the job row instead of any terminal.
pub fn poll_jobs(bootstrap: rt::Handle) {
    let Some(job_id) = jobs::next_running_job_id() else {
        return;
    };
    let mut cmd = [0u8; jobs::JOB_CMD_BYTES];
    let Some(cmd_len) = jobs::job_cmd_copy(job_id, &mut cmd) else {
        jobs::job_mark_done_err(job_id, "InvalidArgument");
        return;
    };
    let Ok(line) = ::core::str::from_utf8(&cmd[..cmd_len]) else {
        jobs::job_mark_done_err(job_id, "InvalidArgument");
        return;
    };
    pipeline::capture_begin_job(job_id);
    let result = run_sync(bootstrap, pipeline::capturing_output(), line);
    pipeline::capture_end();
    match result {
        Ok(()) => jobs::job_mark_done_ok(job_id),
        Err(error) => jobs::job_mark_done_err(job_id, crate::util::error_name(error)),
    };
}

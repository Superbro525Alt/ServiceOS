use serviceos_userspace_runtime as rt;
use rt::{RuntimeKind, RuntimeRunState, RuntimeWorkloadKind, ServiceId};

use crate::util::{ShellOutput, shell_output_write, write_output_linef};

const MAX_RUNTIME_ENVS: usize = 4;
const MAX_RUNTIME_RUNS: usize = 4;
const MAX_RUNTIME_PATH: usize = 64;
const MAX_RUNTIME_SOURCE: usize = 96;
const MAX_RUNTIME_KEY: usize = 32;
const MAX_RUNTIME_VALUE: usize = 80;
const MAX_RUNTIME_OUTPUT_CHUNK: usize = (rt::IPC_MAX_WORDS - 1) * 8;

pub(crate) fn cmd_runtime<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    mut parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("envs") => cmd_runtime_envs(bootstrap, output),
        Some("create") => match parts.next() {
            Some("posix") => cmd_runtime_create(bootstrap, output, RuntimeKind::Posix),
            Some("windows") => write_output_linef(
                output,
                format_args!("windows runtime kind is reserved for a later phase"),
            ),
            _ => write_output_linef(output, format_args!("usage: runtime create posix")),
        },
        Some("inspect") => match parts.next().and_then(parse_u32) {
            Some(env_id) => cmd_runtime_inspect(bootstrap, output, env_id),
            None => write_output_linef(output, format_args!("usage: runtime inspect <env-id>")),
        },
        Some("mounts") => match parts.next().and_then(parse_u32) {
            Some(env_id) => cmd_runtime_mounts(bootstrap, output, env_id),
            None => write_output_linef(output, format_args!("usage: runtime mounts <env-id>")),
        },
        Some("vars") => match parts.next().and_then(parse_u32) {
            Some(env_id) => cmd_runtime_vars(bootstrap, output, env_id),
            None => write_output_linef(output, format_args!("usage: runtime vars <env-id>")),
        },
        Some("runs") => cmd_runtime_runs(bootstrap, output),
        Some("launch") => match (
            parts.next().and_then(parse_u32),
            parts.next().and_then(parse_workload),
        ) {
            (Some(env_id), Some((workload, needs_path))) => {
                let argument = parts.next().unwrap_or("");
                if needs_path && argument.is_empty() {
                    write_output_linef(
                        output,
                        format_args!("usage: runtime launch <env-id> cat <guest-path>"),
                    )
                } else {
                    cmd_runtime_launch(bootstrap, output, env_id, workload, argument)
                }
            }
            _ => write_output_linef(
                output,
                format_args!(
                    "usage: runtime launch <env-id> <inspect|env|mounts|cat> [guest-path]"
                ),
            ),
        },
        Some("destroy") => match parts.next().and_then(parse_u32) {
            Some(env_id) => cmd_runtime_destroy(bootstrap, output, env_id),
            None => write_output_linef(output, format_args!("usage: runtime destroy <env-id>")),
        },
        _ => write_output_linef(
            output,
            format_args!(
                "usage: runtime <envs|create|inspect|mounts|vars|runs|launch|destroy> ..."
            ),
        ),
    }
}

fn cmd_runtime_envs(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let runtime_handle = match lookup_runtime_service(bootstrap, output)? {
        Some(handle) => handle,
        None => return Ok(()),
    };
    let mut envs = [rt::RuntimeEnvInfo {
        env_id: 0,
        kind: RuntimeKind::Posix,
        state: rt::RuntimeEnvState::Destroyed,
        capabilities: 0,
        mount_count: 0,
        var_count: 0,
        active_runs: 0,
    }; MAX_RUNTIME_ENVS];
    let count = rt::runtime_env_list(runtime_handle, &mut envs)?;
    let _ = rt::handle_close(runtime_handle);
    if count == 0 {
        return write_output_linef(output, format_args!("no runtime environments"));
    }
    for env in envs.iter().take(count).copied() {
        write_output_linef(
            output,
            format_args!(
                "env{} kind={} state={} caps={} mounts={} vars={} runs={}",
                env.env_id,
                runtime_kind_name(env.kind),
                runtime_env_state_name(env.state),
                capability_summary(env.capabilities),
                env.mount_count,
                env.var_count,
                env.active_runs,
            ),
        )?;
    }
    Ok(())
}

fn cmd_runtime_create(
    bootstrap: rt::Handle,
    output: ShellOutput,
    kind: RuntimeKind,
) -> rt::Result<()> {
    let runtime_handle = match lookup_runtime_service(bootstrap, output)? {
        Some(handle) => handle,
        None => return Ok(()),
    };
    let env_id = rt::runtime_env_create(runtime_handle, kind)?;
    let _ = rt::handle_close(runtime_handle);
    write_output_linef(
        output,
        format_args!("created {} env{}", runtime_kind_name(kind), env_id),
    )
}

fn cmd_runtime_inspect(
    bootstrap: rt::Handle,
    output: ShellOutput,
    env_id: u32,
) -> rt::Result<()> {
    let runtime_handle = match lookup_runtime_service(bootstrap, output)? {
        Some(handle) => handle,
        None => return Ok(()),
    };
    let env = rt::runtime_env_status(runtime_handle, env_id)?;
    let _ = rt::handle_close(runtime_handle);
    write_output_linef(output, format_args!("env{}", env.env_id))?;
    write_output_linef(output, format_args!("  kind={}", runtime_kind_name(env.kind)))?;
    write_output_linef(output, format_args!("  state={}", runtime_env_state_name(env.state)))?;
    write_output_linef(
        output,
        format_args!("  caps={}", capability_summary(env.capabilities)),
    )?;
    write_output_linef(output, format_args!("  mounts={}", env.mount_count))?;
    write_output_linef(output, format_args!("  vars={}", env.var_count))?;
    write_output_linef(output, format_args!("  runs={}", env.active_runs))
}

fn cmd_runtime_mounts(
    bootstrap: rt::Handle,
    output: ShellOutput,
    env_id: u32,
) -> rt::Result<()> {
    let runtime_handle = match lookup_runtime_service(bootstrap, output)? {
        Some(handle) => handle,
        None => return Ok(()),
    };
    let mut guest = [0u8; MAX_RUNTIME_PATH];
    let mut source = [0u8; MAX_RUNTIME_SOURCE];
    let mut index = 0usize;
    while let Some((guest_len, source_len)) =
        rt::runtime_env_mount(runtime_handle, env_id, index, &mut guest, &mut source)?
    {
        let guest =
            core::str::from_utf8(&guest[..guest_len]).map_err(|_| rt::Error::InvalidArgument)?;
        let source =
            core::str::from_utf8(&source[..source_len]).map_err(|_| rt::Error::InvalidArgument)?;
        write_output_linef(output, format_args!("{} -> {}", guest, source))?;
        index += 1;
    }
    let _ = rt::handle_close(runtime_handle);
    if index == 0 {
        write_output_linef(output, format_args!("no mounts"))
    } else {
        Ok(())
    }
}

fn cmd_runtime_vars(
    bootstrap: rt::Handle,
    output: ShellOutput,
    env_id: u32,
) -> rt::Result<()> {
    let runtime_handle = match lookup_runtime_service(bootstrap, output)? {
        Some(handle) => handle,
        None => return Ok(()),
    };
    let mut key = [0u8; MAX_RUNTIME_KEY];
    let mut value = [0u8; MAX_RUNTIME_VALUE];
    let mut index = 0usize;
    while let Some((key_len, value_len)) =
        rt::runtime_env_var(runtime_handle, env_id, index, &mut key, &mut value)?
    {
        let key =
            core::str::from_utf8(&key[..key_len]).map_err(|_| rt::Error::InvalidArgument)?;
        let value =
            core::str::from_utf8(&value[..value_len]).map_err(|_| rt::Error::InvalidArgument)?;
        write_output_linef(output, format_args!("{}={}", key, value))?;
        index += 1;
    }
    let _ = rt::handle_close(runtime_handle);
    if index == 0 {
        write_output_linef(output, format_args!("no vars"))
    } else {
        Ok(())
    }
}

fn cmd_runtime_runs(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let runtime_handle = match lookup_runtime_service(bootstrap, output)? {
        Some(handle) => handle,
        None => return Ok(()),
    };
    let mut runs = [rt::RuntimeRunInfo {
        run_id: 0,
        env_id: 0,
        workload: RuntimeWorkloadKind::Inspect,
        state: RuntimeRunState::Exited,
        exit_code: 0,
    }; MAX_RUNTIME_RUNS];
    let count = rt::runtime_run_list(runtime_handle, &mut runs)?;
    let _ = rt::handle_close(runtime_handle);
    if count == 0 {
        return write_output_linef(output, format_args!("no runtime workloads"));
    }
    for run in runs.iter().take(count).copied() {
        write_output_linef(
            output,
            format_args!(
                "run{} env={} workload={} state={} exit={:#x}",
                run.run_id,
                run.env_id,
                runtime_workload_name(run.workload),
                runtime_run_state_name(run.state),
                run.exit_code,
            ),
        )?;
    }
    Ok(())
}

fn cmd_runtime_launch(
    bootstrap: rt::Handle,
    output: ShellOutput,
    env_id: u32,
    workload: RuntimeWorkloadKind,
    argument: &str,
) -> rt::Result<()> {
    let runtime_handle = match lookup_runtime_service(bootstrap, output)? {
        Some(handle) => handle,
        None => return Ok(()),
    };
    let relay = rt::channel_create()?;
    let arg_bytes = argument.as_bytes();
    let max_inline_bytes = (rt::IPC_MAX_WORDS.saturating_sub(3)) * 8;
    if arg_bytes.len() > max_inline_bytes {
        let _ = rt::handle_close(relay.first);
        let _ = rt::handle_close(relay.second);
        let _ = rt::handle_close(runtime_handle);
        return write_output_linef(output, format_args!("runtime launch argument too large"));
    }
    let transferred_output = match rt::handle_duplicate(
        relay.second,
        rt::rights::SEND | rt::rights::DUPLICATE | rt::rights::TRANSFER,
    ) {
        Ok(handle) => handle,
        Err(error) => {
            let _ = rt::handle_close(relay.first);
            let _ = rt::handle_close(relay.second);
            let _ = rt::handle_close(runtime_handle);
            return write_output_linef(
                output,
                format_args!(
                    "runtime launch relay duplicate failed: {}",
                    crate::util::error_name(error),
                ),
            );
        }
    };
    let reply = rt::channel_create()?;
    let mut request = rt::RawMessage::empty(rt::RuntimeTag::RunLaunchRequest as u32);
    request.word_count = 3 + rt::pack_bytes(arg_bytes, &mut request.words[3..])?;
    request.words[0] = env_id as u64;
    request.words[1] = workload as u32 as u64;
    request.words[2] = arg_bytes.len() as u64;
    request.handle_count = 2;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rt::rights::SEND;
    request.handles[1] = transferred_output;
    request.handle_rights[1] = rt::rights::SEND | rt::rights::DUPLICATE | rt::rights::TRANSFER;
    if let Err(error) = rt::channel_send(runtime_handle, &request) {
        let _ = rt::handle_close(transferred_output);
        let _ = rt::handle_close(reply.second);
        let _ = rt::handle_close(reply.first);
        let _ = rt::handle_close(relay.first);
        let _ = rt::handle_close(relay.second);
        let _ = rt::handle_close(runtime_handle);
        return write_output_linef(
            output,
            format_args!(
                "runtime launch request send failed: {}",
                crate::util::error_name(error),
            ),
        );
    }
    let _ = rt::handle_close(transferred_output);
    let _ = rt::handle_close(reply.second);

    let mut response = rt::RawMessage::empty(0);
    if let Err(error) = rt::channel_receive_blocking(reply.first, &mut response) {
        let _ = rt::handle_close(reply.first);
        let _ = rt::handle_close(relay.first);
        let _ = rt::handle_close(relay.second);
        let _ = rt::handle_close(runtime_handle);
        return write_output_linef(
            output,
            format_args!(
                "runtime launch reply receive failed: {}",
                crate::util::error_name(error),
            ),
        );
    }
    let _ = rt::handle_close(reply.first);
    if response.tag != rt::RuntimeTag::RunLaunchReply as u32 || response.word_count < 2 {
        let _ = rt::handle_close(relay.first);
        let _ = rt::handle_close(relay.second);
        let _ = rt::handle_close(runtime_handle);
        return write_output_linef(output, format_args!("runtime launch reply was malformed"));
    }
    let status = match response.words[0] as u32 {
        x if x == rt::RuntimeStatus::Ok as u32 => rt::RuntimeStatus::Ok,
        x if x == rt::RuntimeStatus::NotFound as u32 => rt::RuntimeStatus::NotFound,
        x if x == rt::RuntimeStatus::Busy as u32 => rt::RuntimeStatus::Busy,
        x if x == rt::RuntimeStatus::Denied as u32 => rt::RuntimeStatus::Denied,
        x if x == rt::RuntimeStatus::InvalidPath as u32 => rt::RuntimeStatus::InvalidPath,
        x if x == rt::RuntimeStatus::Unsupported as u32 => rt::RuntimeStatus::Unsupported,
        x if x == rt::RuntimeStatus::Closed as u32 => rt::RuntimeStatus::Closed,
        _ => rt::RuntimeStatus::Busy,
    };
    if status != rt::RuntimeStatus::Ok {
        let _ = rt::handle_close(relay.first);
        let _ = rt::handle_close(relay.second);
        let _ = rt::handle_close(runtime_handle);
        return write_output_linef(
            output,
            format_args!("runtime launch failed: {}", runtime_status_name(status)),
        );
    }
    let run_id = response.words[1] as u32;
    let _ = rt::handle_close(relay.second);
    write_output_linef(
        output,
        format_args!(
            "launched run{} env={} workload={}",
            run_id,
            env_id,
            runtime_workload_name(workload),
        ),
    )?;

    let mut buffer = [0u8; MAX_RUNTIME_OUTPUT_CHUNK];
    loop {
        let mut drained_output = false;
        loop {
            match rt::runtime_output_relay_try_read(relay.first, &mut buffer) {
                Ok(read) => {
                    drained_output = true;
                    let text = core::str::from_utf8(&buffer[..read])
                        .map_err(|_| rt::Error::InvalidArgument)?;
                    shell_output_write(output, text)?;
                }
                Err(rt::Error::QueueEmpty) => break,
                Err(error) => {
                    let _ = rt::handle_close(relay.first);
                    let _ = rt::handle_close(runtime_handle);
                    return Err(error);
                }
            }
        }

        let run = rt::runtime_run_status(runtime_handle, run_id)?;
        if matches!(run.state, RuntimeRunState::Exited | RuntimeRunState::Failed) {
            let _ = rt::handle_close(relay.first);
            let _ = rt::handle_close(runtime_handle);
            return write_output_linef(
                output,
                format_args!(
                    "run{} state={} exit={:#x}",
                    run.run_id,
                    runtime_run_state_name(run.state),
                    run.exit_code,
                ),
            );
        }

        if !drained_output {
            rt::yield_current()?;
        }
    }
}

fn cmd_runtime_destroy(
    bootstrap: rt::Handle,
    output: ShellOutput,
    env_id: u32,
) -> rt::Result<()> {
    let runtime_handle = match lookup_runtime_service(bootstrap, output)? {
        Some(handle) => handle,
        None => return Ok(()),
    };
    rt::runtime_env_destroy(runtime_handle, env_id)?;
    let _ = rt::handle_close(runtime_handle);
    write_output_linef(output, format_args!("destroyed env{}", env_id))
}

fn lookup_runtime_service(
    bootstrap: rt::Handle,
    output: ShellOutput,
) -> rt::Result<Option<rt::Handle>> {
    match rt::lookup_service(bootstrap, ServiceId::Runtime) {
        Ok(handle) => Ok(Some(handle)),
        Err(rt::Error::NotFound) => {
            write_output_linef(
                output,
                format_args!("runtime-service unavailable; install package runtime"),
            )?;
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn parse_u32(value: &str) -> Option<u32> {
    value.parse::<u32>().ok()
}

fn parse_workload(value: &str) -> Option<(RuntimeWorkloadKind, bool)> {
    match value {
        "inspect" => Some((RuntimeWorkloadKind::Inspect, false)),
        "env" => Some((RuntimeWorkloadKind::Env, false)),
        "mounts" => Some((RuntimeWorkloadKind::Mounts, false)),
        "cat" => Some((RuntimeWorkloadKind::Cat, true)),
        _ => None,
    }
}

fn runtime_kind_name(kind: RuntimeKind) -> &'static str {
    match kind {
        RuntimeKind::Posix => "posix",
        RuntimeKind::Windows => "windows",
    }
}

fn runtime_env_state_name(state: rt::RuntimeEnvState) -> &'static str {
    match state {
        rt::RuntimeEnvState::Ready => "ready",
        rt::RuntimeEnvState::Busy => "busy",
        rt::RuntimeEnvState::Destroyed => "destroyed",
        rt::RuntimeEnvState::PendingApproval => "pending-approval",
        rt::RuntimeEnvState::Denied => "denied",
    }
}

fn runtime_run_state_name(state: RuntimeRunState) -> &'static str {
    match state {
        RuntimeRunState::Launching => "launching",
        RuntimeRunState::Running => "running",
        RuntimeRunState::Exited => "exited",
        RuntimeRunState::Failed => "failed",
    }
}

fn runtime_workload_name(kind: RuntimeWorkloadKind) -> &'static str {
    match kind {
        RuntimeWorkloadKind::Inspect => "inspect",
        RuntimeWorkloadKind::Env => "env",
        RuntimeWorkloadKind::Mounts => "mounts",
        RuntimeWorkloadKind::Cat => "cat",
    }
}

fn capability_summary(capabilities: u32) -> CapabilitySummary {
    CapabilitySummary(capabilities)
}

fn runtime_status_name(status: rt::RuntimeStatus) -> &'static str {
    match status {
        rt::RuntimeStatus::Ok => "ok",
        rt::RuntimeStatus::NotFound => "not-found",
        rt::RuntimeStatus::Busy => "busy",
        rt::RuntimeStatus::Denied => "denied",
        rt::RuntimeStatus::InvalidPath => "invalid-path",
        rt::RuntimeStatus::Unsupported => "unsupported",
        rt::RuntimeStatus::Closed => "closed",
        rt::RuntimeStatus::PendingApproval => "pending-approval",
    }
}

struct CapabilitySummary(u32);

impl core::fmt::Display for CapabilitySummary {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut wrote = false;
        for (name, mask) in [
            ("file-read", rt::runtime_capability::FILE_READ),
            ("terminal-io", rt::runtime_capability::TERMINAL_IO),
            ("network", rt::runtime_capability::NETWORK),
            ("graphics", rt::runtime_capability::GRAPHICS),
            ("audio", rt::runtime_capability::AUDIO),
        ] {
            if self.0 & mask == 0 {
                continue;
            }
            if wrote {
                write!(f, ",")?;
            }
            write!(f, "{name}")?;
            wrote = true;
        }
        if !wrote {
            write!(f, "none")?;
        }
        Ok(())
    }
}

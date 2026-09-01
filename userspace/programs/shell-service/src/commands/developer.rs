use core::fmt::Write;

use rt::{
    DeveloperArtifactFormat, DeveloperJobState, DeveloperStatus, DeveloperTag, DeveloperTarget,
    DeveloperToolchainState, RawMessage, ServiceId,
};
use serviceos_userspace_runtime as rt;

use crate::util::{ShellOutput, shell_output_write, write_output_linef};

const MAX_TOOLCHAINS: usize = 8;
const MAX_WORKSPACES: usize = 8;
const MAX_JOBS: usize = 8;
const MAX_NAME: usize = 64;
const MAX_PATH: usize = 96;
const MAX_OUTPUT_CHUNK: usize = (rt::IPC_MAX_WORDS - 1) * 8;
const MAX_STORAGE_PATH: usize = 96;
// Mirror of developer-service's local profile tags (0xd24 request, 0xd25
// reply) and its IDE-tail grammar; the shared DeveloperTag range stays
// 0xd00-0xd0f.
const DEV_PROFILE_REQUEST_TAG: u32 = 0xd24;
const DEV_PROFILE_REPLY_TAG: u32 = 0xd25;
const IDE_TAIL_MAGIC: u64 = 0x4944_4531;
/// Profile reply fields after the magic: five phase stamps plus the
/// rate/valid-mask word.
const PROFILE_FIELDS: usize = 6;
/// Phase slots, matching developer-service's timing module order.
const PHASE_QUEUE: usize = 0;
const PHASE_START: usize = 1;
const PHASE_TOOL_EXIT: usize = 2;
const PHASE_ARTIFACT: usize = 3;
const PHASE_FINISH: usize = 4;
const PHASE_COUNT: usize = 5;

pub(crate) fn cmd_dev<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    mut parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("toolchains") => cmd_toolchains(bootstrap, output),
        Some("toolchain") => match parts.next().and_then(parse_u32) {
            Some(toolchain_id) => cmd_toolchain(bootstrap, output, toolchain_id),
            None => write_output_linef(output, format_args!("usage: dev toolchain <id>")),
        },
        Some("workspaces") => cmd_workspaces(bootstrap, output),
        Some("workspace") => match parts.next().and_then(parse_u32) {
            Some(workspace_id) => cmd_workspace(bootstrap, output, workspace_id),
            None => write_output_linef(output, format_args!("usage: dev workspace <id>")),
        },
        Some("build") => match (
            parts.next().and_then(parse_u32),
            parts.next().and_then(parse_target),
        ) {
            (Some(workspace_id), Some(target)) => {
                cmd_build(bootstrap, output, workspace_id, target)
            }
            _ => write_output_linef(
                output,
                format_args!("usage: dev build <workspace-id> <native|linux|windows|macos>"),
            ),
        },
        Some("jobs") => cmd_jobs(bootstrap, output),
        Some("profile") => match parts.next().and_then(parse_u32) {
            Some(job_id) => cmd_profile(bootstrap, output, job_id),
            None => write_output_linef(output, format_args!("usage: dev profile <job-id>")),
        },
        Some("artifact") => match parts.next().and_then(parse_u32) {
            Some(job_id) => cmd_artifact(bootstrap, output, job_id),
            None => write_output_linef(output, format_args!("usage: dev artifact <job-id>")),
        },
        Some("save") => match (parts.next().and_then(parse_u32), parts.next()) {
            (Some(job_id), Some(path)) => cmd_save_artifact(bootstrap, output, job_id, path),
            _ => write_output_linef(output, format_args!("usage: dev save <job-id> <path>")),
        },
        _ => write_output_linef(
            output,
            format_args!(
                "usage: dev <toolchains|toolchain|workspaces|workspace|build|jobs|profile|artifact|save> ..."
            ),
        ),
    }
}

fn cmd_toolchains(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let developer = match lookup_developer_service(bootstrap, output)? {
        Some(handle) => handle,
        None => return Ok(()),
    };
    let mut toolchains = [empty_toolchain(); MAX_TOOLCHAINS];
    let count = rt::developer_toolchain_list(developer, &mut toolchains)?;
    let _ = rt::handle_close(developer);
    if count == 0 {
        return write_output_linef(output, format_args!("no developer toolchains"));
    }
    for toolchain in toolchains.iter().take(count).copied() {
        let name = core::str::from_utf8(&toolchain.name[..toolchain.name_len as usize])
            .map_err(|_| rt::Error::InvalidArgument)?;
        write_output_linef(
            output,
            format_args!(
                "toolchain{} {} target={} state={} format={}",
                toolchain.toolchain_id,
                name,
                target_name(toolchain.target),
                toolchain_state_name(toolchain.state),
                format_name(toolchain.format),
            ),
        )?;
    }
    Ok(())
}

fn cmd_toolchain(bootstrap: rt::Handle, output: ShellOutput, toolchain_id: u32) -> rt::Result<()> {
    let developer = match lookup_developer_service(bootstrap, output)? {
        Some(handle) => handle,
        None => return Ok(()),
    };
    let mut name = [0u8; MAX_NAME];
    let mut sdk_root = [0u8; MAX_PATH];
    let (toolchain, name_len, sdk_len) =
        rt::developer_toolchain_status(developer, toolchain_id, &mut name, &mut sdk_root)?;
    let _ = rt::handle_close(developer);
    let name = core::str::from_utf8(&name[..name_len]).map_err(|_| rt::Error::InvalidArgument)?;
    let sdk_root =
        core::str::from_utf8(&sdk_root[..sdk_len]).map_err(|_| rt::Error::InvalidArgument)?;
    write_output_linef(
        output,
        format_args!("toolchain{} {}", toolchain.toolchain_id, name),
    )?;
    write_output_linef(
        output,
        format_args!("  target={}", target_name(toolchain.target)),
    )?;
    write_output_linef(
        output,
        format_args!("  state={}", toolchain_state_name(toolchain.state)),
    )?;
    write_output_linef(
        output,
        format_args!("  format={}", format_name(toolchain.format)),
    )?;
    write_output_linef(output, format_args!("  sdk-root={}", sdk_root))
}

fn cmd_workspaces(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let developer = match lookup_developer_service(bootstrap, output)? {
        Some(handle) => handle,
        None => return Ok(()),
    };
    let mut workspaces = [empty_workspace(); MAX_WORKSPACES];
    let count = rt::developer_workspace_list(developer, &mut workspaces)?;
    let _ = rt::handle_close(developer);
    if count == 0 {
        return write_output_linef(output, format_args!("no developer workspaces"));
    }
    for workspace in workspaces.iter().take(count).copied() {
        let name = core::str::from_utf8(&workspace.name[..workspace.name_len as usize])
            .map_err(|_| rt::Error::InvalidArgument)?;
        write_output_linef(
            output,
            format_args!(
                "workspace{} {} targets={}",
                workspace.workspace_id,
                name,
                target_mask_name(workspace.target_mask),
            ),
        )?;
    }
    Ok(())
}

fn cmd_workspace(bootstrap: rt::Handle, output: ShellOutput, workspace_id: u32) -> rt::Result<()> {
    let developer = match lookup_developer_service(bootstrap, output)? {
        Some(handle) => handle,
        None => return Ok(()),
    };
    let mut name = [0u8; MAX_NAME];
    let mut source = [0u8; MAX_PATH];
    let workspace =
        rt::developer_workspace_status(developer, workspace_id, &mut name, &mut source)?;
    let _ = rt::handle_close(developer);
    let name = core::str::from_utf8(&name[..workspace.name_len as usize])
        .map_err(|_| rt::Error::InvalidArgument)?;
    let source = core::str::from_utf8(&source[..workspace.source_path_len as usize])
        .map_err(|_| rt::Error::InvalidArgument)?;
    write_output_linef(
        output,
        format_args!("workspace{} {}", workspace.workspace_id, name),
    )?;
    write_output_linef(output, format_args!("  source={}", source))?;
    write_output_linef(
        output,
        format_args!("  targets={}", target_mask_name(workspace.target_mask)),
    )?;
    write_output_linef(
        output,
        format_args!(
            "  toolchains native={} linux={} windows={} macos={}",
            printable_toolchain_id(workspace.toolchains[0]),
            printable_toolchain_id(workspace.toolchains[1]),
            printable_toolchain_id(workspace.toolchains[2]),
            printable_toolchain_id(workspace.toolchains[3]),
        ),
    )
}

fn cmd_build(
    bootstrap: rt::Handle,
    output: ShellOutput,
    workspace_id: u32,
    target: DeveloperTarget,
) -> rt::Result<()> {
    let developer = match lookup_developer_service(bootstrap, output)? {
        Some(handle) => handle,
        None => return Ok(()),
    };
    let relay = rt::channel_create()?;
    match rt::developer_build_submit(developer, workspace_id, target, relay.second) {
        Ok(job_id) => {
            let _ = rt::handle_close(relay.second);
            write_output_linef(
                output,
                format_args!(
                    "started build job{} workspace={} target={}",
                    job_id,
                    workspace_id,
                    target_name(target)
                ),
            )?;
            let mut buffer = [0u8; MAX_OUTPUT_CHUNK];
            let mut name = [0u8; MAX_NAME];
            loop {
                let mut drained = false;
                loop {
                    match rt::text_relay_try_read(relay.first, &mut buffer) {
                        Ok(read) => {
                            drained = true;
                            let text = core::str::from_utf8(&buffer[..read])
                                .map_err(|_| rt::Error::InvalidArgument)?;
                            shell_output_write(output, text)?;
                        }
                        Err(rt::Error::QueueEmpty) => break,
                        Err(error) => {
                            let _ = rt::handle_close(relay.first);
                            let _ = rt::handle_close(developer);
                            return Err(error);
                        }
                    }
                }

                let job = rt::developer_job_status(developer, job_id, &mut name)?;
                if matches!(
                    job.state,
                    DeveloperJobState::Succeeded
                        | DeveloperJobState::Failed
                        | DeveloperJobState::Unsupported
                ) {
                    let _ = rt::handle_close(relay.first);
                    let _ = rt::handle_close(developer);
                    let artifact_name =
                        core::str::from_utf8(&name[..job.artifact_name_len as usize])
                            .map_err(|_| rt::Error::InvalidArgument)?;
                    return write_output_linef(
                        output,
                        format_args!(
                            "job{} state={} format={} size={} artifact={}",
                            job.job_id,
                            job_state_name(job.state),
                            format_name(job.format),
                            job.artifact_size,
                            artifact_name,
                        ),
                    );
                }

                if !drained {
                    rt::yield_current()?;
                }
            }
        }
        Err(rt::Error::Unsupported) => {
            let _ = rt::handle_close(relay.first);
            let _ = rt::handle_close(relay.second);
            let _ = rt::handle_close(developer);
            write_output_linef(
                output,
                format_args!(
                    "target {} is not locally supported yet",
                    target_name(target)
                ),
            )
        }
        Err(error) => {
            let _ = rt::handle_close(relay.first);
            let _ = rt::handle_close(relay.second);
            let _ = rt::handle_close(developer);
            Err(error)
        }
    }
}

fn cmd_jobs(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let developer = match lookup_developer_service(bootstrap, output)? {
        Some(handle) => handle,
        None => return Ok(()),
    };
    // Raw loop instead of rt::developer_job_list: the reply tail is
    // self-describing (IDE1 magic, field count 5) and carries the per-run
    // duration, which the fixed-shape wrapper drops.
    let mut listed = 0usize;
    let mut duration = 0u64;
    let mut tick_hz = 0u32;
    let mut valid_mask = 0u32;
    while listed < MAX_JOBS {
        let reply = rt::channel_create()?;
        let mut request = RawMessage::empty(DeveloperTag::JobListRequest as u32);
        request.word_count = 1;
        request.words[0] = listed as u64;
        request.handle_count = 1;
        request.handles[0] = reply.second;
        request.handle_rights[0] = rt::rights::SEND;
        rt::channel_send(developer, &request)?;
        let _ = rt::handle_close(reply.second);
        let mut response = RawMessage::empty(0);
        rt::channel_receive_blocking(reply.first, &mut response)?;
        let _ = rt::handle_close(reply.first);
        if response.tag != DeveloperTag::JobListReply as u32 || response.word_count < 7 {
            let _ = rt::handle_close(developer);
            return Err(rt::Error::InvalidArgument);
        }
        if response.words[0] != DeveloperStatus::Ok as u32 as u64 {
            break;
        }
        decode_job_list_timing(
            &response.words[7..response.word_count as usize],
            &mut duration,
            &mut tick_hz,
            &mut valid_mask,
        );
        write_output_linef(
            output,
            format_args!(
                "job{} workspace={} target={} state={} format={} size={} run={}",
                response.words[1] as u32,
                response.words[2] as u32,
                target_name(developer_target_from_word(response.words[3])),
                job_state_name(developer_job_state_from_word(response.words[4])),
                format_name(developer_format_from_word(response.words[5])),
                response.words[6],
                format_duration(duration, tick_hz, valid_mask).as_str(),
            ),
        )?;
        listed += 1;
    }
    let _ = rt::handle_close(developer);
    if listed == 0 {
        return write_output_linef(output, format_args!("no developer jobs"));
    }
    Ok(())
}

/// Decode the additive job-list tail fields: [magic|count][phase][chain]
/// [flags][duration][rate|valid]. Only the trailing duration/rate words
/// are new; readers of the original three fields stay valid.
fn decode_job_list_timing(
    tail: &[u64],
    duration: &mut u64,
    tick_hz: &mut u32,
    valid_mask: &mut u32,
) {
    let Some(magic) = tail.first().copied() else {
        return;
    };
    if magic & 0xFFFF_FFFF != IDE_TAIL_MAGIC || (magic >> 32) & 0xFF != 5 {
        return;
    }
    *duration = tail.get(4).copied().unwrap_or(0);
    let rate_word = tail.get(5).copied().unwrap_or(0);
    *tick_hz = rate_word as u32;
    *valid_mask = (rate_word >> 32) as u32;
}

/// Human run summary: span in ticks plus milliseconds when the reply
/// carries a tick rate; "pending" while the mask shows no finish stamp.
fn format_duration(duration: u64, tick_hz: u32, valid_mask: u32) -> rt::FixedLogBuffer<40> {
    let mut text = rt::FixedLogBuffer::<40>::new();
    if valid_mask & (1 << PHASE_FINISH) == 0 || duration == 0 {
        let _ = write!(text, "pending");
        return text;
    }
    match tick_hz {
        0 => {
            let _ = write!(text, "{}t", duration);
        }
        hz => {
            let _ = write!(text, "{}t {}ms", duration, duration * 1000 / u64::from(hz));
        }
    }
    text
}

fn developer_target_from_word(word: u64) -> DeveloperTarget {
    match word as u32 {
        x if x == DeveloperTarget::LinuxX64 as u32 => DeveloperTarget::LinuxX64,
        x if x == DeveloperTarget::WindowsX64 as u32 => DeveloperTarget::WindowsX64,
        x if x == DeveloperTarget::MacosX64 as u32 => DeveloperTarget::MacosX64,
        _ => DeveloperTarget::NativeX64,
    }
}

fn developer_job_state_from_word(word: u64) -> DeveloperJobState {
    match word as u32 {
        x if x == DeveloperJobState::Running as u32 => DeveloperJobState::Running,
        x if x == DeveloperJobState::Succeeded as u32 => DeveloperJobState::Succeeded,
        x if x == DeveloperJobState::Failed as u32 => DeveloperJobState::Failed,
        x if x == DeveloperJobState::Unsupported as u32 => DeveloperJobState::Unsupported,
        _ => DeveloperJobState::Queued,
    }
}

fn developer_format_from_word(word: u64) -> DeveloperArtifactFormat {
    match word as u32 {
        x if x == DeveloperArtifactFormat::Elf64 as u32 => DeveloperArtifactFormat::Elf64,
        x if x == DeveloperArtifactFormat::Pe32Plus as u32 => DeveloperArtifactFormat::Pe32Plus,
        x if x == DeveloperArtifactFormat::MachO64 as u32 => DeveloperArtifactFormat::MachO64,
        _ => DeveloperArtifactFormat::ServiceOsFlat,
    }
}

/// `dev profile <job-id>`: raw request against the developer-service
/// profile tags (0xd24 -> 0xd25) followed by a pure decode + render, so
/// the phase table is host-testable without a channel grant.
fn cmd_profile(bootstrap: rt::Handle, output: ShellOutput, job_id: u32) -> rt::Result<()> {
    let developer = match lookup_developer_service(bootstrap, output)? {
        Some(handle) => handle,
        None => return Ok(()),
    };
    let reply = rt::channel_create()?;
    let mut request = RawMessage::empty(DEV_PROFILE_REQUEST_TAG);
    request.word_count = 1;
    request.words[0] = job_id as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rt::rights::SEND;
    rt::channel_send(developer, &request)?;
    let _ = rt::handle_close(reply.second);
    let _ = rt::handle_close(developer);
    let mut response = RawMessage::empty(0);
    rt::channel_receive_blocking(reply.first, &mut response)?;
    let _ = rt::handle_close(reply.first);
    if response.tag != DEV_PROFILE_REPLY_TAG || response.word_count < 2 {
        return Err(rt::Error::InvalidArgument);
    }
    if response.words[0] != DeveloperStatus::Ok as u32 as u64 {
        return write_output_linef(output, format_args!("job{} not found", job_id));
    }
    let tail = &response.words[2..response.word_count as usize];
    let mut buffer = rt::FixedLogBuffer::<512>::new();
    render_profile(tail, &mut buffer);
    shell_output_write(
        output,
        core::str::from_utf8(buffer.as_bytes()).unwrap_or(""),
    )
}

/// Decode + render the profile reply tail [magic|6][queue][start]
/// [tool-exit][artifact][finish][rate|valid] into the operator phase
/// table. Unreached phases print dashes; deltas are computed between
/// consecutive RECORDED stamps and rendered in ticks plus milliseconds
/// when the rate word carries a tick rate (100 Hz = 10 ms per tick).
fn render_profile(tail: &[u64], out: &mut rt::FixedLogBuffer<512>) {
    use core::fmt::Write as _;
    let mut ticks = [0u64; PHASE_COUNT];
    let mut tick_hz = 0u32;
    let mut valid_mask = 0u32;
    let magic = tail.first().copied().unwrap_or(0);
    if tail.len() >= PROFILE_FIELDS + 1
        && magic & 0xFFFF_FFFF == IDE_TAIL_MAGIC
        && (magic >> 32) & 0xFF == PROFILE_FIELDS as u64
    {
        for (slot, word) in ticks.iter_mut().zip(tail[1..1 + PHASE_COUNT].iter()) {
            *slot = *word;
        }
        let rate_word = tail[1 + PHASE_COUNT];
        tick_hz = rate_word as u32;
        valid_mask = (rate_word >> 32) as u32;
    }
    let _ = write!(out, "phase     ticks       delta\r\n");
    let mut previous: Option<usize> = None;
    for (phase, name) in [
        (PHASE_QUEUE, "queue"),
        (PHASE_START, "start"),
        (PHASE_TOOL_EXIT, "tool-exit"),
        (PHASE_ARTIFACT, "artifact"),
        (PHASE_FINISH, "finish"),
    ] {
        if valid_mask & (1 << phase) == 0 {
            let _ = write!(out, "{:<9} -           -\r\n", name);
            continue;
        }
        match previous {
            Some(from) => {
                let delta = ticks[phase].saturating_sub(ticks[from]);
                let _ = write!(
                    out,
                    "{:<9} {:<11} +{}t {}\r\n",
                    name,
                    ticks[phase],
                    delta,
                    delta_ms(delta, tick_hz).as_str()
                );
            }
            None => {
                let _ = write!(out, "{:<9} {:<11} -\r\n", name, ticks[phase]);
            }
        }
        previous = Some(phase);
    }
    if valid_mask & (1 << PHASE_FINISH) != 0 && valid_mask & (1 << PHASE_QUEUE) != 0 {
        let total = ticks[PHASE_FINISH].saturating_sub(ticks[PHASE_QUEUE]);
        let _ = write!(
            out,
            "total {}t {}\r\n",
            total,
            delta_ms(total, tick_hz).as_str()
        );
    }
}

fn delta_ms(delta_ticks: u64, tick_hz: u32) -> rt::FixedLogBuffer<24> {
    let mut text = rt::FixedLogBuffer::<24>::new();
    match tick_hz {
        0 => {
            let _ = write!(text, "raw");
        }
        hz => {
            let _ = write!(text, "{}ms", delta_ticks * 1000 / u64::from(hz));
        }
    }
    text
}

fn cmd_artifact(bootstrap: rt::Handle, output: ShellOutput, job_id: u32) -> rt::Result<()> {
    let developer = match lookup_developer_service(bootstrap, output)? {
        Some(handle) => handle,
        None => return Ok(()),
    };
    let mut name = [0u8; MAX_NAME];
    let (artifact, size, format, name_len) =
        rt::developer_artifact_open(developer, job_id, &mut name)?;
    let _ = rt::handle_close(developer);
    let artifact_name =
        core::str::from_utf8(&name[..name_len]).map_err(|_| rt::Error::InvalidArgument)?;
    let mut header = [0u8; 16];
    let preview_len = rt::memory_read(artifact, 0, &mut header)?;
    let _ = rt::handle_close(artifact);
    write_output_linef(
        output,
        format_args!(
            "job{} artifact={} format={} size={}",
            job_id,
            artifact_name,
            format_name(format),
            size,
        ),
    )?;
    let hex = hex_bytes(&header[..preview_len]);
    let hex = core::str::from_utf8(hex.as_bytes()).map_err(|_| rt::Error::InvalidArgument)?;
    write_output_linef(output, format_args!("  magic={}", hex,))
}

fn cmd_save_artifact(
    bootstrap: rt::Handle,
    output: ShellOutput,
    job_id: u32,
    path: &str,
) -> rt::Result<()> {
    let developer = match lookup_developer_service(bootstrap, output)? {
        Some(handle) => handle,
        None => return Ok(()),
    };
    let mut name = [0u8; MAX_NAME];
    let (artifact, size, format, _) = rt::developer_artifact_open(developer, job_id, &mut name)?;
    let _ = rt::handle_close(developer);

    let storage = rt::lookup_service(bootstrap, ServiceId::Storage)?;
    let mut parent_buffer = rt::FixedLogBuffer::<MAX_STORAGE_PATH>::new();
    let file_name = split_parent_path(path, &mut parent_buffer)?;
    let directory = rt::storage_open_directory(storage, parent_buffer.as_str(), true)?;
    let _ = rt::handle_close(storage);
    let (file, _) = rt::storage_directory_open_file(directory, file_name, true, true)?;
    let _ = rt::handle_close(directory);

    let mut offset = 0usize;
    let mut chunk = [0u8; 96];
    while offset < size {
        let chunk_len = (size - offset).min(chunk.len());
        let read = rt::memory_read(artifact, offset, &mut chunk[..chunk_len])?;
        if read == 0 {
            break;
        }
        let _ = rt::storage_write(file, offset, size, &chunk[..read])?;
        offset += read;
    }
    let _ = rt::storage_blob_close(file);
    let _ = rt::handle_close(artifact);
    write_output_linef(
        output,
        format_args!(
            "saved job{} artifact to {} format={} size={}",
            job_id,
            path,
            format_name(format),
            size,
        ),
    )
}

fn lookup_developer_service(
    bootstrap: rt::Handle,
    output: ShellOutput,
) -> rt::Result<Option<rt::Handle>> {
    match rt::lookup_service(bootstrap, ServiceId::Developer) {
        Ok(handle) => Ok(Some(handle)),
        Err(rt::Error::NotFound) => {
            write_output_linef(
                output,
                format_args!("developer-service unavailable; install package developer"),
            )?;
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn parse_u32(value: &str) -> Option<u32> {
    value.parse::<u32>().ok()
}

fn parse_target(value: &str) -> Option<DeveloperTarget> {
    match value {
        "native" => Some(DeveloperTarget::NativeX64),
        "linux" => Some(DeveloperTarget::LinuxX64),
        "windows" => Some(DeveloperTarget::WindowsX64),
        "macos" => Some(DeveloperTarget::MacosX64),
        _ => None,
    }
}

fn split_parent_path<'a>(
    path: &'a str,
    parent_buffer: &mut rt::FixedLogBuffer<MAX_STORAGE_PATH>,
) -> rt::Result<&'a str> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return Err(rt::Error::InvalidArgument);
    }
    match trimmed.rsplit_once('/') {
        Some((parent, name)) if !name.is_empty() => {
            let _ = parent_buffer.write_str(parent);
            let _ = parent_buffer.write_str("/");
            Ok(name)
        }
        Some(_) => Err(rt::Error::InvalidArgument),
        None => Ok(trimmed),
    }
}

fn target_name(target: DeveloperTarget) -> &'static str {
    match target {
        DeveloperTarget::NativeX64 => "native-x64",
        DeveloperTarget::LinuxX64 => "linux-x64",
        DeveloperTarget::WindowsX64 => "windows-x64",
        DeveloperTarget::MacosX64 => "macos-x64",
    }
}

fn target_mask_name(mask: u32) -> &'static str {
    match mask {
        0b0001 => "native",
        0b0011 => "native,linux",
        0b0111 => "native,linux,windows",
        0b1111 => "native,linux,windows,macos",
        _ => "mixed",
    }
}

fn toolchain_state_name(state: DeveloperToolchainState) -> &'static str {
    match state {
        DeveloperToolchainState::Installed => "installed",
        DeveloperToolchainState::RemoteOnly => "remote-only",
    }
}

fn format_name(format: DeveloperArtifactFormat) -> &'static str {
    match format {
        DeveloperArtifactFormat::ServiceOsFlat => "serviceos-flat",
        DeveloperArtifactFormat::Elf64 => "elf64",
        DeveloperArtifactFormat::Pe32Plus => "pe32+",
        DeveloperArtifactFormat::MachO64 => "macho64",
    }
}

fn job_state_name(state: DeveloperJobState) -> &'static str {
    match state {
        DeveloperJobState::Queued => "queued",
        DeveloperJobState::Running => "running",
        DeveloperJobState::Succeeded => "succeeded",
        DeveloperJobState::Failed => "failed",
        DeveloperJobState::Unsupported => "unsupported",
    }
}

fn printable_toolchain_id(value: u32) -> u32 {
    if value == u32::MAX { u32::MAX } else { value }
}

fn hex_bytes(bytes: &[u8]) -> rt::FixedLogBuffer<64> {
    let mut out = rt::FixedLogBuffer::<64>::new();
    for (index, byte) in bytes.iter().copied().enumerate() {
        let _ = if index == 0 {
            write!(out, "{:02x}", byte)
        } else {
            write!(out, " {:02x}", byte)
        };
    }
    out
}

const fn empty_toolchain() -> rt::DeveloperToolchainInfo {
    rt::DeveloperToolchainInfo {
        toolchain_id: 0,
        target: DeveloperTarget::NativeX64,
        state: DeveloperToolchainState::Installed,
        format: DeveloperArtifactFormat::ServiceOsFlat,
        name_len: 0,
        name: [0; 64],
    }
}

const fn empty_workspace() -> rt::DeveloperWorkspaceInfo {
    rt::DeveloperWorkspaceInfo {
        workspace_id: 0,
        target_mask: 0,
        name_len: 0,
        name: [0; 64],
        source_path_len: 0,
        source_path: [0; 96],
        toolchains: [u32::MAX; 4],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate_word(tick_hz: u32, valid_mask: u32) -> u64 {
        u64::from(tick_hz) | (u64::from(valid_mask) << 32)
    }

    fn full_run_tail() -> [u64; PROFILE_FIELDS + 1] {
        let mut tail = [0u64; PROFILE_FIELDS + 1];
        tail[0] = IDE_TAIL_MAGIC | ((PROFILE_FIELDS as u64) << 32);
        tail[1 + PHASE_QUEUE] = 1000;
        tail[1 + PHASE_START] = 1000;
        tail[1 + PHASE_TOOL_EXIT] = 1100;
        tail[1 + PHASE_ARTIFACT] = 1100;
        tail[1 + PHASE_FINISH] = 1200;
        tail[1 + PHASE_COUNT] = rate_word(100, 0b1_1111);
        tail
    }

    #[test]
    fn list_tail_decode_reads_new_fields_and_ignores_old_grammar() {
        let mut duration = 0u64;
        let mut tick_hz = 0u32;
        let mut valid_mask = 0u32;
        // Old three-field tail: no timing words, decode leaves defaults.
        let old = [IDE_TAIL_MAGIC | (3u64 << 32), 7, 0, 1];
        decode_job_list_timing(&old, &mut duration, &mut tick_hz, &mut valid_mask);
        assert_eq!((duration, tick_hz, valid_mask), (0, 0, 0));
        // New five-field tail: duration + rate/valid land.
        let new = [
            IDE_TAIL_MAGIC | (5u64 << 32),
            7,
            0,
            1,
            120,
            rate_word(100, 0b1_1111),
        ];
        decode_job_list_timing(&new, &mut duration, &mut tick_hz, &mut valid_mask);
        assert_eq!(duration, 120);
        assert_eq!(tick_hz, 100);
        assert_eq!(valid_mask, 0b1_1111);
    }

    #[test]
    fn duration_formats_pending_raw_and_ms() {
        assert!(starts_with(format_duration(0, 100, 0).as_str(), "pending"));
        assert!(starts_with(format_duration(90, 0, 0b11111).as_str(), "90t"));
        assert!(starts_with(
            format_duration(120, 100, 0b1_1111).as_str(),
            "120t 1200ms"
        ));
    }

    #[test]
    fn profile_renders_full_phase_table_with_ms_deltas() {
        let mut out = rt::FixedLogBuffer::<512>::new();
        render_profile(&full_run_tail(), &mut out);
        let text = core::str::from_utf8(out.as_bytes()).unwrap();
        assert!(contains(text, "queue     1000        -"));
        assert!(contains(text, "start     1000        +0t 0ms"));
        assert!(contains(text, "tool-exit 1100        +100t 1000ms"));
        assert!(contains(text, "artifact  1100        +0t 0ms"));
        assert!(contains(text, "finish    1200        +100t 1000ms"));
        assert!(contains(text, "total 200t 2000ms"));
    }

    #[test]
    fn profile_renders_unreached_phases_as_dashes() {
        let mut tail = [0u64; PROFILE_FIELDS + 1];
        tail[0] = IDE_TAIL_MAGIC | ((PROFILE_FIELDS as u64) << 32);
        tail[1 + PHASE_QUEUE] = 500;
        tail[1 + PHASE_START] = 500;
        tail[1 + PHASE_COUNT] = rate_word(100, (1 << PHASE_QUEUE) | (1 << PHASE_START));
        let mut out = rt::FixedLogBuffer::<512>::new();
        render_profile(&tail, &mut out);
        let text = core::str::from_utf8(out.as_bytes()).unwrap();
        assert!(contains(text, "tool-exit -"));
        assert!(contains(text, "finish    -"));
        assert!(!contains(text, "total"));
    }

    #[test]
    fn profile_with_bad_magic_renders_all_dashes() {
        let mut tail = full_run_tail();
        tail[0] = 0xdead_beef;
        let mut out = rt::FixedLogBuffer::<512>::new();
        render_profile(&tail, &mut out);
        let text = core::str::from_utf8(out.as_bytes()).unwrap();
        assert!(contains(text, "queue     -"));
        assert!(contains(text, "finish    -"));
        assert!(!contains(text, "total"));
    }

    fn starts_with(haystack: &str, needle: &str) -> bool {
        haystack.as_bytes().starts_with(needle.as_bytes())
    }

    fn contains(haystack: &str, needle: &str) -> bool {
        let hay = haystack.as_bytes();
        let needle = needle.as_bytes();
        hay.windows(needle.len()).any(|window| window == needle)
    }
}

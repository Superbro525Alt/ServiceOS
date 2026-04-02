use core::fmt::Write;

use serviceos_userspace_runtime as rt;
use rt::{DeveloperArtifactFormat, DeveloperJobState, DeveloperTarget, DeveloperToolchainState, ServiceId};

use crate::util::{ShellOutput, shell_output_write, write_output_linef};

const MAX_TOOLCHAINS: usize = 8;
const MAX_WORKSPACES: usize = 8;
const MAX_JOBS: usize = 8;
const MAX_NAME: usize = 64;
const MAX_PATH: usize = 96;
const MAX_OUTPUT_CHUNK: usize = (rt::IPC_MAX_WORDS - 1) * 8;
const MAX_STORAGE_PATH: usize = 96;

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
            (Some(workspace_id), Some(target)) => cmd_build(bootstrap, output, workspace_id, target),
            _ => write_output_linef(
                output,
                format_args!("usage: dev build <workspace-id> <native|linux|windows|macos>"),
            ),
        },
        Some("jobs") => cmd_jobs(bootstrap, output),
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
                "usage: dev <toolchains|toolchain|workspaces|workspace|build|jobs|artifact|save> ..."
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
    write_output_linef(output, format_args!("toolchain{} {}", toolchain.toolchain_id, name))?;
    write_output_linef(output, format_args!("  target={}", target_name(toolchain.target)))?;
    write_output_linef(output, format_args!("  state={}", toolchain_state_name(toolchain.state)))?;
    write_output_linef(output, format_args!("  format={}", format_name(toolchain.format)))?;
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
    let workspace = rt::developer_workspace_status(developer, workspace_id, &mut name, &mut source)?;
    let _ = rt::handle_close(developer);
    let name = core::str::from_utf8(&name[..workspace.name_len as usize])
        .map_err(|_| rt::Error::InvalidArgument)?;
    let source = core::str::from_utf8(&source[..workspace.source_path_len as usize])
        .map_err(|_| rt::Error::InvalidArgument)?;
    write_output_linef(output, format_args!("workspace{} {}", workspace.workspace_id, name))?;
    write_output_linef(output, format_args!("  source={}", source))?;
    write_output_linef(output, format_args!("  targets={}", target_mask_name(workspace.target_mask)))?;
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
                format_args!("started build job{} workspace={} target={}", job_id, workspace_id, target_name(target)),
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
                    DeveloperJobState::Succeeded | DeveloperJobState::Failed | DeveloperJobState::Unsupported
                ) {
                    let _ = rt::handle_close(relay.first);
                    let _ = rt::handle_close(developer);
                    let artifact_name = core::str::from_utf8(&name[..job.artifact_name_len as usize])
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
                format_args!("target {} is not locally supported yet", target_name(target)),
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
    let mut jobs = [empty_job(); MAX_JOBS];
    let count = rt::developer_job_list(developer, &mut jobs)?;
    let _ = rt::handle_close(developer);
    if count == 0 {
        return write_output_linef(output, format_args!("no developer jobs"));
    }
    for job in jobs.iter().take(count).copied() {
        write_output_linef(
            output,
            format_args!(
                "job{} workspace={} target={} state={} format={} size={}",
                job.job_id,
                job.workspace_id,
                target_name(job.target),
                job_state_name(job.state),
                format_name(job.format),
                job.artifact_size,
            ),
        )?;
    }
    Ok(())
}

fn cmd_artifact(bootstrap: rt::Handle, output: ShellOutput, job_id: u32) -> rt::Result<()> {
    let developer = match lookup_developer_service(bootstrap, output)? {
        Some(handle) => handle,
        None => return Ok(()),
    };
    let mut name = [0u8; MAX_NAME];
    let (artifact, size, format, name_len) = rt::developer_artifact_open(developer, job_id, &mut name)?;
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
    write_output_linef(
        output,
        format_args!(
            "  magic={}",
            hex,
        ),
    )
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

const fn empty_job() -> rt::DeveloperJobInfo {
    rt::DeveloperJobInfo {
        job_id: 0,
        workspace_id: 0,
        target: DeveloperTarget::NativeX64,
        state: DeveloperJobState::Queued,
        format: DeveloperArtifactFormat::ServiceOsFlat,
        artifact_size: 0,
        artifact_name_len: 0,
        artifact_name: [0; 64],
    }
}

use rt::{
    DeveloperArtifactFormat, DeveloperJobState, DeveloperStatus, DeveloperTag, DeveloperTarget,
    DeveloperToolchainState, LogEvent, LogSeverity, RawMessage,
};
use serviceos_userspace_runtime as rt;

use crate::{
    consts::{BUILDER_REPORT_TAG, MAX_JOBS},
    types::{JobSlot, ToolchainSlot, WorkspaceSlot},
    util::{
        allocate_job, duplicate_artifact_for_reply, emit_log, load_source_into_memory,
        send_builder_log, target_slot_index, workspace_target_mask,
    },
};

pub(crate) struct Catalog<'a> {
    pub(crate) toolchains: &'a [ToolchainSlot],
    pub(crate) toolchain_count: usize,
    pub(crate) workspaces: &'a [WorkspaceSlot],
    pub(crate) workspace_count: usize,
}

pub(crate) fn handle_public_request(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    log_handle: rt::Handle,
    catalog: Catalog<'_>,
    jobs: &mut [JobSlot; MAX_JOBS],
    message: &RawMessage,
) -> rt::Result<()> {
    match message.tag {
        x if x == DeveloperTag::ToolchainListRequest as u32 => {
            reply_toolchain_list(catalog.toolchains, catalog.toolchain_count, message)
        }
        x if x == DeveloperTag::ToolchainInfoRequest as u32 => {
            reply_toolchain_info(catalog.toolchains, catalog.toolchain_count, message)
        }
        x if x == DeveloperTag::WorkspaceListRequest as u32 => {
            reply_workspace_list(catalog.workspaces, catalog.workspace_count, message)
        }
        x if x == DeveloperTag::WorkspaceInfoRequest as u32 => {
            reply_workspace_info(catalog.workspaces, catalog.workspace_count, message)
        }
        x if x == DeveloperTag::BuildRequest as u32 => handle_build_request(
            bootstrap,
            storage_handle,
            log_handle,
            catalog,
            jobs,
            message,
        ),
        x if x == DeveloperTag::JobListRequest as u32 => reply_job_list(jobs, message),
        x if x == DeveloperTag::JobInfoRequest as u32 => reply_job_info(jobs, message),
        x if x == DeveloperTag::ArtifactOpenRequest as u32 => {
            handle_artifact_open_request(log_handle, jobs, message)
        }
        _ => Ok(()),
    }
}

fn reply_toolchain_list(
    toolchains: &[ToolchainSlot],
    toolchain_count: usize,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.handle_count < 1 || message.word_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let mut reply = RawMessage::empty(DeveloperTag::ToolchainListReply as u32);
    reply.word_count = 6;
    reply.words[0] = DeveloperStatus::NotFound as u32 as u64;
    let index = message.words[0] as usize;
    if let Some(toolchain) = toolchains[..toolchain_count]
        .get(index)
        .copied()
        .filter(|toolchain| toolchain.occupied)
    {
        reply.words[0] = DeveloperStatus::Ok as u32 as u64;
        reply.words[1] = index as u64;
        reply.words[2] = toolchain.target as u32 as u64;
        reply.words[3] = toolchain.state as u32 as u64;
        reply.words[4] = toolchain.format as u32 as u64;
        reply.words[5] = toolchain.name.len as u64;
        reply.word_count += rt::pack_bytes(toolchain.name.as_bytes(), &mut reply.words[6..])?;
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn reply_toolchain_info(
    toolchains: &[ToolchainSlot],
    toolchain_count: usize,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.handle_count < 1 || message.word_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let mut reply = RawMessage::empty(DeveloperTag::ToolchainInfoReply as u32);
    reply.word_count = 7;
    reply.words[0] = DeveloperStatus::NotFound as u32 as u64;
    let index = message.words[0] as usize;
    if let Some(toolchain) = toolchains[..toolchain_count]
        .get(index)
        .copied()
        .filter(|toolchain| toolchain.occupied)
    {
        reply.words[0] = DeveloperStatus::Ok as u32 as u64;
        reply.words[1] = index as u64;
        reply.words[2] = toolchain.target as u32 as u64;
        reply.words[3] = toolchain.state as u32 as u64;
        reply.words[4] = toolchain.format as u32 as u64;
        reply.words[5] = toolchain.name.len as u64;
        reply.words[6] = toolchain.sdk_root.len as u64;
        let mut combined = [0u8; (rt::IPC_MAX_WORDS - 7) * 8];
        let mut total = 0usize;
        combined[..toolchain.name.len].copy_from_slice(toolchain.name.as_bytes());
        total += toolchain.name.len;
        combined[total..total + toolchain.sdk_root.len]
            .copy_from_slice(toolchain.sdk_root.as_bytes());
        total += toolchain.sdk_root.len;
        reply.word_count += rt::pack_bytes(&combined[..total], &mut reply.words[7..])?;
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn reply_workspace_list(
    workspaces: &[WorkspaceSlot],
    workspace_count: usize,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.handle_count < 1 || message.word_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let mut reply = RawMessage::empty(DeveloperTag::WorkspaceListReply as u32);
    reply.word_count = 4;
    reply.words[0] = DeveloperStatus::NotFound as u32 as u64;
    let index = message.words[0] as usize;
    if let Some(workspace) = workspaces[..workspace_count]
        .get(index)
        .copied()
        .filter(|workspace| workspace.occupied)
    {
        reply.words[0] = DeveloperStatus::Ok as u32 as u64;
        reply.words[1] = index as u64;
        reply.words[2] = workspace_target_mask(&workspace) as u64;
        reply.words[3] = workspace.name.len as u64;
        reply.word_count += rt::pack_bytes(workspace.name.as_bytes(), &mut reply.words[4..])?;
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn reply_workspace_info(
    workspaces: &[WorkspaceSlot],
    workspace_count: usize,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.handle_count < 1 || message.word_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let mut reply = RawMessage::empty(DeveloperTag::WorkspaceInfoReply as u32);
    reply.word_count = 5;
    reply.words[0] = DeveloperStatus::NotFound as u32 as u64;
    let index = message.words[0] as usize;
    if let Some(workspace) = workspaces[..workspace_count]
        .get(index)
        .copied()
        .filter(|workspace| workspace.occupied)
    {
        reply.words[0] = DeveloperStatus::Ok as u32 as u64;
        reply.words[1] = (index as u64) | ((workspace_target_mask(&workspace) as u64) << 32);
        reply.words[2] = (workspace.name.len as u64) | ((workspace.source_path.len as u64) << 32);
        reply.words[3] =
            (workspace.toolchains[0] as u64) | ((workspace.toolchains[1] as u64) << 32);
        reply.words[4] =
            (workspace.toolchains[2] as u64) | ((workspace.toolchains[3] as u64) << 32);
        let mut combined = [0u8; (rt::IPC_MAX_WORDS - 5) * 8];
        let mut total = 0usize;
        combined[..workspace.name.len].copy_from_slice(workspace.name.as_bytes());
        total += workspace.name.len;
        combined[total..total + workspace.source_path.len]
            .copy_from_slice(workspace.source_path.as_bytes());
        total += workspace.source_path.len;
        reply.word_count += rt::pack_bytes(&combined[..total], &mut reply.words[5..])?;
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn handle_build_request(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    log_handle: rt::Handle,
    catalog: Catalog<'_>,
    jobs: &mut [JobSlot; MAX_JOBS],
    message: &RawMessage,
) -> rt::Result<()> {
    if message.handle_count < 2 || message.word_count < 2 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let output_handle = message.handles[1];
    let workspace_id = message.words[0] as usize;
    let target = match message.words[1] as u32 {
        x if x == DeveloperTarget::LinuxX64 as u32 => DeveloperTarget::LinuxX64,
        x if x == DeveloperTarget::WindowsX64 as u32 => DeveloperTarget::WindowsX64,
        x if x == DeveloperTarget::MacosX64 as u32 => DeveloperTarget::MacosX64,
        _ => DeveloperTarget::NativeX64,
    };

    let mut reply = RawMessage::empty(DeveloperTag::BuildReply as u32);
    reply.word_count = 2;
    reply.words[0] = DeveloperStatus::NotFound as u32 as u64;
    reply.words[1] = u32::MAX as u64;

    let Some(workspace) = catalog.workspaces[..catalog.workspace_count]
        .get(workspace_id)
        .copied()
        .filter(|workspace| workspace.occupied)
    else {
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(reply_handle);
        let _ = rt::handle_close(output_handle);
        return Ok(());
    };

    let toolchain_index = workspace.toolchains[target_slot_index(target)];
    if toolchain_index == u32::MAX {
        reply.words[0] = DeveloperStatus::Unsupported as u32 as u64;
        let _ = emit_log(
            log_handle,
            LogSeverity::Warn,
            LogEvent::DeveloperBuildFailed,
            workspace_id as u64,
            target as u32 as u64,
        );
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(reply_handle);
        let _ = rt::handle_close(output_handle);
        return Ok(());
    }

    let toolchain = catalog.toolchains[toolchain_index as usize];
    if toolchain.state == DeveloperToolchainState::RemoteOnly {
        reply.words[0] = DeveloperStatus::Unsupported as u32 as u64;
        let _ = emit_log(
            log_handle,
            LogSeverity::Warn,
            LogEvent::DeveloperBuildFailed,
            workspace_id as u64,
            target as u32 as u64,
        );
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(reply_handle);
        let _ = rt::handle_close(output_handle);
        return Ok(());
    }

    let job_id = match allocate_job(jobs) {
        Ok(index) => index,
        Err(_) => {
            reply.words[0] = DeveloperStatus::Busy as u32 as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
            let _ = rt::handle_close(output_handle);
            return Ok(());
        }
    };

    let (source_memory, source_len) = match load_source_into_memory(storage_handle, &workspace) {
        Ok(source) => source,
        Err(_) => {
            reply.words[0] = DeveloperStatus::Busy as u32 as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
            let _ = rt::handle_close(output_handle);
            return Ok(());
        }
    };

    let report = rt::channel_create()?;
    send_builder_log(output_handle, "dev: preparing build environment\r\n");
    send_builder_log(output_handle, "dev: launching cross-builder-tool\r\n");

    let startup_handles = [
        rt::StartupHandle {
            handle: output_handle,
            rights: rt::rights::SEND | rt::rights::DUPLICATE | rt::rights::TRANSFER,
        },
        rt::StartupHandle {
            handle: report.second,
            rights: rt::rights::SEND | rt::rights::DUPLICATE | rt::rights::TRANSFER,
        },
        rt::StartupHandle {
            handle: source_memory,
            rights: rt::rights::READ | rt::rights::DUPLICATE | rt::rights::TRANSFER,
        },
    ];
    let mut startup_words = [0u64; rt::IPC_MAX_WORDS];
    startup_words[0] = target as u32 as u64;
    startup_words[1] = source_len as u64;
    startup_words[2] = workspace.artifact.len as u64;
    let packed =
        rt::pack_bytes(workspace.artifact.as_bytes(), &mut startup_words[3..]).unwrap_or(0);
    let task_handle = rt::manager_launch_program_with_payload(
        bootstrap,
        rt::ServiceImageId::CrossBuilderTool,
        &startup_words[..3 + packed as usize],
        &startup_handles,
    );
    let _ = rt::handle_close(output_handle);
    let _ = rt::handle_close(report.second);
    let _ = rt::handle_close(source_memory);

    match task_handle {
        Ok(task_handle) => {
            jobs[job_id] = JobSlot {
                occupied: true,
                workspace_id: workspace_id as u32,
                target,
                state: DeveloperJobState::Running,
                format: toolchain.format,
                artifact_name: workspace.artifact,
                artifact_size: 0,
                artifact_handle: rt::INVALID_HANDLE,
                task_handle,
                report_handle: report.first,
            };
            reply.words[0] = DeveloperStatus::Ok as u32 as u64;
            reply.words[1] = job_id as u64;
            let _ = emit_log(
                log_handle,
                LogSeverity::Info,
                LogEvent::DeveloperBuildStarted,
                job_id as u64,
                ((workspace_id as u64) << 32) | target as u32 as u64,
            );
        }
        Err(error) => {
            let _ = rt::handle_close(report.first);
            reply.words[0] = match error {
                rt::Error::PermissionDenied => DeveloperStatus::Denied as u32 as u64,
                rt::Error::NotFound => DeveloperStatus::NotFound as u32 as u64,
                rt::Error::Unsupported => DeveloperStatus::Unsupported as u32 as u64,
                _ => DeveloperStatus::Busy as u32 as u64,
            };
        }
    }

    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn reply_job_list(jobs: &[JobSlot; MAX_JOBS], message: &RawMessage) -> rt::Result<()> {
    if message.handle_count < 1 || message.word_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let mut reply = RawMessage::empty(DeveloperTag::JobListReply as u32);
    reply.word_count = 7;
    reply.words[0] = DeveloperStatus::NotFound as u32 as u64;
    let index = message.words[0] as usize;
    if let Some(job) = jobs.get(index).copied().filter(|job| job.occupied) {
        reply.words[0] = DeveloperStatus::Ok as u32 as u64;
        reply.words[1] = index as u64;
        reply.words[2] = job.workspace_id as u64;
        reply.words[3] = job.target as u32 as u64;
        reply.words[4] = job.state as u32 as u64;
        reply.words[5] = job.format as u32 as u64;
        reply.words[6] = job.artifact_size as u64;
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn reply_job_info(jobs: &[JobSlot; MAX_JOBS], message: &RawMessage) -> rt::Result<()> {
    if message.handle_count < 1 || message.word_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let mut reply = RawMessage::empty(DeveloperTag::JobInfoReply as u32);
    reply.word_count = 8;
    reply.words[0] = DeveloperStatus::NotFound as u32 as u64;
    let index = message.words[0] as usize;
    if let Some(job) = jobs.get(index).copied().filter(|job| job.occupied) {
        reply.words[0] = DeveloperStatus::Ok as u32 as u64;
        reply.words[1] = index as u64;
        reply.words[2] = job.workspace_id as u64;
        reply.words[3] = job.target as u32 as u64;
        reply.words[4] = job.state as u32 as u64;
        reply.words[5] = job.format as u32 as u64;
        reply.words[6] = job.artifact_size as u64;
        reply.words[7] = job.artifact_name.len as u64;
        reply.word_count += rt::pack_bytes(job.artifact_name.as_bytes(), &mut reply.words[8..])?;
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn handle_artifact_open_request(
    log_handle: rt::Handle,
    jobs: &[JobSlot; MAX_JOBS],
    message: &RawMessage,
) -> rt::Result<()> {
    if message.handle_count < 1 || message.word_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let mut reply = RawMessage::empty(DeveloperTag::ArtifactOpenReply as u32);
    reply.word_count = 4;
    reply.words[0] = DeveloperStatus::NotFound as u32 as u64;
    let index = message.words[0] as usize;
    if let Some(job) = jobs.get(index).copied().filter(|job| {
        job.occupied
            && job.state == DeveloperJobState::Succeeded
            && job.artifact_handle != rt::INVALID_HANDLE
    }) {
        let duplicated = duplicate_artifact_for_reply(job.artifact_handle)?;
        reply.words[0] = DeveloperStatus::Ok as u32 as u64;
        reply.words[1] = job.artifact_size as u64;
        reply.words[2] = job.format as u32 as u64;
        reply.words[3] = job.artifact_name.len as u64;
        reply.word_count += rt::pack_bytes(job.artifact_name.as_bytes(), &mut reply.words[4..])?;
        reply.handle_count = 1;
        reply.handles[0] = duplicated;
        reply.handle_rights[0] = rt::rights::READ;
        let _ = emit_log(
            log_handle,
            LogSeverity::Info,
            LogEvent::DeveloperArtifactOpened,
            index as u64,
            job.artifact_size as u64,
        );
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(duplicated);
        let _ = rt::handle_close(reply_handle);
        return Ok(());
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

pub(crate) fn poll_job_reports(log_handle: rt::Handle, jobs: &mut [JobSlot; MAX_JOBS]) {
    for (job_id, job) in jobs.iter_mut().enumerate() {
        if !job.occupied || job.report_handle == rt::INVALID_HANDLE {
            continue;
        }
        let mut report = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(job.report_handle, &mut report) {
            Ok(()) if report.tag == BUILDER_REPORT_TAG && report.word_count >= 4 => {
                let result = report.words[0] as u32;
                job.format = match report.words[1] as u32 {
                    x if x == DeveloperArtifactFormat::Elf64 as u32 => {
                        DeveloperArtifactFormat::Elf64
                    }
                    x if x == DeveloperArtifactFormat::Pe32Plus as u32 => {
                        DeveloperArtifactFormat::Pe32Plus
                    }
                    x if x == DeveloperArtifactFormat::MachO64 as u32 => {
                        DeveloperArtifactFormat::MachO64
                    }
                    _ => DeveloperArtifactFormat::ServiceOsFlat,
                };
                job.artifact_size = report.words[2] as usize;
                let name_len = report.words[3] as usize;
                let _ = rt::unpack_bytes(
                    &report.words[4..report.word_count as usize],
                    name_len,
                    &mut job.artifact_name.bytes,
                );
                job.artifact_name.len = name_len;
                if result == 0 && report.handle_count > 0 {
                    job.state = DeveloperJobState::Succeeded;
                    job.artifact_handle = report.handles[0];
                    let _ = emit_log(
                        log_handle,
                        LogSeverity::Info,
                        LogEvent::DeveloperBuildFinished,
                        job_id as u64,
                        job.artifact_size as u64,
                    );
                } else if result == 1 {
                    job.state = DeveloperJobState::Unsupported;
                    let _ = emit_log(
                        log_handle,
                        LogSeverity::Warn,
                        LogEvent::DeveloperBuildFailed,
                        job_id as u64,
                        1,
                    );
                } else {
                    job.state = DeveloperJobState::Failed;
                    let _ = emit_log(
                        log_handle,
                        LogSeverity::Error,
                        LogEvent::DeveloperBuildFailed,
                        job_id as u64,
                        2,
                    );
                }
                let _ = rt::handle_close(job.report_handle);
                job.report_handle = rt::INVALID_HANDLE;
            }
            _ => {}
        }
    }
}

pub(crate) fn poll_job_exits(log_handle: rt::Handle, jobs: &mut [JobSlot; MAX_JOBS]) {
    for (job_id, job) in jobs.iter_mut().enumerate() {
        if !job.occupied || job.task_handle == rt::INVALID_HANDLE {
            continue;
        }
        match rt::task_status(job.task_handle) {
            Ok(status)
                if matches!(
                    status.state,
                    rt::TaskStateCode::Exited | rt::TaskStateCode::Faulted
                ) =>
            {
                let _ = rt::handle_close(job.task_handle);
                job.task_handle = rt::INVALID_HANDLE;
                if job.state == DeveloperJobState::Running {
                    job.state = DeveloperJobState::Failed;
                    let _ = emit_log(
                        log_handle,
                        LogSeverity::Error,
                        LogEvent::DeveloperBuildFailed,
                        job_id as u64,
                        status.exit_code,
                    );
                }
            }
            _ => {}
        }
    }
}

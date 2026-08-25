use rt::{
    DeveloperArtifactFormat, DeveloperJobState, DeveloperStatus, DeveloperTag, DeveloperTarget,
    DeveloperToolchainState, LogEvent, LogSeverity, RawMessage, RuntimeStatus, RuntimeTag,
};
use serviceos_userspace_runtime as rt;

use crate::{
    consts::{
        BUILDER_REPORT_TAG, IDE_JOB_INFO_REPLY_TAG, IDE_JOB_INFO_REQUEST_TAG, MAX_JOBS,
        MAX_TOOLCHAINS,
    },
    farm,
    registry::{self, RegistryRecord},
    routing, sandbox,
    types::{ExportState, FixedBytes, JobSlot, ToolchainSlot, WorkspaceSlot},
    util::{
        allocate_job, create_memory_from_bytes, duplicate_artifact_for_reply, emit_log,
        load_source_into_memory, send_builder_log, target_slot_index, workspace_target_mask,
    },
};

pub(crate) struct Catalog<'a> {
    pub(crate) toolchains: &'a [ToolchainSlot],
    pub(crate) toolchain_count: usize,
    pub(crate) workspaces: &'a [WorkspaceSlot],
    pub(crate) workspace_count: usize,
    pub(crate) registry: &'a mut [RegistryRecord; MAX_TOOLCHAINS],
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
        x if x == DeveloperTag::ToolchainListRequest as u32 => reply_toolchain_list(
            catalog.registry,
            catalog.toolchains,
            catalog.toolchain_count,
            message,
        ),
        x if x == DeveloperTag::ToolchainInfoRequest as u32 => reply_toolchain_info(
            storage_handle,
            catalog.registry,
            catalog.toolchains,
            catalog.toolchain_count,
            message,
        ),
        x if x == DeveloperTag::WorkspaceListRequest as u32 => {
            reply_workspace_list(catalog.workspaces, catalog.workspace_count, message)
        }
        x if x == DeveloperTag::WorkspaceInfoRequest as u32 => reply_workspace_info(
            catalog.workspaces,
            catalog.workspace_count,
            catalog.toolchains,
            catalog.toolchain_count,
            message,
        ),
        x if x == DeveloperTag::BuildRequest as u32 => handle_build_request(
            bootstrap,
            storage_handle,
            log_handle,
            catalog,
            jobs,
            message,
        ),
        x if x == DeveloperTag::JobListRequest as u32 => {
            reply_job_list(catalog.workspaces, catalog.workspace_count, jobs, message)
        }
        x if x == DeveloperTag::JobInfoRequest as u32 => {
            reply_job_info(catalog.workspaces, catalog.workspace_count, jobs, message)
        }
        x if x == IDE_JOB_INFO_REQUEST_TAG => {
            reply_ide_job_info(catalog.workspaces, catalog.workspace_count, jobs, message)
        }
        x if x == DeveloperTag::ArtifactOpenRequest as u32 => {
            handle_artifact_open_request(log_handle, jobs, message)
        }
        _ => Ok(()),
    }
}

fn reply_toolchain_list(
    registry: &[RegistryRecord; MAX_TOOLCHAINS],
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
        let packed_name =
            rt::pack_bytes(toolchain.name.as_bytes(), &mut reply.words[6..])? as usize;
        // Trailing registry fields appended after the packed name region so
        // existing clients that stop reading at the name are unaffected:
        // family, newest-first rank within the family, version text.
        let record = registry
            .get(index)
            .copied()
            .unwrap_or(RegistryRecord::empty());
        let mut version_text = [0u8; 32];
        let version_bytes: &[u8] = record
            .version
            .as_ref()
            .map(|version| registry::format_version_text(version, &mut version_text))
            .unwrap_or(&[]);
        let tail_base = 6 + packed_name;
        reply.words[tail_base] = record.family as u64;
        reply.words[tail_base + 1] = u64::from(record.rank);
        reply.words[tail_base + 2] = version_bytes.len() as u64;
        let packed_version = rt::pack_bytes(version_bytes, &mut reply.words[tail_base + 3..])?;
        reply.word_count += (packed_name + 3) as u32 + packed_version;
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn reply_toolchain_info(
    storage_handle: rt::Handle,
    registry: &mut [RegistryRecord; MAX_TOOLCHAINS],
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
        // Verify-present operation: probe the SDK install root live and
        // record the result on the registry entry before answering.
        let present = registry::verify_present(storage_handle, &toolchain);
        if let Some(record) = registry.get_mut(index) {
            record.present = present;
        }
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
        // Trailing verify word appended after the packed byte region so
        // existing clients that stop reading at name/sdk lengths are
        // unaffected.
        reply.words[reply.word_count as usize] = present as u64;
        reply.word_count += 1;
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
    toolchains: &[ToolchainSlot],
    toolchain_count: usize,
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
        // IDE tail (additive): [magic|2][project][farm]; project packs the
        // target-slot mask with the default (first resolved) slot in bits
        // 8..; farm is a bitmask of target slots whose descriptor registers
        // a configured remote endpoint.
        if ide_tail_fits(reply.word_count, 2) {
            let base = reply.word_count as usize;
            reply.word_count += 3;
            reply.words[base] = ide_tail_magic(2);
            let default_slot = workspace
                .toolchains
                .iter()
                .position(|toolchain| *toolchain != u32::MAX)
                .unwrap_or(usize::MAX) as u64;
            reply.words[base + 1] = workspace_target_mask(&workspace) as u64 | (default_slot << 8);
            reply.words[base + 2] = farm::configured_mask(toolchains, toolchain_count) as u64;
        }
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
        return reply_remote_target_build(
            bootstrap,
            log_handle,
            catalog,
            jobs,
            workspace_id,
            target,
            &workspace,
            &toolchain,
            message,
        );
    }
    // Registry verify-present gate: an installed toolchain whose SDK root
    // no longer resolves in storage cannot run a build job.
    let present = registry::verify_present(storage_handle, &toolchain);
    if let Some(record) = catalog.registry.get_mut(toolchain_index as usize) {
        record.present = present;
    }
    if !present {
        reply.words[0] = DeveloperStatus::Unsupported as u32 as u64;
        let _ = emit_log(
            log_handle,
            LogSeverity::Warn,
            LogEvent::DeveloperBuildFailed,
            workspace_id as u64,
            ((target as u64) << 32) | registry::TOOLCHAIN_ROOT_MISSING,
        );
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(reply_handle);
        let _ = rt::handle_close(output_handle);
        return Ok(());
    }

    // Runtime-aware routing: word[2] carries an optional runtime profile
    // tag; when it matches an active runtime-service environment the job is
    // routed through that contract, otherwise the direct worker spawn path
    // is reused.
    let profile = if message.word_count >= 3 {
        message.words[2] as u32
    } else {
        routing::RUNTIME_PROFILE_NONE
    };
    let probed = if profile == routing::RUNTIME_PROFILE_NONE {
        None
    } else {
        routing::probe_runtime_envs(bootstrap)
    };
    let empty = [routing::RuntimeEnvSnapshot {
        env_id: 0,
        kind: 0,
        state: 0,
        capabilities: 0,
    }; 0];
    let route = match profile {
        routing::RUNTIME_PROFILE_NONE => routing::BuildRoute::DirectSpawn,
        _ => match probed.as_ref() {
            Some(envs) => routing::route_for(profile, envs),
            None => routing::route_for(profile, &empty),
        },
    };

    // Capability manifest: explicit fs scope prefixes derived from the
    // workspace descriptor plus the toolchain SDK root; network is always
    // denied. The requested read/write paths must fit inside the scopes
    // before any worker spawns.
    let permission = sandbox::derive_permission_set(&workspace, &toolchain);
    let artifact_out = sandbox::workspace_output_path(&workspace);
    if !sandbox::validate_job_paths(
        &permission,
        workspace.source_path.as_bytes(),
        artifact_out.as_bytes(),
    ) {
        reply.words[0] = DeveloperStatus::Denied as u32 as u64;
        let _ = emit_log(
            log_handle,
            LogSeverity::Warn,
            LogEvent::DeveloperBuildFailed,
            workspace_id as u64,
            sandbox::BUILDER_STATUS_SANDBOX_DENIED,
        );
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(reply_handle);
        let _ = rt::handle_close(output_handle);
        return Ok(());
    }
    let decision = sandbox::decision_for(
        &permission,
        workspace.source_path.as_bytes(),
        artifact_out.as_bytes(),
    );

    // Execution-mode selection: a routed job runs inside the runtime env's
    // exec contract with the permission set intersected against the env's
    // capability grants; every refusal falls back to the direct spawn.
    let mut mode = routing::ExecutionMode::DirectSpawn;
    if let routing::BuildRoute::RuntimeEnv { env_id } = route {
        let snapshot = probed
            .as_ref()
            .and_then(|envs| envs.iter().find(|env| env.env_id == env_id).copied());
        let merged = sandbox::intersect_with_env(
            &permission,
            snapshot.map(|env| env.capabilities).unwrap_or(0),
        );
        let candidate = routing::select_execution_mode(route, snapshot, merged.scope_count);
        if candidate.routed() {
            match try_routed_exec(bootstrap, toolchain, env_id, output_handle) {
                RoutedOutcome::Started => {
                    // The transferred duplicate carries the log stream into
                    // the env; our own copy is closed here.
                    let _ = rt::handle_close(output_handle);
                    let job_id = match allocate_job(jobs) {
                        Ok(index) => index,
                        Err(_) => {
                            reply.words[0] = DeveloperStatus::Busy as u32 as u64;
                            let _ = rt::channel_send(reply_handle, &reply);
                            let _ = rt::handle_close(reply_handle);
                            return Ok(());
                        }
                    };
                    jobs[job_id] = JobSlot {
                        occupied: true,
                        workspace_id: workspace_id as u32,
                        target,
                        state: DeveloperJobState::Running,
                        format: toolchain.format,
                        artifact_name: workspace.artifact,
                        artifact_size: 0,
                        artifact_handle: rt::INVALID_HANDLE,
                        // The run is tracked by runtime-service; no local
                        // task handle exists to poll (documented gap).
                        task_handle: rt::INVALID_HANDLE,
                        report_handle: rt::INVALID_HANDLE,
                        sandbox: sandbox::decision_for(
                            &merged,
                            workspace.source_path.as_bytes(),
                            artifact_out.as_bytes(),
                        ),
                        route,
                        mode: candidate,
                        export: ExportState::Local,
                    };
                    emit_log(
                        log_handle,
                        LogSeverity::Info,
                        LogEvent::DeveloperBuildStarted,
                        job_id as u64,
                        ((workspace_id as u64) << 32)
                            | ((candidate.status_word() & 0xFFFF_FFFF) << 8)
                            | target as u32 as u64,
                    )
                    .ok();
                    reply.words[0] = DeveloperStatus::Ok as u32 as u64;
                    reply.words[1] = job_id as u64;
                    let _ = rt::channel_send(reply_handle, &reply);
                    let _ = rt::handle_close(reply_handle);
                    return Ok(());
                }
                RoutedOutcome::Refused(status) => {
                    mode = routing::ExecutionMode::RoutedFallback {
                        env_id,
                        reason: routing::FALLBACK_EXEC_REFUSED,
                    };
                    let _ = status;
                }
                RoutedOutcome::TransportClosed => {
                    mode = routing::ExecutionMode::RoutedFallback {
                        env_id,
                        reason: routing::FALLBACK_EXEC_REFUSED,
                    };
                }
            }
        } else {
            mode = candidate;
        }
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

    let mut sandbox_text = [0u8; sandbox::SANDBOX_TEXT_MAX];
    let sandbox_memory = match sandbox::serialize_permission_text(
        &permission,
        workspace.source_path.as_bytes(),
        artifact_out.as_bytes(),
        &mut sandbox_text,
    )
    .and_then(|len| create_memory_from_bytes(&sandbox_text[..len]))
    {
        Ok(memory) => memory,
        Err(_) => {
            let _ = rt::handle_close(source_memory);
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
        rt::StartupHandle {
            handle: sandbox_memory,
            rights: rt::rights::READ | rt::rights::DUPLICATE | rt::rights::TRANSFER,
        },
    ];
    let mut startup_words = [0u64; rt::IPC_MAX_WORDS];
    startup_words[0] = target as u32 as u64;
    startup_words[1] = source_len as u64;
    startup_words[2] = workspace.artifact.len as u64;
    let packed =
        rt::pack_bytes(workspace.artifact.as_bytes(), &mut startup_words[3..]).unwrap_or(0);
    // Route word appended after the packed artifact name so older worker
    // images that read exactly name_len bytes are unaffected. A routed job
    // that fell back encodes DirectSpawn so the worker echo states how it
    // actually ran.
    let worker_route = if mode.routed() {
        route
    } else {
        routing::BuildRoute::DirectSpawn
    };
    startup_words[3 + packed as usize] = routing::encode_route_word(worker_route);
    let task_handle = rt::manager_launch_program_with_payload(
        bootstrap,
        rt::ServiceImageId::CrossBuilderTool,
        &startup_words[..4 + packed as usize],
        &startup_handles,
    );
    let _ = rt::handle_close(output_handle);
    let _ = rt::handle_close(report.second);
    let _ = rt::handle_close(source_memory);
    let _ = rt::handle_close(sandbox_memory);

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
                sandbox: decision,
                route,
                mode,
                export: ExportState::Local,
            };
            reply.words[0] = DeveloperStatus::Ok as u32 as u64;
            reply.words[1] = job_id as u64;
            let route_bits = (routing::encode_route_word(route).min(0xFF)) << 16;
            let _ = emit_log(
                log_handle,
                LogSeverity::Info,
                LogEvent::DeveloperBuildStarted,
                job_id as u64,
                ((workspace_id as u64) << 32)
                    | route_bits
                    | ((mode.status_word() & 0xF) << 24)
                    | target as u32 as u64,
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

/// Build requests against a remote-only target resolve through the farm
/// registry instead of failing obscurely: unconfigured endpoints are an
/// explicit refusal, configured endpoints without network transport report
/// unreachable, and registered targets queue a job whose artifact refs stay
/// exported-pending at the endpoint until a farm fetches them.
#[allow(clippy::too_many_arguments)]
fn reply_remote_target_build(
    bootstrap: rt::Handle,
    log_handle: rt::Handle,
    catalog: Catalog<'_>,
    jobs: &mut [JobSlot; MAX_JOBS],
    workspace_id: usize,
    target: DeveloperTarget,
    workspace: &WorkspaceSlot,
    toolchain: &ToolchainSlot,
    message: &RawMessage,
) -> rt::Result<()> {
    let Some(reply_handle) = message.handles.first().copied() else {
        return Ok(());
    };
    if let Some(output_handle) = message.handles.get(1).copied() {
        let _ = rt::handle_close(output_handle);
    }
    let mut reply = RawMessage::empty(DeveloperTag::BuildReply as u32);
    reply.word_count = 3;
    reply.words[0] = DeveloperStatus::Unsupported as u32 as u64;
    reply.words[1] = u32::MAX as u64;

    let farm_records = farm::build_farm(catalog.toolchains, catalog.toolchain_count);
    let record = farm::endpoint_for_target(&farm_records, target);
    let endpoint_bytes: &[u8] = record
        .map(|(_, record)| record.endpoint.as_bytes())
        .unwrap_or(&[]);
    let transport = !endpoint_bytes.is_empty() && farm::probe_transport(bootstrap);

    match farm::dispatch_outcome(endpoint_bytes, transport) {
        farm::DispatchOutcome::NotConfigured => {
            reply.words[2] = farm::FARM_STATUS_NOT_CONFIGURED;
            let _ = emit_log(
                log_handle,
                LogSeverity::Warn,
                LogEvent::DeveloperBuildFailed,
                workspace_id as u64,
                ((target as u64) << 32) | (farm::FARM_STATUS_NOT_CONFIGURED << 4),
            );
        }
        farm::DispatchOutcome::Unreachable => {
            reply.words[2] = farm::FARM_STATUS_UNREACHABLE;
            let capped = fit_bytes(endpoint_bytes, rt::IPC_MAX_WORDS - 4);
            reply.word_count = 4;
            reply.words[3] = capped.len() as u64;
            reply.word_count += rt::pack_bytes(capped, &mut reply.words[4..])?;
            let _ = emit_log(
                log_handle,
                LogSeverity::Warn,
                LogEvent::DeveloperBuildFailed,
                workspace_id as u64,
                ((target as u64) << 32) | (farm::FARM_STATUS_UNREACHABLE << 4),
            );
        }
        farm::DispatchOutcome::Registered => {
            let job_id = match allocate_job(jobs) {
                Ok(index) => index,
                Err(_) => {
                    reply.words[0] = DeveloperStatus::Busy as u32 as u64;
                    reply.word_count = 2;
                    let _ = rt::channel_send(reply_handle, &reply);
                    let _ = rt::handle_close(reply_handle);
                    return Ok(());
                }
            };
            let endpoint_id = record.map(|(index, _)| index).unwrap_or(0);
            let mut export_endpoint = FixedBytes::<{ crate::consts::MAX_PATH }>::empty();
            let _ = export_endpoint.set(endpoint_bytes);
            jobs[job_id] = JobSlot {
                occupied: true,
                workspace_id: workspace_id as u32,
                target,
                state: DeveloperJobState::Queued,
                format: toolchain.format,
                artifact_name: workspace.artifact,
                artifact_size: 0,
                artifact_handle: rt::INVALID_HANDLE,
                task_handle: rt::INVALID_HANDLE,
                report_handle: rt::INVALID_HANDLE,
                sandbox: sandbox::SandboxDecision {
                    allowed: false,
                    scope_count: 0,
                },
                route: routing::BuildRoute::RemoteFarm {
                    endpoint_id: endpoint_id as u32,
                },
                mode: routing::ExecutionMode::DirectSpawn,
                export: ExportState::PendingRemote {
                    endpoint: export_endpoint,
                },
            };
            reply.words[0] = DeveloperStatus::Ok as u32 as u64;
            reply.words[1] = job_id as u64;
            reply.words[2] = farm::FARM_STATUS_REGISTERED;
            reply.word_count = 5;
            reply.words[3] = pack_phase(
                DeveloperJobState::Queued as u32,
                routing::ROUTE_KIND_REMOTE_FARM,
                EXPORT_STATE_PENDING,
            );
            reply.words[4] = endpoint_bytes.len() as u64;
            let capped = fit_bytes(endpoint_bytes, rt::IPC_MAX_WORDS - 5);
            reply.word_count += rt::pack_bytes(capped, &mut reply.words[5..])?;
            let _ = emit_log(
                log_handle,
                LogSeverity::Info,
                LogEvent::DeveloperBuildStarted,
                job_id as u64,
                ((workspace_id as u64) << 32)
                    | (routing::ROUTE_KIND_REMOTE_FARM << 16)
                    | target as u32 as u64,
            );
        }
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn reply_job_list(
    workspaces: &[WorkspaceSlot],
    workspace_count: usize,
    jobs: &[JobSlot; MAX_JOBS],
    message: &RawMessage,
) -> rt::Result<()> {
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
        // IDE tail (additive): [magic|field_count][phase][toolchain][flags]
        // phase = state | route_kind<<8 | export_state<<16; toolchain is the
        // resolved descriptor index or u32::MAX; flags bit0 has artifact
        // handle, bits 8.. name length.
        if ide_tail_fits(reply.word_count, 3) {
            let base = reply.word_count as usize;
            reply.word_count += 4;
            reply.words[base] = ide_tail_magic(3);
            reply.words[base + 1] = pack_job_phase(&job);
            reply.words[base + 2] = job_toolchain_index(workspaces, workspace_count, &job) as u64;
            reply.words[base + 3] = (job.artifact_handle != rt::INVALID_HANDLE) as u64
                | ((job.artifact_name.len as u64) << 8);
        }
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn reply_job_info(
    workspaces: &[WorkspaceSlot],
    workspace_count: usize,
    jobs: &[JobSlot; MAX_JOBS],
    message: &RawMessage,
) -> rt::Result<()> {
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
        // IDE tail (additive, only when the name leaves room):
        // [magic|5][phase][toolchain][flags][farm][exec-mode]; farm =
        // export_state | endpoint_len<<8 | farm_status<<16 (registered
        // endpoints report FARM_STATUS_REGISTERED while a remote fetch is
        // pending); exec-mode distinguishes direct spawn vs routed runtime
        // environment vs routed-then-fallback (routing::ExecutionMode).
        if ide_tail_fits(reply.word_count, 5) {
            let base = reply.word_count as usize;
            reply.word_count += 6;
            reply.words[base] = ide_tail_magic(5);
            reply.words[base + 1] = pack_job_phase(&job);
            reply.words[base + 2] = job_toolchain_index(workspaces, workspace_count, &job) as u64;
            reply.words[base + 3] = (job.artifact_handle != rt::INVALID_HANDLE) as u64
                | ((job.artifact_name.len as u64) << 8);
            let farm_status = if job.exported_pending() {
                farm::FARM_STATUS_REGISTERED
            } else {
                farm::FARM_STATUS_NOT_CONFIGURED
            };
            reply.words[base + 4] = export_state_code(&job)
                | ((job.endpoint_bytes().len() as u64) << 8)
                | (farm_status << 16);
            reply.words[base + 5] = job.mode.status_word();
        }
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

/// Machine-readable IDE/editor job snapshot (local tag 0xd20 -> 0xd21).
/// Reply shape: w0 status, w1 job_id, w2 phase(state|route<<8|export<<16),
/// w3 workspace_id|toolchain<<32, w4 artifact_size|format<<32|has_handle<<40,
/// w5 farm(status|endpoint_len<<8), w6 name_len, then packed [name][endpoint]
/// truncated to fit. Editors can poll this single message for everything.
fn reply_ide_job_info(
    workspaces: &[WorkspaceSlot],
    workspace_count: usize,
    jobs: &[JobSlot; MAX_JOBS],
    message: &RawMessage,
) -> rt::Result<()> {
    if message.handle_count < 1 || message.word_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let mut reply = RawMessage::empty(IDE_JOB_INFO_REPLY_TAG);
    reply.word_count = 7;
    reply.words[0] = DeveloperStatus::NotFound as u32 as u64;
    let index = message.words[0] as usize;
    if let Some(job) = jobs.get(index).copied().filter(|job| job.occupied) {
        let toolchain = job_toolchain_index(workspaces, workspace_count, &job);
        let endpoint_bytes = job.endpoint_bytes();
        let farm_status = if job.exported_pending() {
            farm::FARM_STATUS_REGISTERED
        } else {
            farm::FARM_STATUS_NOT_CONFIGURED
        };
        let name_cap = (rt::IPC_MAX_WORDS - 7) * 8;
        let name_len = job.artifact_name.len.min(name_cap);
        let endpoint_fit = &endpoint_bytes[..endpoint_bytes.len().min(name_cap - name_len)];
        let name_fit = &job.artifact_name.as_bytes()[..name_len];
        let mut combined = [0u8; (rt::IPC_MAX_WORDS - 7) * 8];
        combined[..name_fit.len()].copy_from_slice(name_fit);
        combined[name_fit.len()..name_fit.len() + endpoint_fit.len()].copy_from_slice(endpoint_fit);
        let total = name_fit.len() + endpoint_fit.len();
        reply.words[0] = DeveloperStatus::Ok as u32 as u64;
        reply.words[1] = index as u64;
        reply.words[2] = pack_job_phase(&job);
        reply.words[3] = job.workspace_id as u64 | ((toolchain as u64) << 32);
        reply.words[4] = job.artifact_size as u64
            | ((job.format as u32 as u64) << 32)
            | (((job.artifact_handle != rt::INVALID_HANDLE) as u64) << 40);
        reply.words[5] = farm_status | ((endpoint_fit.len() as u64) << 8);
        reply.words[6] = name_fit.len() as u64;
        reply.word_count += rt::pack_bytes(&combined[..total], &mut reply.words[7..])?;
        // Trailing exec-mode word (additive): 0 direct, 1 routed env,
        // 2 routed-then-fallback with reason bits.
        if ide_tail_fits(reply.word_count, 1) {
            let base = reply.word_count as usize;
            reply.word_count += 1;
            reply.words[base] = job.mode.status_word();
        }
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
    let index = message.words[0] as usize;
    let mut reply = RawMessage::empty(DeveloperTag::ArtifactOpenReply as u32);
    reply.word_count = 4;
    reply.words[0] = DeveloperStatus::NotFound as u32 as u64;
    // Exported-pending jobs own no local artifact handle: the output lives
    // at the remote farm endpoint, so the reply carries the pending marker
    // plus the endpoint reference instead of a bare NotFound.
    if let Some(job) = jobs
        .get(index)
        .copied()
        .filter(|job| job.exported_pending())
    {
        let endpoint_bytes = job.endpoint_bytes();
        let capped = fit_bytes(endpoint_bytes, rt::IPC_MAX_WORDS - 7);
        reply.word_count = 7;
        reply.words[0] = DeveloperStatus::NotFound as u32 as u64;
        reply.words[4] = ide_tail_magic(2);
        reply.words[5] = EXPORT_STATE_PENDING
            | ((capped.len() as u64) << 8)
            | (farm::FARM_STATUS_REGISTERED << 16);
        reply.words[6] = pack_job_phase(&job);
        reply.word_count += rt::pack_bytes(capped, &mut reply.words[7..])?;
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(reply_handle);
        return Ok(());
    }
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
                } else if result == sandbox::BUILDER_STATUS_SANDBOX_DENIED as u32 {
                    job.state = DeveloperJobState::Failed;
                    let _ = emit_log(
                        log_handle,
                        LogSeverity::Warn,
                        LogEvent::DeveloperBuildFailed,
                        job_id as u64,
                        ((u64::from(job.sandbox.allowed)) << 32)
                            | ((job.sandbox.scope_count.min(0xFF) as u64) << 8)
                            | sandbox::BUILDER_STATUS_SANDBOX_DENIED,
                    );
                } else {
                    job.state = DeveloperJobState::Failed;
                    let route_bits = match job.route {
                        routing::BuildRoute::DirectSpawn => 0u64,
                        routing::BuildRoute::RuntimeEnv { env_id } => {
                            (u64::from(env_id).min(0xFF)) << 8
                        }
                        routing::BuildRoute::RemoteFarm { endpoint_id } => {
                            (routing::ROUTE_KIND_REMOTE_FARM << 8)
                                | (u64::from(endpoint_id).min(0xFF) << 16)
                        }
                    };
                    let _ = emit_log(
                        log_handle,
                        LogSeverity::Error,
                        LogEvent::DeveloperBuildFailed,
                        job_id as u64,
                        2 | route_bits,
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

/// Export-state codes for the IDE tails: 0 local artifact, 1 exported-pending
/// at a remote farm endpoint.
pub(crate) const EXPORT_STATE_LOCAL: u64 = 0;
pub(crate) const EXPORT_STATE_PENDING: u64 = 1;

/// Magic word opening an additive IDE tail: "IDE1" with the field count in
/// bits 32.. so editors can validate and skip the block without out-of-band
/// knowledge.
pub(crate) const IDE_TAIL_MAGIC: u64 = 0x4944_4531;

pub(crate) fn ide_tail_magic(field_count: usize) -> u64 {
    IDE_TAIL_MAGIC | (((field_count as u64) & 0xFF) << 32)
}

/// Guest-exec workload marker used by runtime-service's raw-image spawn
/// path (private constant there; mirrored here so the exec contract can be
/// driven without a shared ABI edit).
const EXEC_GUEST_WORKLOAD: u32 = 5;

/// Result of a routed launch attempt through the runtime env's exec
/// contract.
enum RoutedOutcome {
    /// Runtime-service accepted the run; it now owns the workload.
    Started,
    /// The contract answered with a non-Ok status (carried for logs).
    Refused(u64),
    /// Unavailable service, malformed reply, or transport failure.
    TransportClosed,
}

/// Launch the toolchain's routed entry through runtime-service: a
/// `RunLaunchRequest` with the guest-exec marker asks the env to stage and
/// run the builder payload image inside the environment, where artifacts
/// are written to the job-scoped output directory via the env mounts.
fn try_routed_exec(
    bootstrap: rt::Handle,
    toolchain: ToolchainSlot,
    env_id: u32,
    output_handle: rt::Handle,
) -> RoutedOutcome {
    let arg = routed_exec_arg(toolchain);
    if arg.len == 0 {
        // Nothing resolvable to stage inside the env: refuse without
        // touching the caller's output handle (the direct fallback needs
        // it).
        return RoutedOutcome::TransportClosed;
    }
    let Ok(runtime_handle) = rt::lookup_service(bootstrap, rt::ServiceId::Runtime) else {
        return RoutedOutcome::TransportClosed;
    };
    let pair = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => {
            let _ = rt::handle_close(runtime_handle);
            return RoutedOutcome::TransportClosed;
        }
    };
    let transferred_output = match rt::handle_duplicate(
        output_handle,
        rt::rights::SEND | rt::rights::DUPLICATE | rt::rights::TRANSFER,
    ) {
        Ok(handle) => handle,
        Err(_) => {
            let _ = rt::handle_close(pair.first);
            let _ = rt::handle_close(pair.second);
            let _ = rt::handle_close(runtime_handle);
            return RoutedOutcome::TransportClosed;
        }
    };

    let mut request = RawMessage::empty(RuntimeTag::RunLaunchRequest as u32);
    request.word_count = 3 + rt::pack_bytes(arg.as_bytes(), &mut request.words[3..]).unwrap_or(0);
    request.words[0] = u64::from(env_id);
    request.words[1] = EXEC_GUEST_WORKLOAD as u64;
    request.words[2] = arg.len as u64;
    request.handle_count = 2;
    request.handles[0] = pair.second;
    request.handle_rights[0] = rt::rights::SEND;
    request.handles[1] = transferred_output;
    request.handle_rights[1] = rt::rights::SEND | rt::rights::DUPLICATE | rt::rights::TRANSFER;

    let outcome = match rt::channel_send(runtime_handle, &request) {
        Ok(()) => {
            let _ = rt::handle_close(transferred_output);
            let mut response = RawMessage::empty(0);
            match rt::channel_receive_blocking(pair.first, &mut response) {
                Ok(())
                    if response.tag == RuntimeTag::RunLaunchReply as u32
                        && response.word_count >= 1 =>
                {
                    if response.words[0] == RuntimeStatus::Ok as u32 as u64 {
                        RoutedOutcome::Started
                    } else {
                        RoutedOutcome::Refused(response.words[0])
                    }
                }
                Ok(()) => RoutedOutcome::TransportClosed,
                Err(_) => RoutedOutcome::TransportClosed,
            }
        }
        Err(_) => {
            let _ = rt::handle_close(transferred_output);
            RoutedOutcome::TransportClosed
        }
    };
    let _ = rt::handle_close(pair.first);
    let _ = rt::handle_close(runtime_handle);
    outcome
}

/// Storage path handed to the env's guest-exec staging: the first declared
/// storage-path payload (the packaged SDK content), else the bare SDK root.
fn routed_exec_arg(toolchain: ToolchainSlot) -> FixedBytes<{ crate::consts::MAX_PATH }> {
    for payload in toolchain.payloads[..toolchain.payload_count].iter() {
        if let crate::payload::PayloadRef::StoragePath(path) = payload.reference {
            return path;
        }
    }
    toolchain.sdk_root
}

pub(crate) fn ide_tail_fits(word_count: u32, fields: usize) -> bool {
    word_count as usize + fields + 1 <= rt::IPC_MAX_WORDS
}

fn export_state_code(job: &JobSlot) -> u64 {
    if job.exported_pending() {
        EXPORT_STATE_PENDING
    } else {
        EXPORT_STATE_LOCAL
    }
}

fn pack_job_phase(job: &JobSlot) -> u64 {
    pack_phase(
        job.state as u32,
        routing::route_kind(job.route),
        export_state_code(job),
    )
}

pub(crate) fn pack_phase(state: u32, route_kind: u64, export_state: u64) -> u64 {
    u64::from(state) | (route_kind << 8) | (export_state << 16)
}

pub(crate) fn job_toolchain_index(
    workspaces: &[WorkspaceSlot],
    workspace_count: usize,
    job: &JobSlot,
) -> u32 {
    workspaces[..workspace_count]
        .get(job.workspace_id as usize)
        .filter(|workspace| workspace.occupied)
        .map(|workspace| workspace.toolchains[target_slot_index(job.target)])
        .unwrap_or(u32::MAX)
}

pub(crate) fn fit_bytes<'a>(source: &'a [u8], cap_words: usize) -> &'a [u8] {
    &source[..source.len().min(cap_words * 8)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::MAX_PATH;

    #[test]
    fn ide_magic_embeds_field_count() {
        assert_eq!(ide_tail_magic(3), 0x4944_4531 | (3 << 32));
        assert_eq!(ide_tail_magic(0), IDE_TAIL_MAGIC);
    }

    #[test]
    fn ide_tail_fits_respects_ipc_word_limit() {
        assert!(ide_tail_fits(7, 3));
        assert!(ide_tail_fits(11, 4));
        assert!(!ide_tail_fits(12, 5));
        assert!(!ide_tail_fits(16, 0));
    }

    #[test]
    fn phase_packs_state_route_export() {
        let phase = pack_phase(
            DeveloperJobState::Queued as u32,
            routing::ROUTE_KIND_REMOTE_FARM,
            EXPORT_STATE_PENDING,
        );
        assert_eq!(phase & 0xFF, DeveloperJobState::Queued as u64);
        assert_eq!((phase >> 8) & 0xFF, routing::ROUTE_KIND_REMOTE_FARM);
        assert_eq!((phase >> 16) & 0xFF, EXPORT_STATE_PENDING);
    }

    #[test]
    fn toolchain_index_resolves_through_workspace() {
        let mut workspace = WorkspaceSlot::empty();
        workspace.occupied = true;
        workspace.toolchains[3] = 5;
        let mut job = JobSlot::empty();
        job.workspace_id = 0;
        job.target = DeveloperTarget::MacosX64;
        assert_eq!(job_toolchain_index(&[workspace], 1, &job), 5);
        assert_eq!(
            job_toolchain_index(&[WorkspaceSlot::empty()], 1, &job),
            u32::MAX
        );
        assert_eq!(job_toolchain_index(&[workspace], 0, &job), u32::MAX);
    }

    #[test]
    fn fit_bytes_caps_at_word_boundary() {
        let data = [7u8; 20];
        assert_eq!(fit_bytes(&data, 2).len(), 16);
        assert_eq!(fit_bytes(&data, 4).len(), 20);
        assert_eq!(fit_bytes(&data, 0).len(), 0);
    }

    #[test]
    fn export_codes_distinguish_local_and_pending() {
        let mut job = JobSlot::empty();
        assert_eq!(export_state_code(&job), EXPORT_STATE_LOCAL);
        let mut endpoint = FixedBytes::<MAX_PATH>::empty();
        let _ = endpoint.set(b"farm@host");
        job.export = ExportState::PendingRemote { endpoint };
        assert_eq!(export_state_code(&job), EXPORT_STATE_PENDING);
        assert!(job.exported_pending());
        assert_eq!(job.endpoint_bytes(), b"farm@host");
    }
}

use rt::{
    LogEvent, LogSeverity, RawMessage, RuntimeRunState, RuntimeStatus, RuntimeTag,
    RuntimeWorkloadKind, TaskStateCode,
};
use serviceos_userspace_runtime as rt;

use crate::{
    abi_image::{EXEC_GUEST_WORKLOAD, ImageFormat, classify_image},
    consts::{MAX_ENVS, MAX_IMAGE_HEADER_BYTES, MAX_RUNS},
    types::{EnvSlot, FixedBytes, RunSlot},
    util::{emit_log, release_run_slot, resolve_guest_path},
};

pub(crate) fn poll_run_exits(
    log_handle: rt::Handle,
    envs: &mut [EnvSlot; MAX_ENVS],
    runs: &mut [RunSlot; MAX_RUNS],
) {
    for (run_id, run) in runs.iter_mut().enumerate() {
        if !run.occupied
            || run.state == RuntimeRunState::Exited
            || run.state == RuntimeRunState::Failed
        {
            continue;
        }
        let Ok(status) = rt::task_status(run.task_handle) else {
            continue;
        };
        if !matches!(status.state, TaskStateCode::Exited | TaskStateCode::Faulted) {
            continue;
        }
        run.exit_code = status.exit_code;
        run.state = if status.exit_code == 0 {
            RuntimeRunState::Exited
        } else {
            RuntimeRunState::Failed
        };
        if let Some(env) = envs.get_mut(run.env_id as usize)
            && env.occupied
            && env.active_runs > 0
        {
            env.active_runs -= 1;
        }
        if run.session_handle != rt::INVALID_HANDLE {
            let _ = rt::handle_close(run.session_handle);
            run.session_handle = rt::INVALID_HANDLE;
        }
        let _ = emit_log(
            log_handle,
            LogSeverity::Info,
            LogEvent::RuntimeLaunchExited,
            run_id as u64,
            status.exit_code,
        );
    }
}

fn allocate_run_slot(runs: &mut [RunSlot; MAX_RUNS]) -> rt::Result<usize> {
    if let Some(index) = (0..runs.len()).find(|index| {
        !runs[*index].occupied
            || matches!(
                runs[*index].state,
                RuntimeRunState::Exited | RuntimeRunState::Failed
            )
    }) {
        if runs[index].occupied {
            release_run_slot(&mut runs[index]);
        }
        return Ok(index);
    }
    Err(rt::Error::CapacityExceeded)
}

pub(crate) fn handle_run_launch_request(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    log_handle: rt::Handle,
    envs: &mut [EnvSlot; MAX_ENVS],
    runs: &mut [RunSlot; MAX_RUNS],
    message: &RawMessage,
) -> rt::Result<()> {
    if message.handle_count < 2 || message.word_count < 3 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let output_handle = message.handles[1];
    let env_id = message.words[0] as usize;
    let workload_word = message.words[1] as u32;
    let workload = match workload_word {
        x if x == RuntimeWorkloadKind::Inspect as u32 => RuntimeWorkloadKind::Inspect,
        x if x == RuntimeWorkloadKind::Env as u32 => RuntimeWorkloadKind::Env,
        x if x == RuntimeWorkloadKind::Mounts as u32 => RuntimeWorkloadKind::Mounts,
        x if x == RuntimeWorkloadKind::Cat as u32 => RuntimeWorkloadKind::Cat,
        _ => RuntimeWorkloadKind::Inspect,
    };
    let arg_len = message.words[2] as usize;
    let mut arg_bytes = [0u8; (rt::IPC_MAX_WORDS - 3) * 8];
    let _ = rt::unpack_bytes(
        &message.words[3..message.word_count as usize],
        arg_len,
        &mut arg_bytes,
    );

    // Optional per-workload sandbox manifest riding the launch envelope as
    // additive trailing words. Decoded once here; each launch path validates
    // and gates on it after the environment lookup (malformed or widening
    // manifests refuse with `Unsupported` — never a silent fallback).
    let manifest = crate::sandbox::SandboxManifest::from_launch_words(
        &message.words,
        arg_len,
        message.word_count as usize,
    );

    // Guest-image execution: the runtime-service-local exec marker routes
    // the launch through the raw-image spawn path (flat v2 / flat v1 /
    // static ELF64) instead of the hosted posix tool image.
    if workload_word == EXEC_GUEST_WORKLOAD {
        return handle_guest_exec_launch(
            bootstrap,
            storage_handle,
            log_handle,
            envs,
            runs,
            reply_handle,
            output_handle,
            env_id,
            &arg_bytes[..arg_len.min(arg_bytes.len())],
            manifest,
        );
    }

    let mut reply = RawMessage::empty(RuntimeTag::RunLaunchReply as u32);
    reply.word_count = 2;

    let Some(env) = envs.get_mut(env_id).filter(|env| env.occupied) else {
        reply.words[0] = RuntimeStatus::NotFound as u32 as u64;
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(reply_handle);
        let _ = rt::handle_close(output_handle);
        return Ok(());
    };
    // S11 enforcement point: the sandbox capability matrix decides whether
    // this environment may host a workload right now. Pending requested-but-
    // ungranted device classes (network/graphics/input/audio) and explicit
    // denials both refuse launch through the existing status contract. A
    // per-workload manifest narrows the effective profile for this launch
    // (narrow-only, validated); manifest refusals use `Unsupported`.
    let manifest = match manifest {
        Ok(manifest) => manifest,
        Err(_) => {
            reply.words[0] = RuntimeStatus::Unsupported as u32 as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
            let _ = rt::handle_close(output_handle);
            return Ok(());
        }
    };
    match crate::sandbox::gate_launch(env, manifest, false) {
        crate::sandbox::GateOutcome::Refuse(status) => {
            reply.words[0] = status as u32 as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
            let _ = rt::handle_close(output_handle);
            return Ok(());
        }
        crate::sandbox::GateOutcome::Proceed(crate::sandbox::LaunchDecision::PendingApproval) => {
            reply.words[0] = RuntimeStatus::PendingApproval as u32 as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
            let _ = rt::handle_close(output_handle);
            return Ok(());
        }
        crate::sandbox::GateOutcome::Proceed(crate::sandbox::LaunchDecision::Denied) => {
            reply.words[0] = RuntimeStatus::Denied as u32 as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
            let _ = rt::handle_close(output_handle);
            return Ok(());
        }
        crate::sandbox::GateOutcome::Proceed(crate::sandbox::LaunchDecision::Allowed) => {}
    }

    let slot_index = match allocate_run_slot(runs) {
        Ok(index) => index,
        Err(_) => {
            reply.words[0] = RuntimeStatus::Busy as u32 as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
            let _ = rt::handle_close(output_handle);
            return Ok(());
        }
    };

    let pair = rt::channel_create()?;
    let startup_handles = [
        rt::StartupHandle {
            handle: output_handle,
            rights: rt::rights::SEND | rt::rights::DUPLICATE | rt::rights::TRANSFER,
        },
        rt::StartupHandle {
            handle: pair.second,
            rights: rt::rights::SEND
                | rt::rights::RECEIVE
                | rt::rights::DUPLICATE
                | rt::rights::TRANSFER,
        },
    ];
    let mut startup_words = [0u64; rt::IPC_MAX_WORDS];
    startup_words[0] = workload as u32 as u64;
    startup_words[1] = arg_len as u64;
    let packed = rt::pack_bytes(&arg_bytes[..arg_len], &mut startup_words[2..])?;
    let task_handle = rt::manager_launch_program_with_payload(
        bootstrap,
        rt::ServiceImageId::PosixHostTool,
        &startup_words[..2 + packed as usize],
        &startup_handles,
    );
    let _ = rt::handle_close(output_handle);
    let _ = rt::handle_close(pair.second);

    match task_handle {
        Ok(task_handle) => {
            runs[slot_index] = RunSlot {
                occupied: true,
                env_id: env_id as u32,
                workload,
                guest_exec: false,
                state: RuntimeRunState::Running,
                task_handle,
                session_handle: pair.first,
                exit_code: 0,
            };
            env.active_runs = env.active_runs.saturating_add(1);
            env.updated_tick = crate::util::now_tick();
            reply.words[0] = RuntimeStatus::Ok as u32 as u64;
            reply.words[1] = slot_index as u64;
            let _ = emit_log(
                log_handle,
                LogSeverity::Info,
                LogEvent::RuntimeLaunchStarted,
                slot_index as u64,
                ((env_id as u64) << 32) | workload as u32 as u64,
            );
        }
        Err(error) => {
            let _ = rt::handle_close(pair.first);
            reply.words[0] = match error {
                rt::Error::PermissionDenied => RuntimeStatus::Denied as u32 as u64,
                rt::Error::NotFound => RuntimeStatus::NotFound as u32 as u64,
                _ => RuntimeStatus::Busy as u32 as u64,
            };
        }
    }

    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn exec_reply_status(reply_handle: rt::Handle, output_handle: rt::Handle, status: RuntimeStatus) {
    let mut reply = RawMessage::empty(RuntimeTag::RunLaunchReply as u32);
    reply.word_count = 2;
    reply.words[0] = status as u32 as u64;
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    if output_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(output_handle);
    }
}

/// Launch a guest image through the raw-image spawn path.
///
/// The arg is a guest path resolved through the environment's mounts; the
/// image is staged into a memory object and classified with the same
/// fallback order the kernel loader applies (flat v2 → flat v1 → static
/// ELF64). Declared bundled libraries are validated as flat dependency
/// images before launch. The kernel remains the mapping authority: it
/// enforces user-window containment and per-segment W^X/NX policy for
/// whatever bytes reach `TaskSpawnImage`.
#[allow(clippy::too_many_arguments)]
fn handle_guest_exec_launch(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    log_handle: rt::Handle,
    envs: &mut [EnvSlot; MAX_ENVS],
    runs: &mut [RunSlot; MAX_RUNS],
    reply_handle: rt::Handle,
    output_handle: rt::Handle,
    env_id: usize,
    guest_path: &[u8],
    manifest: Result<Option<crate::sandbox::SandboxManifest>, crate::sandbox::ManifestError>,
) -> rt::Result<()> {
    let Some(env) = envs.get_mut(env_id).filter(|env| env.occupied) else {
        exec_reply_status(reply_handle, output_handle, RuntimeStatus::NotFound);
        return Ok(());
    };
    // Same sandbox gate as hosted workloads — guest images get no bypass.
    // A latched manifest must accompany guest-image launches here.
    let manifest = match manifest {
        Ok(manifest) => manifest,
        Err(_) => {
            exec_reply_status(reply_handle, output_handle, RuntimeStatus::Unsupported);
            return Ok(());
        }
    };
    match crate::sandbox::gate_launch(env, manifest, true) {
        crate::sandbox::GateOutcome::Refuse(status) => {
            exec_reply_status(reply_handle, output_handle, status);
            return Ok(());
        }
        crate::sandbox::GateOutcome::Proceed(crate::sandbox::LaunchDecision::PendingApproval) => {
            exec_reply_status(reply_handle, output_handle, RuntimeStatus::PendingApproval);
            return Ok(());
        }
        crate::sandbox::GateOutcome::Proceed(crate::sandbox::LaunchDecision::Denied) => {
            exec_reply_status(reply_handle, output_handle, RuntimeStatus::Denied);
            return Ok(());
        }
        crate::sandbox::GateOutcome::Proceed(crate::sandbox::LaunchDecision::Allowed) => {}
    }

    // Validate every declared bundled library up front: it must resolve
    // through the mounts and stage as a flat image (the only dependency
    // format the loader maps).
    for lib_index in 0..env.lib_count {
        let guest = env.libs[lib_index].guest.as_bytes();
        let mut resolved = FixedBytes::<{ crate::consts::MAX_STORAGE_PATH }>::empty();
        if resolve_guest_path(env, guest, &mut resolved).is_err() {
            exec_reply_status(reply_handle, output_handle, RuntimeStatus::InvalidPath);
            return Ok(());
        }
        let Ok(path) = core::str::from_utf8(resolved.as_bytes()) else {
            exec_reply_status(reply_handle, output_handle, RuntimeStatus::InvalidPath);
            return Ok(());
        };
        match stage_image_header(storage_handle, path) {
            Ok((blob_handle, header)) => {
                let flat = matches!(
                    classify_image(&header),
                    Ok(ImageFormat::FlatV2) | Ok(ImageFormat::FlatV1)
                );
                let _ = rt::storage_blob_close(blob_handle);
                if !flat {
                    exec_reply_status(reply_handle, output_handle, RuntimeStatus::Unsupported);
                    return Ok(());
                }
            }
            Err(_) => {
                exec_reply_status(reply_handle, output_handle, RuntimeStatus::Unsupported);
                return Ok(());
            }
        }
    }

    let slot_index = match allocate_run_slot(runs) {
        Ok(index) => index,
        Err(_) => {
            exec_reply_status(reply_handle, output_handle, RuntimeStatus::Busy);
            return Ok(());
        }
    };

    // Resolve and classify the executable itself.
    let mut resolved = FixedBytes::<{ crate::consts::MAX_STORAGE_PATH }>::empty();
    if resolve_guest_path(env, guest_path, &mut resolved).is_err() {
        release_run_slot(&mut runs[slot_index]);
        exec_reply_status(reply_handle, output_handle, RuntimeStatus::InvalidPath);
        return Ok(());
    }
    let Ok(path) = core::str::from_utf8(resolved.as_bytes()) else {
        release_run_slot(&mut runs[slot_index]);
        exec_reply_status(reply_handle, output_handle, RuntimeStatus::InvalidPath);
        return Ok(());
    };

    let staged = match stage_image_header(storage_handle, path) {
        Ok((blob_handle, header)) => {
            let verdict = classify_image(&header);
            let _ = rt::storage_blob_close(blob_handle);
            verdict
        }
        Err(_) => Err(crate::abi_image::ImageParseError::Truncated),
    };
    let Ok(format) = staged else {
        release_run_slot(&mut runs[slot_index]);
        exec_reply_status(reply_handle, output_handle, RuntimeStatus::Unsupported);
        return Ok(());
    };

    // Stage the full image bytes into a memory object for the manager's
    // LaunchImage → TaskSpawnImage path (mirrors root-manager storage
    // staging).
    let image_handle = match stage_image_object(storage_handle, path) {
        Ok(handle) => handle,
        Err(_) => {
            release_run_slot(&mut runs[slot_index]);
            exec_reply_status(reply_handle, output_handle, RuntimeStatus::Unsupported);
            return Ok(());
        }
    };

    let pair = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => {
            let _ = rt::handle_close(image_handle);
            release_run_slot(&mut runs[slot_index]);
            exec_reply_status(reply_handle, output_handle, RuntimeStatus::Busy);
            return Ok(());
        }
    };
    let startup_handles = [
        rt::StartupHandle {
            handle: output_handle,
            rights: rt::rights::SEND | rt::rights::DUPLICATE | rt::rights::TRANSFER,
        },
        rt::StartupHandle {
            handle: pair.second,
            rights: rt::rights::SEND
                | rt::rights::RECEIVE
                | rt::rights::DUPLICATE
                | rt::rights::TRANSFER,
        },
    ];
    // Every guest-image launch carries the kernel-visible GUEST isolation
    // class plus the owning environment id in the additive extended
    // spawn-attributes word; the declared syscall-ABI bit rides alongside.
    // Non-guest launch paths never emit this word and stay byte-identical.
    let task_handle = rt::manager_launch_image_with_payload_abi(
        bootstrap,
        image_handle,
        &[EXEC_GUEST_WORKLOAD as u64, env_id as u64],
        &startup_handles,
        guest_exec_spawn_attrs(env.linux_syscall, env_id),
    );
    let _ = rt::handle_close(image_handle);
    let _ = rt::handle_close(pair.second);

    match task_handle {
        Ok(task_handle) => {
            runs[slot_index] = RunSlot {
                occupied: true,
                env_id: env_id as u32,
                workload: RuntimeWorkloadKind::Inspect,
                guest_exec: true,
                state: RuntimeRunState::Running,
                task_handle,
                session_handle: pair.first,
                exit_code: 0,
            };
            env.active_runs = env.active_runs.saturating_add(1);
            env.updated_tick = crate::util::now_tick();
            let mut reply = RawMessage::empty(RuntimeTag::RunLaunchReply as u32);
            reply.word_count = 2;
            reply.words[0] = RuntimeStatus::Ok as u32 as u64;
            reply.words[1] = slot_index as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
            let _ = emit_log(
                log_handle,
                LogSeverity::Info,
                LogEvent::RuntimeLaunchStarted,
                slot_index as u64,
                ((env_id as u64) << 48) | ((env.lib_count as u64) << 40) | (format.marker() as u64),
            );
        }
        Err(error) => {
            let _ = rt::handle_close(pair.first);
            release_run_slot(&mut runs[slot_index]);
            let status = match error {
                rt::Error::PermissionDenied => RuntimeStatus::Denied,
                rt::Error::NotFound => RuntimeStatus::NotFound,
                _ => RuntimeStatus::Busy,
            };
            exec_reply_status(reply_handle, rt::INVALID_HANDLE, status);
        }
    }
    Ok(())
}

/// Read the leading header bytes of a stored image for classification.
/// The blob handle is returned open so callers control its lifetime.
fn stage_image_header(
    storage_handle: rt::Handle,
    path: &str,
) -> rt::Result<(rt::Handle, [u8; MAX_IMAGE_HEADER_BYTES])> {
    let (blob_handle, _len) = rt::storage_open(storage_handle, path)?;
    let mut header = [0u8; MAX_IMAGE_HEADER_BYTES];
    let mut offset = 0usize;
    while offset < header.len() {
        let read = rt::storage_read(blob_handle, offset, &mut header[offset..])?;
        if read == 0 {
            break;
        }
        offset += read;
    }
    Ok((blob_handle, header))
}

/// Stage the full image bytes into a MAP-able memory object suitable for
/// the manager's `LaunchImageRequest` → `TaskSpawnImage` path.
fn stage_image_object(storage_handle: rt::Handle, path: &str) -> rt::Result<rt::Handle> {
    let (blob_handle, blob_len) = rt::storage_open(storage_handle, path)?;
    let staging = match rt::memory_create(blob_len, true) {
        Ok(handle) => handle,
        Err(error) => {
            let _ = rt::storage_blob_close(blob_handle);
            return Err(error);
        }
    };
    let mut chunk = [0u8; 96];
    let mut offset = 0usize;
    while offset < blob_len {
        let read = match rt::storage_read(blob_handle, offset, &mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => {
                let _ = rt::handle_close(staging);
                let _ = rt::storage_blob_close(blob_handle);
                return Err(error);
            }
        };
        if rt::memory_write(staging, offset, &chunk[..read]).is_err() {
            let _ = rt::handle_close(staging);
            let _ = rt::storage_blob_close(blob_handle);
            return Err(rt::Error::BufferTooSmall);
        }
        offset += read;
    }
    let _ = rt::storage_blob_close(blob_handle);
    Ok(staging)
}

/// The guest-exec launch isolation word: every guest-image launch carries
/// the kernel-visible GUEST isolation class plus the owning environment id,
/// with the declared syscall-ABI bit packed alongside. Unflagged non-guest
/// launch paths never touch this encoding and stay byte-identical.
pub(crate) fn guest_exec_spawn_attrs(linux_syscall: bool, env_id: usize) -> u64 {
    rt::task_spawn_attrs::encode(rt::task_spawn_attrs::SpawnAttrs {
        linux_abi: linux_syscall,
        isolation_guest: true,
        owner_env: Some(u16::try_from(env_id).unwrap_or(u16::MAX)),
    })
}

#[cfg(test)]
mod tests {
    use super::guest_exec_spawn_attrs;
    use rt::task_spawn_attrs;
    use serviceos_userspace_runtime as rt;

    #[test]
    fn guest_exec_attrs_always_carry_guest_class_and_env() {
        let word = guest_exec_spawn_attrs(false, 2);
        let attrs = task_spawn_attrs::decode_extended(word).expect("guest attrs word decodes");
        assert!(attrs.isolation_guest);
        assert_eq!(attrs.owner_env, Some(2));
        assert!(!attrs.linux_abi);

        let word = guest_exec_spawn_attrs(true, 5);
        let attrs = task_spawn_attrs::decode_extended(word).expect("guest attrs word decodes");
        assert!(attrs.isolation_guest);
        assert_eq!(attrs.owner_env, Some(5));
        assert!(attrs.linux_abi);
    }

    #[test]
    fn guest_exec_attrs_are_never_the_legacy_words() {
        // The combined word must never collide with the legacy ABI magic
        // words: extended encoding sets bit 63, legacy magics do not.
        assert!(task_spawn_attrs::is_legacy(
            rt::linux_abi::spawn_abi::LINUX_SYSCALL
        ));
        assert!(!task_spawn_attrs::is_legacy(guest_exec_spawn_attrs(
            false, 0
        )));
    }
}

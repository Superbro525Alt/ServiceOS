use alloc::vec::Vec;
use serviceos_abi::{Handle, TaskLoadedLibrary, TaskStateCode, TaskStatus as AbiTaskStatus};

use super::{
    super::{
        SyscallContext, SyscallError, SyscallReturn,
        resolve::{current_task, resolve_object},
    },
    common::{map_capability_error, map_spawn_error},
};
use crate::{
    capability::{CapabilityHandle, CapabilityResolver, CapabilityRights, TransferMode},
    memory::{MappingError, MappingFlags, VirtualAddress},
    task::TaskRole,
    user,
};

pub(crate) fn handle_service_spawn(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let authority = match resolve_object(
        &current_task,
        context.arguments[1] as Handle,
        CapabilityRights::bootstrap(),
    ) {
        Ok(authority) => authority,
        Err(error) => return SyscallReturn::error(error),
    };
    if authority.object.bootstrap_capability().is_none() {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    }

    let bootstrap_transfer = if context.arguments[2] == 0 {
        None
    } else {
        let handle = CapabilityHandle(context.arguments[2] as Handle);
        let Some(descriptor) = task.capability_space().resolve_descriptor(handle) else {
            return SyscallReturn::error(SyscallError::NotFound);
        };
        match task.capability_space().prepare_transfer(
            handle,
            descriptor.rights,
            TransferMode::Move,
        ) {
            Ok(transfer) => Some(transfer),
            Err(error) => return SyscallReturn::error(map_capability_error(error)),
        }
    };

    let spawned = match user::spawn_builtin_task(
        context.arguments[0] as u32,
        TaskRole::SystemService,
        bootstrap_transfer.clone(),
    ) {
        Ok(spawned) => spawned,
        Err(error) => {
            // The Move already removed the source handle; give it back so a
            // failed spawn does not consume the caller's capability.
            if let Some(transfer) = &bootstrap_transfer {
                task.capability_space().rollback_moved(transfer);
            }
            return SyscallReturn::error(map_spawn_error(error));
        }
    };

    match task
        .capability_space()
        .install(spawned.task, CapabilityRights::task(), None)
    {
        Ok(handle) => SyscallReturn::success(handle.0 as u64),
        Err(error) => SyscallReturn::error(map_capability_error(error)),
    }
}

pub(crate) fn handle_task_status(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(target_task) = object.task() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(status_out) = (unsafe { super::super::user_mut::<AbiTaskStatus>(context.arguments[1]) })
    else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    *status_out = match target_task.exit_status() {
        user::TaskExitStatus::Running => AbiTaskStatus {
            state: TaskStateCode::Running,
            exit_code: 0,
        },
        user::TaskExitStatus::Exited { code } => AbiTaskStatus {
            state: TaskStateCode::Exited,
            exit_code: code,
        },
        user::TaskExitStatus::Faulted { code } => AbiTaskStatus {
            state: TaskStateCode::Faulted,
            exit_code: code,
        },
    };

    SyscallReturn::success(0)
}

pub(crate) fn handle_task_loaded_libraries(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(target_task) = object.task() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Some(address_space_id) = target_task.address_space() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Some(image) = user::loaded_image_for(address_space_id) else {
        return SyscallReturn::error(SyscallError::NotFound);
    };
    // Runtime-loaded libraries append after the spawn-time companions;
    // spawn companions carry their boot-store image id, runtime loads the
    // TaskLoadLibrary handle (image_id = 0).
    let runtime_records = user::runtime()
        .map(|runtime| runtime.runtime_library_records(address_space_id))
        .unwrap_or_default();
    let total = image.library_count + runtime_records.len();

    let capacity = context.arguments[2] as usize;
    if capacity < total {
        return SyscallReturn::error(SyscallError::CapacityExceeded);
    }
    if total > 0 {
        let mut staged: Vec<TaskLoadedLibrary> = Vec::with_capacity(total);
        for record in &image.libraries[..image.library_count] {
            staged.push(TaskLoadedLibrary {
                image_id: record.image_id,
                library_handle: 0,
                base: record.base.as_u64(),
                mapped_bytes: record.mapped_bytes as u64,
            });
        }
        for record in &runtime_records {
            staged.push(TaskLoadedLibrary {
                image_id: 0,
                library_handle: record.handle,
                base: record.base,
                mapped_bytes: record.mapped_bytes,
            });
        }
        let byte_len = core::mem::size_of::<TaskLoadedLibrary>() * total;
        let Ok(out_bytes) =
            (unsafe { super::super::user_slice_mut(context.arguments[1], byte_len) })
        else {
            return SyscallReturn::error(SyscallError::InvalidArgument);
        };
        let src = core::ptr::slice_from_raw_parts(staged.as_ptr().cast::<u8>(), byte_len);
        out_bytes.copy_from_slice(unsafe { &*src });
    }

    SyscallReturn::success(total as u64)
}

pub(crate) fn handle_task_spawn_image(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let authority = match task.capability_space().resolve(
        CapabilityHandle(context.arguments[1] as Handle),
        CapabilityRights::bootstrap(),
    ) {
        Ok(view) => view,
        Err(error) => return SyscallReturn::error(map_capability_error(error)),
    };
    if authority.object.bootstrap_capability().is_none() {
        return SyscallReturn::error(SyscallError::PermissionDenied);
    }
    let image_view = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ,
    ) {
        Ok(view) => view,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(image_object) = image_view.object.memory_object() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let image_len = image_object.info().size_bytes;
    let mut image_bytes = alloc::vec![0u8; image_len];
    let copied = image_object.read(0, &mut image_bytes);
    image_bytes.truncate(copied);

    let bootstrap_transfer = if context.arguments[2] == 0 {
        None
    } else {
        match task.capability_space().prepare_transfer(
            CapabilityHandle(context.arguments[2] as Handle),
            CapabilityRights::channel_endpoint(),
            TransferMode::Copy,
        ) {
            Ok(transfer) => Some(transfer),
            Err(error) => return SyscallReturn::error(map_capability_error(error)),
        }
    };

    // Additive ABI-flag slot: 0 = native numbering (every legacy caller
    // zeroes this register through the runtime wrappers), the shared-ABI
    // `spawn_abi::LINUX_SYSCALL` word opts the task into Linux x86_64
    // syscall translation, and the extended-attributes form (bit 63 set)
    // additionally carries the kernel-visible isolation class and
    // owner-environment id. Unknown words are rejected loudly. The next
    // argument slot is reserved and must stay zero.
    let attributes = match crate::syscall::SpawnAttributes::from_flag_word(context.arguments[3]) {
        Some(attributes) => attributes,
        None => return SyscallReturn::error(SyscallError::InvalidArgument),
    };
    if context.arguments[4] != 0 {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    }

    let spawned = match user::spawn_image_bytes_with_attributes(
        &image_bytes,
        TaskRole::UserService,
        bootstrap_transfer,
        user::SpawnImageAttributes {
            syscall_abi: attributes.abi,
            isolation: if attributes.isolation_guest {
                crate::task::TaskIsolationClass::Guest
            } else {
                crate::task::TaskIsolationClass::Unrestricted
            },
            owner_env: attributes.owner_env.map(u32::from),
        },
    ) {
        Ok(spawned) => spawned,
        Err(error) => return SyscallReturn::error(map_spawn_error(error)),
    };
    match task
        .capability_space()
        .install(spawned.task, CapabilityRights::task(), None)
    {
        Ok(handle) => SyscallReturn::success(handle.0 as u64),
        Err(error) => SyscallReturn::error(map_capability_error(error)),
    }
}

/// Upper bound on a single runtime library's file size (kernel-heap
/// staging buffer) and mapping span (shared-mapping band growth).
const MAX_RUNTIME_LIBRARY_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_RUNTIME_LIBRARY_MAPPING_BYTES: u64 = 64 * 1024 * 1024;

fn map_library_load_error(error: user::LoadError) -> SyscallError {
    use user::LoadError;
    match error {
        LoadError::FrameExhausted => SyscallError::CapacityExceeded,
        LoadError::UnsupportedFormat
        | LoadError::UnsupportedAbi
        | LoadError::UnsupportedHeader
        | LoadError::UnsupportedMachine
        | LoadError::UnsupportedRelocation
        | LoadError::KernelAbiTooNew
        | LoadError::InterpreterUnsupported
        | LoadError::DependencyInvalid
        | LoadError::UnresolvedSymbol { .. }
        | LoadError::SymbolSpaceExhausted => SyscallError::Unsupported,
        LoadError::DependencyUnavailable => SyscallError::NotFound,
        LoadError::Truncated | LoadError::InvalidMagic | LoadError::AddressAlignment => {
            SyscallError::InvalidArgument
        }
        LoadError::Mapping(MappingError::Unsupported) => SyscallError::InvalidArgument,
        LoadError::Mapping(_) => SyscallError::Busy,
    }
}

/// Runtime-load an additional ELF64 `ET_DYN` shared object into the
/// calling task's address space. Wire shape mirrors `TaskSpawnImage`: the
/// library bytes arrive through a memory object the caller already owns.
/// The load is atomic from the task's point of view — every failure path
/// releases staged frames and unmaps partial mappings before answering.
pub(crate) fn handle_task_load_library(context: &SyscallContext) -> SyscallReturn {
    use serviceos_abi::task_load_library_flags;

    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(address_space_id) = task.address_space() else {
        return SyscallReturn::error(SyscallError::Unsupported);
    };
    let Some(runtime) = user::runtime() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(hooks) = user::arch_hooks() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(memory) = crate::memory::manager() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };

    if context.arguments[1] & !task_load_library_flags::VALID_MASK != 0
        || context.arguments[2] != 0
        || context.arguments[3] != 0
    {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    }

    let image_view = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ,
    ) {
        Ok(view) => view,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(image_object) = image_view.object.memory_object() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let image_len = image_object.info().size_bytes;
    if image_len == 0 || image_len > MAX_RUNTIME_LIBRARY_IMAGE_BYTES {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    }
    let mut image_bytes = alloc::vec![0u8; image_len];
    let copied = image_object.read(0, &mut image_bytes);
    image_bytes.truncate(copied);
    if !user::is_elf64_image(&image_bytes) {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    }

    let plan = match user::plan_elf_dependency(&image_bytes, user::HOST_ELF_MACHINE) {
        Ok(plan) => plan,
        Err(error) => return SyscallReturn::error(map_library_load_error(error)),
    };
    let span_bytes = plan.mapping_span_bytes();
    if span_bytes == 0 || span_bytes > MAX_RUNTIME_LIBRARY_MAPPING_BYTES {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    }
    let Some(base) = runtime.reserve_mapping_range(address_space_id, span_bytes as usize) else {
        return SyscallReturn::error(SyscallError::Busy);
    };

    // Stage: validate + fill frames. Staging releases its own frames when
    // it fails; the allocator lock must not be held across arch hook calls
    // (the hooks lock it internally).
    let staged = {
        let mut frame_allocator = memory.frame_allocator().lock();
        match user::stage_runtime_library(
            &image_bytes,
            user::HOST_ELF_MACHINE,
            base,
            &mut frame_allocator,
        ) {
            Ok(staged) => staged,
            Err(error) => return SyscallReturn::error(map_library_load_error(error)),
        }
    };

    let Some((handle, mut staging_table)) = runtime.begin_runtime_load(address_space_id) else {
        let mut frame_allocator = memory.frame_allocator().lock();
        staged.release_frames(&mut frame_allocator);
        return SyscallReturn::error(SyscallError::CapacityExceeded);
    };

    // Register exports into the staging copy, then apply this module's
    // relocations against the staged table (own exports + task seed +
    // previously loaded libraries). All writes hit staged frames; nothing
    // is user-visible yet.
    let resolution = (|| -> Result<(), user::LoadError> {
        staged.register_exports(&mut staging_table)?;
        staged.apply_relocations(&staging_table)
    })();
    if let Err(error) = resolution {
        let mut frame_allocator = memory.frame_allocator().lock();
        staged.release_frames(&mut frame_allocator);
        return SyscallReturn::error(map_library_load_error(error));
    }

    // Map the staged frames into the task: read-only or read-write per
    // segment first, then executable segments flip to RX through the
    // protection hook — the house W^X policy, applied segment-wide.
    let mut mapped_segments: Vec<(u64, usize)> = Vec::new();
    let mapping = (|| -> Result<(), MappingError> {
        for segment in staged.segments() {
            (hooks.map_memory_object)(
                address_space_id,
                VirtualAddress::new(segment.virtual_start),
                &segment.pages,
                segment.writable,
            )?;
            mapped_segments.push((segment.virtual_start, segment.pages.len()));
            if segment.executable {
                (hooks.update_memory_protection)(
                    address_space_id,
                    VirtualAddress::new(segment.virtual_start),
                    segment.pages.len(),
                    MappingFlags::EXECUTABLE,
                )?;
            }
        }
        Ok(())
    })();
    if mapping.is_err() {
        for (virtual_start, pages) in mapped_segments.iter().rev() {
            let _ = (hooks.unmap_memory_range)(
                address_space_id,
                VirtualAddress::new(*virtual_start),
                *pages,
            );
        }
        let mut frame_allocator = memory.frame_allocator().lock();
        staged.release_frames(&mut frame_allocator);
        return SyscallReturn::error(SyscallError::Busy);
    }

    let mapping_bytes = staged.mapping_bytes();
    if !runtime.commit_runtime_load(
        address_space_id,
        handle,
        base.as_u64(),
        mapping_bytes,
        staging_table,
    ) {
        for (virtual_start, pages) in mapped_segments.iter().rev() {
            let _ = (hooks.unmap_memory_range)(
                address_space_id,
                VirtualAddress::new(*virtual_start),
                *pages,
            );
        }
        let mut frame_allocator = memory.frame_allocator().lock();
        staged.release_frames(&mut frame_allocator);
        return SyscallReturn::error(SyscallError::NotInitialized);
    }

    SyscallReturn::success(handle as u64)
}

/// Resolve a symbol name against the calling task's load-scoped symbol
/// table. The handle must name a library this task runtime-loaded (it
/// validates ownership); the search itself covers the whole task-scoped
/// namespace — spawn seed plus every loaded library — matching the load
/// time resolution rules.
pub(crate) fn handle_task_symbol_lookup(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(address_space_id) = task.address_space() else {
        return SyscallReturn::error(SyscallError::Unsupported);
    };
    let Some(runtime) = user::runtime() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };

    let handle = context.arguments[0] as Handle;
    let name_len = context.arguments[2] as usize;
    if handle == 0 || name_len == 0 || name_len > user::MAX_EXPORT_NAME {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    }
    let name = match unsafe { super::super::user_slice(context.arguments[1], name_len) } {
        Ok(name) => name,
        Err(error) => return SyscallReturn::error(error),
    };
    if !runtime.runtime_library_known(address_space_id, handle) {
        return SyscallReturn::error(SyscallError::NotFound);
    }
    match runtime.lookup_task_symbol(address_space_id, name) {
        Some(address) => SyscallReturn::success(address),
        None => SyscallReturn::error(SyscallError::NotFound),
    }
}

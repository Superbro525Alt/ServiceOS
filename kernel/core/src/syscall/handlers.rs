use serviceos_abi::{
    AudioEndpointInfo as AbiAudioEndpointInfo, AudioToneRequest as AbiAudioToneRequest,
    DisplayOutputInfo as AbiDisplayOutputInfo, Handle, HandlePair, INPUT_SOURCE_FLAG_NONBLOCK,
    IPC_FLAG_NONBLOCK, IPC_MAX_HANDLES, IPC_MAX_WORDS, InputEventInfo as AbiInputEventInfo,
    InputSourceInfo as AbiInputSourceInfo, PACKET_INTERFACE_FLAG_NONBLOCK,
    PacketInterfaceInfo as AbiPacketInterfaceInfo, RawMessage, TaskStateCode,
    TaskStatus as AbiTaskStatus,
};

use crate::{
    capability::{
        CapabilityError, CapabilityHandle, CapabilityResolver, CapabilityRights, TransferMode,
    },
    ipc::{self, IpcError, MessageTag, OutgoingMessage},
    task::TaskRole,
    time,
    user::{self, AddressSpacePreparationError, LoadError, SpawnError},
};

use super::{
    SYSCALL_ABI_VERSION, SyscallAction, SyscallContext, SyscallError, SyscallReturn,
    resolve::{current_task, resolve_object},
    user_mut, user_ref, user_slice, user_slice_mut,
};

pub(super) static DEBUG_LOG_WRITER: spin::Once<fn(&[u8])> = spin::Once::new();
pub(super) static DEBUG_CONSOLE_READER: spin::Once<fn() -> Option<u8>> = spin::Once::new();
pub(super) static DEBUG_CONSOLE_WRITER: spin::Once<fn(&[u8])> = spin::Once::new();

pub(super) fn handle_abi_version(_context: &SyscallContext) -> SyscallReturn {
    SyscallReturn::success(SYSCALL_ABI_VERSION)
}

pub(super) fn handle_monotonic_now(_context: &SyscallContext) -> SyscallReturn {
    match time::manager() {
        Some(manager) => SyscallReturn::success(manager.now().0),
        None => SyscallReturn::error(SyscallError::NotInitialized),
    }
}

pub(super) fn handle_thread_exit(context: &SyscallContext) -> SyscallReturn {
    SyscallReturn::exit_current_thread(context.arguments[0])
}

pub(super) fn handle_yield_current(_context: &SyscallContext) -> SyscallReturn {
    SyscallReturn::action(0, SyscallAction::YieldCurrentThread)
}

pub(super) fn handle_debug_log_write(context: &SyscallContext) -> SyscallReturn {
    let Some(writer) = DEBUG_LOG_WRITER.get().copied() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Ok(length) = usize::try_from(context.arguments[1]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(bytes) = (unsafe { user_slice(context.arguments[0], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    writer(bytes);
    SyscallReturn::success(length as u64)
}

pub(super) fn handle_channel_create(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(ipc) = ipc::kernel() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(objects) = crate::object::model() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };

    let (first, second) = ipc.create_channel_pair(objects);
    let first_handle =
        match task
            .capability_space()
            .install(first, CapabilityRights::channel_endpoint(), None)
        {
            Ok(handle) => handle,
            Err(error) => return SyscallReturn::error(map_capability_error(error)),
        };
    let second_handle =
        match task
            .capability_space()
            .install(second, CapabilityRights::channel_endpoint(), None)
        {
            Ok(handle) => handle,
            Err(error) => return SyscallReturn::error(map_capability_error(error)),
        };
    let Ok(pair_out) = (unsafe { user_mut::<HandlePair>(context.arguments[0]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    *pair_out = HandlePair {
        first: first_handle.0,
        second: second_handle.0,
    };
    SyscallReturn::success(0)
}

pub(super) fn handle_channel_send(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(ipc) = ipc::kernel() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Ok(raw) = (unsafe { user_ref::<RawMessage>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    let word_count = raw.word_count as usize;
    let handle_count = raw.handle_count as usize;
    if word_count > IPC_MAX_WORDS || handle_count > IPC_MAX_HANDLES {
        return SyscallReturn::error(SyscallError::BufferTooSmall);
    }

    let mut message = match OutgoingMessage::new(MessageTag(raw.tag), &raw.words[..word_count]) {
        Ok(message) => message,
        Err(error) => return SyscallReturn::error(map_ipc_error(error)),
    };
    for (index, handle) in raw.handles[..handle_count].iter().copied().enumerate() {
        let Some(descriptor) = task
            .capability_space()
            .resolve_descriptor(CapabilityHandle(handle))
        else {
            return SyscallReturn::error(SyscallError::NotFound);
        };
        let requested_bits = raw.handle_rights[index];
        let transfer_rights = if requested_bits == 0 {
            descriptor
                .rights
                .without(CapabilityRights::DUPLICATE.union(CapabilityRights::TRANSFER))
        } else {
            CapabilityRights::from_bits(requested_bits)
        };
        let transfer = match task.capability_space().prepare_transfer(
            CapabilityHandle(handle),
            transfer_rights,
            TransferMode::Copy,
        ) {
            Ok(transfer) => transfer,
            Err(error) => return SyscallReturn::error(map_capability_error(error)),
        };
        message = match message.add_transfer(transfer) {
            Ok(message) => message,
            Err(error) => return SyscallReturn::error(map_ipc_error(error)),
        };
    }

    match ipc.send(
        task.capability_space(),
        CapabilityHandle(context.arguments[0] as Handle),
        message,
    ) {
        Ok(_) => SyscallReturn::success(0),
        Err(error) => SyscallReturn::error(map_ipc_error(error)),
    }
}

pub(super) fn handle_channel_receive(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(ipc_kernel) = ipc::kernel() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Ok(message_out) = (unsafe { user_mut::<RawMessage>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    match ipc_kernel.receive(
        task.capability_space(),
        CapabilityHandle(context.arguments[0] as Handle),
    ) {
        Ok(message) => {
            if message.word_count > IPC_MAX_WORDS
                || message.transferred_capability_count > IPC_MAX_HANDLES
            {
                return SyscallReturn::error(SyscallError::BufferTooSmall);
            }

            let mut raw = RawMessage::empty(message.tag.0);
            raw.word_count = message.word_count as u32;
            raw.handle_count = message.transferred_capability_count as u32;
            raw.flags = message_out.flags;
            for (index, word) in message.words().iter().copied().enumerate() {
                raw.words[index] = word;
            }
            for (index, handle) in message
                .transferred_capabilities()
                .iter()
                .copied()
                .enumerate()
            {
                raw.handles[index] = handle.0;
            }
            *message_out = raw;
            SyscallReturn::success(message.tag.0 as u64)
        }
        Err(IpcError::QueueEmpty) if message_out.flags & IPC_FLAG_NONBLOCK != 0 => {
            SyscallReturn::error(SyscallError::QueueEmpty)
        }
        Err(IpcError::QueueEmpty) => {
            let endpoint = match ipc_kernel.endpoint_object_id(
                task.capability_space(),
                CapabilityHandle(context.arguments[0] as Handle),
                CapabilityRights::RECEIVE,
            ) {
                Ok(endpoint) => endpoint,
                Err(error) => return SyscallReturn::error(map_ipc_error(error)),
            };
            SyscallReturn::error_with_action(
                SyscallError::QueueEmpty,
                SyscallAction::BlockCurrentThreadOnReceive { endpoint },
            )
        }
        Err(error) => SyscallReturn::error(map_ipc_error(error)),
    }
}

pub(super) fn handle_handle_duplicate(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let rights = CapabilityRights::from_bits(context.arguments[1]);
    match task.capability_space().duplicate(
        CapabilityHandle(context.arguments[0] as Handle),
        rights,
        None,
    ) {
        Ok(handle) => SyscallReturn::success(handle.0 as u64),
        Err(error) => SyscallReturn::error(map_capability_error(error)),
    }
}

pub(super) fn handle_handle_close(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    match task
        .capability_space()
        .close(CapabilityHandle(context.arguments[0] as Handle))
    {
        Ok(_) => SyscallReturn::success(0),
        Err(error) => SyscallReturn::error(map_capability_error(error)),
    }
}

pub(super) fn handle_service_spawn(context: &SyscallContext) -> SyscallReturn {
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
        bootstrap_transfer,
    ) {
        Ok(spawned) => spawned,
        Err(SpawnError::ImageNotFound) => return SyscallReturn::error(SyscallError::NotFound),
        Err(SpawnError::Capability(error)) => {
            return SyscallReturn::error(map_capability_error(error));
        }
        Err(SpawnError::Scheduler(_)) => return SyscallReturn::error(SyscallError::Busy),
        Err(SpawnError::AddressSpace(AddressSpacePreparationError::Load(
            LoadError::FrameExhausted,
        )))
        | Err(SpawnError::AddressSpace(AddressSpacePreparationError::Mapping(
            crate::memory::MappingError::FrameAllocationFailed,
        ))) => {
            return SyscallReturn::error(SyscallError::CapacityExceeded);
        }
        Err(_) => return SyscallReturn::error(SyscallError::NotInitialized),
    };

    match task
        .capability_space()
        .install(spawned.task, CapabilityRights::task(), None)
    {
        Ok(handle) => SyscallReturn::success(handle.0 as u64),
        Err(error) => SyscallReturn::error(map_capability_error(error)),
    }
}

pub(super) fn handle_task_status(context: &SyscallContext) -> SyscallReturn {
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
    let Ok(status_out) = (unsafe { user_mut::<AbiTaskStatus>(context.arguments[1]) }) else {
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

pub(super) fn handle_memory_read(context: &SyscallContext) -> SyscallReturn {
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
    let Some(memory) = object.memory_object() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(offset) = usize::try_from(context.arguments[1]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(length) = usize::try_from(context.arguments[3]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(destination) = (unsafe { user_slice_mut(context.arguments[2], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    SyscallReturn::success(memory.read(offset, destination) as u64)
}

pub(super) fn handle_memory_create(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(objects) = crate::object::model() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Ok(size_bytes) = usize::try_from(context.arguments[0]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let writable = context.arguments[1] != 0;
    let object = objects
        .registry()
        .create_memory_object(size_bytes, writable);

    match task
        .capability_space()
        .install(object, CapabilityRights::memory_object(), None)
    {
        Ok(handle) => SyscallReturn::success(handle.0 as u64),
        Err(error) => SyscallReturn::error(map_capability_error(error)),
    }
}

pub(super) fn handle_memory_write(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::WRITE,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(memory) = object.memory_object() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(offset) = usize::try_from(context.arguments[1]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(length) = usize::try_from(context.arguments[3]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(source) = (unsafe { user_slice(context.arguments[2], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    match memory.write(offset, source) {
        Ok(written) => SyscallReturn::success(written as u64),
        Err(crate::object::MemoryAccessError::ReadOnly) => {
            SyscallReturn::error(SyscallError::PermissionDenied)
        }
        Err(crate::object::MemoryAccessError::Busy) => SyscallReturn::error(SyscallError::Busy),
        Err(crate::object::MemoryAccessError::Unsupported) => {
            SyscallReturn::error(SyscallError::Unsupported)
        }
    }
}

pub(super) fn handle_memory_map(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let view = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ.union(CapabilityRights::MAP),
    ) {
        Ok(view) => view,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(address_space_id) = task.address_space() else {
        return SyscallReturn::error(SyscallError::Unsupported);
    };
    let Some(memory) = view.object.memory_object() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let writable = context.arguments[1] != 0;
    if writable && !view.rights.contains(CapabilityRights::WRITE) {
        return SyscallReturn::error(SyscallError::PermissionDenied);
    }
    let Some(runtime) = user::runtime() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(hooks) = user::arch_hooks() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(base) = runtime.reserve_mapping_range(address_space_id, memory.info().size_bytes)
    else {
        return SyscallReturn::error(SyscallError::Busy);
    };
    let frames = match memory.page_frames() {
        Ok(frames) => frames,
        Err(crate::object::MemoryAccessError::ReadOnly) => {
            return SyscallReturn::error(SyscallError::PermissionDenied);
        }
        Err(crate::object::MemoryAccessError::Busy) => {
            return SyscallReturn::error(SyscallError::Busy);
        }
        Err(crate::object::MemoryAccessError::Unsupported) => {
            return SyscallReturn::error(SyscallError::Unsupported);
        }
    };

    match (hooks.map_memory_object)(address_space_id, base, frames.as_ref(), writable) {
        Ok(()) => SyscallReturn::success(base.as_u64()),
        Err(crate::memory::MappingError::AddressAlignment) => {
            SyscallReturn::error(SyscallError::InvalidArgument)
        }
        Err(crate::memory::MappingError::AlreadyMapped) => SyscallReturn::error(SyscallError::Busy),
        Err(crate::memory::MappingError::FrameAllocationFailed) => {
            SyscallReturn::error(SyscallError::Busy)
        }
        Err(crate::memory::MappingError::Unsupported) => {
            SyscallReturn::error(SyscallError::Unsupported)
        }
    }
}

pub(super) fn handle_task_spawn_image(context: &SyscallContext) -> SyscallReturn {
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

    let spawned =
        match user::spawn_image_bytes(&image_bytes, TaskRole::UserService, bootstrap_transfer) {
            Ok(spawned) => spawned,
            Err(SpawnError::ImageNotFound) => return SyscallReturn::error(SyscallError::NotFound),
            Err(SpawnError::Capability(error)) => {
                return SyscallReturn::error(map_capability_error(error));
            }
            Err(SpawnError::Scheduler(_)) => return SyscallReturn::error(SyscallError::Busy),
            Err(SpawnError::AddressSpace(AddressSpacePreparationError::Load(
                LoadError::FrameExhausted,
            )))
            | Err(SpawnError::AddressSpace(AddressSpacePreparationError::Mapping(
                crate::memory::MappingError::FrameAllocationFailed,
            ))) => {
                return SyscallReturn::error(SyscallError::CapacityExceeded);
            }
            Err(_) => return SyscallReturn::error(SyscallError::NotInitialized),
        };
    match task
        .capability_space()
        .install(spawned.task, CapabilityRights::task(), None)
    {
        Ok(handle) => SyscallReturn::success(handle.0 as u64),
        Err(error) => SyscallReturn::error(map_capability_error(error)),
    }
}

pub(super) fn handle_debug_console_read(_context: &SyscallContext) -> SyscallReturn {
    let Some(reader) = DEBUG_CONSOLE_READER.get().copied() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };

    match reader() {
        Some(byte) => SyscallReturn::success(byte as u64),
        None => SyscallReturn::error(SyscallError::QueueEmpty),
    }
}

pub(super) fn handle_debug_console_write(context: &SyscallContext) -> SyscallReturn {
    let Some(writer) = DEBUG_CONSOLE_WRITER.get().copied() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Ok(length) = usize::try_from(context.arguments[1]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(bytes) = (unsafe { user_slice(context.arguments[0], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    writer(bytes);
    SyscallReturn::success(length as u64)
}

pub(super) fn handle_packet_interface_info(context: &SyscallContext) -> SyscallReturn {
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
    let Some(interface) = object.packet_interface() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(info_out) = (unsafe { user_mut::<AbiPacketInterfaceInfo>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    *info_out = interface.info();
    SyscallReturn::success(0)
}

pub(super) fn handle_packet_interface_receive(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let view = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ.union(CapabilityRights::WAIT),
    ) {
        Ok(view) => view,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(interface) = view.object.packet_interface() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(length) = usize::try_from(context.arguments[2]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(buffer) = (unsafe { user_slice_mut(context.arguments[1], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    match interface.receive(buffer) {
        Ok(received) => SyscallReturn::success(received as u64),
        Err(crate::network::PacketInterfaceError::QueueEmpty)
            if context.arguments[3] as u32 & PACKET_INTERFACE_FLAG_NONBLOCK != 0 =>
        {
            SyscallReturn::error(SyscallError::QueueEmpty)
        }
        Err(crate::network::PacketInterfaceError::QueueEmpty) => SyscallReturn::error_with_action(
            SyscallError::QueueEmpty,
            SyscallAction::BlockCurrentThreadOnPacketReceive {
                interface: view.object.id(),
            },
        ),
        Err(crate::network::PacketInterfaceError::BufferTooSmall) => {
            SyscallReturn::error(SyscallError::BufferTooSmall)
        }
        Err(crate::network::PacketInterfaceError::Busy) => SyscallReturn::error(SyscallError::Busy),
        Err(crate::network::PacketInterfaceError::Unsupported) => {
            SyscallReturn::error(SyscallError::Unsupported)
        }
    }
}

pub(super) fn handle_packet_interface_transmit(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::WRITE,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(interface) = object.packet_interface() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(length) = usize::try_from(context.arguments[2]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(buffer) = (unsafe { user_slice(context.arguments[1], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    match interface.transmit(buffer) {
        Ok(()) => SyscallReturn::success(length as u64),
        Err(crate::network::PacketInterfaceError::QueueEmpty) => {
            SyscallReturn::error(SyscallError::QueueEmpty)
        }
        Err(crate::network::PacketInterfaceError::BufferTooSmall) => {
            SyscallReturn::error(SyscallError::BufferTooSmall)
        }
        Err(crate::network::PacketInterfaceError::Busy) => SyscallReturn::error(SyscallError::Busy),
        Err(crate::network::PacketInterfaceError::Unsupported) => {
            SyscallReturn::error(SyscallError::Unsupported)
        }
    }
}

pub(super) fn handle_display_output_info(context: &SyscallContext) -> SyscallReturn {
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
    let Some(output) = object.display_output() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(info_out) = (unsafe { user_mut::<AbiDisplayOutputInfo>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    *info_out = output.info();
    SyscallReturn::success(0)
}

pub(super) fn handle_display_output_present(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::WRITE,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(output) = object.display_output() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(length) = usize::try_from(context.arguments[2]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(buffer) = (unsafe { user_slice(context.arguments[1], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    match output.present(buffer) {
        Ok(()) => SyscallReturn::success(length as u64),
        Err(crate::display::DisplayOutputError::BufferTooSmall) => {
            SyscallReturn::error(SyscallError::BufferTooSmall)
        }
        Err(crate::display::DisplayOutputError::Busy) => SyscallReturn::error(SyscallError::Busy),
        Err(crate::display::DisplayOutputError::Unsupported) => {
            SyscallReturn::error(SyscallError::Unsupported)
        }
    }
}

pub(super) fn handle_input_source_info(context: &SyscallContext) -> SyscallReturn {
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
    let Some(source) = object.input_source() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(info_out) = (unsafe { user_mut::<AbiInputSourceInfo>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    *info_out = source.info();
    SyscallReturn::success(0)
}

pub(super) fn handle_input_source_receive(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let view = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ.union(CapabilityRights::WAIT),
    ) {
        Ok(view) => view,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(source) = view.object.input_source() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(event_out) = (unsafe { user_mut::<AbiInputEventInfo>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    let receive_result = if context.arguments[2] as u32 & INPUT_SOURCE_FLAG_NONBLOCK != 0 {
        source.try_receive_with_fallback()
    } else {
        source.receive()
    };

    match receive_result {
        Ok(event) => {
            *event_out = event;
            SyscallReturn::success(0)
        }
        Err(crate::input::InputSourceError::QueueEmpty)
            if context.arguments[2] as u32 & INPUT_SOURCE_FLAG_NONBLOCK != 0 =>
        {
            SyscallReturn::error(SyscallError::QueueEmpty)
        }
        Err(crate::input::InputSourceError::QueueEmpty) => SyscallReturn::error_with_action(
            SyscallError::QueueEmpty,
            SyscallAction::BlockCurrentThreadOnInputReceive {
                source: view.object.id(),
            },
        ),
        Err(crate::input::InputSourceError::Busy) => SyscallReturn::error(SyscallError::Busy),
        Err(crate::input::InputSourceError::Unsupported) => {
            SyscallReturn::error(SyscallError::Unsupported)
        }
    }
}

pub(super) fn handle_audio_endpoint_info(context: &SyscallContext) -> SyscallReturn {
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
    let Some(endpoint) = object.audio_endpoint() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(info_out) = (unsafe { user_mut::<AbiAudioEndpointInfo>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    *info_out = endpoint.info();
    SyscallReturn::success(0)
}

pub(super) fn handle_audio_endpoint_play_tone(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::WRITE,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(endpoint) = object.audio_endpoint() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(request) = (unsafe { user_ref::<AbiAudioToneRequest>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    match endpoint.play_tone(*request) {
        Ok(()) => SyscallReturn::success(0),
        Err(crate::audio::AudioEndpointError::Busy) => SyscallReturn::error(SyscallError::Busy),
        Err(crate::audio::AudioEndpointError::Unsupported) => {
            SyscallReturn::error(SyscallError::Unsupported)
        }
    }
}

pub(super) fn handle_audio_endpoint_stop(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::WRITE,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(endpoint) = object.audio_endpoint() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    match endpoint.stop() {
        Ok(()) => SyscallReturn::success(0),
        Err(crate::audio::AudioEndpointError::Busy) => SyscallReturn::error(SyscallError::Busy),
        Err(crate::audio::AudioEndpointError::Unsupported) => {
            SyscallReturn::error(SyscallError::Unsupported)
        }
    }
}

pub(super) fn map_capability_error(error: CapabilityError) -> SyscallError {
    match error {
        CapabilityError::InvalidHandle => SyscallError::NotFound,
        CapabilityError::HandleSpaceExhausted => SyscallError::CapacityExceeded,
        CapabilityError::RightsViolation { .. }
        | CapabilityError::DuplicateForbidden
        | CapabilityError::TransferForbidden
        | CapabilityError::RequestedRightsExceedSource => SyscallError::PermissionDenied,
    }
}

pub(super) fn map_ipc_error(error: IpcError) -> SyscallError {
    match error {
        IpcError::Capability(error) => map_capability_error(error),
        IpcError::EndpointNotReady | IpcError::EndpointClosed => SyscallError::Busy,
        IpcError::BufferShapeInvalid
        | IpcError::ObjectKindMismatch
        | IpcError::InvalidReplyEndpoint => SyscallError::InvalidArgument,
        IpcError::QueueEmpty => SyscallError::QueueEmpty,
        IpcError::QueueFull { .. }
        | IpcError::MessageTooLarge { .. }
        | IpcError::TooManyTransfers { .. } => SyscallError::CapacityExceeded,
    }
}

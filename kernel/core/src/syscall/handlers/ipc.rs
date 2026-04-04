use serviceos_abi::{
    Handle, HandlePair, IPC_FLAG_NONBLOCK, IPC_MAX_HANDLES, IPC_MAX_WORDS, RawMessage,
};

use super::{
    common::{map_capability_error, map_ipc_error},
    super::{
        SyscallAction, SyscallContext, SyscallError, SyscallReturn, resolve::current_task,
        user_mut, user_ref,
    },
};
use crate::{
    capability::{
        CapabilityHandle, CapabilityResolver, CapabilityRights, TransferMode,
    },
    ipc::{self, IpcError, MessageTag, OutgoingMessage},
};

pub(crate) fn handle_channel_create(context: &SyscallContext) -> SyscallReturn {
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

pub(crate) fn handle_channel_send(context: &SyscallContext) -> SyscallReturn {
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

pub(crate) fn handle_channel_receive(context: &SyscallContext) -> SyscallReturn {
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

pub(crate) fn handle_handle_duplicate(context: &SyscallContext) -> SyscallReturn {
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

pub(crate) fn handle_handle_close(context: &SyscallContext) -> SyscallReturn {
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

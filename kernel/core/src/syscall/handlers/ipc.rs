use serviceos_abi::{
    Handle, HandlePair, IPC_FLAG_NONBLOCK, IPC_FLAG_RECEIVE_TIMEOUT, IPC_MAX_HANDLES,
    IPC_MAX_WORDS, RawMessage,
};

use super::{
    super::{
        SyscallAction, SyscallContext, SyscallError, SyscallReturn, resolve::current_task,
        user_mut, user_ref,
    },
    common::{map_capability_error, map_ipc_error},
};
use crate::{
    capability::{CapabilityHandle, CapabilityResolver, CapabilityRights, TransferMode},
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
    // Validate the user's out-pointer before installing anything so a bad
    // address cannot strand the pair of handles about to be created.
    if unsafe { user_mut::<HandlePair>(context.arguments[0]) }.is_err() {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    }

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
            Err(error) => {
                // Roll back the first install so no half-pair leaks.
                let _ = task.capability_space().close(first_handle);
                return SyscallReturn::error(map_capability_error(error));
            }
        };
    // The pointer was validated above and nothing unmaps in between, so this
    // write cannot fault; both handles stay installed.
    let Ok(pair_out) = (unsafe { user_mut::<HandlePair>(context.arguments[0]) }) else {
        return SyscallReturn::success(0);
    };
    *pair_out = HandlePair {
        first: first_handle.0,
        second: second_handle.0,
    };
    SyscallReturn::success(0)
}

#[cfg(feature = "ipc-trace")]
struct TraceBuf {
    bytes: [u8; 128],
    len: usize,
}

#[cfg(feature = "ipc-trace")]
impl core::fmt::Write for TraceBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let len = self.len;
        let capacity = self.bytes.len();
        if len >= capacity {
            return Ok(());
        }
        let remaining = capacity - len;
        let source = s.as_bytes();
        let n = source.len().min(remaining);
        self.bytes[len..len + n].copy_from_slice(&source[..n]);
        self.len = len + n;
        Ok(())
    }
}

#[cfg(feature = "ipc-trace")]
fn trace_ipc(args: core::fmt::Arguments<'_>) {
    if let Some(writer) = super::DEBUG_LOG_WRITER.get() {
        let mut buf = TraceBuf {
            bytes: [0u8; 128],
            len: 0,
        };
        let _ = core::fmt::write(&mut buf, args);
        writer(&buf.bytes[..buf.len]);
    }
}

#[cfg(not(feature = "ipc-trace"))]
fn trace_ipc(_args: core::fmt::Arguments<'_>) {}

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
        Ok(_) => {
            trace_ipc(format_args!(
                "S h={} tag={:#x} w0={:#x} wc={} hc={} ok\r\n",
                context.arguments[0], raw.tag, raw.words[0], word_count, handle_count
            ));
            SyscallReturn::success(0)
        }
        Err(error) => {
            trace_ipc(format_args!(
                "S h={} tag={:#x} w0={:#x} wc={} hc={} err={error:?}\r\n",
                context.arguments[0], raw.tag, raw.words[0], word_count, handle_count
            ));
            SyscallReturn::error(map_ipc_error(error))
        }
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
            trace_ipc(format_args!(
                "R h={} tag={:#x} w0={:#x} wc={} hc={} ok\r\n",
                context.arguments[0],
                message.tag.0,
                message.words().first().copied().unwrap_or(0),
                message.word_count,
                message.transferred_capability_count
            ));
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
            let timed = message_out.flags & IPC_FLAG_RECEIVE_TIMEOUT != 0;
            let deadline_ticks = if timed { context.arguments[2] } else { 0 };
            SyscallReturn::error_with_action(
                SyscallError::QueueEmpty,
                SyscallAction::BlockCurrentThreadOnReceive {
                    endpoint,
                    deadline_ticks,
                },
            )
        }
        Err(error) => {
            trace_ipc(format_args!(
                "R h={} err={error:?}\r\n",
                context.arguments[0]
            ));
            SyscallReturn::error(map_ipc_error(error))
        }
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
        Ok(handle) => {
            // A duplicated pipe handle joins its side's refcount so EOF and
            // broken-pipe stay keyed to the last handle of each side.
            super::pipe::note_pipe_handle_duplicated(&current_task, handle);
            SyscallReturn::success(handle.0 as u64)
        }
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
    // Pipe handles update side refcounts before the table entry disappears;
    // closing the last writer flips the reader side to EOF and vice versa.
    super::pipe::note_pipe_handle_closed(
        &current_task,
        CapabilityHandle(context.arguments[0] as Handle),
    );
    match task
        .capability_space()
        .close(CapabilityHandle(context.arguments[0] as Handle))
    {
        Ok(_) => SyscallReturn::success(0),
        Err(error) => SyscallReturn::error(map_capability_error(error)),
    }
}

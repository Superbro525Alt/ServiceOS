use serviceos_abi::{
    Handle, OBJECT_WAIT_FLAG_NONBLOCK, ObjectInfo as AbiObjectInfo, ObjectKindCode,
    object_state_flags,
};

use super::{
    super::{
        SyscallAction, SyscallContext, SyscallError, SyscallReturn,
        resolve::{current_task, resolve_object},
        user_mut,
    },
    common::map_capability_error,
};
use crate::{
    capability::CapabilityRights,
    object::{KernelObjectRecord, ObjectKind},
    task,
    user::TaskExitStatus,
};

pub(crate) fn handle_event_create(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(objects) = crate::object::model() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = objects.registry().create_event(context.arguments[0] != 0);

    match task
        .capability_space()
        .install(object, CapabilityRights::event(), None)
    {
        Ok(handle) => SyscallReturn::success(handle.0 as u64),
        Err(error) => SyscallReturn::error(map_capability_error(error)),
    }
}

pub(crate) fn handle_event_signal(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::SIGNAL,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(event) = object.event() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    event.signal();
    let _ = task::notify_object_ready(object.id());
    SyscallReturn::success(0)
}

pub(crate) fn handle_event_reset(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::SIGNAL,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(event) = object.event() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    event.reset();
    SyscallReturn::success(0)
}

pub(crate) fn handle_object_info(context: &SyscallContext) -> SyscallReturn {
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
    let Ok(info_out) = (unsafe { user_mut::<AbiObjectInfo>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    *info_out = object_info_view(&object);
    SyscallReturn::success(0)
}

pub(crate) fn handle_object_wait(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let view = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::WAIT,
    ) {
        Ok(view) => view,
        Err(error) => return SyscallReturn::error(error),
    };

    match object_is_wait_ready(&view.object) {
        Ok(true) => SyscallReturn::success(0),
        Ok(false) if (context.arguments[1] as u32 & OBJECT_WAIT_FLAG_NONBLOCK) != 0 => {
            SyscallReturn::error(SyscallError::QueueEmpty)
        }
        Ok(false) => SyscallReturn::action(
            0,
            SyscallAction::BlockCurrentThreadOnObject {
                object: view.object.id(),
            },
        ),
        Err(error) => SyscallReturn::error(error),
    }
}

fn object_is_wait_ready(object: &KernelObjectRecord) -> Result<bool, SyscallError> {
    match object.kind() {
        ObjectKind::Task => Ok(!matches!(
            object.task().expect("task object").exit_status(),
            TaskExitStatus::Running
        )),
        ObjectKind::ChannelEndpoint => Ok(object
            .channel_endpoint()
            .expect("channel endpoint object")
            .snapshot()
            .queued_messages
            != 0),
        ObjectKind::Event => Ok(object.event().expect("event object").snapshot().signaled),
        ObjectKind::PacketInterface => Ok(object
            .packet_interface()
            .expect("packet interface object")
            .info()
            .rx_ready
            != 0),
        ObjectKind::InputSource => Ok(object
            .input_source()
            .expect("input source object")
            .info()
            .pending_events
            != 0),
        _ => Err(SyscallError::InvalidArgument),
    }
}

fn object_info_view(object: &KernelObjectRecord) -> AbiObjectInfo {
    let mut info = AbiObjectInfo {
        object_id: object.id().0,
        kind: map_kind(object.kind()),
        state_flags: 0,
        reserved: 0,
        detail0: 0,
        detail1: 0,
        detail2: 0,
        detail3: 0,
    };

    match object.kind() {
        ObjectKind::Task => {
            let snapshot = object.task().expect("task object").snapshot();
            match snapshot.exit_status {
                TaskExitStatus::Running => {
                    info.state_flags |= object_state_flags::RUNNING;
                }
                TaskExitStatus::Exited { code } => {
                    info.state_flags |= object_state_flags::EXITED | object_state_flags::READY;
                    info.detail0 = code;
                }
                TaskExitStatus::Faulted { code } => {
                    info.state_flags |= object_state_flags::FAULTED | object_state_flags::READY;
                    info.detail0 = code;
                }
            }
            info.detail1 = snapshot.thread_count as u64;
            info.detail2 = snapshot.address_space.map_or(0, |space| space.0);
        }
        ObjectKind::Thread => {
            let snapshot = object.thread().expect("thread object").snapshot();
            if matches!(
                snapshot.execution_state,
                crate::task::ExecutionState::Running
            ) {
                info.state_flags |= object_state_flags::RUNNING;
            }
            info.detail0 = snapshot.owner.0;
            info.detail1 = snapshot.execution_state as u64;
            info.detail2 = snapshot.entry_instruction_pointer.unwrap_or(0);
        }
        ObjectKind::ChannelEndpoint => {
            let snapshot = object
                .channel_endpoint()
                .expect("channel endpoint object")
                .snapshot();
            if snapshot.queued_messages != 0 {
                info.state_flags |= object_state_flags::READY;
            }
            info.detail0 = snapshot.queued_messages as u64;
            info.detail1 = snapshot.peer.map_or(0, |peer| peer.0);
        }
        ObjectKind::Event => {
            let snapshot = object.event().expect("event object").snapshot();
            if snapshot.signaled {
                info.state_flags |= object_state_flags::SIGNALED | object_state_flags::READY;
            }
            info.detail0 = snapshot.signal_count;
        }
        ObjectKind::Timer => {
            let snapshot = object.timer().expect("timer object").snapshot();
            if snapshot.armed {
                info.state_flags |= object_state_flags::ARMED;
            }
            info.detail0 = snapshot.deadline.map_or(0, |deadline| deadline.0);
            info.detail1 = snapshot.periodic_interval_ticks.unwrap_or(0);
        }
        ObjectKind::MemoryObject => {
            let snapshot = object.memory_object().expect("memory object").info();
            if snapshot.writable {
                info.state_flags |= object_state_flags::WRITABLE;
            }
            info.detail0 = snapshot.size_bytes as u64;
            info.detail1 = snapshot.page_count as u64;
        }
        ObjectKind::BootstrapCapability => {}
        ObjectKind::PacketInterface => {
            let snapshot = object
                .packet_interface()
                .expect("packet interface object")
                .info();
            if snapshot.rx_ready != 0 {
                info.state_flags |= object_state_flags::READY;
            }
            info.detail0 = snapshot.rx_ready as u64;
            info.detail1 = snapshot.rx_packets;
            info.detail2 = snapshot.tx_packets;
            info.detail3 = snapshot.dropped_packets;
        }
        ObjectKind::DisplayOutput => {
            let snapshot = object
                .display_output()
                .expect("display output object")
                .info();
            info.detail0 = snapshot.width as u64;
            info.detail1 = snapshot.height as u64;
            info.detail2 = snapshot.present_count;
        }
        ObjectKind::InputSource => {
            let snapshot = object.input_source().expect("input source object").info();
            if snapshot.pending_events != 0 {
                info.state_flags |= object_state_flags::READY;
            }
            info.detail0 = snapshot.pending_events as u64;
            info.detail1 = snapshot.device_count as u64;
            info.detail2 = snapshot.capabilities as u64;
        }
        ObjectKind::AudioEndpoint => {
            let snapshot = object
                .audio_endpoint()
                .expect("audio endpoint object")
                .info();
            info.detail0 = snapshot.state as u64;
            info.detail1 = snapshot.play_count;
            info.detail2 = snapshot.capabilities as u64;
        }
        ObjectKind::BlockDevice => {
            let snapshot = object.block_device().expect("block device object").info();
            if snapshot.writable != 0 {
                info.state_flags |= object_state_flags::WRITABLE;
            }
            info.detail0 = snapshot.block_size as u64;
            info.detail1 = snapshot.block_count;
            info.detail2 = snapshot.read_ops;
            info.detail3 = snapshot.write_ops;
        }
    }

    info
}

const fn map_kind(kind: ObjectKind) -> ObjectKindCode {
    match kind {
        ObjectKind::Task => ObjectKindCode::Task,
        ObjectKind::Thread => ObjectKindCode::Thread,
        ObjectKind::ChannelEndpoint => ObjectKindCode::ChannelEndpoint,
        ObjectKind::Event => ObjectKindCode::Event,
        ObjectKind::Timer => ObjectKindCode::Timer,
        ObjectKind::MemoryObject => ObjectKindCode::MemoryObject,
        ObjectKind::BootstrapCapability => ObjectKindCode::BootstrapCapability,
        ObjectKind::PacketInterface => ObjectKindCode::PacketInterface,
        ObjectKind::DisplayOutput => ObjectKindCode::DisplayOutput,
        ObjectKind::InputSource => ObjectKindCode::InputSource,
        ObjectKind::AudioEndpoint => ObjectKindCode::AudioEndpoint,
        ObjectKind::BlockDevice => ObjectKindCode::BlockDevice,
    }
}

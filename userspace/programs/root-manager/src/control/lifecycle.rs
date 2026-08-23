use rt::{ControlTag, LifecycleEvent, RawMessage, TaskStateCode};
use serviceos_userspace_runtime as rt;

use crate::state::ServiceSlot;

pub(super) fn close_message_handles(message: &RawMessage) {
    for handle in message.handles[..message.handle_count as usize]
        .iter()
        .copied()
    {
        let _ = rt::handle_close(handle);
    }
}

pub(crate) fn stop_service_slot(slot: &mut ServiceSlot) -> rt::Result<()> {
    if slot.control_handle != rt::INVALID_HANDLE {
        let _ = send_lifecycle(slot.control_handle, LifecycleEvent::Stopped);
    }
    if slot.task_handle != rt::INVALID_HANDLE {
        loop {
            match rt::task_status(slot.task_handle) {
                Ok(status)
                    if matches!(status.state, TaskStateCode::Exited | TaskStateCode::Faulted) =>
                {
                    slot.last_exit_code = status.exit_code;
                    break;
                }
                Ok(_) => rt::yield_current()?,
                Err(_) => break,
            }
        }
    }
    crate::util::close_slot_handles(slot);
    slot.phase = crate::state::ServicePhase::Exited;
    slot.blocked_dependency = rt::ServiceId::RootManager;
    slot.next_restart_tick = 0;
    slot.restart_requested = false;
    Ok(())
}

pub(crate) fn close_slot_for_failure(slot: &mut ServiceSlot) -> rt::Result<()> {
    stop_service_slot(slot)?;
    Ok(())
}

pub(super) fn send_lifecycle(control_handle: rt::Handle, event: LifecycleEvent) -> rt::Result<()> {
    let mut message = RawMessage::empty(ControlTag::Lifecycle as u32);
    message.word_count = 1;
    message.words[0] = event as u32 as u64;
    rt::channel_send(control_handle, &message)
}

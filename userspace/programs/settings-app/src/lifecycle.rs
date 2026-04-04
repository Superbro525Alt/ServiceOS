use serviceos_userspace_runtime as rt;
use rt::{AppKeyAction, AppPointerAction, ControlTag, LifecycleEvent};

pub(crate) fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
    let mut message = rt::RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut message) {
        Ok(()) if message.tag == ControlTag::Lifecycle as u32 && message.word_count > 0 => Ok(
            matches!(
                lifecycle_event_from_word(message.words[0]),
                LifecycleEvent::Restarting | LifecycleEvent::Stopped
            ),
        ),
        Ok(()) => Ok(false),
        Err(rt::Error::QueueEmpty) => Ok(false),
        Err(error) => Err(error),
    }
}

pub(crate) fn app_pointer_action_from_word(value: u64) -> Option<AppPointerAction> {
    match value as u32 {
        x if x == AppPointerAction::Down as u32 => Some(AppPointerAction::Down),
        x if x == AppPointerAction::Move as u32 => Some(AppPointerAction::Move),
        x if x == AppPointerAction::Up as u32 => Some(AppPointerAction::Up),
        _ => None,
    }
}

pub(crate) fn app_key_action_from_word(value: u64) -> Option<AppKeyAction> {
    match value as u32 {
        x if x == AppKeyAction::Down as u32 => Some(AppKeyAction::Down),
        x if x == AppKeyAction::Up as u32 => Some(AppKeyAction::Up),
        _ => None,
    }
}

fn lifecycle_event_from_word(value: u64) -> LifecycleEvent {
    match value as u32 {
        x if x == LifecycleEvent::Starting as u32 => LifecycleEvent::Starting,
        x if x == LifecycleEvent::Ready as u32 => LifecycleEvent::Ready,
        x if x == LifecycleEvent::Failed as u32 => LifecycleEvent::Failed,
        x if x == LifecycleEvent::Stopped as u32 => LifecycleEvent::Stopped,
        _ => LifecycleEvent::Restarting,
    }
}

use serviceos_userspace_runtime as rt;
use rt::{AppKeyAction, AppPointerAction, ControlTag, LifecycleEvent, RawMessage};

pub(crate) fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
    let mut message = RawMessage::empty(0);
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

pub(crate) fn pointer_action_from_word(value: u64) -> Option<AppPointerAction> {
    Some(match value as u32 {
        x if x == AppPointerAction::Down as u32 => AppPointerAction::Down,
        x if x == AppPointerAction::Move as u32 => AppPointerAction::Move,
        x if x == AppPointerAction::Up as u32 => AppPointerAction::Up,
        x if x == AppPointerAction::Scroll as u32 => AppPointerAction::Scroll,
        _ => return None,
    })
}

pub(crate) fn key_action_from_word(value: u64) -> Option<AppKeyAction> {
    Some(match value as u32 {
        x if x == AppKeyAction::Down as u32 => AppKeyAction::Down,
        x if x == AppKeyAction::Up as u32 => AppKeyAction::Up,
        _ => return None,
    })
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

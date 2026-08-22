use alloc::sync::Arc;
use serviceos_abi::{InputEventInfo, InputSourceInfo};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputSourceError {
    QueueEmpty,
    Busy,
    Unsupported,
}

/// Blocking receive primitive for an input source object.
///
/// Wait semantics mirror the other kernel objects (channel/event/packet): the
/// caller attempts a receive and, on `QueueEmpty`, blocks through the
/// scheduler's object-wait substrate (`SyscallAction::BlockCurrentThreadOnInputReceive`
/// -> `scheduler::block_current_on_input_receive`), woken by
/// `task::notify_input_ready` when the device IRQ path makes events pending.
///
/// Missed-edge guard: a wakeup can race the receiver's empty check (IRQ poll
/// completing between the final queue probe and the block decision). The IRQ
/// path latches wakeups in `InputCore`, so before reporting empty this
/// primitive consumes any latched wakeup and re-drains once - coalesced or
/// late arrivals can never strand behind a consumed notification.
pub trait InputBackend: Send + Sync {
    fn info(&self) -> InputSourceInfo;
    fn receive(&self) -> Result<InputEventInfo, InputSourceError>;
    fn poll(&self) -> bool;
}

pub struct InputSourceObject {
    backend: Arc<dyn InputBackend>,
}

impl InputSourceObject {
    pub fn new(backend: Arc<dyn InputBackend>) -> Self {
        Self { backend }
    }

    pub fn info(&self) -> InputSourceInfo {
        self.backend.info()
    }

    pub fn receive(&self) -> Result<InputEventInfo, InputSourceError> {
        match self.try_receive_with_fallback() {
            Ok(event) => Ok(event),
            Err(InputSourceError::QueueEmpty) => {
                if super::core::manager().is_some_and(|core| core.consume_wakeup(&self.backend)) {
                    return self.try_receive_with_fallback();
                }
                Err(InputSourceError::QueueEmpty)
            }
            Err(error) => Err(error),
        }
    }

    pub fn try_receive(&self) -> Result<InputEventInfo, InputSourceError> {
        self.backend.receive()
    }

    pub fn try_receive_with_fallback(&self) -> Result<InputEventInfo, InputSourceError> {
        match self.backend.receive() {
            Ok(event) => Ok(event),
            Err(InputSourceError::QueueEmpty) => {
                let _ = self.backend.poll();
                self.backend.receive()
            }
            Err(error) => Err(error),
        }
    }

    pub fn backend(&self) -> Arc<dyn InputBackend> {
        Arc::clone(&self.backend)
    }
}

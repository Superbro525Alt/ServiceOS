use alloc::sync::Arc;
use alloc::vec::Vec;
use serviceos_abi::{InputDeviceInfo, InputEventInfo, InputSourceInfo};

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

    /// Enumerates every physical input instance behind this source distinctly
    /// (id, class, semantic role flags, presence) instead of an aggregate
    /// device count. Empty for backends without per-instance tracking.
    fn enumerate_devices(&self) -> Vec<InputDeviceInfo> {
        Vec::new()
    }

    /// Marks a device instance absent after removal. A stale instance must
    /// stop producing events and must never wedge the poll pipeline; marking
    /// it present again restores routing.
    fn set_device_present(&self, _source_id: u32, _present: bool) {}
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

    /// Per-instance enumeration pass-through (multi-host visibility).
    pub fn enumerate_devices(&self) -> Vec<InputDeviceInfo> {
        self.backend.enumerate_devices()
    }

    /// Marks a device instance absent (hot-unplug): its events are ignored
    /// from then on and the pipeline keeps draining other hosts.
    pub fn mark_device_absent(&self, source_id: u32) {
        self.backend.set_device_present(source_id, false);
    }

    /// Restores a previously absent device instance.
    pub fn mark_device_present(&self, source_id: u32) {
        self.backend.set_device_present(source_id, true);
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

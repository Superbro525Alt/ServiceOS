use alloc::{sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::{Mutex, Once};

use super::InputBackend;

const MAX_REGISTERED_SOURCES: usize = 8;

struct RegisteredSource {
    object_id: u64,
    backend: Arc<dyn InputBackend>,
    /// Latched wakeup memory for the missed-edge guard: the IRQ/poll path sets
    /// this before notifying so a receiver that already observed an empty
    /// queue can detect a wakeup that raced its block decision.
    wakeup_pending: Arc<AtomicBool>,
}

impl RegisteredSource {
    fn matches_backend(&self, backend: &Arc<dyn InputBackend>) -> bool {
        Arc::ptr_eq(&self.backend, backend)
    }
}

pub struct InputCore {
    sources: Mutex<Vec<RegisteredSource>>,
}

impl InputCore {
    fn new() -> Self {
        Self {
            sources: Mutex::new(Vec::new()),
        }
    }

    pub fn register_source(&self, object_id: u64, backend: Arc<dyn InputBackend>) -> bool {
        let mut sources = self.sources.lock();
        if sources.len() >= MAX_REGISTERED_SOURCES
            || sources.iter().any(|entry| entry.object_id == object_id)
        {
            return false;
        }
        sources.push(RegisteredSource {
            object_id,
            backend,
            wakeup_pending: Arc::new(AtomicBool::new(false)),
        });
        true
    }

    /// Drives one backend poll pass. A source is treated as wake-worthy while
    /// it reports pending events, not only on the 0 -> nonempty transition:
    /// coalesced arrivals that land between two polls must still produce a
    /// notification (and a latched wakeup) or a receiver that just observed an
    /// empty queue would sleep through them until the next device edge.
    pub fn poll_ready<F>(&self, mut notify_ready: F)
    where
        F: FnMut(u64),
    {
        let sources = self.sources.lock();
        for source in sources.iter() {
            let became_ready = source.backend.poll();
            let pending = source.backend.info().pending_events > 0;
            if became_ready || pending {
                source.wakeup_pending.store(true, Ordering::Release);
                notify_ready(source.object_id);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn latch_wakeup(&self, backend: &Arc<dyn InputBackend>) {
        if let Some(source) = self.sources.lock().iter().find(|s| s.matches_backend(backend)) {
            source.wakeup_pending.store(true, Ordering::Release);
        }
    }

    #[cfg(test)]
    pub(crate) fn latch_peek(&self, backend: &Arc<dyn InputBackend>) -> bool {
        self.lookup_latch(backend).is_some_and(|latch| latch.load(Ordering::Acquire))
    }

    /// Consumes the latched wakeup for `backend`, returning true when at least
    /// one notification arrived since the last consumption. Used by the
    /// blocking receive path to re-drain after a raced wakeup instead of
    /// reporting a missed-edge empty queue.
    pub(crate) fn consume_wakeup(&self, backend: &Arc<dyn InputBackend>) -> bool {
        self.lookup_latch(backend)
            .is_some_and(|latch| latch.swap(false, Ordering::AcqRel))
    }

    fn lookup_latch(&self, backend: &Arc<dyn InputBackend>) -> Option<Arc<AtomicBool>> {
        self.sources
            .lock()
            .iter()
            .find(|source| source.matches_backend(backend))
            .map(|source| Arc::clone(&source.wakeup_pending))
    }

    #[cfg(test)]
    pub(crate) fn register_test_source_for_latch(
        &self,
        object_id: u64,
        backend: Arc<dyn InputBackend>,
    ) -> bool {
        self.register_source(object_id, backend)
    }

    #[cfg(test)]
    pub(crate) fn poll_ready_for_test<F>(&self, _backend: Arc<dyn InputBackend>, notify: F)
    where
        F: FnMut(u64),
    {
        self.poll_ready(notify);
    }
}

static INPUT_CORE: Once<InputCore> = Once::new();

pub fn initialize() -> &'static InputCore {
    INPUT_CORE.call_once(InputCore::new)
}

pub fn manager() -> Option<&'static InputCore> {
    INPUT_CORE.get()
}

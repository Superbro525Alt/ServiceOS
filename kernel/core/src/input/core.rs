use alloc::{sync::Arc, vec::Vec};
use spin::{Mutex, Once};

use super::InputBackend;

const MAX_REGISTERED_SOURCES: usize = 8;

#[derive(Clone)]
struct RegisteredSource {
    object_id: u64,
    backend: Arc<dyn InputBackend>,
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
        sources.push(RegisteredSource { object_id, backend });
        true
    }

    pub fn poll_ready<F>(&self, mut notify_ready: F)
    where
        F: FnMut(u64),
    {
        let sources = self.sources.lock();
        for source in sources.iter() {
            if source.backend.poll() {
                notify_ready(source.object_id);
            }
        }
    }
}

static INPUT_CORE: Once<InputCore> = Once::new();

pub fn initialize() -> &'static InputCore {
    INPUT_CORE.call_once(InputCore::new)
}

pub fn manager() -> Option<&'static InputCore> {
    INPUT_CORE.get()
}

use alloc::{sync::Arc, vec::Vec};
use serviceos_abi::InputSourceInfo;
use spin::{Mutex, Once};

const MAX_REGISTERED_SOURCES: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputSourceError {
    QueueEmpty,
    Busy,
    Unsupported,
}

pub trait InputBackend: Send + Sync {
    fn info(&self) -> InputSourceInfo;
    fn receive(&self) -> Result<serviceos_abi::InputEventInfo, InputSourceError>;
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

    pub fn receive(&self) -> Result<serviceos_abi::InputEventInfo, InputSourceError> {
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

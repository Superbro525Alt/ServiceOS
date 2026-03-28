use alloc::{sync::Arc, vec::Vec};
use spin::{Mutex, Once};

use super::PacketBackend;

const MAX_REGISTERED_INTERFACES: usize = 8;

#[derive(Clone)]
struct RegisteredInterface {
    object_id: u64,
    backend: Arc<dyn PacketBackend>,
}

pub struct NetworkCore {
    interfaces: Mutex<Vec<RegisteredInterface>>,
}

impl NetworkCore {
    pub fn new() -> Self {
        Self {
            interfaces: Mutex::new(Vec::new()),
        }
    }

    pub fn register_interface(&self, object_id: u64, backend: Arc<dyn PacketBackend>) -> bool {
        let mut interfaces = self.interfaces.lock();
        if interfaces.len() >= MAX_REGISTERED_INTERFACES
            || interfaces.iter().any(|entry| entry.object_id == object_id)
        {
            return false;
        }
        interfaces.push(RegisteredInterface { object_id, backend });
        true
    }

    pub fn poll_ready<F>(&self, mut notify_ready: F)
    where
        F: FnMut(u64),
    {
        let interfaces = self.interfaces.lock();
        for interface in interfaces.iter() {
            if interface.backend.poll() {
                notify_ready(interface.object_id);
            }
        }
    }
}

static NETWORK_CORE: Once<NetworkCore> = Once::new();

pub fn initialize() -> &'static NetworkCore {
    NETWORK_CORE.call_once(NetworkCore::new)
}

pub fn manager() -> Option<&'static NetworkCore> {
    NETWORK_CORE.get()
}

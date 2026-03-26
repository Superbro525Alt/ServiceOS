use alloc::{sync::Arc, vec::Vec};
use serviceos_abi::PacketInterfaceInfo;
use spin::{Mutex, Once};

const MAX_REGISTERED_INTERFACES: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PacketInterfaceError {
    QueueEmpty,
    BufferTooSmall,
    Busy,
    Unsupported,
}

pub trait PacketBackend: Send + Sync {
    fn info(&self) -> PacketInterfaceInfo;
    fn transmit(&self, frame: &[u8]) -> Result<(), PacketInterfaceError>;
    fn receive(&self, buffer: &mut [u8]) -> Result<usize, PacketInterfaceError>;
    fn poll(&self) -> bool;
}

pub struct PacketInterfaceObject {
    backend: Arc<dyn PacketBackend>,
}

impl PacketInterfaceObject {
    pub fn new(backend: Arc<dyn PacketBackend>) -> Self {
        Self { backend }
    }

    pub fn info(&self) -> PacketInterfaceInfo {
        self.backend.info()
    }

    pub fn transmit(&self, frame: &[u8]) -> Result<(), PacketInterfaceError> {
        self.backend.transmit(frame)
    }

    pub fn receive(&self, buffer: &mut [u8]) -> Result<usize, PacketInterfaceError> {
        self.backend.receive(buffer)
    }

    pub fn backend(&self) -> Arc<dyn PacketBackend> {
        Arc::clone(&self.backend)
    }
}

#[derive(Clone)]
struct RegisteredInterface {
    object_id: u64,
    backend: Arc<dyn PacketBackend>,
}

pub struct NetworkCore {
    interfaces: Mutex<Vec<RegisteredInterface>>,
}

impl NetworkCore {
    fn new() -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicBool, Ordering};
    use serviceos_abi::{PacketInterfaceBackend, PacketInterfaceLinkState};

    struct FakeBackend {
        polled: AtomicBool,
    }

    impl PacketBackend for FakeBackend {
        fn info(&self) -> PacketInterfaceInfo {
            PacketInterfaceInfo {
                backend: PacketInterfaceBackend::Unknown as u32,
                link_state: PacketInterfaceLinkState::Up as u32,
                mtu: 1500,
                rx_ready: 0,
                mac: [0; 6],
                reserved: [0; 2],
                rx_packets: 0,
                tx_packets: 0,
                dropped_packets: 0,
            }
        }

        fn transmit(&self, _frame: &[u8]) -> Result<(), PacketInterfaceError> {
            Ok(())
        }

        fn receive(&self, _buffer: &mut [u8]) -> Result<usize, PacketInterfaceError> {
            Err(PacketInterfaceError::QueueEmpty)
        }

        fn poll(&self) -> bool {
            !self.polled.swap(true, Ordering::SeqCst)
        }
    }

    #[test]
    fn packet_interface_registration_is_bounded() {
        let core = NetworkCore::new();
        let backend = Arc::new(FakeBackend {
            polled: AtomicBool::new(false),
        });

        assert!(core.register_interface(1, backend));
        assert!(!core.register_interface(
            1,
            Arc::new(FakeBackend {
                polled: AtomicBool::new(false),
            })
        ));
    }
}

mod backend;
mod core;
pub mod ring;

pub use backend::{PacketBackend, PacketInterfaceError, PacketInterfaceObject};
pub use core::{NetworkCore, initialize, manager};

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::sync::atomic::{AtomicBool, Ordering};
    use alloc::sync::Arc;
    use serviceos_abi::{PacketInterfaceBackend, PacketInterfaceInfo, PacketInterfaceLinkState};

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

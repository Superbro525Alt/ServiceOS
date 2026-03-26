use alloc::{collections::VecDeque, sync::Arc, vec::Vec};
use core::ptr::NonNull;

use serviceos_abi::{PacketInterfaceBackend, PacketInterfaceInfo, PacketInterfaceLinkState};
use serviceos_kernel_core::{
    memory::{self, PageMapper, PhysicalAddress, VirtualAddress},
    network::{PacketBackend, PacketInterfaceError},
    object::ObjectId,
    task,
};
use spin::{Mutex, Once};
use virtio_drivers::{
    BufferDirection, Hal, PAGE_SIZE,
    device::net::VirtIONet,
    transport::pci::{
        PciTransport,
        bus::{Command, ConfigurationAccess, DeviceFunction, HeaderType, PciRoot},
        virtio_device_type,
    },
};
use x86_64::instructions::port::Port;

use crate::paging::ActivePageTable;

const PCI_CONFIG_ADDRESS_PORT: u16 = 0xCF8;
const PCI_CONFIG_DATA_PORT: u16 = 0xCFC;
const NETWORK_QUEUE_SIZE: usize = 8;
const NETWORK_BUFFER_BYTES: usize = 1536;
const MAX_RECEIVE_QUEUE: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkBringupSummary {
    pub backend: PacketInterfaceBackend,
    pub pci_bus: u8,
    pub pci_device: u8,
    pub pci_function: u8,
    pub mtu: u32,
    pub mac: [u8; 6],
}

pub fn initialize() -> Option<Arc<dyn PacketBackend>> {
    let mut root = PciRoot::new(IoPortPciConfigAccess);
    for bus in 0u16..=255 {
        for (device_function, info) in root.enumerate_bus(bus as u8) {
            if info.header_type != HeaderType::Standard {
                continue;
            }
            if virtio_device_type(&info) != Some(virtio_drivers::transport::DeviceType::Network) {
                continue;
            }

            let mut command = root.get_status_command(device_function).1;
            command.insert(Command::BUS_MASTER | Command::MEMORY_SPACE);
            root.set_command(device_function, command);

            let transport = PciTransport::new::<KernelHal, _>(&mut root, device_function).ok()?;
            let device =
                VirtIONet::<KernelHal, _, NETWORK_QUEUE_SIZE>::new(transport, NETWORK_BUFFER_BYTES)
                    .ok()?;
            let mac = device.mac_address();
            let backend = Arc::new(VirtioPacketBackend::new(device, mac));
            let _ = BRINGUP_SUMMARY.call_once(|| NetworkBringupSummary {
                backend: PacketInterfaceBackend::VirtioPci,
                pci_bus: device_function.bus,
                pci_device: device_function.device,
                pci_function: device_function.function,
                mtu: 1500,
                mac,
            });
            return Some(backend);
        }
    }

    None
}

pub fn bringup_summary() -> Option<NetworkBringupSummary> {
    BRINGUP_SUMMARY.get().copied()
}

pub fn poll_ready_interfaces() {
    if let Some(manager) = serviceos_kernel_core::network::manager() {
        manager.poll_ready(|object_id| {
            let _ = task::notify_packet_ready(ObjectId(object_id));
        });
    }
}

static BRINGUP_SUMMARY: Once<NetworkBringupSummary> = Once::new();

struct VirtioPacketBackend {
    state: Mutex<VirtioPacketState>,
}

struct VirtioPacketState {
    device: VirtIONet<KernelHal, PciTransport, NETWORK_QUEUE_SIZE>,
    mac: [u8; 6],
    mtu: u32,
    receive_queue: VecDeque<Vec<u8>>,
    rx_packets: u64,
    tx_packets: u64,
    dropped_packets: u64,
}

impl VirtioPacketBackend {
    fn new(device: VirtIONet<KernelHal, PciTransport, NETWORK_QUEUE_SIZE>, mac: [u8; 6]) -> Self {
        Self {
            state: Mutex::new(VirtioPacketState {
                device,
                mac,
                mtu: 1500,
                receive_queue: VecDeque::new(),
                rx_packets: 0,
                tx_packets: 0,
                dropped_packets: 0,
            }),
        }
    }
}

impl PacketBackend for VirtioPacketBackend {
    fn info(&self) -> PacketInterfaceInfo {
        let state = self.state.lock();
        PacketInterfaceInfo {
            backend: PacketInterfaceBackend::VirtioPci as u32,
            link_state: PacketInterfaceLinkState::Up as u32,
            mtu: state.mtu,
            rx_ready: u32::from(!state.receive_queue.is_empty()),
            mac: state.mac,
            reserved: [0; 2],
            rx_packets: state.rx_packets,
            tx_packets: state.tx_packets,
            dropped_packets: state.dropped_packets,
        }
    }

    fn transmit(&self, frame: &[u8]) -> Result<(), PacketInterfaceError> {
        if frame.is_empty() || frame.len() > NETWORK_BUFFER_BYTES {
            return Err(PacketInterfaceError::BufferTooSmall);
        }

        let mut state = self.state.lock();
        if !state.device.can_send() {
            return Err(PacketInterfaceError::Busy);
        }

        let mut tx = state.device.new_tx_buffer(frame.len());
        tx.packet_mut().copy_from_slice(frame);
        state
            .device
            .send(tx)
            .map_err(|_| PacketInterfaceError::Busy)?;
        state.tx_packets = state.tx_packets.saturating_add(1);
        Ok(())
    }

    fn receive(&self, buffer: &mut [u8]) -> Result<usize, PacketInterfaceError> {
        let mut state = self.state.lock();
        let Some(frame) = state.receive_queue.pop_front() else {
            return Err(PacketInterfaceError::QueueEmpty);
        };
        if frame.len() > buffer.len() {
            state.receive_queue.push_front(frame);
            return Err(PacketInterfaceError::BufferTooSmall);
        }
        buffer[..frame.len()].copy_from_slice(&frame);
        Ok(frame.len())
    }

    fn poll(&self) -> bool {
        let mut state = self.state.lock();
        let mut became_ready = false;

        while state.device.can_recv() {
            match state.device.receive() {
                Ok(rx) => {
                    let frame = rx.packet().to_vec();
                    let _ = state.device.recycle_rx_buffer(rx);
                    if state.receive_queue.is_empty() {
                        became_ready = true;
                    }
                    if state.receive_queue.len() == MAX_RECEIVE_QUEUE {
                        state.receive_queue.pop_front();
                        state.dropped_packets = state.dropped_packets.saturating_add(1);
                    }
                    state.receive_queue.push_back(frame);
                    state.rx_packets = state.rx_packets.saturating_add(1);
                }
                Err(_) => break,
            }
        }

        became_ready
    }
}

#[derive(Clone, Copy)]
struct IoPortPciConfigAccess;

impl ConfigurationAccess for IoPortPciConfigAccess {
    fn read_word(&self, device_function: DeviceFunction, register_offset: u8) -> u32 {
        let address = pci_config_address(device_function, register_offset);
        let mut address_port = Port::<u32>::new(PCI_CONFIG_ADDRESS_PORT);
        let mut data_port = Port::<u32>::new(PCI_CONFIG_DATA_PORT);

        unsafe {
            address_port.write(address);
            data_port.read()
        }
    }

    fn write_word(&mut self, device_function: DeviceFunction, register_offset: u8, data: u32) {
        let address = pci_config_address(device_function, register_offset);
        let mut address_port = Port::<u32>::new(PCI_CONFIG_ADDRESS_PORT);
        let mut data_port = Port::<u32>::new(PCI_CONFIG_DATA_PORT);

        unsafe {
            address_port.write(address);
            data_port.write(data);
        }
    }

    unsafe fn unsafe_clone(&self) -> Self {
        *self
    }
}

fn pci_config_address(device_function: DeviceFunction, register_offset: u8) -> u32 {
    0x8000_0000
        | ((device_function.bus as u32) << 16)
        | ((device_function.device as u32) << 11)
        | ((device_function.function as u32) << 8)
        | (register_offset as u32 & 0xfc)
}

struct KernelHal;

unsafe impl Hal for KernelHal {
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (u64, NonNull<u8>) {
        let Some(memory) = memory::manager() else {
            return (0, NonNull::dangling());
        };
        let mut allocator = memory.frame_allocator().lock();
        let Some(first) = allocator.allocate_4kib() else {
            return (0, NonNull::dangling());
        };
        let base = first.base.as_u64();
        unsafe {
            core::ptr::write_bytes(base as *mut u8, 0, PAGE_SIZE);
        }

        for page in 1..pages {
            let Some(next) = allocator.allocate_4kib() else {
                return (0, NonNull::dangling());
            };
            if next.base.as_u64() != base + (page as u64 * PAGE_SIZE as u64) {
                return (0, NonNull::dangling());
            }
            unsafe {
                core::ptr::write_bytes(next.base.as_u64() as *mut u8, 0, PAGE_SIZE);
            }
        }

        (
            base,
            NonNull::new(base as *mut u8).unwrap_or(NonNull::dangling()),
        )
    }

    unsafe fn dma_dealloc(_paddr: u64, _vaddr: NonNull<u8>, _pages: usize) -> i32 {
        0
    }

    unsafe fn mmio_phys_to_virt(paddr: u64, _size: usize) -> NonNull<u8> {
        NonNull::new(paddr as *mut u8).unwrap_or(NonNull::dangling())
    }

    unsafe fn share(buffer: NonNull<[u8]>, _direction: BufferDirection) -> u64 {
        translate_kernel_pointer(buffer.as_ptr().cast::<u8>() as u64)
            .map(PhysicalAddress::as_u64)
            .unwrap_or(0)
    }

    unsafe fn unshare(_paddr: u64, _buffer: NonNull<[u8]>, _direction: BufferDirection) {}
}

fn translate_kernel_pointer(virtual_address: u64) -> Option<PhysicalAddress> {
    let mapper = unsafe { ActivePageTable::new_identity_mapped() };
    mapper.translate(VirtualAddress::new(virtual_address))
}

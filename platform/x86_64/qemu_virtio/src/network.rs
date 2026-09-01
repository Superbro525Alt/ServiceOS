use alloc::sync::Arc;
use core::ptr::NonNull;

use serviceos_abi::{PacketInterfaceBackend, PacketInterfaceInfo, PacketInterfaceLinkState};
use serviceos_kernel_arch_x86_64::{interrupts, paging::ActivePageTable};
use serviceos_kernel_core::{
    memory::{self, PageMapper, PhysicalAddress, VirtualAddress},
    msi,
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
        bus::{Command, ConfigurationAccess, DeviceFunction, HeaderType, PCI_CAP_ID_VNDR, PciRoot},
        virtio_device_type,
    },
};
use x86_64::instructions::port::Port;

const PCI_CONFIG_ADDRESS_PORT: u16 = 0xCF8;
const PCI_CONFIG_DATA_PORT: u16 = 0xCFC;
const NETWORK_QUEUE_SIZE: usize = 8;
const NETWORK_BUFFER_BYTES: usize = 1536;
const MAX_RECEIVE_QUEUE: usize = 32;

/// Build-time opt-out for the MSI-X interrupt model (SERVICEOS_MSIX_DISABLE
/// makes the NIC fall back to the legacy INT#x line exactly as before).
const MSI_X_DISABLED: bool = option_env!("SERVICEOS_MSIX_DISABLE").is_some();

/// Common-config structure field offsets (virtio 1.0 4.1.4.3 layout, as
/// mirrored by virtio-drivers' `CommonCfg`). The crate keeps those fields
/// `pub(crate)`, so queue-vector assignment goes through raw identity-mapped
/// MMIO instead of the public transport API.
const COMMON_CFG_NUM_QUEUES: u64 = 0x12;
const COMMON_CFG_QUEUE_SELECT: u64 = 0x16;
const COMMON_CFG_QUEUE_MSIX_VECTOR: u64 = 0x1a;
/// Common-config vector value meaning "no MSI-X signal" (config-change
/// events stay disabled for v0; see the bring-up comment).
const COMMON_CFG_NO_VECTOR: u16 = 0xffff;

/// How the NIC's interrupts reach the kernel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkInterruptModel {
    /// Legacy INT#x pin emulation through the external (PIC) line.
    Legacy(u8),
    /// MSI-X: message-signaled interrupt on an arch MSI vector, delivered
    /// through the LAPIC (no external controller involvement).
    Msix(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkBringupSummary {
    pub backend: PacketInterfaceBackend,
    pub pci_bus: u8,
    pub pci_device: u8,
    pub pci_function: u8,
    pub interrupt: NetworkInterruptModel,
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

            // Interrupt model: prefer MSI-X (message-signaled, LAPIC-delivered)
            // and fall back to the legacy INT#x line when the device or build
            // config doesn't provide it. Every fallible MSI-X step happens
            // before the device is enabled, so a `None` return leaves the
            // device untouched and the legacy path clean.
            let interrupt = match try_setup_msix(&mut root, device_function) {
                Some(vector) => {
                    if !interrupts::register_msi_vector_handler(0, handle_network_irq) {
                        return None;
                    }
                    NetworkInterruptModel::Msix(vector)
                }
                None => {
                    let interrupt_line = read_interrupt_line(device_function)?;
                    if !interrupts::register_external_irq_handler(
                        interrupt_line,
                        handle_network_irq,
                    ) {
                        return None;
                    }
                    NetworkInterruptModel::Legacy(interrupt_line)
                }
            };

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
                interrupt,
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

fn handle_network_irq(_irq_line: u8) {
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
    receive_queue: ReceiveQueue,
    rx_packets: u64,
    tx_packets: u64,
    dropped_packets: u64,
}

struct ReceiveQueue {
    slots: [[u8; NETWORK_BUFFER_BYTES]; MAX_RECEIVE_QUEUE],
    lengths: [usize; MAX_RECEIVE_QUEUE],
    head: usize,
    tail: usize,
    len: usize,
}

impl ReceiveQueue {
    const fn new() -> Self {
        Self {
            slots: [[0; NETWORK_BUFFER_BYTES]; MAX_RECEIVE_QUEUE],
            lengths: [0; MAX_RECEIVE_QUEUE],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    const fn is_empty(&self) -> bool {
        self.len == 0
    }

    const fn is_full(&self) -> bool {
        self.len == MAX_RECEIVE_QUEUE
    }

    fn pop_front_into(&mut self, buffer: &mut [u8]) -> Result<usize, PacketInterfaceError> {
        if self.is_empty() {
            return Err(PacketInterfaceError::QueueEmpty);
        }

        let frame_len = self.lengths[self.head];
        if frame_len > buffer.len() {
            return Err(PacketInterfaceError::BufferTooSmall);
        }

        buffer[..frame_len].copy_from_slice(&self.slots[self.head][..frame_len]);
        self.lengths[self.head] = 0;
        self.head = (self.head + 1) % MAX_RECEIVE_QUEUE;
        self.len -= 1;
        Ok(frame_len)
    }

    fn push_copy(&mut self, frame: &[u8]) -> Result<(), PacketInterfaceError> {
        if frame.len() > NETWORK_BUFFER_BYTES {
            return Err(PacketInterfaceError::BufferTooSmall);
        }

        if self.is_full() {
            self.head = (self.head + 1) % MAX_RECEIVE_QUEUE;
            self.len -= 1;
        }

        self.slots[self.tail][..frame.len()].copy_from_slice(frame);
        self.lengths[self.tail] = frame.len();
        self.tail = (self.tail + 1) % MAX_RECEIVE_QUEUE;
        self.len += 1;
        Ok(())
    }
}

impl VirtioPacketBackend {
    fn new(device: VirtIONet<KernelHal, PciTransport, NETWORK_QUEUE_SIZE>, mac: [u8; 6]) -> Self {
        Self {
            state: Mutex::new(VirtioPacketState {
                device,
                mac,
                mtu: 1500,
                receive_queue: ReceiveQueue::new(),
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

    /// Copy strategy (documented per the shared TX ring design): this
    /// backend COPIES each frame into a driver-owned TxBuffer before
    /// handing it to the virtio queue. Mapping a userspace slot page
    /// directly into a virtio descriptor is not possible with the current
    /// virtio-drivers API (`VirtIONet::send` accepts only buffers it
    /// allocated via `new_tx_buffer`), so the shared TX ring eliminates the
    /// per-frame IPC/syscall copy while this one slot→desc copy remains.
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
        state.receive_queue.pop_front_into(buffer)
    }

    fn poll(&self) -> bool {
        let mut state = self.state.lock();
        let mut became_ready = false;

        // Read-and-clear the ISR status so a level-triggered PCI interrupt
        // deasserts and can fire again for packets that arrive later. Without
        // this the line stays stuck asserted after the first delivery and
        // every subsequent inbound frame is silently lost.
        let _ = state.device.ack_interrupt();

        while state.device.can_recv() {
            match state.device.receive() {
                Ok(rx) => {
                    if state.receive_queue.is_empty() {
                        became_ready = true;
                    }
                    let queue_result = state.receive_queue.push_copy(rx.packet());
                    let _ = state.device.recycle_rx_buffer(rx);
                    if queue_result.is_err() {
                        state.dropped_packets = state.dropped_packets.saturating_add(1);
                        continue;
                    }
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

fn read_interrupt_line(device_function: DeviceFunction) -> Option<u8> {
    let value = IoPortPciConfigAccess.read_word(device_function, 0x3c) as u8;
    if value == 0 || value == 0xff {
        None
    } else {
        Some(value)
    }
}

/// Diagnostics for the last skipped MSI-X bring-up (printed once by the
/// image's network summary; temporary instrumentation).
pub static MSIX_SETUP_DIAG: Once<&'static str> = Once::new();

fn msix_diag(reason: &'static str) -> Option<u8> {
    let _ = MSIX_SETUP_DIAG.call_once(|| reason);
    None
}

/// Configure the virtio NIC for MSI-X delivery and return the arch vector
/// slot's vector number, or `None` when MSI-X is unavailable (build opt-out,
/// missing capability, missing table BAR, or missing common-config region).
///
/// Sequence (PCI Local Bus spec 6.8 / virtio 1.0 4.1.4):
///   1. parse the MSI-X capability (cap id 0x11) for the table BIR/offset
///   2. locate the MSI-X table BAR and the virtio common-config region
///   3. program table entry 0 with the LAPIC message (masked)
///   4. set MSI-X Enable (Function Mask stays clear; every vector starts
///      masked so the device cannot signal mid-setup)
///   5. point every virtio queue at MSI-X vector 0 in the common config
///      (the config-change vector stays NO_QUEUE: link/MAC config events
///      are not delivered — documented v0 limitation)
///   6. unmask entry 0
///   7. set the PCI Command INTx-Disable bit so the legacy pin route cannot
///      race the message-signaled path
///
/// All fallible steps precede step 4, so a `None` return never leaves the
/// device half-switched over. The handler registration happens in the caller
/// before `PciTransport::new` lets the device negotiate.
fn try_setup_msix(
    root: &mut PciRoot<IoPortPciConfigAccess>,
    device_function: DeviceFunction,
) -> Option<u8> {
    if MSI_X_DISABLED {
        return None;
    }

    let mut cam = IoPortPciConfigAccess;

    let cap_offset = match root
        .capabilities(device_function)
        .find(|capability| capability.id == msi::MSI_X_CAP_ID)
    {
        Some(capability) => capability.offset,
        None => return msix_diag("no-msix-cap"),
    };
    let cap = match msi::parse_msix_capability(
        |offset| cam.read_word(device_function, offset),
        cap_offset,
    ) {
        Some(cap) => cap,
        None => return msix_diag("cap-parse"),
    };
    if cap.table_size() < 1 {
        return msix_diag("table-size-0");
    }

    let (bar_address, _) = match root
        .bar_info(device_function, cap.table_bir)
        .ok()
        .and_then(|bar| bar.and_then(|b| b.memory_address_size()))
    {
        Some(address) => address,
        None => return msix_diag("table-bar"),
    };
    let table_base = bar_address + u64::from(cap.table_offset);

    let common_base = match locate_common_config(&mut cam, root, device_function) {
        Some(base) => base,
        None => return msix_diag("no-common-cfg"),
    };

    // Program entry 0 masked: LAPIC physical destination 0 (BSP), fixed
    // delivery, edge trigger, on the arch MSI vector.
    let entry = msi::MsixTableEntry::new_edge_fixed(0, interrupts::MSI_VECTOR_BASE, true);
    write_msix_table_entry(table_base, 0, entry);

    // Enable MSI-X with every vector still masked.
    let header = cam.read_word(device_function, cap_offset);
    let control = (header >> 16) as u16 | msi::MSI_X_MSG_CTRL_ENABLE;
    cam.write_word(
        device_function,
        cap_offset,
        (header & 0x0000_ffff) | (u32::from(control) << 16),
    );

    // Assign all virtio queues to MSI-X vector 0 (config + queues share one
    // vector for v0).
    let num_queues = read_common_config(common_base, COMMON_CFG_NUM_QUEUES);
    for queue in 0..num_queues {
        write_common_config(common_base, COMMON_CFG_QUEUE_SELECT, queue);
        write_common_config(common_base, COMMON_CFG_QUEUE_MSIX_VECTOR, 0);
    }
    write_common_config(common_base, 0x10, COMMON_CFG_NO_VECTOR); // msix_config: config-change events stay off

    // Unmask entry 0: the device may now signal via the LAPIC.
    write_msix_table_entry(
        table_base,
        0,
        msi::MsixTableEntry {
            address_lower: entry.address_lower,
            address_upper: entry.address_upper,
            data: entry.data,
            vector_control: 0,
        },
    );

    // Kill the legacy INT#x pin route so interrupts arrive ONLY via MSI-X.
    let (_status, mut command) = root.get_status_command(device_function);
    command.insert(Command::INTERRUPT_DISABLE);
    root.set_command(device_function, command);

    Some(interrupts::MSI_VECTOR_BASE)
}

/// Find the virtio common-config structure's physical address by walking the
/// vendor capabilities (same shape `PciTransport::new` consumes).
fn locate_common_config(
    cam: &mut IoPortPciConfigAccess,
    root: &mut PciRoot<IoPortPciConfigAccess>,
    device_function: DeviceFunction,
) -> Option<u64> {
    for capability in root.capabilities(device_function) {
        if capability.id != PCI_CAP_ID_VNDR {
            continue;
        }
        // Bytes 2-3 of the capability header (carried in private_header by
        // the crate's iterator): struct length, then config type. The port-I/O
        // CAM only does dword access, so these bytes cannot be fetched with a
        // standalone word read at offset+2.
        let cap_len = capability.private_header as u8;
        let cfg_type = (capability.private_header >> 8) as u8;
        if cap_len < 16 || cfg_type != VIRTIO_PCI_CAP_COMMON_CFG {
            continue;
        }
        let bar = cam.read_word(device_function, capability.offset + 4) as u8;
        let bar_offset = cam.read_word(device_function, capability.offset + 8);
        let (bar_address, _) = root
            .bar_info(device_function, bar)
            .ok()?
            .and_then(|bar| bar.memory_address_size())?;
        return Some(bar_address + u64::from(bar_offset));
    }
    None
}

const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;

unsafe fn write_mmio_u32(address: u64, value: u32) {
    unsafe {
        core::ptr::write_volatile(address as *mut u32, value);
    }
}

unsafe fn read_mmio_u16(address: u64) -> u16 {
    unsafe { core::ptr::read_volatile(address as *const u16) }
}

unsafe fn write_mmio_u16(address: u64, value: u16) {
    unsafe {
        core::ptr::write_volatile(address as *mut u16, value);
    }
}

fn write_msix_table_entry(table_base: u64, index: u16, entry: msi::MsixTableEntry) {
    let entry_base = table_base + u64::from(msi::msix_table_entry_offset(index));
    let words = entry.to_words();
    unsafe {
        write_mmio_u32(entry_base, words[0]);
        write_mmio_u32(entry_base + 4, words[1]);
        write_mmio_u32(entry_base + 8, words[2]);
        // Vector control last: an unmask write takes effect only after the
        // address/data words are in place.
        write_mmio_u32(entry_base + 12, words[3]);
    }
}

fn read_common_config(base: u64, offset: u64) -> u16 {
    unsafe { read_mmio_u16(base + offset) }
}

fn write_common_config(base: u64, offset: u64, value: u16) {
    unsafe { write_mmio_u16(base + offset, value) }
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

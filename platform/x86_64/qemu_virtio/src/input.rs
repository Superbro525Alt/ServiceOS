use alloc::{sync::Arc, vec::Vec};
use core::ptr::NonNull;

use serviceos_abi::{
    InputButton, InputEventInfo, InputEventKind, InputSourceBackend, InputSourceInfo,
    input_capability,
};
use serviceos_kernel_arch_x86_64::paging::ActivePageTable;
use serviceos_kernel_core::{
    input::{InputBackend, InputSourceError},
    memory::{self, PageMapper, PhysicalAddress, VirtualAddress},
    object::ObjectId,
    task,
};
use spin::{Mutex, Once};
use virtio_drivers::{
    BufferDirection, Hal, PAGE_SIZE,
    device::input::{InputEvent, VirtIOInput},
    transport::pci::{
        PciTransport,
        bus::{Command, ConfigurationAccess, DeviceFunction, HeaderType, PciRoot},
        virtio_device_type,
    },
};
use x86_64::instructions::port::Port;

const PCI_CONFIG_ADDRESS_PORT: u16 = 0xCF8;
const PCI_CONFIG_DATA_PORT: u16 = 0xCFC;
const MAX_PENDING_EVENTS: usize = 128;

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const EV_ABS: u16 = 0x03;

const SYN_REPORT: u16 = 0x00;
const REL_X: u16 = 0x00;
const REL_Y: u16 = 0x01;
const REL_WHEEL: u16 = 0x08;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;

const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputBringupSummary {
    pub backend: InputSourceBackend,
    pub keyboard_devices: u32,
    pub pointer_devices: u32,
}

pub fn initialize() -> Option<Arc<dyn InputBackend>> {
    let mut root = PciRoot::new(IoPortPciConfigAccess);
    let mut devices = Vec::new();
    let mut keyboard_devices = 0u32;
    let mut pointer_devices = 0u32;

    for bus in 0u16..=255 {
        for (device_function, info) in root.enumerate_bus(bus as u8) {
            if info.header_type != HeaderType::Standard {
                continue;
            }
            if virtio_device_type(&info) != Some(virtio_drivers::transport::DeviceType::Input) {
                continue;
            }

            let mut command = root.get_status_command(device_function).1;
            command.insert(Command::BUS_MASTER | Command::MEMORY_SPACE);
            root.set_command(device_function, command);

            let transport = PciTransport::new::<KernelHal, _>(&mut root, device_function).ok()?;
            let mut device = VirtIOInput::<KernelHal, _>::new(transport).ok()?;
            let pointer = pointer_axes(&mut device).ok().flatten();
            let has_keys = device
                .ev_bits(EV_KEY as u8)
                .ok()
                .is_some_and(|bits| !bits.is_empty());

            if pointer.is_some() {
                pointer_devices = pointer_devices.saturating_add(1);
            }
            if has_keys {
                keyboard_devices = keyboard_devices.saturating_add(1);
            }

            devices.push(InputDeviceState {
                device,
                pointer,
                keyboard: has_keys,
                pending_x: 0,
                pending_y: 0,
                motion_dirty: false,
            });
        }
    }

    if devices.is_empty() {
        return None;
    }

    let _ = BRINGUP_SUMMARY.call_once(|| InputBringupSummary {
        backend: InputSourceBackend::VirtioPci,
        keyboard_devices,
        pointer_devices,
    });

    Some(Arc::new(VirtioInputBackend::new(
        devices,
        keyboard_devices,
        pointer_devices,
    )))
}

pub fn bringup_summary() -> Option<InputBringupSummary> {
    BRINGUP_SUMMARY.get().copied()
}

pub fn poll_ready_sources() {
    if let Some(manager) = serviceos_kernel_core::input::manager() {
        manager.poll_ready(|object_id| {
            let _ = task::notify_input_ready(ObjectId(object_id));
        });
    }
}

static BRINGUP_SUMMARY: Once<InputBringupSummary> = Once::new();

struct VirtioInputBackend {
    state: Mutex<VirtioInputState>,
}

struct VirtioInputState {
    devices: Vec<InputDeviceState>,
    queue: [InputEventInfo; MAX_PENDING_EVENTS],
    queue_head: usize,
    queue_len: usize,
    capabilities: u32,
}

struct InputDeviceState {
    device: VirtIOInput<KernelHal, PciTransport>,
    pointer: Option<PointerSource>,
    keyboard: bool,
    pending_x: i32,
    pending_y: i32,
    motion_dirty: bool,
}

#[derive(Clone, Copy)]
enum PointerSource {
    Absolute(PointerAxes),
    Relative,
}

#[derive(Clone, Copy)]
struct PointerAxes {
    min_x: i32,
    max_x: i32,
    min_y: i32,
    max_y: i32,
}

impl VirtioInputBackend {
    fn new(devices: Vec<InputDeviceState>, keyboard_devices: u32, pointer_devices: u32) -> Self {
        let mut capabilities = 0u32;
        if pointer_devices > 0 {
            capabilities |= input_capability::POINTER;
        }
        if keyboard_devices > 0 {
            capabilities |= input_capability::KEYBOARD;
        }

        Self {
            state: Mutex::new(VirtioInputState {
                devices,
                queue: [InputEventInfo {
                    kind: 0,
                    code: 0,
                    value0: 0,
                    value1: 0,
                }; MAX_PENDING_EVENTS],
                queue_head: 0,
                queue_len: 0,
                capabilities,
            }),
        }
    }
}

impl InputBackend for VirtioInputBackend {
    fn info(&self) -> InputSourceInfo {
        let state = self.state.lock();
        InputSourceInfo {
            backend: InputSourceBackend::VirtioPci as u32,
            capabilities: state.capabilities,
            device_count: state.devices.len() as u32,
            pending_events: state.queue_len as u32,
        }
    }

    fn receive(&self) -> Result<InputEventInfo, InputSourceError> {
        let mut state = self.state.lock();
        if state.queue_len == 0 {
            return Err(InputSourceError::QueueEmpty);
        }
        let event = state.queue[state.queue_head];
        state.queue_head = (state.queue_head + 1) % MAX_PENDING_EVENTS;
        state.queue_len -= 1;
        Ok(event)
    }

    fn poll(&self) -> bool {
        let mut state = self.state.lock();
        let mut became_ready = false;

        let mut device_index = 0usize;
        while device_index < state.devices.len() {
            {
                let device = &mut state.devices[device_index];
                let _ = device.device.ack_interrupt();
            }
            loop {
                let normalized = {
                    let device = &mut state.devices[device_index];
                    device
                        .device
                        .pop_pending_event()
                        .and_then(|event| normalize_event(device, event))
                };
                let Some(normalized) = normalized else {
                    break;
                };
                if state.queue_len == MAX_PENDING_EVENTS {
                    state.queue_head = (state.queue_head + 1) % MAX_PENDING_EVENTS;
                    state.queue_len -= 1;
                }
                if state.queue_len == 0 {
                    became_ready = true;
                }
                let insert_index = (state.queue_head + state.queue_len) % MAX_PENDING_EVENTS;
                state.queue[insert_index] = normalized;
                state.queue_len += 1;
            }

            device_index += 1;
        }

        became_ready
    }
}

fn pointer_axes(
    device: &mut VirtIOInput<KernelHal, PciTransport>,
) -> Result<Option<PointerSource>, ()> {
    if let (Ok(x), Ok(y)) = (device.abs_info(ABS_X as u8), device.abs_info(ABS_Y as u8)) {
        if x.max > x.min && y.max > y.min {
            return Ok(Some(PointerSource::Absolute(PointerAxes {
                min_x: x.min as i32,
                max_x: x.max as i32,
                min_y: y.min as i32,
                max_y: y.max as i32,
            })));
        }
    }

    let rel_bits = device.ev_bits(EV_REL as u8).map_err(|_| ())?;
    let has_rel_x = bit_is_set(&rel_bits, REL_X as usize);
    let has_rel_y = bit_is_set(&rel_bits, REL_Y as usize);
    if has_rel_x || has_rel_y {
        return Ok(Some(PointerSource::Relative));
    }

    Ok(None)
}

fn normalize_event(device: &mut InputDeviceState, event: InputEvent) -> Option<InputEventInfo> {
    match event.event_type {
        EV_ABS => {
            if let Some(PointerSource::Absolute(_)) = device.pointer {
                match event.code {
                    ABS_X => {
                        device.pending_x = event.value as i32;
                        device.motion_dirty = true;
                    }
                    ABS_Y => {
                        device.pending_y = event.value as i32;
                        device.motion_dirty = true;
                    }
                    _ => {}
                }
            }
            None
        }
        EV_REL => {
            if device.pointer.is_some() {
                match event.code {
                    REL_X if matches!(device.pointer, Some(PointerSource::Relative)) => {
                        device.pending_x = device.pending_x.saturating_add(event.value as i32);
                        device.motion_dirty = true;
                    }
                    REL_Y if matches!(device.pointer, Some(PointerSource::Relative)) => {
                        device.pending_y = device.pending_y.saturating_add(event.value as i32);
                        device.motion_dirty = true;
                    }
                    REL_WHEEL => {
                        return Some(InputEventInfo {
                            kind: InputEventKind::PointerScroll as u32,
                            code: 0,
                            value0: 0,
                            value1: event.value as i32,
                        });
                    }
                    _ => {}
                }
            }
            None
        }
        EV_SYN if event.code == SYN_REPORT => {
            let pointer = device.pointer?;
            if !device.motion_dirty {
                return None;
            }
            device.motion_dirty = false;
            match pointer {
                PointerSource::Absolute(axes) => Some(InputEventInfo {
                    kind: InputEventKind::PointerMotion as u32,
                    code: 0,
                    value0: normalize_axis(device.pending_x, axes.min_x, axes.max_x),
                    value1: normalize_axis(device.pending_y, axes.min_y, axes.max_y),
                }),
                PointerSource::Relative => {
                    let delta_x = device.pending_x;
                    let delta_y = device.pending_y;
                    device.pending_x = 0;
                    device.pending_y = 0;
                    Some(InputEventInfo {
                        kind: InputEventKind::PointerDelta as u32,
                        code: 0,
                        value0: delta_x,
                        value1: delta_y,
                    })
                }
            }
        }
        EV_KEY if device.pointer.is_some() => map_pointer_button(event),
        EV_KEY if device.keyboard => Some(InputEventInfo {
            kind: InputEventKind::Key as u32,
            code: event.code as u32,
            value0: if event.value == 0 { 0 } else { 1 },
            value1: 0,
        }),
        _ => None,
    }
}

fn map_pointer_button(event: InputEvent) -> Option<InputEventInfo> {
    let button = match event.code {
        BTN_LEFT => InputButton::Left,
        BTN_RIGHT => InputButton::Right,
        BTN_MIDDLE => InputButton::Middle,
        _ => return None,
    };
    Some(InputEventInfo {
        kind: InputEventKind::PointerButton as u32,
        code: button as u32,
        value0: if event.value == 0 { 0 } else { 1 },
        value1: 0,
    })
}

fn normalize_axis(value: i32, min: i32, max: i32) -> i32 {
    if max <= min {
        return 0;
    }
    let span = (max - min) as i64;
    let clamped = value.clamp(min, max) as i64 - min as i64;
    ((clamped.saturating_mul(65_535)) / span) as i32
}

fn bit_is_set(bits: &[u8], index: usize) -> bool {
    let byte = index / 8;
    let bit = index % 8;
    bits.get(byte).is_some_and(|value| value & (1 << bit) != 0)
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

        let pointer = NonNull::new(base as *mut u8).unwrap_or(NonNull::dangling());
        (base, pointer)
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

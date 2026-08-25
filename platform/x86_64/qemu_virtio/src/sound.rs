use alloc::sync::Arc;
use core::ptr::NonNull;

use serviceos_abi::{
    AudioEndpointBackend, AudioEndpointInfo, AudioEndpointState, AudioToneRequest, audio_capability,
};
use serviceos_kernel_arch_x86_64::paging::ActivePageTable;
use serviceos_kernel_core::{
    audio::{AudioBackend, AudioEndpointError},
    memory::{self, PageMapper, PhysicalAddress, VirtualAddress},
};
use spin::{Mutex, Once};
use virtio_drivers::{
    BufferDirection, Hal, PAGE_SIZE,
    device::sound::{PcmFeatures, PcmFormat, PcmRate, VirtIOSound},
    transport::{
        DeviceType,
        pci::{
            PciTransport,
            bus::{Command, ConfigurationAccess, DeviceFunction, HeaderType, PciRoot},
            virtio_device_type,
        },
    },
};
use x86_64::instructions::port::Port;

const PCI_CONFIG_ADDRESS_PORT: u16 = 0xCF8;
const PCI_CONFIG_DATA_PORT: u16 = 0xCFC;

/// Mixed-PCM batches arrive as 256 stereo s16 frames (1 KiB); the device
/// buffer holds four such periods so short bursts queue without drops.
const PCM_PERIOD_BYTES: u32 = 1024;
const PCM_BUFFER_BYTES: u32 = PCM_PERIOD_BYTES * 4;
const SINK_RATE_HZ: u32 = 48_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SoundBringupSummary {
    pub backend: AudioEndpointBackend,
    pub pci_bus: u8,
    pub pci_device: u8,
    pub pci_function: u8,
    pub stream_id: u32,
    pub rate_hz: u32,
    pub channels: u32,
}

pub fn initialize() -> Option<Arc<dyn AudioBackend>> {
    let mut root = PciRoot::new(IoPortPciConfigAccess);
    for bus in 0u16..=255 {
        for (device_function, info) in root.enumerate_bus(bus as u8) {
            if info.header_type != HeaderType::Standard {
                continue;
            }
            if virtio_device_type(&info) != Some(DeviceType::Sound) {
                continue;
            }

            let mut command = root.get_status_command(device_function).1;
            command.insert(Command::BUS_MASTER | Command::MEMORY_SPACE);
            root.set_command(device_function, command);

            let transport = PciTransport::new::<KernelHal, _>(&mut root, device_function).ok()?;
            let mut device = VirtIOSound::<KernelHal, _>::new(transport).ok()?;
            // Prefer the first advertised output stream; capture streams are
            // reserved groundwork and never selected here.
            let stream_id = *device.output_streams().ok()?.first()?;
            device
                .pcm_set_params(
                    stream_id,
                    PCM_BUFFER_BYTES,
                    PCM_PERIOD_BYTES,
                    PcmFeatures::empty(),
                    2,
                    PcmFormat::S16,
                    PcmRate::Rate48000,
                )
                .ok()?;
            device.pcm_prepare(stream_id).ok()?;
            device.pcm_start(stream_id).ok()?;

            let summary = SoundBringupSummary {
                backend: AudioEndpointBackend::VirtioSound,
                pci_bus: device_function.bus,
                pci_device: device_function.device,
                pci_function: device_function.function,
                stream_id,
                rate_hz: SINK_RATE_HZ,
                channels: 2,
            };
            let backend = Arc::new(VirtioSoundBackend {
                state: Mutex::new(VirtioSoundState {
                    device,
                    stream_id,
                    write_calls: 0,
                    bytes_queued: 0,
                    failed_writes: 0,
                }),
            });
            let _ = BRINGUP_SUMMARY.call_once(|| summary);
            return Some(backend);
        }
    }

    None
}

pub fn bringup_summary() -> Option<SoundBringupSummary> {
    BRINGUP_SUMMARY.get().copied()
}

static BRINGUP_SUMMARY: Once<SoundBringupSummary> = Once::new();

struct VirtioSoundBackend {
    state: Mutex<VirtioSoundState>,
}

struct VirtioSoundState {
    device: VirtIOSound<KernelHal, PciTransport>,
    stream_id: u32,
    write_calls: u64,
    bytes_queued: u64,
    failed_writes: u64,
}

impl AudioBackend for VirtioSoundBackend {
    fn info(&self) -> AudioEndpointInfo {
        let state = self.state.lock();
        AudioEndpointInfo {
            backend: AudioEndpointBackend::VirtioSound as u32,
            direction: serviceos_abi::AudioEndpointDirection::Output as u32,
            state: if state.bytes_queued == 0 {
                AudioEndpointState::Idle as u32
            } else {
                AudioEndpointState::Active as u32
            },
            capabilities: audio_capability::PLAYBACK | audio_capability::PCM,
            nominal_rate_hz: SINK_RATE_HZ,
            channels: 2,
            min_frequency_hz: 0,
            max_frequency_hz: 0,
            current_frequency_hz: 0,
            reserved: 0,
            play_count: state.write_calls,
        }
    }

    fn play_tone(&self, _request: AudioToneRequest) -> Result<(), AudioEndpointError> {
        Err(AudioEndpointError::Unsupported)
    }

    fn stop(&self) -> Result<(), AudioEndpointError> {
        Ok(())
    }

    fn pcm_write_s16le_stereo(&self, bytes: &[u8]) -> Result<usize, AudioEndpointError> {
        if bytes.is_empty() || bytes.len() % 4 != 0 || bytes.len() > PCM_PERIOD_BYTES as usize {
            return Err(AudioEndpointError::Unsupported);
        }
        let mut state = self.state.lock();
        let stream_id = state.stream_id;
        match state.device.pcm_xfer(stream_id, bytes) {
            Ok(()) => {
                state.write_calls = state.write_calls.saturating_add(1);
                state.bytes_queued = state.bytes_queued.saturating_add(bytes.len() as u64);
                Ok(bytes.len())
            }
            Err(_) => {
                state.failed_writes = state.failed_writes.saturating_add(1);
                Err(AudioEndpointError::Busy)
            }
        }
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
            data_port.write(data)
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

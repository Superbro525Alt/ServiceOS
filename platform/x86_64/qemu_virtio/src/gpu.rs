//! virtio-gpu display backend (x86_64 PCI, single scanout, 2D-only).
//!
//! DMA-backed, damage-driven presentation through virtqueue transfers —
//! NOT GPU-accelerated rendering. The vendored `virtio-drivers` GPU driver
//! owns the 2D resource lifecycle (resource_create_2d →
//! resource_attach_backing → set_scanout → transfer_to_host_2d →
//! resource_flush) over a polled control virtqueue (no IRQ registration;
//! completions pop the used ring inline, mirroring the polled block I/O
//! model). The cursor virtqueue is created by the driver but stays unused.
//!
//! Damage semantics: `present`/`present_damage` copy only dirty rows into
//! the DMA backing (shadow-compare against the last presented frame) and
//! flush only when something changed. The DMA transfer granularity is the
//! whole resource — the vendored crate exposes no rect-scoped
//! transfer_to_host_2d — so "damage-driven" here means damage-driven
//! CPU-side updates plus one whole-resource transfer per flushed present.
//!
//! Resource geometry matches the boot GOP mode for continuity with the
//! linear-framebuffer path (stride `width * 4`, 32bpp, Xrgb8888 byte
//! layout == the device's B8G8R8A8UNORM row bytes). Any probe or
//! negotiation failure falls back to the GOP backend with a one-line
//! `display: virtio-gpu unavailable reason=...` diagnostic, and the
//! build-time `SERVICEOS_VGPU_DISABLE` opt-out forces that fallback
//! (mirroring `SERVICEOS_MSIX_DISABLE`).

use alloc::sync::Arc;
use core::ptr::NonNull;

use serviceos_abi::{
    DisplayOutputBackend, DisplayOutputInfo, DisplayOutputState, DisplayPixelFormat,
};
use serviceos_kernel_arch_x86_64::paging::ActivePageTable;
use serviceos_kernel_core::{
    display::{dirty_row_span, DamageRect, DisplayBackend, DisplayOutputError},
    memory::{self, PageMapper, PhysicalAddress, VirtualAddress},
};
use spin::{Mutex, Once};
use virtio_drivers::{
    device::gpu::VirtIOGpu,
    transport::{
        pci::{
            bus::{Command, HeaderType, PciRoot},
            virtio_device_type, PciTransport,
        },
        DeviceType,
    },
    BufferDirection, Hal, PAGE_SIZE,
};

use crate::msix::IoPortPciConfigAccess;

const MAX_FRAMEBUFFER_BYTES: usize = 8 * 1024 * 1024;

/// Build-time opt-out for the virtio-gpu display backend
/// (SERVICEOS_VGPU_DISABLE): forces the GOP linear-framebuffer fallback
/// with a greppable reason, no other behavior change.
pub(crate) const VGPU_DISABLED: bool = option_env!("SERVICEOS_VGPU_DISABLE").is_some();

static mut SHADOW_FRAME_BYTES: [u8; MAX_FRAMEBUFFER_BYTES] = [0; MAX_FRAMEBUFFER_BYTES];

/// Last probe failure reason, printed once by the image's display summary
/// when the GOP backend is used instead.
pub static VGPU_UNAVAILABLE: Once<&'static str> = Once::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GpuDisplaySummary {
    pub pci_bus: u8,
    pub pci_device: u8,
    pub pci_function: u8,
    pub width: u32,
    pub height: u32,
    pub stride_bytes: usize,
    pub byte_len: usize,
}

pub fn bringup_summary() -> Option<GpuDisplaySummary> {
    BRINGUP_SUMMARY.get().copied()
}

pub fn unavailable_reason() -> Option<&'static str> {
    VGPU_UNAVAILABLE.get().copied()
}

fn record_unavailable(reason: &'static str) {
    let _ = VGPU_UNAVAILABLE.call_once(|| reason);
}

static BRINGUP_SUMMARY: Once<GpuDisplaySummary> = Once::new();

/// Probe the PCI buses for a modern virtio-gpu device and bring up the 2D
/// display backend sized to the boot GOP mode. `None` (with a reason
/// recorded for the display summary line) means "use the GOP fallback".
pub fn initialize(
    framebuffer: &serviceos_kernel_core::bootstrap::FramebufferInfo,
) -> Option<Arc<dyn DisplayBackend>> {
    if VGPU_DISABLED {
        record_unavailable("opted-out via SERVICEOS_VGPU_DISABLE");
        return None;
    }
    if framebuffer.bytes_per_pixel != 4 {
        record_unavailable("boot framebuffer is not 32bpp");
        return None;
    }
    if framebuffer.stride != framebuffer.width {
        record_unavailable("boot framebuffer rows are padded (stride != width)");
        return None;
    }
    if framebuffer.byte_len > MAX_FRAMEBUFFER_BYTES {
        record_unavailable("boot framebuffer exceeds the 8MiB backend cap");
        return None;
    }

    let mut root = PciRoot::new(IoPortPciConfigAccess);
    for bus in 0u16..=255 {
        for (device_function, info) in root.enumerate_bus(bus as u8) {
            if info.header_type != HeaderType::Standard {
                continue;
            }
            if virtio_device_type(&info) != Some(DeviceType::GPU) {
                continue;
            }

            let mut command = root.get_status_command(device_function).1;
            command.insert(Command::BUS_MASTER | Command::MEMORY_SPACE);
            root.set_command(device_function, command);

            let transport = match PciTransport::new::<KernelHal, _>(&mut root, device_function) {
                Ok(transport) => transport,
                Err(_) => {
                    record_unavailable("pci transport init failed");
                    return None;
                }
            };
            let mut gpu = match VirtIOGpu::<KernelHal, _>::new(transport) {
                Ok(gpu) => gpu,
                Err(_) => {
                    record_unavailable("feature negotiation / queue setup failed");
                    return None;
                }
            };
            // Continuity: size the 2D resource to the boot GOP mode so the
            // swap from the linear framebuffer is invisible to presenters.
            let framebuffer_slice =
                match gpu.change_resolution(framebuffer.width as u32, framebuffer.height as u32) {
                    Ok(slice) => slice,
                    Err(_) => {
                        record_unavailable("2d resource / scanout setup failed");
                        return None;
                    }
                };
            let byte_len = framebuffer_slice.len();
            let framebuffer_pointer = framebuffer_slice.as_ptr() as u64;
            let _ = framebuffer_slice;

            let summary = GpuDisplaySummary {
                pci_bus: device_function.bus,
                pci_device: device_function.device,
                pci_function: device_function.function,
                width: framebuffer.width as u32,
                height: framebuffer.height as u32,
                stride_bytes: framebuffer.width * 4,
                byte_len,
            };
            let backend = Arc::new(VirtioGpuDisplayBackend {
                summary,
                framebuffer_pointer,
                state: Mutex::new(GpuState {
                    gpu,
                    present_count: 0,
                }),
            });
            let _ = BRINGUP_SUMMARY.call_once(|| summary);
            return Some(backend);
        }
    }

    record_unavailable("no virtio-gpu device found on pci bus walk");
    None
}

struct GpuState {
    gpu: VirtIOGpu<KernelHal, PciTransport>,
    present_count: u64,
}

struct VirtioGpuDisplayBackend {
    summary: GpuDisplaySummary,
    framebuffer_pointer: u64,
    state: Mutex<GpuState>,
}

impl VirtioGpuDisplayBackend {
    /// The crate-owned DMA backing for the 2D resource. The `Dma`
    /// allocation lives inside `GpuState.gpu`, so the pointer stays valid
    /// for the backend's lifetime (aarch64 virt precedent).
    fn framebuffer_slice(&self) -> &mut [u8] {
        unsafe {
            core::slice::from_raw_parts_mut(
                self.framebuffer_pointer as *mut u8,
                self.summary.byte_len,
            )
        }
    }

    /// Copy dirty rows (compared against the last-presented shadow) from
    /// `frame` into the DMA backing; returns whether anything changed.
    fn copy_dirty_rows(
        &self,
        frame: &[u8],
        rows: impl Iterator<Item = usize>,
        span: (usize, usize),
    ) -> bool {
        let mut dirty = false;
        let framebuffer = self.framebuffer_slice();
        let shadow_frame = shadow_frame_slice(frame.len());
        for row in rows {
            let row_offset = row * self.summary.stride_bytes;
            let previous = &mut shadow_frame[row_offset + span.0..row_offset + span.1];
            let current = &frame[row_offset + span.0..row_offset + span.1];
            let Some((start, end)) = dirty_row_span(previous, current) else {
                continue;
            };
            framebuffer[row_offset + span.0 + start..row_offset + span.0 + end]
                .copy_from_slice(&current[start..end]);
            previous[start..end].copy_from_slice(&current[start..end]);
            dirty = true;
        }
        dirty
    }

    fn flush_if_dirty(&self, state: &mut GpuState, dirty: bool) -> Result<(), DisplayOutputError> {
        if dirty {
            state.gpu.flush().map_err(|_| DisplayOutputError::Busy)?;
        }
        state.present_count = state.present_count.saturating_add(1);
        Ok(())
    }
}

impl DisplayBackend for VirtioGpuDisplayBackend {
    fn info(&self) -> DisplayOutputInfo {
        let state = self.state.lock();
        DisplayOutputInfo {
            backend: DisplayOutputBackend::BootFramebuffer as u32,
            state: DisplayOutputState::Connected as u32,
            pixel_format: DisplayPixelFormat::Xrgb8888 as u32,
            reserved: 0,
            width: self.summary.width,
            height: self.summary.height,
            stride: self.summary.width,
            bytes_per_pixel: 4,
            byte_len: self.summary.byte_len as u64,
            present_count: state.present_count,
        }
    }

    fn present(&self, frame: &[u8]) -> Result<(), DisplayOutputError> {
        if frame.len() != self.summary.byte_len || frame.len() > MAX_FRAMEBUFFER_BYTES {
            return Err(DisplayOutputError::BufferTooSmall);
        }
        let mut state = self.state.lock();
        let rows = 0..self.summary.height as usize;
        let span = (0usize, self.summary.stride_bytes);
        let dirty = self.copy_dirty_rows(frame, rows, span);
        self.flush_if_dirty(&mut state, dirty)
    }

    fn present_damage(
        &self,
        frame: &[u8],
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), DisplayOutputError> {
        if frame.len() != self.summary.byte_len || frame.len() > MAX_FRAMEBUFFER_BYTES {
            return Err(DisplayOutputError::BufferTooSmall);
        }
        let rect = DamageRect::new(x, y, width, height)
            .clamped_to(self.summary.width, self.summary.height);
        if rect.is_empty() {
            let mut state = self.state.lock();
            self.flush_if_dirty(&mut state, false)?;
            return Ok(());
        }
        let rows = rect.y as usize..(rect.y + rect.height as i32) as usize;
        let span = rect.row_byte_span(self.summary.stride_bytes, 4);
        let mut state = self.state.lock();
        let dirty = self.copy_dirty_rows(frame, rows, span);
        self.flush_if_dirty(&mut state, dirty)
    }
}

fn shadow_frame_slice(len: usize) -> &'static mut [u8] {
    unsafe {
        core::slice::from_raw_parts_mut(
            core::ptr::addr_of_mut!(SHADOW_FRAME_BYTES).cast::<u8>(),
            len,
        )
    }
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

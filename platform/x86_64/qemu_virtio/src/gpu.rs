//! virtio-gpu display backend (x86_64 PCI, single scanout, 2D-only).
//!
//! DMA-backed, damage-driven presentation through virtqueue transfers —
//! NOT GPU-accelerated rendering. Presentation is rect-scoped: each
//! flushed present ships exactly the dirty rectangle, first as
//! TRANSFER_TO_HOST_2D (backing → host resource) and then as
//! RESOURCE_FLUSH (host resource → scanout), both over the polled control
//! virtqueue (no IRQ registration; completions pop the used ring inline,
//! mirroring the polled block I/O model). The control-queue request
//! envelopes are packed in-tree — the vendored `virtio-drivers` GPU driver
//! keeps its wire structs private and only exposes a whole-resource
//! `flush()`, so its `VirtQueue`/`Dma`/`Transport` plumbing is reused
//! under its own wire layout (opcodes/rect/offset packing pinned by the
//! `kernel-core` display-transfer goldens). The cursor virtqueue is left
//! unconfigured.
//!
//! Damage semantics: `present`/`present_damage` copy only dirty rows into
//! the DMA backing (shadow-compare against the last presented frame) and
//! flush only when something changed. The transfer/flush rect is the
//! bounding box of the bytes that actually changed, clamped to the
//! resource — a clean present does no transfer and no flush at all.
//!
//! Fallback discipline: if the in-tree control path fails at init
//! (feature/queue/probe mismatch), the device is re-negotiated from
//! scratch on the vendored crate's whole-resource `VirtIOGpu` path
//! (greppable `vgpu: rect transfers unavailable reason=...` line) —
//! behavior stays correct either way. Any probe failure of both paths
//! falls back to the GOP backend with a one-line
//! `display: virtio-gpu unavailable reason=...` diagnostic, and the
//! build-time `SERVICEOS_VGPU_DISABLE` opt-out forces that fallback
//! (mirroring `SERVICEOS_MSIX_DISABLE`).
//!
//! Resource geometry matches the boot GOP mode for continuity with the
//! linear-framebuffer path (stride `width * 4`, 32bpp, Xrgb8888 byte
//! layout == the device's B8G8R8A8UNORM row bytes).

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec;
use core::ptr::NonNull;

use serviceos_abi::{
    DisplayOutputBackend, DisplayOutputInfo, DisplayOutputState, DisplayPixelFormat,
};
use serviceos_kernel_arch_x86_64::paging::ActivePageTable;
use serviceos_kernel_core::{
    display::{
        backing_offset, dirty_row_span, fits_resource, pack_ctrl_header,
        pack_resource_attach_backing, pack_resource_create_2d, pack_resource_flush,
        pack_set_scanout, pack_transfer_to_host_2d, response_type, transfer_bytes, vgpu_wire,
        DamageRect, DirtyBounds, DisplayBackend, DisplayOutputError, GET_DISPLAY_INFO_LEN,
        OK_DISPLAY_INFO, OK_NODATA, RESOURCE_ATTACH_BACKING_LEN, RESOURCE_CREATE_2D_LEN,
        RESOURCE_FLUSH_LEN, SET_SCANOUT_LEN, TRANSFER_TO_HOST_2D_LEN,
    },
    memory::{self, PageMapper, PhysicalAddress, VirtualAddress},
};
use spin::{Mutex, Once};
use virtio_drivers::{
    device::gpu::VirtIOGpu,
    queue::VirtQueue,
    transport::{
        pci::{
            bus::{Command, HeaderType, PciRoot},
            virtio_device_type, PciTransport,
        },
        DeviceStatus, DeviceType, Transport,
    },
    BufferDirection, Hal, PAGE_SIZE,
};

use crate::msix::IoPortPciConfigAccess;
use crate::serial;

const MAX_FRAMEBUFFER_BYTES: usize = 8 * 1024 * 1024;

/// Control-virtqueue depth (matches the vendored driver's QUEUE_SIZE).
const CTRL_QUEUE_SIZE: usize = 2;
/// Raw feature bits negotiated for the in-tree control path: indirect
/// descriptors, event idx, and (mandatory) virtio 1.0.
const FEATURE_RING_INDIRECT_DESC: u64 = 1 << 28;
const FEATURE_RING_EVENT_IDX: u64 = 1 << 29;
const FEATURE_VERSION_1: u64 = 1 << 32;
const SUPPORTED_FEATURES: u64 =
    FEATURE_RING_INDIRECT_DESC | FEATURE_RING_EVENT_IDX | FEATURE_VERSION_1;

/// First N rect transfers print a serial line
/// (`vgpu: rect transfer w=W h=H bytes=K`); after that the present path
/// stays silent to keep default-boot noise minimal.
const RECT_TRANSFER_LOG_FRAMES: u32 = 3;

/// `GET_DISPLAY_INFO` opcode (virtio-gpu spec 5.6.4) — init-time probe
/// command proving the in-tree control-queue submit path works.
const GET_DISPLAY_INFO: u32 = 0x100;

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

            let summary = GpuDisplaySummary {
                pci_bus: device_function.bus,
                pci_device: device_function.device,
                pci_function: device_function.function,
                width: framebuffer.width as u32,
                height: framebuffer.height as u32,
                stride_bytes: framebuffer.width * 4,
                byte_len: framebuffer.byte_len,
            };

            // Primary path: in-tree rect-scoped transfers. On init failure
            // the device is handed back for a from-scratch crate fallback
            // (the vendored driver re-negotiates from a device reset).
            match InTreeGpu::bring_up(
                transport,
                framebuffer.width as u32,
                framebuffer.height as u32,
            ) {
                Ok(gpu) => {
                    let _ = BRINGUP_SUMMARY.call_once(|| summary);
                    return Some(Arc::new(VirtioGpuDisplayBackend {
                        summary,
                        framebuffer_pointer: gpu.framebuffer_pointer(),
                        state: Mutex::new(GpuState {
                            presenter: Presenter::Rect(gpu),
                            present_count: 0,
                            rect_transfers_printed: 0,
                        }),
                    }));
                }
                Err((transport, reason)) => {
                    serial::write_args(format_args!(
                        "serviceos: display: vgpu: rect transfers unavailable reason={}\n",
                        reason
                    ));
                    match CrateGpu::bring_up(
                        transport,
                        framebuffer.width as u32,
                        framebuffer.height as u32,
                    ) {
                        Ok(gpu) => {
                            let _ = BRINGUP_SUMMARY.call_once(|| summary);
                            return Some(Arc::new(VirtioGpuDisplayBackend {
                                summary,
                                framebuffer_pointer: gpu.framebuffer_pointer(),
                                state: Mutex::new(GpuState {
                                    presenter: Presenter::Whole(gpu),
                                    present_count: 0,
                                    rect_transfers_printed: RECT_TRANSFER_LOG_FRAMES,
                                }),
                            }));
                        }
                        Err(reason) => {
                            record_unavailable(reason);
                            return None;
                        }
                    }
                }
            }
        }
    }

    record_unavailable("no virtio-gpu device found on pci bus walk");
    None
}

/// In-tree control-queue presenter: rect-scoped TRANSFER_TO_HOST_2D +
/// RESOURCE_FLUSH with the vendored crate's queue/DMA/transport plumbing.
struct InTreeGpu {
    transport: PciTransport,
    ctrl: VirtQueue<KernelHal, CTRL_QUEUE_SIZE>,
    /// Control-request staging buffer (one descriptor, exact length).
    send: Box<[u8]>,
    /// Control-reply buffer.
    recv: Box<[u8]>,
    /// DMA backing of the 2D resource (guest memory the host reads).
    frame_dma: FrameBacking,
    width: u32,
    height: u32,
    stride_bytes: usize,
    byte_len: usize,
}

/// Guest-visible DMA backing of the 2D resource. The crate's `Dma` type
/// is not publicly re-exported, so the allocation goes through
/// `KernelHal::dma_alloc` directly — the same call `Dma::new` makes.
struct FrameBacking {
    paddr: u64,
}

// SAFETY: DMA memory can be accessed from any thread; all CPU access is
// serialized through the backend `Mutex` (mirrors the crate's `Dma`).
unsafe impl Send for FrameBacking {}
unsafe impl Sync for FrameBacking {}

impl FrameBacking {
    fn allocate(byte_len: usize) -> Result<Self, ()> {
        let pages = (byte_len + PAGE_SIZE - 1) / PAGE_SIZE;
        let (paddr, _) = KernelHal::dma_alloc(pages, BufferDirection::DriverToDevice);
        if paddr == 0 {
            return Err(());
        }
        Ok(FrameBacking { paddr })
    }
}

impl InTreeGpu {
    /// Negotiate features, set up the control virtqueue, prove the raw
    /// submit path with a GET_DISPLAY_INFO probe, then create the 2D
    /// resource + backing + scanout binding. On any failure the transport
    /// is returned untouched so the vendored-driver fallback can reset
    /// and re-negotiate the device from scratch.
    fn bring_up(
        mut transport: PciTransport,
        width: u32,
        height: u32,
    ) -> Result<Self, (PciTransport, &'static str)> {
        let fail = |transport, reason| Err((transport, reason));

        // Manual feature negotiation (the crate's begin_init is generic
        // over bitflags::Flags; raw bits avoid a new dependency).
        transport.set_status(DeviceStatus::empty());
        transport.set_status(DeviceStatus::ACKNOWLEDGE | DeviceStatus::DRIVER);
        let negotiated = transport.read_device_features() & SUPPORTED_FEATURES;
        if negotiated & FEATURE_VERSION_1 == 0 {
            return fail(transport, "device did not offer virtio 1.0");
        }
        transport.write_driver_features(negotiated);
        transport.set_status(
            DeviceStatus::ACKNOWLEDGE | DeviceStatus::DRIVER | DeviceStatus::FEATURES_OK,
        );
        if !transport.get_status().contains(DeviceStatus::FEATURES_OK) {
            return fail(transport, "features-ok rejected");
        }
        transport.set_guest_page_size(PAGE_SIZE as u32);

        let ctrl = match VirtQueue::new(
            &mut transport,
            0,
            negotiated & FEATURE_RING_INDIRECT_DESC != 0,
            negotiated & FEATURE_RING_EVENT_IDX != 0,
        ) {
            Ok(ctrl) => ctrl,
            Err(_) => return fail(transport, "control queue setup failed"),
        };
        transport.finish_init();

        // Probe: one full raw request/response round trip before any
        // resource state exists, so a submit-path mismatch fails here.
        let mut gpu = InTreeGpu {
            transport,
            ctrl,
            send: vec![0u8; PAGE_SIZE].into_boxed_slice(),
            recv: vec![0u8; PAGE_SIZE].into_boxed_slice(),
            frame_dma: FrameBacking { paddr: 0 },
            width,
            height,
            stride_bytes: width as usize * 4,
            byte_len: width as usize * height as usize * 4,
        };
        pack_ctrl_header(&mut gpu.send, GET_DISPLAY_INFO);
        if gpu.request(GET_DISPLAY_INFO_LEN, OK_DISPLAY_INFO).is_err() {
            return fail(gpu.transport, "control-queue probe failed");
        }

        // Resource lifecycle: create_2d → attach_backing → set_scanout.
        pack_resource_create_2d(&mut gpu.send, vgpu_wire::RESOURCE_ID_FB, width, height);
        if gpu.request(RESOURCE_CREATE_2D_LEN, OK_NODATA).is_err() {
            return fail(gpu.transport, "resource create failed");
        }
        let byte_len = gpu.byte_len;
        let frame_dma = match FrameBacking::allocate(byte_len) {
            Ok(dma) => dma,
            Err(()) => return fail(gpu.transport, "framebuffer dma alloc failed"),
        };
        pack_resource_attach_backing(
            &mut gpu.send,
            vgpu_wire::RESOURCE_ID_FB,
            frame_dma.paddr,
            byte_len as u32,
        );
        if gpu.request(RESOURCE_ATTACH_BACKING_LEN, OK_NODATA).is_err() {
            return fail(gpu.transport, "backing attach failed");
        }
        let full = DamageRect::new(0, 0, width, height);
        pack_set_scanout(
            &mut gpu.send,
            &full,
            vgpu_wire::SCANOUT_ID,
            vgpu_wire::RESOURCE_ID_FB,
        );
        if gpu.request(SET_SCANOUT_LEN, OK_NODATA).is_err() {
            return fail(gpu.transport, "scanout setup failed");
        }
        gpu.frame_dma = frame_dma;
        Ok(gpu)
    }

    /// Submit one control request and await its reply; `len` is the exact
    /// packed request length, `expect` the required reply type.
    fn request(&mut self, len: usize, expect: u32) -> Result<(), ()> {
        self.ctrl
            .add_notify_wait_pop(
                &[&self.send[..len]],
                &mut [&mut self.recv],
                &mut self.transport,
            )
            .map_err(|_| ())?;
        if response_type(&self.recv) != Some(expect) {
            return Err(());
        }
        Ok(())
    }

    /// Ship exactly `rect` to the host resource, then present it on the
    /// scanout. `rect` must be clamped to the resource bounds.
    fn transfer_and_flush(&mut self, rect: &DamageRect) -> Result<(), ()> {
        if !fits_resource(rect, self.width, self.height) {
            return Err(());
        }
        let offset = backing_offset(rect, self.stride_bytes);
        pack_transfer_to_host_2d(&mut self.send, rect, offset, vgpu_wire::RESOURCE_ID_FB);
        self.request(TRANSFER_TO_HOST_2D_LEN, OK_NODATA)?;
        pack_resource_flush(&mut self.send, rect, vgpu_wire::RESOURCE_ID_FB);
        self.request(RESOURCE_FLUSH_LEN, OK_NODATA)
    }

    /// The DMA backing for the 2D resource, shared with the device. The
    /// allocation lives inside `self.frame_dma`, so the pointer stays
    /// valid for the backend's lifetime (aarch64 virt precedent).
    fn framebuffer_pointer(&self) -> u64 {
        self.frame_dma.paddr
    }
}

/// The vendored crate presenter: whole-resource transfer + flush per
/// dirty present (fallback path when the in-tree control path cannot
/// come up).
struct CrateGpu {
    gpu: VirtIOGpu<KernelHal, PciTransport>,
    framebuffer_pointer: u64,
}

impl CrateGpu {
    fn bring_up(transport: PciTransport, width: u32, height: u32) -> Result<Self, &'static str> {
        // begin_init starts from a device reset, so a partially
        // initialized in-tree attempt is cleanly re-negotiated here.
        let mut gpu = VirtIOGpu::<KernelHal, _>::new(transport)
            .map_err(|_| "feature negotiation / queue setup failed")?;
        let slice = gpu
            .change_resolution(width, height)
            .map_err(|_| "2d resource / scanout setup failed")?;
        let framebuffer_pointer = slice.as_ptr() as u64;
        let _ = slice;
        Ok(CrateGpu {
            gpu,
            framebuffer_pointer,
        })
    }

    fn framebuffer_pointer(&self) -> u64 {
        self.framebuffer_pointer
    }

    /// Whole-resource flush (the vendored driver's only granularity).
    fn flush(&mut self) -> Result<(), ()> {
        self.gpu.flush().map_err(|_| ())
    }
}

/// Which presentation path the backend drives.
enum Presenter {
    Rect(InTreeGpu),
    Whole(CrateGpu),
}

struct GpuState {
    presenter: Presenter,
    present_count: u64,
    rect_transfers_printed: u32,
}

struct VirtioGpuDisplayBackend {
    summary: GpuDisplaySummary,
    framebuffer_pointer: u64,
    state: Mutex<GpuState>,
}

impl VirtioGpuDisplayBackend {
    /// Copy dirty rows (compared against the last-presented shadow) from
    /// `frame` into the DMA backing; returns the bounding rect of the
    /// bytes that actually changed (`None` when nothing changed).
    fn copy_dirty_rows(
        &self,
        frame: &[u8],
        rows: impl Iterator<Item = usize>,
        span: (usize, usize),
    ) -> Option<DamageRect> {
        let mut bounds = DirtyBounds::new();
        let framebuffer = unsafe {
            core::slice::from_raw_parts_mut(self.framebuffer_pointer as *mut u8, frame.len())
        };
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
            bounds.absorb_row_span(row, span.0 + start, span.0 + end);
        }
        bounds.bounding()
    }

    /// Flush the already-copied dirty bounding rect via the active
    /// presenter; clean presents skip the device entirely.
    fn flush_if_dirty(
        &self,
        state: &mut GpuState,
        bounding: Option<DamageRect>,
    ) -> Result<(), DisplayOutputError> {
        match (&mut state.presenter, bounding) {
            (Presenter::Rect(gpu), Some(rect)) => {
                gpu.transfer_and_flush(&rect)
                    .map_err(|_| DisplayOutputError::Busy)?;
                if state.rect_transfers_printed < RECT_TRANSFER_LOG_FRAMES {
                    state.rect_transfers_printed += 1;
                    serial::write_args(format_args!(
                        "serviceos: display: vgpu: rect transfer w={} h={} bytes={}\n",
                        rect.width,
                        rect.height,
                        transfer_bytes(&rect)
                    ));
                }
            }
            (Presenter::Whole(gpu), Some(_)) => {
                gpu.flush().map_err(|_| DisplayOutputError::Busy)?;
            }
            _ => {}
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
        let bounding = self.copy_dirty_rows(frame, rows, span);
        self.flush_if_dirty(&mut state, bounding)
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
            self.flush_if_dirty(&mut state, None)?;
            return Ok(());
        }
        let rows = rect.y as usize..(rect.y + rect.height as i32) as usize;
        let span = rect.row_byte_span(self.summary.stride_bytes, 4);
        let mut state = self.state.lock();
        let bounding = self.copy_dirty_rows(frame, rows, span);
        self.flush_if_dirty(&mut state, bounding)
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

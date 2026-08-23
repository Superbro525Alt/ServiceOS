use alloc::sync::Arc;
use core::ptr;
use spin::{Mutex, Once};

use serviceos_abi::{
    DisplayOutputBackend, DisplayOutputInfo, DisplayOutputState, DisplayPixelFormat,
};
use serviceos_kernel_core::display::{DisplayBackend, DisplayOutputError};
use virtio_drivers::{device::gpu::VirtIOGpu, transport::DeviceType};

use crate::dtb::VirtioMmioDevice;
use crate::virtio::{KernelHal, VirtioTransport, discover};

const DEFAULT_WIDTH: u32 = 1024;
const DEFAULT_HEIGHT: u32 = 768;
const BYTES_PER_PIXEL: usize = 4;
const MAX_FRAMEBUFFER_BYTES: usize = 8 * 1024 * 1024;

static mut SHADOW_FRAME_BYTES: [u8; MAX_FRAMEBUFFER_BYTES] = [0; MAX_FRAMEBUFFER_BYTES];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GpuDisplaySummary {
    pub mmio_base: u64,
    pub width: u32,
    pub height: u32,
    pub stride_bytes: usize,
    pub byte_len: usize,
}

pub fn initialize(devices: &[VirtioMmioDevice]) -> Option<Arc<dyn DisplayBackend>> {
    let (discovered, transport) = discover(devices, DeviceType::GPU).into_iter().next()?;
    let mut gpu = VirtIOGpu::<KernelHal, VirtioTransport>::new(transport).ok()?;
    let framebuffer = gpu.change_resolution(DEFAULT_WIDTH, DEFAULT_HEIGHT).ok()?;
    let byte_len = framebuffer.len();
    let stride_bytes = byte_len / DEFAULT_HEIGHT as usize;
    let summary = GpuDisplaySummary {
        mmio_base: discovered.mmio_base,
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
        stride_bytes,
        byte_len,
    };
    let backend = Arc::new(VirtioGpuDisplayBackend {
        summary,
        framebuffer_pointer: framebuffer.as_ptr() as u64,
        state: Mutex::new(GpuState {
            gpu,
            present_count: 0,
        }),
    });
    let _ = BRINGUP_SUMMARY.call_once(|| summary);
    Some(backend)
}

pub fn bringup_summary() -> Option<GpuDisplaySummary> {
    BRINGUP_SUMMARY.get().copied()
}

static BRINGUP_SUMMARY: Once<GpuDisplaySummary> = Once::new();

struct GpuState {
    gpu: VirtIOGpu<KernelHal, VirtioTransport>,
    present_count: u64,
}

struct VirtioGpuDisplayBackend {
    summary: GpuDisplaySummary,
    framebuffer_pointer: u64,
    state: Mutex<GpuState>,
}

impl VirtioGpuDisplayBackend {
    fn framebuffer_slice(&self) -> &mut [u8] {
        unsafe {
            core::slice::from_raw_parts_mut(
                self.framebuffer_pointer as *mut u8,
                self.summary.byte_len,
            )
        }
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
            stride: self.summary.stride_bytes as u32,
            bytes_per_pixel: BYTES_PER_PIXEL as u32,
            byte_len: self.summary.byte_len as u64,
            present_count: state.present_count,
        }
    }

    fn present(&self, frame: &[u8]) -> Result<(), DisplayOutputError> {
        if frame.len() != self.summary.byte_len || frame.len() > MAX_FRAMEBUFFER_BYTES {
            return Err(DisplayOutputError::BufferTooSmall);
        }

        let mut state = self.state.lock();
        let framebuffer = self.framebuffer_slice();
        let shadow_frame = shadow_frame_slice(frame.len());
        for row in 0..self.summary.height as usize {
            let start = row * self.summary.stride_bytes;
            let end = start + self.summary.stride_bytes;
            let previous = &mut shadow_frame[start..end];
            let current = &frame[start..end];
            if let Some((dirty_start, dirty_end)) = dirty_span(previous, current) {
                unsafe {
                    ptr::copy_nonoverlapping(
                        current.as_ptr().add(dirty_start),
                        framebuffer.as_ptr().add(start + dirty_start) as *mut u8,
                        dirty_end - dirty_start,
                    );
                }
                previous[dirty_start..dirty_end].copy_from_slice(&current[dirty_start..dirty_end]);
            }
        }

        let _ = state.gpu.flush();
        state.present_count = state.present_count.saturating_add(1);
        Ok(())
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

        let start_x = x.max(0) as usize;
        let start_y = y.max(0) as usize;
        let end_x = ((x + width as i32).max(0) as usize).min(self.summary.width as usize);
        let end_y = ((y + height as i32).max(0) as usize).min(self.summary.height as usize);
        if start_x >= end_x || start_y >= end_y {
            return Ok(());
        }

        let mut state = self.state.lock();
        let bytes_per_pixel = BYTES_PER_PIXEL;
        let copy_start = start_x * bytes_per_pixel;
        let copy_end = end_x * bytes_per_pixel;
        let framebuffer = self.framebuffer_slice();
        let shadow_frame = shadow_frame_slice(frame.len());
        for row in start_y..end_y {
            let row_offset = row * self.summary.stride_bytes;
            let previous = &mut shadow_frame[row_offset + copy_start..row_offset + copy_end];
            let current = &frame[row_offset + copy_start..row_offset + copy_end];
            if previous == current {
                continue;
            }
            unsafe {
                ptr::copy_nonoverlapping(
                    current.as_ptr(),
                    framebuffer.as_mut_ptr().add(row_offset + copy_start),
                    copy_end - copy_start,
                );
            }
            previous.copy_from_slice(current);
        }

        let _ = state.gpu.flush();
        state.present_count = state.present_count.saturating_add(1);
        Ok(())
    }
}

fn dirty_span(previous: &[u8], current: &[u8]) -> Option<(usize, usize)> {
    if previous.len() != current.len() {
        return Some((0, current.len()));
    }

    let start = previous
        .iter()
        .zip(current.iter())
        .position(|(left, right)| left != right)?;
    let end = previous
        .iter()
        .zip(current.iter())
        .rposition(|(left, right)| left != right)
        .map(|index| index + 1)
        .unwrap_or(start + 1);
    Some((start, end))
}

fn shadow_frame_slice(len: usize) -> &'static mut [u8] {
    unsafe {
        core::slice::from_raw_parts_mut(ptr::addr_of_mut!(SHADOW_FRAME_BYTES).cast::<u8>(), len)
    }
}

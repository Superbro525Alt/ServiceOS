use alloc::sync::Arc;
use core::ptr;

use serviceos_abi::{
    DisplayOutputBackend, DisplayOutputInfo, DisplayOutputState, DisplayPixelFormat,
};
use serviceos_kernel_core::{
    bootstrap::{FramebufferInfo, FramebufferPixelFormat},
    display::{DisplayBackend, DisplayOutputError},
};
use spin::Mutex;

const MAX_FRAMEBUFFER_BYTES: usize = 8 * 1024 * 1024;

static mut SHADOW_FRAME_BYTES: [u8; MAX_FRAMEBUFFER_BYTES] = [0; MAX_FRAMEBUFFER_BYTES];

struct DisplayState {
    present_count: u64,
}

pub fn initialize(framebuffer: FramebufferInfo) -> Arc<dyn DisplayBackend> {
    Arc::new(BootFramebufferBackend {
        framebuffer,
        state: Mutex::new(DisplayState { present_count: 0 }),
    })
}

struct BootFramebufferBackend {
    framebuffer: FramebufferInfo,
    state: Mutex<DisplayState>,
}

impl DisplayBackend for BootFramebufferBackend {
    fn info(&self) -> DisplayOutputInfo {
        let state = self.state.lock();
        DisplayOutputInfo {
            backend: DisplayOutputBackend::BootFramebuffer as u32,
            state: DisplayOutputState::Connected as u32,
            pixel_format: match self.framebuffer.pixel_format {
                FramebufferPixelFormat::Xrgb8888 => DisplayPixelFormat::Xrgb8888 as u32,
                FramebufferPixelFormat::Bgrx8888 => DisplayPixelFormat::Bgrx8888 as u32,
            },
            reserved: 0,
            width: self.framebuffer.width as u32,
            height: self.framebuffer.height as u32,
            stride: self.framebuffer.stride as u32,
            bytes_per_pixel: self.framebuffer.bytes_per_pixel as u32,
            byte_len: self.framebuffer.byte_len as u64,
            present_count: state.present_count,
        }
    }

    fn present(&self, frame: &[u8]) -> Result<(), DisplayOutputError> {
        if frame.len() != self.framebuffer.byte_len || frame.len() > MAX_FRAMEBUFFER_BYTES {
            return Err(DisplayOutputError::BufferTooSmall);
        }

        let mut state = self.state.lock();
        let row_bytes = self.framebuffer.stride * self.framebuffer.bytes_per_pixel;
        let framebuffer = self.framebuffer.physical_base.as_u64() as *mut u8;
        let shadow_frame = shadow_frame_slice(frame.len());
        for row in 0..self.framebuffer.height {
            let start = row * row_bytes;
            let end = start + row_bytes;
            let previous = &mut shadow_frame[start..end];
            let current = &frame[start..end];
            let Some((dirty_start, dirty_end)) = dirty_span(previous, current) else {
                continue;
            };

            unsafe {
                ptr::copy_nonoverlapping(
                    current.as_ptr().add(dirty_start),
                    framebuffer.add(start + dirty_start),
                    dirty_end - dirty_start,
                );
            }
            previous[dirty_start..dirty_end].copy_from_slice(&current[dirty_start..dirty_end]);
        }

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

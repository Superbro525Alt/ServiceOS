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
        if frame.len() > self.framebuffer.byte_len {
            return Err(DisplayOutputError::BufferTooSmall);
        }

        unsafe {
            ptr::copy_nonoverlapping(
                frame.as_ptr(),
                self.framebuffer.physical_base.as_u64() as *mut u8,
                frame.len(),
            );
        }

        let mut state = self.state.lock();
        state.present_count = state.present_count.saturating_add(1);
        Ok(())
    }
}

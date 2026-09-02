use alloc::sync::Arc;
use alloc::vec::Vec;
use serviceos_abi::DisplayOutputInfo;

use super::mode::{find_mode, DisplayModeInfo};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayOutputError {
    BufferTooSmall,
    Busy,
    Unsupported,
}

pub trait DisplayBackend: Send + Sync {
    fn info(&self) -> DisplayOutputInfo;
    fn present(&self, frame: &[u8]) -> Result<(), DisplayOutputError>;
    fn present_damage(
        &self,
        frame: &[u8],
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), DisplayOutputError> {
        let _ = (x, y, width, height);
        self.present(frame)
    }

    /// Modes this output supports. Default: exactly the boot mode —
    /// single-mode honesty until a real mode-set path exists.
    fn supported_modes(&self) -> Vec<DisplayModeInfo> {
        Vec::from([DisplayModeInfo::from_output_info(&self.info())])
    }

    /// Validate a mode-set request against the enumerated list. Matching the
    /// already-active mode is an accepted no-op; any other listed entry is
    /// [`DisplayOutputError::Busy`] (recognized but not switchable yet), and
    /// unlisted geometry is [`DisplayOutputError::Unsupported`].
    fn set_mode(
        &self,
        width: u32,
        height: u32,
        bytes_per_pixel: u32,
    ) -> Result<(), DisplayOutputError> {
        let current = DisplayModeInfo::from_output_info(&self.info());
        match find_mode(&self.supported_modes(), width, height, bytes_per_pixel) {
            Some(mode) if mode == current => Ok(()),
            Some(_) => Err(DisplayOutputError::Busy),
            None => Err(DisplayOutputError::Unsupported),
        }
    }
}

pub struct DisplayOutputObject {
    backend: Arc<dyn DisplayBackend>,
}

impl DisplayOutputObject {
    pub fn new(backend: Arc<dyn DisplayBackend>) -> Self {
        Self { backend }
    }

    pub fn info(&self) -> DisplayOutputInfo {
        self.backend.info()
    }

    pub fn present(&self, frame: &[u8]) -> Result<(), DisplayOutputError> {
        self.backend.present(frame)
    }

    pub fn present_damage(
        &self,
        frame: &[u8],
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), DisplayOutputError> {
        self.backend.present_damage(frame, x, y, width, height)
    }

    /// The currently active display mode (w/h/bpp/stride from BootInfo).
    pub fn current_mode(&self) -> DisplayModeInfo {
        DisplayModeInfo::from_output_info(&self.backend.info())
    }

    /// Enumerated supported modes; single honest entry for boot framebuffers.
    pub fn supported_modes(&self) -> Vec<DisplayModeInfo> {
        let mut modes = self.backend.supported_modes();
        if modes.len() > super::mode::MAX_DISPLAY_MODES {
            modes.truncate(super::mode::MAX_DISPLAY_MODES);
        }
        modes
    }

    /// Mode-set contract: validates against `supported_modes()`. Requesting
    /// the active mode succeeds as a no-op; any other listed combination
    /// reports `Busy` (known-but-unswitchable), unlisted geometry reports
    /// `Unsupported`. Real mode-set is deferred.
    pub fn set_mode(
        &self,
        width: u32,
        height: u32,
        bytes_per_pixel: u32,
    ) -> Result<(), DisplayOutputError> {
        self.backend.set_mode(width, height, bytes_per_pixel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::mode::DisplayModeInfo;

    #[derive(Clone, Copy)]
    struct StubBackend {
        width: u32,
        height: u32,
        bpp: u32,
        stride: u32,
        extra_modes: &'static [(&'static str, u32, u32, u32, u32)],
    }

    impl DisplayBackend for StubBackend {
        fn info(&self) -> DisplayOutputInfo {
            DisplayOutputInfo {
                backend: 1,
                state: 1,
                pixel_format: 1,
                reserved: 0,
                width: self.width,
                height: self.height,
                stride: self.stride,
                bytes_per_pixel: self.bpp,
                byte_len: self.stride as u64 * self.height as u64 * self.bpp as u64,
                present_count: 0,
            }
        }

        fn present(&self, _frame: &[u8]) -> Result<(), DisplayOutputError> {
            Ok(())
        }

        fn supported_modes(&self) -> Vec<DisplayModeInfo> {
            let mut modes = Vec::from([DisplayModeInfo::from_output_info(&self.info())]);
            for (_, width, height, bpp, stride) in self.extra_modes.iter().copied() {
                modes.push(DisplayModeInfo::new(width, height, bpp, stride));
            }
            modes
        }
    }

    fn boot_backend() -> DisplayOutputObject {
        DisplayOutputObject::new(Arc::new(StubBackend {
            width: 1024,
            height: 768,
            bpp: 4,
            stride: 1024,
            extra_modes: &[],
        }))
    }

    #[test]
    fn boot_backend_enumerates_single_honest_mode() {
        let output = boot_backend();
        assert_eq!(
            output.current_mode(),
            DisplayModeInfo::new(1024, 768, 4, 1024)
        );
        assert_eq!(output.supported_modes().len(), 1);
        assert_eq!(output.supported_modes()[0], output.current_mode());
    }

    #[test]
    fn set_mode_accepts_active_mode_as_noop() {
        let output = boot_backend();
        assert_eq!(output.set_mode(1024, 768, 4), Ok(()));
    }

    #[test]
    fn set_mode_refuses_unlisted_geometry_as_unsupported() {
        let output = boot_backend();
        assert_eq!(
            output.set_mode(800, 600, 4),
            Err(DisplayOutputError::Unsupported)
        );
        assert_eq!(
            output.set_mode(1024, 768, 2),
            Err(DisplayOutputError::Unsupported)
        );
    }

    #[test]
    fn set_mode_reports_listed_but_unswitchable_as_busy() {
        let output = DisplayOutputObject::new(Arc::new(StubBackend {
            width: 640,
            height: 480,
            bpp: 4,
            stride: 640,
            extra_modes: &[("hd", 1920, 1080, 4, 1920)],
        }));
        assert_eq!(output.set_mode(640, 480, 4), Ok(()));
        assert_eq!(
            output.set_mode(1920, 1080, 4),
            Err(DisplayOutputError::Busy)
        );
    }
}

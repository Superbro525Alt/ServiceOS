use serviceos_abi::DisplayOutputInfo;

/// Maximum number of modes a backend may enumerate.
pub const MAX_DISPLAY_MODES: usize = 8;

/// A display timing/mode descriptor. Groundwork only: the boot framebuffer
/// exposes exactly one mode today, and `set_mode` refuses anything else.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DisplayModeInfo {
    pub width: u32,
    pub height: u32,
    pub bytes_per_pixel: u32,
    /// Row stride in pixels (matches [`serviceos_abi::DisplayOutputInfo`]).
    pub stride: u32,
}

impl DisplayModeInfo {
    pub const fn new(width: u32, height: u32, bytes_per_pixel: u32, stride: u32) -> Self {
        Self {
            width,
            height,
            bytes_per_pixel,
            stride,
        }
    }

    pub const fn from_output_info(info: &DisplayOutputInfo) -> Self {
        Self {
            width: info.width,
            height: info.height,
            bytes_per_pixel: info.bytes_per_pixel,
            stride: info.stride,
        }
    }

    /// Whether this mode satisfies the requested geometry exactly.
    pub const fn matches(&self, width: u32, height: u32, bytes_per_pixel: u32) -> bool {
        self.width == width && self.height == height && self.bytes_per_pixel == bytes_per_pixel
    }

    /// Byte length of a full frame in this mode.
    pub const fn byte_len(&self) -> u64 {
        self.stride as u64 * self.height as u64 * self.bytes_per_pixel as u64
    }
}

/// Find a mode matching the request, if the list contains one.
pub fn find_mode(
    modes: &[DisplayModeInfo],
    width: u32,
    height: u32,
    bytes_per_pixel: u32,
) -> Option<DisplayModeInfo> {
    modes
        .iter()
        .copied()
        .find(|mode| mode.matches(width, height, bytes_per_pixel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_from_output_info_copies_geometry() {
        let info = DisplayOutputInfo {
            backend: 1,
            state: 1,
            pixel_format: 1,
            reserved: 0,
            width: 1024,
            height: 768,
            stride: 1024,
            bytes_per_pixel: 4,
            byte_len: 1024 * 768 * 4,
            present_count: 0,
        };
        let mode = DisplayModeInfo::from_output_info(&info);
        assert_eq!(
            mode,
            DisplayModeInfo::new(1024, 768, 4, 1024),
            "current mode must mirror BootInfo-derived output geometry"
        );
        assert_eq!(mode.byte_len(), 1024 * 768 * 4);
    }

    #[test]
    fn find_mode_matches_exact_geometry_only() {
        let modes = [
            DisplayModeInfo::new(640, 480, 4, 640),
            DisplayModeInfo::new(1024, 768, 4, 1024),
        ];
        assert_eq!(
            find_mode(&modes, 1024, 768, 4),
            Some(DisplayModeInfo::new(1024, 768, 4, 1024))
        );
        assert_eq!(find_mode(&modes, 800, 600, 4), None);
        assert_eq!(find_mode(&modes, 1024, 768, 2), None);
        assert_eq!(find_mode(&[], 1, 1, 4), None);
    }

    #[test]
    fn matches_requires_all_three_fields() {
        let mode = DisplayModeInfo::new(1920, 1080, 4, 1920);
        assert!(mode.matches(1920, 1080, 4));
        assert!(!mode.matches(1919, 1080, 4));
        assert!(!mode.matches(1920, 1079, 4));
        assert!(!mode.matches(1920, 1080, 3));
    }

    #[test]
    fn byte_len_uses_stride_not_width() {
        let padded = DisplayModeInfo::new(100, 10, 4, 128);
        assert_eq!(padded.byte_len(), 128 * 10 * 4);
    }
}

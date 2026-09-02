//! Pure display-damage math and virtio-gpu 2D parameter constants.
//!
//! Split out of the platform virtio-gpu backend so the damage-rect algebra
//! and the wire-visible parameters the backend drives the vendored
//! `virtio-drivers` GPU device with are host-testable (the platform crate
//! builds `no_std` with `test = false`).

use core::cmp::{max, min};

/// Wire-visible parameters of the single-scanout 2D presentation path. The
/// ctrl-payload envelopes themselves are built by the vendored
/// `virtio-drivers` GPU driver; these constants pin the parameters this
/// backend passes through it (resource/scanout ids, X8888 format, command
/// opcodes from the virtio-gpu specification).
pub mod vgpu_wire {
    /// Fixed resource id for the framebuffer 2D resource (single-resource
    /// v0: one resource, one scanout, recreated only on teardown).
    pub const RESOURCE_ID_FB: u32 = 0;
    /// Single scanout the v0 backend drives.
    pub const SCANOUT_ID: u32 = 0;
    /// `VIRTIO_GPU_FORMAT_B8G8R8A8UNORM` — byte layout B,G,R,A in memory,
    /// identical to the UEFI GOP Xrgb8888 row bytes the service presents.
    pub const FORMAT_B8G8R8A8UNORM: u32 = 1;
    /// Bytes per pixel of every format the v0 path uses.
    pub const BYTES_PER_PIXEL: u32 = 4;

    /// Ctrl opcodes the v0 flow exercises, per the virtio-gpu spec
    /// (documentation for the vendored driver's wire stream).
    pub mod command {
        /// RESOURCE_CREATE_2D (0x101): allocate the 2D resource.
        pub const RESOURCE_CREATE_2D: u32 = 0x101;
        /// RESOURCE_ATTACH_BACKING (0x106): attach guest DMA backing.
        pub const RESOURCE_ATTACH_BACKING: u32 = 0x106;
        /// SET_SCANOUT (0x103): bind resource to the scanout.
        pub const SET_SCANOUT: u32 = 0x103;
        /// TRANSFER_TO_HOST_2D (0x105): copy guest backing to host resource.
        pub const TRANSFER_TO_HOST_2D: u32 = 0x105;
        /// RESOURCE_FLUSH (0x104): present host resource on the scanout.
        pub const RESOURCE_FLUSH: u32 = 0x104;
    }

    /// Packed scanout/resource word for SET_SCANOUT in the vendored
    /// driver's envelope: scanout in the low 16 bits, resource id above.
    pub const fn scanout_resource_word(scanout_id: u32, resource_id: u32) -> u32 {
        (scanout_id & 0xffff) | ((resource_id & 0xffff) << 16)
    }
}

/// A pixel-space damage rectangle: the region of a presented frame that
/// changed and therefore needs to reach the display backend's backing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DamageRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl DamageRect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Clamp to a `width x height` output, negative-origin safe. Empty
    /// (degenerate) results are possible and must be checked with
    /// [`DamageRect::is_empty`].
    pub fn clamped_to(&self, width: u32, height: u32) -> DamageRect {
        let start_x = self.x.max(0) as u32;
        let start_y = self.y.max(0) as u32;
        let end_x = ((self.x + self.width as i32).max(0) as u32).min(width);
        let end_y = ((self.y + self.height as i32).max(0) as u32).min(height);
        if start_x >= end_x || start_y >= end_y {
            return DamageRect::new(0, 0, 0, 0);
        }
        DamageRect {
            x: start_x as i32,
            y: start_y as i32,
            width: end_x - start_x,
            height: end_y - start_y,
        }
    }

    pub const fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Inclusive-exclusive byte range of the damaged columns within a row
    /// whose full stride is `stride_bytes`. Rows shorter than the rect are
    /// clamped (callers guarantee the rect fits the mode first).
    pub fn row_byte_span(&self, stride_bytes: usize, bytes_per_pixel: u32) -> (usize, usize) {
        let start = self.x.max(0) as usize * bytes_per_pixel as usize;
        let end = min(
            (self.x.max(0) as usize + self.width as usize) * bytes_per_pixel as usize,
            stride_bytes,
        );
        (start, end.max(start))
    }
}

/// Inclusive-exclusive byte span of the first/last differing byte between
/// two equal-length rows, `None` when identical. Unequal lengths compare as
/// fully dirty (defensive; callers pass same-length rows).
pub fn dirty_row_span(previous: &[u8], current: &[u8]) -> Option<(usize, usize)> {
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

/// Union of two damage rects bounding box (used when a present path must
/// coalesce damage from two sources). Empty inputs drop out.
pub fn union_rect(a: &DamageRect, b: &DamageRect) -> DamageRect {
    if a.is_empty() {
        return *b;
    }
    if b.is_empty() {
        return *a;
    }
    let left = min(a.x, b.x);
    let top = min(a.y, b.y);
    let right = max(a.x + a.width as i32, b.x + b.width as i32);
    let bottom = max(a.y + a.height as i32, b.y + b.height as i32);
    DamageRect {
        x: left,
        y: top,
        width: (right - left).max(0) as u32,
        height: (bottom - top).max(0) as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_keeps_in_damage_rect() {
        let rect = DamageRect::new(10, 20, 30, 40).clamped_to(1024, 768);
        assert_eq!(rect, DamageRect::new(10, 20, 30, 40));
        assert_eq!(rect.row_byte_span(1024 * 4, 4), (40, 160));
    }

    #[test]
    fn clamp_cuts_negative_origin() {
        let rect = DamageRect::new(-8, -4, 16, 8).clamped_to(100, 100);
        assert_eq!(rect, DamageRect::new(0, 0, 8, 4));
    }

    #[test]
    fn clamp_cuts_overrun_to_bounds() {
        let rect = DamageRect::new(90, 90, 40, 40).clamped_to(100, 100);
        assert_eq!(rect, DamageRect::new(90, 90, 10, 10));
    }

    #[test]
    fn clamp_reports_fully_offscreen_as_empty() {
        assert!(
            DamageRect::new(200, 0, 10, 10)
                .clamped_to(100, 100)
                .is_empty()
        );
        assert!(
            DamageRect::new(0, 200, 10, 10)
                .clamped_to(100, 100)
                .is_empty()
        );
        assert!(
            DamageRect::new(-20, 0, 10, 10)
                .clamped_to(100, 100)
                .is_empty()
        );
        assert!(DamageRect::new(0, 0, 0, 10).clamped_to(100, 100).is_empty());
    }

    #[test]
    fn row_byte_span_clamps_to_stride() {
        let rect = DamageRect::new(0, 0, 512, 1);
        assert_eq!(rect.row_byte_span(1024, 4), (0, 1024));
        assert_eq!(rect.row_byte_span(512 * 4, 4), (0, 2048));
    }

    #[test]
    fn row_byte_span_empty_rect_gives_empty_span() {
        let rect = DamageRect::new(0, 0, 0, 0);
        let (start, end) = rect.row_byte_span(1024, 4);
        assert_eq!(start, end);
    }

    #[test]
    fn dirty_row_span_finds_first_and_last_change() {
        assert_eq!(dirty_row_span(&[0, 0, 0, 0], &[0, 9, 9, 0]), Some((1, 3)));
        assert_eq!(dirty_row_span(&[1, 2, 3], &[1, 2, 3]), None);
    }

    #[test]
    fn dirty_row_span_treats_unequal_lengths_as_fully_dirty() {
        assert_eq!(dirty_row_span(&[0], &[0, 0]), Some((0, 2)));
    }

    #[test]
    fn dirty_row_span_single_byte_change() {
        assert_eq!(dirty_row_span(&[7, 7], &[7, 8]), Some((1, 2)));
    }

    #[test]
    fn union_bounding_boxes_and_drops_empty() {
        assert_eq!(
            union_rect(&DamageRect::new(0, 0, 4, 4), &DamageRect::new(8, 8, 4, 4)),
            DamageRect::new(0, 0, 12, 12)
        );
        assert_eq!(
            union_rect(&DamageRect::new(0, 0, 0, 0), &DamageRect::new(2, 2, 4, 4)),
            DamageRect::new(2, 2, 4, 4)
        );
        assert_eq!(
            union_rect(&DamageRect::new(5, 5, 1, 1), &DamageRect::new(0, 0, 0, 0)),
            DamageRect::new(5, 5, 1, 1)
        );
    }

    #[test]
    fn vgpu_wire_constants_match_virtio_gpu_spec() {
        use vgpu_wire::*;
        // Spec values the vendored driver sends; goldens pin our parameters
        // against accidental drift.
        assert_eq!(RESOURCE_ID_FB, 0);
        assert_eq!(SCANOUT_ID, 0);
        assert_eq!(FORMAT_B8G8R8A8UNORM, 1); // B8G8R8A8UNORM == GOP Xrgb8888
        assert_eq!(BYTES_PER_PIXEL, 4);
        assert_eq!(command::RESOURCE_CREATE_2D, 0x101);
        assert_eq!(command::RESOURCE_ATTACH_BACKING, 0x106);
        assert_eq!(command::SET_SCANOUT, 0x103);
        assert_eq!(command::TRANSFER_TO_HOST_2D, 0x105);
        assert_eq!(command::RESOURCE_FLUSH, 0x104);
    }

    #[test]
    fn vgpu_scanout_resource_word_packing() {
        use vgpu_wire::scanout_resource_word;
        assert_eq!(scanout_resource_word(0, 0), 0);
        assert_eq!(scanout_resource_word(1, 2), 0x2_0001);
        assert_eq!(scanout_resource_word(3, 4) & 0xffff, 3);
        assert_eq!(scanout_resource_word(3, 4) >> 16, 4);
    }

    #[test]
    fn vgpu_flow_parameter_block_is_single_scanout_x8888() {
        // The v0 flow: create_2d(FORMAT_X8888) -> attach_backing -> set_scanout
        // (scanout 0, resource 0) -> transfer_to_host_2d -> resource_flush.
        // Pins the exact ids/format used for every command envelope.
        use vgpu_wire::*;
        let rect = DamageRect::new(0, 0, 1024, 768);
        let packed = scanout_resource_word(SCANOUT_ID, RESOURCE_ID_FB);
        assert_eq!((rect.width, rect.height), (1024, 768));
        assert_eq!(packed, scanout_resource_word(0, RESOURCE_ID_FB));
        assert_eq!(FORMAT_B8G8R8A8UNORM, 1);
    }
}

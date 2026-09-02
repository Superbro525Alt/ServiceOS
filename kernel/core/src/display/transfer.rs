//! In-tree virtio-gpu control-wire packing for rect-scoped 2D transfers.
//!
//! The vendored `virtio-drivers` GPU driver keeps its control-queue request
//! structs private and only offers whole-resource `flush()`. These packers
//! build the wire-visible envelopes for the commands the x86 backend drives
//! itself (little-endian, as the virtio-gpu specification pins them), so a
//! present can ship exactly the dirty rectangle instead of the whole
//! resource. Byte layout is pinned by the goldens below and by the
//! constants in [`super::vgpu_wire`].

use super::{vgpu_wire, DamageRect};

/// `VIRTIO_GPU_RESP_OK_NODATA` — success reply for all no-data commands.
pub const OK_NODATA: u32 = 0x1100;
/// `VIRTIO_GPU_RESP_OK_DISPLAY_INFO` — success reply for get-display-info.
pub const OK_DISPLAY_INFO: u32 = 0x1101;

/// Control header: type(u32) flags(u32) fence_id(u64) ctx_id(u32) padding(u32).
pub const CTRL_HEADER_LEN: usize = 24;
/// CtrlHeader + Rect + offset(u64) + resource_id(u32) + padding(u32).
pub const TRANSFER_TO_HOST_2D_LEN: usize = CTRL_HEADER_LEN + 16 + 8 + 4 + 4;
/// CtrlHeader + Rect + resource_id(u32) + padding(u32).
pub const RESOURCE_FLUSH_LEN: usize = CTRL_HEADER_LEN + 16 + 4 + 4;
/// CtrlHeader + resource_id(u32) + format(u32) + width(u32) + height(u32).
pub const RESOURCE_CREATE_2D_LEN: usize = CTRL_HEADER_LEN + 16;
/// CtrlHeader + resource_id(u32) + nr_entries(u32) + addr(u64) + length(u32) + padding(u32).
pub const RESOURCE_ATTACH_BACKING_LEN: usize = CTRL_HEADER_LEN + 4 + 4 + 8 + 4 + 4;
/// CtrlHeader + Rect + scanout_id(u32) + resource_id(u32).
pub const SET_SCANOUT_LEN: usize = CTRL_HEADER_LEN + 16 + 8;
/// CtrlHeader only (get-display-info request).
pub const GET_DISPLAY_INFO_LEN: usize = CTRL_HEADER_LEN;

fn put32(buf: &mut [u8], at: usize, value: u32) {
    buf[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(buf: &mut [u8], at: usize, value: u64) {
    buf[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

/// Pack a zero-fence, zero-context control header at `buf[0..24]`.
///
/// Wire layout: type(u32) flags(u32) fence_id(u64) ctx_id(u32) padding(u32).
pub fn pack_ctrl_header(buf: &mut [u8], opcode: u32) {
    assert!(buf.len() >= CTRL_HEADER_LEN);
    put32(buf, 0, opcode);
    put32(buf, 4, 0); // flags
    put64(buf, 8, 0); // fence_id
    put32(buf, 16, 0); // ctx_id
    put32(buf, 20, 0); // padding
}

/// Pack `TRANSFER_TO_HOST_2D`: copy the backing-store rectangle starting at
/// byte `offset` to the host resource. Rect fields must be in-range for the
/// resource (caller clamps via [`DamageRect::clamped_to`]).
pub fn pack_transfer_to_host_2d(buf: &mut [u8], rect: &DamageRect, offset: u64, resource_id: u32) {
    assert!(buf.len() >= TRANSFER_TO_HOST_2D_LEN);
    pack_ctrl_header(buf, vgpu_wire::command::TRANSFER_TO_HOST_2D);
    pack_rect(buf, 24, rect);
    put64(buf, 40, offset);
    put32(buf, 48, resource_id);
    put32(buf, 52, 0); // padding
}

/// Pack `RESOURCE_FLUSH`: present the host-resource rectangle on the
/// bound scanout.
pub fn pack_resource_flush(buf: &mut [u8], rect: &DamageRect, resource_id: u32) {
    assert!(buf.len() >= RESOURCE_FLUSH_LEN);
    pack_ctrl_header(buf, vgpu_wire::command::RESOURCE_FLUSH);
    pack_rect(buf, 24, rect);
    put32(buf, 40, resource_id);
    put32(buf, 44, 0); // padding
}

/// Pack `RESOURCE_CREATE_2D` for the 2D framebuffer resource.
pub fn pack_resource_create_2d(buf: &mut [u8], resource_id: u32, width: u32, height: u32) {
    assert!(buf.len() >= RESOURCE_CREATE_2D_LEN);
    pack_ctrl_header(buf, vgpu_wire::command::RESOURCE_CREATE_2D);
    put32(buf, 24, resource_id);
    put32(buf, 28, vgpu_wire::FORMAT_B8G8R8A8UNORM);
    put32(buf, 32, width);
    put32(buf, 36, height);
}

/// Pack `RESOURCE_ATTACH_BACKING` with the single-entry descriptor table
/// this backend uses (one contiguous DMA region).
pub fn pack_resource_attach_backing(buf: &mut [u8], resource_id: u32, addr: u64, length: u32) {
    assert!(buf.len() >= RESOURCE_ATTACH_BACKING_LEN);
    pack_ctrl_header(buf, vgpu_wire::command::RESOURCE_ATTACH_BACKING);
    put32(buf, 24, resource_id);
    put32(buf, 28, 1); // nr_entries: always 1
    put64(buf, 32, addr);
    put32(buf, 40, length);
    put32(buf, 44, 0); // padding
}

/// Pack `SET_SCANOUT` binding `resource_id` to `scanout_id` over `rect`.
pub fn pack_set_scanout(buf: &mut [u8], rect: &DamageRect, scanout_id: u32, resource_id: u32) {
    assert!(buf.len() >= SET_SCANOUT_LEN);
    pack_ctrl_header(buf, vgpu_wire::command::SET_SCANOUT);
    pack_rect(buf, 24, rect);
    put32(buf, 40, scanout_id);
    put32(buf, 44, resource_id);
}

/// Pack the rect block (x, y, width, height as little-endian u32s) at `at`.
pub fn pack_rect(buf: &mut [u8], at: usize, rect: &DamageRect) {
    put32(buf, at, rect.x.max(0) as u32);
    put32(buf, at + 4, rect.y.max(0) as u32);
    put32(buf, at + 8, rect.width);
    put32(buf, at + 12, rect.height);
}

/// First word of a control response: the reply type. `None` when the
/// completion buffer is too short to hold a header.
pub fn response_type(buf: &[u8]) -> Option<u32> {
    if buf.len() < 4 {
        return None;
    }
    Some(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]))
}

/// Backing-store byte offset of the first pixel of `rect` for a resource
/// whose rows are `stride_bytes` long: `(y * stride + x) * 4`. Clamped
/// callers guarantee non-negative x/y.
pub fn backing_offset(rect: &DamageRect, stride_bytes: usize) -> u64 {
    (rect.y.max(0) as usize * stride_bytes
        + rect.x.max(0) as usize * vgpu_wire::BYTES_PER_PIXEL as usize) as u64
}

/// Exact number of backing bytes one transfer of `rect` reads:
/// `width * height * 4` (rows are contiguous in the backing because the
/// transfer strides by the resource stride on the host side).
pub fn transfer_bytes(rect: &DamageRect) -> usize {
    rect.width as usize * rect.height as usize * vgpu_wire::BYTES_PER_PIXEL as usize
}

/// Whether `rect` stays inside a `width x height` resource (the device
/// rejects out-of-range rects).
pub fn fits_resource(rect: &DamageRect, width: u32, height: u32) -> bool {
    if rect.is_empty() {
        return false;
    }
    let x = rect.x.max(0) as u32;
    let y = rect.y.max(0) as u32;
    x + rect.width <= width && y + rect.height <= height
}

/// Last backing byte (exclusive) a transfer of `rect` touches for a
/// resource with `stride_bytes` row stride — used by the goldens to prove
/// edge rects stay in the backing.
pub fn transfer_end(rect: &DamageRect, stride_bytes: usize) -> usize {
    backing_offset(rect, stride_bytes) as usize
        + (rect.height as usize - 1) * stride_bytes
        + rect.width as usize * vgpu_wire::BYTES_PER_PIXEL as usize
}

/// Running union of per-row dirty pixel spans into one bounding rect.
#[derive(Default)]
pub struct DirtyBounds {
    bounds: Option<DamageRect>,
}

impl DirtyBounds {
    pub const fn new() -> Self {
        Self { bounds: None }
    }

    /// Absorb one dirty byte span `[start, end)` on row `y` of a frame
    /// whose rows are `stride_bytes` wide. Byte positions round outward to
    /// pixel boundaries so partially-touched pixels stay covered.
    pub fn absorb_row_span(&mut self, y: usize, start: usize, end: usize) {
        if end <= start {
            return;
        }
        let bpp = vgpu_wire::BYTES_PER_PIXEL as usize;
        let x0 = start / bpp;
        let x1 = (end + bpp - 1) / bpp;
        let row = DamageRect::new(x0 as i32, y as i32, (x1 - x0) as u32, 1);
        self.bounds = Some(match self.bounds {
            Some(existing) => super::union_rect(&existing, &row),
            None => row,
        });
    }

    /// The bounding rect of every absorbed span, `None` when nothing was.
    pub fn bounding(&self) -> Option<DamageRect> {
        self.bounds
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vgpu_wire::{RESOURCE_ID_FB, SCANOUT_ID};

    #[test]
    fn vgpu_resource_id_matches_device_visible_value() {
        // The vendored driver's proven-on-device resource id; id 0 is
        // reserved/invalid per the virtio-gpu spec.
        assert_eq!(RESOURCE_ID_FB, 0xbabe);
    }

    #[test]
    fn control_header_packs_opcode_first_and_zeroes_rest() {
        let mut buf = [0xa5u8; 32];
        pack_ctrl_header(&mut buf, vgpu_wire::command::TRANSFER_TO_HOST_2D);
        assert_eq!(&buf[..24], &{
            let mut expect = [0u8; 24];
            expect[0..4].copy_from_slice(&0x105u32.to_le_bytes());
            expect
        });
        assert_eq!(&buf[24..], &[0xa5u8; 8]);
        assert_eq!(
            response_type(&buf),
            Some(vgpu_wire::command::TRANSFER_TO_HOST_2D)
        );
    }

    #[test]
    fn transfer_to_host_2d_wire_layout() {
        let rect = DamageRect::new(3, 2, 5, 4);
        let mut buf = [0u8; TRANSFER_TO_HOST_2D_LEN];
        pack_transfer_to_host_2d(&mut buf, &rect, 0x1234, RESOURCE_ID_FB);
        assert_eq!(TRANSFER_TO_HOST_2D_LEN, 56);
        assert_eq!(&buf[0..4], &0x105u32.to_le_bytes()); // opcode
        assert_eq!(&buf[24..28], &3u32.to_le_bytes()); // x
        assert_eq!(&buf[28..32], &2u32.to_le_bytes()); // y
        assert_eq!(&buf[32..36], &5u32.to_le_bytes()); // width
        assert_eq!(&buf[36..40], &4u32.to_le_bytes()); // height
        assert_eq!(&buf[40..48], &0x1234u64.to_le_bytes()); // offset
        assert_eq!(&buf[48..52], &0xbabeu32.to_le_bytes()); // resource id
        assert_eq!(&buf[52..56], &[0, 0, 0, 0]); // padding
    }

    #[test]
    fn resource_flush_wire_layout() {
        let rect = DamageRect::new(1, 1, 2, 2);
        let mut buf = [0u8; RESOURCE_FLUSH_LEN];
        pack_resource_flush(&mut buf, &rect, RESOURCE_ID_FB);
        assert_eq!(RESOURCE_FLUSH_LEN, 48);
        assert_eq!(&buf[0..4], &0x104u32.to_le_bytes());
        assert_eq!(
            &buf[24..40],
            &[1, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 2, 0, 0, 0]
        );
        assert_eq!(&buf[40..44], &0xbabeu32.to_le_bytes());
        assert_eq!(&buf[44..48], &[0, 0, 0, 0]);
    }

    #[test]
    fn resource_create_2d_wire_layout_pins_format() {
        let mut buf = [0u8; RESOURCE_CREATE_2D_LEN];
        pack_resource_create_2d(&mut buf, RESOURCE_ID_FB, 1280, 800);
        assert_eq!(RESOURCE_CREATE_2D_LEN, 40);
        assert_eq!(&buf[0..4], &0x101u32.to_le_bytes());
        assert_eq!(&buf[24..28], &0xbabeu32.to_le_bytes());
        assert_eq!(&buf[28..32], &vgpu_wire::FORMAT_B8G8R8A8UNORM.to_le_bytes());
        assert_eq!(&buf[32..36], &1280u32.to_le_bytes());
        assert_eq!(&buf[36..40], &800u32.to_le_bytes());
    }

    #[test]
    fn resource_attach_backing_wire_layout() {
        let mut buf = [0u8; RESOURCE_ATTACH_BACKING_LEN];
        pack_resource_attach_backing(&mut buf, RESOURCE_ID_FB, 0xcafe0000, 4096000);
        assert_eq!(RESOURCE_ATTACH_BACKING_LEN, 48);
        assert_eq!(&buf[0..4], &0x106u32.to_le_bytes());
        assert_eq!(&buf[24..28], &0xbabeu32.to_le_bytes());
        assert_eq!(&buf[28..32], &1u32.to_le_bytes()); // nr_entries
        assert_eq!(&buf[32..40], &0xcafe0000u64.to_le_bytes());
        assert_eq!(&buf[40..44], &4096000u32.to_le_bytes());
        assert_eq!(&buf[44..48], &[0, 0, 0, 0]);
    }

    #[test]
    fn set_scanout_wire_layout() {
        let mut buf = [0u8; SET_SCANOUT_LEN];
        pack_set_scanout(
            &mut buf,
            &DamageRect::new(0, 0, 1280, 800),
            SCANOUT_ID,
            RESOURCE_ID_FB,
        );
        assert_eq!(SET_SCANOUT_LEN, 48);
        assert_eq!(&buf[0..4], &0x103u32.to_le_bytes());
        assert_eq!(&buf[40..44], &SCANOUT_ID.to_le_bytes());
        assert_eq!(&buf[44..48], &0xbabeu32.to_le_bytes());
    }

    #[test]
    fn response_type_parses_le_and_rejects_short() {
        assert_eq!(response_type(&[0x00, 0x11, 0x00, 0x00]), Some(OK_NODATA));
        assert_eq!(
            response_type(&[0x01, 0x11, 0x00, 0x00]),
            Some(OK_DISPLAY_INFO)
        );
        assert_eq!(response_type(&[]), None);
        assert_eq!(response_type(&[1, 2, 3]), None);
    }

    #[test]
    fn backing_offset_rect_math_matrix() {
        let stride = 1280 * 4;
        // Full frame starts at offset 0 and covers the whole backing.
        let full = DamageRect::new(0, 0, 1280, 800);
        assert_eq!(backing_offset(&full, stride), 0);
        assert_eq!(transfer_bytes(&full), 800 * stride);
        assert_eq!(transfer_end(&full, stride), 800 * stride);
        // Bottom row only.
        let bottom = DamageRect::new(0, 799, 1280, 1);
        assert_eq!(backing_offset(&bottom, stride), (799 * stride) as u64);
        assert_eq!(transfer_end(&bottom, stride), 800 * stride);
        // Right column only.
        let right = DamageRect::new(1279, 0, 1, 800);
        assert_eq!(backing_offset(&right, stride), (1279 * 4) as u64);
        assert_eq!(transfer_end(&right, stride), 799 * stride + 1280 * 4);
        // Bottom-right corner pixel.
        let corner = DamageRect::new(1279, 799, 1, 1);
        assert_eq!(
            backing_offset(&corner, stride),
            (799 * stride + 1279 * 4) as u64
        );
        assert_eq!(transfer_end(&corner, stride), 800 * stride);
        // Arbitrary interior rect.
        let inner = DamageRect::new(10, 20, 30, 40);
        assert_eq!(
            backing_offset(&inner, stride),
            ((20 * 1280 + 10) * 4) as u64
        );
        assert_eq!(transfer_bytes(&inner), 30 * 40 * 4);
    }

    #[test]
    fn edge_rects_stay_within_backing() {
        let stride = 1280 * 4;
        let backing_len = 800 * stride;
        for rect in [
            DamageRect::new(0, 0, 1280, 800),
            DamageRect::new(0, 799, 1280, 1),
            DamageRect::new(1279, 0, 1, 800),
            DamageRect::new(1279, 799, 1, 1),
            DamageRect::new(0, 0, 1, 1),
            DamageRect::new(640, 400, 640, 400),
        ] {
            assert!(fits_resource(&rect, 1280, 800));
            assert!(transfer_end(&rect, stride) <= backing_len);
        }
    }

    #[test]
    fn fits_resource_rejects_empty_and_overrun() {
        assert!(!fits_resource(&DamageRect::new(0, 0, 0, 5), 1280, 800));
        assert!(!fits_resource(&DamageRect::new(1279, 0, 2, 1), 1280, 800));
        assert!(!fits_resource(&DamageRect::new(0, 799, 1, 2), 1280, 800));
    }

    #[test]
    fn dirty_bounds_unions_rows_into_bounding_rect() {
        let mut bounds = DirtyBounds::new();
        assert!(bounds.bounding().is_none());
        bounds.absorb_row_span(3, 8, 20);
        assert_eq!(bounds.bounding(), Some(DamageRect::new(2, 3, 3, 1)));
        bounds.absorb_row_span(7, 0, 4);
        assert_eq!(bounds.bounding(), Some(DamageRect::new(0, 3, 5, 5)));
        // Empty spans never contribute.
        bounds.absorb_row_span(9, 5, 5);
        assert_eq!(bounds.bounding(), Some(DamageRect::new(0, 3, 5, 5)));
    }

    #[test]
    fn dirty_bounds_rounds_partial_pixels_outward() {
        let mut bounds = DirtyBounds::new();
        // Bytes 1..7 of a row touch pixels 0 (partially) and 1 (fully).
        bounds.absorb_row_span(0, 1, 7);
        assert_eq!(bounds.bounding(), Some(DamageRect::new(0, 0, 2, 1)));
    }
}

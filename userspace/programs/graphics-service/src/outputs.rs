use core::{ptr, slice};

use serviceos_userspace_runtime as rt;

use crate::compose::PresentOutcome;
use crate::types::{DamageRect, MAX_FRAMEBUFFER_BYTES};

/// Registry capacity today: the boot framebuffer plus one virtual mirror.
/// Backing stores for further virtual outputs are deferred honestly.
pub(crate) const MAX_OUTPUTS: usize = 2;
/// Backend ids on the wire: kernel boot framebuffer reports 1
/// (`kernel/core/src/display/mode.rs`); service-local virtual outputs use 2.
pub(crate) const OUTPUT_BACKEND_VIRTUAL: u32 = 2;

/// Control-op tags for on-demand secondary output creation. Kept local to the
/// service so the shared ABI stays untouched; values sit in the unallocated
/// 0x910 range of the graphics tag space and are additive by construction.
pub(crate) const OUTPUT_CREATE_REQUEST_TAG: u32 = 0x910;
pub(crate) const OUTPUT_CREATE_REPLY_TAG: u32 = 0x911;

const VIRTUAL_BYTES_PER_PIXEL: u32 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OutputKind {
    Primary,
    VirtualMirror,
}

#[derive(Clone, Copy)]
pub(crate) struct OutputSlot {
    pub(crate) id: u32,
    pub(crate) occupied: bool,
    pub(crate) kind: OutputKind,
    pub(crate) handle: rt::Handle,
    pub(crate) info: rt::DisplayOutputInfo,
    pub(crate) present_count: u64,
    pub(crate) noop_skips: u64,
    pub(crate) noop_saved_bytes: u64,
}

impl OutputSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            id: 0,
            occupied: false,
            kind: OutputKind::Primary,
            handle: rt::INVALID_HANDLE,
            info: rt::DisplayOutputInfo {
                backend: 0,
                state: 0,
                pixel_format: 0,
                reserved: 0,
                width: 0,
                height: 0,
                stride: 0,
                bytes_per_pixel: 0,
                byte_len: 0,
                present_count: 0,
            },
            present_count: 0,
            noop_skips: 0,
            noop_saved_bytes: 0,
        }
    }

    pub(crate) fn record_outcome(&mut self, outcome: &PresentOutcome) {
        self.present_count = self.present_count.saturating_add(1);
        if outcome.skipped {
            self.noop_skips = self.noop_skips.saturating_add(1);
            self.noop_saved_bytes = self
                .noop_saved_bytes
                .saturating_add(outcome.saved_bytes);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OutputCreateError {
    CapacityExceeded,
    GeometryUnsupported,
}

#[derive(Clone, Copy)]
pub(crate) struct OutputRegistry {
    slots: [OutputSlot; MAX_OUTPUTS],
    next_output_id: u32,
}

impl OutputRegistry {
    pub(crate) const fn new() -> Self {
        Self {
            slots: [OutputSlot::empty(); MAX_OUTPUTS],
            next_output_id: 1,
        }
    }

    pub(crate) fn register_primary(
        &mut self,
        handle: rt::Handle,
        info: rt::DisplayOutputInfo,
    ) -> Option<u32> {
        let slot = self.slots.iter_mut().find(|slot| !slot.occupied)?;
        slot.id = self.next_output_id;
        self.next_output_id = self.next_output_id.saturating_add(1);
        slot.occupied = true;
        slot.kind = OutputKind::Primary;
        slot.handle = handle;
        slot.info = info;
        Some(slot.id)
    }

    /// Create a memory-backed mirror output modeled on `template` (the
    /// primary): it shares the primary's state and pixel format so raw row
    /// blits are meaningful; there is no kernel object — presentation lands
    /// in in-process surfaces (headless plug-test semantics).
    pub(crate) fn create_virtual_mirror(
        &mut self,
        template: &rt::DisplayOutputInfo,
        width: u32,
        height: u32,
    ) -> Result<u32, OutputCreateError> {
        if width == 0 || height == 0 {
            return Err(OutputCreateError::GeometryUnsupported);
        }
        let byte_len = (width as u64)
            .saturating_mul(height as u64)
            .saturating_mul(VIRTUAL_BYTES_PER_PIXEL as u64);
        if byte_len > MAX_FRAMEBUFFER_BYTES as u64 {
            return Err(OutputCreateError::CapacityExceeded);
        }
        if self.active_count() >= MAX_OUTPUTS {
            return Err(OutputCreateError::CapacityExceeded);
        }
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| !slot.occupied)
            .ok_or(OutputCreateError::CapacityExceeded)?;
        slot.id = self.next_output_id;
        self.next_output_id = self.next_output_id.saturating_add(1);
        slot.occupied = true;
        slot.kind = OutputKind::VirtualMirror;
        slot.handle = rt::INVALID_HANDLE;
        slot.info = rt::DisplayOutputInfo {
            backend: OUTPUT_BACKEND_VIRTUAL,
            state: template.state,
            pixel_format: template.pixel_format,
            reserved: 0,
            width,
            height,
            stride: width,
            bytes_per_pixel: VIRTUAL_BYTES_PER_PIXEL,
            byte_len,
            present_count: 0,
        };
        Ok(slot.id)
    }

    pub(crate) fn primary(&self) -> Option<&OutputSlot> {
        self.slots
            .iter()
            .find(|slot| slot.occupied && slot.kind == OutputKind::Primary)
    }

    pub(crate) fn primary_mut(&mut self) -> Option<&mut OutputSlot> {
        self.slots
            .iter_mut()
            .find(|slot| slot.occupied && slot.kind == OutputKind::Primary)
    }

    pub(crate) fn virtual_mirror_mut(&mut self) -> Option<&mut OutputSlot> {
        self.slots
            .iter_mut()
            .find(|slot| slot.occupied && slot.kind == OutputKind::VirtualMirror)
    }

    pub(crate) fn has_virtual_mirror(&self) -> bool {
        self.slots
            .iter()
            .any(|slot| slot.occupied && slot.kind == OutputKind::VirtualMirror)
    }

    pub(crate) fn active_count(&self) -> usize {
        self.slots.iter().filter(|slot| slot.occupied).count()
    }

    /// Slot lookup by enumeration index (stable slot order).
    pub(crate) fn by_index(&self, index: usize) -> Option<&OutputSlot> {
        self.slots.get(index).filter(|slot| slot.occupied)
    }

    /// Fill `ids` with active output ids; returns the count written.
    pub(crate) fn enumerate_ids(&self, ids: &mut [u32]) -> usize {
        let mut written = 0usize;
        for slot in self.slots.iter().filter(|slot| slot.occupied) {
            if written >= ids.len() {
                break;
            }
            ids[written] = slot.id;
            written += 1;
        }
        written
    }
}

static mut VIRTUAL_FRAMEBUFFER_BYTES: [u8; MAX_FRAMEBUFFER_BYTES] = [0; MAX_FRAMEBUFFER_BYTES];
static mut VIRTUAL_PRESENTED_BYTES: [u8; MAX_FRAMEBUFFER_BYTES] = [0; MAX_FRAMEBUFFER_BYTES];

fn virtual_frame_slice(len: usize) -> &'static mut [u8] {
    unsafe {
        slice::from_raw_parts_mut(
            ptr::addr_of_mut!(VIRTUAL_FRAMEBUFFER_BYTES).cast::<u8>(),
            len,
        )
    }
}

fn virtual_presented_slice(len: usize) -> &'static mut [u8] {
    unsafe {
        slice::from_raw_parts_mut(
            ptr::addr_of_mut!(VIRTUAL_PRESENTED_BYTES).cast::<u8>(),
            len,
        )
    }
}

/// Map a damage rect from primary coordinates into a mirror output's
/// coordinates under nearest-neighbour scaling, clipped to both bounds.
/// Returns `None` when the mapped region is empty.
pub(crate) fn mirror_damage_rect(
    primary: &rt::DisplayOutputInfo,
    secondary: &rt::DisplayOutputInfo,
    damage: DamageRect,
) -> Option<DamageRect> {
    if primary.width == 0 || primary.height == 0 || secondary.width == 0 || secondary.height == 0 {
        return None;
    }
    let src_x0 = damage.x.clamp(0, primary.width as i32) as u64;
    let src_y0 = damage.y.clamp(0, primary.height as i32) as u64;
    let src_x1 = (damage.x.saturating_add(damage.width as i32)).clamp(0, primary.width as i32) as u64;
    let src_y1 =
        (damage.y.saturating_add(damage.height as i32)).clamp(0, primary.height as i32) as u64;
    if src_x0 >= src_x1 || src_y0 >= src_y1 {
        return None;
    }
    let scale_x = secondary.width as u64;
    let scale_y = secondary.height as u64;
    let primary_width = primary.width as u64;
    let primary_height = primary.height as u64;
    let dst_x0 = (src_x0 * scale_x) / primary_width;
    let dst_y0 = (src_y0 * scale_y) / primary_height;
    let dst_x1 = ((src_x1 * scale_x) + primary_width - 1) / primary_width;
    let dst_y1 = ((src_y1 * scale_y) + primary_height - 1) / primary_height;
    let dst_x1 = dst_x1.min(secondary.width as u64);
    let dst_y1 = dst_y1.min(secondary.height as u64);
    if dst_x0 >= dst_x1 || dst_y0 >= dst_y1 {
        return None;
    }
    Some(DamageRect {
        x: dst_x0 as i32,
        y: dst_y0 as i32,
        width: (dst_x1 - dst_x0) as u32,
        height: (dst_y1 - dst_y0) as u32,
    })
}

/// Nearest-neighbour full-frame blit from a source framebuffer into a mirror
/// destination. Row bytes are `stride_pixels * bytes_per_pixel`; padding
/// between rows is never read past `byte_len`. Returns `false` when formats
/// are incompatible or buffers are too small.
pub(crate) fn mirror_blit(
    dst: &mut [u8],
    dst_info: &rt::DisplayOutputInfo,
    src: &[u8],
    src_info: &rt::DisplayOutputInfo,
) -> bool {
    if dst_info.bytes_per_pixel != src_info.bytes_per_pixel || dst_info.bytes_per_pixel == 0 {
        return false;
    }
    if dst.len() < dst_info.byte_len as usize || src.len() < src_info.byte_len as usize {
        return false;
    }
    let bpp = dst_info.bytes_per_pixel as usize;
    let dst_stride = dst_info.stride as usize * bpp;
    let src_stride = src_info.stride as usize * bpp;
    let dw = dst_info.width as u64;
    let dh = dst_info.height as u64;
    let sw = src_info.width as u64;
    let sh = src_info.height as u64;
    let (sw, sh) = (sw.max(1), sh.max(1));
    if dw == 0 || dh == 0 {
        return false;
    }
    for dy in 0..dh as usize {
        let sy = (dy as u64 * sh / dh) as usize;
        let dst_row = dy * dst_stride;
        let src_row = sy * src_stride;
        if dw as usize == sw as usize && dst_info.stride == src_info.stride {
            let row_len = dw as usize * bpp;
            dst[dst_row..dst_row + row_len].copy_from_slice(&src[src_row..src_row + row_len]);
            continue;
        }
        for dx in 0..dw as usize {
            let sx = (dx as u64 * sw / dw) as usize;
            let dst_at = dst_row + dx * bpp;
            let src_at = src_row + sx * bpp;
            dst[dst_at..dst_at + bpp].copy_from_slice(&src[src_at..src_at + bpp]);
        }
    }
    true
}

struct MirrorSpan {
    row_start: usize,
    row_end: usize,
    col_start: usize,
    col_end: usize,
    stride_bytes: usize,
}

impl MirrorSpan {
    fn rows_identical(&self, left: &[u8], right: &[u8]) -> bool {
        for row in self.row_start..self.row_end {
            let offset = row * self.stride_bytes;
            if left[offset + self.col_start..offset + self.col_end]
                != right[offset + self.col_start..offset + self.col_end]
            {
                return false;
            }
        }
        true
    }

    fn update_rows(&self, presented: &mut [u8], frame: &[u8]) {
        for row in self.row_start..self.row_end {
            let offset = row * self.stride_bytes;
            let range = offset + self.col_start..offset + self.col_end;
            presented[range.clone()].copy_from_slice(&frame[range]);
        }
    }
}

fn mirror_span(
    output: &rt::DisplayOutputInfo,
    damage: DamageRect,
) -> Option<MirrorSpan> {
    let start_x = damage.x.max(0) as usize;
    let start_y = damage.y.max(0) as usize;
    let end_x =
        ((damage.x.saturating_add(damage.width as i32)).max(0) as usize).min(output.width as usize);
    let end_y = ((damage.y.saturating_add(damage.height as i32)).max(0) as usize)
        .min(output.height as usize);
    if start_x >= end_x || start_y >= end_y {
        return None;
    }
    let stride_bytes = output.stride as usize * output.bytes_per_pixel as usize;
    Some(MirrorSpan {
        row_start: start_y,
        row_end: end_y,
        col_start: start_x * output.bytes_per_pixel as usize,
        col_end: end_x * output.bytes_per_pixel as usize,
        stride_bytes,
    })
}

/// Core mirror-present step over caller-owned buffers so it stays unit
/// testable without touching the service statics. `damage` is already in
/// secondary coordinates (`None` means full-frame).
pub(crate) fn mirror_present_into(
    dst_frame: &mut [u8],
    dst_presented: &mut [u8],
    secondary: &rt::DisplayOutputInfo,
    src: &[u8],
    primary: &rt::DisplayOutputInfo,
    damage: Option<DamageRect>,
    allow_noop_skip: bool,
) -> PresentOutcome {
    if !mirror_blit(dst_frame, secondary, src, primary) {
        return PresentOutcome::presented();
    }
    let rect = damage.unwrap_or(DamageRect {
        x: 0,
        y: 0,
        width: secondary.width,
        height: secondary.height,
    });
    if allow_noop_skip {
        if let (Some(span), true) = (
            mirror_span(secondary, rect),
            dst_presented.len() == dst_frame.len(),
        ) {
            if span.rows_identical(dst_presented, dst_frame) {
                return PresentOutcome::noop(span.col_start.abs_diff(span.col_end) as u64
                    * (span.row_end - span.row_start) as u64);
            }
            span.update_rows(dst_presented, dst_frame);
            return PresentOutcome::presented();
        }
    }
    if let Some(span) = mirror_span(secondary, rect) {
        if dst_presented.len() == dst_frame.len() {
            span.update_rows(dst_presented, dst_frame);
        }
    }
    PresentOutcome::presented()
}

/// Refresh every virtual mirror from the primary's just-presented shadow.
/// Purely in-memory: no kernel calls, per-output skip accounting. When
/// `primary_damage` carries the region the primary just repainted, it is
/// scaled into mirror coordinates so only those rows are compared/copied;
/// `None` mirrors the whole frame.
pub(crate) fn refresh_virtual_mirrors(
    registry: &mut OutputRegistry,
    primary_damage: Option<DamageRect>,
) {
    if !registry.has_virtual_mirror() {
        return;
    }
    let Some(primary_info) = registry.primary().map(|slot| slot.info) else {
        return;
    };
    let (secondary_info, allow_skip) = match registry.virtual_mirror_mut() {
        Some(slot) => (slot.info, slot.present_count > 0),
        None => return,
    };
    if let Some(rect) = primary_damage {
        let mapped = mirror_damage_rect(&primary_info, &secondary_info, rect);
        if mapped.is_none() {
            return;
        }
    }
    let primary_byte_len = primary_info.byte_len as usize;
    let src = crate::compose::presented_frame_slice(primary_byte_len);
    let outcome = {
        let dst = virtual_frame_slice(secondary_info.byte_len as usize);
        let shadow = virtual_presented_slice(secondary_info.byte_len as usize);
        mirror_present_into(
            dst,
            shadow,
            &secondary_info,
            &src[..primary_byte_len],
            &primary_info,
            primary_damage
                .and_then(|rect| mirror_damage_rect(&primary_info, &secondary_info, rect)),
            allow_skip,
        )
    };
    if let Some(slot) = registry.virtual_mirror_mut() {
        slot.record_outcome(&outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kernel boot-framebuffer backend id (display/mode.rs).
    const OUTPUT_BACKEND_BOOT_FB: u32 = 1;

    fn info(width: u32, height: u32, stride: u32, bpp: u32, backend: u32) -> rt::DisplayOutputInfo {
        rt::DisplayOutputInfo {
            backend,
            state: 1,
            pixel_format: 1,
            reserved: 0,
            width,
            height,
            stride,
            bytes_per_pixel: bpp,
            byte_len: stride as u64 * height as u64 * bpp as u64,
            present_count: 0,
        }
    }

    fn rect(x: i32, y: i32, width: u32, height: u32) -> DamageRect {
        DamageRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn registry_registers_primary_and_enumerates() {
        let mut registry = OutputRegistry::new();
        assert!(registry.primary().is_none());
        assert_eq!(registry.active_count(), 0);

        let id = registry
            .register_primary(42, info(1024, 768, 1024, 4, OUTPUT_BACKEND_BOOT_FB))
            .expect("empty registry accepts primary");
        assert_eq!(id, 1);
        assert_eq!(registry.active_count(), 1);
        assert_eq!(registry.by_index(0).map(|slot| slot.id), Some(id));
        assert!(registry.primary().is_some());

        let mut ids = [0u32; MAX_OUTPUTS];
        assert_eq!(registry.enumerate_ids(&mut ids), 1);
        assert_eq!(ids[0], id);
    }

    #[test]
    fn registry_virtual_create_assigns_ids_and_enforces_capacity() {
        let mut registry = OutputRegistry::new();
        let primary_id = registry
            .register_primary(7, info(640, 480, 640, 4, OUTPUT_BACKEND_BOOT_FB))
            .unwrap();
        let template = registry.primary().unwrap().info;
        let mirror_id = registry
            .create_virtual_mirror(&template, 320, 240)
            .expect("capacity 2 accepts one virtual mirror");
        assert_ne!(primary_id, mirror_id);

        let slot = registry.by_index(1).unwrap();
        assert_eq!(slot.kind, OutputKind::VirtualMirror);
        assert_eq!(slot.info.backend, OUTPUT_BACKEND_VIRTUAL);
        assert_eq!(slot.info.width, 320);
        assert_eq!(slot.info.height, 240);
        assert_eq!(
            slot.info.pixel_format,
            registry.primary().unwrap().info.pixel_format
        );
        assert_eq!(slot.handle, rt::INVALID_HANDLE);

        let overflow = registry.create_virtual_mirror(&template, 10, 10);
        assert_eq!(overflow, Err(OutputCreateError::CapacityExceeded));

        let bad_geometry = registry.create_virtual_mirror(&template, 0, 240);
        assert_eq!(bad_geometry, Err(OutputCreateError::GeometryUnsupported));

        let oversized = registry.create_virtual_mirror(&template, u32::MAX, u32::MAX);
        assert_eq!(oversized, Err(OutputCreateError::CapacityExceeded));
        assert_eq!(registry.active_count(), 2);
    }

    #[test]
    fn registry_per_output_stats_are_isolated() {
        let mut registry = OutputRegistry::new();
        registry
            .register_primary(7, info(640, 480, 640, 4, OUTPUT_BACKEND_BOOT_FB))
            .unwrap();
        let template = registry.primary().unwrap().info;
        registry
            .create_virtual_mirror(&template, 320, 240)
            .unwrap();

        registry.primary_mut().unwrap().record_outcome(&PresentOutcome::presented());
        registry
            .virtual_mirror_mut()
            .unwrap()
            .record_outcome(&PresentOutcome::noop(4096));

        let primary = registry.primary().unwrap();
        assert_eq!(primary.present_count, 1);
        assert_eq!(primary.noop_skips, 0);
        assert_eq!(primary.noop_saved_bytes, 0);

        let mirror = registry.virtual_mirror_mut().unwrap();
        assert_eq!(mirror.present_count, 1);
        assert_eq!(mirror.noop_skips, 1);
        assert_eq!(mirror.noop_saved_bytes, 4096);
    }

    #[test]
    fn mirror_blit_identity_copies_rows_exactly() {
        let out = info(4, 3, 4, 4, OUTPUT_BACKEND_BOOT_FB);
        let src = vec![0xA5u8; out.byte_len as usize];
        let mut dst = vec![0u8; out.byte_len as usize];
        assert!(mirror_blit(&mut dst, &out, &src, &out));
        assert_eq!(dst, src);
    }

    #[test]
    fn mirror_blit_handles_wider_source_stride() {
        let src_info = info(4, 2, 6, 4, OUTPUT_BACKEND_BOOT_FB);
        let dst_info = info(4, 2, 4, 4, OUTPUT_BACKEND_VIRTUAL);
        let mut src = vec![0u8; src_info.byte_len as usize];
        for y in 0..2usize {
            for x in 0..4usize {
                let at = y * 6 * 4 + x * 4;
                src[at] = (y * 4 + x) as u8;
            }
        }
        let mut dst = vec![0u8; dst_info.byte_len as usize];
        assert!(mirror_blit(&mut dst, &dst_info, &src, &src_info));
        for y in 0..2usize {
            for x in 0..4usize {
                let at = y * 4 * 4 + x * 4;
                assert_eq!(dst[at], (y * 4 + x) as u8);
            }
        }
    }

    #[test]
    fn mirror_blit_scales_nearest_neighbour() {
        let src_info = info(2, 2, 2, 4, OUTPUT_BACKEND_BOOT_FB);
        let dst_info = info(4, 4, 4, 4, OUTPUT_BACKEND_VIRTUAL);
        let mut src = vec![0u8; src_info.byte_len as usize];
        src[0] = 0x11;
        src[4] = 0x22;
        src[8] = 0x33;
        src[12] = 0x44;
        let mut dst = vec![0u8; dst_info.byte_len as usize];
        assert!(mirror_blit(&mut dst, &dst_info, &src, &src_info));
        // Row r starts at r*stride*bpp = r*16.
        assert_eq!(dst[0], 0x11);
        assert_eq!(dst[12], 0x22);
        assert_eq!(dst[32], 0x33);
        assert_eq!(dst[44], 0x44);

        let down_src = info(4, 4, 4, 4, OUTPUT_BACKEND_BOOT_FB);
        let down_dst = info(2, 2, 2, 4, OUTPUT_BACKEND_VIRTUAL);
        let big = vec![0x77u8; down_src.byte_len as usize];
        let mut small = vec![0u8; down_dst.byte_len as usize];
        assert!(mirror_blit(&mut small, &down_dst, &big, &down_src));
        assert_eq!(small, vec![0x77u8; small.len()]);
    }

    #[test]
    fn mirror_blit_rejects_bad_formats_and_short_buffers() {
        let four = info(4, 2, 4, 4, OUTPUT_BACKEND_BOOT_FB);
        let two = info(4, 2, 4, 2, OUTPUT_BACKEND_VIRTUAL);
        let src = vec![0u8; four.byte_len as usize];
        let mut dst = vec![0u8; two.byte_len as usize];
        assert!(!mirror_blit(&mut dst, &two, &src, &four));
        assert!(!mirror_blit(&mut [], &two, &src, &four));
        assert!(!mirror_blit(&mut dst, &two, &[], &four));
    }

    #[test]
    fn mirror_damage_rect_passes_through_when_same_size() {
        let primary = info(100, 80, 100, 4, OUTPUT_BACKEND_BOOT_FB);
        let secondary = info(100, 80, 100, 4, OUTPUT_BACKEND_VIRTUAL);
        assert_eq!(
            mirror_damage_rect(&primary, &secondary, rect(10, 20, 30, 15)),
            Some(rect(10, 20, 30, 15))
        );
        assert_eq!(
            mirror_damage_rect(&primary, &secondary, rect(-5, -5, 10, 10)),
            Some(rect(0, 0, 5, 5))
        );
        assert_eq!(
            mirror_damage_rect(&primary, &secondary, rect(90, 70, 50, 50)),
            Some(rect(90, 70, 10, 10))
        );
        assert_eq!(
            mirror_damage_rect(&primary, &secondary, rect(-20, 0, 10, 10)),
            None
        );
    }

    #[test]
    fn mirror_damage_rect_scales_between_sizes() {
        let primary = info(100, 100, 100, 4, OUTPUT_BACKEND_BOOT_FB);
        let half = info(50, 50, 50, 4, OUTPUT_BACKEND_VIRTUAL);
        assert_eq!(
            mirror_damage_rect(&primary, &half, rect(10, 20, 30, 40)),
            Some(rect(5, 10, 15, 20))
        );
        let double = info(200, 200, 200, 4, OUTPUT_BACKEND_VIRTUAL);
        assert_eq!(
            mirror_damage_rect(&primary, &double, rect(10, 10, 25, 25)),
            Some(rect(20, 20, 50, 50))
        );
    }

    #[test]
    fn mirror_present_into_full_frame_tracks_shadow() {
        let primary = info(4, 2, 4, 4, OUTPUT_BACKEND_BOOT_FB);
        let secondary = info(4, 2, 4, 4, OUTPUT_BACKEND_VIRTUAL);
        let src = vec![1u8; primary.byte_len as usize];
        let mut frame = vec![0u8; secondary.byte_len as usize];
        let mut shadow = vec![0u8; secondary.byte_len as usize];

        let first = mirror_present_into(
            &mut frame,
            &mut shadow,
            &secondary,
            &src,
            &primary,
            None,
            true,
        );
        assert!(!first.skipped);
        assert_eq!(frame, src);
        assert_eq!(shadow, src);

        let second = mirror_present_into(
            &mut frame,
            &mut shadow,
            &secondary,
            &src,
            &primary,
            None,
            true,
        );
        assert!(second.skipped);
        assert_eq!(second.saved_bytes, secondary.byte_len);
    }

    #[test]
    fn mirror_present_into_damage_updates_only_mapped_rows() {
        let primary = info(4, 2, 4, 4, OUTPUT_BACKEND_BOOT_FB);
        let secondary = info(4, 2, 4, 4, OUTPUT_BACKEND_VIRTUAL);
        let mut frame = vec![0u8; secondary.byte_len as usize];
        let mut shadow = vec![0xAAu8; secondary.byte_len as usize];

        let mut src = vec![0x55u8; primary.byte_len as usize];
        let first = mirror_present_into(
            &mut frame,
            &mut shadow,
            &secondary,
            &src,
            &primary,
            None,
            false,
        );
        assert!(!first.skipped);

        src[..16].fill(0xEE);
        let second = mirror_present_into(
            &mut frame,
            &mut shadow,
            &secondary,
            &src,
            &primary,
            Some(rect(0, 0, 4, 1)),
            false,
        );
        assert!(!second.skipped);
        // Top row refreshed, bottom row still holds the previous frame.
        assert_eq!(&shadow[..16], &[0xEE; 16]);
        assert_eq!(&shadow[16..], &[0x55; 16]);
    }
}

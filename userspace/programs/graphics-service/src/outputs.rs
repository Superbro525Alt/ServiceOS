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

/// Service-local control-op tags for EXTEND-mode configuration. Same
/// additive policy as the 0x910 pair: unallocated graphics tag space,
/// shared ABI untouched. Request words: [1]=output id, [2]=side (1 =
/// right-of-primary, 2 = left-of-primary). Reply words: [0]=status,
/// [1..2]=render origin x/y, [3..6]=combined desktop bounds rect.
pub(crate) const OUTPUT_EXTEND_REQUEST_TAG: u32 = 0x914;
pub(crate) const OUTPUT_EXTEND_REPLY_TAG: u32 = 0x915;

/// Presentation relationship of a virtual output to the primary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OutputMode {
    /// Nearest-neighbour mirror of the primary's presented frame
    /// (legacy behavior).
    Mirror,
    /// Own desktop-space rectangle beside the primary; not refreshed from
    /// the primary (independent content is future work — the surface is
    /// reserved by placement today).
    Extend,
}

/// Side of the primary an EXTEND-mode output is placed on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExtendSide {
    RightOfPrimary = 1,
    LeftOfPrimary = 2,
}

impl ExtendSide {
    pub(crate) fn from_word(word: u64) -> Option<Self> {
        match word {
            1 => Some(Self::RightOfPrimary),
            2 => Some(Self::LeftOfPrimary),
            _ => None,
        }
    }
}

/// Axis-aligned desktop-space rectangle (origin top-left, y down).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DesktopRect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl DesktopRect {
    fn right(&self) -> i32 {
        self.x.saturating_add(self.width as i32)
    }

    fn bottom(&self) -> i32 {
        self.y.saturating_add(self.height as i32)
    }

    /// Bounding box of two rects. When outputs have differing heights the
    /// union is non-rectangular; v0 pointer math clamps to the bounding box
    /// and per-output hit-testing arrives with real input routing.
    fn bounding_union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Self {
            x,
            y,
            width: self.right().max(other.right()).saturating_sub(x) as u32,
            height: self.bottom().max(other.bottom()).saturating_sub(y) as u32,
        }
    }

    fn clamp_point(&self, x: i32, y: i32) -> (i32, i32) {
        (
            x.clamp(self.x, self.right().saturating_sub(1)),
            y.clamp(self.y, self.bottom().saturating_sub(1)),
        )
    }
}

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
    /// Presentation mode (mirror vs extend); primaries are always
    /// effectively mirror-at-origin.
    pub(crate) mode: OutputMode,
    /// Desktop-space origin used when `mode == Extend`.
    pub(crate) desktop_origin: (i32, i32),
    pub(crate) handle: rt::Handle,
    pub(crate) info: rt::DisplayOutputInfo,
    pub(crate) present_count: u64,
    pub(crate) noop_skips: u64,
    pub(crate) noop_saved_bytes: u64,
    pub(crate) band_saved_bytes: u64,
}

/// The display ABI defines `stride` as the row stride in pixels with
/// `byte_len == stride * height * bytes_per_pixel` (kernel display/mode.rs).
/// Boot-framebuffer backends on some platforms report the stride in bytes
/// instead; `byte_len` stays authoritative because the kernel present path
/// validates frames against it. Re-derive the pixel stride from `byte_len`
/// whenever the reported triple would index past the real buffer, keeping
/// every compose/present row calculation inside the framebuffer.
pub(crate) fn reconcile_output_stride(mut info: rt::DisplayOutputInfo) -> rt::DisplayOutputInfo {
    let height = info.height as u64;
    let bytes_per_pixel = info.bytes_per_pixel as u64;
    if info.width == 0 || height == 0 || bytes_per_pixel == 0 {
        return info;
    }
    if info.stride as u64 * height * bytes_per_pixel <= info.byte_len {
        return info;
    }
    info.stride = (info.byte_len / (height * bytes_per_pixel)) as u32;
    info
}

impl OutputSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            id: 0,
            occupied: false,
            kind: OutputKind::Primary,
            mode: OutputMode::Mirror,
            desktop_origin: (0, 0),
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
            band_saved_bytes: 0,
        }
    }

    pub(crate) fn record_outcome(&mut self, outcome: &PresentOutcome) {
        self.present_count = self.present_count.saturating_add(1);
        if outcome.skipped {
            self.noop_skips = self.noop_skips.saturating_add(1);
            self.noop_saved_bytes = self.noop_saved_bytes.saturating_add(outcome.saved_bytes);
        }
        self.band_saved_bytes = self
            .band_saved_bytes
            .saturating_add(outcome.band_saved_bytes);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OutputCreateError {
    CapacityExceeded,
    GeometryUnsupported,
    /// No occupied output with the requested id.
    NotFound,
    /// The output exists but cannot take the requested mode (e.g. placing
    /// the primary in EXTEND mode).
    ModeUnsupported,
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
        slot.mode = OutputMode::Mirror;
        slot.desktop_origin = (0, 0);
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
        slot.mode = OutputMode::Mirror;
        slot.desktop_origin = (0, 0);
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

    /// The mirror-mode virtual output the refresh path blits into.
    fn active_mirror_mut(&mut self) -> Option<&mut OutputSlot> {
        self.slots.iter_mut().find(|slot| {
            slot.occupied
                && slot.kind == OutputKind::VirtualMirror
                && slot.mode == OutputMode::Mirror
        })
    }

    fn by_id(&self, id: u32) -> Option<&OutputSlot> {
        self.slots
            .iter()
            .find(|slot| slot.occupied && slot.id == id)
    }

    fn by_id_mut(&mut self, id: u32) -> Option<&mut OutputSlot> {
        self.slots
            .iter_mut()
            .find(|slot| slot.occupied && slot.id == id)
    }

    /// Place a virtual output in EXTEND mode on `side` of the primary,
    /// top-aligned with zero gap. Returns its desktop-space render origin.
    /// Only virtual outputs can be placed; the primary stays at (0, 0).
    pub(crate) fn configure_extend(
        &mut self,
        output_id: u32,
        side: ExtendSide,
    ) -> Result<(i32, i32), OutputCreateError> {
        let Some(target) = self.by_id(output_id) else {
            return Err(OutputCreateError::NotFound);
        };
        if target.kind != OutputKind::VirtualMirror {
            // The primary anchors desktop space; it cannot be re-placed.
            return Err(OutputCreateError::ModeUnsupported);
        }
        let target_size = (target.info.width as i32, target.info.height as i32);
        let Some(primary) = self.primary() else {
            return Err(OutputCreateError::NotFound);
        };
        let origin = match side {
            ExtendSide::RightOfPrimary => (primary.info.width as i32, 0),
            ExtendSide::LeftOfPrimary => (-(target_size.0), 0),
        };
        if let Some(slot) = self.by_id_mut(output_id) {
            slot.mode = OutputMode::Extend;
            slot.desktop_origin = origin;
        }
        Ok(origin)
    }

    /// Return an output to mirror mode; clears its desktop placement.
    pub(crate) fn revert_to_mirror(&mut self, output_id: u32) -> bool {
        let Some(slot) = self.by_id_mut(output_id) else {
            return false;
        };
        if slot.mode == OutputMode::Mirror {
            return false;
        }
        slot.mode = OutputMode::Mirror;
        slot.desktop_origin = (0, 0);
        true
    }

    /// Combined desktop bounds: primary rect unioned with every EXTEND-mode
    /// output's placed rect (bounding box when heights differ). Mirror-mode
    /// outputs share the primary's rect and add nothing.
    pub(crate) fn desktop_bounds(&self) -> Option<DesktopRect> {
        let primary = self.primary()?;
        let mut bounds = DesktopRect {
            x: 0,
            y: 0,
            width: primary.info.width,
            height: primary.info.height,
        };
        for slot in self
            .slots
            .iter()
            .filter(|slot| slot.occupied && slot.mode == OutputMode::Extend)
        {
            bounds = bounds.bounding_union(DesktopRect {
                x: slot.desktop_origin.0,
                y: slot.desktop_origin.1,
                width: slot.info.width,
                height: slot.info.height,
            });
        }
        Some(bounds)
    }

    /// Clamp a pointer position into the combined desktop bounds so the
    /// cursor cannot leave the desktop when moving across outputs.
    pub(crate) fn clamp_pointer(&self, x: i32, y: i32) -> Option<(i32, i32)> {
        self.desktop_bounds().map(|bounds| bounds.clamp_point(x, y))
    }

    /// Desktop-space origin each output renders at: the primary and any
    /// mirror sit at (0, 0); EXTEND outputs carry their placement.
    pub(crate) fn render_origin(&self, output_id: u32) -> Option<(i32, i32)> {
        self.by_id(output_id).map(|slot| slot.desktop_origin)
    }

    pub(crate) fn has_virtual_mirror(&self) -> bool {
        self.slots.iter().any(|slot| {
            slot.occupied
                && slot.kind == OutputKind::VirtualMirror
                && slot.mode == OutputMode::Mirror
        })
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
        slice::from_raw_parts_mut(ptr::addr_of_mut!(VIRTUAL_PRESENTED_BYTES).cast::<u8>(), len)
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
    let src_x1 =
        (damage.x.saturating_add(damage.width as i32)).clamp(0, primary.width as i32) as u64;
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

fn mirror_span(output: &rt::DisplayOutputInfo, damage: DamageRect) -> Option<MirrorSpan> {
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
                return PresentOutcome::noop(
                    span.col_start.abs_diff(span.col_end) as u64
                        * (span.row_end - span.row_start) as u64,
                );
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
    let (secondary_info, allow_skip) = match registry.active_mirror_mut() {
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
    if let Some(slot) = registry.active_mirror_mut() {
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

    fn extend_registry() -> (OutputRegistry, u32) {
        let mut registry = OutputRegistry::new();
        registry
            .register_primary(7, info(1024, 768, 1024, 4, OUTPUT_BACKEND_BOOT_FB))
            .unwrap();
        let template = registry.primary().unwrap().info;
        let id = registry.create_virtual_mirror(&template, 800, 600).unwrap();
        (registry, id)
    }

    #[test]
    fn extend_places_right_of_primary_with_own_geometry() {
        let (mut registry, id) = extend_registry();
        let origin = registry
            .configure_extend(id, ExtendSide::RightOfPrimary)
            .expect("virtual output accepts EXTEND");
        // Own geometry: placed at the primary's right edge, top-aligned.
        assert_eq!(origin, (1024, 0));
        assert_eq!(
            registry.render_origin(id),
            Some((1024, 0)),
            "render offset math exposes the placement"
        );
        // Primary anchors desktop space at the origin.
        assert_eq!(registry.render_origin(1), Some((0, 0)));
    }

    #[test]
    fn extend_places_left_of_primary_at_negative_x() {
        let (mut registry, id) = extend_registry();
        let origin = registry
            .configure_extend(id, ExtendSide::LeftOfPrimary)
            .expect("left-of placement");
        assert_eq!(origin, (-800, 0));
    }

    #[test]
    fn extend_rejects_unknown_and_primary_targets() {
        let (mut registry, _id) = extend_registry();
        assert_eq!(
            registry.configure_extend(999, ExtendSide::RightOfPrimary),
            Err(OutputCreateError::NotFound)
        );
        // The primary cannot be re-placed; it defines desktop origin.
        let primary_id = registry.primary().unwrap().id;
        assert_eq!(
            registry.configure_extend(primary_id, ExtendSide::RightOfPrimary),
            Err(OutputCreateError::ModeUnsupported)
        );
        assert_eq!(
            ExtendSide::from_word(3),
            None,
            "wire words outside the table are rejected"
        );
    }

    #[test]
    fn revert_to_mirror_clears_placement() {
        let (mut registry, id) = extend_registry();
        registry
            .configure_extend(id, ExtendSide::RightOfPrimary)
            .unwrap();
        assert!(registry.revert_to_mirror(id));
        assert_eq!(registry.render_origin(id), Some((0, 0)));
        // Already-mirror outputs report false (nothing to clear).
        assert!(!registry.revert_to_mirror(id));
        assert!(!registry.revert_to_mirror(999));
    }

    #[test]
    fn combined_desktop_bounds_union_primary_and_extended_outputs() {
        let (mut registry, id) = extend_registry();
        // Mirror-only desktop is exactly the primary rect.
        let base = registry.desktop_bounds().unwrap();
        assert_eq!(
            base,
            DesktopRect {
                x: 0,
                y: 0,
                width: 1024,
                height: 768
            }
        );
        registry
            .configure_extend(id, ExtendSide::RightOfPrimary)
            .unwrap();
        let bounds = registry.desktop_bounds().unwrap();
        // Secondary has its own geometry (800x600): bounding box keeps the
        // taller primary height.
        assert_eq!(
            bounds,
            DesktopRect {
                x: 0,
                y: 0,
                width: 1824,
                height: 768
            }
        );
    }

    #[test]
    fn combined_bounds_span_negative_left_placement() {
        let (mut registry, id) = extend_registry();
        registry
            .configure_extend(id, ExtendSide::LeftOfPrimary)
            .unwrap();
        let bounds = registry.desktop_bounds().unwrap();
        assert_eq!(
            bounds,
            DesktopRect {
                x: -800,
                y: 0,
                width: 1824,
                height: 768
            }
        );
    }

    #[test]
    fn pointer_clamps_to_combined_desktop_bounds() {
        let mut registry = OutputRegistry::new();
        registry
            .register_primary(7, info(1024, 768, 1024, 4, OUTPUT_BACKEND_BOOT_FB))
            .unwrap();
        // No secondary yet: clamping stays inside the primary.
        assert_eq!(
            registry.clamp_pointer(-5, -1),
            Some((0, 0)),
            "negative positions clamp to the top-left corner"
        );
        assert_eq!(registry.clamp_pointer(5000, 90), Some((1023, 90)));

        let template = registry.primary().unwrap().info;
        let id = registry.create_virtual_mirror(&template, 800, 600).unwrap();
        registry
            .configure_extend(id, ExtendSide::RightOfPrimary)
            .unwrap();
        // Combined bounds now reach x=1823.
        assert_eq!(registry.clamp_pointer(1500, 400), Some((1500, 400)));
        assert_eq!(
            registry.clamp_pointer(9999, 9999),
            Some((1823, 767)),
            "far-out positions clamp to the bottom-right of the union"
        );
        assert_eq!(registry.clamp_pointer(-100, 10), Some((0, 10)));
        // A degenerate desktop without a primary has no bounds at all.
        let empty = OutputRegistry::new();
        assert_eq!(empty.clamp_pointer(0, 0), None);
    }

    #[test]
    fn registry_per_output_stats_are_isolated() {
        let mut registry = OutputRegistry::new();
        registry
            .register_primary(7, info(640, 480, 640, 4, OUTPUT_BACKEND_BOOT_FB))
            .unwrap();
        let template = registry.primary().unwrap().info;
        registry.create_virtual_mirror(&template, 320, 240).unwrap();

        registry
            .primary_mut()
            .unwrap()
            .record_outcome(&PresentOutcome::presented());
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

    #[test]
    fn reconcile_output_stride_rederives_pixel_stride_from_byte_len() {
        // aarch64 virt boot-framebuffer shape: the backend reports the row
        // stride in bytes while byte_len stays the real buffer size.
        let mut boot = info(1024, 768, 4096, 4, OUTPUT_BACKEND_BOOT_FB);
        boot.byte_len = 1024 * 768 * 4;
        let fixed = reconcile_output_stride(boot);
        assert_eq!(fixed.stride, 1024);
        assert_eq!(fixed.byte_len, 1024 * 768 * 4);
        assert_eq!(fixed.width, 1024);
        assert_eq!(fixed.height, 768);
        assert_eq!(fixed.bytes_per_pixel, 4);
    }

    #[test]
    fn reconcile_output_stride_keeps_abi_conformant_info() {
        let conformant = info(1024, 768, 1024, 4, OUTPUT_BACKEND_BOOT_FB);
        assert_eq!(reconcile_output_stride(conformant), conformant);
        let padded = info(1000, 600, 1024, 4, OUTPUT_BACKEND_BOOT_FB);
        assert_eq!(reconcile_output_stride(padded), padded);
    }

    #[test]
    fn reconcile_output_stride_leaves_zeroed_info_untouched() {
        let empty = OutputSlot::empty().info;
        assert_eq!(reconcile_output_stride(empty), empty);
    }
}

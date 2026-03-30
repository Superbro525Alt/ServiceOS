use serviceos_userspace_runtime as rt;

pub(crate) const MAX_SURFACES: usize = 16;
pub(crate) const MAX_FRAMEBUFFER_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_SURFACE_RECTS: usize = 16;
pub(crate) const MAX_SURFACE_LABELS: usize = 16;
pub(crate) const MAX_SURFACE_BUFFERS: usize = 2;
pub(crate) const MAX_LABEL_BYTES: usize = 56;
pub(crate) const MAX_BUFFER_ROW_BYTES: usize = 8192;
pub(crate) const DEFAULT_BACKGROUND_RGB: u32 = 0x10151d;
pub(crate) const PRESENT_COALESCE_TICKS: u64 = 2;
pub(crate) const CURSOR_SURFACE_Z_ORDER_MIN: u32 = 4_096;
pub(crate) const CURSOR_SURFACE_MAX_SIZE: u32 = 64;

pub(crate) type Surfaces = [SurfaceSlot; MAX_SURFACES];

#[derive(Clone, Copy)]
pub(crate) struct RectSlot {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) color_rgb: u32,
    pub(crate) visible: bool,
    pub(crate) occupied: bool,
}

impl RectSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            color_rgb: 0,
            visible: false,
            occupied: false,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct LabelSlot {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) color_rgb: u32,
    pub(crate) len: usize,
    pub(crate) bytes: [u8; MAX_LABEL_BYTES],
    pub(crate) occupied: bool,
}

impl LabelSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            x: 0,
            y: 0,
            color_rgb: 0,
            len: 0,
            bytes: [0; MAX_LABEL_BYTES],
            occupied: false,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct BufferBinding {
    pub(crate) handle: rt::Handle,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) stride_pixels: u32,
    pub(crate) mapped_ptr: *mut u8,
}

impl BufferBinding {
    pub(crate) const fn empty() -> Self {
        Self {
            handle: rt::INVALID_HANDLE,
            width: 0,
            height: 0,
            stride_pixels: 0,
            mapped_ptr: core::ptr::null_mut(),
        }
    }

    pub(crate) const fn attached(self) -> bool {
        self.handle != rt::INVALID_HANDLE
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SurfaceSlot {
    pub(crate) id: u32,
    pub(crate) owner_session: u32,
    pub(crate) endpoint: rt::Handle,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) z_order: u32,
    pub(crate) fill_rgb: u32,
    pub(crate) visible: bool,
    pub(crate) occupied: bool,
    pub(crate) buffers: [BufferBinding; MAX_SURFACE_BUFFERS],
    pub(crate) active_buffer_slot: Option<usize>,
    pub(crate) rects: [RectSlot; MAX_SURFACE_RECTS],
    pub(crate) labels: [LabelSlot; MAX_SURFACE_LABELS],
}

impl SurfaceSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            id: 0,
            owner_session: 0,
            endpoint: rt::INVALID_HANDLE,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            z_order: 0,
            fill_rgb: 0,
            visible: false,
            occupied: false,
            buffers: [BufferBinding::empty(); MAX_SURFACE_BUFFERS],
            active_buffer_slot: None,
            rects: [RectSlot::empty(); MAX_SURFACE_RECTS],
            labels: [LabelSlot::empty(); MAX_SURFACE_LABELS],
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct DamageRect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl DamageRect {
    pub(crate) const fn empty() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        }
    }

    pub(crate) fn merge(self, other: Self) -> Self {
        if self.width == 0 || self.height == 0 {
            return other;
        }
        if other.width == 0 || other.height == 0 {
            return self;
        }
        let left = self.x.min(other.x);
        let top = self.y.min(other.y);
        let right = self
            .x
            .saturating_add(self.width as i32)
            .max(other.x.saturating_add(other.width as i32));
        let bottom = self
            .y
            .saturating_add(self.height as i32)
            .max(other.y.saturating_add(other.height as i32));
        Self {
            x: left,
            y: top,
            width: right.saturating_sub(left) as u32,
            height: bottom.saturating_sub(top) as u32,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum DirtyState {
    Clean,
    CursorOnly(DamageRect),
    Region { damage: DamageRect, immediate: bool },
    Full { immediate: bool },
}

pub(crate) fn active_surface_count(surfaces: &Surfaces) -> usize {
    surfaces.iter().filter(|surface| surface.occupied).count()
}

pub(crate) fn find_surface(surfaces: &Surfaces, surface_id: u32) -> Option<&SurfaceSlot> {
    surfaces
        .iter()
        .find(|surface| surface.occupied && surface.id == surface_id)
}

pub(crate) fn release_surface(surface: &mut SurfaceSlot) {
    if surface.endpoint != rt::INVALID_HANDLE {
        let _ = rt::handle_close(surface.endpoint);
    }
    for buffer in &mut surface.buffers {
        if buffer.attached() {
            let _ = rt::handle_close(buffer.handle);
        }
    }
    *surface = SurfaceSlot::empty();
}

pub(crate) fn surface_bounds(surface: &SurfaceSlot) -> DamageRect {
    DamageRect {
        x: surface.x,
        y: surface.y,
        width: surface.width,
        height: surface.height,
    }
}

pub(crate) fn is_cursor_surface(surface: &SurfaceSlot) -> bool {
    surface.z_order >= CURSOR_SURFACE_Z_ORDER_MIN
        && surface.width <= CURSOR_SURFACE_MAX_SIZE
        && surface.height <= CURSOR_SURFACE_MAX_SIZE
}

pub(crate) fn active_buffer(surface: &SurfaceSlot) -> Option<BufferBinding> {
    surface
        .active_buffer_slot
        .and_then(|slot| surface.buffers.get(slot).copied())
        .filter(|buffer| buffer.attached())
}

pub(crate) fn attached_buffer_count(surface: &SurfaceSlot) -> usize {
    surface.buffers.iter().filter(|buffer| buffer.attached()).count()
}

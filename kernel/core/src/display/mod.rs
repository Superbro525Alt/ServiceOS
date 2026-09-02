mod backend;
mod damage;
mod mode;
mod transfer;

pub use backend::{DisplayBackend, DisplayOutputError, DisplayOutputObject};
pub use damage::{DamageRect, dirty_row_span, union_rect, vgpu_wire};
pub use mode::{DisplayModeInfo, MAX_DISPLAY_MODES, find_mode};
pub use transfer::{
    DirtyBounds, GET_DISPLAY_INFO_LEN, OK_DISPLAY_INFO, OK_NODATA, RESOURCE_ATTACH_BACKING_LEN,
    RESOURCE_CREATE_2D_LEN, RESOURCE_FLUSH_LEN, SET_SCANOUT_LEN, TRANSFER_TO_HOST_2D_LEN,
    backing_offset, fits_resource, pack_ctrl_header, pack_rect, pack_resource_attach_backing,
    pack_resource_create_2d, pack_resource_flush, pack_set_scanout, pack_transfer_to_host_2d,
    response_type, transfer_bytes, transfer_end,
};

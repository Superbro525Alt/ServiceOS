use serviceos_userspace_runtime as rt;
use rt::{GraphicsStatus, RawMessage, SurfaceTag};

use crate::{
    types::{
        BufferBinding, DirtyState, MAX_BUFFER_ROW_BYTES, MAX_LABEL_BYTES,
        MAX_SURFACE_BUFFERS, MAX_SURFACE_LABELS, MAX_SURFACE_MESSAGES_PER_SLOT_PER_TURN,
        MAX_SURFACE_RECTS, MAX_SURFACE_REQUESTS_PER_TURN, SurfaceSlot, Surfaces,
        is_cursor_surface, release_surface, surface_bounds,
    },
};

use super::common::{merge_region_dirty, reply_surface_status, unpack_bytes};

fn visible_surface_damage(surface: &SurfaceSlot) -> crate::types::DamageRect {
    if surface.visible {
        surface_bounds(surface)
    } else {
        crate::types::DamageRect::empty()
    }
}

fn mark_surface_dirty(dirty: &mut DirtyState, surface: &SurfaceSlot, immediate: bool) {
    let damage = visible_surface_damage(surface);
    if damage.width != 0 && damage.height != 0 {
        merge_region_dirty(dirty, damage, immediate);
    }
}

pub(crate) fn drain_surface_requests(
    surfaces: &mut Surfaces,
    dirty: &mut DirtyState,
) -> rt::Result<bool> {
    let mut had_work = false;
    let mut processed = 0usize;
    for surface in surfaces {
        if !surface.occupied {
            continue;
        }
        let mut surface_processed = 0usize;
        let mut deferred: Option<RawMessage> = None;
        loop {
            if processed >= MAX_SURFACE_REQUESTS_PER_TURN {
                return Ok(had_work);
            }
            if surface_processed >= MAX_SURFACE_MESSAGES_PER_SLOT_PER_TURN {
                break;
            }
            let mut message = deferred.take().unwrap_or(RawMessage::empty(0));
            let receive_result = if message.tag == 0 {
                rt::channel_receive_nonblocking(surface.endpoint, &mut message)
            } else {
                Ok(())
            };
            match receive_result {
                Ok(()) => {
                    had_work = true;
                    processed += 1;
                    surface_processed += 1;
                    if is_async_geometry_request(&message) {
                        deferred = coalesce_async_geometry_request(
                            surface,
                            &mut message,
                            processed,
                            surface_processed,
                        )?;
                    }
                    handle_surface_request(surface, &message, dirty)?;
                }
                Err(rt::Error::QueueEmpty) => break,
                Err(_) => {
                    let old_rect = visible_surface_damage(surface);
                    release_surface(surface);
                    if old_rect.width != 0 && old_rect.height != 0 {
                        merge_region_dirty(dirty, old_rect, false);
                    }
                    break;
                }
            }
        }
    }
    Ok(had_work)
}

fn is_async_geometry_request(message: &RawMessage) -> bool {
    message.tag == SurfaceTag::SetGeometryRequest as u32 && message.handle_count == 0
}

fn coalesce_async_geometry_request(
    surface: &mut SurfaceSlot,
    message: &mut RawMessage,
    processed: usize,
    surface_processed: usize,
) -> rt::Result<Option<RawMessage>> {
    let mut processed = processed;
    let mut surface_processed = surface_processed;
    loop {
        if processed >= MAX_SURFACE_REQUESTS_PER_TURN
            || surface_processed >= MAX_SURFACE_MESSAGES_PER_SLOT_PER_TURN
        {
            return Ok(None);
        }
        let mut next = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(surface.endpoint, &mut next) {
            Ok(()) => {
                processed += 1;
                surface_processed += 1;
                if is_async_geometry_request(&next) {
                    *message = next;
                    continue;
                }
                return Ok(Some(next));
            }
            Err(rt::Error::QueueEmpty) => return Ok(None),
            Err(error) => return Err(error),
        }
    }
}

fn handle_surface_request(
    surface: &mut SurfaceSlot,
    message: &RawMessage,
    dirty: &mut DirtyState,
) -> rt::Result<()> {
    match message.tag {
        x if x == SurfaceTag::SetGeometryRequest as u32 => {
            if message.word_count < 5 {
                return Ok(());
            }
            let old_rect = surface_bounds(surface);
            surface.x = message.words[0] as i64 as i32;
            surface.y = message.words[1] as i64 as i32;
            surface.width = message.words[2] as u32;
            surface.height = message.words[3] as u32;
            surface.z_order = message.words[4] as u32;
            let new_rect = surface_bounds(surface);
            let damage = old_rect.merge(new_rect);
            if is_cursor_surface(surface) && !matches!(dirty, DirtyState::Full { .. }) {
                *dirty = match *dirty {
                    DirtyState::CursorOnly(existing) => DirtyState::CursorOnly(existing.merge(damage)),
                    DirtyState::Region { damage: existing, immediate } => DirtyState::Region {
                        damage: existing.merge(damage),
                        immediate,
                    },
                    DirtyState::Clean => DirtyState::CursorOnly(damage),
                    DirtyState::Full { immediate } => DirtyState::Full { immediate },
                };
            } else {
                merge_region_dirty(dirty, damage, false);
            }
            reply_surface_status(
                message.handles,
                message.handle_count,
                SurfaceTag::SetGeometryReply,
                GraphicsStatus::Ok,
            );
        }
        x if x == SurfaceTag::SetFillRequest as u32 => {
            if message.word_count < 1 || message.handle_count < 1 {
                return Ok(());
            }
            surface.fill_rgb = message.words[0] as u32;
            mark_surface_dirty(dirty, surface, false);
            reply_surface_status(
                message.handles,
                message.handle_count,
                SurfaceTag::SetFillReply,
                GraphicsStatus::Ok,
            );
        }
        x if x == SurfaceTag::SetVisibilityRequest as u32 => {
            if message.word_count < 1 {
                return Ok(());
            }
            let old_rect = visible_surface_damage(surface);
            surface.visible = message.words[0] != 0;
            let new_rect = visible_surface_damage(surface);
            let damage = old_rect.merge(new_rect);
            if damage.width != 0 && damage.height != 0 {
                merge_region_dirty(dirty, damage, false);
            }
            reply_surface_status(
                message.handles,
                message.handle_count,
                SurfaceTag::SetVisibilityReply,
                GraphicsStatus::Ok,
            );
        }
        x if x == SurfaceTag::ClearSceneRequest as u32 => {
            if message.handle_count < 1 {
                return Ok(());
            }
            surface.rects = [crate::types::RectSlot::empty(); MAX_SURFACE_RECTS];
            surface.labels = [crate::types::LabelSlot::empty(); MAX_SURFACE_LABELS];
            mark_surface_dirty(dirty, surface, false);
            reply_surface_status(
                message.handles,
                message.handle_count,
                SurfaceTag::ClearSceneReply,
                GraphicsStatus::Ok,
            );
        }
        x if x == SurfaceTag::SetRectRequest as u32 => {
            if message.word_count < 7 || message.handle_count < 1 {
                return Ok(());
            }
            let slot_index = message.words[0] as usize;
            let status = if let Some(slot) = surface.rects.get_mut(slot_index) {
                slot.x = message.words[1] as i64 as i32;
                slot.y = message.words[2] as i64 as i32;
                slot.width = message.words[3] as u32;
                slot.height = message.words[4] as u32;
                slot.color_rgb = message.words[5] as u32;
                slot.visible = message.words[6] != 0;
                slot.occupied = slot.visible;
                mark_surface_dirty(dirty, surface, false);
                GraphicsStatus::Ok
            } else {
                GraphicsStatus::CapacityExceeded
            };
            reply_surface_status(
                message.handles,
                message.handle_count,
                SurfaceTag::SetRectReply,
                status,
            );
        }
        x if x == SurfaceTag::SetLabelRequest as u32 => {
            if message.word_count < 5 || message.handle_count < 1 {
                return Ok(());
            }
            let slot_index = message.words[0] as usize;
            let status = if let Some(slot) = surface.labels.get_mut(slot_index) {
                let text_len = message.words[4] as usize;
                if text_len > MAX_LABEL_BYTES
                    || unpack_bytes(
                        &message.words[5..message.word_count as usize],
                        text_len,
                        &mut slot.bytes,
                    )
                    .is_err()
                {
                    GraphicsStatus::CapacityExceeded
                } else {
                    slot.x = message.words[1] as i64 as i32;
                    slot.y = message.words[2] as i64 as i32;
                    slot.color_rgb = message.words[3] as u32;
                    slot.len = text_len;
                    slot.occupied = text_len != 0;
                    mark_surface_dirty(dirty, surface, false);
                    GraphicsStatus::Ok
                }
            } else {
                GraphicsStatus::CapacityExceeded
            };
            reply_surface_status(
                message.handles,
                message.handle_count,
                SurfaceTag::SetLabelReply,
                status,
            );
        }
        x if x == SurfaceTag::AttachBufferRequest as u32 => {
            if message.word_count < 4 || message.handle_count < 2 {
                return Ok(());
            }
            let slot = message.words[0] as usize;
            let width = message.words[1] as u32;
            let height = message.words[2] as u32;
            let stride_pixels = message.words[3] as u32;
            let status = if slot >= MAX_SURFACE_BUFFERS
                || width == 0
                || height == 0
                || stride_pixels < width
                || width as usize * 4 > MAX_BUFFER_ROW_BYTES
            {
                GraphicsStatus::CapacityExceeded
            } else {
                let mapped_ptr = match rt::memory_map(message.handles[1], false) {
                    Ok(ptr) => ptr,
                    Err(rt::Error::PermissionDenied) => core::ptr::null_mut(),
                    Err(_) => core::ptr::null_mut(),
                };
                if mapped_ptr.is_null() {
                    if message.handle_count > 0 {
                        reply_surface_status(
                            message.handles,
                            1,
                            SurfaceTag::AttachBufferReply,
                            GraphicsStatus::Denied,
                        );
                    }
                    let _ = rt::handle_close(message.handles[1]);
                    return Ok(());
                }
                if surface.buffers[slot].attached() {
                    let _ = rt::handle_close(surface.buffers[slot].handle);
                }
                surface.buffers[slot] = BufferBinding {
                    handle: message.handles[1],
                    width,
                    height,
                    stride_pixels,
                    mapped_ptr,
                };
                if surface.active_buffer_slot.is_none() {
                    surface.active_buffer_slot = Some(slot);
                }
                mark_surface_dirty(dirty, surface, false);
                GraphicsStatus::Ok
            };
            if status != GraphicsStatus::Ok {
                let _ = rt::handle_close(message.handles[1]);
            }
            reply_surface_status(message.handles, 1, SurfaceTag::AttachBufferReply, status);
        }
        x if x == SurfaceTag::PresentBufferRequest as u32 => {
            if message.word_count < 5 {
                return Ok(());
            }
            let slot = message.words[0] as usize;
            let status = if slot >= MAX_SURFACE_BUFFERS {
                GraphicsStatus::CapacityExceeded
            } else if !surface.buffers[slot].attached() {
                GraphicsStatus::NotFound
            } else {
                let previous_slot = surface.active_buffer_slot;
                surface.active_buffer_slot = Some(slot);
                let damage = crate::types::DamageRect {
                    x: surface.x.saturating_add(message.words[1] as i64 as i32),
                    y: surface.y.saturating_add(message.words[2] as i64 as i32),
                    width: message.words[3] as u32,
                    height: message.words[4] as u32,
                };
                if damage.width == 0 || damage.height == 0 || previous_slot != Some(slot) {
                    mark_surface_dirty(dirty, surface, false);
                } else if is_cursor_surface(surface) && !matches!(dirty, DirtyState::Full { .. }) {
                    *dirty = match *dirty {
                        DirtyState::CursorOnly(existing) => DirtyState::CursorOnly(existing.merge(damage)),
                        DirtyState::Region { damage: existing, immediate } => {
                            DirtyState::Region {
                                damage: existing.merge(damage),
                                immediate,
                            }
                        }
                        DirtyState::Clean => DirtyState::CursorOnly(damage),
                        DirtyState::Full { immediate } => DirtyState::Full { immediate },
                    };
                } else {
                    merge_region_dirty(dirty, damage, true);
                }
                GraphicsStatus::Ok
            };
            reply_surface_status(
                message.handles,
                message.handle_count,
                SurfaceTag::PresentBufferReply,
                status,
            );
        }
        x if x == SurfaceTag::ReleaseBufferRequest as u32 => {
            if message.word_count < 1 {
                return Ok(());
            }
            let slot = message.words[0] as usize;
            let status = if slot >= MAX_SURFACE_BUFFERS {
                GraphicsStatus::CapacityExceeded
            } else if !surface.buffers[slot].attached() {
                GraphicsStatus::NotFound
            } else {
                let _ = rt::handle_close(surface.buffers[slot].handle);
                surface.buffers[slot] = BufferBinding::empty();
                if surface.active_buffer_slot == Some(slot) {
                    surface.active_buffer_slot = surface
                        .buffers
                        .iter()
                        .enumerate()
                        .find(|(_, buffer)| buffer.attached())
                        .map(|(index, _)| index);
                }
                mark_surface_dirty(dirty, surface, false);
                GraphicsStatus::Ok
            };
            reply_surface_status(
                message.handles,
                message.handle_count,
                SurfaceTag::ReleaseBufferReply,
                status,
            );
        }
        x if x == SurfaceTag::CloseRequest as u32 => {
            let old_rect = visible_surface_damage(surface);
            release_surface(surface);
            if old_rect.width != 0 && old_rect.height != 0 {
                merge_region_dirty(dirty, old_rect, false);
            }
        }
        _ => {}
    }

    Ok(())
}

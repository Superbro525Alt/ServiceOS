use serviceos_userspace_runtime as rt;
use rt::{GraphicsStatus, GraphicsTag, LogEvent, LogSeverity, RawMessage, SurfaceTag};

use crate::{
    logging::emit_log,
    types::{
        BufferBinding, DirtyState, MAX_BUFFER_ROW_BYTES, MAX_LABEL_BYTES, MAX_SURFACE_LABELS,
        MAX_SURFACE_RECTS, SurfaceSlot, Surfaces, active_surface_count, find_surface,
        is_cursor_surface, release_surface, surface_bounds,
    },
};

pub(crate) fn drain_public_requests(
    public_handle: rt::Handle,
    log_handle: rt::Handle,
    output: rt::DisplayOutputInfo,
    present_count: u64,
    surfaces: &mut Surfaces,
    next_surface_id: &mut u32,
    dirty: &mut DirtyState,
) -> rt::Result<bool> {
    let mut had_work = false;
    loop {
        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public_handle, &mut request) {
            Ok(()) => {
                had_work = true;
                handle_public_request(
                    &request,
                    log_handle,
                    output,
                    present_count,
                    surfaces,
                    next_surface_id,
                    dirty,
                )?;
            }
            Err(rt::Error::QueueEmpty) => return Ok(had_work),
            Err(error) => return Err(error),
        }
    }
}

pub(crate) fn drain_surface_requests(
    log_handle: rt::Handle,
    surfaces: &mut Surfaces,
    dirty: &mut DirtyState,
) -> rt::Result<bool> {
    let mut had_work = false;
    for surface in surfaces {
        if !surface.occupied {
            continue;
        }
        loop {
            let mut message = RawMessage::empty(0);
            match rt::channel_receive_nonblocking(surface.endpoint, &mut message) {
                Ok(()) => {
                    had_work = true;
                    handle_surface_request(log_handle, surface, &message, dirty)?;
                }
                Err(rt::Error::QueueEmpty) => break,
                Err(_) => {
                    release_surface(surface);
                    *dirty = DirtyState::Full { immediate: true };
                    break;
                }
            }
        }
    }
    Ok(had_work)
}

fn handle_public_request(
    request: &RawMessage,
    log_handle: rt::Handle,
    output: rt::DisplayOutputInfo,
    present_count: u64,
    surfaces: &mut Surfaces,
    next_surface_id: &mut u32,
    dirty: &mut DirtyState,
) -> rt::Result<()> {
    match request.tag {
        x if x == GraphicsTag::OutputListRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(GraphicsTag::OutputListReply as u32);
            reply.word_count = 2;
            reply.words[0] = GraphicsStatus::Ok as u32 as u64;
            reply.words[1] = 1;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == GraphicsTag::OutputStatusRequest as u32 => {
            if request.word_count < 1 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(GraphicsTag::OutputStatusReply as u32);
            reply.word_count = 12;
            if request.words[0] != 0 {
                reply.words[0] = GraphicsStatus::NotFound as u32 as u64;
            } else {
                reply.words[0] = GraphicsStatus::Ok as u32 as u64;
                reply.words[1] = 0;
                reply.words[2] = output.backend as u64;
                reply.words[3] = output.state as u64;
                reply.words[4] = output.pixel_format as u64;
                reply.words[5] = output.width as u64;
                reply.words[6] = output.height as u64;
                reply.words[7] = output.stride as u64;
                reply.words[8] = output.bytes_per_pixel as u64;
                reply.words[9] = output.byte_len;
                reply.words[10] = present_count;
                reply.words[11] = active_surface_count(surfaces) as u64;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == GraphicsTag::SurfaceListRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(GraphicsTag::SurfaceListReply as u32);
            reply.words[0] = GraphicsStatus::Ok as u32 as u64;
            let mut count = 0usize;
            for surface in surfaces.iter().filter(|surface| surface.occupied) {
                if 2 + count >= rt::IPC_MAX_WORDS {
                    break;
                }
                reply.words[2 + count] = surface.id as u64;
                count += 1;
            }
            reply.word_count = (2 + count) as u32;
            reply.words[1] = count as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == GraphicsTag::SurfaceStatusRequest as u32 => {
            if request.word_count < 1 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(GraphicsTag::SurfaceStatusReply as u32);
            reply.word_count = 11;
            if let Some(surface) = find_surface(surfaces, request.words[0] as u32) {
                reply.words[0] = GraphicsStatus::Ok as u32 as u64;
                reply.words[1] = surface.id as u64;
                reply.words[2] = 0;
                reply.words[3] = surface.owner_session as u64;
                reply.words[4] = surface.x as i64 as u64;
                reply.words[5] = surface.y as i64 as u64;
                reply.words[6] = surface.width as u64;
                reply.words[7] = surface.height as u64;
                reply.words[8] = surface.z_order as u64;
                reply.words[9] = surface.fill_rgb as u64;
                reply.words[10] = u64::from(surface.visible);
            } else {
                reply.words[0] = GraphicsStatus::NotFound as u32 as u64;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == GraphicsTag::SurfaceCreateRequest as u32 => {
            if request.word_count < 8 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let Some(slot) = surfaces.iter_mut().find(|surface| !surface.occupied) else {
                let mut reply = RawMessage::empty(GraphicsTag::SurfaceCreateReply as u32);
                reply.word_count = 1;
                reply.words[0] = GraphicsStatus::CapacityExceeded as u32 as u64;
                let _ = rt::channel_send(reply_handle, &reply);
                let _ = rt::handle_close(reply_handle);
                return Ok(());
            };

            let pair = rt::channel_create()?;
            slot.id = *next_surface_id;
            *next_surface_id = next_surface_id.saturating_add(1);
            slot.owner_session = request.words[0] as u32;
            slot.endpoint = pair.first;
            slot.x = request.words[1] as i64 as i32;
            slot.y = request.words[2] as i64 as i32;
            slot.width = request.words[3] as u32;
            slot.height = request.words[4] as u32;
            slot.z_order = request.words[5] as u32;
            slot.fill_rgb = request.words[6] as u32;
            slot.visible = request.words[7] != 0;
            slot.occupied = true;
            slot.rects = [crate::types::RectSlot::empty(); MAX_SURFACE_RECTS];
            slot.labels = [crate::types::LabelSlot::empty(); MAX_SURFACE_LABELS];
            *dirty = DirtyState::Full { immediate: true };

            let mut reply = RawMessage::empty(GraphicsTag::SurfaceCreateReply as u32);
            reply.word_count = 2;
            reply.words[0] = GraphicsStatus::Ok as u32 as u64;
            reply.words[1] = slot.id as u64;
            reply.handle_count = 1;
            reply.handles[0] = pair.second;
            reply.handle_rights[0] =
                rt::rights::SEND | rt::rights::RECEIVE | rt::rights::DUPLICATE | rt::rights::TRANSFER;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
            let _ = rt::handle_close(pair.second);
            let _ = emit_log(
                log_handle,
                LogSeverity::Info,
                LogEvent::SurfaceCreated,
                slot.id as u64,
                slot.owner_session as u64,
            );
        }
        _ => {}
    }

    Ok(())
}

fn handle_surface_request(
    log_handle: rt::Handle,
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
            if is_cursor_surface(surface) && !matches!(dirty, DirtyState::Full { .. }) {
                let damage = old_rect.merge(new_rect);
                *dirty = match *dirty {
                    DirtyState::CursorOnly(existing) => DirtyState::CursorOnly(existing.merge(damage)),
                    _ => DirtyState::CursorOnly(damage),
                };
            } else {
                *dirty = DirtyState::Full { immediate: true };
            }
            let _ = emit_log(
                log_handle,
                LogSeverity::Debug,
                LogEvent::SurfaceUpdated,
                surface.id as u64,
                0,
            );
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
            *dirty = DirtyState::Full { immediate: false };
            let _ = emit_log(
                log_handle,
                LogSeverity::Debug,
                LogEvent::SurfaceUpdated,
                surface.id as u64,
                1,
            );
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
            surface.visible = message.words[0] != 0;
            *dirty = DirtyState::Full { immediate: true };
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
            *dirty = DirtyState::Full { immediate: false };
            let _ = emit_log(
                log_handle,
                LogSeverity::Debug,
                LogEvent::SurfaceUpdated,
                surface.id as u64,
                2,
            );
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
                *dirty = DirtyState::Full { immediate: false };
                let _ = emit_log(
                    log_handle,
                    LogSeverity::Debug,
                    LogEvent::SurfaceUpdated,
                    surface.id as u64,
                    slot_index as u64,
                );
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
                    *dirty = DirtyState::Full { immediate: false };
                    let _ = emit_log(
                        log_handle,
                        LogSeverity::Debug,
                        LogEvent::SurfaceUpdated,
                        surface.id as u64,
                        (MAX_SURFACE_RECTS + slot_index) as u64,
                    );
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
            if message.word_count < 3 || message.handle_count < 2 {
                return Ok(());
            }
            let width = message.words[0] as u32;
            let height = message.words[1] as u32;
            let stride_pixels = message.words[2] as u32;
            let status = if width == 0
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
                if surface.buffer.attached() {
                    let _ = rt::handle_close(surface.buffer.handle);
                }
                surface.buffer = BufferBinding {
                    handle: message.handles[1],
                    width,
                    height,
                    stride_pixels,
                    mapped_ptr,
                };
                *dirty = DirtyState::Full { immediate: true };
                GraphicsStatus::Ok
            };
            if status != GraphicsStatus::Ok {
                let _ = rt::handle_close(message.handles[1]);
            }
            reply_surface_status(message.handles, 1, SurfaceTag::AttachBufferReply, status);
        }
        x if x == SurfaceTag::PresentBufferRequest as u32 => {
            if message.word_count < 4 {
                return Ok(());
            }
            if surface.buffer.attached() {
                *dirty = DirtyState::Full { immediate: true };
                let _ = emit_log(
                    log_handle,
                    LogSeverity::Debug,
                    LogEvent::SurfaceUpdated,
                    surface.id as u64,
                    0xff,
                );
            }
            reply_surface_status(
                message.handles,
                message.handle_count,
                SurfaceTag::PresentBufferReply,
                GraphicsStatus::Ok,
            );
        }
        x if x == SurfaceTag::CloseRequest as u32 => {
            release_surface(surface);
            *dirty = DirtyState::Full { immediate: true };
        }
        _ => {}
    }

    Ok(())
}

fn reply_surface_status(
    handles: [rt::Handle; rt::IPC_MAX_HANDLES],
    handle_count: u32,
    tag: SurfaceTag,
    status: GraphicsStatus,
) {
    if handle_count == 0 {
        return;
    }
    let handle = handles[0];
    let mut reply = RawMessage::empty(tag as u32);
    reply.word_count = 1;
    reply.words[0] = status as u32 as u64;
    let _ = rt::channel_send(handle, &reply);
    let _ = rt::handle_close(handle);
}

fn unpack_bytes(words: &[u64], len: usize, destination: &mut [u8]) -> rt::Result<()> {
    if len > destination.len() || len > words.len() * 8 {
        return Err(rt::Error::BufferTooSmall);
    }
    let mut copied = 0usize;
    for word in words.iter().copied() {
        if copied >= len {
            break;
        }
        let bytes = word.to_le_bytes();
        let chunk = (len - copied).min(bytes.len());
        destination[copied..copied + chunk].copy_from_slice(&bytes[..chunk]);
        copied += chunk;
    }
    Ok(())
}

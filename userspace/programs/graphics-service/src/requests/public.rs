use serviceos_userspace_runtime as rt;
use rt::{GraphicsStatus, GraphicsTag, LogEvent, LogSeverity, RawMessage};

use crate::{
    logging::emit_log,
    types::{
        DirtyState, MAX_PUBLIC_REQUESTS_PER_TURN, MAX_SURFACE_LABELS, MAX_SURFACE_RECTS,
        Surfaces, active_buffer, active_surface_count, attached_buffer_count, find_surface,
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
    let mut processed = 0usize;
    loop {
        if processed >= MAX_PUBLIC_REQUESTS_PER_TURN {
            return Ok(had_work);
        }
        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public_handle, &mut request) {
            Ok(()) => {
                had_work = true;
                processed += 1;
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
            reply.word_count = 16;
            if let Some(surface) = find_surface(surfaces, request.words[0] as u32) {
                let active = active_buffer(surface);
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
                reply.words[11] = attached_buffer_count(surface) as u64;
                reply.words[12] = surface
                    .active_buffer_slot
                    .map(|slot| slot as u64)
                    .unwrap_or(u32::MAX as u64);
                reply.words[13] = active.map(|buffer| buffer.width as u64).unwrap_or(0);
                reply.words[14] = active.map(|buffer| buffer.height as u64).unwrap_or(0);
                reply.words[15] = active
                    .map(|buffer| buffer.stride_pixels as u64)
                    .unwrap_or(0);
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

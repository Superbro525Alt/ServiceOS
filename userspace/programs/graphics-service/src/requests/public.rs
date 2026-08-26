use rt::{GraphicsStatus, GraphicsTag, LogEvent, LogSeverity, RawMessage};
use serviceos_userspace_runtime as rt;

use crate::{
    fence::{
        FENCE_WAIT_REPLY_TAG, FENCE_WAIT_REQUEST_TAG, FenceWaiter, FenceWaiters, MAX_FENCE_WAITERS,
        ReapedWait, WaitDecision, decide_fence_wait,
    },
    logging::emit_log,
    outputs::{
        ExtendSide, MAX_OUTPUTS, OUTPUT_CREATE_REPLY_TAG, OUTPUT_CREATE_REQUEST_TAG,
        OUTPUT_EXTEND_REPLY_TAG, OUTPUT_EXTEND_REQUEST_TAG, OutputCreateError, OutputRegistry,
    },
    types::{
        DirtyState, MAX_PUBLIC_REQUESTS_PER_TURN, MAX_SURFACE_LABELS, MAX_SURFACE_RECTS, Surfaces,
        active_buffer, active_surface_count, attached_buffer_count, close_pending_count,
        find_surface, surface_bounds,
    },
};

pub(crate) fn drain_public_requests(
    public_handle: rt::Handle,
    log_handle: rt::Handle,
    registry: &mut OutputRegistry,
    fence_completed: u64,
    waiters: &mut FenceWaiters,
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
                    registry,
                    fence_completed,
                    waiters,
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

/// Answer parked fence waiters that completed or expired. Runs after each
/// compositor present (completion side) and once per main-loop turn (timeout
/// side), so clients block on the reply instead of spinning.
pub(crate) fn release_fence_waiters(waiters: &mut FenceWaiters, completed: u64) -> usize {
    if waiters.is_empty() {
        return 0;
    }
    let now = rt::monotonic_now().unwrap_or(0);
    let mut out = [ReapedWait::TimedOut(rt::INVALID_HANDLE); MAX_FENCE_WAITERS];
    let reaped = waiters.reap(completed, now, &mut out);
    for wait in out[..reaped].iter().copied() {
        reply_fence_wait(
            wait.handle(),
            if wait.completed() {
                GraphicsStatus::Ok
            } else {
                GraphicsStatus::Busy
            },
            completed,
        );
    }
    reaped
}

fn reply_fence_wait(reply_handle: rt::Handle, status: GraphicsStatus, completed: u64) {
    let mut reply = RawMessage::empty(FENCE_WAIT_REPLY_TAG as u32);
    reply.word_count = 2;
    reply.words[0] = status as u32 as u64;
    reply.words[1] = completed;
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
}

pub(crate) fn handle_public_request(
    request: &RawMessage,
    log_handle: rt::Handle,
    registry: &mut OutputRegistry,
    fence_completed: u64,
    waiters: &mut FenceWaiters,
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
            let mut ids = [0u32; MAX_OUTPUTS];
            let count = registry.enumerate_ids(&mut ids);
            let mut reply = RawMessage::empty(GraphicsTag::OutputListReply as u32);
            reply.word_count = (2 + count) as u32;
            reply.words[0] = GraphicsStatus::Ok as u32 as u64;
            reply.words[1] = count as u64;
            for index in 0..count {
                reply.words[2 + index] = ids[index] as u64;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == OUTPUT_CREATE_REQUEST_TAG as u32 => {
            if request.word_count < 3 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let width = request.words[1] as u32;
            let height = request.words[2] as u32;
            let mut reply = RawMessage::empty(OUTPUT_CREATE_REPLY_TAG as u32);
            reply.word_count = 2;
            match registry.primary().map(|slot| slot.info) {
                Some(template) => match registry.create_virtual_mirror(&template, width, height) {
                    Ok(id) => {
                        reply.words[0] = GraphicsStatus::Ok as u32 as u64;
                        reply.words[1] = id as u64;
                        *dirty = DirtyState::Full { immediate: true };
                        let _ = rt::write_logf(
                            "graphics",
                            format_args!(
                                "multi-output: virtual mirror output id={} created at {}x{} \
                                 (memory-backed mirror of primary)",
                                id, width, height
                            ),
                        );
                    }
                    Err(OutputCreateError::CapacityExceeded) => {
                        reply.word_count = 1;
                        reply.words[0] = GraphicsStatus::CapacityExceeded as u32 as u64;
                    }
                    Err(
                        OutputCreateError::GeometryUnsupported
                        | OutputCreateError::NotFound
                        | OutputCreateError::ModeUnsupported,
                    ) => {
                        reply.word_count = 1;
                        reply.words[0] = GraphicsStatus::Denied as u32 as u64;
                    }
                },
                None => {
                    reply.word_count = 1;
                    reply.words[0] = GraphicsStatus::NotFound as u32 as u64;
                }
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == OUTPUT_EXTEND_REQUEST_TAG as u32 => {
            if request.word_count < 3 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let output_id = request.words[1] as u32;
            let mut reply = RawMessage::empty(OUTPUT_EXTEND_REPLY_TAG as u32);
            reply.word_count = 7;
            let side = match ExtendSide::from_word(request.words[2]) {
                Some(side) => side,
                None => {
                    reply.word_count = 1;
                    reply.words[0] = GraphicsStatus::Denied as u32 as u64;
                    let _ = rt::channel_send(reply_handle, &reply);
                    let _ = rt::handle_close(reply_handle);
                    return Ok(());
                }
            };
            match registry.configure_extend(output_id, side) {
                Ok((origin_x, origin_y)) => {
                    if let Some(bounds) = registry.desktop_bounds() {
                        reply.words[3] = bounds.x as i64 as u64;
                        reply.words[4] = bounds.y as i64 as u64;
                        reply.words[5] = bounds.width as u64;
                        reply.words[6] = bounds.height as u64;
                    }
                    reply.words[0] = GraphicsStatus::Ok as u32 as u64;
                    reply.words[1] = origin_x as i64 as u64;
                    reply.words[2] = origin_y as i64 as u64;
                    *dirty = DirtyState::Full { immediate: true };
                    let _ = rt::write_logf(
                        "graphics",
                        format_args!(
                            "multi-output: output id={} EXTEND {} primary at ({},{}) desktop {}x{}+({},{})",
                            output_id,
                            if side == ExtendSide::RightOfPrimary {
                                "right-of"
                            } else {
                                "left-of"
                            },
                            origin_x,
                            origin_y,
                            reply.words[5],
                            reply.words[6],
                            reply.words[3] as i64,
                            reply.words[4] as i64
                        ),
                    );
                }
                Err(OutputCreateError::NotFound | OutputCreateError::ModeUnsupported) => {
                    reply.word_count = 1;
                    reply.words[0] = GraphicsStatus::Denied as u32 as u64;
                }
                Err(_) => {
                    reply.word_count = 1;
                    reply.words[0] = GraphicsStatus::CapacityExceeded as u32 as u64;
                }
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == GraphicsTag::OutputStatusRequest as u32 => {
            if request.word_count < 1 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(GraphicsTag::OutputStatusReply as u32);
            reply.word_count = 16;
            match registry.by_index(request.words[0] as usize) {
                Some(slot) => {
                    let info = slot.info;
                    reply.words[0] = GraphicsStatus::Ok as u32 as u64;
                    // Word 1 stays the enumeration index (legacy clients see
                    // 0 for the primary); stable ids ride OutputListReply.
                    reply.words[1] = request.words[0];
                    reply.words[2] = info.backend as u64;
                    reply.words[3] = info.state as u64;
                    reply.words[4] = info.pixel_format as u64;
                    reply.words[5] = info.width as u64;
                    reply.words[6] = info.height as u64;
                    reply.words[7] = info.stride as u64;
                    reply.words[8] = info.bytes_per_pixel as u64;
                    reply.words[9] = info.byte_len;
                    reply.words[10] = slot.present_count;
                    reply.words[11] = active_surface_count(surfaces) as u64;
                    reply.words[12] = fence_completed;
                    reply.words[13] = slot.noop_skips;
                    reply.words[14] = slot.noop_saved_bytes;
                    reply.words[15] = close_pending_count(surfaces) as u64;
                }
                None => {
                    reply.words[0] = GraphicsStatus::NotFound as u32 as u64;
                }
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
            if slot.visible {
                super::common::merge_region_dirty(dirty, surface_bounds(slot), false);
            }

            let mut reply = RawMessage::empty(GraphicsTag::SurfaceCreateReply as u32);
            reply.word_count = 2;
            reply.words[0] = GraphicsStatus::Ok as u32 as u64;
            reply.words[1] = slot.id as u64;
            reply.handle_count = 1;
            reply.handles[0] = pair.second;
            reply.handle_rights[0] = rt::rights::SEND
                | rt::rights::RECEIVE
                | rt::rights::DUPLICATE
                | rt::rights::TRANSFER;
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
        x if x == FENCE_WAIT_REQUEST_TAG as u32 => {
            if request.word_count < 2 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let token = request.words[0];
            let timeout_ticks = request.words[1];
            let now = rt::monotonic_now().unwrap_or(0);
            match decide_fence_wait(fence_completed, token, now, timeout_ticks) {
                WaitDecision::AlreadyComplete => {
                    reply_fence_wait(reply_handle, GraphicsStatus::Ok, fence_completed);
                }
                WaitDecision::ImmediateTimeout => {
                    reply_fence_wait(reply_handle, GraphicsStatus::Busy, fence_completed);
                }
                WaitDecision::Park { deadline_tick } => {
                    let waiter = FenceWaiter {
                        reply_handle,
                        token,
                        deadline_tick,
                    };
                    if waiters.park(waiter).is_err() {
                        reply_fence_wait(
                            waiter.reply_handle,
                            GraphicsStatus::CapacityExceeded,
                            fence_completed,
                        );
                    }
                }
            }
        }
        _ => {}
    }

    Ok(())
}

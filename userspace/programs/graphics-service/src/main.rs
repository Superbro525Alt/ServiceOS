#![no_std]
#![no_main]

use core::ptr;

use serviceos_userspace_runtime as rt;
use rt::{
    ControlTag, DisplayPixelFormat, GraphicsStatus, GraphicsTag, LifecycleEvent, LogDomain,
    LogEvent, LogSeverity, RawMessage, ServiceId, SurfaceTag,
};

const MAX_SURFACES: usize = 16;
const MAX_FRAMEBUFFER_BYTES: usize = 8 * 1024 * 1024;
const MAX_SURFACE_RECTS: usize = 16;
const MAX_SURFACE_LABELS: usize = 16;
const MAX_LABEL_BYTES: usize = 56;
const DEFAULT_BACKGROUND_RGB: u32 = 0x10151d;
const LABEL_ADVANCE: usize = 6;
const GLYPH_WIDTH: usize = 5;
const GLYPH_HEIGHT: usize = 7;

static mut FRAMEBUFFER_BYTES: [u8; MAX_FRAMEBUFFER_BYTES] = [0; MAX_FRAMEBUFFER_BYTES];

#[derive(Clone, Copy)]
struct RectSlot {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    color_rgb: u32,
    visible: bool,
    occupied: bool,
}

impl RectSlot {
    const fn empty() -> Self {
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
struct LabelSlot {
    x: i32,
    y: i32,
    color_rgb: u32,
    len: usize,
    bytes: [u8; MAX_LABEL_BYTES],
    occupied: bool,
}

impl LabelSlot {
    const fn empty() -> Self {
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
struct SurfaceSlot {
    id: u32,
    owner_session: u32,
    endpoint: rt::Handle,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    z_order: u32,
    fill_rgb: u32,
    visible: bool,
    occupied: bool,
    rects: [RectSlot; MAX_SURFACE_RECTS],
    labels: [LabelSlot; MAX_SURFACE_LABELS],
}

impl SurfaceSlot {
    const fn empty() -> Self {
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
            rects: [RectSlot::empty(); MAX_SURFACE_RECTS],
            labels: [LabelSlot::empty(); MAX_SURFACE_LABELS],
        }
    }
}

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xfc01;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 2 {
        return 0xfc02;
    }

    let output_handle = startup.handles[0];
    let log_handle = startup.handles[1];
    let output = match rt::display_output_info(output_handle) {
        Ok(info) => info,
        Err(_) => return 0xfc03,
    };
    if output.bytes_per_pixel != 4 || output.byte_len as usize > MAX_FRAMEBUFFER_BYTES {
        return 0xfc04;
    }

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xfc05,
    };
    if rt::register_service(bootstrap, ServiceId::Graphics, public.second).is_err() {
        return 0xfc06;
    }
    let _ = rt::handle_close(public.second);

    let _ = emit_log(
        log_handle,
        LogSeverity::Info,
        LogEvent::DisplayOutputReady,
        output.width as u64,
        output.height as u64,
    );

    let mut surfaces = [SurfaceSlot::empty(); MAX_SURFACES];
    let mut next_surface_id = 1u32;
    let mut present_count = 0u64;
    let mut last_logged_surface_count = 0usize;
    let mut dirty = true;

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xfc07,
        }

        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                if handle_public_request(
                    &request,
                    log_handle,
                    output,
                    present_count,
                    &mut surfaces,
                    &mut next_surface_id,
                    &mut dirty,
                )
                .is_err()
                {
                    return 0xfc08;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xfc09,
        }

        for surface in &mut surfaces {
            if !surface.occupied {
                continue;
            }
            let mut message = RawMessage::empty(0);
            match rt::channel_receive_nonblocking(surface.endpoint, &mut message) {
                Ok(()) => {
                    if handle_surface_request(log_handle, surface, &message, &mut dirty).is_err() {
                        return 0xfc0a;
                    }
                }
                Err(rt::Error::QueueEmpty) => {}
                Err(_) => {
                    release_surface(surface);
                    dirty = true;
                }
            }
        }

        if dirty {
            if compose_and_present(output_handle, output, &surfaces).is_err() {
                return 0xfc0b;
            }
            present_count = present_count.saturating_add(1);
            let active_surface_count = active_surface_count(&surfaces);
            if present_count == 1 || active_surface_count != last_logged_surface_count {
                let _ = emit_log(
                    log_handle,
                    LogSeverity::Info,
                    LogEvent::CompositorPresented,
                    active_surface_count as u64,
                    present_count,
                );
                last_logged_surface_count = active_surface_count;
            }
            dirty = false;
        }

        if rt::yield_current().is_err() {
            return 0xfc0c;
        }
    }
}

fn handle_public_request(
    request: &RawMessage,
    log_handle: rt::Handle,
    output: rt::DisplayOutputInfo,
    present_count: u64,
    surfaces: &mut [SurfaceSlot; MAX_SURFACES],
    next_surface_id: &mut u32,
    dirty: &mut bool,
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
            slot.rects = [RectSlot::empty(); MAX_SURFACE_RECTS];
            slot.labels = [LabelSlot::empty(); MAX_SURFACE_LABELS];
            *dirty = true;

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
    dirty: &mut bool,
) -> rt::Result<()> {
    match message.tag {
        x if x == SurfaceTag::SetGeometryRequest as u32 => {
            if message.word_count < 5 || message.handle_count < 1 {
                return Ok(());
            }
            surface.x = message.words[0] as i64 as i32;
            surface.y = message.words[1] as i64 as i32;
            surface.width = message.words[2] as u32;
            surface.height = message.words[3] as u32;
            surface.z_order = message.words[4] as u32;
            *dirty = true;
            let _ = emit_log(
                log_handle,
                LogSeverity::Debug,
                LogEvent::SurfaceUpdated,
                surface.id as u64,
                0,
            );
            reply_surface_status(message.handles[0], SurfaceTag::SetGeometryReply, GraphicsStatus::Ok);
        }
        x if x == SurfaceTag::SetFillRequest as u32 => {
            if message.word_count < 1 || message.handle_count < 1 {
                return Ok(());
            }
            surface.fill_rgb = message.words[0] as u32;
            *dirty = true;
            let _ = emit_log(
                log_handle,
                LogSeverity::Debug,
                LogEvent::SurfaceUpdated,
                surface.id as u64,
                1,
            );
            reply_surface_status(message.handles[0], SurfaceTag::SetFillReply, GraphicsStatus::Ok);
        }
        x if x == SurfaceTag::SetVisibilityRequest as u32 => {
            if message.word_count < 1 || message.handle_count < 1 {
                return Ok(());
            }
            surface.visible = message.words[0] != 0;
            *dirty = true;
            reply_surface_status(
                message.handles[0],
                SurfaceTag::SetVisibilityReply,
                GraphicsStatus::Ok,
            );
        }
        x if x == SurfaceTag::ClearSceneRequest as u32 => {
            if message.handle_count < 1 {
                return Ok(());
            }
            surface.rects = [RectSlot::empty(); MAX_SURFACE_RECTS];
            surface.labels = [LabelSlot::empty(); MAX_SURFACE_LABELS];
            *dirty = true;
            let _ = emit_log(
                log_handle,
                LogSeverity::Debug,
                LogEvent::SurfaceUpdated,
                surface.id as u64,
                2,
            );
            reply_surface_status(message.handles[0], SurfaceTag::ClearSceneReply, GraphicsStatus::Ok);
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
                *dirty = true;
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
            reply_surface_status(message.handles[0], SurfaceTag::SetRectReply, status);
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
                    *dirty = true;
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
            reply_surface_status(message.handles[0], SurfaceTag::SetLabelReply, status);
        }
        x if x == SurfaceTag::CloseRequest as u32 => {
            release_surface(surface);
            *dirty = true;
        }
        _ => {}
    }

    Ok(())
}

fn reply_surface_status(handle: rt::Handle, tag: SurfaceTag, status: GraphicsStatus) {
    let mut reply = RawMessage::empty(tag as u32);
    reply.word_count = 1;
    reply.words[0] = status as u32 as u64;
    let _ = rt::channel_send(handle, &reply);
    let _ = rt::handle_close(handle);
}

fn compose_and_present(
    output_handle: rt::Handle,
    output: rt::DisplayOutputInfo,
    surfaces: &[SurfaceSlot; MAX_SURFACES],
) -> rt::Result<()> {
    let byte_len = output.byte_len as usize;
    let frame = framebuffer_slice(byte_len);
    fill_frame(frame, output, DEFAULT_BACKGROUND_RGB);

    let mut order = [0usize; MAX_SURFACES];
    let mut count = 0usize;
    for (index, surface) in surfaces.iter().enumerate() {
        if surface.occupied && surface.visible {
            order[count] = index;
            count += 1;
        }
    }
    for idx in 1..count {
        let key = order[idx];
        let key_z = surfaces[key].z_order;
        let mut cursor = idx;
        while cursor > 0 && surfaces[order[cursor - 1]].z_order > key_z {
            order[cursor] = order[cursor - 1];
            cursor -= 1;
        }
        order[cursor] = key;
    }

    for index in order[..count].iter().copied() {
        draw_surface(frame, output, &surfaces[index]);
    }

    let _ = rt::display_output_present(output_handle, &frame[..byte_len])?;
    Ok(())
}

fn fill_frame(frame: &mut [u8], output: rt::DisplayOutputInfo, rgb: u32) {
    for y in 0..output.height as usize {
        for x in 0..output.width as usize {
            write_pixel(frame, output, x, y, rgb);
        }
    }
}

fn draw_surface(frame: &mut [u8], output: rt::DisplayOutputInfo, surface: &SurfaceSlot) {
    draw_rect(
        frame,
        output,
        surface.x,
        surface.y,
        surface.width,
        surface.height,
        surface.fill_rgb,
    );
    for rect in surface.rects.iter().filter(|rect| rect.occupied && rect.visible) {
        draw_rect(
            frame,
            output,
            surface.x.saturating_add(rect.x),
            surface.y.saturating_add(rect.y),
            rect.width,
            rect.height,
            rect.color_rgb,
        );
    }
    for label in surface.labels.iter().filter(|label| label.occupied) {
        draw_label(
            frame,
            output,
            surface.x.saturating_add(label.x),
            surface.y.saturating_add(label.y),
            label.color_rgb,
            &label.bytes[..label.len],
        );
    }
}

fn draw_rect(
    frame: &mut [u8],
    output: rt::DisplayOutputInfo,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    rgb: u32,
) {
    let start_x = x.max(0) as usize;
    let start_y = y.max(0) as usize;
    let end_x = ((x + width as i32).max(0) as usize).min(output.width as usize);
    let end_y = ((y + height as i32).max(0) as usize).min(output.height as usize);
    if start_x >= end_x || start_y >= end_y {
        return;
    }

    for py in start_y..end_y {
        for px in start_x..end_x {
            write_pixel(frame, output, px, py, rgb);
        }
    }
}

fn draw_label(
    frame: &mut [u8],
    output: rt::DisplayOutputInfo,
    x: i32,
    y: i32,
    color_rgb: u32,
    text: &[u8],
) {
    for (index, byte) in text.iter().copied().enumerate() {
        let ch = normalize_glyph(byte);
        let glyph_x = x.saturating_add((index * LABEL_ADVANCE) as i32);
        draw_glyph(frame, output, glyph_x, y, color_rgb, ch);
    }
}

fn draw_glyph(
    frame: &mut [u8],
    output: rt::DisplayOutputInfo,
    x: i32,
    y: i32,
    color_rgb: u32,
    ch: u8,
) {
    let rows = glyph_rows(ch);
    for (row_index, bits) in rows.iter().copied().enumerate().take(GLYPH_HEIGHT) {
        for column in 0..GLYPH_WIDTH {
            if (bits >> (GLYPH_WIDTH - 1 - column)) & 1 == 0 {
                continue;
            }
            let px = x.saturating_add(column as i32);
            let py = y.saturating_add(row_index as i32);
            if px < 0 || py < 0 {
                continue;
            }
            write_pixel(frame, output, px as usize, py as usize, color_rgb);
        }
    }
}

fn normalize_glyph(byte: u8) -> u8 {
    if byte.is_ascii_lowercase() {
        byte - 32
    } else {
        byte
    }
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

fn glyph_rows(ch: u8) -> [u8; GLYPH_HEIGHT] {
    match ch {
        b'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        b'B' => [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e],
        b'C' => [0x0f, 0x10, 0x10, 0x10, 0x10, 0x10, 0x0f],
        b'D' => [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e],
        b'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f],
        b'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10],
        b'G' => [0x0f, 0x10, 0x10, 0x17, 0x11, 0x11, 0x0f],
        b'H' => [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        b'I' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x1f],
        b'J' => [0x01, 0x01, 0x01, 0x01, 0x11, 0x11, 0x0e],
        b'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        b'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f],
        b'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11],
        b'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        b'O' => [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        b'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10],
        b'Q' => [0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d],
        b'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11],
        b'S' => [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e],
        b'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        b'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        b'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04],
        b'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1b, 0x11],
        b'X' => [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11],
        b'Y' => [0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04],
        b'Z' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f],
        b'0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
        b'1' => [0x04, 0x0c, 0x14, 0x04, 0x04, 0x04, 0x1f],
        b'2' => [0x0e, 0x11, 0x01, 0x06, 0x08, 0x10, 0x1f],
        b'3' => [0x1f, 0x02, 0x04, 0x06, 0x01, 0x11, 0x0e],
        b'4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
        b'5' => [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e],
        b'6' => [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e],
        b'7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        b'8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
        b'9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c],
        b'.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x0c],
        b':' => [0x00, 0x0c, 0x0c, 0x00, 0x0c, 0x0c, 0x00],
        b'-' => [0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00],
        b'/' => [0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10],
        b'_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1f],
        b' ' => [0x00; GLYPH_HEIGHT],
        _ => [0x0e, 0x11, 0x01, 0x06, 0x04, 0x00, 0x04],
    }
}

fn write_pixel(frame: &mut [u8], output: rt::DisplayOutputInfo, x: usize, y: usize, rgb: u32) {
    let offset = (y * output.stride as usize + x) * output.bytes_per_pixel as usize;
    if offset + 3 >= frame.len() {
        return;
    }

    let red = ((rgb >> 16) & 0xff) as u8;
    let green = ((rgb >> 8) & 0xff) as u8;
    let blue = (rgb & 0xff) as u8;
    match output.pixel_format {
        x if x == DisplayPixelFormat::Xrgb8888 as u32 => {
            frame[offset] = red;
            frame[offset + 1] = green;
            frame[offset + 2] = blue;
            frame[offset + 3] = 0;
        }
        _ => {
            frame[offset] = blue;
            frame[offset + 1] = green;
            frame[offset + 2] = red;
            frame[offset + 3] = 0;
        }
    }
}

fn framebuffer_slice(len: usize) -> &'static mut [u8] {
    unsafe {
        core::slice::from_raw_parts_mut(ptr::addr_of_mut!(FRAMEBUFFER_BYTES).cast::<u8>(), len)
    }
}

fn active_surface_count(surfaces: &[SurfaceSlot; MAX_SURFACES]) -> usize {
    surfaces.iter().filter(|surface| surface.occupied).count()
}

fn find_surface(
    surfaces: &[SurfaceSlot; MAX_SURFACES],
    surface_id: u32,
) -> Option<&SurfaceSlot> {
    surfaces
        .iter()
        .find(|surface| surface.occupied && surface.id == surface_id)
}

fn release_surface(surface: &mut SurfaceSlot) {
    if surface.endpoint != rt::INVALID_HANDLE {
        let _ = rt::handle_close(surface.endpoint);
    }
    *surface = SurfaceSlot::empty();
}

fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
    let mut message = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut message) {
        Ok(()) if message.tag == ControlTag::Lifecycle as u32 && message.word_count > 0 => Ok(
            matches!(
                lifecycle_event_from_word(message.words[0]),
                LifecycleEvent::Restarting | LifecycleEvent::Stopped
            ),
        ),
        Ok(()) => Ok(false),
        Err(rt::Error::QueueEmpty) => Ok(false),
        Err(error) => Err(error),
    }
}

fn lifecycle_event_from_word(value: u64) -> LifecycleEvent {
    match value as u32 {
        x if x == LifecycleEvent::Starting as u32 => LifecycleEvent::Starting,
        x if x == LifecycleEvent::Ready as u32 => LifecycleEvent::Ready,
        x if x == LifecycleEvent::Failed as u32 => LifecycleEvent::Failed,
        x if x == LifecycleEvent::Stopped as u32 => LifecycleEvent::Stopped,
        _ => LifecycleEvent::Restarting,
    }
}

fn emit_log(
    log_handle: rt::Handle,
    severity: LogSeverity,
    event: LogEvent,
    arg0: u64,
    arg1: u64,
) -> rt::Result<()> {
    rt::send_log_record(
        log_handle,
        ServiceId::Graphics,
        severity,
        LogDomain::Graphics,
        event,
        arg0,
        arg1,
    )
}

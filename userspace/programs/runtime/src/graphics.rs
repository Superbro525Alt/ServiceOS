use crate::{
    channel_create, channel_receive_blocking, channel_send, display_backend_from_word,
    display_pixel_format_from_word, display_state_from_word, graphics_status_error,
    graphics_status_from_word, handle_close, pack_bytes, rights, Error, GraphicsOutputStatusInfo,
    GraphicsStatus, GraphicsSurfaceStatusInfo, GraphicsTag, Handle, RawMessage, Result,
    SurfaceTag, IPC_MAX_WORDS,
};

pub fn graphics_output_count(graphics_handle: Handle) -> Result<usize> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(GraphicsTag::OutputListRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(graphics_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != GraphicsTag::OutputListReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }

    match graphics_status_from_word(response.words[0]) {
        GraphicsStatus::Ok => Ok(response.words[1] as usize),
        status => Err(graphics_status_error(status)),
    }
}

pub fn graphics_output_status(
    graphics_handle: Handle,
    index: usize,
) -> Result<Option<GraphicsOutputStatusInfo>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(GraphicsTag::OutputStatusRequest as u32);
    request.word_count = 1;
    request.words[0] = index as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(graphics_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != GraphicsTag::OutputStatusReply as u32 || response.word_count < 12 {
        return Err(Error::InvalidArgument);
    }

    let status = graphics_status_from_word(response.words[0]);
    if status == GraphicsStatus::NotFound {
        return Ok(None);
    }
    if status != GraphicsStatus::Ok {
        return Err(graphics_status_error(status));
    }

    Ok(Some(GraphicsOutputStatusInfo {
        index: response.words[1] as u32,
        backend: display_backend_from_word(response.words[2]),
        state: display_state_from_word(response.words[3]),
        pixel_format: display_pixel_format_from_word(response.words[4]),
        width: response.words[5] as u32,
        height: response.words[6] as u32,
        stride: response.words[7] as u32,
        bytes_per_pixel: response.words[8] as u32,
        byte_len: response.words[9],
        present_count: response.words[10],
        surface_count: response.words[11] as u32,
    }))
}

pub fn graphics_surface_create(
    graphics_handle: Handle,
    owner_session: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    z_order: u32,
    fill_rgb: u32,
    visible: bool,
) -> Result<(u32, Handle)> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(GraphicsTag::SurfaceCreateRequest as u32);
    request.word_count = 8;
    request.words[0] = owner_session as u64;
    request.words[1] = x as i64 as u64;
    request.words[2] = y as i64 as u64;
    request.words[3] = width as u64;
    request.words[4] = height as u64;
    request.words[5] = z_order as u64;
    request.words[6] = fill_rgb as u64;
    request.words[7] = u64::from(visible);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(graphics_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != GraphicsTag::SurfaceCreateReply as u32
        || response.word_count < 2
        || response.handle_count < 1
    {
        return Err(Error::InvalidArgument);
    }

    match graphics_status_from_word(response.words[0]) {
        GraphicsStatus::Ok => Ok((response.words[1] as u32, response.handles[0])),
        status => Err(graphics_status_error(status)),
    }
}

pub fn graphics_surface_list(graphics_handle: Handle, ids: &mut [u32]) -> Result<usize> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(GraphicsTag::SurfaceListRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(graphics_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != GraphicsTag::SurfaceListReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }

    let status = graphics_status_from_word(response.words[0]);
    if status != GraphicsStatus::Ok {
        return Err(graphics_status_error(status));
    }
    let count = response.words[1] as usize;
    if count > ids.len() || (response.word_count as usize) < 2 + count {
        return Err(Error::BufferTooSmall);
    }
    for (index, id) in ids.iter_mut().enumerate().take(count) {
        *id = response.words[2 + index] as u32;
    }
    Ok(count)
}

pub fn graphics_surface_status(
    graphics_handle: Handle,
    surface_id: u32,
) -> Result<Option<GraphicsSurfaceStatusInfo>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(GraphicsTag::SurfaceStatusRequest as u32);
    request.word_count = 1;
    request.words[0] = surface_id as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(graphics_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != GraphicsTag::SurfaceStatusReply as u32 || response.word_count < 11 {
        return Err(Error::InvalidArgument);
    }

    let status = graphics_status_from_word(response.words[0]);
    if status == GraphicsStatus::NotFound {
        return Ok(None);
    }
    if status != GraphicsStatus::Ok {
        return Err(graphics_status_error(status));
    }

    Ok(Some(GraphicsSurfaceStatusInfo {
        surface_id: response.words[1] as u32,
        output_index: response.words[2] as u32,
        owner_session: response.words[3] as u32,
        x: response.words[4] as i64 as i32,
        y: response.words[5] as i64 as i32,
        width: response.words[6] as u32,
        height: response.words[7] as u32,
        z_order: response.words[8] as u32,
        fill_rgb: response.words[9] as u32,
        visible: response.words[10] != 0,
    }))
}

pub fn surface_set_geometry(
    surface_handle: Handle,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    z_order: u32,
) -> Result<()> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SurfaceTag::SetGeometryRequest as u32);
    request.word_count = 5;
    request.words[0] = x as i64 as u64;
    request.words[1] = y as i64 as u64;
    request.words[2] = width as u64;
    request.words[3] = height as u64;
    request.words[4] = z_order as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(surface_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SurfaceTag::SetGeometryReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match graphics_status_from_word(response.words[0]) {
        GraphicsStatus::Ok => Ok(()),
        status => Err(graphics_status_error(status)),
    }
}

pub fn surface_set_geometry_async(
    surface_handle: Handle,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    z_order: u32,
) -> Result<()> {
    let mut request = RawMessage::empty(SurfaceTag::SetGeometryRequest as u32);
    request.word_count = 5;
    request.words[0] = x as i64 as u64;
    request.words[1] = y as i64 as u64;
    request.words[2] = width as u64;
    request.words[3] = height as u64;
    request.words[4] = z_order as u64;
    channel_send(surface_handle, &request)
}

pub fn surface_set_fill(surface_handle: Handle, fill_rgb: u32) -> Result<()> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SurfaceTag::SetFillRequest as u32);
    request.word_count = 1;
    request.words[0] = fill_rgb as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(surface_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SurfaceTag::SetFillReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match graphics_status_from_word(response.words[0]) {
        GraphicsStatus::Ok => Ok(()),
        status => Err(graphics_status_error(status)),
    }
}

pub fn surface_set_visibility(surface_handle: Handle, visible: bool) -> Result<()> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SurfaceTag::SetVisibilityRequest as u32);
    request.word_count = 1;
    request.words[0] = u64::from(visible);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(surface_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SurfaceTag::SetVisibilityReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match graphics_status_from_word(response.words[0]) {
        GraphicsStatus::Ok => Ok(()),
        status => Err(graphics_status_error(status)),
    }
}

pub fn surface_clear_scene(surface_handle: Handle) -> Result<()> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SurfaceTag::ClearSceneRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(surface_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SurfaceTag::ClearSceneReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match graphics_status_from_word(response.words[0]) {
        GraphicsStatus::Ok => Ok(()),
        status => Err(graphics_status_error(status)),
    }
}

pub fn surface_set_rect(
    surface_handle: Handle,
    slot: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    color_rgb: u32,
    visible: bool,
) -> Result<()> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SurfaceTag::SetRectRequest as u32);
    request.word_count = 7;
    request.words[0] = slot as u64;
    request.words[1] = x as i64 as u64;
    request.words[2] = y as i64 as u64;
    request.words[3] = width as u64;
    request.words[4] = height as u64;
    request.words[5] = color_rgb as u64;
    request.words[6] = u64::from(visible);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(surface_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SurfaceTag::SetRectReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match graphics_status_from_word(response.words[0]) {
        GraphicsStatus::Ok => Ok(()),
        status => Err(graphics_status_error(status)),
    }
}

pub fn surface_set_label(
    surface_handle: Handle,
    slot: u32,
    x: i32,
    y: i32,
    color_rgb: u32,
    text: &str,
) -> Result<()> {
    let text_bytes = text.as_bytes();
    let packed_words = text_bytes.len().div_ceil(8);
    if 5 + packed_words > IPC_MAX_WORDS {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(SurfaceTag::SetLabelRequest as u32);
    request.word_count = 5 + pack_bytes(text_bytes, &mut request.words[5..])?;
    request.words[0] = slot as u64;
    request.words[1] = x as i64 as u64;
    request.words[2] = y as i64 as u64;
    request.words[3] = color_rgb as u64;
    request.words[4] = text_bytes.len() as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(surface_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SurfaceTag::SetLabelReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match graphics_status_from_word(response.words[0]) {
        GraphicsStatus::Ok => Ok(()),
        status => Err(graphics_status_error(status)),
    }
}

pub fn surface_close(surface_handle: Handle) -> Result<()> {
    let request = RawMessage::empty(SurfaceTag::CloseRequest as u32);
    channel_send(surface_handle, &request).map(|_| ())
}

pub fn surface_attach_buffer(
    surface_handle: Handle,
    buffer_handle: Handle,
    width: u32,
    height: u32,
    stride_pixels: u32,
) -> Result<()> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SurfaceTag::AttachBufferRequest as u32);
    request.word_count = 3;
    request.words[0] = width as u64;
    request.words[1] = height as u64;
    request.words[2] = stride_pixels as u64;
    request.handle_count = 2;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    request.handles[1] = buffer_handle;
    request.handle_rights[1] = rights::READ;
    channel_send(surface_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SurfaceTag::AttachBufferReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match graphics_status_from_word(response.words[0]) {
        GraphicsStatus::Ok => Ok(()),
        status => Err(graphics_status_error(status)),
    }
}

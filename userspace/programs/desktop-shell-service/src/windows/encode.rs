use super::*;

pub(crate) fn encode_window_page(state: &DesktopState, start: usize, reply: &mut rt::RawMessage) {
    let mut windows = [WindowState::empty(); APP_COUNT];
    let mut app_ids = [DesktopAppId::Settings; APP_COUNT];
    let mut total = 0usize;
    for slot in state.apps.iter().copied() {
        if !slot.running || slot.window.surface_id == 0 {
            continue;
        }
        windows[total] = slot.window;
        app_ids[total] = slot.app_id;
        total += 1;
    }
    for index in 0..total {
        let mut best = index;
        for candidate in index + 1..total {
            if windows[candidate].z_order < windows[best].z_order {
                best = candidate;
            }
        }
        windows.swap(index, best);
        app_ids.swap(index, best);
    }

    let mut returned = 0usize;
    for index in start..total.min(start + crate::WINDOW_PAGE_SIZE) {
        let base = 3 + returned * 5;
        let app_id = app_ids[index];
        let window = windows[index];
        reply.words[base] = app_id as u32 as u64;
        reply.words[base + 1] = window.surface_id as u64;
        reply.words[base + 2] = pack_window_flags(
            window.z_order,
            state.focused_app == Some(app_id),
            window.minimized,
            visible_on_workspace(state, app_ids[index]),
        );
        reply.words[base + 3] = pack_i32_pair(window.x, window.y);
        reply.words[base + 4] = pack_u32_pair(window.width, window.height);
        returned += 1;
    }
    reply.words[1] = returned as u64;
    reply.words[2] = if start + returned >= total {
        u32::MAX as u64
    } else {
        (start + returned) as u64
    };
    reply.word_count = (3 + returned * 5) as u32;
}

pub(crate) fn pack_window_flags(
    z_order: u32,
    focused: bool,
    minimized: bool,
    visible: bool,
) -> u64 {
    let mut flags = (z_order as u64) << 32;
    if focused {
        flags |= 0x1;
    }
    if minimized {
        flags |= 0x2;
    }
    if visible {
        flags |= 0x4;
    }
    flags
}

pub(crate) fn pack_i32_pair(first: i32, second: i32) -> u64 {
    (first as u32 as u64) | ((second as u32 as u64) << 32)
}

pub(crate) fn pack_u32_pair(first: u32, second: u32) -> u64 {
    first as u64 | ((second as u64) << 32)
}

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

mod assoc;
mod bridge;
mod control;
mod navigation;
mod ops;
mod persist;
mod recent;
mod render;
mod state;

use rt::{ControlTag, RawMessage};
use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::assoc::AssocTable;
use crate::control::{ControlFlow, poll_control};
use crate::navigation::{reload_directory, reopen_directory};
use crate::recent::RecentRing;
use crate::render::render;
use crate::state::{
    BUFFER_BYTES, BUFFER_HEIGHT, BUFFER_WIDTH, ExplorerEntry, ExplorerState, SURFACE_BUFFER_SLOTS,
    ViewMode,
};

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf101;
    }
    if startup.tag != ControlTag::Startup as u32
        || startup.handle_count < 3
        || startup.word_count < 4
    {
        return 0xf102;
    }

    let surface_handle = startup.handles[0];
    let control_handle = startup.handles[1];
    let storage_handle = startup.handles[2];
    let desktop_handle =
        rt::lookup_service(bootstrap, rt::ServiceId::DesktopShell).unwrap_or(rt::INVALID_HANDLE);
    let mut state = ExplorerState {
        width: startup.words[1] as u32,
        height: startup.words[2] as u32,
        focused: startup.words[3] != 0,
        loading_initial_directory: true,
        current_directory_handle: rt::INVALID_HANDLE,
        current_path: [0; state::MAX_STORAGE_PATH],
        current_path_len: 0,
        entries: [ExplorerEntry::empty(); state::MAX_ENTRIES],
        entry_count: 0,
        selected_index: 0,
        scroll_offset: 0,
        load_failed: false,
        view_mode: ViewMode::Directory,
        recent_sel: 0,
        press: None,
        dragging: false,
        open_with_pick: None,
        assoc: AssocTable::empty(),
        recent: RecentRing::empty(),
        persist_dir: rt::INVALID_HANDLE,
        dialog: None,
        prompt_input: [0; ops::NAME_MAX],
        prompt_len: 0,
        menu: None,
        await_context: None,
    };

    let mut buffers = match ui::SurfaceBuffers::<SURFACE_BUFFER_SLOTS>::new(
        surface_handle,
        BUFFER_WIDTH,
        BUFFER_HEIGHT,
        BUFFER_WIDTH,
        BUFFER_BYTES,
    ) {
        Ok(buffers) => buffers,
        Err(_) => return 0xf103,
    };
    let mut presenter = ui::FirstPresentSurface::new(surface_handle);
    let mut startup = ui::DeferredStartup::new();
    let (slot, buffer) = buffers.current();
    let _ = render(&mut presenter, slot, buffer, &state);

    loop {
        match ui::poll_app_lifecycle(bootstrap) {
            Ok(true) => break,
            Ok(false) => {}
            Err(_) => return 0xf105,
        }

        match poll_control(
            control_handle,
            &mut buffers,
            &mut presenter,
            storage_handle,
            desktop_handle,
            &mut state,
        ) {
            Ok(ControlFlow::Idle) => {}
            Ok(ControlFlow::Worked) => continue,
            Ok(ControlFlow::Exit) => break,
            Err(_) => return 0xf106,
        }

        match startup.run(|| {
            state.persist_dir = persist::ensure_store_dir(storage_handle);
            persist::load_associations(state.persist_dir, &mut state.assoc);
            persist::load_recent(state.persist_dir, &mut state.recent);
            let result = reopen_directory(&mut state, storage_handle)
                .and_then(|_| reload_directory(&mut state));
            state.loading_initial_directory = false;
            match result {
                Ok(()) => Ok(true),
                Err(_) => Ok(true),
            }
        }) {
            Ok(true) => {
                let (slot, buffer) = buffers.advance();
                let _ = render(&mut presenter, slot, buffer, &state);
                continue;
            }
            Ok(false) => {}
            Err(_) => return 0xf106,
        }

        if rt::yield_current().is_err() {
            return 0xf107;
        }
    }

    if state.current_directory_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(state.current_directory_handle);
    }
    0
}

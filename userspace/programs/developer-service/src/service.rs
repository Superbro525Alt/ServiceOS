use rt::{ControlTag, LifecycleEvent, RawMessage, ServiceId};
use serviceos_userspace_runtime as rt;

use crate::{
    consts::{MAX_JOBS, MAX_TOOLCHAINS, MAX_WORKSPACES},
    protocol::{Catalog, handle_public_request, poll_job_exits, poll_job_reports},
    registry,
    types::{JobSlot, ToolchainSlot, WorkspaceSlot},
    util::{emit_log, read_catalog},
};

pub(crate) fn run() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xfd01;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 3 {
        return 0xfd02;
    }

    let log_handle = startup.handles[0];
    let storage_handle = startup.handles[1];
    let catalog_handle = startup.handles[2];

    let mut toolchains = [ToolchainSlot::empty(); MAX_TOOLCHAINS];
    let mut workspaces = [WorkspaceSlot::empty(); MAX_WORKSPACES];
    let (toolchain_count, workspace_count) = match read_catalog(
        storage_handle,
        catalog_handle,
        &mut toolchains,
        &mut workspaces,
    ) {
        Ok(counts) => counts,
        Err(_) => return 0xfd03,
    };

    let mut registry = registry::build_registry(&toolchains, toolchain_count);

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xfd04,
    };
    if rt::register_service(bootstrap, ServiceId::Developer, public.second).is_err() {
        return 0xfd05;
    }
    let _ = rt::handle_close(public.second);

    let _ = emit_log(
        log_handle,
        rt::LogSeverity::Info,
        rt::LogEvent::DeveloperCatalogLoaded,
        toolchain_count as u64,
        workspace_count as u64
            | (registry::family_mask(&registry) << 16)
            | (registry::versioned_count(&registry).min(0xFF) << 44),
    );

    let mut jobs = [JobSlot::empty(); MAX_JOBS];

    loop {
        if poll_lifecycle(bootstrap).unwrap_or(false) {
            for job in &mut jobs {
                if job.occupied {
                    crate::util::release_job(job);
                }
            }
            let _ = rt::handle_close(storage_handle);
            return 0;
        }

        let mut had_work = false;
        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                had_work = true;
                let catalog = Catalog {
                    toolchains: &toolchains,
                    toolchain_count,
                    workspaces: &workspaces,
                    workspace_count,
                    registry: &mut registry,
                };
                if handle_public_request(
                    bootstrap,
                    storage_handle,
                    log_handle,
                    catalog,
                    &mut jobs,
                    &request,
                )
                .is_err()
                {
                    return 0xfd06;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xfd07,
        }

        poll_job_reports(log_handle, &mut jobs);
        poll_job_exits(log_handle, &mut jobs);

        if !had_work && rt::yield_current().is_err() {
            return 0xfd08;
        }
    }
}

fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
    let mut lifecycle = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut lifecycle) {
        Ok(()) if lifecycle.tag == ControlTag::Lifecycle as u32 && lifecycle.word_count >= 1 => {
            Ok(lifecycle.words[0] == LifecycleEvent::Stopped as u32 as u64)
        }
        Ok(()) => Ok(false),
        Err(rt::Error::QueueEmpty) => Ok(false),
        Err(error) => Err(error),
    }
}

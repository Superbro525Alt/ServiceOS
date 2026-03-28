#![no_std]
#![no_main]

mod control;
mod graph;
mod state;
mod util;

use serviceos_userspace_runtime as rt;
use rt::{ControlTag, LogEvent, LogSeverity, RawMessage, ServiceId, rights};

use crate::graph::{
    activate_base_service_graph, start_service, supervision_loop, wait_until_ready,
};
use crate::state::{storage_manifest, BootstrapResource, BootstrapResources, ServiceSlot, MAX_SERVICE_SLOTS};
use crate::util::{emit_manager_event, fallback_log};

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf601;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 2 || startup.word_count < 1
    {
        return 0xf602;
    }

    let bootstore_handle = startup.handles[0];
    let bootstrap_authority = startup.handles[1];
    let bootstore_len = startup.words[0] as usize;
    let network_resource = if startup.handle_count > 2 {
        Some(BootstrapResource {
            handle: startup.handles[2],
            len: 0,
            rights: rights::READ | rights::WRITE | rights::WAIT,
        })
    } else {
        None
    };
    let display_resource = if startup.handle_count > 3 {
        Some(BootstrapResource {
            handle: startup.handles[3],
            len: 0,
            rights: rights::READ | rights::WRITE,
        })
    } else {
        None
    };
    let input_resource = if startup.handle_count > 4 {
        Some(BootstrapResource {
            handle: startup.handles[4],
            len: 0,
            rights: rights::READ | rights::WAIT,
        })
    } else {
        None
    };
    let bootstrap_resources = BootstrapResources {
        bootstore: BootstrapResource {
            handle: bootstore_handle,
            len: bootstore_len,
            rights: rights::READ,
        },
        network: network_resource,
        display: display_resource,
        input: input_resource,
    };

    fallback_log("bootstrap started");

    let mut slots = [ServiceSlot::empty(); MAX_SERVICE_SLOTS];
    slots[0].manifest = storage_manifest();
    slots[0].occupied = true;
    let mut service_count = 1usize;

    if start_service(
        &mut slots,
        service_count,
        0,
        bootstrap_authority,
        Some((
            bootstrap_resources.bootstore.handle,
            bootstrap_resources.bootstore.len,
            bootstrap_resources.bootstore.rights,
        )),
    )
    .is_err()
    {
        return 0xf603;
    }
    if wait_until_ready(
        &mut slots,
        &mut service_count,
        bootstrap_authority,
        ServiceId::Storage,
    )
    .is_err()
    {
        return 0xf604;
    }

    if graph::load_base_service_graph(&mut slots, &mut service_count).is_err() {
        return 0xf605;
    }
    if activate_base_service_graph(
        &mut slots,
        &mut service_count,
        bootstrap_authority,
        bootstrap_resources,
    )
    .is_err()
    {
        return 0xf606;
    }

    let _ = emit_manager_event(
        &slots,
        service_count,
        LogSeverity::Info,
        LogEvent::ServiceReady,
        ServiceId::RootManager,
        0,
    );

    supervision_loop(
        &mut slots,
        &mut service_count,
        bootstrap_authority,
        bootstrap_resources,
    )
}

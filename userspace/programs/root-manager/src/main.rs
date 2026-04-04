#![no_std]
#![no_main]

mod control;
mod graph;
mod state;
mod util;

use serviceos_abi::bootstrap_resource;
use serviceos_userspace_runtime as rt;
use rt::{ControlTag, LogEvent, LogSeverity, RawMessage, ServiceId, rights};

use crate::graph::{
    activate_base_service_graph, start_service, supervision_loop, wait_until_ready,
};
use crate::state::{
    storage_manifest, BootstrapResource, BootstrapResources, GraphStatus, ServiceSlot,
    MAX_SERVICE_SLOTS,
};
use crate::util::{emit_manager_event, fallback_log, service_index_path};

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
    let bootstrap_flags = if startup.word_count > 2 {
        startup.words[2]
    } else {
        0
    };
    let platform = if startup.word_count > 1 {
        match startup.words[1] as u32 {
            x if x == rt::BootstrapPlatform::QemuVirtio as u32 => rt::BootstrapPlatform::QemuVirtio,
            x if x == rt::BootstrapPlatform::Raspi5 as u32 => rt::BootstrapPlatform::Raspi5,
            _ => rt::BootstrapPlatform::Unknown,
        }
    } else {
        rt::BootstrapPlatform::Unknown
    };
    let mut next_handle = 2usize;
    let take_bootstrap_resource =
        |next_handle: &mut usize, present: bool, rights_mask: u64| -> Option<BootstrapResource> {
            if !present || *next_handle >= startup.handle_count as usize {
                return None;
            }
            let resource = BootstrapResource {
                handle: startup.handles[*next_handle],
                len: 0,
                rights: rights_mask,
            };
            *next_handle += 1;
            Some(resource)
        };
    let block_resource = take_bootstrap_resource(
        &mut next_handle,
        bootstrap_flags & bootstrap_resource::BLOCK != 0,
        rights::READ | rights::WRITE,
    );
    let network_resource = take_bootstrap_resource(
        &mut next_handle,
        bootstrap_flags & bootstrap_resource::NETWORK != 0,
        rights::READ | rights::WRITE | rights::WAIT,
    );
    let display_resource = take_bootstrap_resource(
        &mut next_handle,
        bootstrap_flags & bootstrap_resource::DISPLAY != 0,
        rights::READ | rights::WRITE,
    );
    let input_resource = take_bootstrap_resource(
        &mut next_handle,
        bootstrap_flags & bootstrap_resource::INPUT != 0,
        rights::READ | rights::WAIT,
    );
    let audio_resource = take_bootstrap_resource(
        &mut next_handle,
        bootstrap_flags & bootstrap_resource::AUDIO != 0,
        rights::READ | rights::WRITE,
    );
    let bootstrap_resources = BootstrapResources {
        bootstore: BootstrapResource {
            handle: bootstore_handle,
            len: bootstore_len,
            rights: rights::READ,
        },
        block: block_resource,
        network: network_resource,
        display: display_resource,
        input: input_resource,
        audio: audio_resource,
    };

    fallback_log("bootstrap started");

    let mut slots = [ServiceSlot::empty(); MAX_SERVICE_SLOTS];
    let mut graph_status = GraphStatus::empty();
    slots[0].manifest = storage_manifest();
    slots[0].occupied = true;
    let mut service_count = 1usize;

    if start_service(
        &mut slots,
        service_count,
        0,
        bootstrap_authority,
        bootstrap_resources,
    )
    .is_err()
    {
        return 0xf603;
    }
    if wait_until_ready(
        &mut slots,
        &mut service_count,
        bootstrap_authority,
        bootstrap_resources,
        ServiceId::Storage,
    )
    .is_err()
    {
        return 0xf604;
    }

    if graph::load_base_service_graph(
        &mut slots,
        &mut service_count,
        service_index_path(platform),
    )
    .is_err()
    {
        return 0xf605;
    }
    if activate_base_service_graph(
        &mut slots,
        &mut service_count,
        bootstrap_authority,
        bootstrap_resources,
        &mut graph_status,
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
        &mut graph_status,
    )
}

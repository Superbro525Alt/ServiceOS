#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

use smoltcp::{
    iface::{Config as IfaceConfig, Interface, SocketSet, SocketStorage},
    socket::{dhcpv4, icmp, tcp, udp},
    wire::{EthernetAddress, HardwareAddress},
};

use rt::{ControlTag, LogEvent, LogSeverity, NetworkConfigState, RawMessage, ServiceId};
use serviceos_userspace_runtime as rt;

mod beacon;
mod cache;
mod config;
mod consts;
mod device;
mod diag;
mod discover;
mod dnsmsg;
mod dnsresolv;
mod firewall;
mod mdns;
mod protocol;
mod types;
mod util;
mod wifi;

rt::entry!(run);

use crate::{
    cache::ResolverCache,
    config::{load_hosts, read_network_config},
    consts::{
        DNS_UDP_BUFFER_BYTES, MAX_HOSTS, MAX_TCP_LISTENERS, MAX_TCP_SOCKETS, MAX_UDP_SOCKETS,
        TCP_SOCKET_BUFFER_BYTES, UDP_DATAGRAM_BUFFER_BYTES,
    },
    device::KernelPacketDevice,
    firewall::FirewallState,
    protocol::{
        apply_interface_runtime, drive_dynamic_ipv4, handle_datagram_request,
        handle_listener_request, handle_public_request, handle_socket_request, pump_listeners,
        run_network_selftest, update_transport_states,
    },
    types::{HostEntry, InterfaceRuntimeState, TcpListenerSlot, TcpTransportSlot, UdpDatagramSlot},
    util::{emit_log, now_instant, pack_mac, poll_lifecycle, ticks_to_millis},
};

pub(crate) fn run() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xfb01;
    }
    if startup.tag != ControlTag::Startup as u32
        || startup.handle_count < 2
        || startup.word_count < 4
    {
        return 0xfb02;
    }

    let grant_count = startup.words[2] as usize;
    let resource_count = startup.words[3] as usize;
    let packet_handle = startup.handles[0];
    let log_handle = startup.handles[1];
    let resource_base = 1 + grant_count;
    let hosts_handle = if resource_count > 0 && resource_base < startup.handle_count as usize {
        startup.handles[resource_base]
    } else {
        rt::INVALID_HANDLE
    };

    let config_handle = match rt::lookup_service(bootstrap, ServiceId::Config) {
        Ok(handle) => handle,
        Err(_) => return 0xfb03,
    };
    let config = match read_network_config(config_handle) {
        Ok(config) => config,
        Err(_) => return 0xfb04,
    };
    let _ = rt::handle_close(config_handle);

    let mut wifi = crate::wifi::WifiState::new();
    let mut hosts = [HostEntry::empty(); MAX_HOSTS];
    let mut net_options = crate::config::NetFileOptions::defaults();
    let host_count = match load_hosts(hosts_handle, &mut hosts, &mut net_options, &mut wifi) {
        Ok(count) => count,
        Err(_) => return 0xfb05,
    };
    let mut identity =
        types::HostIdentity::from_label(&net_options.hostname[..net_options.hostname_len]);

    let packet_info = match rt::packet_interface_info(packet_handle) {
        Ok(info) => info,
        Err(error) => {
            let _ = rt::write_logf(
                "network",
                format_args!("packet-interface-info failed: {:?}", error),
            );
            return 0xfb06;
        }
    };

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xfb07,
    };
    if rt::register_service(bootstrap, ServiceId::Network, public.second).is_err() {
        return 0xfb08;
    }
    let _ = rt::handle_close(public.second);

    let mut device = KernelPacketDevice::new(packet_handle, packet_info);
    if net_options.rx_ring_enabled {
        if device::enable_shared_rx(packet_handle) {
            let _ = rt::write_logf(
                "network",
                format_args!(
                    "rx-ring enabled slots=16 zero-copy rx path active (rx-ring=off reverts to copied frames)"
                ),
            );
        } else {
            let _ = rt::write_logf(
                "network",
                format_args!("rx-ring negotiation failed; legacy copied-frame path active"),
            );
        }
    } else {
        let _ = rt::write_logf(
            "network",
            format_args!("rx-ring disabled by config; legacy copied-frame path active"),
        );
    }
    if net_options.tx_ring_enabled {
        if device::enable_shared_tx(packet_handle) {
            let _ = rt::write_logf(
                "network",
                format_args!(
                    "tx-ring enabled slots=16 zero-copy tx path active (tx-ring=off reverts to copied transmits)"
                ),
            );
        } else {
            let _ = rt::write_logf(
                "network",
                format_args!("tx-ring negotiation failed; legacy copied-transmit path active"),
            );
        }
    } else {
        let _ = rt::write_logf(
            "network",
            format_args!("tx-ring disabled by config; legacy copied-transmit path active"),
        );
    }
    let now = now_instant();
    let mac = EthernetAddress(device.info.mac);
    let mut iface = Interface::new(
        IfaceConfig::new(HardwareAddress::Ethernet(mac)),
        &mut device,
        now,
    );

    let mut runtime_state = if config.dynamic_ipv4 {
        InterfaceRuntimeState::pending_dynamic()
    } else {
        InterfaceRuntimeState::static_config(config)
    };
    apply_interface_runtime(&mut iface, runtime_state);

    let mut socket_storage = [SocketStorage::EMPTY; 16];
    // Two rx slots: a self-addressed ping6 loopback delivers BOTH the echo
    // request (ident-matched on the way back through the stack) and the
    // echo reply to this socket; one slot would drop the reply.
    let mut icmp_rx_meta = [icmp::PacketMetadata::EMPTY; 2];
    let mut icmp_tx_meta = [icmp::PacketMetadata::EMPTY];
    let mut icmp_rx_data = [0u8; 256];
    let mut icmp_tx_data = [0u8; 256];
    let icmp_socket = icmp::Socket::new(
        icmp::PacketBuffer::new(&mut icmp_rx_meta[..], &mut icmp_rx_data[..]),
        icmp::PacketBuffer::new(&mut icmp_tx_meta[..], &mut icmp_tx_data[..]),
    );
    let dhcp_socket = dhcpv4::Socket::new();
    // Dedicated DNS client socket (own wire codec; see dnsresolv.rs). Bound
    // to an ephemeral local port; queries are identified by transaction id.
    let mut dns_rx = [0u8; DNS_UDP_BUFFER_BYTES];
    let mut dns_tx = [0u8; DNS_UDP_BUFFER_BYTES];
    let mut dns_rx_meta = [udp::PacketMetadata::EMPTY];
    let mut dns_tx_meta = [udp::PacketMetadata::EMPTY];
    let dns_client_socket = udp::Socket::new(
        udp::PacketBuffer::new(&mut dns_rx_meta[..], &mut dns_rx[..]),
        udp::PacketBuffer::new(&mut dns_tx_meta[..], &mut dns_tx[..]),
    );
    // mDNS-LITE responder socket (<hostname>.local A answers, UDP 5353).
    let mut mdns_rx = [0u8; DNS_UDP_BUFFER_BYTES];
    let mut mdns_tx = [0u8; DNS_UDP_BUFFER_BYTES];
    let mut mdns_rx_meta = [udp::PacketMetadata::EMPTY];
    let mut mdns_tx_meta = [udp::PacketMetadata::EMPTY];
    let mdns_socket = udp::Socket::new(
        udp::PacketBuffer::new(&mut mdns_rx_meta[..], &mut mdns_rx[..]),
        udp::PacketBuffer::new(&mut mdns_tx_meta[..], &mut mdns_tx[..]),
    );
    // Discovery beacon socket (service-local announce/query protocol).
    let mut beacon_rx = [0u8; DNS_UDP_BUFFER_BYTES];
    let mut beacon_tx = [0u8; DNS_UDP_BUFFER_BYTES];
    let mut beacon_rx_meta = [udp::PacketMetadata::EMPTY];
    let mut beacon_tx_meta = [udp::PacketMetadata::EMPTY];
    let beacon_socket = udp::Socket::new(
        udp::PacketBuffer::new(&mut beacon_rx_meta[..], &mut beacon_rx[..]),
        udp::PacketBuffer::new(&mut beacon_tx_meta[..], &mut beacon_tx[..]),
    );
    let mut tcp0_rx = [0u8; TCP_SOCKET_BUFFER_BYTES];
    let mut tcp0_tx = [0u8; TCP_SOCKET_BUFFER_BYTES];
    let mut tcp1_rx = [0u8; TCP_SOCKET_BUFFER_BYTES];
    let mut tcp1_tx = [0u8; TCP_SOCKET_BUFFER_BYTES];
    let tcp0 = tcp::Socket::new(
        tcp::SocketBuffer::new(&mut tcp0_rx[..]),
        tcp::SocketBuffer::new(&mut tcp0_tx[..]),
    );
    let tcp1 = tcp::Socket::new(
        tcp::SocketBuffer::new(&mut tcp1_rx[..]),
        tcp::SocketBuffer::new(&mut tcp1_tx[..]),
    );
    let mut sockets = SocketSet::new(&mut socket_storage[..]);
    let icmp_handle = sockets.add(icmp_socket);
    let dhcp_handle = sockets.add(dhcp_socket);
    let dns_client_handle = sockets.add(dns_client_socket);
    let mdns_handle = sockets.add(mdns_socket);
    let beacon_handle = sockets.add(beacon_socket);
    let tcp_handles = [sockets.add(tcp0), sockets.add(tcp1)];
    let mut udp_a_rx_meta = [udp::PacketMetadata::EMPTY; 4];
    let mut udp_a_rx_data = [0u8; UDP_DATAGRAM_BUFFER_BYTES];
    let mut udp_a_tx_meta = [udp::PacketMetadata::EMPTY; 4];
    let mut udp_a_tx_data = [0u8; UDP_DATAGRAM_BUFFER_BYTES];
    let mut udp_b_rx_meta = [udp::PacketMetadata::EMPTY; 4];
    let mut udp_b_rx_data = [0u8; UDP_DATAGRAM_BUFFER_BYTES];
    let mut udp_b_tx_meta = [udp::PacketMetadata::EMPTY; 4];
    let mut udp_b_tx_data = [0u8; UDP_DATAGRAM_BUFFER_BYTES];
    let mut udp_c_rx_meta = [udp::PacketMetadata::EMPTY; 4];
    let mut udp_c_rx_data = [0u8; UDP_DATAGRAM_BUFFER_BYTES];
    let mut udp_c_tx_meta = [udp::PacketMetadata::EMPTY; 4];
    let mut udp_c_tx_data = [0u8; UDP_DATAGRAM_BUFFER_BYTES];
    let mut udp_d_rx_meta = [udp::PacketMetadata::EMPTY; 4];
    let mut udp_d_rx_data = [0u8; UDP_DATAGRAM_BUFFER_BYTES];
    let mut udp_d_tx_meta = [udp::PacketMetadata::EMPTY; 4];
    let mut udp_d_tx_data = [0u8; UDP_DATAGRAM_BUFFER_BYTES];
    let udp_sockets = [
        udp::Socket::new(
            udp::PacketBuffer::new(&mut udp_a_rx_meta[..], &mut udp_a_rx_data[..]),
            udp::PacketBuffer::new(&mut udp_a_tx_meta[..], &mut udp_a_tx_data[..]),
        ),
        udp::Socket::new(
            udp::PacketBuffer::new(&mut udp_b_rx_meta[..], &mut udp_b_rx_data[..]),
            udp::PacketBuffer::new(&mut udp_b_tx_meta[..], &mut udp_b_tx_data[..]),
        ),
        udp::Socket::new(
            udp::PacketBuffer::new(&mut udp_c_rx_meta[..], &mut udp_c_rx_data[..]),
            udp::PacketBuffer::new(&mut udp_c_tx_meta[..], &mut udp_c_tx_data[..]),
        ),
        udp::Socket::new(
            udp::PacketBuffer::new(&mut udp_d_rx_meta[..], &mut udp_d_rx_data[..]),
            udp::PacketBuffer::new(&mut udp_d_tx_meta[..], &mut udp_d_tx_data[..]),
        ),
    ];
    let mut udp_handles = [icmp_handle; MAX_UDP_SOCKETS];
    for (index, datagram) in udp_sockets.into_iter().enumerate() {
        udp_handles[index] = sockets.add(datagram);
    }
    let mut transports = [TcpTransportSlot::empty(); MAX_TCP_SOCKETS];
    let mut udp_slots = [UdpDatagramSlot::empty(); MAX_UDP_SOCKETS];
    let mut listeners = [TcpListenerSlot::empty(); MAX_TCP_LISTENERS];
    let mut next_sequence = 1u16;
    let mut next_local_port = crate::consts::EPHEMERAL_PORT_BASE;
    let mut dhcp_started_at = rt::monotonic_now().unwrap_or(0);
    let mut selftest_done = false;
    let mut resolver_cache = ResolverCache::new();
    let mut firewall = FirewallState::new();
    let mut next_query_id = 1u16;

    // Bind the DNS client socket to a deterministic ephemeral port before any
    // request-driven ephemeral allocation can collide with it.
    {
        let dns_local_port = next_local_port;
        next_local_port = next_local_port.saturating_add(1);
        if sockets
            .get_mut::<udp::Socket>(dns_client_handle)
            .bind(dns_local_port)
            .is_err()
        {
            return 0xfb12;
        }
    }

    let _ = rt::write_logf(
        "network",
        format_args!(
            "resolver-cache ready capacity={} entries=0 hits=0 misses=0",
            crate::consts::MAX_RESOLVER_CACHE_ENTRIES
        ),
    );
    let _ = rt::write_logf(
        "network",
        format_args!(
            "firewall state rules=0 default-inbound={} dns-client-port={}",
            if firewall.default_inbound_allow {
                "allow"
            } else {
                "deny"
            },
            next_local_port.wrapping_sub(1)
        ),
    );

    let _ = emit_log(
        log_handle,
        LogSeverity::Info,
        LogEvent::NetworkInterfaceReady,
        0,
        pack_mac(device.info.mac),
    );
    if runtime_state.state != NetworkConfigState::Pending {
        let _ = emit_log(
            log_handle,
            LogSeverity::Info,
            LogEvent::NetworkAddressConfigured,
            crate::util::ipv4_to_u32(runtime_state.address) as u64,
            crate::util::ipv4_to_u32(runtime_state.gateway) as u64,
        );
    }

    let mut loop_ticks: u64 = 0;
    let mut rx_ring_first_frame_logged = false;
    let mut tx_ring_first_flush_logged = false;
    let mut registry = discover::Registry::new();
    let mut peer_table = discover::PeerTable::new();
    let mut next_announce_loop = 0u64;
    let mut announce_logged = false;

    // Bind the fixed internal service ports before any request-driven
    // ephemeral allocation can collide with them.
    if sockets
        .get_mut::<udp::Socket>(mdns_handle)
        .bind(crate::consts::MDNS_UDP_PORT)
        .is_err()
        || sockets
            .get_mut::<udp::Socket>(beacon_handle)
            .bind(crate::consts::BEACON_UDP_PORT)
            .is_err()
    {
        return 0xfb13;
    }

    if net_options.mdns_enabled {
        let _ = rt::write_logf(
            "network",
            format_args!(
                "mdns-lite responder ready hostname={}.local port={} enabled=1",
                core::str::from_utf8(&identity.name[..identity.name_len]).unwrap_or("host"),
                crate::consts::MDNS_UDP_PORT
            ),
        );
    }
    if net_options.discovery_enabled {
        let _ = rt::write_logf(
            "network",
            format_args!(
                "discovery beacon ready port={} services={} peers-capacity={}",
                crate::consts::BEACON_UDP_PORT,
                registry.count,
                peer_table.count
            ),
        );
    }

    loop {
        loop_ticks += 1;
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xfb09,
        }

        let _ = iface.poll(now_instant(), &mut device, &mut sockets);
        // First-frame evidence: as soon as any frame has flowed through the
        // shared RX ring (e.g. the DHCP offer from slirp), log the live
        // zero-copy counters once so boot logs prove the shared path works.
        if !rx_ring_first_frame_logged {
            let snapshot = device::rx_ring_snapshot();
            if snapshot.active && snapshot.copies_avoided > 0 {
                rx_ring_first_frame_logged = true;
                let _ = rt::write_logf(
                    "network",
                    format_args!(
                        "rx-ring first frame via shared path copies-avoided={} bytes-saved={} frames-pushed={}",
                        snapshot.copies_avoided, snapshot.bytes_saved, snapshot.frames_pushed
                    ),
                );
            }
        }
        // First-flush evidence: as soon as any outbound frame has drained
        // through the shared TX ring (the DHCP discover is typically first),
        // log the kernel-banked tx-copies-avoided counter once so boot logs
        // prove the shared TX path works end to end.
        if !tx_ring_first_flush_logged {
            let snapshot = device::tx_ring_snapshot();
            if snapshot.active && snapshot.copies_avoided > 0 {
                tx_ring_first_flush_logged = true;
                let _ = rt::write_logf(
                    "network",
                    format_args!(
                        "tx-ring first flush via shared path tx-copies-avoided={} tx-bytes-saved={} tx-frames-pushed={}",
                        snapshot.copies_avoided, snapshot.bytes_saved, snapshot.frames_pushed
                    ),
                );
            }
        }
        if config.dynamic_ipv4
            && drive_dynamic_ipv4(
                &config,
                log_handle,
                &mut runtime_state,
                &mut dhcp_started_at,
                &mut iface,
                &mut sockets,
                dhcp_handle,
            )
            .is_err()
        {
            return 0xfb0a;
        }

        update_transport_states(log_handle, config, &mut sockets, &mut transports);
        if pump_listeners(
            log_handle,
            &mut listeners,
            &mut transports,
            tcp_handles,
            &mut sockets,
            &mut firewall,
        )
        .is_err()
        {
            return 0xfb0f;
        }

        // NOTE: the userspace monotonic clock observable through
        // rt::monotonic_now() does not advance on this kernel build, so the
        // selftest trigger uses loop iterations as its delay instead of a
        // tick delta (see docs: pre-existing kernel-side behavior).
        if !selftest_done
            && runtime_state.state != NetworkConfigState::Pending
            && loop_ticks >= 1024
        {
            selftest_done = true;
            run_network_selftest(
                log_handle,
                &mut iface,
                &mut device,
                &mut sockets,
                icmp_handle,
                runtime_state.gateway,
            );
            let snapshot = device::rx_ring_snapshot();
            if snapshot.active {
                let _ = rt::write_logf(
                    "network",
                    format_args!(
                        "rx-ring stats frames-pushed={} copies-avoided={} bytes-saved={} dropped={}",
                        snapshot.frames_pushed,
                        snapshot.copies_avoided,
                        snapshot.bytes_saved,
                        snapshot.dropped
                    ),
                );
            }
            let tx = device::tx_ring_snapshot();
            if tx.active {
                let _ = rt::write_logf(
                    "network",
                    format_args!(
                        "tx-ring stats tx-frames-pushed={} tx-copies-avoided={} tx-bytes-saved={}",
                        tx.frames_pushed, tx.copies_avoided, tx.bytes_saved
                    ),
                );
            }
        }

        // mDNS-LITE responder + discovery beacon: served directly from the
        // main loop once the interface has an address.
        if runtime_state.state != NetworkConfigState::Pending
            && runtime_state.address != smoltcp::wire::Ipv4Address::UNSPECIFIED
        {
            if net_options.mdns_enabled {
                let _ = mdns::pump(
                    &mut iface,
                    &mut device,
                    &mut sockets,
                    mdns_handle,
                    &identity,
                    runtime_state.address,
                );
            }
            if net_options.discovery_enabled {
                let now_ms = ticks_to_millis(rt::monotonic_now().unwrap_or(0));
                let _ = beacon::pump(
                    &mut iface,
                    &mut device,
                    &mut sockets,
                    beacon_handle,
                    &registry,
                    &mut peer_table,
                    &identity,
                    now_ms,
                    runtime_state.address,
                );
                if loop_ticks >= next_announce_loop {
                    next_announce_loop =
                        loop_ticks.saturating_add(crate::consts::BEACON_ANNOUNCE_PERIOD_LOOPS);
                    match beacon::announce(
                        &mut iface,
                        &mut device,
                        &mut sockets,
                        beacon_handle,
                        &registry,
                        &identity,
                        runtime_state.address,
                    ) {
                        Ok(true) => {
                            // Pair every announce with a query solicitation so
                            // quiet peers introduce themselves.
                            let _ = beacon::solicit(
                                &mut iface,
                                &mut device,
                                &mut sockets,
                                beacon_handle,
                            );
                            if !announce_logged {
                                announce_logged = true;
                                let _ = rt::write_logf(
                                    "network",
                                    format_args!(
                                        "beacon announce sent services={} peers={}",
                                        registry.count, peer_table.count
                                    ),
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                if handle_public_request(
                    &request,
                    packet_handle,
                    log_handle,
                    config,
                    runtime_state,
                    &hosts[..host_count],
                    &mut iface,
                    &mut device,
                    &mut sockets,
                    dns_client_handle,
                    icmp_handle,
                    &mut next_sequence,
                    &mut next_query_id,
                    &mut resolver_cache,
                    &mut firewall,
                    &mut transports,
                    tcp_handles,
                    &mut next_local_port,
                    &mut udp_slots,
                    udp_handles,
                    &mut listeners,
                    &mut identity,
                    &mut registry,
                    &mut peer_table,
                    &mut wifi,
                )
                .is_err()
                {
                    return 0xfb0b;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xfb0c,
        }

        for index in 0..transports.len() {
            if !transports[index].active {
                continue;
            }
            let mut socket_request = RawMessage::empty(0);
            match rt::channel_receive_nonblocking(
                transports[index].control_handle,
                &mut socket_request,
            ) {
                Ok(()) => {
                    if handle_socket_request(
                        log_handle,
                        config,
                        &mut sockets,
                        &mut transports[index],
                        &socket_request,
                    )
                    .is_err()
                    {
                        return 0xfb0d;
                    }
                }
                Err(rt::Error::QueueEmpty) => {}
                Err(_) => {
                    crate::protocol::close_transport_slot(
                        log_handle,
                        &mut sockets,
                        &mut transports[index],
                    );
                }
            }
        }

        for index in 0..udp_slots.len() {
            if !udp_slots[index].active {
                continue;
            }
            let mut datagram_request = RawMessage::empty(0);
            match rt::channel_receive_nonblocking(
                udp_slots[index].control_handle,
                &mut datagram_request,
            ) {
                Ok(()) => {
                    if handle_datagram_request(
                        log_handle,
                        &mut sockets,
                        &mut udp_slots[index],
                        &datagram_request,
                        &mut firewall,
                    )
                    .is_err()
                    {
                        return 0xfb10;
                    }
                }
                Err(rt::Error::QueueEmpty) => {}
                Err(_) => {
                    crate::protocol::close_udp_slot(&mut sockets, &mut udp_slots[index]);
                }
            }
        }

        for index in 0..listeners.len() {
            if !listeners[index].active {
                continue;
            }
            let mut listener_request = RawMessage::empty(0);
            match rt::channel_receive_nonblocking(
                listeners[index].control_handle,
                &mut listener_request,
            ) {
                Ok(()) => {
                    if handle_listener_request(
                        log_handle,
                        index,
                        &mut listeners,
                        &mut transports,
                        &mut sockets,
                        &listener_request,
                    )
                    .is_err()
                    {
                        return 0xfb11;
                    }
                }
                Err(rt::Error::QueueEmpty) => {}
                Err(_) => {
                    let mut listener = TcpListenerSlot::empty();
                    core::mem::swap(&mut listener, &mut listeners[index]);
                    crate::protocol::close_listener_slot(
                        log_handle,
                        &mut transports,
                        &mut sockets,
                        &mut listener,
                    );
                }
            }
        }

        if rt::yield_current().is_err() {
            return 0xfb0e;
        }
    }
}

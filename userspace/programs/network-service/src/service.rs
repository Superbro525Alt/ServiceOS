use smoltcp::{
    iface::{Config as IfaceConfig, Interface, SocketSet, SocketStorage},
    socket::{dhcpv4, dns, icmp, tcp},
    wire::{EthernetAddress, HardwareAddress},
};

use serviceos_userspace_runtime as rt;
use rt::{
    ControlTag, LogEvent, LogSeverity, NetworkConfigState, RawMessage, ServiceId,
};

use crate::{
    config::{load_hosts, read_network_config},
    consts::{MAX_DNS_QUERY_SLOTS, MAX_HOSTS, TCP_SOCKET_BUFFER_BYTES},
    device::KernelPacketDevice,
    protocol::{
        apply_interface_runtime, drive_dynamic_ipv4, handle_public_request, handle_socket_request,
        update_transport_states,
    },
    types::{HostEntry, InterfaceRuntimeState, TcpTransportSlot},
    util::{emit_log, now_instant, pack_mac, poll_lifecycle},
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

    let mut hosts = [HostEntry::empty(); MAX_HOSTS];
    let host_count = match load_hosts(hosts_handle, &mut hosts) {
        Ok(count) => count,
        Err(_) => return 0xfb05,
    };

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

    let mut socket_storage = [SocketStorage::EMPTY; 5];
    let mut icmp_rx_meta = [icmp::PacketMetadata::EMPTY];
    let mut icmp_tx_meta = [icmp::PacketMetadata::EMPTY];
    let mut icmp_rx_data = [0u8; 256];
    let mut icmp_tx_data = [0u8; 256];
    let icmp_socket = icmp::Socket::new(
        icmp::PacketBuffer::new(&mut icmp_rx_meta[..], &mut icmp_rx_data[..]),
        icmp::PacketBuffer::new(&mut icmp_tx_meta[..], &mut icmp_tx_data[..]),
    );
    let dhcp_socket = dhcpv4::Socket::new();
    let mut dns_queries = [const { None }; MAX_DNS_QUERY_SLOTS];
    let initial_dns_servers = [smoltcp::wire::IpAddress::Ipv4(runtime_state.dns_server)];
    let dns_socket = dns::Socket::new(&initial_dns_servers, &mut dns_queries[..]);
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
    let dns_handle = sockets.add(dns_socket);
    let tcp_handles = [sockets.add(tcp0), sockets.add(tcp1)];
    let mut transports = [TcpTransportSlot::empty(); 2];
    let mut next_sequence = 1u16;
    let mut next_local_port = crate::consts::EPHEMERAL_PORT_BASE;
    let mut dhcp_started_at = rt::monotonic_now().unwrap_or(0);

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

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xfb09,
        }

        let _ = iface.poll(now_instant(), &mut device, &mut sockets);
        if config.dynamic_ipv4
            && drive_dynamic_ipv4(
                &config,
                log_handle,
                &mut runtime_state,
                &mut dhcp_started_at,
                &mut iface,
                &mut sockets,
                dhcp_handle,
                dns_handle,
            )
            .is_err()
        {
            return 0xfb0a;
        }

        update_transport_states(log_handle, config, &mut sockets, &mut transports);

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
                    dns_handle,
                    icmp_handle,
                    &mut next_sequence,
                    &mut transports,
                    tcp_handles,
                    &mut next_local_port,
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

        if rt::yield_current().is_err() {
            return 0xfb0e;
        }
    }
}

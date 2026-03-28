#![no_std]
#![no_main]

use smoltcp::{
    iface::{Config as IfaceConfig, Interface, SocketHandle, SocketSet, SocketStorage},
    phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken},
    socket::{dhcpv4, dns, icmp, tcp},
    time::Instant,
    wire::{
        DnsQueryType, EthernetAddress, HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress,
        IpCidr, Ipv4Address, Ipv4Cidr,
    },
};

use serviceos_userspace_runtime as rt;
use rt::{
    ConfigKey, ControlTag, LifecycleEvent, LogEvent, LogSeverity, NetworkConfigMode,
    NetworkConfigState, NetworkSocketKind, NetworkSocketState, NetworkSocketTag, NetworkStatus,
    NetworkTag, PacketInterfaceInfo, RawMessage, ServiceId,
};

const MAX_HOSTS: usize = 8;
const MAX_HOSTNAME_BYTES: usize = 48;
const MAX_HOSTS_RESOURCE_BYTES: usize = 256;
const MAX_FRAME_BYTES: usize = 1536;
const MAX_DNS_QUERY_SLOTS: usize = 4;
const MAX_TCP_SOCKETS: usize = 2;
const TCP_SOCKET_BUFFER_BYTES: usize = 1024;
const PING_IDENTIFIER: u16 = 0x534f;
const EPHEMERAL_PORT_BASE: u16 = 49_152;
const MAX_SOCKET_INLINE_BYTES: usize = (rt::IPC_MAX_WORDS - 2) * 8;

#[derive(Clone, Copy)]
struct HostEntry {
    name: [u8; MAX_HOSTNAME_BYTES],
    name_len: usize,
    address: Ipv4Address,
}

impl HostEntry {
    const fn empty() -> Self {
        Self {
            name: [0; MAX_HOSTNAME_BYTES],
            name_len: 0,
            address: Ipv4Address::UNSPECIFIED,
        }
    }

    fn matches(&self, target: &str) -> bool {
        self.name_len == target.len() && self.name[..self.name_len] == *target.as_bytes()
    }
}

#[derive(Clone, Copy)]
struct NetworkConfig {
    static_address: Ipv4Address,
    static_prefix_len: u8,
    static_gateway: Ipv4Address,
    dynamic_ipv4: bool,
    dns_server: Ipv4Address,
    probe_timeout_ticks: u64,
    dns_query_timeout_ticks: u64,
    dhcp_acquire_timeout_ticks: u64,
    tcp_connect_timeout_ticks: u64,
    tcp_idle_timeout_ticks: u64,
}

#[derive(Clone, Copy)]
struct InterfaceRuntimeState {
    mode: NetworkConfigMode,
    state: NetworkConfigState,
    address: Ipv4Address,
    prefix_len: u8,
    gateway: Ipv4Address,
    dns_server: Ipv4Address,
}

impl InterfaceRuntimeState {
    fn pending_dynamic() -> Self {
        Self {
            mode: NetworkConfigMode::Dynamic,
            state: NetworkConfigState::Pending,
            address: Ipv4Address::UNSPECIFIED,
            prefix_len: 0,
            gateway: Ipv4Address::UNSPECIFIED,
            dns_server: Ipv4Address::UNSPECIFIED,
        }
    }

    fn static_config(config: NetworkConfig) -> Self {
        Self {
            mode: if config.dynamic_ipv4 {
                NetworkConfigMode::Dynamic
            } else {
                NetworkConfigMode::Static
            },
            state: if config.dynamic_ipv4 {
                NetworkConfigState::FallbackStatic
            } else {
                NetworkConfigState::Configured
            },
            address: config.static_address,
            prefix_len: config.static_prefix_len,
            gateway: config.static_gateway,
            dns_server: config.dns_server,
        }
    }
}

struct KernelPacketDevice {
    handle: rt::Handle,
    info: PacketInterfaceInfo,
    rx_buffer: [u8; MAX_FRAME_BYTES],
    tx_buffer: [u8; MAX_FRAME_BYTES],
}

impl KernelPacketDevice {
    fn new(handle: rt::Handle, info: PacketInterfaceInfo) -> Self {
        Self {
            handle,
            info,
            rx_buffer: [0; MAX_FRAME_BYTES],
            tx_buffer: [0; MAX_FRAME_BYTES],
        }
    }
}

struct KernelRxToken<'a> {
    buffer: &'a mut [u8],
}

struct KernelTxToken<'a> {
    handle: rt::Handle,
    buffer: &'a mut [u8],
}

impl Device for KernelPacketDevice {
    type RxToken<'a>
        = KernelRxToken<'a>
    where
        Self: 'a;
    type TxToken<'a>
        = KernelTxToken<'a>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        match rt::packet_interface_receive_nonblocking(self.handle, &mut self.rx_buffer) {
            Ok(length) => Some((
                KernelRxToken {
                    buffer: &mut self.rx_buffer[..length],
                },
                KernelTxToken {
                    handle: self.handle,
                    buffer: &mut self.tx_buffer,
                },
            )),
            Err(rt::Error::QueueEmpty) => None,
            Err(_) => None,
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(KernelTxToken {
            handle: self.handle,
            buffer: &mut self.tx_buffer,
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = self.info.mtu as usize;
        caps.max_burst_size = Some(1);
        caps
    }
}

impl RxToken for KernelRxToken<'_> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(self.buffer)
    }
}

impl TxToken for KernelTxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let result = f(&mut self.buffer[..len]);
        let _ = rt::packet_interface_transmit(self.handle, &self.buffer[..len]);
        result
    }
}

#[derive(Clone, Copy)]
struct TcpTransportSlot {
    active: bool,
    control_handle: rt::Handle,
    socket_handle: Option<SocketHandle>,
    state: NetworkSocketState,
    remote_address: Ipv4Address,
    remote_port: u16,
    local_port: u16,
    rx_bytes: u64,
    tx_bytes: u64,
    opened_at_ticks: u64,
    last_activity_ticks: u64,
}

impl TcpTransportSlot {
    const fn empty() -> Self {
        Self {
            active: false,
            control_handle: rt::INVALID_HANDLE,
            socket_handle: None,
            state: NetworkSocketState::Closed,
            remote_address: Ipv4Address::UNSPECIFIED,
            remote_port: 0,
            local_port: 0,
            rx_bytes: 0,
            tx_bytes: 0,
            opened_at_ticks: 0,
            last_activity_ticks: 0,
        }
    }
}

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xfb01;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 2 || startup.word_count < 4
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
    let initial_dns_servers = dns_server_list(runtime_state.dns_server);
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
    let mut transports = [TcpTransportSlot::empty(); MAX_TCP_SOCKETS];
    let mut next_sequence = 1u16;
    let mut next_local_port = EPHEMERAL_PORT_BASE;
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
            ipv4_to_u32(runtime_state.address) as u64,
            ipv4_to_u32(runtime_state.gateway) as u64,
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
            match rt::channel_receive_nonblocking(transports[index].control_handle, &mut socket_request) {
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
                    close_transport_slot(log_handle, &mut sockets, &mut transports[index]);
                }
            }
        }

        if rt::yield_current().is_err() {
            return 0xfb0e;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_public_request(
    request: &RawMessage,
    packet_handle: rt::Handle,
    log_handle: rt::Handle,
    config: NetworkConfig,
    runtime_state: InterfaceRuntimeState,
    hosts: &[HostEntry],
    iface: &mut Interface,
    device: &mut KernelPacketDevice,
    sockets: &mut SocketSet<'_>,
    dns_handle: SocketHandle,
    icmp_handle: SocketHandle,
    next_sequence: &mut u16,
    transports: &mut [TcpTransportSlot; MAX_TCP_SOCKETS],
    tcp_handles: [SocketHandle; MAX_TCP_SOCKETS],
    next_local_port: &mut u16,
) -> rt::Result<()> {
    match request.tag {
        x if x == NetworkTag::InterfaceListRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(NetworkTag::InterfaceListReply as u32);
            reply.word_count = 2;
            reply.words[0] = NetworkStatus::Ok as u32 as u64;
            reply.words[1] = 1;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == NetworkTag::InterfaceStatusRequest as u32 => {
            if request.word_count < 1 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let index = request.words[0] as usize;
            let mut reply = RawMessage::empty(NetworkTag::InterfaceStatusReply as u32);
            reply.word_count = 15;
            if index != 0 {
                reply.words[0] = NetworkStatus::NotFound as u32 as u64;
            } else {
                let info = rt::packet_interface_info(packet_handle)?;
                reply.words[0] = NetworkStatus::Ok as u32 as u64;
                reply.words[1] = 0;
                reply.words[2] = info.backend as u64;
                reply.words[3] = info.link_state as u64;
                reply.words[4] = info.mtu as u64;
                reply.words[5] = runtime_state.mode as u32 as u64;
                reply.words[6] = runtime_state.state as u32 as u64;
                reply.words[7] = ipv4_to_u32(runtime_state.address) as u64;
                reply.words[8] = runtime_state.prefix_len as u64;
                reply.words[9] = ipv4_to_u32(runtime_state.gateway) as u64;
                reply.words[10] = ipv4_to_u32(runtime_state.dns_server) as u64;
                reply.words[11] = pack_mac(info.mac);
                reply.words[12] = info.rx_packets;
                reply.words[13] = info.tx_packets;
                reply.words[14] = info.dropped_packets;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == NetworkTag::ResolveRequest as u32 => {
            if request.word_count < 1 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut text = [0u8; MAX_HOSTNAME_BYTES];
            let target = decode_inline_text(
                &request.words[1..request.word_count as usize],
                request.words[0] as usize,
                &mut text,
            )?;
            let mut reply = RawMessage::empty(NetworkTag::ResolveReply as u32);
            reply.word_count = 2;
            if runtime_state.state == NetworkConfigState::Pending && parse_ipv4(target).is_none() {
                reply.words[0] = NetworkStatus::Busy as u32 as u64;
                reply.words[1] = 0;
            } else {
                match resolve_target(
                    target,
                    hosts,
                    config.dns_query_timeout_ticks,
                    iface,
                    device,
                    sockets,
                    dns_handle,
                )? {
                    Some(address) => {
                        reply.words[0] = NetworkStatus::Ok as u32 as u64;
                        reply.words[1] = 1;
                        reply.words[2] = ipv4_to_u32(address) as u64;
                        reply.word_count = 3;
                        let _ = emit_log(
                            log_handle,
                            LogSeverity::Debug,
                            LogEvent::NetworkResolveCompleted,
                            ipv4_to_u32(address) as u64,
                            1,
                        );
                    }
                    None => {
                        reply.words[0] = NetworkStatus::NotFound as u32 as u64;
                        reply.words[1] = 0;
                    }
                }
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == NetworkTag::PingRequest as u32 => {
            if request.word_count < 1 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut text = [0u8; MAX_HOSTNAME_BYTES];
            let target = decode_inline_text(
                &request.words[1..request.word_count as usize],
                request.words[0] as usize,
                &mut text,
            )?;
            let mut reply = RawMessage::empty(NetworkTag::PingReply as u32);
            reply.word_count = 3;

            if runtime_state.address == Ipv4Address::UNSPECIFIED {
                reply.words[0] = NetworkStatus::Busy as u32 as u64;
                reply.words[1] = 0;
                reply.words[2] = 0;
            } else {
                match resolve_target(
                    target,
                    hosts,
                    config.dns_query_timeout_ticks,
                    iface,
                    device,
                    sockets,
                    dns_handle,
                )? {
                    Some(address) => match perform_ping(
                        iface,
                        device,
                        sockets,
                        icmp_handle,
                        address,
                        config.probe_timeout_ticks,
                        next_sequence,
                    )? {
                        Some(elapsed_ms) => {
                            reply.words[0] = NetworkStatus::Ok as u32 as u64;
                            reply.words[1] = ipv4_to_u32(address) as u64;
                            reply.words[2] = elapsed_ms;
                            let _ = emit_log(
                                log_handle,
                                LogSeverity::Info,
                                LogEvent::NetworkProbeCompleted,
                                ipv4_to_u32(address) as u64,
                                elapsed_ms,
                            );
                        }
                        None => {
                            reply.words[0] = NetworkStatus::Timeout as u32 as u64;
                            reply.words[1] = ipv4_to_u32(address) as u64;
                            reply.words[2] = 0;
                        }
                    },
                    None => {
                        reply.words[0] = NetworkStatus::NotFound as u32 as u64;
                        reply.words[1] = 0;
                        reply.words[2] = 0;
                    }
                }
            }

            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == NetworkTag::SocketOpenRequest as u32 => {
            if request.word_count < 2 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let kind = match request.words[0] as u32 {
                x if x == NetworkSocketKind::TcpStream as u32 => NetworkSocketKind::TcpStream,
                _ => NetworkSocketKind::TcpStream,
            };
            let packed = request.words[1];
            let target_len = (packed >> 16) as usize;
            let remote_port = packed as u16;
            let mut text = [0u8; MAX_HOSTNAME_BYTES];
            let target = decode_inline_text(
                &request.words[2..request.word_count as usize],
                target_len,
                &mut text,
            )?;
            let mut reply = RawMessage::empty(NetworkTag::SocketOpenReply as u32);
            reply.word_count = 1;

            if runtime_state.address == Ipv4Address::UNSPECIFIED || remote_port == 0 {
                reply.words[0] = NetworkStatus::Busy as u32 as u64;
            } else if kind != NetworkSocketKind::TcpStream {
                reply.words[0] = NetworkStatus::Unsupported as u32 as u64;
            } else if let Some(remote_address) = resolve_target(
                target,
                hosts,
                config.dns_query_timeout_ticks,
                iface,
                device,
                sockets,
                dns_handle,
            )? {
                if let Some(slot_index) = allocate_transport_slot(transports) {
                    let session = rt::channel_create()?;
                    let local_port = allocate_ephemeral_port(next_local_port);
                    let socket_handle = tcp_handles[slot_index];
                    let connected = {
                        let socket = sockets.get_mut::<tcp::Socket>(socket_handle);
                        if socket.is_open() {
                            socket.abort();
                        }
                        socket
                            .connect(
                                iface.context(),
                                (IpAddress::Ipv4(remote_address), remote_port),
                                local_port,
                            )
                            .is_ok()
                    };
                    if connected {
                        transports[slot_index] = TcpTransportSlot {
                            active: true,
                            control_handle: session.first,
                            socket_handle: Some(socket_handle),
                            state: NetworkSocketState::Connecting,
                            remote_address,
                            remote_port,
                            local_port,
                            rx_bytes: 0,
                            tx_bytes: 0,
                            opened_at_ticks: rt::monotonic_now()?,
                            last_activity_ticks: rt::monotonic_now()?,
                        };
                        reply.words[0] = NetworkStatus::Ok as u32 as u64;
                        reply.handle_count = 1;
                        reply.handles[0] = session.second;
                        reply.handle_rights[0] = rt::rights::SEND | rt::rights::RECEIVE;
                        let _ = emit_log(
                            log_handle,
                            LogSeverity::Info,
                            LogEvent::NetworkSocketOpened,
                            ipv4_to_u32(remote_address) as u64,
                            remote_port as u64,
                        );
                        let _ = rt::channel_send(reply_handle, &reply);
                        let _ = rt::handle_close(session.second);
                        let _ = rt::handle_close(reply_handle);
                        return Ok(());
                    }
                    let _ = rt::handle_close(session.first);
                    let _ = rt::handle_close(session.second);
                    reply.words[0] = NetworkStatus::Busy as u32 as u64;
                } else {
                    reply.words[0] = NetworkStatus::CapacityExceeded as u32 as u64;
                }
            } else {
                reply.words[0] = NetworkStatus::NotFound as u32 as u64;
            }

            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == NetworkTag::SocketListRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(NetworkTag::SocketListReply as u32);
            reply.words[0] = NetworkStatus::Ok as u32 as u64;
            let mut count = 0usize;
            for slot in transports.iter().filter(|slot| slot.active) {
                if 2 + (count + 1) * 7 > rt::IPC_MAX_WORDS {
                    break;
                }
                let base = 2 + count * 7;
                reply.words[base] = count as u64;
                reply.words[base + 1] = NetworkSocketKind::TcpStream as u32 as u64;
                reply.words[base + 2] = slot.state as u32 as u64;
                reply.words[base + 3] = ipv4_to_u32(slot.remote_address) as u64;
                reply.words[base + 4] = slot.remote_port as u64;
                reply.words[base + 5] = slot.local_port as u64;
                reply.words[base + 6] = ((slot.rx_bytes.min(u32::MAX as u64)) << 32)
                    | slot.tx_bytes.min(u32::MAX as u64);
                count += 1;
            }
            reply.word_count = 2 + count as u32 * 7;
            reply.words[1] = count as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        _ => {}
    }

    Ok(())
}

fn handle_socket_request(
    log_handle: rt::Handle,
    config: NetworkConfig,
    sockets: &mut SocketSet<'_>,
    slot: &mut TcpTransportSlot,
    request: &RawMessage,
) -> rt::Result<()> {
    if request.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = request.handles[0];
    let reply = match request.tag {
        x if x == NetworkSocketTag::StatusRequest as u32 => {
            let mut reply = RawMessage::empty(NetworkSocketTag::StatusReply as u32);
            reply.word_count = 8;
            reply.words[0] = NetworkStatus::Ok as u32 as u64;
            reply.words[1] = 0;
            reply.words[2] = NetworkSocketKind::TcpStream as u32 as u64;
            reply.words[3] = slot.state as u32 as u64;
            reply.words[4] = ipv4_to_u32(slot.remote_address) as u64;
            reply.words[5] = slot.remote_port as u64;
            reply.words[6] = slot.local_port as u64;
            reply.words[7] =
                ((slot.rx_bytes.min(u32::MAX as u64)) << 32) | slot.tx_bytes.min(u32::MAX as u64);
            reply
        }
        x if x == NetworkSocketTag::SendRequest as u32 => {
            let mut reply = RawMessage::empty(NetworkSocketTag::SendReply as u32);
            reply.word_count = 2;
            if request.word_count < 1 {
                reply.words[0] = NetworkStatus::InvalidTarget as u32 as u64;
            } else {
                let byte_len = request.words[0] as usize;
                let mut payload = [0u8; MAX_SOCKET_INLINE_BYTES];
                let payload = decode_inline_bytes(
                    &request.words[1..request.word_count as usize],
                    byte_len,
                    &mut payload,
                )?;
                if let Some(socket_handle) = slot.socket_handle {
                    let socket = sockets.get_mut::<tcp::Socket>(socket_handle);
                    if socket.may_send() && socket.can_send() {
                        match socket.send_slice(payload) {
                            Ok(written) => {
                                slot.last_activity_ticks =
                                    rt::monotonic_now().unwrap_or(slot.last_activity_ticks);
                                slot.tx_bytes = slot.tx_bytes.saturating_add(written as u64);
                                reply.words[0] = NetworkStatus::Ok as u32 as u64;
                                reply.words[1] = written as u64;
                            }
                            Err(_) => {
                                reply.words[0] = NetworkStatus::Busy as u32 as u64;
                                reply.words[1] = 0;
                            }
                        }
                    } else if socket.state() == tcp::State::Closed {
                        reply.words[0] = NetworkStatus::Closed as u32 as u64;
                        reply.words[1] = 0;
                    } else {
                        reply.words[0] = NetworkStatus::Busy as u32 as u64;
                        reply.words[1] = 0;
                    }
                } else {
                    reply.words[0] = NetworkStatus::Closed as u32 as u64;
                    reply.words[1] = 0;
                }
            }
            reply
        }
        x if x == NetworkSocketTag::ReceiveRequest as u32 => {
            let mut reply = RawMessage::empty(NetworkSocketTag::ReceiveReply as u32);
            reply.word_count = 2;
            let requested = request.words.get(0).copied().unwrap_or(0) as usize;
            let read_len = requested.min(MAX_SOCKET_INLINE_BYTES);
            let mut buffer = [0u8; MAX_SOCKET_INLINE_BYTES];
            if let Some(socket_handle) = slot.socket_handle {
                let socket = sockets.get_mut::<tcp::Socket>(socket_handle);
                if socket.can_recv() {
                    match socket.recv_slice(&mut buffer[..read_len]) {
                        Ok(count) => {
                            slot.last_activity_ticks =
                                rt::monotonic_now().unwrap_or(slot.last_activity_ticks);
                            slot.rx_bytes = slot.rx_bytes.saturating_add(count as u64);
                            reply.words[0] = NetworkStatus::Ok as u32 as u64;
                            reply.words[1] = count as u64;
                            let packed = pack_inline_bytes(&buffer[..count], &mut reply.words[2..])?;
                            reply.word_count = 2 + packed;
                        }
                        Err(_) => {
                            reply.words[0] = NetworkStatus::Busy as u32 as u64;
                            reply.words[1] = 0;
                        }
                    }
                } else if !socket.may_recv()
                    || rt::monotonic_now()
                        .unwrap_or(slot.last_activity_ticks)
                        .saturating_sub(slot.last_activity_ticks)
                        >= config.tcp_idle_timeout_ticks
                {
                    reply.words[0] = NetworkStatus::Closed as u32 as u64;
                    reply.words[1] = 0;
                } else {
                    reply.words[0] = NetworkStatus::Busy as u32 as u64;
                    reply.words[1] = 0;
                }
            } else {
                reply.words[0] = NetworkStatus::Closed as u32 as u64;
                reply.words[1] = 0;
            }
            reply
        }
        x if x == NetworkSocketTag::CloseRequest as u32 => {
            let mut reply = RawMessage::empty(NetworkSocketTag::CloseReply as u32);
            reply.word_count = 1;
            reply.words[0] = NetworkStatus::Ok as u32 as u64;
            close_transport_slot(log_handle, sockets, slot);
            reply
        }
        _ => return Ok(()),
    };

    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn drive_dynamic_ipv4(
    config: &NetworkConfig,
    log_handle: rt::Handle,
    runtime_state: &mut InterfaceRuntimeState,
    dhcp_started_at: &mut u64,
    iface: &mut Interface,
    sockets: &mut SocketSet<'_>,
    dhcp_handle: SocketHandle,
    dns_handle: SocketHandle,
) -> rt::Result<()> {
    if let Some(event) = sockets.get_mut::<dhcpv4::Socket>(dhcp_handle).poll() {
        match event {
            dhcpv4::Event::Configured(configured) => {
                *runtime_state = InterfaceRuntimeState {
                    mode: NetworkConfigMode::Dynamic,
                    state: NetworkConfigState::Configured,
                    address: configured.address.address(),
                    prefix_len: configured.address.prefix_len(),
                    gateway: configured.router.unwrap_or(Ipv4Address::UNSPECIFIED),
                    dns_server: configured
                        .dns_servers
                        .first()
                        .copied()
                        .unwrap_or(config.dns_server),
                };
                *dhcp_started_at = rt::monotonic_now().unwrap_or(*dhcp_started_at);
                apply_interface_runtime(iface, *runtime_state);
                update_dns_servers(
                    sockets.get_mut::<dns::Socket>(dns_handle),
                    runtime_state.dns_server,
                );
                let _ = emit_log(
                    log_handle,
                    LogSeverity::Info,
                    LogEvent::NetworkLeaseChanged,
                    ipv4_to_u32(runtime_state.address) as u64,
                    ipv4_to_u32(runtime_state.gateway) as u64,
                );
                let _ = emit_log(
                    log_handle,
                    LogSeverity::Info,
                    LogEvent::NetworkAddressConfigured,
                    ipv4_to_u32(runtime_state.address) as u64,
                    ipv4_to_u32(runtime_state.gateway) as u64,
                );
            }
            dhcpv4::Event::Deconfigured => {
                *runtime_state = InterfaceRuntimeState::pending_dynamic();
                *dhcp_started_at = rt::monotonic_now().unwrap_or(*dhcp_started_at);
                apply_interface_runtime(iface, *runtime_state);
                update_dns_servers(
                    sockets.get_mut::<dns::Socket>(dns_handle),
                    runtime_state.dns_server,
                );
                let _ = emit_log(
                    log_handle,
                    LogSeverity::Warn,
                    LogEvent::NetworkLeaseChanged,
                    0,
                    0,
                );
            }
        }
    }

    if runtime_state.state == NetworkConfigState::Pending
        && rt::monotonic_now()?
            .saturating_sub(*dhcp_started_at)
            >= config.dhcp_acquire_timeout_ticks
    {
        *runtime_state = InterfaceRuntimeState::static_config(*config);
        apply_interface_runtime(iface, *runtime_state);
        update_dns_servers(
            sockets.get_mut::<dns::Socket>(dns_handle),
            runtime_state.dns_server,
        );
        let _ = emit_log(
            log_handle,
            LogSeverity::Warn,
            LogEvent::NetworkLeaseChanged,
            ipv4_to_u32(runtime_state.address) as u64,
            ipv4_to_u32(runtime_state.gateway) as u64,
        );
        let _ = emit_log(
            log_handle,
            LogSeverity::Warn,
            LogEvent::NetworkAddressConfigured,
            ipv4_to_u32(runtime_state.address) as u64,
            ipv4_to_u32(runtime_state.gateway) as u64,
        );
    }

    Ok(())
}

fn update_transport_states(
    log_handle: rt::Handle,
    config: NetworkConfig,
    sockets: &mut SocketSet<'_>,
    transports: &mut [TcpTransportSlot; MAX_TCP_SOCKETS],
) {
    for slot in transports.iter_mut().filter(|slot| slot.active) {
        let Some(socket_handle) = slot.socket_handle else {
            continue;
        };
        let now = rt::monotonic_now().unwrap_or(slot.last_activity_ticks);
        let socket = sockets.get_mut::<tcp::Socket>(socket_handle);
        let raw_state = socket.state();
        if raw_state == tcp::State::Established && (socket.can_send() || socket.can_recv()) {
            slot.last_activity_ticks = now;
        }
        let mut state = socket_network_state(raw_state);
        if state == NetworkSocketState::Connecting
            && now.saturating_sub(slot.opened_at_ticks) >= config.tcp_connect_timeout_ticks
        {
            socket.abort();
            state = NetworkSocketState::Failed;
        }
        if slot.state != state {
            slot.state = state;
            if state == NetworkSocketState::Closed {
                let _ = emit_log(
                    log_handle,
                    LogSeverity::Info,
                    LogEvent::NetworkSocketClosed,
                    ipv4_to_u32(slot.remote_address) as u64,
                    slot.remote_port as u64,
                );
            }
        }
    }
}

fn socket_network_state(state: tcp::State) -> NetworkSocketState {
    match state {
        tcp::State::SynSent | tcp::State::SynReceived => NetworkSocketState::Connecting,
        tcp::State::Established | tcp::State::CloseWait => NetworkSocketState::Established,
        tcp::State::FinWait1
        | tcp::State::FinWait2
        | tcp::State::Closing
        | tcp::State::LastAck
        | tcp::State::TimeWait => NetworkSocketState::Closing,
        tcp::State::Listen => NetworkSocketState::Connecting,
        tcp::State::Closed => NetworkSocketState::Closed,
    }
}

fn close_transport_slot(
    log_handle: rt::Handle,
    sockets: &mut SocketSet<'_>,
    slot: &mut TcpTransportSlot,
) {
    if let Some(socket_handle) = slot.socket_handle {
        sockets.get_mut::<tcp::Socket>(socket_handle).abort();
    }
    let _ = emit_log(
        log_handle,
        LogSeverity::Info,
        LogEvent::NetworkSocketClosed,
        ipv4_to_u32(slot.remote_address) as u64,
        slot.remote_port as u64,
    );
    if slot.control_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(slot.control_handle);
    }
    *slot = TcpTransportSlot::empty();
}

fn allocate_transport_slot(transports: &[TcpTransportSlot; MAX_TCP_SOCKETS]) -> Option<usize> {
    transports.iter().position(|slot| !slot.active)
}

fn allocate_ephemeral_port(next_local_port: &mut u16) -> u16 {
    let current = *next_local_port;
    *next_local_port = if *next_local_port >= u16::MAX - 1 {
        EPHEMERAL_PORT_BASE
    } else {
        next_local_port.saturating_add(1)
    };
    current
}

fn resolve_target(
    target: &str,
    hosts: &[HostEntry],
    timeout_ticks: u64,
    iface: &mut Interface,
    device: &mut KernelPacketDevice,
    sockets: &mut SocketSet<'_>,
    dns_handle: SocketHandle,
) -> rt::Result<Option<Ipv4Address>> {
    if let Some(address) = parse_ipv4(target) {
        return Ok(Some(address));
    }
    if let Some(address) = hosts
        .iter()
        .find(|entry| entry.name_len != 0 && entry.matches(target))
        .map(|entry| entry.address)
    {
        return Ok(Some(address));
    }

    let query = {
        let socket = sockets.get_mut::<dns::Socket>(dns_handle);
        match socket.start_query(iface.context(), target, DnsQueryType::A) {
            Ok(handle) => handle,
            Err(_) => return Ok(None),
        }
    };
    let start_ticks = rt::monotonic_now()?;
    loop {
        let _ = iface.poll(now_instant(), device, sockets);
        match sockets.get_mut::<dns::Socket>(dns_handle).get_query_result(query) {
            Ok(addresses) => {
                for address in addresses {
                    let IpAddress::Ipv4(ipv4) = address;
                    return Ok(Some(ipv4));
                }
                return Ok(None);
            }
            Err(dns::GetQueryResultError::Pending) => {}
            Err(_) => return Ok(None),
        }

        if rt::monotonic_now()?.saturating_sub(start_ticks) >= timeout_ticks {
            sockets.get_mut::<dns::Socket>(dns_handle).cancel_query(query);
            return Ok(None);
        }

        rt::yield_current()?;
    }
}

fn perform_ping(
    iface: &mut Interface,
    device: &mut KernelPacketDevice,
    sockets: &mut SocketSet<'_>,
    icmp_handle: SocketHandle,
    target: Ipv4Address,
    timeout_ticks: u64,
    next_sequence: &mut u16,
) -> rt::Result<Option<u64>> {
    let start_ticks = rt::monotonic_now()?;
    let start_ms = ticks_to_millis(start_ticks);
    let checksum = device.capabilities().checksum;

    {
        let socket = sockets.get_mut::<icmp::Socket>(icmp_handle);
        if !socket.is_open() {
            let _ = socket.bind(icmp::Endpoint::Ident(PING_IDENTIFIER));
        }
        if !socket.can_send() {
            return Ok(None);
        }

        let payload = [0x53, 0x4f, (*next_sequence >> 8) as u8, *next_sequence as u8];
        let icmp_repr = Icmpv4Repr::EchoRequest {
            ident: PING_IDENTIFIER,
            seq_no: *next_sequence,
            data: &payload,
        };
        let packet = socket
            .send(icmp_repr.buffer_len(), IpAddress::Ipv4(target))
            .map_err(|_| rt::Error::Busy)?;
        icmp_repr.emit(&mut Icmpv4Packet::new_unchecked(packet), &checksum);
    }

    let sequence = *next_sequence;
    *next_sequence = next_sequence.wrapping_add(1);

    loop {
        let _ = iface.poll(now_instant(), device, sockets);
        let socket = sockets.get_mut::<icmp::Socket>(icmp_handle);
        if socket.can_recv() {
            let (payload, _) = socket.recv().map_err(|_| rt::Error::Busy)?;
            let packet =
                Icmpv4Packet::new_checked(&payload).map_err(|_| rt::Error::InvalidArgument)?;
            let reply =
                Icmpv4Repr::parse(&packet, &checksum).map_err(|_| rt::Error::InvalidArgument)?;
            if let Icmpv4Repr::EchoReply {
                ident,
                seq_no,
                data: _,
            } = reply
            {
                if ident == PING_IDENTIFIER && seq_no == sequence {
                    let elapsed_ms =
                        ticks_to_millis(rt::monotonic_now()?).saturating_sub(start_ms);
                    return Ok(Some(elapsed_ms));
                }
            }
        }

        if rt::monotonic_now()?.saturating_sub(start_ticks) >= timeout_ticks {
            return Ok(None);
        }

        rt::yield_current()?;
    }
}

fn apply_interface_runtime(iface: &mut Interface, runtime_state: InterfaceRuntimeState) {
    iface.update_ip_addrs(|addrs| {
        addrs.clear();
        if runtime_state.address != Ipv4Address::UNSPECIFIED && runtime_state.prefix_len != 0 {
            let _ = addrs.push(IpCidr::Ipv4(Ipv4Cidr::new(
                runtime_state.address,
                runtime_state.prefix_len,
            )));
        }
    });
    let _ = iface.routes_mut().remove_default_ipv4_route();
    if runtime_state.gateway != Ipv4Address::UNSPECIFIED {
        let _ = iface.routes_mut().add_default_ipv4_route(runtime_state.gateway);
    }
}

fn update_dns_servers(socket: &mut dns::Socket<'_>, server: Ipv4Address) {
    let servers = dns_server_list(server);
    socket.update_servers(&servers);
}

fn dns_server_list(server: Ipv4Address) -> [IpAddress; 1] {
    [IpAddress::Ipv4(server)]
}

fn read_network_config(config_handle: rt::Handle) -> rt::Result<NetworkConfig> {
    Ok(NetworkConfig {
        static_address: u32_to_ipv4(read_config_value(config_handle, ConfigKey::NetworkIpv4Address, 0)? as u32),
        static_prefix_len: read_config_value(config_handle, ConfigKey::NetworkIpv4PrefixLength, 24)? as u8,
        static_gateway: u32_to_ipv4(read_config_value(config_handle, ConfigKey::NetworkIpv4Gateway, 0)? as u32),
        dynamic_ipv4: read_config_value(config_handle, ConfigKey::NetworkDynamicIpv4, 0)? != 0,
        dns_server: u32_to_ipv4(read_config_value(config_handle, ConfigKey::NetworkDnsServer, 0)? as u32),
        probe_timeout_ticks: read_config_value(config_handle, ConfigKey::NetworkProbeTimeoutTicks, 300)?,
        dns_query_timeout_ticks: read_config_value(config_handle, ConfigKey::NetworkDnsQueryTimeoutTicks, 400)?,
        dhcp_acquire_timeout_ticks: read_config_value(config_handle, ConfigKey::NetworkDhcpAcquireTimeoutTicks, 600)?,
        tcp_connect_timeout_ticks: read_config_value(config_handle, ConfigKey::NetworkTcpConnectTimeoutTicks, 600)?,
        tcp_idle_timeout_ticks: read_config_value(config_handle, ConfigKey::NetworkTcpIdleTimeoutTicks, 300)?,
    })
}

fn read_config_value(handle: rt::Handle, key: ConfigKey, default: u64) -> rt::Result<u64> {
    match rt::config_read(handle, key) {
        Ok((_, value)) => Ok(value),
        Err(rt::Error::InvalidArgument) => Ok(default),
        Err(error) => Err(error),
    }
}

fn load_hosts(handle: rt::Handle, hosts: &mut [HostEntry; MAX_HOSTS]) -> rt::Result<usize> {
    if handle == rt::INVALID_HANDLE {
        return Ok(0);
    }

    let mut buffer = [0u8; MAX_HOSTS_RESOURCE_BYTES];
    let expected_len = buffer.len();
    let loaded = rt::storage_read_all(handle, &mut buffer, expected_len).unwrap_or(0);
    let _ = rt::storage_blob_close(handle);
    let text = core::str::from_utf8(&buffer[..loaded]).map_err(|_| rt::Error::InvalidArgument)?;
    let mut count = 0usize;

    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if count == hosts.len() {
            break;
        }
        let Some(address) = parse_ipv4(value.trim()) else {
            continue;
        };
        let name = name.trim().as_bytes();
        if name.len() > MAX_HOSTNAME_BYTES {
            continue;
        }
        hosts[count].name_len = name.len();
        hosts[count].name[..name.len()].copy_from_slice(name);
        hosts[count].address = address;
        count += 1;
    }

    Ok(count)
}

fn parse_ipv4(value: &str) -> Option<Ipv4Address> {
    let mut octets = [0u8; 4];
    let mut count = 0usize;
    for part in value.split('.') {
        if count == 4 {
            return None;
        }
        let byte = part.parse::<u8>().ok()?;
        octets[count] = byte;
        count += 1;
    }
    if count == 4 {
        Some(Ipv4Address::new(
            octets[0], octets[1], octets[2], octets[3],
        ))
    } else {
        None
    }
}

fn decode_inline_text<'a>(
    words: &[u64],
    length: usize,
    buffer: &'a mut [u8],
) -> rt::Result<&'a str> {
    decode_inline_bytes(words, length, buffer).and_then(|bytes| {
        core::str::from_utf8(bytes).map_err(|_| rt::Error::InvalidArgument)
    })
}

fn decode_inline_bytes<'a>(
    words: &[u64],
    length: usize,
    buffer: &'a mut [u8],
) -> rt::Result<&'a [u8]> {
    if length > buffer.len() || length > words.len() * 8 {
        return Err(rt::Error::BufferTooSmall);
    }
    unpack_inline_bytes(words, length, buffer)?;
    Ok(&buffer[..length])
}

fn pack_inline_bytes(source: &[u8], words: &mut [u64]) -> rt::Result<u32> {
    let required_words = source.len().div_ceil(8);
    if required_words > words.len() {
        return Err(rt::Error::BufferTooSmall);
    }
    for (index, chunk) in source.chunks(8).enumerate() {
        let mut bytes = [0u8; 8];
        bytes[..chunk.len()].copy_from_slice(chunk);
        words[index] = u64::from_le_bytes(bytes);
    }
    Ok(required_words as u32)
}

fn unpack_inline_bytes(words: &[u64], length: usize, destination: &mut [u8]) -> rt::Result<()> {
    if length > destination.len() || length > words.len() * 8 {
        return Err(rt::Error::BufferTooSmall);
    }

    let mut copied = 0usize;
    for word in words.iter().copied() {
        if copied >= length {
            break;
        }
        let bytes = word.to_le_bytes();
        let chunk = (length - copied).min(bytes.len());
        destination[copied..copied + chunk].copy_from_slice(&bytes[..chunk]);
        copied += chunk;
    }
    Ok(())
}

fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
    let mut message = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut message) {
        Ok(()) if message.tag == ControlTag::Lifecycle as u32 && message.word_count > 0 => {
            Ok(matches!(
                lifecycle_event_from_word(message.words[0]),
                LifecycleEvent::Restarting | LifecycleEvent::Stopped
            ))
        }
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
        ServiceId::Network,
        severity,
        rt::LogDomain::Network,
        event,
        arg0,
        arg1,
    )
}

fn pack_mac(mac: [u8; 6]) -> u64 {
    (mac[0] as u64)
        | ((mac[1] as u64) << 8)
        | ((mac[2] as u64) << 16)
        | ((mac[3] as u64) << 24)
        | ((mac[4] as u64) << 32)
        | ((mac[5] as u64) << 40)
}

fn ipv4_to_u32(address: Ipv4Address) -> u32 {
    let [a, b, c, d] = address.octets();
    u32::from_be_bytes([a, b, c, d])
}

fn u32_to_ipv4(value: u32) -> Ipv4Address {
    let [a, b, c, d] = value.to_be_bytes();
    Ipv4Address::new(a, b, c, d)
}

fn now_instant() -> Instant {
    Instant::from_millis(ticks_to_millis(rt::monotonic_now().unwrap_or(0)) as i64)
}

fn ticks_to_millis(ticks: u64) -> u64 {
    ticks.saturating_mul(10)
}

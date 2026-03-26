#![no_std]
#![no_main]

use smoltcp::{
    iface::{Config as IfaceConfig, Interface, SocketSet, SocketStorage},
    phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken},
    socket::icmp,
    time::Instant,
    wire::{
        EthernetAddress, HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr,
        Ipv4Address,
    },
};

use serviceos_userspace_runtime as rt;
use rt::{
    ConfigKey, ControlTag, LifecycleEvent, LogDomain, LogEvent, LogSeverity, NetworkStatus,
    NetworkTag, PacketInterfaceInfo, RawMessage, ServiceId,
};

const MAX_HOSTS: usize = 8;
const MAX_HOSTNAME_BYTES: usize = 48;
const MAX_HOSTS_RESOURCE_BYTES: usize = 256;
const MAX_FRAME_BYTES: usize = 1536;
const PING_IDENTIFIER: u16 = 0x534f;

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
    address: Ipv4Address,
    prefix_len: u8,
    gateway: Ipv4Address,
    probe_timeout_ticks: u64,
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
            let _ = rt::write_logf("network", format_args!("packet-interface-info failed: {:?}", error));
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
    iface.update_ip_addrs(|addrs| {
        let _ = addrs.push(IpCidr::new(IpAddress::Ipv4(config.address), config.prefix_len));
    });
    let _ = iface.routes_mut().add_default_ipv4_route(config.gateway);

    let mut socket_storage = [SocketStorage::EMPTY];
    let mut icmp_rx_meta = [icmp::PacketMetadata::EMPTY];
    let mut icmp_tx_meta = [icmp::PacketMetadata::EMPTY];
    let mut icmp_rx_data = [0u8; 256];
    let mut icmp_tx_data = [0u8; 256];
    let icmp_socket = icmp::Socket::new(
        icmp::PacketBuffer::new(&mut icmp_rx_meta[..], &mut icmp_rx_data[..]),
        icmp::PacketBuffer::new(&mut icmp_tx_meta[..], &mut icmp_tx_data[..]),
    );
    let mut sockets = SocketSet::new(&mut socket_storage[..]);
    let icmp_handle = sockets.add(icmp_socket);
    let mut next_sequence = 1u16;

    let _ = emit_log(
        log_handle,
        LogSeverity::Info,
        LogEvent::NetworkInterfaceReady,
        0,
        pack_mac(device.info.mac),
    );
    let _ = emit_log(
        log_handle,
        LogSeverity::Info,
        LogEvent::NetworkAddressConfigured,
        ipv4_to_u32(config.address) as u64,
        ipv4_to_u32(config.gateway) as u64,
    );

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xfb09,
        }

        let _ = iface.poll(now_instant(), &mut device, &mut sockets);

        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                if handle_request(
                    &request,
                    packet_handle,
                    log_handle,
                    config,
                    &hosts[..host_count],
                    &mut iface,
                    &mut device,
                    &mut sockets,
                    icmp_handle,
                    &mut next_sequence,
                )
                .is_err()
                {
                    return 0xfb0a;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xfb0b,
        }

        if rt::yield_current().is_err() {
            return 0xfb0c;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_request(
    request: &RawMessage,
    packet_handle: rt::Handle,
    log_handle: rt::Handle,
    config: NetworkConfig,
    hosts: &[HostEntry],
    iface: &mut Interface,
    device: &mut KernelPacketDevice,
    sockets: &mut SocketSet<'_>,
    icmp_handle: smoltcp::iface::SocketHandle,
    next_sequence: &mut u16,
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
            reply.word_count = 12;
            if index != 0 {
                reply.words[0] = NetworkStatus::NotFound as u32 as u64;
            } else {
                let info = rt::packet_interface_info(packet_handle)?;
                reply.words[0] = NetworkStatus::Ok as u32 as u64;
                reply.words[1] = 0;
                reply.words[2] = info.backend as u64;
                reply.words[3] = info.link_state as u64;
                reply.words[4] = 1500;
                reply.words[5] = ipv4_to_u32(config.address) as u64;
                reply.words[6] = config.prefix_len as u64;
                reply.words[7] = ipv4_to_u32(config.gateway) as u64;
                reply.words[8] = pack_mac(info.mac);
                reply.words[9] = info.rx_packets;
                reply.words[10] = info.tx_packets;
                reply.words[11] = info.dropped_packets;
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
            let resolved = resolve_target(target, hosts);
            let mut reply = RawMessage::empty(NetworkTag::ResolveReply as u32);
            reply.word_count = 2;
            match resolved {
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
            let resolved = resolve_target(target, hosts);
            let mut reply = RawMessage::empty(NetworkTag::PingReply as u32);
            reply.word_count = 3;

            match resolved {
                Some(address) => {
                    match perform_ping(
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
                    }
                }
                None => {
                    reply.words[0] = NetworkStatus::NotFound as u32 as u64;
                    reply.words[1] = 0;
                    reply.words[2] = 0;
                }
            }

            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        _ => {}
    }

    Ok(())
}

fn perform_ping(
    iface: &mut Interface,
    device: &mut KernelPacketDevice,
    sockets: &mut SocketSet<'_>,
    icmp_handle: smoltcp::iface::SocketHandle,
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
            let packet = Icmpv4Packet::new_checked(&payload).map_err(|_| rt::Error::InvalidArgument)?;
            let reply = Icmpv4Repr::parse(&packet, &checksum).map_err(|_| rt::Error::InvalidArgument)?;
            if let Icmpv4Repr::EchoReply {
                ident,
                seq_no,
                data: _,
            } = reply
            {
                if ident == PING_IDENTIFIER && seq_no == sequence {
                    let elapsed_ms = ticks_to_millis(rt::monotonic_now()?).saturating_sub(start_ms);
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

fn read_network_config(config_handle: rt::Handle) -> rt::Result<NetworkConfig> {
    let (_, address) = rt::config_read(config_handle, ConfigKey::NetworkIpv4Address)?;
    let (_, prefix_len) = rt::config_read(config_handle, ConfigKey::NetworkIpv4PrefixLength)?;
    let (_, gateway) = rt::config_read(config_handle, ConfigKey::NetworkIpv4Gateway)?;
    let (_, timeout) = rt::config_read(config_handle, ConfigKey::NetworkProbeTimeoutTicks)?;
    Ok(NetworkConfig {
        address: u32_to_ipv4(address as u32),
        prefix_len: prefix_len as u8,
        gateway: u32_to_ipv4(gateway as u32),
        probe_timeout_ticks: timeout,
    })
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

fn resolve_target(target: &str, hosts: &[HostEntry]) -> Option<Ipv4Address> {
    parse_ipv4(target).or_else(|| {
        hosts
            .iter()
            .find(|entry| entry.name_len != 0 && entry.matches(target))
            .map(|entry| entry.address)
    })
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
    unpack_bytes(words, length, buffer)?;
    core::str::from_utf8(&buffer[..length]).map_err(|_| rt::Error::InvalidArgument)
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
        LogDomain::Network,
        event,
        arg0,
        arg1,
    )
}

fn ipv4_to_u32(address: Ipv4Address) -> u32 {
    let octets = address.octets();
    ((octets[0] as u32) << 24)
        | ((octets[1] as u32) << 16)
        | ((octets[2] as u32) << 8)
        | (octets[3] as u32)
}

fn u32_to_ipv4(value: u32) -> Ipv4Address {
    Ipv4Address::new(
        ((value >> 24) & 0xff) as u8,
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
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

fn now_instant() -> Instant {
    let ticks = rt::monotonic_now().unwrap_or(0);
    Instant::from_millis(ticks_to_millis(ticks) as i64)
}

fn ticks_to_millis(ticks: u64) -> u64 {
    ticks.saturating_mul(1000) / 100
}

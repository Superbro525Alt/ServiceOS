use smoltcp::{
    iface::{Interface, SocketSet, SocketStorage},
    socket::{icmp, tcp, udp},
    wire::IpAddress,
};

use serviceos_userspace_runtime as rt;

use crate::{
    consts::{
        LOOPBACK_ADDRESS, SELFTEST_POLL_LIMIT, SELFTEST_TCP_PORT, SELFTEST_UDP_PORT_A,
        SELFTEST_UDP_PORT_B,
    },
    device::KernelPacketDevice,
    util::now_instant,
};

const SELFTEST_BUFFER_BYTES: usize = 512;
const SELFTEST_LOCAL_TCP_PORT: u16 = 40_126;
const TCP_PAYLOAD: &[u8] = b"net-selftest-tcp-hello";
const TCP_REPLY: &[u8] = b"net-selftest-tcp-world";
const UDP_PAYLOAD: &[u8] = b"net-selftest-udp-ping";
const UDP_REPLY: &[u8] = b"net-selftest-udp-pong";

/// Structured-record discriminator shared with the status-service timeline
/// classifier: selftest records carry this tag in `arg0` so they stay distinct
/// from operator ping records on the same `NetworkProbeCompleted` event
/// (whose `arg0` is always an IPv4 address word below `0x1_0000_0000`).
pub(crate) const SELFTEST_RECORD_ARG0_TAG: u64 = 0x5345_4C46; // "SELF"

/// Phase codes carried in `arg1`; mirrors the status-service decoder.
pub(crate) mod selftest_phase {
    pub const BEGIN: u64 = 0;
    pub const PASSED: u64 = 1;
    pub const FAILED: u64 = 2;
}

/// Emits one selftest outcome into the shared log stream (same path the
/// developer/graphics domain feeds consume) with the UDP/TCP sub-outcomes
/// packed into `arg2`.
fn emit_phase_record(log_handle: rt::Handle, severity: rt::LogSeverity, phase: u64, detail: u64) {
    let _ = rt::send_log_record_ex(
        log_handle,
        rt::ServiceId::Network,
        severity,
        rt::LogDomain::Network,
        rt::LogEvent::NetworkProbeCompleted,
        SELFTEST_RECORD_ARG0_TAG,
        phase,
        detail,
    );
}

/// Guest-internal networking proof: drives one UDP datagram round-trip and
/// one full TCP listen/connect/accept/data/close sequence against 127.0.0.1
/// through the real stack and device (loopback frames never reach slirp).
/// Runs once per boot right after the interface address is configured; all
/// outcomes are logged as greppable `net-selftest` lines.
pub(crate) fn run(
    log_handle: rt::Handle,
    iface: &mut Interface,
    device: &mut KernelPacketDevice,
    sockets: &mut SocketSet<'_>,
    icmp_handle: smoltcp::iface::SocketHandle,
    gateway: smoltcp::wire::Ipv4Address,
) {
    let _ = rt::write_logf(
        "network",
        format_args!(
            "net-selftest begin udp={} tcp={}",
            SELFTEST_UDP_PORT_A, SELFTEST_TCP_PORT
        ),
    );
    emit_phase_record(
        log_handle,
        rt::LogSeverity::Debug,
        selftest_phase::BEGIN,
        ((SELFTEST_UDP_PORT_A as u64) << 16) | SELFTEST_TCP_PORT as u64,
    );

    let (mut sent, mut got, mut replied, mut echoed, mut estab) =
        (false, false, false, false, false);
    let udp_ok = udp_round_trip(
        iface,
        device,
        &mut sent,
        &mut got,
        &mut replied,
        &mut echoed,
    );
    let _ = rt::write_logf(
        "network",
        format_args!(
            "net-selftest udp {} sent={} got={} replied={} echoed={}",
            if udp_ok { "ok" } else { "failed" },
            sent,
            got,
            replied,
            echoed
        ),
    );

    let mut forwarded = false;
    let mut replied = false;
    let mut closed = false;
    let mut final_states = (0u8, 0u8);
    let tcp_ok = tcp_listen_accept_round_trip(
        iface,
        device,
        &mut estab,
        &mut forwarded,
        &mut replied,
        &mut closed,
        &mut final_states,
    );
    let (pushed, virtio, dropped) = crate::device::loopback_stats();
    let _ = rt::write_logf(
        "network",
        format_args!(
            "net-selftest tcp {} estab={} fwd={} rep={} clo={} cli_st={} srv_st={} lb_pushed={} virtio={} lb_dropped={}",
            if tcp_ok { "ok" } else { "failed" },
            estab,
            forwarded,
            replied,
            closed,
            final_states.0,
            final_states.1,
            pushed,
            virtio,
            dropped
        ),
    );

    // External probe: send one UDP datagram at the gateway's discard port.
    // Resolving the gateway forces an ARP exchange and slirp answers with
    // ICMP port-unreachable, so real inbound frames arrive through the
    // virtio -> kernel -> shared RX ring path (loopback frames above never
    // touch the kernel). Informational: environments without a gateway stay
    // quiet without failing the selftest.
    let ext_before = crate::device::rx_ring_snapshot().frames_pushed;
    let ext_pushed = external_probe(iface, device, gateway);
    let (driver_rx, driver_dropped) = match rt::packet_interface_info(device.handle) {
        Ok(info) => (info.rx_packets, info.dropped_packets),
        Err(_) => (u64::MAX, u64::MAX),
    };
    let _ = rt::write_logf(
        "network",
        format_args!(
            "net-selftest ext pushed={ext_pushed} total={} drv_rx={driver_rx} drv_drop={driver_dropped}",
            crate::device::rx_ring_snapshot().frames_pushed,
        ),
    );
    let _ = ext_before;

    let _ = rt::write_logf(
        "network",
        format_args!(
            "net-selftest end {}",
            if udp_ok && tcp_ok { "pass" } else { "fail" }
        ),
    );
    let passed = udp_ok && tcp_ok;
    emit_phase_record(
        log_handle,
        if passed {
            rt::LogSeverity::Info
        } else {
            rt::LogSeverity::Error
        },
        if passed {
            selftest_phase::PASSED
        } else {
            selftest_phase::FAILED
        },
        udp_ok as u64 | ((tcp_ok as u64) << 1),
    );

    // IPv6 v0 slice witnesses (build-time gated: images built without
    // SERVICEOS_E2E_NETWORK=1 never run these probes and emit nothing, so
    // default boot serial stays byte-identical).
    if crate::consts::ipv6_e2e_probe_enabled() {
        let (mut sent, mut got, mut echoed) = (false, false, false);
        let udp6_ok = udp_v6_round_trip(iface, device, &mut sent, &mut got, &mut echoed);
        let _ = rt::write_logf(
            "network",
            format_args!(
                "E2E net.ipv6-udp {} sent={} got={} echoed={}",
                if udp6_ok { "PASS" } else { "FAIL" },
                sent as u8,
                got as u8,
                echoed as u8
            ),
        );

        let mut requested = false;
        let mut replied = false;
        let mut sequence = 0xe600u16;
        let (ping6_ok, polls) = ping6_round_trip(
            iface,
            device,
            sockets,
            icmp_handle,
            &mut requested,
            &mut replied,
            &mut sequence,
        );
        let _ = rt::write_logf(
            "network",
            format_args!(
                "E2E net.ipv6-ping6 {} sent={} reply={} polls={}",
                if ping6_ok { "PASS" } else { "FAIL" },
                requested as u8,
                replied as u8,
                polls
            ),
        );
    }
}

/// One UDP datagram at `<gateway>:9` plus a bounded poll loop; returns how
/// many new frames the shared RX ring published while waiting.
fn external_probe(
    iface: &mut Interface,
    device: &mut KernelPacketDevice,
    gateway: smoltcp::wire::Ipv4Address,
) -> u64 {
    if gateway.is_unspecified() {
        return 0;
    }
    let target = IpAddress::Ipv4(gateway);
    let mut meta_r = [udp::PacketMetadata::EMPTY];
    let mut data_r = [0u8; SELFTEST_BUFFER_BYTES];
    let mut meta_t = [udp::PacketMetadata::EMPTY];
    let mut data_t = [0u8; SELFTEST_BUFFER_BYTES];
    let mut probe_storage = [SocketStorage::EMPTY; 1];
    let mut probe_sockets = SocketSet::new(&mut probe_storage[..]);
    let handle = probe_sockets.add(udp::Socket::new(
        udp::PacketBuffer::new(&mut meta_r[..], &mut data_r[..]),
        udp::PacketBuffer::new(&mut meta_t[..], &mut data_t[..]),
    ));
    if probe_sockets
        .get_mut::<udp::Socket>(handle)
        .bind(SELFTEST_UDP_PORT_B)
        .is_err()
    {
        return 0;
    }
    let payload = b"zero-copy-probe";
    let sent = probe_sockets
        .get_mut::<udp::Socket>(handle)
        .send_slice(payload, (target, 9))
        .is_ok();
    if !sent {
        return 0;
    }
    for _ in 0..600 {
        pump(iface, device, &mut probe_sockets);
        if crate::device::rx_ring_snapshot().frames_pushed > 0 {
            break;
        }
    }
    crate::device::rx_ring_snapshot().frames_pushed
}

fn pump<D: smoltcp::phy::Device>(
    iface: &mut Interface,
    device: &mut D,
    sockets: &mut SocketSet<'_>,
) {
    let result = iface.poll(now_instant(), device, sockets);
    let _ = result;
}

fn udp_round_trip(
    iface: &mut Interface,
    device: &mut KernelPacketDevice,
    sent_flag: &mut bool,
    got_flag: &mut bool,
    replied_flag: &mut bool,
    echoed_flag: &mut bool,
) -> bool {
    let mut a_meta_r = [udp::PacketMetadata::EMPTY; 2];
    let mut a_data_r = [0u8; SELFTEST_BUFFER_BYTES];
    let mut a_meta_t = [udp::PacketMetadata::EMPTY; 2];
    let mut a_data_t = [0u8; SELFTEST_BUFFER_BYTES];
    let mut b_meta_r = [udp::PacketMetadata::EMPTY; 2];
    let mut b_data_r = [0u8; SELFTEST_BUFFER_BYTES];
    let mut b_meta_t = [udp::PacketMetadata::EMPTY; 2];
    let mut b_data_t = [0u8; SELFTEST_BUFFER_BYTES];

    let mut test_storage = [SocketStorage::EMPTY; 4];
    let mut sockets = SocketSet::new(&mut test_storage[..]);
    let a = sockets.add(udp::Socket::new(
        udp::PacketBuffer::new(&mut a_meta_r[..], &mut a_data_r[..]),
        udp::PacketBuffer::new(&mut a_meta_t[..], &mut a_data_t[..]),
    ));
    let b = sockets.add(udp::Socket::new(
        udp::PacketBuffer::new(&mut b_meta_r[..], &mut b_data_r[..]),
        udp::PacketBuffer::new(&mut b_meta_t[..], &mut b_data_t[..]),
    ));

    let bound_a = sockets
        .get_mut::<udp::Socket>(a)
        // Bind the explicit loopback endpoint so the datagram's source
        // address is 127.0.0.1 (a port-only bind would let the stack pick
        // the primary interface address as source instead).
        .bind(smoltcp::wire::IpListenEndpoint::from((
            LOOPBACK_ADDRESS,
            SELFTEST_UDP_PORT_A,
        )))
        .is_ok();
    let bound_b = sockets
        .get_mut::<udp::Socket>(b)
        .bind(SELFTEST_UDP_PORT_B)
        .is_ok();
    if !bound_a || !bound_b {
        sockets.remove(a);
        sockets.remove(b);
        return false;
    }

    *sent_flag = true;
    let sent = sockets
        .get_mut::<udp::Socket>(a)
        .send_slice(
            UDP_PAYLOAD,
            smoltcp::wire::IpEndpoint {
                addr: IpAddress::Ipv4(LOOPBACK_ADDRESS),
                port: SELFTEST_UDP_PORT_B,
            },
        )
        .is_ok();

    let mut received = None;
    if sent {
        for _ in 0..SELFTEST_POLL_LIMIT {
            pump(iface, device, &mut sockets);
            let mut buffer = [0u8; SELFTEST_BUFFER_BYTES];
            if let Ok((count, meta)) = sockets.get_mut::<udp::Socket>(b).recv_slice(&mut buffer) {
                if count == UDP_PAYLOAD.len()
                    && &buffer[..count] == UDP_PAYLOAD
                    && meta.endpoint.addr == IpAddress::Ipv4(LOOPBACK_ADDRESS)
                    && meta.endpoint.port == SELFTEST_UDP_PORT_A
                {
                    received = Some(meta.endpoint);
                    *got_flag = true;
                }
                break;
            }
        }
    }

    let replied = received.is_some();
    let mut echoed = false;
    if let Some(endpoint) = received {
        let sent_back = sockets
            .get_mut::<udp::Socket>(b)
            .send_slice(UDP_REPLY, endpoint)
            .is_ok();
        *replied_flag = true;
        if sent_back {
            for _ in 0..SELFTEST_POLL_LIMIT {
                pump(iface, device, &mut sockets);
                let mut buffer = [0u8; SELFTEST_BUFFER_BYTES];
                match sockets.get_mut::<udp::Socket>(a).recv_slice(&mut buffer) {
                    Ok((count, _)) => {
                        echoed = count == UDP_REPLY.len() && &buffer[..count] == UDP_REPLY;
                        *echoed_flag = true;
                        break;
                    }
                    Err(_) => {}
                }
            }
        }
    }

    sockets.remove(a);
    sockets.remove(b);
    replied && echoed
}

#[allow(clippy::too_many_arguments)]
fn tcp_listen_accept_round_trip(
    iface: &mut Interface,
    device: &mut KernelPacketDevice,
    estab_flag: &mut bool,
    forwarded_flag: &mut bool,
    replied_flag: &mut bool,
    closed_flag: &mut bool,
    final_states: &mut (u8, u8),
) -> bool {
    let mut server_rx = [0u8; SELFTEST_BUFFER_BYTES];
    let mut server_tx = [0u8; SELFTEST_BUFFER_BYTES];
    let mut client_rx = [0u8; SELFTEST_BUFFER_BYTES];
    let mut client_tx = [0u8; SELFTEST_BUFFER_BYTES];

    let mut test_storage = [SocketStorage::EMPTY; 4];
    let mut sockets = SocketSet::new(&mut test_storage[..]);
    let server = sockets.add(tcp::Socket::new(
        tcp::SocketBuffer::new(&mut server_rx[..]),
        tcp::SocketBuffer::new(&mut server_tx[..]),
    ));
    let client = sockets.add(tcp::Socket::new(
        tcp::SocketBuffer::new(&mut client_rx[..]),
        tcp::SocketBuffer::new(&mut client_tx[..]),
    ));

    let listening = sockets
        .get_mut::<tcp::Socket>(server)
        .listen(SELFTEST_TCP_PORT)
        .is_ok();
    let connecting = sockets
        .get_mut::<tcp::Socket>(client)
        .connect(
            iface.context(),
            (IpAddress::Ipv4(LOOPBACK_ADDRESS), SELFTEST_TCP_PORT),
            SELFTEST_LOCAL_TCP_PORT,
        )
        .is_ok();

    let mut established = false;
    if listening && connecting {
        for _ in 0..SELFTEST_POLL_LIMIT {
            pump(iface, device, &mut sockets);
            let client_ready =
                sockets.get_mut::<tcp::Socket>(client).state() == tcp::State::Established;
            let server_ready =
                sockets.get_mut::<tcp::Socket>(server).state() == tcp::State::Established;
            if client_ready && server_ready {
                established = true;
                *estab_flag = true;
                break;
            }
        }
    }

    // Client -> server data.
    let mut forwarded = false;
    if established {
        let sent = sockets
            .get_mut::<tcp::Socket>(client)
            .send_slice(TCP_PAYLOAD)
            .is_ok();
        if sent {
            for _ in 0..SELFTEST_POLL_LIMIT {
                pump(iface, device, &mut sockets);
                let mut buffer = [0u8; SELFTEST_BUFFER_BYTES];
                match sockets
                    .get_mut::<tcp::Socket>(server)
                    .recv_slice(&mut buffer)
                {
                    Ok(count) if count > 0 => {
                        forwarded = count == TCP_PAYLOAD.len() && &buffer[..count] == TCP_PAYLOAD;
                        *forwarded_flag = true;
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    // Server -> client reply.
    let mut replied = false;
    if forwarded {
        let sent = sockets
            .get_mut::<tcp::Socket>(server)
            .send_slice(TCP_REPLY)
            .is_ok();
        if sent {
            for _ in 0..SELFTEST_POLL_LIMIT {
                pump(iface, device, &mut sockets);
                let mut buffer = [0u8; SELFTEST_BUFFER_BYTES];
                match sockets
                    .get_mut::<tcp::Socket>(client)
                    .recv_slice(&mut buffer)
                {
                    Ok(count) if count > 0 => {
                        replied = count == TCP_REPLY.len() && &buffer[..count] == TCP_REPLY;
                        *replied_flag = true;
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    // Graceful close: client sends FIN, server closes from CloseWait, both
    // sides drain to closed states.
    let mut closed = false;
    if replied {
        sockets.get_mut::<tcp::Socket>(client).close();
        for _ in 0..SELFTEST_POLL_LIMIT {
            pump(iface, device, &mut sockets);
            let server_state = sockets.get_mut::<tcp::Socket>(server).state();
            if server_state == tcp::State::CloseWait {
                sockets.get_mut::<tcp::Socket>(server).close();
                break;
            }
        }
        for _ in 0..SELFTEST_POLL_LIMIT {
            pump(iface, device, &mut sockets);
            let client_state = sockets.get_mut::<tcp::Socket>(client).state();
            let server_state = sockets.get_mut::<tcp::Socket>(server).state();
            if matches!(client_state, tcp::State::Closed | tcp::State::TimeWait)
                && matches!(server_state, tcp::State::Closed | tcp::State::LastAck)
            {
                closed = true;
                *closed_flag = true;
                break;
            }
        }
    }

    *final_states = (
        sockets.get_mut::<tcp::Socket>(client).state() as u8,
        sockets.get_mut::<tcp::Socket>(server).state() as u8,
    );
    sockets.remove(client);
    sockets.remove(server);
    established && forwarded && replied && closed
}

/// IPv6 v0 slice probe: one UDP datagram round-trip between two sockets on
/// the interface's own link-local address through the real stack and device
/// loopback path. First delivery forces the same NS/NA neighbor dance real
/// v6 uses (the solicited-node multicast frame is kept in-process by the
/// device loopback predicate, then the NA reply fills the neighbor cache).
fn udp_v6_round_trip<D: smoltcp::phy::Device>(
    iface: &mut Interface,
    device: &mut D,
    sent_flag: &mut bool,
    got_flag: &mut bool,
    echoed_flag: &mut bool,
) -> bool {
    let link_local = crate::device::local_link_local();
    if link_local == smoltcp::wire::Ipv6Address::UNSPECIFIED {
        return false;
    }

    let mut a_meta_r = [udp::PacketMetadata::EMPTY; 2];
    let mut a_data_r = [0u8; SELFTEST_BUFFER_BYTES];
    let mut a_meta_t = [udp::PacketMetadata::EMPTY; 2];
    let mut a_data_t = [0u8; SELFTEST_BUFFER_BYTES];
    let mut b_meta_r = [udp::PacketMetadata::EMPTY; 2];
    let mut b_data_r = [0u8; SELFTEST_BUFFER_BYTES];
    let mut b_meta_t = [udp::PacketMetadata::EMPTY; 2];
    let mut b_data_t = [0u8; SELFTEST_BUFFER_BYTES];

    let mut test_storage = [SocketStorage::EMPTY; 4];
    let mut sockets = SocketSet::new(&mut test_storage[..]);
    let a = sockets.add(udp::Socket::new(
        udp::PacketBuffer::new(&mut a_meta_r[..], &mut a_data_r[..]),
        udp::PacketBuffer::new(&mut a_meta_t[..], &mut a_data_t[..]),
    ));
    let b = sockets.add(udp::Socket::new(
        udp::PacketBuffer::new(&mut b_meta_r[..], &mut b_data_r[..]),
        udp::PacketBuffer::new(&mut b_meta_t[..], &mut b_data_t[..]),
    ));

    let bound_a = sockets
        .get_mut::<udp::Socket>(a)
        // Explicit link-local bind so the datagram's source address is the
        // v6 interface address (mirrors the v4 loopback bind above).
        .bind(smoltcp::wire::IpListenEndpoint::from((
            link_local,
            SELFTEST_UDP_PORT_A,
        )))
        .is_ok();
    let bound_b = sockets
        .get_mut::<udp::Socket>(b)
        .bind(SELFTEST_UDP_PORT_B)
        .is_ok();
    if !bound_a || !bound_b {
        sockets.remove(a);
        sockets.remove(b);
        return false;
    }

    *sent_flag = true;
    let sent = sockets
        .get_mut::<udp::Socket>(a)
        .send_slice(
            UDP_PAYLOAD,
            smoltcp::wire::IpEndpoint {
                addr: smoltcp::wire::IpAddress::Ipv6(link_local),
                port: SELFTEST_UDP_PORT_B,
            },
        )
        .is_ok();

    let mut received = None;
    if sent {
        // Yield between polls: the smoltcp neighbor cache rate-limits
        // discovery for 1s cache-wide after any dispatch (the v4 probes
        // trigger it just before us), and the userspace monotonic clock only
        // advances across yields. Without this the first NS attempt would
        // stay rate-limited for the whole bounded loop on a quiet boot.
        // Host tests skip the yield entirely (see now_instant).
        #[cfg(not(test))]
        {
            let _ = rt::yield_current();
        }
        for poll_i in 0..SELFTEST_POLL_LIMIT {
            pump(iface, device, &mut sockets);
            #[cfg(not(test))]
            {
                let _ = rt::yield_current();
            }
            let mut buffer = [0u8; SELFTEST_BUFFER_BYTES];
            if let Ok((count, meta)) = sockets.get_mut::<udp::Socket>(b).recv_slice(&mut buffer) {
                if count == UDP_PAYLOAD.len()
                    && &buffer[..count] == UDP_PAYLOAD
                    && meta.endpoint.addr == smoltcp::wire::IpAddress::Ipv6(link_local)
                    && meta.endpoint.port == SELFTEST_UDP_PORT_A
                {
                    received = Some(meta.endpoint);
                    *got_flag = true;
                }
                break;
            }
        }
    }

    let mut echoed = false;
    if let Some(endpoint) = received {
        let sent_back = sockets
            .get_mut::<udp::Socket>(b)
            .send_slice(UDP_REPLY, endpoint)
            .is_ok();
        if sent_back {
            for _ in 0..SELFTEST_POLL_LIMIT {
                #[cfg(not(test))]
                {
                    let _ = rt::yield_current();
                }
                pump(iface, device, &mut sockets);
                let mut buffer = [0u8; SELFTEST_BUFFER_BYTES];
                match sockets.get_mut::<udp::Socket>(a).recv_slice(&mut buffer) {
                    Ok((count, _)) => {
                        echoed = count == UDP_REPLY.len() && &buffer[..count] == UDP_REPLY;
                        *echoed_flag = true;
                        break;
                    }
                    Err(_) => {}
                }
            }
        }
    }

    sockets.remove(a);
    sockets.remove(b);
    received.is_some() && echoed
}

/// IPv6 v0 slice probe: one ICMPv6 echo request to the interface's own
/// link-local address, answered by the stack's echo-reply path through the
/// device loopback. Bounded by poll iterations (the userspace monotonic
/// clock does not advance on this kernel build, so a wall-clock timeout
/// would never fire); returns the loop count at which the reply landed.
#[allow(clippy::too_many_arguments)]
fn ping6_round_trip<D: smoltcp::phy::Device>(
    iface: &mut Interface,
    device: &mut D,
    sockets: &mut SocketSet<'_>,
    icmp_handle: smoltcp::iface::SocketHandle,
    requested_flag: &mut bool,
    replied_flag: &mut bool,
    sequence: &mut u16,
) -> (bool, u64) {
    use smoltcp::wire::{Icmpv6Packet, Icmpv6Repr, Ipv6Address};

    let link_local = crate::device::local_link_local();
    if link_local == Ipv6Address::UNSPECIFIED {
        return (false, 0);
    }
    let checksum = device.capabilities().checksum;

    {
        let socket = sockets.get_mut::<icmp::Socket>(icmp_handle);
        if !socket.is_open() {
            let _ = socket.bind(icmp::Endpoint::Ident(crate::consts::PING_IDENTIFIER));
        }
        if !socket.can_send() {
            return (false, 0);
        }
        let payload = [0x53, 0x4f, (*sequence >> 8) as u8, *sequence as u8];
        let icmp_repr = Icmpv6Repr::EchoRequest {
            ident: crate::consts::PING_IDENTIFIER,
            seq_no: *sequence,
            data: &payload,
        };
        let packet = match socket.send(
            icmp_repr.buffer_len(),
            smoltcp::wire::IpAddress::Ipv6(link_local),
        ) {
            Ok(packet) => packet,
            Err(_) => return (false, 0),
        };
        icmp_repr.emit(
            &link_local,
            &link_local,
            &mut Icmpv6Packet::new_unchecked(packet),
            &checksum,
        );
    }
    *requested_flag = true;
    let seq_no = *sequence;
    *sequence = sequence.wrapping_add(1);

    for poll in 0..SELFTEST_POLL_LIMIT {
        // Same rate-limit yield as the v6 UDP probe above.
        #[cfg(not(test))]
        {
            let _ = rt::yield_current();
        }
        pump(iface, device, sockets);
        let socket = sockets.get_mut::<icmp::Socket>(icmp_handle);
        if socket.can_recv() {
            let Ok((payload, remote)) = socket.recv() else {
                continue;
            };
            let IpAddress::Ipv6(remote_v6) = remote else {
                continue;
            };
            let Ok(packet) = Icmpv6Packet::new_checked(&payload) else {
                continue;
            };
            let Ok(reply) = Icmpv6Repr::parse(&remote_v6, &link_local, &packet, &checksum) else {
                continue;
            };
            if let Icmpv6Repr::EchoReply {
                ident,
                seq_no: reply_seq,
                ..
            } = reply
            {
                if ident == crate::consts::PING_IDENTIFIER && reply_seq == seq_no {
                    *replied_flag = true;
                    return (true, poll as u64);
                }
            }
        }
    }
    (false, SELFTEST_POLL_LIMIT as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use smoltcp::{
        iface::Config as IfaceConfig,
        phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken},
        time::Instant,
        wire::{EthernetAddress, HardwareAddress},
    };

    /// In-memory stand-in for KernelPacketDevice that applies the same
    /// self-frame rule: frames the device loopback predicate claims are
    /// pushed back onto the RX queue, everything else is parked in an
    /// "escaped" sink for assertions.
    type SharedQueue = std::rc::Rc<std::cell::RefCell<std::collections::VecDeque<Vec<u8>>>>;

    pub(crate) fn shared_queue() -> SharedQueue {
        std::rc::Rc::new(std::cell::RefCell::new(std::collections::VecDeque::new()))
    }

    pub(crate) struct LoopDevice {
        pub(crate) rx: SharedQueue,
        pub(crate) escaped: SharedQueue,
    }

    pub(crate) struct LoopRx {
        pub(crate) frame: Vec<u8>,
    }

    #[derive(Clone)]
    pub(crate) struct LoopTx {
        rx: SharedQueue,
        escaped: SharedQueue,
    }

    impl LoopDevice {
        pub(crate) fn new() -> Self {
            Self {
                rx: shared_queue(),
                escaped: shared_queue(),
            }
        }
    }

    impl Device for LoopDevice {
        type RxToken<'a> = LoopRx;
        type TxToken<'a> = LoopTx;

        fn receive(&mut self, _t: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
            let frame = self.rx.borrow_mut().pop_front()?;
            Some((
                LoopRx { frame },
                LoopTx {
                    rx: self.rx.clone(),
                    escaped: self.escaped.clone(),
                },
            ))
        }

        fn transmit(&mut self, _t: Instant) -> Option<Self::TxToken<'_>> {
            Some(LoopTx {
                rx: self.rx.clone(),
                escaped: self.escaped.clone(),
            })
        }

        fn capabilities(&self) -> DeviceCapabilities {
            let mut caps = DeviceCapabilities::default();
            caps.medium = Medium::Ethernet;
            caps.max_transmission_unit = 1500;
            caps.max_burst_size = Some(1);
            caps
        }
    }

    impl RxToken for LoopRx {
        fn consume<R, F>(self, f: F) -> R
        where
            F: FnOnce(&[u8]) -> R,
        {
            f(&self.frame)
        }
    }

    impl TxToken for LoopTx {
        fn consume<R, F>(self, len: usize, f: F) -> R
        where
            F: FnOnce(&mut [u8]) -> R,
        {
            let mut buffer = vec![0u8; len];
            let result = f(&mut buffer);
            if crate::device::frame_targets_guest(&buffer) {
                self.rx.borrow_mut().push_back(buffer);
            } else {
                self.escaped.borrow_mut().push_back(buffer);
            }
            result
        }
    }

    /// Host-side proof of the whole v0 mechanism: link-local configuration,
    /// the NS/NA neighbor dance through the device loopback predicate, and
    /// one UDP datagram round-trip between two sockets on the same
    /// link-local address — the exact sequence the gated in-guest probe
    /// drives, without kernel or QEMU.
    #[test]
    fn v6_loopback_udp_round_trip_host() {
        let mac: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let own = crate::util::eui64_link_local(mac);
        crate::device::set_loopback_identity_for_tests(
            mac,
            smoltcp::wire::Ipv4Address::new(10, 0, 2, 15),
            own,
        );

        let mut device = LoopDevice::new();
        let mut iface = Interface::new(
            IfaceConfig::new(HardwareAddress::Ethernet(EthernetAddress(mac))),
            &mut device,
            Instant::from_secs(0),
        );
        crate::protocol::apply_interface_runtime(
            &mut iface,
            crate::types::InterfaceRuntimeState::static_config(crate::types::NetworkConfig {
                static_address: smoltcp::wire::Ipv4Address::new(10, 0, 2, 15),
                static_prefix_len: 24,
                static_gateway: smoltcp::wire::Ipv4Address::new(10, 0, 2, 2),
                dynamic_ipv4: false,
                dns_server: smoltcp::wire::Ipv4Address::new(10, 0, 2, 3),
                probe_timeout_ticks: 100,
                dns_query_timeout_ticks: 100,
                dhcp_acquire_timeout_ticks: 100,
                tcp_connect_timeout_ticks: 100,
                tcp_idle_timeout_ticks: 100,
            }),
        );

        let (mut sent, mut got, mut echoed) = (false, false, false);
        let ok = udp_v6_round_trip(&mut iface, &mut device, &mut sent, &mut got, &mut echoed);
        assert!(sent, "send_slice must accept the queued datagram");
        assert!(ok, "v6 UDP round trip must complete over the loopback path");
        assert!(got && echoed);
        assert!(
            device.escaped.borrow().is_empty(),
            "self-addressed v6 frames must never escape to the backend"
        );
    }

    /// Same machinery for ICMPv6 echo: request to the own link-local, echo
    /// reply produced by the stack's echo path, received back via loopback.
    #[test]
    fn v6_loopback_ping6_host() {
        let mac: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let own = crate::util::eui64_link_local(mac);
        crate::device::set_loopback_identity_for_tests(
            mac,
            smoltcp::wire::Ipv4Address::new(10, 0, 2, 15),
            own,
        );

        let mut device = LoopDevice::new();
        let mut iface = Interface::new(
            IfaceConfig::new(HardwareAddress::Ethernet(EthernetAddress(mac))),
            &mut device,
            Instant::from_secs(0),
        );
        crate::protocol::apply_interface_runtime(
            &mut iface,
            crate::types::InterfaceRuntimeState::static_config(crate::types::NetworkConfig {
                static_address: smoltcp::wire::Ipv4Address::new(10, 0, 2, 15),
                static_prefix_len: 24,
                static_gateway: smoltcp::wire::Ipv4Address::new(10, 0, 2, 2),
                dynamic_ipv4: false,
                dns_server: smoltcp::wire::Ipv4Address::new(10, 0, 2, 3),
                probe_timeout_ticks: 100,
                dns_query_timeout_ticks: 100,
                dhcp_acquire_timeout_ticks: 100,
                tcp_connect_timeout_ticks: 100,
                tcp_idle_timeout_ticks: 100,
            }),
        );

        let mut socket_storage = [SocketStorage::EMPTY; 2];
        let mut icmp_rx_meta = [icmp::PacketMetadata::EMPTY; 2];
        let mut icmp_tx_meta = [icmp::PacketMetadata::EMPTY];
        let mut icmp_rx_data = [0u8; 256];
        let mut icmp_tx_data = [0u8; 256];
        let mut sockets = SocketSet::new(&mut socket_storage[..]);
        let icmp_handle = sockets.add(icmp::Socket::new(
            icmp::PacketBuffer::new(&mut icmp_rx_meta[..], &mut icmp_rx_data[..]),
            icmp::PacketBuffer::new(&mut icmp_tx_meta[..], &mut icmp_tx_data[..]),
        ));

        let mut requested = false;
        let mut replied = false;
        let mut sequence = 1u16;
        let (ok, polls) = ping6_round_trip(
            &mut iface,
            &mut device,
            &mut sockets,
            icmp_handle,
            &mut requested,
            &mut replied,
            &mut sequence,
        );
        assert!(requested);
        assert!(ok, "ICMPv6 echo must round-trip over the loopback path");
        assert!(replied);
        assert!(polls < 4096);
        assert!(device.escaped.borrow().is_empty());
    }
}

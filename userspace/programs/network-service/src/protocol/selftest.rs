use smoltcp::{
    iface::{Interface, SocketSet, SocketStorage},
    socket::{tcp, udp},
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
fn emit_phase_record(
    log_handle: rt::Handle,
    severity: rt::LogSeverity,
    phase: u64,
    detail: u64,
) {
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

fn pump(iface: &mut Interface, device: &mut KernelPacketDevice, sockets: &mut SocketSet<'_>) {
    let _ = iface.poll(now_instant(), device, sockets);
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

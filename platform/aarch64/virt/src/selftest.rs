use alloc::sync::Arc;
use core::fmt::Write as _;

use serviceos_kernel_core::block::BlockBackend;
use serviceos_kernel_core::display::DisplayBackend;
use serviceos_kernel_core::input::InputBackend;
use serviceos_kernel_core::network::PacketBackend;

use crate::timer;

const BLOCK_BYTES: usize = 512;
const NET_BUFFER_BYTES: usize = 1600;
const INPUT_WAIT_SECONDS: u64 = 5;

const DHCP_XID: u32 = 0x5345_5256;
const DHCP_PAYLOAD_BYTES: usize = 300;

const DISPLAY_FRAME_BYTES: usize = 1024 * 768 * 4;

static mut DISPLAY_FRAME: [u8; DISPLAY_FRAME_BYTES] = [0; DISPLAY_FRAME_BYTES];

fn log(scope: &str, message: core::fmt::Arguments<'_>) {
    crate::uart::write_bytes(b"serviceos: ");
    crate::uart::write_bytes(scope.as_bytes());
    crate::uart::write_bytes(b": ");
    struct Writer;
    impl core::fmt::Write for Writer {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            crate::uart::write_bytes(s.as_bytes());
            Ok(())
        }
    }
    let _ = Writer.write_fmt(message);
    crate::uart::write_bytes(b"\r\n");
}

fn deadline_cycles(seconds: u64) -> u64 {
    timer::counter_value().saturating_add(seconds * timer::counter_frequency_hz().max(1))
}

pub fn block_selftest(backend: &Arc<dyn BlockBackend>) {
    let info = backend.info();
    log(
        "storage-selftest",
        format_args!(
            "start backend={} writable={} blocks={} block-size={}",
            info.backend, info.writable, info.block_count, info.block_size
        ),
    );
    if info.block_size as usize != BLOCK_BYTES || info.block_count < 2 {
        log("storage-selftest", format_args!("FAIL unsupported-geometry"));
        return;
    }

    let mut scratch = [0u8; BLOCK_BYTES];
    match backend.read_blocks(0, &mut scratch) {
        Ok(len) => log(
            "storage-selftest",
            format_args!(
                "read block0 len={} head={:02x}{:02x}{:02x}{:02x} ops={}",
                len,
                scratch[0],
                scratch[1],
                scratch[2],
                scratch[3],
                backend.info().read_ops
            ),
        ),
        Err(error) => {
            log("storage-selftest", format_args!("FAIL read-block0 {error:?}"));
            return;
        }
    }

    let probe_block = info.block_count - 1;
    let mut pattern = [0u8; BLOCK_BYTES];
    for (index, byte) in pattern.iter_mut().enumerate() {
        *byte = (index as u8).rotate_left(3) ^ 0x5a;
    }
    if let Err(error) = backend.write_blocks(probe_block, &pattern) {
        log("storage-selftest", format_args!("FAIL write {error:?}"));
        return;
    }
    let mut readback = [0u8; BLOCK_BYTES];
    if let Err(error) = backend.read_blocks(probe_block, &mut readback) {
        log("storage-selftest", format_args!("FAIL readback {error:?}"));
        return;
    }
    if readback != pattern {
        log("storage-selftest", format_args!("FAIL verify-mismatch"));
        return;
    }
    let after = backend.info();
    log(
        "storage-selftest",
        format_args!(
            "PASS write-read-back block={} read-ops={} write-ops={}",
            probe_block, after.read_ops, after.write_ops
        ),
    );
}

fn ipv4_checksum(header: &[u8]) -> u16 {
    let mut sum = 0u32;
    for pair in header.chunks_exact(2) {
        sum += u16::from_be_bytes([pair[0], pair[1]]) as u32;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn build_dhcp_discover(frame: &mut [u8; NET_BUFFER_BYTES], mac: &[u8; 6]) -> usize {
    let payload_start = 14 + 20 + 8;
    for byte in frame.iter_mut() {
        *byte = 0;
    }
    frame[..6].fill(0xff);
    frame[6..12].copy_from_slice(mac);
    frame[12] = 0x08;
    frame[13] = 0x00;

    let ip_total = 20 + 8 + DHCP_PAYLOAD_BYTES;
    frame[14] = 0x45;
    frame[15] = 0x00;
    frame[16..18].copy_from_slice(&(ip_total as u16).to_be_bytes());
    frame[18..20].copy_from_slice(&(DHCP_XID & 0xffff).to_be_bytes());
    frame[20] = 0x00;
    frame[21] = 0x00;
    frame[22] = 64;
    frame[23] = 17;
    frame[24..26].copy_from_slice(&[0, 0]);
    frame[26..30].copy_from_slice(&[0, 0, 0, 0]);
    frame[30..34].copy_from_slice(&[255, 255, 255, 255]);
    let checksum = ipv4_checksum(&frame[14..34]);
    frame[24..26].copy_from_slice(&checksum.to_be_bytes());

    let udp_len = (8 + DHCP_PAYLOAD_BYTES) as u16;
    frame[34] = 68;
    frame[35] = 67;
    frame[36] = 0x00;
    frame[37] = 0x00;
    frame[38..40].copy_from_slice(&udp_len.to_be_bytes());
    frame[40] = 0x00;
    frame[41] = 0x00;

    let p = payload_start;
    frame[p] = 1;
    frame[p + 1] = 1;
    frame[p + 2] = 6;
    frame[p + 3] = 0;
    frame[p + 4..p + 8].copy_from_slice(&DHCP_XID.to_be_bytes());
    frame[p + 8] = 0;
    frame[p + 9] = 0;
    frame[p + 10] = 0x80;
    frame[p + 11] = 0x00;
    frame[p + 28..p + 34].copy_from_slice(mac);
    frame[p + 236..p + 240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]);
    let mut option = p + 240;
    frame[option] = 53;
    frame[option + 1] = 1;
    frame[option + 2] = 1;
    option += 3;
    frame[option] = 61;
    frame[option + 1] = 7;
    frame[option + 2] = 1;
    frame[option + 3..option + 9].copy_from_slice(mac);
    option += 9;
    frame[option] = 255;
    option += 1;
    let _ = option;

    payload_start + DHCP_PAYLOAD_BYTES
}

pub fn network_selftest(backend: &Arc<dyn PacketBackend>) {
    let info = backend.info();
    log(
        "network-selftest",
        format_args!(
            "start mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} mtu={} link={}",
            info.mac[0],
            info.mac[1],
            info.mac[2],
            info.mac[3],
            info.mac[4],
            info.mac[5],
            info.mtu,
            info.link_state
        ),
    );

    let mut frame = [0u8; NET_BUFFER_BYTES];
    let frame_len = build_dhcp_discover(&mut frame, &info.mac);
    if let Err(error) = backend.transmit(&frame[..frame_len]) {
        log("network-selftest", format_args!("FAIL transmit {error:?}"));
        return;
    }
    log(
        "network-selftest",
        format_args!("dhcp-discover sent bytes={} tx-ops={}", frame_len, backend.info().tx_packets),
    );

    let deadline = deadline_cycles(8);
    let mut rx_buffer = [0u8; NET_BUFFER_BYTES];
    let mut saw_reply = false;
    let mut yiaddr = [0u8; 4];
    while timer::counter_value() < deadline {
        backend.poll();
        match backend.receive(&mut rx_buffer) {
            Ok(len) => {
                if len >= 42 + 17 && rx_buffer[42] == 2 {
                    yiaddr.copy_from_slice(&rx_buffer[42 + 16..42 + 20]);
                    saw_reply = true;
                    break;
                }
            }
            Err(_) => {}
        }
    }

    if saw_reply {
        log(
            "network-selftest",
            format_args!(
                "PASS dhcp-offer yiaddr={}.{}.{}.{} rx-ops={}",
                yiaddr[0],
                yiaddr[1],
                yiaddr[2],
                yiaddr[3],
                backend.info().rx_packets
            ),
        );
    } else {
        log(
            "network-selftest",
            format_args!("FAIL no-dhcp-reply rx-ops={}", backend.info().rx_packets),
        );
    }
}

pub fn display_selftest(backend: &Arc<dyn DisplayBackend>) {
    let info = backend.info();
    log(
        "display-selftest",
        format_args!(
            "start {}x{} stride={} bytes={} format={}",
            info.width, info.height, info.stride, info.byte_len, info.pixel_format
        ),
    );
    let frame_len = info.byte_len as usize;
    if frame_len > DISPLAY_FRAME_BYTES {
        log("display-selftest", format_args!("FAIL frame-too-large"));
        return;
    }
    let frame = unsafe {
        core::slice::from_raw_parts_mut(
            core::ptr::addr_of_mut!(DISPLAY_FRAME).cast::<u8>(),
            frame_len,
        )
    };
    for (index, pixel) in frame.chunks_exact_mut(4).enumerate() {
        let x = (index % 1024) as u32;
        let y = (index / 1024) as u32;
        pixel[0] = (x * 255 / 1024) as u8;
        pixel[1] = (y * 255 / 768) as u8;
        pixel[2] = 0x40;
        pixel[3] = 0xff;
    }
    match backend.present(frame) {
        Ok(()) => log(
            "display-selftest",
            format_args!(
                "PASS present bytes={} present-count={}",
                frame_len,
                backend.info().present_count
            ),
        ),
        Err(error) => log("display-selftest", format_args!("FAIL present {error:?}")),
    }
}

pub fn input_selftest(backend: &Arc<dyn InputBackend>) {
    let info = backend.info();
    log(
        "input-selftest",
        format_args!(
            "start capabilities={:#x} devices={} waiting-{}s-for-events",
            info.capabilities, info.device_count, INPUT_WAIT_SECONDS
        ),
    );

    let deadline = deadline_cycles(INPUT_WAIT_SECONDS);
    let mut events = 0u64;
    let mut kinds = [0u32; 4];
    let mut first = [0u32; 4];
    while timer::counter_value() < deadline {
        if backend.poll() {
            while let Ok(event) = backend.receive() {
                if events == 0 {
                    first = [event.kind, event.code, event.value0 as u32, event.value1 as u32];
                } else if events < 4 {
                    kinds[(events - 1) as usize] = event.kind;
                }
                events = events.saturating_add(1);
            }
        }
    }

    if events > 0 {
        log(
            "input-selftest",
            format_args!(
                "PASS events={} first-kind={} code={} v0={} v1={}",
                events, first[0], first[1], first[2], first[3]
            ),
        );
    } else {
        log(
            "input-selftest",
            format_args!("FAIL no-events devices={}", info.device_count),
        );
    }
}

pub fn run_all(
    block: &Option<Arc<dyn BlockBackend>>,
    network: &Option<Arc<dyn PacketBackend>>,
    display: &Option<Arc<dyn DisplayBackend>>,
    input: &Option<Arc<dyn InputBackend>>,
) {
    if let Some(backend) = block {
        block_selftest(backend);
    } else {
        log("storage-selftest", format_args!("SKIP no-backend"));
    }
    if let Some(backend) = network {
        network_selftest(backend);
    } else {
        log("network-selftest", format_args!("SKIP no-backend"));
    }
    if let Some(backend) = display {
        display_selftest(backend);
    } else {
        log("display-selftest", format_args!("SKIP no-backend"));
    }
    if let Some(backend) = input {
        input_selftest(backend);
    } else {
        log("input-selftest", format_args!("SKIP no-backend"));
    }
}

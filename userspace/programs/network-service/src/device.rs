use core::cell::UnsafeCell;

use smoltcp::{
    phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken},
    time::Instant,
    wire::{ArpOperation, ArpPacket, ArpRepr, EthernetAddress, EthernetFrame, Ipv4Address},
};

use rt::PacketInterfaceInfo;
use serviceos_userspace_runtime as rt;

use crate::consts::{LOOPBACK_ADDRESS, MAX_FRAME_BYTES};

/// Frames emitted by the stack that are destined for the guest itself are
/// routed through this ring instead of the virtio queue so guest-internal
/// loopback (127.0.0.1) works under slirp NAT, which would otherwise never
/// echo such frames back.
struct LoopbackRing {
    slots: [[u8; MAX_FRAME_BYTES]; LOOPBACK_RING_SLOTS],
    lens: [usize; LOOPBACK_RING_SLOTS],
    head: usize,
    len: usize,
    dropped: u64,
}

const LOOPBACK_RING_SLOTS: usize = 8;

impl LoopbackRing {
    const fn new() -> Self {
        Self {
            slots: [[0; MAX_FRAME_BYTES]; LOOPBACK_RING_SLOTS],
            lens: [0; LOOPBACK_RING_SLOTS],
            head: 0,
            len: 0,
            dropped: 0,
        }
    }

    fn push(&mut self, frame: &[u8]) {
        if self.len == LOOPBACK_RING_SLOTS || frame.len() > MAX_FRAME_BYTES {
            self.dropped += 1;
            return;
        }
        let tail = (self.head + self.len) % LOOPBACK_RING_SLOTS;
        self.slots[tail][..frame.len()].copy_from_slice(frame);
        self.lens[tail] = frame.len();
        self.len += 1;
    }

    fn pop(&mut self) -> Option<([u8; MAX_FRAME_BYTES], usize)> {
        if self.len == 0 {
            return None;
        }
        let index = self.head;
        let mut frame = [0u8; MAX_FRAME_BYTES];
        let len = self.lens[index];
        frame[..len].copy_from_slice(&self.slots[index][..len]);
        self.head = (self.head + 1) % LOOPBACK_RING_SLOTS;
        self.len -= 1;
        Some((frame, len))
    }
}

// SAFETY (all three cells): the network service is a single-threaded process
// and every access happens on its main loop while servicing `iface.poll`.
// `SyncCell` exists only to satisfy the `Sync` bound on statics.
struct SyncCell<T>(UnsafeCell<T>);
unsafe impl<T> Sync for SyncCell<T> {}

static LOOPBACK_RX_RING: SyncCell<Option<LoopbackRing>> = SyncCell(UnsafeCell::new(None));
static OWN_MAC: SyncCell<[u8; 6]> = SyncCell(UnsafeCell::new([0; 6]));
static LOCAL_IPV4: SyncCell<[u8; 4]> = SyncCell(UnsafeCell::new([127, 0, 0, 1]));
static LB_STATS: SyncCell<(u64, u64, u64)> = SyncCell(UnsafeCell::new((0, 0, 0)));

pub(crate) fn loopback_stats() -> (u64, u64, u64) {
    // SAFETY: see static declarations above.
    unsafe { *LB_STATS.0.get() }
}

fn with_ring<R>(f: impl FnOnce(&mut LoopbackRing) -> R) -> R {
    // SAFETY: see static declarations above.
    let cell = unsafe { &mut *LOOPBACK_RX_RING.0.get() };
    if cell.is_none() {
        *cell = Some(LoopbackRing::new());
    }
    f(cell.as_mut().expect("ring initialized"))
}

pub(crate) fn set_local_ipv4(address: Ipv4Address) {
    // SAFETY: see static declarations above.
    unsafe {
        *LOCAL_IPV4.0.get() = address.octets();
    }
}

pub(crate) struct KernelPacketDevice {
    pub(crate) handle: rt::Handle,
    pub(crate) info: PacketInterfaceInfo,
    rx_buffer: [u8; MAX_FRAME_BYTES],
    tx_buffer: [u8; MAX_FRAME_BYTES],
}

impl KernelPacketDevice {
    pub(crate) fn new(handle: rt::Handle, info: PacketInterfaceInfo) -> Self {
        // SAFETY: see static declarations above.
        unsafe {
            *OWN_MAC.0.get() = info.mac;
        }
        Self {
            handle,
            info,
            rx_buffer: [0; MAX_FRAME_BYTES],
            tx_buffer: [0; MAX_FRAME_BYTES],
        }
    }
}

pub(crate) struct KernelRxToken<'a> {
    buffer: &'a mut [u8],
}

pub(crate) struct KernelTxToken<'a> {
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
        let looped = with_ring(|ring| ring.pop());
        let length = match looped {
            Some((frame, length)) => {
                self.rx_buffer[..length].copy_from_slice(&frame[..length]);
                length
            }
            None => {
                rt::packet_interface_receive_nonblocking(self.handle, &mut self.rx_buffer).ok()?
            }
        };
        Some((
            KernelRxToken {
                buffer: &mut self.rx_buffer[..length],
            },
            KernelTxToken {
                handle: self.handle,
                buffer: &mut self.tx_buffer,
            },
        ))
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
        let frame = &self.buffer[..len];
        if frame_targets_guest(frame) {
            // SAFETY: see static declarations above.
            unsafe {
                let stats = &mut *LB_STATS.0.get();
                stats.0 += 1;
            }
            with_ring(|ring| {
                let before = ring.len;
                ring.push(frame);
                if ring.len == before {
                    // SAFETY: see static declarations above.
                    unsafe {
                        (*LB_STATS.0.get()).2 += 1;
                    }
                }
            });
        } else {
            // SAFETY: see static declarations above.
            unsafe {
                (*LB_STATS.0.get()).1 += 1;
            }
            let _ = rt::packet_interface_transmit(self.handle, frame);
        }
        result
    }
}

/// Whether an emitted frame must be delivered back into our own RX path:
/// unicast addressed to our own MAC from our own MAC, or an ARP request
/// resolving one of the guest's own addresses (loopnet or assigned IP). ARP
/// replies we emit are unicast-to-self and loop back through the first rule,
/// which also fills the neighbor cache when reprocessed.
fn frame_targets_guest(frame: &[u8]) -> bool {
    let Ok(eth) = EthernetFrame::new_checked(frame) else {
        return false;
    };
    // SAFETY: see static declarations above.
    let own_mac = EthernetAddress(unsafe { *OWN_MAC.0.get() });
    let octets = unsafe { *LOCAL_IPV4.0.get() };
    let local = Ipv4Address::new(octets[0], octets[1], octets[2], octets[3]);
    if eth.dst_addr() == own_mac && eth.src_addr() == own_mac {
        return true;
    }
    if eth.ethertype() != smoltcp::wire::EthernetProtocol::Arp {
        return false;
    }
    let Ok(arp) = ArpPacket::new_checked(eth.payload()) else {
        return false;
    };
    let Ok(ArpRepr::EthernetIpv4 {
        operation,
        target_protocol_addr,
        ..
    }) = ArpRepr::parse(&arp)
    else {
        return false;
    };
    operation == ArpOperation::Request
        && (target_protocol_addr == LOOPBACK_ADDRESS || target_protocol_addr == local)
}

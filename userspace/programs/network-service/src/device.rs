use core::cell::UnsafeCell;
use core::ptr::NonNull;

use smoltcp::{
    phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken},
    time::Instant,
    wire::{ArpOperation, ArpPacket, ArpRepr, EthernetAddress, EthernetFrame, Ipv4Address},
};

use rt::{MappedMemory, PacketInterfaceInfo, PacketRingLayout};
use serviceos_userspace_runtime as rt;

use crate::consts::{LOOPBACK_ADDRESS, MAX_FRAME_BYTES};

/// Consumer-side mirror of the shared RX ring header layout (see
/// `kernel/core/src/network/ring.rs`). The image is one header page followed
/// by one page per slot; each slot page starts with a u64 length then the
/// frame data.
const RING_MAGIC: u32 = 0x534f_5258;
const RING_VERSION: u32 = 1;
const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 4;
const OFF_HEAD: usize = 16;
const OFF_TAIL: usize = 24;
const OFF_FRAMES_PUSHED: usize = 32;
const OFF_COPIES_AVOIDED: usize = 40;
const OFF_BYTES_SAVED: usize = 48;
const OFF_DROPPED: usize = 56;

fn load_u64(image: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(image[offset..offset + 8].try_into().expect("u64 word"))
}

/// Zero-copy statistics exposed through the public control channel.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RxRingSnapshot {
    pub(crate) active: bool,
    pub(crate) frames_pushed: u64,
    pub(crate) copies_avoided: u64,
    pub(crate) bytes_saved: u64,
    pub(crate) dropped: u64,
}

struct MappedRxRing {
    mem: MappedMemory,
    slot_count: usize,
    tail: u64,
}

impl MappedRxRing {
    fn image(&self) -> &[u8] {
        self.mem.as_slice()
    }

    fn head(&self) -> u64 {
        load_u64(self.image(), OFF_HEAD)
    }

    fn page_bytes(&self) -> usize {
        4096
    }

    fn next_sequence(&self) -> Option<u64> {
        let head = self.head();
        if self.tail >= head {
            return None;
        }
        Some(self.tail)
    }

    /// In-place claim of the frame at `sequence`: returns a pointer into the
    /// shared mapping plus the frame length. The caller must commit the
    /// sequence once its hot paths finish parsing the borrowed bytes.
    ///
    /// # Safety
    /// Single-threaded service; the returned pointer stays valid for the
    /// process lifetime because the mapping is never unmapped.
    unsafe fn claim(&mut self, sequence: u64) -> Option<(*const u8, usize)> {
        if sequence >= self.head() {
            return None;
        }
        let index = (sequence % self.slot_count as u64) as usize;
        let len_offset = (index + 1) * self.page_bytes();
        let length = load_u64(self.image(), len_offset) as usize;
        if length == 0 || length > MAX_FRAME_BYTES {
            return None;
        }
        let ptr = unsafe { self.mem.as_ptr().add(len_offset + 8) };
        Some((ptr, length))
    }

    fn commit(&mut self, sequence: u64, length: usize) {
        // SAFETY: single consumer; aligned counter stores into the shared
        // image, which outlives every use here.
        unsafe {
            let base = self.mem.as_ptr();
            core::ptr::write_unaligned(base.add(OFF_TAIL) as *mut u64, sequence + 1);
            let avoided = core::ptr::read_unaligned(base.add(OFF_COPIES_AVOIDED) as *const u64);
            core::ptr::write_unaligned(
                base.add(OFF_COPIES_AVOIDED) as *mut u64,
                avoided.wrapping_add(1),
            );
            let saved = core::ptr::read_unaligned(base.add(OFF_BYTES_SAVED) as *const u64);
            core::ptr::write_unaligned(
                base.add(OFF_BYTES_SAVED) as *mut u64,
                saved.wrapping_add(length as u64),
            );
        }
        self.tail = sequence + 1;
    }

    /// Advance past a sequence whose payload cannot be claimed (keeps the
    /// consumer cursor live without inflating the zero-copy counters).
    fn discard(&mut self, sequence: u64) {
        // SAFETY: single consumer; aligned store into the shared image.
        unsafe {
            core::ptr::write_unaligned(
                self.mem.as_ptr().add(OFF_TAIL) as *mut u64,
                sequence + 1,
            );
        }
        self.tail = sequence + 1;
    }
}

static RX_RING: SyncCell<Option<MappedRxRing>> = SyncCell(UnsafeCell::new(None));

fn with_rx_ring<R>(f: impl FnOnce(&mut Option<MappedRxRing>) -> R) -> R {
    // SAFETY: single-threaded service, main-loop-only access.
    let cell = unsafe { &mut *RX_RING.0.get() };
    f(cell)
}

/// Negotiate the shared RX ring with the kernel and map it into this
/// service. Any failure leaves the legacy copied-frame path in place.
/// Returns true when the shared path became active.
pub(crate) fn enable_shared_rx(packet_handle: rt::Handle) -> bool {
    if with_rx_ring(|ring| ring.is_some()) {
        return true;
    }
    let mut layout = PacketRingLayout {
        magic: 0,
        version: 0,
        slot_count: 0,
        slot_data_bytes: 0,
        slot_stride_bytes: 0,
        total_bytes: 0,
    };
    let memory_handle =
        match rt::packet_interface_ring_setup(packet_handle, &mut layout) {
            Ok(handle) => handle,
            Err(_) => return false,
        };
    if layout.magic != RING_MAGIC || layout.version != RING_VERSION || layout.slot_count == 0 {
        return false;
    }
    let total_bytes = layout.total_bytes as usize;
    let mapped = match MappedMemory::map(memory_handle, total_bytes, true) {
        Ok(mapped) => mapped,
        Err(_) => return false,
    };
    let _ = rt::handle_close(memory_handle);
    let slots = layout.slot_count as usize;
    with_rx_ring(move |ring| {
        *ring = Some(MappedRxRing {
            mem: mapped,
            slot_count: slots,
            tail: 0,
        });
    });
    true
}

pub(crate) fn rx_ring_snapshot() -> RxRingSnapshot {
    with_rx_ring(|ring| {
        let Some(ring) = ring.as_ref() else {
            return RxRingSnapshot::default();
        };
        let image = ring.image();
        RxRingSnapshot {
            active: true,
            frames_pushed: load_u64(image, OFF_FRAMES_PUSHED),
            copies_avoided: load_u64(image, OFF_COPIES_AVOIDED),
            bytes_saved: load_u64(image, OFF_BYTES_SAVED),
            dropped: load_u64(image, OFF_DROPPED),
        }
    })
}

/// Producer-side mirror of the shared TX ring header layout (see
/// `kernel/core/src/network/ring.rs`): identical image shape to the RX ring,
/// but this service is the PRODUCER (owns the free-running head counter and
/// fills slots with outbound frames) while the kernel is the consumer
/// (drains slots through the virtio backend on doorbell and owns tail).
struct MappedTxRing {
    mem: MappedMemory,
    slot_count: usize,
    head: u64,
    /// Consecutive doorbells that failed to drain our published frames.
    stalled_doorbells: u32,
    /// Set once the kernel side stops draining: every transmit then takes
    /// the legacy copied path for the rest of the session (correctness over
    /// zero-copy).
    disabled: bool,
}

/// Doorbells with zero drain progress tolerated before the producer
/// permanently reverts to the legacy copied-transmit path.
const TX_STALL_DOORBELL_LIMIT: u32 = 8;

const RING_PAGE_BYTES: usize = 4096;

impl MappedTxRing {
    /// Consumer cursor view (kernel-owned; read-only for this side).
    fn kernel_tail(&self) -> u64 {
        load_u64(self.image(), OFF_TAIL)
    }

    fn image(&self) -> &[u8] {
        self.mem.as_slice()
    }

    /// Room check mirroring production discipline: only publish while
    /// `head - tail < slot_count`, so a slow kernel can never make this
    /// producer overwrite an in-flight frame.
    fn has_room(&self) -> bool {
        self.head.wrapping_sub(self.kernel_tail()) < self.slot_count as u64
    }

    /// Mutable borrow of one slot's full frame-data region.
    ///
    /// # Safety
    /// `sequence % slot_count` must index a live slot; single-threaded
    /// service and the mapping lives for the process lifetime.
    unsafe fn slot_mut(&mut self, sequence: u64) -> &mut [u8] {
        let index = (sequence % self.slot_count as u64) as usize;
        let data_offset = (index + 1) * RING_PAGE_BYTES + 8;
        // SAFETY: slot data region lies inside the live mapping by layout.
        unsafe { core::slice::from_raw_parts_mut(self.mem.as_ptr().add(data_offset), MAX_FRAME_BYTES) }
    }

    /// Publish a filled slot: write its length, advance head past
    /// `sequence`, count the push. The frame becomes visible to the kernel
    /// consumer exactly here.
    fn publish(&mut self, sequence: u64, length: usize) {
        let index = (sequence % self.slot_count as u64) as usize;
        let len_offset = (index + 1) * RING_PAGE_BYTES;
        // SAFETY: length field at the slot page start inside the mapping.
        unsafe {
            core::ptr::write_unaligned(
                self.mem.as_ptr().add(len_offset) as *mut u64,
                length as u64,
            );
            let base = self.mem.as_ptr();
            let pushed = core::ptr::read_unaligned(base.add(OFF_FRAMES_PUSHED) as *const u64);
            core::ptr::write_unaligned(
                base.add(OFF_FRAMES_PUSHED) as *mut u64,
                pushed.wrapping_add(1),
            );
        }
        self.head = sequence.wrapping_add(1);
    }
}

static TX_RING: SyncCell<Option<MappedTxRing>> = SyncCell(UnsafeCell::new(None));

fn with_tx_ring<R>(f: impl FnOnce(&mut Option<MappedTxRing>) -> R) -> R {
    // SAFETY: single-threaded service, main-loop-only access.
    let cell = unsafe { &mut *TX_RING.0.get() };
    f(cell)
}

/// Negotiate the shared TX ring with the kernel and map it into this
/// service. Any failure leaves the legacy copied-transmit path in place.
/// Returns true when the shared path became active.
pub(crate) fn enable_shared_tx(packet_handle: rt::Handle) -> bool {
    if with_tx_ring(|ring| ring.is_some()) {
        return true;
    }
    let mut layout = PacketRingLayout {
        magic: 0,
        version: 0,
        slot_count: 0,
        slot_data_bytes: 0,
        slot_stride_bytes: 0,
        total_bytes: 0,
    };
    let memory_handle = match rt::packet_interface_tx_ring_setup(packet_handle, &mut layout) {
        Ok(handle) => handle,
        Err(_) => return false,
    };
    if layout.magic != RING_MAGIC || layout.version != RING_VERSION || layout.slot_count == 0 {
        return false;
    }
    let total_bytes = layout.total_bytes as usize;
    let mapped = match MappedMemory::map(memory_handle, total_bytes, true) {
        Ok(mapped) => mapped,
        Err(_) => return false,
    };
    let _ = rt::handle_close(memory_handle);
    let slots = layout.slot_count as usize;
    with_tx_ring(move |ring| {
        *ring = Some(MappedTxRing {
            mem: mapped,
            slot_count: slots,
            head: 0,
            stalled_doorbells: 0,
            disabled: false,
        });
    });
    true
}

/// Zero-copy statistics from the shared TX ring header (the kernel banks
/// tx-copies-avoided per completed transmit).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct TxRingSnapshot {
    pub(crate) active: bool,
    pub(crate) frames_pushed: u64,
    pub(crate) copies_avoided: u64,
    pub(crate) bytes_saved: u64,
}

pub(crate) fn tx_ring_snapshot() -> TxRingSnapshot {
    with_tx_ring(|ring| {
        let Some(ring) = ring.as_ref() else {
            return TxRingSnapshot::default();
        };
        let image = ring.image();
        TxRingSnapshot {
            active: true,
            frames_pushed: load_u64(image, OFF_FRAMES_PUSHED),
            copies_avoided: load_u64(image, OFF_COPIES_AVOIDED),
            bytes_saved: load_u64(image, OFF_BYTES_SAVED),
        }
    })
}

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
static NEIGHBORS: SyncCell<NeighborTable> = SyncCell(UnsafeCell::new(NeighborTable::new()));

/// Snooped ARP neighbor entries for the NEIGHBOR_DUMP diagnostic. This is a
/// passive observation table (whoever announced itself in an ARP frame we
/// saw), not the stack's own resolution cache.
#[derive(Clone, Copy)]
pub(crate) struct NeighborEntry {
    pub(crate) valid: bool,
    pub(crate) address: Ipv4Address,
    pub(crate) mac: [u8; 6],
}

struct NeighborTable {
    entries: [NeighborEntry; crate::consts::MAX_NEIGHBOR_ENTRIES],
}

impl NeighborTable {
    const fn new() -> Self {
        Self {
            entries: [NeighborEntry {
                valid: false,
                address: Ipv4Address::UNSPECIFIED,
                mac: [0; 6],
            }; crate::consts::MAX_NEIGHBOR_ENTRIES],
        }
    }

    fn note(&mut self, address: Ipv4Address, mac: [u8; 6]) {
        if address == Ipv4Address::UNSPECIFIED || address.is_broadcast() {
            return;
        }
        let mut free = None;
        for entry in &mut self.entries {
            if entry.valid && entry.address == address {
                entry.mac = mac;
                return;
            }
            if !entry.valid && free.is_none() {
                free = Some(entry);
            }
        }
        if let Some(entry) = free {
            entry.valid = true;
            entry.address = address;
            entry.mac = mac;
        }
    }
}

/// Copy of the snooped table; returns the number of valid entries.
pub(crate) fn neighbor_snapshot(out: &mut [NeighborEntry]) -> usize {
    // SAFETY: single-threaded service, main-loop-only access.
    let table = unsafe { &mut *NEIGHBORS.0.get() };
    let mut written = 0usize;
    for entry in &table.entries {
        if !entry.valid || written == out.len() {
            continue;
        }
        out[written] = *entry;
        written += 1;
    }
    written
}

fn snoop_arp_frame(frame: &[u8]) {
    let Ok(eth) = EthernetFrame::new_checked(frame) else {
        return;
    };
    if eth.ethertype() != smoltcp::wire::EthernetProtocol::Arp {
        return;
    }
    let Ok(arp) = ArpPacket::new_checked(eth.payload()) else {
        return;
    };
    let Ok(ArpRepr::EthernetIpv4 {
        source_protocol_addr,
        source_hardware_addr,
        ..
    }) = ArpRepr::parse(&arp)
    else {
        return;
    };
    // SAFETY: see static declarations above.
    unsafe {
        (*NEIGHBORS.0.get()).note(source_protocol_addr, source_hardware_addr.0);
    }
}

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
    frame: RxFrame<'a>,
}

/// Where the received frame's bytes live. `Local` is the legacy copied path
/// (syscall copy into `rx_buffer`); `Shared` borrows a slot of the mapped
/// RX ring directly, so every consumer parses the frame in place.
enum RxFrame<'a> {
    Local(&'a mut [u8]),
    Shared { ptr: NonNull<u8>, len: usize, sequence: u64 },
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
        // Guest-internal loopback first (unchanged legacy behavior).
        if let Some((frame, length)) = with_ring(|ring| ring.pop()) {
            self.rx_buffer[..length].copy_from_slice(&frame[..length]);
            snoop_arp_frame(&self.rx_buffer[..length]);
            return Some((
                KernelRxToken {
                    frame: RxFrame::Local(&mut self.rx_buffer[..length]),
                },
                KernelTxToken {
                    handle: self.handle,
                    buffer: &mut self.tx_buffer,
                },
            ));
        }

        let ring_attached = with_rx_ring(|ring| ring.is_some());

        if ring_attached {
            // Shared-ring path: claim a published slot in place. When the
            // ring is locally empty, one doorbell receive asks the kernel to
            // push the next backend frame into a slot (the returned length
            // describes the shared frame, NOT bytes in rx_buffer), then we
            // claim again.
            for _ in 0..2 {
                let sequence = with_rx_ring(|ring| {
                    ring.as_ref().and_then(MappedRxRing::next_sequence)
                });
                let Some(sequence) = sequence else {
                    match rt::packet_interface_receive_nonblocking(
                        self.handle,
                        &mut self.rx_buffer,
                    ) {
                        Ok(0) => return None,
                        Ok(_) => continue, // doorbell: frame published, go claim it
                        Err(_) => return None,
                    }
                };
                let claimed = with_rx_ring(|ring| {
                    // SAFETY: mapping lives for the process; single-threaded.
                    unsafe { ring.as_mut().and_then(|r| r.claim(sequence)) }
                });
                match claimed {
                    Some((ptr, len)) => {
                        let ptr = NonNull::new(ptr as *mut u8)?;
                        // SAFETY: claimed slot region inside the live mapping.
                        let frame =
                            unsafe { core::slice::from_raw_parts(ptr.as_ptr(), len) };
                        snoop_arp_frame(frame);
                        return Some((
                            KernelRxToken {
                                frame: RxFrame::Shared {
                                    ptr,
                                    len,
                                    sequence,
                                },
                            },
                            KernelTxToken {
                                handle: self.handle,
                                buffer: &mut self.tx_buffer,
                            },
                        ));
                    }
                    None => with_rx_ring(|ring| {
                        if let Some(ring) = ring.as_mut() {
                            ring.discard(sequence);
                        }
                    }),
                }
            }
            // Ring stayed empty even after a doorbell poll retry.
            return None;
        }

        // Legacy copied-frame path (no ring negotiated).
        let length =
            rt::packet_interface_receive_nonblocking(self.handle, &mut self.rx_buffer).ok()?;
        if length == 0 || length > MAX_FRAME_BYTES {
            return None;
        }
        snoop_arp_frame(&self.rx_buffer[..length]);
        Some((
            KernelRxToken {
                frame: RxFrame::Local(&mut self.rx_buffer[..length]),
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
        // Parse in place, then commit the ring sequence only after the hot
        // path is done with the borrowed bytes.
        match self.frame {
            RxFrame::Local(buffer) => f(buffer),
            RxFrame::Shared { ptr, len, sequence } => {
                // SAFETY: claimed slot region inside the live mapping.
                let result = f(unsafe { core::slice::from_raw_parts(ptr.as_ptr(), len) });
                with_rx_ring(|ring| {
                    if let Some(ring) = ring.as_mut() {
                        ring.commit(sequence, len);
                    }
                });
                result
            }
        }
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
            // Shared TX ring path: copy the emitted frame straight into the
            // next mapped slot (replacing the per-frame IPC copy through the
            // transmit syscall) and ring the kernel doorbell to drain it.
            // Any miss — ring not negotiated, disabled by a stalled kernel
            // side, backlog full, oversize frame — falls back to the legacy
            // copied transmit below.
            let mut revert_to_copied = false;
            let via_ring = with_tx_ring(|ring| {
                let Some(ring) = ring.as_mut() else {
                    return false;
                };
                if ring.disabled || frame.len() > MAX_FRAME_BYTES || !ring.has_room() {
                    return false;
                }
                let sequence = ring.head;
                // SAFETY: sequence indexes a live slot; mapping outlives use.
                let slot = unsafe { ring.slot_mut(sequence) };
                slot[..frame.len()].copy_from_slice(frame);
                ring.publish(sequence, frame.len());
                true
            });
            if via_ring {
                let _ = rt::packet_interface_tx_ring_flush(self.handle);
                with_tx_ring(|ring| {
                    let Some(ring) = ring.as_mut() else {
                        return;
                    };
                    if ring.kernel_tail() >= ring.head {
                        ring.stalled_doorbells = 0;
                    } else {
                        ring.stalled_doorbells += 1;
                        if ring.stalled_doorbells >= TX_STALL_DOORBELL_LIMIT {
                            ring.disabled = true;
                            revert_to_copied = true;
                        }
                    }
                });
                if revert_to_copied {
                    let _ = rt::write_logf(
                        "network",
                        format_args!(
                            "tx-ring kernel side not draining; reverted to copied-transmit path"
                        ),
                    );
                }
            } else {
                let _ = rt::packet_interface_transmit(self.handle, frame);
            }
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

//! Shared MSI-X bring-up for the platform's virtio PCI devices.
//!
//! Extracted from the virtio-net driver (commit 719cc30) so virtio-blk can
//! come up on its own vector with the same bring-up discipline. The device
//! side (PCI config + identity-mapped MMIO) lives here because it depends on
//! `virtio_drivers`' PCI types and x86 port I/O; the hardware-neutral
//! encoding stays in `serviceos_kernel_core::msi`.
//!
//! Sequence (PCI Local Bus spec 6.8 / virtio 1.0 4.1.4):
//!   1. parse the MSI-X capability (cap id 0x11) for the table BIR/offset
//!   2. locate the MSI-X table BAR and the virtio common-config region
//!   3. validate the caller's steering plan against the device table size
//!      (kernel-core `msi::validate_steering_plan`) and program each planned
//!      table entry with its LAPIC message (masked)
//!   4. set MSI-X Enable (Function Mask stays clear; every vector starts
//!      masked so the device cannot signal mid-setup)
//!   5. point every virtio queue at the plan's queue table entry in the
//!      common config, and config-change at its own entry when planned (the
//!      config-change vector stays NO_VECTOR only when the device table
//!      cannot hold a second entry — a documented degradation, not a failure)
//!   6. unmask every programmed entry
//!   7. set the PCI Command INTx-Disable bit so the legacy pin route cannot
//!      race the message-signaled path
//!
//! All fallible steps precede step 4, so a `Failed` return never leaves the
//! device half-switched over. The handler registration happens in the caller
//! before `PciTransport::new` lets the device negotiate.

use serviceos_kernel_core::msi;
use virtio_drivers::transport::pci::bus::{
    Command, ConfigurationAccess, DeviceFunction, PCI_CAP_ID_VNDR, PciRoot,
};
use x86_64::instructions::port::Port;

/// Build-time opt-out for the MSI-X interrupt model (SERVICEOS_MSIX_DISABLE
/// makes every device fall back to the legacy INT#x line exactly as before).
pub(crate) const MSI_X_DISABLED: bool = option_env!("SERVICEOS_MSIX_DISABLE").is_some();

/// Common-config structure field offsets (virtio 1.0 4.1.4.3 layout, as
/// mirrored by virtio-drivers' `CommonCfg`). The crate keeps those fields
/// `pub(crate)`, so queue-vector assignment goes through raw identity-mapped
/// MMIO instead of the public transport API.
pub(crate) const COMMON_CFG_NUM_QUEUES: u64 = 0x12;
pub(crate) const COMMON_CFG_QUEUE_SELECT: u64 = 0x16;
pub(crate) const COMMON_CFG_QUEUE_MSIX_VECTOR: u64 = 0x1a;
/// Common-config vector value meaning "no MSI-X signal" (config-change
/// stays unsignaled only when the device table cannot hold a second entry;
/// see the module comment).
pub(crate) const COMMON_CFG_NO_VECTOR: u16 = 0xffff;

/// Common-config offset of `msix_config` (virtio 1.0 4.1.4.3): the MSI-X
/// table entry that carries config-change events.
pub(crate) const COMMON_CFG_MSIX_CONFIG: u64 = 0x10;

/// MSI-X table entry each steering role owns. Queues share the first entry
/// (the vendored virtio-drivers 0.13 net driver aggregates rx/tx through one
/// device-wide `ack_interrupt` and exposes no per-queue IRQ callbacks, so
/// per-rx/tx vectors would steer two names at one handler); config-change
/// gets its own entry when the table has room.
pub(crate) const QUEUE_TABLE_INDEX: u16 = 0;
pub(crate) const CONFIG_TABLE_INDEX: u16 = 1;

/// Which arch vectors one device's MSI-X table should steer to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MsixSteeringPlan {
    /// Arch vector all virtio queues signal through (table entry 0).
    pub queue_vector: u8,
    /// Arch vector for config-change events (table entry 1). `None` keeps
    /// the device's config-change delivery off (NO_VECTOR).
    pub config_vector: Option<u8>,
}

/// The steering actually programmed on the device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MsixSteering {
    pub queue_vector: u8,
    /// `None` when the device's MSI-X table was too small to hold a second
    /// entry for config change.
    pub config_vector: Option<u8>,
}

pub(crate) const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;

/// Why MSI-X bring-up was skipped for one device. Stored by each driver in
/// its own diagnostic slot and surfaced once on the boot summary line as
/// `msix setup skipped reason=`.
pub(crate) type MsixSkipReason = &'static str;

/// Outcome of a device's MSI-X bring-up attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MsixOutcome {
    /// Build opt-out (SERVICEOS_MSIX_DISABLE): fall back silently, no
    /// diagnostic line.
    Disabled,
    /// MSI-X unavailable for the stated reason: fall back to the legacy
    /// INT#x path.
    Failed(MsixSkipReason),
    /// MSI-X programmed: the device signals per the steering until the
    /// handler registration and device negotiation complete.
    Ready(MsixSteering),
}

const PCI_CONFIG_ADDRESS_PORT: u16 = 0xCF8;
const PCI_CONFIG_DATA_PORT: u16 = 0xCFC;

#[derive(Clone, Copy)]
pub(crate) struct IoPortPciConfigAccess;

impl ConfigurationAccess for IoPortPciConfigAccess {
    fn read_word(&self, device_function: DeviceFunction, register_offset: u8) -> u32 {
        let address = pci_config_address(device_function, register_offset);
        let mut address_port = Port::<u32>::new(PCI_CONFIG_ADDRESS_PORT);
        let mut data_port = Port::<u32>::new(PCI_CONFIG_DATA_PORT);

        unsafe {
            address_port.write(address);
            data_port.read()
        }
    }

    fn write_word(&mut self, device_function: DeviceFunction, register_offset: u8, data: u32) {
        let address = pci_config_address(device_function, register_offset);
        let mut address_port = Port::<u32>::new(PCI_CONFIG_ADDRESS_PORT);
        let mut data_port = Port::<u32>::new(PCI_CONFIG_DATA_PORT);

        unsafe {
            address_port.write(address);
            data_port.write(data);
        }
    }

    unsafe fn unsafe_clone(&self) -> Self {
        *self
    }
}

fn pci_config_address(device_function: DeviceFunction, register_offset: u8) -> u32 {
    0x8000_0000
        | ((device_function.bus as u32) << 16)
        | ((device_function.device as u32) << 11)
        | ((device_function.function as u32) << 8)
        | (register_offset as u32 & 0xfc)
}

/// Configure the virtio device for MSI-X delivery per `plan` (arch vectors,
/// `MSI_VECTOR_BASE + slot`) and return [`MsixOutcome::Ready`] with the
/// steering actually programmed, or the reason MSI-X was skipped.
pub(crate) fn try_setup_msix(
    root: &mut PciRoot<IoPortPciConfigAccess>,
    device_function: DeviceFunction,
    plan: MsixSteeringPlan,
) -> MsixOutcome {
    if MSI_X_DISABLED {
        return MsixOutcome::Disabled;
    }

    let mut cam = IoPortPciConfigAccess;

    let cap_offset = match root
        .capabilities(device_function)
        .find(|capability| capability.id == msi::MSI_X_CAP_ID)
    {
        Some(capability) => capability.offset,
        None => return MsixOutcome::Failed("no-msix-cap"),
    };
    let cap = match msi::parse_msix_capability(
        |offset| cam.read_word(device_function, offset),
        cap_offset,
    ) {
        Some(cap) => cap,
        None => return MsixOutcome::Failed("cap-parse"),
    };
    if cap.table_size() < 1 {
        return MsixOutcome::Failed("table-size-0");
    }

    // Steer queues on entry 0; config change joins entry 1 only when the
    // device's table can hold it (validated against the table size before
    // anything is programmed). Too-small tables degrade to queues-only
    // rather than dropping MSI-X entirely.
    let queue_slot = msi::MsixSteeringSlot {
        table_index: QUEUE_TABLE_INDEX,
        vector: plan.queue_vector,
    };
    let config_slot = if plan.config_vector.is_some() && cap.table_size() >= 2 {
        Some(msi::MsixSteeringSlot {
            table_index: CONFIG_TABLE_INDEX,
            vector: plan.config_vector.unwrap_or(0),
        })
    } else {
        None
    };
    let slots = [queue_slot, config_slot.unwrap_or(queue_slot)];
    let slot_count = 1 + usize::from(config_slot.is_some());
    debug_assert_eq!(
        msi::validate_steering_plan(cap.table_size(), &slots[..slot_count]),
        Ok(())
    );

    let (bar_address, _) = match root
        .bar_info(device_function, cap.table_bir)
        .ok()
        .and_then(|bar| bar.and_then(|b| b.memory_address_size()))
    {
        Some(address) => address,
        None => return MsixOutcome::Failed("table-bar"),
    };
    let table_base = bar_address + u64::from(cap.table_offset);

    let common_base = match locate_common_config(&mut cam, root, device_function) {
        Some(base) => base,
        None => return MsixOutcome::Failed("no-common-cfg"),
    };

    // Program each planned entry masked: LAPIC physical destination 0 (BSP),
    // fixed delivery, edge trigger, on that role's arch MSI vector.
    for slot in &slots[..slot_count] {
        let entry = msi::MsixTableEntry::new_edge_fixed(0, slot.vector, true);
        write_msix_table_entry(table_base, slot.table_index, entry);
    }

    // Enable MSI-X with every vector still masked.
    let header = cam.read_word(device_function, cap_offset);
    let control = (header >> 16) as u16 | msi::MSI_X_MSG_CTRL_ENABLE;
    cam.write_word(
        device_function,
        cap_offset,
        (header & 0x0000_ffff) | (u32::from(control) << 16),
    );

    // Assign all virtio queues to the plan's queue table entry. The vendored
    // net driver aggregates rx/tx internally, so all queues share one vector
    // (per-queue handler disaggregation is a driver-side limit, not a
    // platform one).
    let num_queues = read_common_config(common_base, COMMON_CFG_NUM_QUEUES);
    for queue in 0..num_queues {
        write_common_config(common_base, COMMON_CFG_QUEUE_SELECT, queue);
        write_common_config(
            common_base,
            COMMON_CFG_QUEUE_MSIX_VECTOR,
            QUEUE_TABLE_INDEX as u16,
        );
    }
    // Config-change events go to their own entry when steered; NO_VECTOR
    // keeps them off otherwise (single-entry tables, block).
    let config_vector = if config_slot.is_some() {
        write_common_config(
            common_base,
            COMMON_CFG_MSIX_CONFIG,
            CONFIG_TABLE_INDEX as u16,
        );
        plan.config_vector
    } else {
        write_common_config(common_base, COMMON_CFG_MSIX_CONFIG, COMMON_CFG_NO_VECTOR);
        None
    };

    // Unmask every programmed entry: the device may now signal via the LAPIC.
    for slot in &slots[..slot_count] {
        let entry = msi::MsixTableEntry::new_edge_fixed(0, slot.vector, false);
        write_msix_table_entry(table_base, slot.table_index, entry);
    }

    // Kill the legacy INT#x pin route so interrupts arrive ONLY via MSI-X.
    let (_status, mut command) = root.get_status_command(device_function);
    command.insert(Command::INTERRUPT_DISABLE);
    root.set_command(device_function, command);

    MsixOutcome::Ready(MsixSteering {
        queue_vector: plan.queue_vector,
        config_vector,
    })
}

/// Find the virtio common-config structure's physical address by walking the
/// vendor capabilities (same shape `PciTransport::new` consumes).
fn locate_common_config(
    cam: &mut IoPortPciConfigAccess,
    root: &mut PciRoot<IoPortPciConfigAccess>,
    device_function: DeviceFunction,
) -> Option<u64> {
    for capability in root.capabilities(device_function) {
        if capability.id != PCI_CAP_ID_VNDR {
            continue;
        }
        // Bytes 2-3 of the capability header (carried in private_header by
        // the crate's iterator): struct length, then config type. The port-I/O
        // CAM only does dword access, so these bytes cannot be fetched with a
        // standalone word read at offset+2.
        let cap_len = capability.private_header as u8;
        let cfg_type = (capability.private_header >> 8) as u8;
        if cap_len < 16 || cfg_type != VIRTIO_PCI_CAP_COMMON_CFG {
            continue;
        }
        let bar = cam.read_word(device_function, capability.offset + 4) as u8;
        let bar_offset = cam.read_word(device_function, capability.offset + 8);
        let (bar_address, _) = root
            .bar_info(device_function, bar)
            .ok()?
            .and_then(|bar| bar.memory_address_size())?;
        return Some(bar_address + u64::from(bar_offset));
    }
    None
}

unsafe fn write_mmio_u32(address: u64, value: u32) {
    unsafe {
        core::ptr::write_volatile(address as *mut u32, value);
    }
}

unsafe fn read_mmio_u16(address: u64) -> u16 {
    unsafe { core::ptr::read_volatile(address as *const u16) }
}

unsafe fn write_mmio_u16(address: u64, value: u16) {
    unsafe {
        core::ptr::write_volatile(address as *mut u16, value);
    }
}

pub(crate) fn write_msix_table_entry(table_base: u64, index: u16, entry: msi::MsixTableEntry) {
    let entry_base = table_base + u64::from(msi::msix_table_entry_offset(index));
    let words = entry.to_words();
    unsafe {
        write_mmio_u32(entry_base, words[0]);
        write_mmio_u32(entry_base + 4, words[1]);
        write_mmio_u32(entry_base + 8, words[2]);
        // Vector control last: an unmask write takes effect only after the
        // address/data words are in place.
        write_mmio_u32(entry_base + 12, words[3]);
    }
}

fn read_common_config(base: u64, offset: u64) -> u16 {
    unsafe { read_mmio_u16(base + offset) }
}

fn write_common_config(base: u64, offset: u64, value: u16) {
    unsafe { write_mmio_u16(base + offset, value) }
}

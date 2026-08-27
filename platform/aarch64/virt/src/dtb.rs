use core::fmt;

use fdt::{
    Fdt,
    nodes::NodeProperty,
    properties::{Compatible, reg::Reg},
};
use serviceos_kernel_core::memory::PhysicalAddress;

use crate::uart::UartDescriptor;

const MAX_MEMORY_RANGES: usize = 8;
pub const MAX_VIRTIO_MMIO_DEVICES: usize = 32;

const VIRTIO_MMIO_COMPATIBLE: &str = "virtio,mmio";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VirtioMmioDevice {
    pub base: PhysicalAddress,
    pub size: usize,
    pub irq: u32,
}

impl VirtioMmioDevice {
    pub const EMPTY: Self = Self {
        base: PhysicalAddress::new(0),
        size: 0,
        irq: 0,
    };

    pub const fn is_populated(&self) -> bool {
        self.size > 0 && self.base.as_u64() != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceTreeStatus {
    pub parser_ready: bool,
    pub stdout_resolution: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceTreeError {
    InvalidBlob,
    MissingMemory,
    MissingStdoutNode,
    MissingStdoutReg,
    UnsupportedStdoutAddress,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryRange {
    pub start: PhysicalAddress,
    pub end: PhysicalAddress,
}

impl MemoryRange {
    pub const fn span_bytes(&self) -> u64 {
        self.end.as_u64().saturating_sub(self.start.as_u64())
    }
}

const REDISTRIBUTOR_MIN_SPAN_BYTES: u64 = 2 * 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterruptControllerRegions {
    pub distributor: MemoryRange,
    pub redistributors: MemoryRange,
}

impl InterruptControllerRegions {
    pub const fn is_usable(&self) -> bool {
        self.distributor.span_bytes() > 0
            && self.redistributors.span_bytes() >= REDISTRIBUTOR_MIN_SPAN_BYTES
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DeviceTreeBootInfo<'boot> {
    pub model: &'boot str,
    pub compatible: Option<&'boot str>,
    pub serial_number: Option<&'boot str>,
    pub dtb_base: PhysicalAddress,
    pub dtb_size: usize,
    pub memory_ranges: [MemoryRange; MAX_MEMORY_RANGES],
    pub memory_range_count: usize,
    pub memory_map_truncated: bool,
    pub stdout_uart: Option<UartDescriptor<'boot>>,
    pub interrupt_controller: Option<InterruptControllerRegions>,
    /// INTID of the non-secure physical timer PPI, taken from the
    /// `arm,armv8-timer` node's second interrupt specifier. The guest arms
    /// `cntp_cval_el0`, which asserts CNTPNSIRQ (PPI 14 -> INTID 30 on
    /// QEMU virt); the secure-physical PPI 13 (INTID 29) never fires.
    pub timer_ppi_intid: Option<u16>,
    pub virtio_mmio_devices: [VirtioMmioDevice; MAX_VIRTIO_MMIO_DEVICES],
    pub virtio_mmio_count: usize,
}

pub const fn status() -> DeviceTreeStatus {
    DeviceTreeStatus {
        parser_ready: true,
        stdout_resolution: true,
    }
}

pub fn parse(dtb_ptr: *const u8) -> Result<DeviceTreeBootInfo<'static>, DeviceTreeError> {
    let fdt =
        unsafe { Fdt::from_ptr_unaligned(dtb_ptr) }.map_err(|_| DeviceTreeError::InvalidBlob)?;
    let root = fdt.root();

    let mut memory_ranges = [MemoryRange {
        start: PhysicalAddress::new(0),
        end: PhysicalAddress::new(0),
    }; MAX_MEMORY_RANGES];
    let mut memory_range_count = 0usize;
    let mut memory_map_truncated = false;
    let mut found_any_memory = false;

    for node in root.find_all_nodes_with_name("memory") {
        let Some(reg) = node.property::<Reg>() else {
            continue;
        };
        for entry in reg.iter::<u64, u64>() {
            let entry = entry.map_err(|_| DeviceTreeError::MissingMemory)?;
            found_any_memory = true;
            if memory_range_count == memory_ranges.len() {
                memory_map_truncated = true;
                continue;
            }
            memory_ranges[memory_range_count] = MemoryRange {
                start: PhysicalAddress::new(entry.address),
                end: PhysicalAddress::new(entry.address.saturating_add(entry.len)),
            };
            memory_range_count += 1;
        }
    }

    if !found_any_memory {
        return Err(DeviceTreeError::MissingMemory);
    }

    let stdout_path = {
        let chosen = root.chosen();
        if let Some(stdout) = chosen.stdout_path() {
            if stdout.path().starts_with('/') {
                Some(stdout.path())
            } else {
                root.aliases()
                    .and_then(|aliases| aliases.resolve_name(stdout.path()))
            }
        } else {
            root.aliases()
                .and_then(|aliases| aliases.resolve_name("serial0"))
        }
    };

    let stdout_uart = if let Some(path) = stdout_path {
        let node = root
            .find_node(path)
            .ok_or(DeviceTreeError::MissingStdoutNode)?;
        let reg = node
            .property::<Reg>()
            .ok_or(DeviceTreeError::MissingStdoutReg)?;
        let entry = reg
            .iter::<u64, u64>()
            .next()
            .ok_or(DeviceTreeError::MissingStdoutReg)?
            .map_err(|_| DeviceTreeError::MissingStdoutReg)?;

        let mut translated = entry.address;
        let mut current_path = parent_path(path);
        while let Some(path_prefix) = current_path {
            if path_prefix == "/" {
                break;
            }
            let parent = root
                .find_node(path_prefix)
                .ok_or(DeviceTreeError::UnsupportedStdoutAddress)?;
            if let Some(ranges) = parent.ranges() {
                let mut matched = false;
                for range in ranges.iter::<u64, u64, u64>() {
                    let range = range.map_err(|_| DeviceTreeError::UnsupportedStdoutAddress)?;
                    let range_end = range
                        .child_bus_address
                        .checked_add(range.len)
                        .ok_or(DeviceTreeError::UnsupportedStdoutAddress)?;
                    if translated >= range.child_bus_address && translated < range_end {
                        translated = range
                            .parent_bus_address
                            .checked_add(translated - range.child_bus_address)
                            .ok_or(DeviceTreeError::UnsupportedStdoutAddress)?;
                        matched = true;
                        break;
                    }
                }
                if !matched && !ranges.iter::<u64, u64, u64>().next().is_none() {
                    return Err(DeviceTreeError::UnsupportedStdoutAddress);
                }
            }
            current_path = parent_path(path_prefix);
        }

        Some(UartDescriptor {
            path,
            base: PhysicalAddress::new(translated),
            span: entry.len as usize,
            compatible: node.property::<Compatible>().map(|value| value.first()),
        })
    } else {
        None
    };

    let mut virtio_mmio_devices = [VirtioMmioDevice::EMPTY; MAX_VIRTIO_MMIO_DEVICES];
    let mut virtio_mmio_count = 0usize;
    for (_, node) in root.all_nodes() {
        let Some(compatible) = node.property::<Compatible>() else {
            continue;
        };
        if !compatible.compatible_with(VIRTIO_MMIO_COMPATIBLE) {
            continue;
        }
        let Some(reg) = node.property::<Reg>() else {
            continue;
        };
        let Some(Ok(entry)) = reg.iter::<u64, u64>().next() else {
            continue;
        };
        let irq = node
            .raw_property("interrupts")
            .and_then(|property| decode_interrupt_intid(&property))
            .unwrap_or(0);
        if virtio_mmio_count == virtio_mmio_devices.len() {
            break;
        }
        virtio_mmio_devices[virtio_mmio_count] = VirtioMmioDevice {
            base: PhysicalAddress::new(entry.address),
            size: entry.len as usize,
            irq,
        };
        virtio_mmio_count += 1;
    }

    let interrupt_controller = (|| {
        for (_, node) in root.all_nodes() {
            let Some(compatible) = node.property::<Compatible>() else {
                continue;
            };
            if !compatible.compatible_with("arm,gic-v3") {
                continue;
            }
            let reg = node.property::<Reg>()?;
            let mut entries = reg.iter::<u64, u64>();
            let Some(Ok(distributor)) = entries.next() else {
                return None;
            };
            let Some(Ok(redistributors)) = entries.next() else {
                return None;
            };
            let regions = InterruptControllerRegions {
                distributor: MemoryRange {
                    start: PhysicalAddress::new(distributor.address),
                    end: PhysicalAddress::new(distributor.address.saturating_add(distributor.len)),
                },
                redistributors: MemoryRange {
                    start: PhysicalAddress::new(redistributors.address),
                    end: PhysicalAddress::new(
                        redistributors.address.saturating_add(redistributors.len),
                    ),
                },
            };
            return if regions.is_usable() {
                Some(regions)
            } else {
                None
            };
        }
        None
    })();

    // Non-secure physical timer PPI: the `arm,armv8-timer` node lists four
    // 3-cell specifiers (secure-phys, non-secure-phys, virtual, hyp). The
    // guest's `cntp_cval_el0` timer is the second one (PPI 14 -> INTID 30).
    let timer_ppi_intid = (|| {
        for (_, node) in root.all_nodes() {
            let Some(compatible) = node.property::<Compatible>() else {
                continue;
            };
            if !compatible.compatible_with("arm,armv8-timer") {
                continue;
            }
            let Some(property) = node.raw_property("interrupts") else {
                return None;
            };
            // Second specifier occupies bytes 12..24; the PPI number is its
            // second cell (bytes 16..20).
            let number = property.value.get(16..20).map(parse_be_u32)?;
            return Some(u16::try_from(16 + number).ok()?);
        }
        None
    })();

    Ok(DeviceTreeBootInfo {
        model: root.model(),
        compatible: Some(root.compatible().first()),
        serial_number: root.serial_number(),
        dtb_base: PhysicalAddress::new(dtb_ptr as u64),
        dtb_size: fdt.header().total_size as usize,
        memory_ranges,
        memory_range_count,
        memory_map_truncated,
        stdout_uart,
        interrupt_controller,
        timer_ppi_intid,
        virtio_mmio_devices,
        virtio_mmio_count,
    })
}

/// Decode a one-interrupt device-tree specifier into a GIC INTID. The first
/// cell selects the domain: 0 = SPI (INTID = 32 + number), 1 = PPI (INTID =
/// 16 + number). The previous parser returned the raw number cell, so every
/// virtio-mmio SPI was recorded 32 too low and the corresponding distributor
/// enable bit was never set.
fn decode_interrupt_intid(property: &NodeProperty) -> Option<u32> {
    let kind = property.value.get(..4).map(parse_be_u32)?;
    let number = property.value.get(4..8).map(parse_be_u32)?;
    let intid = match kind {
        0 => 32u32.checked_add(number)?,
        1 => 16u32.checked_add(number)?,
        _ => return None,
    };
    u16::try_from(intid).ok().map(u32::from)
}

fn parse_be_u32(value: &[u8]) -> u32 {
    u32::from_be_bytes([value[0], value[1], value[2], value[3]])
}

fn parent_path(path: &str) -> Option<&str> {
    if path == "/" {
        return None;
    }
    let trimmed = path.trim_end_matches('/');
    let index = trimmed.rfind('/')?;
    if index == 0 {
        Some("/")
    } else {
        Some(&trimmed[..index])
    }
}

impl fmt::Display for DeviceTreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

use core::fmt;

use fdt::{
    Fdt,
    properties::{Compatible, reg::Reg},
};
use serviceos_kernel_core::memory::PhysicalAddress;

use crate::uart::UartDescriptor;

const MAX_MEMORY_RANGES: usize = 8;

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
                    end: PhysicalAddress::new(
                        distributor.address.saturating_add(distributor.len),
                    ),
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
    })
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

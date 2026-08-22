use alloc::vec::Vec;
use serviceos_kernel_core::memory::PhysicalAddress;

const RSDP_SIGNATURE: &[u8; 8] = b"RSD PTR ";
const MADT_SIGNATURE: &[u8; 4] = b"APIC";
const XSDT_SIGNATURE: &[u8; 4] = b"XSDT";
const RSDT_SIGNATURE: &[u8; 4] = b"RSDT";

const MADT_ENTRY_TYPE_LOCAL_APIC: u8 = 0;
const LOCAL_APIC_FLAG_ENABLED: u32 = 1;

#[repr(C)]
struct RsdpDescriptor {
    signature: [u8; 8],
    checksum: u8,
    oem_id: [u8; 6],
    revision: u8,
    rsdt_address: u32,
    length: u32,
    xsdt_address: u64,
    extended_checksum: u8,
    _reserved: [u8; 3],
}

#[repr(C)]
pub(crate) struct SystemDescriptionTableHeader {
    signature: [u8; 4],
    length: u32,
    revision: u8,
    checksum: u8,
    _oem_id: [u8; 6],
    _oem_table_id: [u8; 8],
    _oem_revision: u32,
    _creator_id: u32,
    _creator_revision: u32,
}

#[repr(C)]
struct MadtLocalApicEntry {
    entry_type: u8,
    length: u8,
    _acpi_processor_id: u8,
    apic_id: u8,
    flags: u32,
}

/// # Safety
/// `address` must point at a readable ACPI table region through the kernel's
/// identity map, and no other code may be mutating that region.
unsafe fn rsdp_at(address: PhysicalAddress) -> Option<&'static RsdpDescriptor> {
    let rsdp = unsafe { &*(address.as_u64() as *const RsdpDescriptor) };
    if rsdp.signature != *RSDP_SIGNATURE {
        return None;
    }

    let bytes = unsafe { core::slice::from_raw_parts(address.as_u64() as *const u8, 20) };
    if bytes.iter().map(|&byte| byte as u32).sum::<u32>() as u8 != 0 {
        return None;
    }

    if rsdp.revision >= 2 {
        let full_bytes = unsafe {
            core::slice::from_raw_parts(
                address.as_u64() as *const u8,
                rsdp.length.min(4096) as usize,
            )
        };
        if full_bytes.iter().map(|&byte| byte as u32).sum::<u32>() as u8 != 0 {
            return None;
        }
    }

    Some(rsdp)
}

/// # Safety
/// The table at `physical` must be a readable, unmutated ACPI table header
/// plus payload through the identity map.
unsafe fn table_at(physical: u64) -> Option<&'static SystemDescriptionTableHeader> {
    let header = unsafe { &*(physical as *const SystemDescriptionTableHeader) };
    if header.length < core::mem::size_of::<SystemDescriptionTableHeader>() as u32
        || header.length > 1 << 20
    {
        return None;
    }
    Some(header)
}

unsafe fn child_tables<'a>(
    header: &'a SystemDescriptionTableHeader,
) -> impl Iterator<Item = u64> + 'a {
    let is_xsdt = header.signature == *XSDT_SIGNATURE;
    let base = header as *const SystemDescriptionTableHeader as u64;

    let header_size = core::mem::size_of::<SystemDescriptionTableHeader>() as u64;
    let entries_end = header.length as u64;
    (header_size..entries_end)
        .step_by(if is_xsdt { 8 } else { 4 })
        .filter_map(move |offset| {
            let pointer = if is_xsdt {
                unsafe { core::ptr::read_unaligned((base + offset) as *const u64) }
            } else {
                (unsafe { core::ptr::read_unaligned((base + offset) as *const u32) }) as u64
            };
            (pointer != 0).then_some(pointer)
        })
}

/// Walk RSDP → XSDT/RSDT looking for the Multiple APIC Description Table.
///
/// Returns `None` silently whenever any step fails (missing RSDP, missing
/// MADT), letting callers stay single-core without log noise.
pub(crate) fn madt_header(rsdp_address: Option<PhysicalAddress>) -> Option<&'static SystemDescriptionTableHeader> {
    let Some(addr) = rsdp_address else { return None; };
    let Some(rsdp) = (unsafe { rsdp_at(addr) }) else { return None; };

    let root_physical = if rsdp.revision >= 2 && rsdp.xsdt_address != 0 {
        rsdp.xsdt_address
    } else {
        rsdp.rsdt_address as u64
    };
    if root_physical == 0 {
        return None;
    }

    let Some(root) = (unsafe { table_at(root_physical) }) else { return None; };
    if root.signature != *XSDT_SIGNATURE && root.signature != *RSDT_SIGNATURE {
        return None;
    }

    unsafe {
        child_tables(root).find_map(|table| {
            let header = table_at(table)?;
            (header.signature == *MADT_SIGNATURE).then_some(header)
        })
    }
}

/// Enabled processor-local APIC IDs from the MADT, in table order.
///
/// The first entry corresponds to the bootstrap processor on every platform
/// this kernel targets (QEMU emits ascending APIC IDs with the BSP first).
pub fn enabled_lapic_ids(
    rsdp_address: Option<PhysicalAddress>,
) -> Option<Vec<u8>> {
    let madt = madt_header(rsdp_address)?;
    let base = madt as *const SystemDescriptionTableHeader as u64;
    let mut ids = Vec::new();

    // Interrupt-controller structure entries begin after the standard
    // table header plus the MADT-specific Local APIC address (u32) and
    // flags (u32) fields.
    const MADT_ENTRY_OFFSET: u32 = (core::mem::size_of::<SystemDescriptionTableHeader>()
        + core::mem::size_of::<u32>()
        + core::mem::size_of::<u32>()) as u32;

    let mut offset = MADT_ENTRY_OFFSET;
    while offset + core::mem::size_of::<MadtLocalApicEntry>() as u32 <= madt.length {
        let entry = unsafe { &*((base + offset as u64) as *const MadtLocalApicEntry) };
        if entry.length == 0 {
            break;
        }
        if entry.entry_type == MADT_ENTRY_TYPE_LOCAL_APIC
            && entry.flags & LOCAL_APIC_FLAG_ENABLED != 0
        {
            ids.push(entry.apic_id);
        }
        offset += entry.length as u32;
    }

    Some(ids)
}

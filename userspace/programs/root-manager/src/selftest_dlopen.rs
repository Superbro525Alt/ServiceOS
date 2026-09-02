//! Gated e2e witness for runtime-loaded libraries (`TaskLoadLibrary` +
//! `TaskSymbolLookup`). Inert unless the image was built with
//! `SERVICEOS_E2E_DLOPEN=1`; default boots never touch this path.
//!
//! The probe builds two minimal ELF64 `ET_DYN` shared objects in a memory
//! object each — a provider exporting a marker function and a consumer
//! importing it through a `R_X86_64_GLOB_DAT` GOT slot — loads them through
//! the real kernel path (plan → stage → relocate → map → register),
//! resolves both export addresses via `TaskSymbolLookup`, and *calls* the
//! consumer's entry, which calls the provider through its relocated GOT. A
//! passing call proves cross-module resolution and execution end-to-end.
//! Negative steps: a missing-symbol lookup and a non-ELF load both fail.
//!
//! Memory objects are zero-filled at creation, so the builder writes only
//! the non-zero regions (headers, code entry, dynamic tables) at their
//! fixed offsets; all fixture geometry is compile-time constant.

use core::arch::asm;
use serviceos_userspace_runtime as rt;

const CODE_VADDR: u64 = 0x1000;
const CODE_ENTRY: u64 = 0x1040;
const CODE_SEG_SIZE: u64 = 0x1000;
const DATA_VADDR: u64 = 0x4000;
const DATA_FILE_SIZE: u64 = 0x300;
const DATA_SEG_SIZE: u64 = 0x1000;
const GOT_SLOT_VADDR: u64 = 0x4800;
const MARKER_VALUE: u64 = 0x5a5a;

const DT_NULL: u64 = 0;
const DT_HASH: u64 = 4;
const DT_STRTAB: u64 = 5;
const DT_SYMTAB: u64 = 6;
const DT_RELA: u64 = 7;
const DT_RELASZ: u64 = 8;
const DT_STRSZ: u64 = 10;
const R_X86_64_GLOB_DAT: u64 = 6;
const STB_GLOBAL: u8 = 1;
const IMAGE_LEN: usize = (DATA_VADDR + DATA_FILE_SIZE) as usize;

/// `mov eax, imm32; ret` — the provider's marker function (padded to the
/// 10-byte code slot; `code_len` carries the real length).
const fn provider_code() -> [u8; 10] {
    [
        0xb8,
        (MARKER_VALUE & 0xff) as u8,
        ((MARKER_VALUE >> 8) & 0xff) as u8,
        ((MARKER_VALUE >> 16) & 0xff) as u8,
        ((MARKER_VALUE >> 24) & 0xff) as u8,
        0xc3,
        0,
        0,
        0,
        0,
    ]
}

/// `mov rax, [rip+rel32]; call rax; ret` — the consumer's entry calls the
/// provider through its GOT slot (patched by the kernel at load time).
const fn consumer_code() -> [u8; 10] {
    // rel32 is relative to the next instruction (CODE_ENTRY + 7).
    let rel32 = (GOT_SLOT_VADDR as i64 - (CODE_ENTRY + 7) as i64) as i32;
    let rel = rel32.to_le_bytes();
    [
        0x48, 0x8b, 0x05, rel[0], rel[1], rel[2], rel[3], 0xff, 0xd0, 0xc3,
    ]
}

struct LibrarySpec {
    /// (name, link-time st_value) exported definitions.
    exports: [(&'static str, u64); 1],
    /// Undefined import name and its dynsym index.
    import: Option<(&'static str, u32)>,
    /// (GOT slot vaddr, imported dynsym index) GLOB_DAT relocation.
    glob_dat: Option<(u64, u32)>,
    /// Function bytes copied at `CODE_ENTRY`.
    code: [u8; 10],
    code_len: usize,
}

/// Write the minimal ET_DYN fixture into `object` (already zero-filled):
/// ELF header + 3 program headers, executable LOAD at 0x1000 carrying the
/// entry, read-write LOAD at 0x4000 carrying dynsym/dynstr/DT_HASH plus an
/// optional `.rela.dyn`.
fn write_library(object: rt::Handle, spec: &LibrarySpec) -> Result<(), rt::Error> {
    let import_count = usize::from(spec.import.is_some());
    let symbol_count = 1 + spec.exports.len() + import_count;

    // dynstr: null + export names + import name.
    let mut strings = [0u8; 64];
    let mut strings_len = 1usize;
    let mut name_offsets = [0u32; 2];
    for (index, (name, _)) in spec.exports.iter().enumerate() {
        name_offsets[index] = strings_len as u32;
        for byte in name.as_bytes() {
            strings[strings_len] = *byte;
            strings_len += 1;
        }
        strings[strings_len] = 0;
        strings_len += 1;
    }
    let import_name_offset = if let Some((name, _)) = spec.import {
        let offset = strings_len as u32;
        for byte in name.as_bytes() {
            strings[strings_len] = *byte;
            strings_len += 1;
        }
        strings[strings_len] = 0;
        strings_len += 1;
        offset
    } else {
        0
    };

    // dynsym: null slot, export, import. Fixed order keeps the fixtures'
    // relocation symbol indices stable.
    let mut symbols = [0u8; 3 * 24];
    {
        let mut push_sym = |index: usize, name_off: u32, bind: u8, shndx: u16, value: u64| {
            let base = index * 24;
            symbols[base..base + 4].copy_from_slice(&name_off.to_le_bytes());
            symbols[base + 4] = bind << 4;
            symbols[base + 5] = 0;
            symbols[base + 6..base + 8].copy_from_slice(&shndx.to_le_bytes());
            symbols[base + 8..base + 16].copy_from_slice(&value.to_le_bytes());
            symbols[base + 16..base + 24].copy_from_slice(&0u64.to_le_bytes());
        };
        push_sym(0, 0, 0, 0, 0);
        for (index, (_, value)) in spec.exports.iter().enumerate() {
            push_sym(index + 1, name_offsets[index], STB_GLOBAL, 1, *value);
        }
        if let Some((_, symbol_index)) = spec.import {
            // The import's dynsym index is fixed by the caller's GLOB_DAT
            // row; with one export it is always slot 2.
            let _ = symbol_index;
            push_sym(1 + spec.exports.len(), import_name_offset, STB_GLOBAL, 0, 0);
        }
    }

    // SysV hash: one bucket; the chain walks import -> export so both are
    // reachable from the single bucket.
    let mut hash = [0u8; 8 + 4 + 4 * 3];
    {
        hash[0..4].copy_from_slice(&1u32.to_le_bytes());
        hash[4..8].copy_from_slice(&(symbol_count as u32).to_le_bytes());
        // buckets[0] = first symbol (1); chains[1] = 2; chains[2] = 0.
        hash[8..12].copy_from_slice(&1u32.to_le_bytes());
        let chains_base = 12;
        for index in 0..symbol_count {
            let next = if index + 1 < symbol_count {
                index + 1
            } else {
                0
            };
            hash[chains_base + index * 4..chains_base + index * 4 + 4]
                .copy_from_slice(&(next as u32).to_le_bytes());
        }
    }

    // Dynamic table at the start of the data segment.
    let strtab_off = 0x100u64;
    let symtab_off = strtab_off + strings_len as u64;
    let hash_off = symtab_off + (symbol_count * 24) as u64;
    let rela_off = hash_off + (8 + 4 + 4 * symbol_count as u64);
    let mut dynamic = [0u8; 7 * 16];
    let mut entries = 0usize;
    {
        let mut push_entry = |tag: u64, value: u64| {
            let base = entries * 16;
            dynamic[base..base + 8].copy_from_slice(&tag.to_le_bytes());
            dynamic[base + 8..base + 16].copy_from_slice(&value.to_le_bytes());
            entries += 1;
        };
        push_entry(DT_SYMTAB, DATA_VADDR + symtab_off);
        push_entry(DT_STRTAB, DATA_VADDR + strtab_off);
        push_entry(DT_STRSZ, strings_len as u64);
        push_entry(DT_HASH, DATA_VADDR + hash_off);
        if spec.glob_dat.is_some() {
            push_entry(DT_RELA, DATA_VADDR + rela_off);
            push_entry(DT_RELASZ, 24);
        }
        push_entry(DT_NULL, 0);
    }

    // One GLOB_DAT row: (GOT slot, symbol index), addend 0.
    let mut rela = [0u8; 24];
    if let Some((slot_vaddr, symbol_index)) = spec.glob_dat {
        rela[0..8].copy_from_slice(&slot_vaddr.to_le_bytes());
        rela[8..16]
            .copy_from_slice(&(R_X86_64_GLOB_DAT | ((symbol_index as u64) << 32)).to_le_bytes());
        rela[16..24].copy_from_slice(&0u64.to_le_bytes());
    }

    // Program headers: LOAD (RX code), LOAD (RW data), PT_DYNAMIC.
    let mut headers = [0u8; 64 + 3 * 56];
    {
        let image = &mut headers[..];
        image[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        image[4] = 2; // ELFCLASS64
        image[5] = 1; // ELFDATA2LSB
        image[6] = 1; // EV_CURRENT
        // e_type/e_machine at 16/18.
        image[16..18].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN
        image[18..20].copy_from_slice(&62u16.to_le_bytes()); // EM_X86_64
        image[24..32].copy_from_slice(&CODE_ENTRY.to_le_bytes()); // e_entry
        image[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
        image[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
        image[56..58].copy_from_slice(&3u16.to_le_bytes()); // e_phnum

        let mut phdr = |index: usize,
                        p_type: u32,
                        flags: u32,
                        offset: u64,
                        vaddr: u64,
                        filesz: u64,
                        memsz: u64| {
            let base = 64 + index * 56;
            image[base..base + 4].copy_from_slice(&p_type.to_le_bytes());
            image[base + 4..base + 8].copy_from_slice(&flags.to_le_bytes());
            image[base + 8..base + 16].copy_from_slice(&offset.to_le_bytes());
            image[base + 16..base + 24].copy_from_slice(&vaddr.to_le_bytes());
            image[base + 24..base + 32].copy_from_slice(&vaddr.to_le_bytes());
            image[base + 32..base + 40].copy_from_slice(&filesz.to_le_bytes());
            image[base + 40..base + 48].copy_from_slice(&memsz.to_le_bytes());
            image[base + 48..base + 56].copy_from_slice(&0x1000u64.to_le_bytes());
        };
        phdr(
            0,
            1,
            1 | 4,
            CODE_VADDR,
            CODE_VADDR,
            (CODE_ENTRY - CODE_VADDR) + spec.code_len as u64,
            CODE_SEG_SIZE,
        );
        phdr(
            1,
            1,
            2 | 4,
            DATA_VADDR,
            DATA_VADDR,
            DATA_FILE_SIZE,
            DATA_SEG_SIZE,
        );
        // PT_DYNAMIC spans exactly the entries built above (through DT_NULL).
        let dyn_size = (entries * 16) as u64;
        phdr(2, 2, 2 | 4, DATA_VADDR, DATA_VADDR, dyn_size, dyn_size);
    }

    rt::memory_write(object, 0, &headers)?;
    rt::memory_write(object, CODE_ENTRY as usize, &spec.code[..spec.code_len])?;

    // Data segment: dynamic table, strings, symbols, hash, then the one
    // GLOB_DAT row — each at its computed offset.
    let mut data_blob = [0u8; 0x300];
    data_blob[..entries * 16].copy_from_slice(&dynamic[..entries * 16]);
    data_blob[strtab_off as usize..strtab_off as usize + strings_len]
        .copy_from_slice(&strings[..strings_len]);
    data_blob[symtab_off as usize..symtab_off as usize + symbol_count * 24]
        .copy_from_slice(&symbols[..symbol_count * 24]);
    data_blob[hash_off as usize..hash_off as usize + 8 + 4 + 4 * symbol_count]
        .copy_from_slice(&hash[..8 + 4 + 4 * symbol_count]);
    if spec.glob_dat.is_some() {
        data_blob[rela_off as usize..rela_off as usize + 24].copy_from_slice(&rela);
    }
    rt::memory_write(object, DATA_VADDR as usize, &data_blob).map(|_| ())
}

fn log_probe(line: core::fmt::Arguments<'_>) {
    let _ = rt::write_logf("root-manager", line);
}

/// Call the resolved address with no arguments. The SysV (x86_64) and
/// AAPCS64 (aarch64) call conventions both pass nothing and return the
/// marker in rax/x0; the fixture functions are leaf calls either way.
#[cfg(target_arch = "x86_64")]
fn call_no_args(address: u64) -> u64 {
    let value: u64;
    unsafe {
        asm!(
            "call {target}",
            target = in(reg) address,
            lateout("rax") value,
            lateout("rcx") _, lateout("rdx") _,
            options(nostack)
        );
    }
    value
}

#[cfg(target_arch = "aarch64")]
fn call_no_args(address: u64) -> u64 {
    let value: u64;
    unsafe {
        asm!(
            "blr {target}",
            target = in(reg) address,
            lateout("x0") value,
            lateout("x30") _,
            options(nostack)
        );
    }
    value
}

fn load_fixture(spec: LibrarySpec) -> Result<rt::Handle, rt::Error> {
    let object = rt::memory_create(IMAGE_LEN, true)?;
    write_library(object, &spec)?;
    rt::load_library(object, 0)
}

/// Run the dlopen-shaped e2e probe. Fully inert (returns before any
/// output or syscall) unless built with SERVICEOS_E2E_DLOPEN=1. The
/// fixture bytes are x86_64 machine code, so non-x86 guests return
/// before touching anything (the syscall surface itself is arch-neutral).
pub(crate) fn run_probe() {
    if !matches!(option_env!("SERVICEOS_E2E_DLOPEN"), Some("1")) {
        return;
    }
    if !cfg!(target_arch = "x86_64") {
        return;
    }

    // Provider: exports probe_marker at its entry; six bytes of code.
    let provider_code = provider_code();
    let provider = LibrarySpec {
        exports: [("probe_marker", CODE_ENTRY)],
        import: None,
        glob_dat: None,
        code: provider_code,
        code_len: 6,
    };

    // Consumer: exports b_entry, imports probe_marker (dynsym index 2),
    // one GLOB_DAT reloc pointing its GOT slot at the import.
    let consumer = LibrarySpec {
        exports: [("b_entry", CODE_ENTRY)],
        import: Some(("probe_marker", 2)),
        glob_dat: Some((GOT_SLOT_VADDR, 2)),
        code: consumer_code(),
        code_len: 10,
    };

    let mut step = 1u32;
    macro_rules! expect_ok {
        ($result:expr) => {
            match $result {
                Ok(value) => value,
                Err(_) => {
                    log_probe(format_args!("E2E dlopen.loadcall FAIL step={step}"));
                    return;
                }
            }
        };
    }

    // Steps 1-2: load the provider, then the consumer — the consumer's
    // load resolves probe_marker against the provider registered one
    // step earlier.
    let provider_handle = expect_ok!(load_fixture(provider));
    step += 1;
    let consumer_handle = expect_ok!(load_fixture(consumer));
    step += 1;

    // Step 3: resolve both exports.
    let provider_fn = expect_ok!(rt::symbol_lookup(provider_handle, b"probe_marker"));
    let consumer_fn = expect_ok!(rt::symbol_lookup(consumer_handle, b"b_entry"));
    step += 1;

    // Step 4: call the consumer; it calls the provider through its
    // relocated GOT slot and the marker rides back in rax.
    let value = call_no_args(consumer_fn);
    if value != MARKER_VALUE {
        log_probe(format_args!(
            "E2E dlopen.loadcall FAIL step={step} value={value:#x}"
        ));
        return;
    }
    step += 1;

    // Step 5: the provider's own function returns the marker when called
    // directly.
    let direct = call_no_args(provider_fn);
    if direct != MARKER_VALUE {
        log_probe(format_args!(
            "E2E dlopen.loadcall FAIL step={step} direct={direct:#x}"
        ));
        return;
    }

    log_probe(format_args!(
        "E2E dlopen.loadcall PASS value={:#x} provider={:#x} consumer={:#x}",
        value, provider_fn, consumer_fn
    ));

    reject_probe();
}

/// Negative-path witness: a missing symbol must fail the lookup (NotFound)
/// and a non-ELF image must fail the load. Emits
/// `E2E dlopen.reject PASS|FAIL`.
fn reject_probe() {
    let provider_code = provider_code();
    let handle = match load_fixture(LibrarySpec {
        exports: [("present_fn", CODE_ENTRY)],
        import: None,
        glob_dat: None,
        code: provider_code,
        code_len: 6,
    }) {
        Ok(handle) => handle,
        Err(_) => {
            log_probe(format_args!("E2E dlopen.reject FAIL step=1"));
            return;
        }
    };
    if rt::symbol_lookup(handle, b"totally_missing").is_ok() {
        log_probe(format_args!("E2E dlopen.reject FAIL step=2"));
        return;
    }
    // A fresh zero-filled object carries no ELF magic.
    match rt::memory_create(64, true) {
        Ok(object) => {
            if rt::load_library(object, 0).is_ok() {
                log_probe(format_args!("E2E dlopen.reject FAIL step=3"));
                return;
            }
        }
        Err(_) => {
            log_probe(format_args!("E2E dlopen.reject FAIL step=3"));
            return;
        }
    }
    log_probe(format_args!("E2E dlopen.reject PASS"));
}

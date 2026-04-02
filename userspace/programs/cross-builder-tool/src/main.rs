#![no_std]
#![no_main]

use serviceos_userspace_runtime as rt;
use rt::{DeveloperArtifactFormat, DeveloperTarget, RawMessage};

const REPORT_TAG: u32 = 1;
const MAX_SOURCE: usize = 256;
const MAX_ARTIFACT: usize = 2048;
const MAX_NAME: usize = 64;
const FLAT_IMAGE_HEADER_LEN: usize = 72;
const IMAGE_BASE: u64 = 0x0000_4000_0000_0000;
const USER_STACK_TOP: u64 = 0x0000_7fff_ffff_0000;

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xfe01;
    }
    if startup.tag != rt::ControlTag::Startup as u32 || startup.handle_count < 3 || startup.word_count < 3 {
        return 0xfe02;
    }

    let output = startup.handles[0];
    let report = startup.handles[1];
    let source_handle = startup.handles[2];
    let target = match startup.words[0] as u32 {
        x if x == DeveloperTarget::LinuxX64 as u32 => DeveloperTarget::LinuxX64,
        x if x == DeveloperTarget::WindowsX64 as u32 => DeveloperTarget::WindowsX64,
        x if x == DeveloperTarget::MacosX64 as u32 => DeveloperTarget::MacosX64,
        _ => DeveloperTarget::NativeX64,
    };
    let mut source_len = startup.words[1] as usize;
    let name_len = startup.words[2] as usize;
    if name_len > MAX_NAME {
        return 0xfe03;
    }
    let mut artifact_name = [0u8; MAX_NAME];
    if rt::unpack_bytes(&startup.words[3..startup.word_count as usize], name_len, &mut artifact_name).is_err() {
        return 0xfe04;
    }

    let mut source = [0u8; MAX_SOURCE];
    if source_len > source.len() || rt::memory_read(source_handle, 0, &mut source[..source_len]).is_err() {
        return 0xfe05;
    }
    trim_message(&mut source, &mut source_len);

    let _ = rt::text_relay_write(output, "builder: loading workspace source\r\n");

    let mut artifact = [0u8; MAX_ARTIFACT];
    let (format, artifact_len, status_code) = match target {
        DeveloperTarget::NativeX64 => {
            let len = build_serviceos_flat(&source[..source_len], &mut artifact);
            (DeveloperArtifactFormat::ServiceOsFlat, len, 0u64)
        }
        DeveloperTarget::LinuxX64 => {
            let len = build_linux_elf(&source[..source_len], &mut artifact);
            (DeveloperArtifactFormat::Elf64, len, 0u64)
        }
        DeveloperTarget::WindowsX64 => {
            let len = build_windows_pe(&mut artifact);
            (DeveloperArtifactFormat::Pe32Plus, len, 0u64)
        }
        DeveloperTarget::MacosX64 => {
            let _ = rt::text_relay_write(output, "builder: macOS target requires future remote build/sign support\r\n");
            let _ = send_report(report, 1, DeveloperArtifactFormat::MachO64, 0, &artifact_name[..name_len], None);
            let _ = rt::handle_close(output);
            let _ = rt::handle_close(report);
            let _ = rt::handle_close(source_handle);
            return 0;
        }
    };

    let _ = rt::text_relay_write(output, "builder: emitting artifact bytes\r\n");
    let artifact_handle = match rt::memory_create(artifact_len, true) {
        Ok(handle) => handle,
        Err(_) => return 0xfe06,
    };
    if rt::memory_write(artifact_handle, 0, &artifact[..artifact_len]).is_err() {
        let _ = rt::handle_close(artifact_handle);
        return 0xfe07;
    }

    let _ = send_report(
        report,
        status_code,
        format,
        artifact_len,
        &artifact_name[..name_len],
        Some(artifact_handle),
    );
    let _ = rt::handle_close(artifact_handle);
    let _ = rt::text_relay_write(output, "builder: build complete\r\n");
    let _ = rt::handle_close(output);
    let _ = rt::handle_close(report);
    let _ = rt::handle_close(source_handle);
    0
}

fn trim_message(bytes: &mut [u8], len: &mut usize) {
    while *len > 0 && matches!(bytes[*len - 1], b'\n' | b'\r') {
        *len -= 1;
    }
}

fn send_report(
    report: rt::Handle,
    status_code: u64,
    format: DeveloperArtifactFormat,
    artifact_len: usize,
    name: &[u8],
    artifact_handle: Option<rt::Handle>,
) -> rt::Result<()> {
    let mut message = RawMessage::empty(REPORT_TAG);
    message.word_count = 4 + rt::pack_bytes(name, &mut message.words[4..])?;
    message.words[0] = status_code;
    message.words[1] = format as u32 as u64;
    message.words[2] = artifact_len as u64;
    message.words[3] = name.len() as u64;
    if let Some(handle) = artifact_handle {
        message.handle_count = 1;
        message.handles[0] = handle;
        message.handle_rights[0] = rt::rights::READ | rt::rights::DUPLICATE | rt::rights::TRANSFER;
    }
    rt::channel_send(report, &message)
}

fn build_serviceos_flat(message: &[u8], output: &mut [u8]) -> usize {
    let code_len = 28usize + message.len();
    let file_size = code_len as u64;
    let mut cursor = 0usize;
    output[cursor..cursor + 8].copy_from_slice(b"SOSUIMG\0");
    cursor += 8;
    output[cursor..cursor + 4].copy_from_slice(&1u32.to_le_bytes());
    cursor += 4;
    output[cursor..cursor + 4].copy_from_slice(&(FLAT_IMAGE_HEADER_LEN as u32).to_le_bytes());
    cursor += 4;
    output[cursor..cursor + 8].copy_from_slice(&IMAGE_BASE.to_le_bytes());
    cursor += 8;
    output[cursor..cursor + 8].copy_from_slice(&0u64.to_le_bytes());
    cursor += 8;
    output[cursor..cursor + 8].copy_from_slice(&file_size.to_le_bytes());
    cursor += 8;
    output[cursor..cursor + 8].copy_from_slice(&file_size.to_le_bytes());
    cursor += 8;
    output[cursor..cursor + 8].copy_from_slice(&file_size.to_le_bytes());
    cursor += 8;
    output[cursor..cursor + 8].copy_from_slice(&file_size.to_le_bytes());
    cursor += 8;
    output[cursor..cursor + 8].copy_from_slice(&USER_STACK_TOP.to_le_bytes());
    cursor += 8;

    let code_start = cursor;
    let message_offset = 21i32;
    output[cursor..cursor + 3].copy_from_slice(&[0x48, 0x8d, 0x3d]);
    cursor += 3;
    output[cursor..cursor + 4].copy_from_slice(&message_offset.to_le_bytes());
    cursor += 4;
    output[cursor] = 0xbe;
    cursor += 1;
    output[cursor..cursor + 4].copy_from_slice(&(message.len() as u32).to_le_bytes());
    cursor += 4;
    output[cursor] = 0xb8;
    cursor += 1;
    output[cursor..cursor + 4].copy_from_slice(&14u32.to_le_bytes());
    cursor += 4;
    output[cursor..cursor + 2].copy_from_slice(&[0xcd, 0x80]);
    cursor += 2;
    output[cursor..cursor + 2].copy_from_slice(&[0x31, 0xff]);
    cursor += 2;
    output[cursor] = 0xb8;
    cursor += 1;
    output[cursor..cursor + 4].copy_from_slice(&2u32.to_le_bytes());
    cursor += 4;
    output[cursor..cursor + 2].copy_from_slice(&[0xcd, 0x80]);
    cursor += 2;
    output[cursor..cursor + message.len()].copy_from_slice(message);
    cursor += message.len();
    debug_assert_eq!(cursor - code_start, code_len);
    cursor
}

fn build_linux_elf(message: &[u8], output: &mut [u8]) -> usize {
    let file_alignment = 0x10usize;
    let header_len = 0x78usize;
    let code_offset = header_len;
    let code_len = 38usize + message.len();
    let file_size = align_up(code_offset + code_len, file_alignment);
    output[..file_size].fill(0);

    output[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
    output[4] = 2;
    output[5] = 1;
    output[6] = 1;
    output[16..18].copy_from_slice(&2u16.to_le_bytes());
    output[18..20].copy_from_slice(&0x3eu16.to_le_bytes());
    output[20..24].copy_from_slice(&1u32.to_le_bytes());
    let image_base = 0x400000u64;
    let entry = image_base + code_offset as u64;
    output[24..32].copy_from_slice(&entry.to_le_bytes());
    output[32..40].copy_from_slice(&0x40u64.to_le_bytes());
    output[40..48].copy_from_slice(&0u64.to_le_bytes());
    output[48..52].copy_from_slice(&0u32.to_le_bytes());
    output[52..54].copy_from_slice(&64u16.to_le_bytes());
    output[54..56].copy_from_slice(&56u16.to_le_bytes());
    output[56..58].copy_from_slice(&1u16.to_le_bytes());

    let ph = 0x40usize;
    output[ph..ph + 4].copy_from_slice(&1u32.to_le_bytes());
    output[ph + 4..ph + 8].copy_from_slice(&5u32.to_le_bytes());
    output[ph + 8..ph + 16].copy_from_slice(&0u64.to_le_bytes());
    output[ph + 16..ph + 24].copy_from_slice(&image_base.to_le_bytes());
    output[ph + 24..ph + 32].copy_from_slice(&image_base.to_le_bytes());
    output[ph + 32..ph + 40].copy_from_slice(&(file_size as u64).to_le_bytes());
    output[ph + 40..ph + 48].copy_from_slice(&(file_size as u64).to_le_bytes());
    output[ph + 48..ph + 56].copy_from_slice(&0x1000u64.to_le_bytes());

    let mut cursor = code_offset;
    output[cursor] = 0xb8;
    cursor += 1;
    output[cursor..cursor + 4].copy_from_slice(&1u32.to_le_bytes());
    cursor += 4;
    output[cursor] = 0xbf;
    cursor += 1;
    output[cursor..cursor + 4].copy_from_slice(&1u32.to_le_bytes());
    cursor += 4;
    output[cursor..cursor + 3].copy_from_slice(&[0x48, 0x8d, 0x35]);
    cursor += 3;
    let rel = (38usize - 15) as i32;
    output[cursor..cursor + 4].copy_from_slice(&rel.to_le_bytes());
    cursor += 4;
    output[cursor] = 0xba;
    cursor += 1;
    output[cursor..cursor + 4].copy_from_slice(&(message.len() as u32).to_le_bytes());
    cursor += 4;
    output[cursor..cursor + 2].copy_from_slice(&[0x0f, 0x05]);
    cursor += 2;
    output[cursor] = 0xb8;
    cursor += 1;
    output[cursor..cursor + 4].copy_from_slice(&60u32.to_le_bytes());
    cursor += 4;
    output[cursor..cursor + 2].copy_from_slice(&[0x31, 0xff]);
    cursor += 2;
    output[cursor..cursor + 2].copy_from_slice(&[0x0f, 0x05]);
    cursor += 2;
    output[cursor..cursor + message.len()].copy_from_slice(message);
    file_size
}

fn build_windows_pe(output: &mut [u8]) -> usize {
    const FILE_ALIGN: usize = 0x200;
    const SECTION_ALIGN: usize = 0x1000;
    const IMAGE_BASE: u64 = 0x140000000;
    const HEADERS_SIZE: usize = 0x200;
    const SECTION_RVA: u32 = 0x1000;
    const SECTION_RAW: usize = 0x200;

    output[..0x400].fill(0);
    output[0..2].copy_from_slice(b"MZ");
    output[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    output[0x80..0x84].copy_from_slice(b"PE\0\0");
    output[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
    output[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
    output[0x94..0x96].copy_from_slice(&0x00f0u16.to_le_bytes());
    output[0x96..0x98].copy_from_slice(&0x0022u16.to_le_bytes());

    let opt = 0x98usize;
    output[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes());
    output[opt + 16..opt + 20].copy_from_slice(&0x1000u32.to_le_bytes());
    output[opt + 20..opt + 24].copy_from_slice(&SECTION_RVA.to_le_bytes());
    output[opt + 24..opt + 32].copy_from_slice(&IMAGE_BASE.to_le_bytes());
    output[opt + 32..opt + 36].copy_from_slice(&(SECTION_ALIGN as u32).to_le_bytes());
    output[opt + 36..opt + 40].copy_from_slice(&(FILE_ALIGN as u32).to_le_bytes());
    output[opt + 56..opt + 60].copy_from_slice(&(0x2000u32).to_le_bytes());
    output[opt + 60..opt + 64].copy_from_slice(&(HEADERS_SIZE as u32).to_le_bytes());
    output[opt + 68..opt + 70].copy_from_slice(&3u16.to_le_bytes());
    output[opt + 80..opt + 88].copy_from_slice(&(0x100000u64).to_le_bytes());
    output[opt + 88..opt + 96].copy_from_slice(&(0x1000u64).to_le_bytes());
    output[opt + 96..opt + 104].copy_from_slice(&(0x100000u64).to_le_bytes());
    output[opt + 104..opt + 112].copy_from_slice(&(0x1000u64).to_le_bytes());
    output[opt + 108..opt + 112].copy_from_slice(&0u32.to_le_bytes());
    output[opt + 112..opt + 116].copy_from_slice(&16u32.to_le_bytes());
    output[opt + 120..opt + 124].copy_from_slice(&(0x1100u32).to_le_bytes());
    output[opt + 124..opt + 128].copy_from_slice(&(0x28u32).to_le_bytes());

    let sh = 0x188usize;
    output[sh..sh + 5].copy_from_slice(b".text");
    output[sh + 8..sh + 12].copy_from_slice(&0x200u32.to_le_bytes());
    output[sh + 12..sh + 16].copy_from_slice(&SECTION_RVA.to_le_bytes());
    output[sh + 16..sh + 20].copy_from_slice(&(SECTION_RAW as u32).to_le_bytes());
    output[sh + 20..sh + 24].copy_from_slice(&(HEADERS_SIZE as u32).to_le_bytes());
    output[sh + 36..sh + 40].copy_from_slice(&0x60000020u32.to_le_bytes());

    let text = HEADERS_SIZE;
    let code_rva = SECTION_RVA as usize;
    let iat_rva = code_rva + 0x20;
    let import_desc_rva = code_rva + 0x40;
    let int_rva = code_rva + 0x68;
    let name_rva = code_rva + 0x78;
    let dll_rva = code_rva + 0x88;

    let mut cursor = text;
    output[cursor..cursor + 3].copy_from_slice(&[0x48, 0x31, 0xc9]);
    cursor += 3;
    output[cursor..cursor + 3].copy_from_slice(&[0x48, 0x8b, 0x05]);
    cursor += 3;
    let disp = (iat_rva as isize - (code_rva as isize + 9)) as i32;
    output[cursor..cursor + 4].copy_from_slice(&disp.to_le_bytes());
    cursor += 4;
    output[cursor..cursor + 2].copy_from_slice(&[0xff, 0xe0]);

    let iat_off = text + 0x20;
    output[iat_off..iat_off + 8].copy_from_slice(&(name_rva as u64).to_le_bytes());
    let import_desc_off = text + 0x40;
    output[import_desc_off..import_desc_off + 4].copy_from_slice(&(int_rva as u32).to_le_bytes());
    output[import_desc_off + 12..import_desc_off + 16].copy_from_slice(&(dll_rva as u32).to_le_bytes());
    output[import_desc_off + 16..import_desc_off + 20].copy_from_slice(&(iat_rva as u32).to_le_bytes());
    let int_off = text + 0x68;
    output[int_off..int_off + 8].copy_from_slice(&(name_rva as u64).to_le_bytes());
    let name_off = text + 0x78;
    output[name_off..name_off + 2].copy_from_slice(&0u16.to_le_bytes());
    output[name_off + 2..name_off + 14].copy_from_slice(b"ExitProcess\0");
    let dll_off = text + 0x88;
    output[dll_off..dll_off + 13].copy_from_slice(b"KERNEL32.dll\0");
    0x400
}

fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

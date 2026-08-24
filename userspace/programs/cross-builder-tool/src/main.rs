#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

use rt::{DeveloperArtifactFormat, DeveloperTarget, RawMessage};
use serviceos_userspace_runtime as rt;

const REPORT_TAG: u32 = 1;
const MAX_SOURCE: usize = 256;
const MAX_ARTIFACT: usize = 2048;
const MAX_NAME: usize = 64;
/// Builder-report status codes shared with `developer-service`:
/// 0 = ok, 1 = unsupported target, 2 = generic failure, 3 = sandbox denial.
const STATUS_SANDBOX_DENIED: u64 = 3;
const MAX_SANDBOX_TEXT: usize = 512;
const MAX_SCOPES: usize = 4;
const MAX_SCOPE_LEN: usize = 96;
const FLAT_IMAGE_HEADER_LEN: usize = 72;
const IMAGE_BASE: u64 = 0x0000_4000_0000_0000;
const USER_STACK_TOP: u64 = 0x0000_7fff_ffff_0000;

#[cfg(not(test))]
rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xfe01;
    }
    if startup.tag != rt::ControlTag::Startup as u32
        || startup.handle_count < 4
        || startup.word_count < 3
    {
        return 0xfe02;
    }

    let output = startup.handles[0];
    let report = startup.handles[1];
    let source_handle = startup.handles[2];
    let sandbox_handle = startup.handles[3];
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
    if rt::unpack_bytes(
        &startup.words[3..startup.word_count as usize],
        name_len,
        &mut artifact_name,
    )
    .is_err()
    {
        return 0xfe04;
    }

    let mut sandbox_text = [0u8; MAX_SANDBOX_TEXT];
    if rt::memory_read(sandbox_handle, 0, &mut sandbox_text).is_err() {
        return 0xfe10;
    }
    let Some(sandbox) = parse_sandbox_text(&sandbox_text) else {
        return 0xfe11;
    };

    // One-line worker sandbox echo for the build log.
    log_sandbox_line(output, &sandbox);

    // Runtime-aware routing echo: the launcher appends a route word after
    // the packed artifact name; absent word means legacy direct spawn.
    let route = decode_startup_route(startup.word_count as usize, name_len, &startup.words);
    log_route_line(output, &route);

    if !sandbox.net_denied
        || !path_in_scopes(&sandbox.scopes[..sandbox.scope_count], &sandbox.request_in)
        || !path_in_scopes(&sandbox.scopes[..sandbox.scope_count], &sandbox.request_out)
    {
        // Requested path outside the granted fs scopes: fail the job cleanly
        // with a distinct sandbox-denied status instead of proceeding.
        let _ = send_report(
            report,
            STATUS_SANDBOX_DENIED,
            DeveloperArtifactFormat::ServiceOsFlat,
            0,
            &artifact_name[..name_len],
            None,
        );
        let _ = rt::handle_close(output);
        let _ = rt::handle_close(report);
        let _ = rt::handle_close(source_handle);
        let _ = rt::handle_close(sandbox_handle);
        return 0;
    }

    let mut source = [0u8; MAX_SOURCE];
    if source_len > source.len()
        || rt::memory_read(source_handle, 0, &mut source[..source_len]).is_err()
    {
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
            let _ = rt::text_relay_write(
                output,
                "builder: macOS target requires future remote build/sign support\r\n",
            );
            let _ = send_report(
                report,
                1,
                DeveloperArtifactFormat::MachO64,
                0,
                &artifact_name[..name_len],
                None,
            );
            let _ = rt::handle_close(output);
            let _ = rt::handle_close(report);
            let _ = rt::handle_close(source_handle);
            let _ = rt::handle_close(sandbox_handle);
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
    let _ = rt::handle_close(sandbox_handle);
    0
}

fn trim_message(bytes: &mut [u8], len: &mut usize) {
    while *len > 0 && matches!(bytes[*len - 1], b'\n' | b'\r') {
        *len -= 1;
    }
}

#[derive(Clone, Copy)]
struct ScopeBuf {
    bytes: [u8; MAX_SCOPE_LEN],
    len: usize,
}

impl ScopeBuf {
    const fn empty() -> Self {
        Self {
            bytes: [0; MAX_SCOPE_LEN],
            len: 0,
        }
    }

    fn set(&mut self, value: &[u8]) -> bool {
        if value.len() > MAX_SCOPE_LEN {
            return false;
        }
        self.bytes[..value.len()].copy_from_slice(value);
        self.len = value.len();
        true
    }

    fn as_bytes(&self) -> &[u8] {
        let mut end = self.len;
        while end > 1 && self.bytes[end - 1] == b'/' {
            end -= 1;
        }
        &self.bytes[..end]
    }
}

struct SandboxSpec {
    scopes: [ScopeBuf; MAX_SCOPES],
    scope_count: usize,
    net_denied: bool,
    request_in: ScopeBuf,
    request_out: ScopeBuf,
}

/// Parse the permission-set text sent by developer-service:
/// `fs=<scope>;<scope>` / `net=<denied|allowed>` / `in=<path>` / `out=<path>`.
fn parse_sandbox_text(text: &[u8]) -> Option<SandboxSpec> {
    let mut spec = SandboxSpec {
        scopes: [ScopeBuf::empty(); MAX_SCOPES],
        scope_count: 0,
        net_denied: false,
        request_in: ScopeBuf::empty(),
        request_out: ScopeBuf::empty(),
    };
    let mut have_in = false;
    let mut have_out = false;
    let text_end = text
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(text.len());
    for line in core::str::from_utf8(&text[..text_end]).ok()?.lines() {
        let (key, value) = line.split_once('=')?;
        match key {
            "fs" => {
                for scope in value.split(';') {
                    if scope.is_empty()
                        || spec.scope_count >= MAX_SCOPES
                        || !spec.scopes[spec.scope_count].set(scope.as_bytes())
                    {
                        return None;
                    }
                    spec.scope_count += 1;
                }
            }
            "net" => spec.net_denied = value == "denied",
            "in" => {
                have_in = spec.request_in.set(value.as_bytes());
            }
            "out" => {
                have_out = spec.request_out.set(value.as_bytes());
            }
            _ => {}
        }
    }
    if spec.scope_count == 0 || !have_in || !have_out {
        return None;
    }
    Some(spec)
}

fn path_in_scopes(scopes: &[ScopeBuf], path: &ScopeBuf) -> bool {
    let path = path.as_bytes();
    if path.is_empty() {
        return false;
    }
    scopes.iter().map(ScopeBuf::as_bytes).any(|scope| {
        !scope.is_empty()
            && path.starts_with(scope)
            && (path.len() == scope.len() || path[scope.len()] == b'/')
    })
}

fn log_sandbox_line(output: rt::Handle, sandbox: &SandboxSpec) {
    let mut line = [0u8; MAX_SANDBOX_TEXT];
    let mut cursor = push_chunk(&mut line, 0, b"worker sandbox: fs=[");
    for index in 0..MAX_SCOPES.min(sandbox.scope_count) {
        if index > 0 {
            cursor = push_chunk(&mut line, cursor, b";");
        }
        cursor = push_chunk(&mut line, cursor, sandbox.scopes[index].as_bytes());
    }
    cursor = push_chunk(&mut line, cursor, b"] net=denied\r\n");
    if let Ok(text) = core::str::from_utf8(&line[..cursor]) {
        let _ = rt::text_relay_write(output, text);
    }
}

fn push_chunk(line: &mut [u8], cursor: usize, chunk: &[u8]) -> usize {
    if cursor + chunk.len() > line.len() {
        return cursor;
    }
    line[cursor..cursor + chunk.len()].copy_from_slice(chunk);
    cursor + chunk.len()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkerRoute {
    Direct,
    RuntimeEnv(u32),
}

/// Decode the optional startup payload route word: nonzero encodes a
/// runtime-service environment as env_id + 1; zero or missing = direct.
fn decode_startup_route(word_count: usize, name_len: usize, words: &[u64]) -> WorkerRoute {
    let route_index = 3 + name_len.div_ceil(8);
    if word_count <= route_index {
        return WorkerRoute::Direct;
    }
    match words.get(route_index).copied().unwrap_or(0) {
        0 => WorkerRoute::Direct,
        encoded => WorkerRoute::RuntimeEnv(encoded.saturating_sub(1) as u32),
    }
}

fn log_route_line(output: rt::Handle, route: &WorkerRoute) {
    let mut line = [0u8; 64];
    let mut cursor = push_chunk(&mut line, 0, b"worker route: ");
    cursor = match *route {
        WorkerRoute::Direct => push_chunk(&mut line, cursor, b"direct\r\n"),
        WorkerRoute::RuntimeEnv(env_id) => {
            cursor = push_chunk(&mut line, cursor, b"runtime-env ");
            let mut digits = [0u8; 10];
            let mut count = 0usize;
            let mut value = env_id;
            loop {
                digits[count] = b'0' + (value % 10) as u8;
                count += 1;
                value /= 10;
                if value == 0 {
                    break;
                }
            }
            for byte in digits[..count].iter().rev() {
                cursor = push_chunk(&mut line, cursor, core::slice::from_ref(byte));
            }
            push_chunk(&mut line, cursor, b"\r\n")
        }
    };
    if let Ok(text) = core::str::from_utf8(&line[..cursor]) {
        let _ = rt::text_relay_write(output, text);
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
    output[import_desc_off + 12..import_desc_off + 16]
        .copy_from_slice(&(dll_rva as u32).to_le_bytes());
    output[import_desc_off + 16..import_desc_off + 20]
        .copy_from_slice(&(iat_rva as u32).to_le_bytes());
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

#[cfg(test)]
mod tests {
    use super::*;

    const HOST_TEXT: &[u8] = b"\
fs=packages/developer-service/1.0.0/projects/hello-cross;\
packages/developer-service/1.0.0/sdk/native\n\
net=denied\n\
in=packages/developer-service/1.0.0/projects/hello-cross/message.txt\n\
out=packages/developer-service/1.0.0/projects/hello-cross/hello-cross\n";

    fn parse(text: &[u8]) -> SandboxSpec {
        parse_sandbox_text(text).expect("sandbox text parses")
    }

    #[test]
    fn host_serialized_text_parses() {
        let spec = parse(HOST_TEXT);
        assert_eq!(spec.scope_count, 2);
        assert!(spec.net_denied);
        assert_eq!(
            spec.request_in.as_bytes(),
            b"packages/developer-service/1.0.0/projects/hello-cross/message.txt"
        );
        assert_eq!(
            path_in_scopes(&spec.scopes[..spec.scope_count], &spec.request_in),
            true
        );
        assert_eq!(
            path_in_scopes(&spec.scopes[..spec.scope_count], &spec.request_out),
            true
        );
    }

    #[test]
    fn out_of_scope_request_is_rejected() {
        let mut spec = parse(HOST_TEXT);
        assert!(spec
            .request_out
            .set(b"packages/elsewhere/escape.bin"));
        assert!(!path_in_scopes(
            &spec.scopes[..spec.scope_count],
            &spec.request_out
        ));
    }

    #[test]
    fn sibling_prefix_is_not_contained() {
        let mut spec = parse(HOST_TEXT);
        assert!(spec
            .request_in
            .set(b"packages/developer-service/1.0.0/projects/hello-crossx/f.txt"));
        assert!(!path_in_scopes(
            &spec.scopes[..spec.scope_count],
            &spec.request_in
        ));
    }

    #[test]
    fn net_allowed_fails_validation_gate() {
        let text = *b"fs=ws/src\nnet=allowed\nin=ws/src/a.txt\nout=ws/src/a.out\n";
        let end = text.iter().position(|byte| *byte == 0).unwrap_or(text.len());
        let spec = parse(&text[..end]);
        assert!(!spec.net_denied);
        assert!(path_in_scopes(&spec.scopes[..spec.scope_count], &spec.request_in));
    }

    #[test]
    fn malformed_text_is_rejected() {
        for bad in [
            &b""[..],
            b"net=denied\nin=x\nout=y\n",           // no scopes
            b"fs=ws/src\nnet=denied\nin=x\n",       // no out
            b"fs=\nnet=denied\nin=x\nout=y\n",      // empty scope value
            b"garbage without equals\n",            // no key=value lines
            &[0u8; 8][..],                          // nul-terminated empty
        ] {
            assert!(parse_sandbox_text(bad).is_none(), "expected reject: {bad:?}");
        }
    }

    #[test]
    fn scope_trailing_slash_normalized_for_containment() {
        let spec = parse(b"fs=ws/src/\nnet=denied\nin=ws/src/deep/a.txt\nout=ws/src/o\n");
        assert!(path_in_scopes(
            &spec.scopes[..spec.scope_count],
            &spec.request_in
        ));
        assert!(path_in_scopes(
            &spec.scopes[..spec.scope_count],
            &spec.request_out
        ));
    }

    #[test]
    fn sandbox_log_line_matches_contract() {
        let spec = parse(HOST_TEXT);
        let output_handle: rt::Handle = 0;
        let _ = output_handle; // log_sandbox_line writes via relay; shape checked here
        let mut line = [0u8; MAX_SANDBOX_TEXT];
        let mut cursor = push_chunk(&mut line, 0, b"worker sandbox: fs=[");
        for index in 0..MAX_SCOPES.min(spec.scope_count) {
            if index > 0 {
                cursor = push_chunk(&mut line, cursor, b";");
            }
            cursor = push_chunk(&mut line, cursor, spec.scopes[index].as_bytes());
        }
        cursor = push_chunk(&mut line, cursor, b"] net=denied\r\n");
        let rendered = core::str::from_utf8(&line[..cursor]).unwrap();
        assert!(rendered.starts_with("worker sandbox: fs=["));
        assert!(rendered.contains(';'));
        assert!(rendered.ends_with("] net=denied\r\n"));
    }

    #[test]
    fn log_line_single_scope_has_no_separator() {
        let spec = parse(b"fs=ws/src\nnet=denied\nin=ws/src/a\nout=ws/src/b\n");
        let mut line = [0u8; MAX_SANDBOX_TEXT];
        let mut cursor = push_chunk(&mut line, 0, b"worker sandbox: fs=[");
        for index in 0..MAX_SCOPES.min(spec.scope_count) {
            if index > 0 {
                cursor = push_chunk(&mut line, cursor, b";");
            }
            cursor = push_chunk(&mut line, cursor, spec.scopes[index].as_bytes());
        }
        cursor = push_chunk(&mut line, cursor, b"] net=denied\r\n");
        let rendered = core::str::from_utf8(&line[..cursor]).unwrap();
        assert_eq!(rendered, "worker sandbox: fs=[ws/src] net=denied\r\n");
    }
}

#[cfg(test)]
mod route_tests {
    use super::*;

    fn payload(name_len: usize, extra: &[u64]) -> (usize, Vec<u64>) {
        let packed = name_len.div_ceil(8);
        let mut words = vec![0u64; 3 + packed];
        words.extend_from_slice(extra);
        (words.len(), words)
    }

    #[test]
    fn missing_route_word_means_direct() {
        let (word_count, words) = payload(11, &[]);
        assert_eq!(
            decode_startup_route(word_count, 11, &words),
            WorkerRoute::Direct
        );
    }

    #[test]
    fn zero_route_word_means_direct() {
        let (word_count, words) = payload(11, &[0]);
        assert_eq!(
            decode_startup_route(word_count, 11, &words),
            WorkerRoute::Direct
        );
    }

    #[test]
    fn nonzero_route_word_decodes_env_id() {
        let (word_count, words) = payload(11, &[4]);
        assert_eq!(
            decode_startup_route(word_count, 11, &words),
            WorkerRoute::RuntimeEnv(3)
        );
    }

    #[test]
    fn route_word_position_follows_packed_name_length() {
        // name_len exactly word-aligned vs not must both land on the right
        // slot.
        for name_len in [8usize, 9usize, 16usize, 17usize] {
            let packed = name_len.div_ceil(8);
            let mut words = vec![0u64; 3 + packed];
            words.push(7);
            assert_eq!(
                decode_startup_route(words.len(), name_len, &words),
                WorkerRoute::RuntimeEnv(6)
            );
        }
    }
}

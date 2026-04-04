use std::{error::Error, fs, path::Path, process::Command};

use super::{
    programs::Program,
    toolchain::{AARCH64_USER_TARGET, X86_64_USER_TARGET, llvm_tool, parse_hex},
};

const IMAGE_BASE: u64 = 0x0000_4000_0000_0000;
const USER_STACK_TOP: u64 = 0x0000_7fff_ffff_0000;
const FLAT_IMAGE_HEADER_LEN: usize = 72;

pub(crate) struct FlatImageLayout {
    pub(crate) executable_limit: u64,
    pub(crate) writable_offset: u64,
    pub(crate) memory_size: u64,
}

pub(crate) fn build_program(
    programs_root: &Path,
    target_dir: &Path,
    profile: &str,
    user_target: &str,
    program: &Program,
) -> Result<(), Box<dyn Error>> {
    let link_script = programs_root.join("link.ld");
    let mut command = Command::new("cargo");
    command.current_dir(programs_root);
    command.env("CARGO_TARGET_DIR", target_dir);
    command.args([
        "rustc",
        "--target",
        user_target,
        "-p",
        program.package,
        "--bin",
        program.bin_name,
    ]);
    if profile == "release" {
        command.arg("--release");
    }
    command.args(["--", "-C", "relocation-model=static"]);
    if user_target == X86_64_USER_TARGET {
        command.args(["-C", "code-model=large"]);
        command.args(["-C", "target-feature=-mmx,-sse,-sse2,+soft-float"]);
    } else if user_target != AARCH64_USER_TARGET {
        return Err(format!("unsupported userspace target: {user_target}").into());
    }
    command.args([
        "-C",
        &format!("link-arg=-T{}", link_script.display()),
        "-C",
        "link-arg=--gc-sections",
    ]);
    let status = command.status()?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("failed to build {}", program.package).into())
    }
}

pub(crate) fn read_flat_image_layout(elf: &Path) -> Result<FlatImageLayout, Box<dyn Error>> {
    let output = Command::new(llvm_tool("LLVM_READELF", "llvm-readelf"))
        .args(["-l"])
        .arg(elf)
        .output()?;
    if !output.status.success() {
        return Err(format!("llvm-readelf failed for {}", elf.display()).into());
    }

    let stdout = String::from_utf8(output.stdout)?;
    let mut base = None;
    let mut executable_limit = 0u64;
    let mut writable_offset = u64::MAX;
    let mut memory_limit = 0u64;

    for line in stdout.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("LOAD") {
            continue;
        }
        let fields: Vec<&str> = trimmed.split_whitespace().collect();
        if fields.len() < 8 {
            continue;
        }

        let virt_addr = parse_hex(fields[2])?;
        let file_size = parse_hex(fields[4])?;
        let mem_size = parse_hex(fields[5])?;
        let flags = fields[6..fields.len() - 1].join("");

        let image_base = *base.get_or_insert(virt_addr);
        if virt_addr < image_base {
            return Err(
                format!("unexpected non-monotonic load segment in {}", elf.display()).into(),
            );
        }

        let segment_offset = virt_addr.saturating_sub(image_base);
        if flags.contains('E') {
            executable_limit = executable_limit.max(segment_offset.saturating_add(mem_size));
        }
        if flags.contains('W') {
            writable_offset = writable_offset.min(segment_offset);
        }
        memory_limit = memory_limit.max(segment_offset.saturating_add(mem_size));
        let _ = file_size;
    }

    let Some(image_base) = base else {
        return Err(format!("no load segments found in {}", elf.display()).into());
    };
    if image_base != IMAGE_BASE {
        return Err(format!(
            "unexpected image base {image_base:#x} for {}, expected {IMAGE_BASE:#x}",
            elf.display()
        )
        .into());
    }

    Ok(FlatImageLayout {
        executable_limit,
        writable_offset: if writable_offset == u64::MAX {
            memory_limit
        } else {
            writable_offset
        },
        memory_size: memory_limit,
    })
}

pub(crate) fn wrap_flat_image(
    raw: &Path,
    output: &Path,
    layout: &FlatImageLayout,
) -> Result<(), Box<dyn Error>> {
    let code = fs::read(raw)?;
    let file_size = code.len() as u64;
    let memory_size = layout.memory_size.max(file_size);
    let executable_limit = layout.executable_limit.min(memory_size);
    let writable_offset = layout.writable_offset.min(memory_size);
    let mut image = Vec::with_capacity(FLAT_IMAGE_HEADER_LEN + code.len());
    image.extend_from_slice(b"SOSUIMG\0");
    image.extend_from_slice(&1u32.to_le_bytes());
    image.extend_from_slice(&(FLAT_IMAGE_HEADER_LEN as u32).to_le_bytes());
    image.extend_from_slice(&IMAGE_BASE.to_le_bytes());
    image.extend_from_slice(&0u64.to_le_bytes());
    image.extend_from_slice(&file_size.to_le_bytes());
    image.extend_from_slice(&executable_limit.to_le_bytes());
    image.extend_from_slice(&writable_offset.to_le_bytes());
    image.extend_from_slice(&memory_size.to_le_bytes());
    image.extend_from_slice(&USER_STACK_TOP.to_le_bytes());
    image.extend_from_slice(&code);
    fs::write(output, image)?;
    Ok(())
}

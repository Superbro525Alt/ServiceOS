use std::{
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serviceos_bundle::{
    BOOT_STORE_MAGIC, BOOT_STORE_PATH_MAX, BOOT_STORE_VERSION, BootStoreEntryKind,
    BootStoreEntryRecord, BootStoreHeader, parse_package_manifest,
};

const IMAGE_BASE: u64 = 0x0000_4000_0000_0000;
const USER_STACK_TOP: u64 = 0x0000_7fff_ffff_0000;
const FLAT_IMAGE_HEADER_LEN: usize = 72;
const X86_64_USER_TARGET: &str = "x86_64-unknown-none";
const AARCH64_USER_TARGET: &str = "aarch64-unknown-none-softfloat";

const PROGRAMS: &[Program] = &[
    Program {
        package: "serviceos-root-service-manager",
        bin_name: "serviceos-root-service-manager",
        image_id: 1,
        service_path: "services/root-manager/program.img",
        service_id: 1,
    },
    Program {
        package: "serviceos-storage-service",
        bin_name: "serviceos-storage-service",
        image_id: 2,
        service_path: "services/storage-service/program.img",
        service_id: 2,
    },
    Program {
        package: "serviceos-console-service",
        bin_name: "serviceos-console-service",
        image_id: 3,
        service_path: "services/console-service/program.img",
        service_id: 3,
    },
    Program {
        package: "serviceos-config-service",
        bin_name: "serviceos-config-service",
        image_id: 4,
        service_path: "services/config-service/program.img",
        service_id: 4,
    },
    Program {
        package: "serviceos-log-service",
        bin_name: "serviceos-log-service",
        image_id: 5,
        service_path: "services/log-service/program.img",
        service_id: 5,
    },
    Program {
        package: "serviceos-status-service",
        bin_name: "serviceos-status-service",
        image_id: 6,
        service_path: "services/status-service/program.img",
        service_id: 6,
    },
    Program {
        package: "serviceos-shell-service",
        bin_name: "serviceos-shell-service",
        image_id: 7,
        service_path: "services/shell-service/program.img",
        service_id: 7,
    },
    Program {
        package: "serviceos-sysinfo-tool",
        bin_name: "serviceos-sysinfo-tool",
        image_id: 8,
        service_path: "tools/sysinfo-tool/program.img",
        service_id: 0,
    },
    Program {
        package: "serviceos-package-service",
        bin_name: "serviceos-package-service",
        image_id: 9,
        service_path: "services/package-service/program.img",
        service_id: 8,
    },
    Program {
        package: "serviceos-announce-service",
        bin_name: "serviceos-announce-service",
        image_id: 10,
        service_path: "services/announce-service/program.img",
        service_id: 9,
    },
    Program {
        package: "serviceos-network-service",
        bin_name: "serviceos-network-service",
        image_id: 11,
        service_path: "services/network-service/program.img",
        service_id: 10,
    },
    Program {
        package: "serviceos-audio-service",
        bin_name: "serviceos-audio-service",
        image_id: 20,
        service_path: "services/audio-service/program.img",
        service_id: 15,
    },
    Program {
        package: "serviceos-runtime-service",
        bin_name: "serviceos-runtime-service",
        image_id: 21,
        service_path: "services/runtime-service/program.img",
        service_id: 16,
    },
    Program {
        package: "serviceos-developer-service",
        bin_name: "serviceos-developer-service",
        image_id: 23,
        service_path: "services/developer-service/program.img",
        service_id: 17,
    },
    Program {
        package: "serviceos-clipboard-service",
        bin_name: "serviceos-clipboard-service",
        image_id: 25,
        service_path: "services/clipboard-service/program.img",
        service_id: 18,
    },
    Program {
        package: "serviceos-security-service",
        bin_name: "serviceos-security-service",
        image_id: 27,
        service_path: "services/security-service/program.img",
        service_id: 19,
    },
    Program {
        package: "serviceos-graphics-service",
        bin_name: "serviceos-graphics-service",
        image_id: 12,
        service_path: "services/graphics-service/program.img",
        service_id: 11,
    },
    Program {
        package: "serviceos-session-service",
        bin_name: "serviceos-session-service",
        image_id: 13,
        service_path: "services/session-service/program.img",
        service_id: 12,
    },
    Program {
        package: "serviceos-desktop-shell-service",
        bin_name: "serviceos-desktop-shell-service",
        image_id: 14,
        service_path: "services/desktop-shell-service/program.img",
        service_id: 13,
    },
    Program {
        package: "serviceos-settings-app",
        bin_name: "serviceos-settings-app",
        image_id: 15,
        service_path: "apps/settings-app/program.img",
        service_id: 0,
    },
    Program {
        package: "serviceos-files-app",
        bin_name: "serviceos-files-app",
        image_id: 16,
        service_path: "apps/files-app/program.img",
        service_id: 0,
    },
    Program {
        package: "serviceos-monitor-app",
        bin_name: "serviceos-monitor-app",
        image_id: 17,
        service_path: "apps/monitor-app/program.img",
        service_id: 0,
    },
    Program {
        package: "serviceos-terminal-service",
        bin_name: "serviceos-terminal-service",
        image_id: 18,
        service_path: "services/terminal-service/program.img",
        service_id: 14,
    },
    Program {
        package: "serviceos-terminal-app",
        bin_name: "serviceos-terminal-app",
        image_id: 19,
        service_path: "apps/terminal-app/program.img",
        service_id: 0,
    },
    Program {
        package: "serviceos-software-center-app",
        bin_name: "serviceos-software-center-app",
        image_id: 26,
        service_path: "apps/software-center-app/program.img",
        service_id: 0,
    },
    Program {
        package: "serviceos-posix-host-tool",
        bin_name: "serviceos-posix-host-tool",
        image_id: 22,
        service_path: "tools/posix-host-tool/program.img",
        service_id: 0,
    },
    Program {
        package: "serviceos-cross-builder-tool",
        bin_name: "serviceos-cross-builder-tool",
        image_id: 24,
        service_path: "tools/cross-builder-tool/program.img",
        service_id: 0,
    },
];

struct Program {
    package: &'static str,
    bin_name: &'static str,
    image_id: u32,
    service_path: &'static str,
    service_id: u32,
}

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let repo_root = manifest_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let programs_root = repo_root.join("userspace").join("programs");
    let bundles_root = repo_root.join("userspace").join("bundles");
    let profile = env::var("PROFILE")?;
    let user_target =
        env::var("SERVICEOS_USER_TARGET").unwrap_or_else(|_| X86_64_USER_TARGET.to_owned());
    let target_dir = repo_root.join("target").join("userspace-programs");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let bootstore_output = target_dir
        .join(&user_target)
        .join(&profile)
        .join("bootstore.bin");

    println!("cargo:rerun-if-changed={}", programs_root.display());
    println!("cargo:rerun-if-changed={}", bundles_root.display());

    validate_package_manifests(&bundles_root)?;

    fs::create_dir_all(&target_dir)?;
    fs::create_dir_all(&out_dir)?;
    fs::create_dir_all(bootstore_output.parent().unwrap())?;

    let mut entries = Vec::new();
    for program in PROGRAMS {
        build_program(&programs_root, &target_dir, &profile, &user_target, program)?;
        let elf = target_dir
            .join(&user_target)
            .join(&profile)
            .join(program.bin_name);
        let raw = out_dir.join(format!("{}.bin", program.bin_name));
        let image = out_dir.join(format!("{}.img", program.bin_name));
        objcopy_binary(&elf, &raw)?;
        let layout = read_flat_image_layout(&elf)?;
        wrap_flat_image(&raw, &image, &layout)?;
        entries.push(BootStoreEntry {
            kind: BootStoreEntryKind::Executable,
            service_id: program.service_id,
            image_id: program.image_id,
            path: program.service_path.to_string(),
            bytes: fs::read(&image)?,
        });
    }

    for path in collect_bundle_files(&bundles_root)? {
        let kind = if path.ends_with("/manifest.svc") {
            BootStoreEntryKind::Manifest
        } else {
            BootStoreEntryKind::Data
        };
        let relative = path
            .strip_prefix(&bundles_root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        entries.push(BootStoreEntry {
            kind,
            service_id: 0,
            image_id: 0,
            path: relative,
            bytes: fs::read(&path)?,
        });
    }

    let bootstore = encode_bootstore(&entries)?;
    fs::write(&bootstore_output, &bootstore)?;

    let generated = format!(
        "pub static BOOT_STORE_IMAGE: &[u8] = include_bytes!(r#\"{}\"#);\n",
        bootstore_output.display()
    );
    fs::write(out_dir.join("catalog.rs"), generated)?;

    Ok(())
}

fn validate_package_manifests(bundles_root: &Path) -> Result<(), Box<dyn Error>> {
    for path in collect_bundle_files(bundles_root)? {
        if path.extension().and_then(|value| value.to_str()) != Some("pkg") {
            continue;
        }
        let bytes = fs::read(&path)?;
        let manifest = parse_package_manifest(&bytes)
            .map_err(|error| format!("invalid package manifest {}: {:?}", path.display(), error))?;
        let actual = compute_package_integrity(bundles_root, &manifest)?;
        if manifest.integrity != actual {
            return Err(format!(
                "package integrity mismatch for {}: manifest=0x{:016x} actual=0x{:016x}",
                path.display(),
                manifest.integrity,
                actual
            )
            .into());
        }
    }
    Ok(())
}

fn compute_package_integrity(
    bundles_root: &Path,
    manifest: &serviceos_bundle::PackageManifest,
) -> Result<u64, Box<dyn Error>> {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for content in manifest.contents[..manifest.content_count].iter() {
        let path = content
            .as_str()
            .map_err(|_| "package content path is not valid utf-8")?;
        update_fnv64(&mut hash, path.as_bytes());
        let bytes = fs::read(bundles_root.join(path))?;
        update_fnv64(&mut hash, &bytes);
    }
    Ok(hash)
}

fn update_fnv64(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes.iter().copied() {
        *hash ^= byte as u64;
        *hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
}

struct BootStoreEntry {
    kind: BootStoreEntryKind,
    service_id: u32,
    image_id: u32,
    path: String,
    bytes: Vec<u8>,
}

fn build_program(
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

fn objcopy_binary(input: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    let status = Command::new(llvm_tool("LLVM_OBJCOPY", "llvm-objcopy"))
        .args(["-O", "binary"])
        .arg(input)
        .arg(output)
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("llvm-objcopy failed for {}", input.display()).into())
    }
}

struct FlatImageLayout {
    executable_limit: u64,
    writable_offset: u64,
    memory_size: u64,
}

fn read_flat_image_layout(elf: &Path) -> Result<FlatImageLayout, Box<dyn Error>> {
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

fn llvm_tool(env_var: &str, binary: &str) -> PathBuf {
    env::var_os(env_var)
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .or_else(|| find_in_path(binary))
        .or_else(|| {
            [
                format!("/usr/bin/{binary}"),
                format!("/usr/sbin/{binary}"),
                format!("/usr/lib/llvm-18/bin/{binary}"),
                format!("/usr/lib/llvm-17/bin/{binary}"),
                format!("/usr/lib/llvm-16/bin/{binary}"),
            ]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| path.exists())
        })
        .unwrap_or_else(|| PathBuf::from(binary))
}

fn find_in_path(binary: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|dir| dir.join(binary))
            .find(|candidate| candidate.exists())
    })
}

fn parse_hex(value: &str) -> Result<u64, Box<dyn Error>> {
    Ok(u64::from_str_radix(value.trim_start_matches("0x"), 16)?)
}

fn wrap_flat_image(
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

fn collect_bundle_files(root: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut files = Vec::new();
    visit_bundle_dir(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn visit_bundle_dir(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();
        if entry.file_type()?.is_dir() {
            visit_bundle_dir(&entry_path, files)?;
        } else {
            files.push(entry_path);
        }
    }
    Ok(())
}

fn encode_bootstore(entries: &[BootStoreEntry]) -> Result<Vec<u8>, Box<dyn Error>> {
    let header_len = BootStoreHeader::encoded_len();
    let entry_len = BootStoreEntryRecord::encoded_len();
    let table_offset = header_len;
    let data_offset = table_offset + entry_len * entries.len();
    let total_len = data_offset + entries.iter().map(|entry| entry.bytes.len()).sum::<usize>();
    let mut image = vec![0u8; total_len];

    image[..8].copy_from_slice(&BOOT_STORE_MAGIC);
    image[8..12].copy_from_slice(&BOOT_STORE_VERSION.to_le_bytes());
    image[12..16].copy_from_slice(&(entries.len() as u32).to_le_bytes());
    image[16..20].copy_from_slice(&(table_offset as u32).to_le_bytes());
    image[20..24].copy_from_slice(&(entry_len as u32).to_le_bytes());

    let mut cursor = data_offset;
    for (index, entry) in entries.iter().enumerate() {
        let entry_offset = table_offset + index * entry_len;
        let entry_end = entry_offset + entry_len;
        let record = &mut image[entry_offset..entry_end];
        record[0..4].copy_from_slice(&(entry.kind as u32).to_le_bytes());
        record[4..8].copy_from_slice(&entry.service_id.to_le_bytes());
        record[8..12].copy_from_slice(&entry.image_id.to_le_bytes());
        record[12..16].copy_from_slice(&0u32.to_le_bytes());
        record[16..20].copy_from_slice(&(cursor as u32).to_le_bytes());
        record[20..24].copy_from_slice(&(entry.bytes.len() as u32).to_le_bytes());

        let path_bytes = entry.path.as_bytes();
        if path_bytes.len() > BOOT_STORE_PATH_MAX {
            return Err(format!("boot-store path too long: {}", entry.path).into());
        }
        record[24..26].copy_from_slice(&(path_bytes.len() as u16).to_le_bytes());
        record[28..28 + path_bytes.len()].copy_from_slice(path_bytes);
        image[cursor..cursor + entry.bytes.len()].copy_from_slice(&entry.bytes);
        cursor += entry.bytes.len();
    }

    Ok(image)
}

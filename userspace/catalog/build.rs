use std::{
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serviceos_bundle::{
    BOOT_STORE_MAGIC, BOOT_STORE_PATH_MAX, BOOT_STORE_VERSION, BootStoreEntryKind,
    BootStoreEntryRecord, BootStoreHeader,
};

const IMAGE_BASE: u64 = 0x0000_4000_0000_0000;
const USER_STACK_TOP: u64 = 0x0000_7fff_ffff_0000;

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
    let target_dir = repo_root.join("target").join("userspace-programs");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let bootstore_output = target_dir.join(&profile).join("bootstore.bin");

    println!("cargo:rerun-if-changed={}", programs_root.display());
    println!("cargo:rerun-if-changed={}", bundles_root.display());

    fs::create_dir_all(&target_dir)?;
    fs::create_dir_all(&out_dir)?;
    fs::create_dir_all(bootstore_output.parent().unwrap())?;

    let mut entries = Vec::new();
    for program in PROGRAMS {
        build_program(&programs_root, &target_dir, program)?;
        let elf = target_dir
            .join("x86_64-unknown-none")
            .join(&profile)
            .join(program.bin_name);
        let raw = out_dir.join(format!("{}.bin", program.bin_name));
        let image = out_dir.join(format!("{}.img", program.bin_name));
        objcopy_binary(&elf, &raw)?;
        wrap_flat_image(&raw, &image)?;
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
    program: &Program,
) -> Result<(), Box<dyn Error>> {
    let link_script = programs_root.join("link.ld");
    let status = Command::new("cargo")
        .current_dir(programs_root)
        .env("CARGO_TARGET_DIR", target_dir)
        .args([
            "rustc",
            "--target",
            "x86_64-unknown-none",
            "-p",
            program.package,
            "--bin",
            program.bin_name,
            "--",
            "-C",
            "relocation-model=static",
            "-C",
            "code-model=large",
            "-C",
            &format!("link-arg=-T{}", link_script.display()),
            "-C",
            "link-arg=--gc-sections",
        ])
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("failed to build {}", program.package).into())
    }
}

fn objcopy_binary(input: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    let status = Command::new("/usr/sbin/llvm-objcopy")
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

fn wrap_flat_image(raw: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    let code = fs::read(raw)?;
    let mut image = Vec::with_capacity(48 + code.len());
    image.extend_from_slice(b"SOSUIMG\0");
    image.extend_from_slice(&1u32.to_le_bytes());
    image.extend_from_slice(&48u32.to_le_bytes());
    image.extend_from_slice(&IMAGE_BASE.to_le_bytes());
    image.extend_from_slice(&0u64.to_le_bytes());
    image.extend_from_slice(&(code.len() as u64).to_le_bytes());
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

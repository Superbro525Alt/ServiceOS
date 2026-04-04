mod build_support;

use std::{env, error::Error, fs, path::PathBuf};

use build_support::{
    bootstore::{BootStoreEntry, encode_bootstore},
    bundle::{collect_bundle_files, validate_package_manifests},
    image::{build_program, read_flat_image_layout, wrap_flat_image},
    programs::PROGRAMS,
    toolchain::objcopy_binary,
};
use serviceos_bundle::BootStoreEntryKind;

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
    let user_target = env::var("SERVICEOS_USER_TARGET")
        .unwrap_or_else(|_| build_support::toolchain::X86_64_USER_TARGET.to_owned());
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

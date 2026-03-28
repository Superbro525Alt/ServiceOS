use std::{
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    build::{BuildArtifacts, ensure_success},
    bundle::stage_platform_bundle,
    platform::ImageKind,
};

pub fn create_platform_image(artifacts: &BuildArtifacts) -> Result<PathBuf, Box<dyn Error>> {
    let layout = stage_platform_bundle(artifacts)?;
    match artifacts.spec.image_kind {
        ImageKind::RawDisk => create_qemu_disk_image(&layout.root_dir),
        ImageKind::RaspberryPiBundle => create_raspi_bundle(artifacts, &layout),
    }
}

fn create_qemu_disk_image(esp_dir: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let img_path = esp_dir.parent().unwrap().join("serviceos.img");
    let size_mb = 64;

    println!("Creating bootable image at: {}", img_path.display());

    let f = fs::File::create(&img_path)?;
    f.set_len(size_mb * 1024 * 1024)?;

    let status = Command::new("mformat")
        .args(["-i", &img_path.to_string_lossy(), "-F", "::"])
        .status()?;
    ensure_success(status, "mformat failed")?;

    for folder in ["EFI", "serviceos"] {
        let folder_path = esp_dir.join(folder);
        if folder_path.exists() {
            let status = Command::new("mcopy")
                .arg("-i")
                .arg(&img_path)
                .arg("-s")
                .arg(&folder_path)
                .arg("::")
                .status()?;
            ensure_success(status, &format!("mcopy failed for {folder}"))?;
        }
    }

    Ok(img_path)
}

fn create_raspi_bundle(
    artifacts: &BuildArtifacts,
    layout: &crate::bundle::StagedPlatformLayout,
) -> Result<PathBuf, Box<dyn Error>> {
    let boot_dir = &layout.boot_dir;
    fs::create_dir_all(boot_dir)?;
    let config_txt = boot_dir.join("config.txt");
    let readme = boot_dir.join("README.txt");
    let release_mode = if artifacts.release {
        "release"
    } else {
        "debug"
    };
    let kernel_elf = layout.serviceos_dir.join("serviceos-kernel.elf");
    let kernel_img = boot_dir.join("kernel8.img");
    convert_elf_to_binary(&kernel_elf, &kernel_img)?;
    let device_tree_line = if let Some(dtb_source) = locate_raspi5_dtb() {
        let file_name = dtb_source
            .file_name()
            .ok_or("invalid Raspberry Pi DTB path")?;
        fs::copy(&dtb_source, boot_dir.join(file_name))?;
        format!("device_tree={}", file_name.to_string_lossy())
    } else {
        String::from(
            "# device_tree=bcm2712-rpi-5-b.dtb  # add this if your target boot partition does not already provide it",
        )
    };

    fs::write(
        &config_txt,
        [
            "arm_64bit=1",
            "enable_uart=1",
            "kernel=kernel8.img",
            &device_tree_line,
            "# ServiceOS Raspberry Pi 5 serial-first boot image",
            "# The userspace boot-store is staged under serviceos/bootstore.bin and embedded into the Pi kernel image for early bootstrap.",
            "",
        ]
        .join("\n"),
    )?;
    fs::write(
        &readme,
        format!(
            "ServiceOS Raspberry Pi 5 boot image\n\nProfile: {release_mode}\nPlatform: raspi5\n\nStaged files:\n- config.txt\n- kernel8.img\n- serviceos/bootstore.bin\n- serviceos/serviceos-kernel.elf\n\nDeployment:\n1. Copy the contents of this directory to a Raspberry Pi 5 FAT boot partition.\n2. If this directory does not include bcm2712-rpi-5-b.dtb, use an existing Pi boot partition that already has the matching DTB, or set RASPI5_DTB before running xtask image.\n3. Connect the Raspberry Pi 5 debug UART and power on.\n\nCurrent state:\n- native AArch64 entry, DTB parsing, memory discovery, and PL011 UART bring-up are implemented\n- the Pi path now reaches generic kernel initialization, embedded-boot-store image resolution, root-manager launch, and a serial-first userspace service graph\n- graphics, pointer/keyboard, networking, and writable storage backends remain deferred on Raspberry Pi 5\n"
        ),
    )?;

    println!(
        "Created Raspberry Pi 5 boot bundle at: {}",
        boot_dir.display()
    );
    Ok(boot_dir.to_path_buf())
}

fn convert_elf_to_binary(source: &Path, destination: &Path) -> Result<(), Box<dyn Error>> {
    let status = Command::new(objcopy_executable())
        .args(["-O", "binary"])
        .arg(source)
        .arg(destination)
        .status()?;
    ensure_success(status, "llvm-objcopy failed for Raspberry Pi kernel image")
}

fn objcopy_executable() -> PathBuf {
    env::var_os("LLVM_OBJCOPY")
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .or_else(|| find_in_path("llvm-objcopy"))
        .or_else(|| {
            [
                "/usr/bin/llvm-objcopy",
                "/usr/sbin/llvm-objcopy",
                "/usr/lib/llvm-18/bin/llvm-objcopy",
                "/usr/lib/llvm-17/bin/llvm-objcopy",
                "/usr/lib/llvm-16/bin/llvm-objcopy",
            ]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| path.exists())
        })
        .unwrap_or_else(|| PathBuf::from("llvm-objcopy"))
}

fn find_in_path(binary: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|dir| dir.join(binary))
            .find(|candidate| candidate.exists())
    })
}

fn locate_raspi5_dtb() -> Option<PathBuf> {
    env::var_os("RASPI5_DTB")
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .or_else(|| {
            [
                "/boot/firmware/bcm2712-rpi-5-b.dtb",
                "/boot/firmware/broadcom/bcm2712-rpi-5-b.dtb",
                "/boot/bcm2712-rpi-5-b.dtb",
                "/boot/broadcom/bcm2712-rpi-5-b.dtb",
                "/usr/lib/firmware/broadcom/bcm2712-rpi-5-b.dtb",
                "/lib/firmware/broadcom/bcm2712-rpi-5-b.dtb",
            ]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| path.exists())
        })
}

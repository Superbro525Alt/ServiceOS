use std::{
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
        ImageKind::RaspberryPiBundle => create_raspi_bundle(artifacts, &layout.root_dir),
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
    boot_dir: &Path,
) -> Result<PathBuf, Box<dyn Error>> {
    fs::create_dir_all(boot_dir)?;
    let config_txt = boot_dir.join("config.txt");
    let readme = boot_dir.join("README.txt");
    let release_mode = if artifacts.release {
        "release"
    } else {
        "debug"
    };

    fs::write(
        &config_txt,
        [
            "arm_64bit=1",
            "enable_uart=1",
            "kernel=kernel8.img",
            "# ServiceOS Raspberry Pi 5 platform scaffold",
            "# The userspace boot-store is staged under serviceos/bootstore.bin.",
            "# A native Raspberry Pi 5 kernel entry path is not implemented yet.",
            "",
        ]
        .join("\n"),
    )?;
    fs::write(
        &readme,
        format!(
            "ServiceOS Raspberry Pi 5 platform image scaffold\n\nProfile: {release_mode}\nPlatform: raspi5\n\nStaged files:\n- config.txt\n- serviceos/bootstore.bin\n\nCurrent state:\n- arch/aarch64 and platform/aarch64/raspi5 crates build and define the long-term split\n- the Raspberry Pi firmware boot parser and executable kernel image are still deferred\n- this directory is the intended boot-partition layout for the eventual Pi image pipeline\n"
        ),
    )?;

    println!(
        "Created Raspberry Pi 5 boot scaffold at: {}",
        boot_dir.display()
    );
    Ok(boot_dir.to_path_buf())
}

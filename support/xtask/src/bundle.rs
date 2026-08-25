use std::{error::Error, fs, path::PathBuf};

use crate::{build::BuildArtifacts, platform::BootKind};

pub struct StagedPlatformLayout {
    pub root_dir: PathBuf,
    pub boot_dir: PathBuf,
    pub serviceos_dir: PathBuf,
}

pub fn stage_platform_bundle(
    artifacts: &BuildArtifacts,
) -> Result<StagedPlatformLayout, Box<dyn Error>> {
    let root_dir = match artifacts.spec.boot_kind {
        BootKind::Uefi => artifacts.image_root.join("esp"),
        BootKind::RaspberryPiFirmware | BootKind::QemuKernel => artifacts.image_root.join("boot"),
        BootKind::MultibootElf => artifacts.image_root.join("boot"),
    };
    let boot_dir = match artifacts.spec.boot_kind {
        BootKind::Uefi => root_dir.join("EFI").join("BOOT"),
        BootKind::RaspberryPiFirmware | BootKind::QemuKernel => root_dir.clone(),
        BootKind::MultibootElf => root_dir.join("kernels"),
    };
    let serviceos_dir = root_dir.join("serviceos");

    fs::create_dir_all(&boot_dir)?;
    fs::create_dir_all(&serviceos_dir)?;
    fs::copy(
        &artifacts.bootstore_binary,
        serviceos_dir.join("bootstore.bin"),
    )?;
    if std::env::var("SERVICEOS_BOOT_MODE").as_deref() == Ok("recovery") {
        // Operator-visible note in the staged bundle; the actual boot-mode
        // word is compiled into the platform loader via SERVICEOS_BOOT_MODE.
        fs::write(serviceos_dir.join("bootmode.txt"), b"recovery\n")?;
    }

    if let Some(kernel_binary) = &artifacts.kernel_binary {
        let destination = match artifacts.spec.boot_kind {
            BootKind::Uefi => boot_dir.join("BOOTX64.EFI"),
            BootKind::RaspberryPiFirmware | BootKind::QemuKernel => {
                serviceos_dir.join("serviceos-kernel.elf")
            }
            BootKind::MultibootElf => boot_dir.join("serviceos-kernel.elf"),
        };
        fs::copy(kernel_binary, destination)?;
    }

    Ok(StagedPlatformLayout {
        root_dir,
        boot_dir,
        serviceos_dir,
    })
}

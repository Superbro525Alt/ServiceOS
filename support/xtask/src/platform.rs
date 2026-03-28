use std::{error::Error, fmt, path::PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Arch {
    X86_64,
    Aarch64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootKind {
    Uefi,
    RaspberryPiFirmware,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageKind {
    RawDisk,
    RaspberryPiBundle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunKind {
    QemuVirtio,
    ManualDeploy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlatformSpec {
    pub name: &'static str,
    pub arch: Arch,
    pub rust_target: Option<&'static str>,
    pub kernel_package: Option<&'static str>,
    pub arch_package: &'static str,
    pub platform_package: &'static str,
    pub image_kind: ImageKind,
    pub boot_kind: BootKind,
    pub run_kind: RunKind,
}

impl PlatformSpec {
    pub fn resolve(name: &str) -> Result<Self, Box<dyn Error>> {
        match name {
            "qemu-virtio" => Ok(Self::qemu_virtio()),
            "raspi5" => Ok(Self::raspi5()),
            _ => Err(Box::new(UnknownPlatform(name.to_owned()))),
        }
    }

    pub const fn qemu_virtio() -> Self {
        Self {
            name: "qemu-virtio",
            arch: Arch::X86_64,
            rust_target: Some("x86_64-unknown-uefi"),
            kernel_package: Some("serviceos-kernel-x86_64"),
            arch_package: "serviceos-kernel-arch-x86_64",
            platform_package: "serviceos-platform-qemu-virtio",
            image_kind: ImageKind::RawDisk,
            boot_kind: BootKind::Uefi,
            run_kind: RunKind::QemuVirtio,
        }
    }

    pub const fn raspi5() -> Self {
        Self {
            name: "raspi5",
            arch: Arch::Aarch64,
            rust_target: Some("aarch64-unknown-none-softfloat"),
            kernel_package: Some("serviceos-kernel-raspi5"),
            arch_package: "serviceos-kernel-arch-aarch64",
            platform_package: "serviceos-platform-raspi5",
            image_kind: ImageKind::RaspberryPiBundle,
            boot_kind: BootKind::RaspberryPiFirmware,
            run_kind: RunKind::ManualDeploy,
        }
    }

    pub fn image_root(self, workspace_root: &std::path::Path, profile: &str) -> PathBuf {
        workspace_root
            .join("target")
            .join("images")
            .join(profile)
            .join(self.name)
    }

    pub fn kernel_binary_path(
        self,
        workspace_root: &std::path::Path,
        profile: &str,
    ) -> Option<PathBuf> {
        let package = self.kernel_package?;
        let target = self.rust_target?;
        let file_name = match self.boot_kind {
            BootKind::Uefi => format!("{package}.efi"),
            BootKind::RaspberryPiFirmware => package.to_owned(),
        };

        Some(
            workspace_root
                .join("target")
                .join(target)
                .join(profile)
                .join(file_name),
        )
    }
}

#[derive(Debug)]
struct UnknownPlatform(String);

impl fmt::Display for UnknownPlatform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown platform: {}", self.0)
    }
}

impl Error for UnknownPlatform {}

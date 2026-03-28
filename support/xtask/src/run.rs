use std::{
    env,
    error::Error,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    build::{BuildArtifacts, ensure_success},
    platform::RunKind,
};

pub fn run_platform(artifacts: &BuildArtifacts, image: &Path) -> Result<(), Box<dyn Error>> {
    match artifacts.spec.run_kind {
        RunKind::QemuVirtio => run_qemu(image),
        RunKind::ManualDeploy => {
            println!(
                "Platform '{}' does not have emulator run support yet.",
                artifacts.spec.name
            );
            println!("Staged output: {}", image.display());
            println!(
                "Copy the contents of that directory to a Raspberry Pi boot partition once the native kernel image exists."
            );
            Ok(())
        }
    }
}

fn run_qemu(disk_image: &Path) -> Result<(), Box<dyn Error>> {
    let ovmf_code = find_ovmf_code().ok_or("no OVMF code firmware found")?;
    let ovmf_vars = create_ovmf_vars_copy(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join("target")
            .join("ovmf"),
    )?;
    let headless = qemu_headless();

    let mut command = Command::new("qemu-system-x86_64");
    command.args(["-machine", "q35"]);
    command.args(["-m", "512"]);
    command.args(["-smp", "2"]);
    command.args(["-cpu", "max"]);
    if kvm_available() {
        command.args(["-accel", "kvm"]);
    } else {
        command.args(["-accel", "tcg,thread=multi"]);
    }
    command.args(["-serial", "stdio"]);
    if headless {
        command.args(["-display", "none"]);
    } else {
        command.args(["-display", "gtk,gl=off"]);
    }
    command.args(["-netdev", "user,id=net0"]);
    command.args([
        "-device",
        "virtio-net-pci,netdev=net0,mac=52:54:00:12:34:56",
    ]);
    command.args(["-device", "virtio-keyboard-pci"]);
    command.args(["-device", "virtio-tablet-pci"]);
    command.args([
        "-drive",
        &format!(
            "if=pflash,format=raw,readonly=on,file={}",
            ovmf_code.display()
        ),
    ]);
    command.args([
        "-drive",
        &format!("if=pflash,format=raw,file={}", ovmf_vars.display()),
    ]);
    command.args([
        "-drive",
        &format!("format=raw,file={}", disk_image.display()),
    ]);
    if let Some(extra_args) = env::var_os("QEMU_EXTRA_ARGS") {
        for arg in extra_args.to_string_lossy().split_whitespace() {
            command.arg(arg);
        }
    }

    let status = command.status()?;
    ensure_success(status, "QEMU UEFI run failed")
}

fn qemu_headless() -> bool {
    matches!(
        env::var("QEMU_HEADLESS").ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

fn kvm_available() -> bool {
    Path::new("/dev/kvm").exists()
}

fn create_ovmf_vars_copy(out_dir: &Path) -> Result<PathBuf, Box<dyn Error>> {
    std::fs::create_dir_all(out_dir)?;
    let destination = out_dir.join("OVMF_VARS.fd");

    if destination.exists() {
        return Ok(destination);
    }

    let source = find_ovmf_vars_template().ok_or("no OVMF variables template found")?;
    std::fs::copy(source, &destination)?;
    Ok(destination)
}

fn find_ovmf_code() -> Option<PathBuf> {
    let env_override = env::var_os("OVMF_CODE").map(PathBuf::from);
    env_override.filter(|path| path.exists()).or_else(|| {
        [
            "/usr/share/edk2/x64/OVMF_CODE.4m.fd",
            "/usr/share/edk2/x64/OVMF_CODE.fd",
            "/usr/share/OVMF/OVMF_CODE.fd",
        ]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.exists())
    })
}

fn find_ovmf_vars_template() -> Option<PathBuf> {
    let env_override = env::var_os("OVMF_VARS").map(PathBuf::from);
    env_override.filter(|path| path.exists()).or_else(|| {
        [
            "/usr/share/edk2/x64/OVMF_VARS.4m.fd",
            "/usr/share/edk2/x64/OVMF_VARS.fd",
            "/usr/share/OVMF/OVMF_VARS.fd",
        ]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.exists())
    })
}

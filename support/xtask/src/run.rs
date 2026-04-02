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
                "Copy the contents of that directory to a Raspberry Pi boot partition and boot it on hardware."
            );
            Ok(())
        }
    }
}

fn run_qemu(disk_image: &Path) -> Result<(), Box<dyn Error>> {
    let data_image = disk_image
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("serviceos-data.img");
    let qemu_binary = find_qemu_binary().ok_or_else(|| {
        "qemu-system-x86_64 not found; install QEMU or set QEMU_SYSTEM_X86_64 to an absolute path"
    })?;
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
    if !disk_image.exists() {
        return Err(format!("QEMU disk image is missing: {}", disk_image.display()).into());
    }
    if !data_image.exists() {
        return Err(format!("QEMU data image is missing: {}", data_image.display()).into());
    }

    println!("Launching QEMU with:");
    println!("  binary: {}", qemu_binary.display());
    println!("  firmware code: {}", ovmf_code.display());
    println!("  firmware vars: {}", ovmf_vars.display());
    println!("  disk image: {}", disk_image.display());
    println!("  data image: {}", data_image.display());
    println!(
        "  display mode: {}",
        if headless { "headless" } else { "graphical" }
    );
    let accel = qemu_accel_mode();
    println!("  accelerator: {}", accel_name(accel));
    let audio_device = qemu_audio_device()?;
    println!("  audio: {}", audio_device);

    let mut command = Command::new(&qemu_binary);
    command.args(["-machine", "q35,pcspk-audiodev=speaker"]);
    command.args(["-m", "1048"]);
    command.args(["-smp", "2"]);
    command.args(["-cpu", "max"]);
    match accel {
        QemuAccelMode::Tcg => {
            command.args(["-accel", "tcg,thread=multi"]);
        }
        QemuAccelMode::Kvm => {
            if !kvm_available() {
                return Err("QEMU accel mode 'kvm' requested but /dev/kvm is not available".into());
            }
            command.args(["-accel", "kvm"]);
        }
    }
    command.args(["-serial", "stdio"]);
    if headless {
        command.args(["-display", "none"]);
    } else {
        command.args(["-display", "gtk,gl=off"]);
    }
    command.args(["-audiodev", &audio_device]);
    command.args(["-netdev", "user,id=net0"]);
    command.args([
        "-device",
        "virtio-net-pci,netdev=net0,mac=52:54:00:12:34:56",
    ]);
    command.args(["-device", "virtio-keyboard-pci"]);
    command.args(["-device", "virtio-mouse-pci"]);
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
    command.args([
        "-drive",
        &format!("if=none,id=data0,format=raw,file={}", data_image.display()),
    ]);
    command.args(["-device", "virtio-blk-pci,drive=data0"]);
    if let Some(extra_args) = env::var_os("QEMU_EXTRA_ARGS") {
        for arg in extra_args.to_string_lossy().split_whitespace() {
            command.arg(arg);
        }
    }

    let status = command.status().map_err(|error| {
        format!(
            "failed to launch QEMU binary {} with disk {}: {}",
            qemu_binary.display(),
            disk_image.display(),
            error
        )
    })?;
    ensure_success(status, "QEMU UEFI run failed")
}

fn qemu_audio_device() -> Result<String, Box<dyn Error>> {
    if let Some(spec) = env::var_os("QEMU_AUDIODEV") {
        return Ok(spec.to_string_lossy().into_owned());
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .join("target")
        .join("qemu-audio");
    std::fs::create_dir_all(&root)?;
    let path = root.join("serviceos-pcspk.wav");
    Ok(format!("wav,id=speaker,path={}", path.display()))
}

fn qemu_headless() -> bool {
    matches!(
        env::var("QEMU_HEADLESS").ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QemuAccelMode {
    Tcg,
    Kvm,
}

fn qemu_accel_mode() -> QemuAccelMode {
    match env::var("QEMU_ACCEL").ok().as_deref() {
        Some("kvm") => QemuAccelMode::Kvm,
        Some("tcg") => QemuAccelMode::Tcg,
        _ if kvm_available() => QemuAccelMode::Kvm,
        _ => QemuAccelMode::Tcg,
    }
}

fn accel_name(mode: QemuAccelMode) -> &'static str {
    match mode {
        QemuAccelMode::Tcg => "tcg,thread=multi",
        QemuAccelMode::Kvm => "kvm",
    }
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

fn find_qemu_binary() -> Option<PathBuf> {
    env::var_os("QEMU_SYSTEM_X86_64")
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .or_else(|| {
            env::var_os("PATH").and_then(|path| {
                env::split_paths(&path)
                    .map(|dir| dir.join("qemu-system-x86_64"))
                    .find(|candidate| candidate.exists())
            })
        })
        .or_else(|| {
            [
                "/usr/bin/qemu-system-x86_64",
                "/usr/sbin/qemu-system-x86_64",
            ]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| path.exists())
        })
}

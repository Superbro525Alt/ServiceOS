use std::{
    env,
    error::Error,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    build::{ensure_success, BuildArtifacts},
    image::ensure_virt_kernel_image,
    platform::RunKind,
};

pub fn run_platform(artifacts: &BuildArtifacts, image: &Path) -> Result<(), Box<dyn Error>> {
    match artifacts.spec.run_kind {
        RunKind::QemuVirtio => run_qemu(image),
        RunKind::QemuArmVirt => run_qemu_virt(artifacts),
        RunKind::QemuIsa => run_qemu_isa(artifacts),
        RunKind::QemuRiscvVirt => run_qemu_riscv_virt(artifacts),
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

fn run_qemu_virt(artifacts: &BuildArtifacts) -> Result<(), Box<dyn Error>> {
    let mut command = qemu_virt_command(artifacts)?;
    let status = command
        .status()
        .map_err(|error| format!("failed to launch QEMU aarch64 command: {}", error))?;

    ensure_success(status, "QEMU virt run failed")
}

/// Assemble the full QEMU aarch64 `virt` command line for the platform
/// artifacts. Shared by the interactive runner and the bounded headless
/// boot logger.
pub fn qemu_virt_command(artifacts: &BuildArtifacts) -> Result<Command, Box<dyn Error>> {
    let kernel_image = ensure_virt_kernel_image(artifacts)?;
    let qemu_binary = find_qemu_aarch64_binary().ok_or_else(|| {
        "qemu-system-aarch64 not found; install QEMU or set QEMU_SYSTEM_AARCH64 to an absolute path"
    })?;
    let data_image = artifacts.image_root.join("serviceos-data.img");
    ensure_virt_data_image(&data_image)?;

    let headless = qemu_headless();
    println!("Launching QEMU with:");
    println!("  binary: {}", qemu_binary.display());
    println!("  machine: virt,gic-version=3");
    println!("  cpu: cortex-a76");
    println!("  kernel: {}", kernel_image.display());
    println!("  data image: {}", data_image.display());
    println!(
        "  display mode: {}",
        if headless { "headless" } else { "graphical" }
    );

    let mut command = Command::new(&qemu_binary);
    command.args(["-machine", "virt,gic-version=3"]);
    command.args(["-cpu", "cortex-a76"]);
    command.args(["-m", "1024"]);
    command.args(["-smp", "2"]);
    command.args(["-accel", "tcg,thread=multi"]);
    command.args(["-kernel", &kernel_image.to_string_lossy()]);
    command.args(["-serial", "stdio"]);
    if headless {
        command.args(["-display", "none"]);
    } else {
        command.args(["-display", "gtk,gl=off"]);
    }
    command.args(["-device", "virtio-gpu-device"]);
    command.args(["-netdev", "user,id=net0"]);
    command.args([
        "-device",
        "virtio-net-device,netdev=net0,mac=52:54:00:12:34:56",
    ]);
    command.args(["-device", "virtio-keyboard-device"]);
    command.args(["-device", "virtio-mouse-device"]);
    command.args(["-device", "virtio-tablet-device"]);
    command.args([
        "-drive",
        &format!("if=none,id=data0,format=raw,file={}", data_image.display()),
    ]);
    command.args(["-device", "virtio-blk-device,drive=data0"]);
    if let Some(extra_args) = env::var_os("QEMU_EXTRA_ARGS") {
        for arg in extra_args.to_string_lossy().split_whitespace() {
            command.arg(arg);
        }
    }

    Ok(command)
}

fn run_qemu_isa(artifacts: &BuildArtifacts) -> Result<(), Box<dyn Error>> {
    let mut command = qemu_isa_command(artifacts)?;
    let status = command
        .status()
        .map_err(|error| format!("failed to launch QEMU binary: {}", error))?;
    ensure_success(status, "QEMU isa run failed")
}

/// Assemble the full qemu-system-x86_64 multiboot command line for the isa
/// platform. Shared by the interactive runner and the e2e runner framework.
/// Under QEMU_HEADLESS=1 this emits `-display none -serial stdio`, exactly
/// as the historical inline builder did.
pub fn qemu_isa_command(artifacts: &BuildArtifacts) -> Result<Command, Box<dyn Error>> {
    let kernel_elf = artifacts
        .kernel_binary
        .as_ref()
        .ok_or_else(|| "qemu-isa requires a kernel ELF".to_string())?;
    if !kernel_elf.exists() {
        return Err(format!("qemu-isa kernel ELF missing: {}", kernel_elf.display()).into());
    }
    let qemu_binary = find_qemu_binary().ok_or_else(|| {
        "qemu-system-x86_64 not found; install QEMU or set QEMU_SYSTEM_X86_64 to an absolute path"
    })?;
    let headless = qemu_headless();
    println!("Launching QEMU with:");
    println!("  binary: {}", qemu_binary.display());
    println!("  machine: pc (legacy BIOS, multiboot kernel)");
    println!("  kernel: {}", kernel_elf.display());
    println!(
        "  display mode: {}",
        if headless { "headless" } else { "graphical" }
    );

    let mut command = Command::new(&qemu_binary);
    command.args(["-machine", "pc"]);
    command.args(["-m", "1024"]);
    command.args(["-kernel", &kernel_elf.to_string_lossy()]);
    if headless {
        command.args(["-display", "none", "-serial", "stdio"]);
    } else {
        command.args(["-display", "gtk,gl=off"]);
    }
    if let Some(extra_args) = env::var_os("QEMU_EXTRA_ARGS") {
        for arg in extra_args.to_string_lossy().split_whitespace() {
            command.arg(arg);
        }
    }

    Ok(command)
}

fn run_qemu_riscv_virt(artifacts: &BuildArtifacts) -> Result<(), Box<dyn Error>> {
    let mut command = qemu_riscv_virt_command(artifacts)?;
    let status = command
        .status()
        .map_err(|error| format!("failed to launch QEMU riscv64: {}", error))?;
    ensure_success(status, "QEMU riscv64-virt run failed")
}

/// Assemble the full qemu-system-riscv64 command line for the riscv64-virt
/// skeleton platform. Shared by the interactive runner and the e2e runner
/// framework; argv is identical to the historical inline builder.
pub fn qemu_riscv_virt_command(artifacts: &BuildArtifacts) -> Result<Command, Box<dyn Error>> {
    let kernel_elf = artifacts
        .kernel_binary
        .as_ref()
        .ok_or_else(|| "riscv64-virt requires a kernel ELF".to_string())?;
    if !kernel_elf.exists() {
        return Err(format!("riscv64-virt kernel ELF missing: {}", kernel_elf.display()).into());
    }
    let qemu_binary = find_qemu_riscv64_binary().ok_or_else(|| {
        "qemu-system-riscv64 not found; install QEMU or set QEMU_SYSTEM_RISCV64 to an absolute path"
    })?;
    println!("Launching QEMU with:");
    println!("  binary: {}", qemu_binary.display());
    println!("  machine: virt (-bios default: OpenSBI hands off at 0x80200000)");
    println!("  kernel: {}", kernel_elf.display());

    let mut command = Command::new(&qemu_binary);
    command.args(["-machine", "virt"]);
    command.args(["-bios", "default"]);
    command.args(["-m", "128M"]);
    command.args(["-smp", "2"]);
    command.args(["-nographic"]);
    command.args(["-kernel", &kernel_elf.to_string_lossy()]);
    if let Some(extra_args) = env::var_os("QEMU_EXTRA_ARGS") {
        for arg in extra_args.to_string_lossy().split_whitespace() {
            command.arg(arg);
        }
    }

    Ok(command)
}

fn ensure_virt_data_image(data_image: &Path) -> Result<(), Box<dyn Error>> {
    if data_image.exists() {
        return Ok(());
    }
    if let Some(parent) = data_image.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(data_image)?;
    file.set_len(64 * 1024 * 1024)?;
    Ok(())
}

pub fn find_qemu_riscv64_binary() -> Option<PathBuf> {
    env::var_os("QEMU_SYSTEM_RISCV64")
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .or_else(|| {
            env::var_os("PATH").and_then(|path| {
                env::split_paths(&path)
                    .map(|dir| dir.join("qemu-system-riscv64"))
                    .find(|candidate| candidate.exists())
            })
        })
        .or_else(|| {
            [
                "/usr/bin/qemu-system-riscv64",
                "/usr/sbin/qemu-system-riscv64",
            ]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| path.exists())
        })
}

pub fn find_qemu_aarch64_binary() -> Option<PathBuf> {
    env::var_os("QEMU_SYSTEM_AARCH64")
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .or_else(|| {
            env::var_os("PATH").and_then(|path| {
                env::split_paths(&path)
                    .map(|dir| dir.join("qemu-system-aarch64"))
                    .find(|candidate| candidate.exists())
            })
        })
        .or_else(|| {
            [
                "/usr/bin/qemu-system-aarch64",
                "/usr/sbin/qemu-system-aarch64",
            ]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| path.exists())
        })
}

fn run_qemu(disk_image: &Path) -> Result<(), Box<dyn Error>> {
    let data_image = disk_image
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("serviceos-data.img");
    let mut command = qemu_virtio_command(&data_image, disk_image)?;
    let status = command
        .status()
        .map_err(|error| format!("failed to launch QEMU UEFI command: {}", error))?;
    ensure_success(status, "QEMU UEFI run failed")
}

/// Assemble the full qemu-system-x86_64 UEFI command line for the virtio
/// platform. Shared by the interactive runner and the bounded headless boot
/// logger.
pub fn qemu_virtio_command(
    data_image: &Path,
    disk_image: &Path,
) -> Result<Command, Box<dyn Error>> {
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
    match &audio_device {
        Some(spec) => println!("  audio: {}", spec),
        None => println!("  audio: off"),
    }
    // SERVICEOS_AUDIO=1 attaches the virtio-sound PCI device so the guest
    // can drive audible PCM playback through QEMU's audio backend. It
    // defaults to off so CI boots stay deterministic; without a user
    // QEMU_AUDIODEV spec, QEMU's silent 'none' backend (no host audio
    // required) is used and the guest path is still exercised end to end.
    let virtio_sound = serviceos_audio_enabled();
    if virtio_sound {
        println!("  virtio-sound: on");
    }

    let mut command = Command::new(&qemu_binary);
    match (&audio_device, virtio_sound) {
        (_, true) | (Some(_), false) => {
            // pcspk-audiodev keeps the legacy speaker wired to the same
            // host backend as virtio-sound.
            command.args(["-machine", "q35,pcspk-audiodev=speaker"]);
        }
        (None, false) => {
            command.args(["-machine", "q35"]);
        }
    }
    command.args(["-m", "1048"]);
    // SERVICEOS_SMP controls the guest CPU count (default single-core, which
    // keeps kernel boot output byte-identical to the pre-SMP kernel).
    let smp_cpus = env::var("SERVICEOS_SMP").unwrap_or_else(|_| "1".to_string());
    command.args(["-smp", &smp_cpus]);
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
        let gl = matches!(env::var("SERVICEOS_GL").as_deref(), Ok("1"));
        command.args(["-display", if gl { "gtk,gl=on" } else { "gtk,gl=off" }]);
    }
    if let Some(audio_device) = &audio_device {
        command.args(["-audiodev", audio_device]);
    }
    if virtio_sound {
        // The virtio-sound device needs a host audiodev; default to the
        // silent 'none' backend when QEMU_AUDIODEV is unset so enabling
        // the guest path never depends on host audio hardware.
        let host_backend = audio_device.clone().unwrap_or_else(|| {
            println!("  audiodev: driver=none,id=speaker (default)");
            "driver=none,id=speaker".to_string()
        });
        if audio_device.is_none() {
            command.args(["-audiodev", &host_backend]);
        }
        command.args(["-device", "virtio-sound-pci,audiodev=speaker"]);
    }
    command.args(["-netdev", "user,id=net0"]);
    command.args([
        "-device",
        "virtio-net-pci,netdev=net0,mac=52:54:00:12:34:56",
    ]);
    command.args(["-device", "virtio-keyboard-pci"]);
    command.args(["-device", "virtio-mouse-pci"]);
    command.args(["-device", "virtio-tablet-pci"]);
    // virtio-gpu-pci: the kernel's display backend probes for this device
    // and prefers it over the UEFI GOP linear framebuffer (SERVICEOS_VGPU_DISABLE
    // forces the GOP fallback at kernel build time).
    command.args(["-device", "virtio-gpu-pci"]);
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

    Ok(command)
}

fn qemu_audio_device() -> Result<Option<String>, Box<dyn Error>> {
    if let Some(spec) = env::var_os("QEMU_AUDIODEV") {
        let spec = spec.to_string_lossy().into_owned();
        if spec.is_empty() || spec == "off" {
            return Ok(None);
        }
        return Ok(Some(spec));
    }

    Ok(None)
}

/// SERVICEOS_AUDIO=1 attaches the virtio-sound PCI playback device.
/// Defaults to off so CI boots stay byte-deterministic.
fn serviceos_audio_enabled() -> bool {
    matches!(env::var("SERVICEOS_AUDIO").ok().as_deref(), Some("1"))
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

pub fn create_ovmf_vars_copy(out_dir: &Path) -> Result<PathBuf, Box<dyn Error>> {
    std::fs::create_dir_all(out_dir)?;
    // Bounded/headless boots can be killed mid-run, which can leave the
    // shared vars image dirty and wedge later firmware phases. They opt out
    // via SERVICEOS_OVMF_VARS and get a throwaway copy instead.
    if let Some(path) = env::var_os("SERVICEOS_OVMF_VARS") {
        let destination = PathBuf::from(path);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let source = find_ovmf_vars_template().ok_or("no OVMF variables template found")?;
        std::fs::copy(&source, &destination)?;
        return Ok(destination);
    }
    let destination = out_dir.join("OVMF_VARS.fd");

    if destination.exists() {
        return Ok(destination);
    }

    let source = find_ovmf_vars_template().ok_or("no OVMF variables template found")?;
    std::fs::copy(source, &destination)?;
    Ok(destination)
}

pub fn find_ovmf_code() -> Option<PathBuf> {
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

pub fn find_ovmf_vars_template() -> Option<PathBuf> {
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

pub fn find_qemu_binary() -> Option<PathBuf> {
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

use std::{
    env,
    error::Error,
    fmt,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

const KERNEL_PACKAGE: &str = "serviceos-kernel-x86_64";
const KERNEL_TARGET: &str = "x86_64-unknown-uefi";
const USERSPACE_CATALOG_PACKAGE: &str = "serviceos-userspace-catalog";

fn main() -> Result<(), Box<dyn Error>> {
    let options = Options::parse(env::args().skip(1).collect())?;
    let artifacts = build_kernel(options.release)?;

    match options.command {
        CommandKind::Build => {
            stage_efi_partition(&artifacts)?;
        }
        CommandKind::Qemu => {
            let esp_dir = stage_efi_partition(&artifacts)?;
            run_qemu(&esp_dir)?;
        }
    }

    Ok(())
}

#[derive(Clone, Copy)]
enum CommandKind {
    Build,
    Qemu,
}

struct Options {
    command: CommandKind,
    release: bool,
}

impl Options {
    fn parse(args: Vec<String>) -> Result<Self, Box<dyn Error>> {
        let Some((command, rest)) = args.split_first() else {
            return Err(Box::new(UsageError));
        };

        let command = match command.as_str() {
            "build" => CommandKind::Build,
            "qemu" => CommandKind::Qemu,
            _ => return Err(Box::new(UsageError)),
        };

        let mut release = false;

        for arg in rest {
            match arg.as_str() {
                "--release" => release = true,
                _ => return Err(Box::new(UsageError)),
            }
        }

        Ok(Self { command, release })
    }
}

#[derive(Debug)]
struct UsageError;

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "usage: cargo xtask <build|qemu> [--release]")
    }
}

impl Error for UsageError {}

struct BuildArtifacts {
    kernel_binary: PathBuf,
    bootstore_binary: PathBuf,
    esp_dir: PathBuf,
}

fn build_kernel(release: bool) -> Result<BuildArtifacts, Box<dyn Error>> {
    let profile = if release { "release" } else { "debug" };
    let workspace_root = workspace_root();
    let userspace_profile_dir = workspace_root
        .join("target")
        .join("userspace-programs")
        .join(profile);

    let mut command = Command::new("cargo");
    command.current_dir(&workspace_root);
    command.args(["build", "-p", KERNEL_PACKAGE, "--target", KERNEL_TARGET]);
    if release {
        command.arg("--release");
    }

    let status = command.status()?;
    ensure_success(status, "cargo build failed")?;

    let mut userspace = Command::new("cargo");
    userspace.current_dir(&workspace_root);
    userspace.args(["build", "-p", USERSPACE_CATALOG_PACKAGE]);
    if release {
        userspace.arg("--release");
    }
    let status = userspace.status()?;
    ensure_success(status, "userspace catalog build failed")?;

    Ok(BuildArtifacts {
        kernel_binary: workspace_root
            .join("target")
            .join(KERNEL_TARGET)
            .join(profile)
            .join(format!("{KERNEL_PACKAGE}.efi")),
        bootstore_binary: userspace_profile_dir.join("bootstore.bin"),
        esp_dir: workspace_root
            .join("target")
            .join("images")
            .join(profile)
            .join("esp"),
    })
}

fn stage_efi_partition(artifacts: &BuildArtifacts) -> Result<PathBuf, Box<dyn Error>> {
    let boot_dir = artifacts.esp_dir.join("EFI").join("BOOT");
    let serviceos_dir = artifacts.esp_dir.join("serviceos");
    std::fs::create_dir_all(&boot_dir)?;
    std::fs::create_dir_all(&serviceos_dir)?;
    std::fs::copy(&artifacts.kernel_binary, boot_dir.join("BOOTX64.EFI"))?;
    std::fs::copy(
        &artifacts.bootstore_binary,
        serviceos_dir.join("bootstore.bin"),
    )?;
    Ok(artifacts.esp_dir.clone())
}

fn run_qemu(esp_dir: &Path) -> Result<(), Box<dyn Error>> {
    let ovmf_code = find_ovmf_code().ok_or("no OVMF code firmware found")?;
    let ovmf_vars = create_ovmf_vars_copy(&workspace_root().join("target").join("ovmf"))?;
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
        &format!("format=raw,file=fat:rw:{}", esp_dir.display()),
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

fn ensure_success(status: ExitStatus, context: &str) -> Result<(), Box<dyn Error>> {
    if status.success() {
        Ok(())
    } else {
        Err(format!("{context}: {status}").into())
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

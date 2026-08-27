use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandKind {
    Build,
    Image,
    Run,
    /// Boot the recovery environment: builds with SERVICEOS_BOOT_MODE=recovery
    /// so the platform loader hands root-manager the recovery boot-mode word.
    Recover,
    CiMatrix,
    /// Build release images for every registered platform and write a
    /// RELEASE-MANIFEST.json artifact manifest.
    Release,
    /// Boot-upgrade-boot cycle on qemu-virtio verifying storage persistence
    /// markers survive a rebuild between boots.
    TestUpgrade,
    /// Workspace check + tests + bounded QEMU boots + selftest greps with a
    /// single summary table and exit code.
    Validate,
    /// End-to-end suite: TOML case files under tests/cases/ executed by the
    /// serviceos-e2e runner framework (docs/test-plan.md §4).
    TestE2e,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Options<'a> {
    pub command: CommandKind,
    pub platform: &'a str,
    pub release: bool,
    /// Raw remainder args for commands that own their flag vocabulary
    /// (`test-e2e` parses its spec in support/xtask/src/e2e.rs).
    pub e2e_extra: Vec<String>,
}

impl<'a> Options<'a> {
    pub fn parse(args: Vec<String>) -> Result<Options<'static>, Box<dyn Error>> {
        let Some((command, rest)) = args.split_first() else {
            return Err(Box::new(UsageError));
        };

        let mut release = false;
        let mut platform = None;
        let command = match command.as_str() {
            "build" => CommandKind::Build,
            "image" => CommandKind::Image,
            "run" => CommandKind::Run,
            "recover" => CommandKind::Recover,
            "ci-matrix" => CommandKind::CiMatrix,
            "release" => CommandKind::Release,
            "test-upgrade" => CommandKind::TestUpgrade,
            "validate" => CommandKind::Validate,
            "test-e2e" => CommandKind::TestE2e,
            "qemu" => {
                platform = Some("qemu-virtio");
                CommandKind::Run
            }
            _ => return Err(Box::new(UsageError)),
        };

        // test-e2e owns its own flag grammar; hand the rest through untouched.
        if command == CommandKind::TestE2e {
            return Ok(Options {
                command,
                platform: platform.unwrap_or("qemu-virtio"),
                release,
                e2e_extra: rest.to_vec(),
            });
        }

        let mut index = 0usize;
        while index < rest.len() {
            match rest[index].as_str() {
                "--release" => {
                    release = true;
                    index += 1;
                }
                "--platform" => {
                    let Some(value) = rest.get(index + 1) else {
                        return Err(Box::new(UsageError));
                    };
                    platform = Some(intern_platform(value)?);
                    index += 2;
                }
                other => {
                    if let Some(value) = other.strip_prefix("--platform=") {
                        platform = Some(intern_platform(value)?);
                        index += 1;
                    } else {
                        return Err(Box::new(UsageError));
                    }
                }
            }
        }

        let platform = platform.unwrap_or("qemu-virtio");

        Ok(Options {
            command,
            platform,
            release,
            e2e_extra: Vec::new(),
        })
    }
}

fn intern_platform(value: &str) -> Result<&'static str, Box<dyn Error>> {
    match value {
        "qemu-virtio" => Ok("qemu-virtio"),
        "raspi5" => Ok("raspi5"),
        "virt" => Ok("virt"),
        "qemu-isa" => Ok("qemu-isa"),
        "riscv64-virt" => Ok("riscv64-virt"),
        _ => Err(Box::new(UsageError)),
    }
}

#[derive(Debug)]
struct UsageError;

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "usage: cargo xtask <build|image|run|recover|qemu|release|test-upgrade|validate|ci-matrix|test-e2e> [--platform <qemu-virtio|raspi5|virt|qemu-isa|riscv64-virt>] [--release]\n       test-e2e flags: [--platform <p>] [--tier <1..4>] [--filter <substr-or-regex>] [--tag <t>] [-j <n>] [--timeout-secs <s>] [--report <path>] [--list]"
        )
    }
}

impl Error for UsageError {}

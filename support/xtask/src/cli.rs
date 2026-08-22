use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandKind {
    Build,
    Image,
    Run,
    CiMatrix,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Options<'a> {
    pub command: CommandKind,
    pub platform: &'a str,
    pub release: bool,
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
            "ci-matrix" => CommandKind::CiMatrix,
            "qemu" => {
                platform = Some("qemu-virtio");
                CommandKind::Run
            }
            "release" => CommandKind::Image,
            _ => return Err(Box::new(UsageError)),
        };

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
        let release = release || matches!(command, CommandKind::Image) && args[0] == "release";

        Ok(Options {
            command,
            platform,
            release,
        })
    }
}

fn intern_platform(value: &str) -> Result<&'static str, Box<dyn Error>> {
    match value {
        "qemu-virtio" => Ok("qemu-virtio"),
        "raspi5" => Ok("raspi5"),
        "virt" => Ok("virt"),
        _ => Err(Box::new(UsageError)),
    }
}

#[derive(Debug)]
struct UsageError;

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "usage: cargo xtask <build|image|run|qemu|release|ci-matrix> [--platform <qemu-virtio|raspi5|virt>] [--release]"
        )
    }
}

impl Error for UsageError {}

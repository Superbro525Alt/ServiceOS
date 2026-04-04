use std::{
    env,
    error::Error,
    path::{Path, PathBuf},
    process::Command,
};

pub(crate) const X86_64_USER_TARGET: &str = "x86_64-unknown-none";
pub(crate) const AARCH64_USER_TARGET: &str = "aarch64-unknown-none-softfloat";

pub(crate) fn objcopy_binary(input: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    let status = Command::new(llvm_tool("LLVM_OBJCOPY", "llvm-objcopy"))
        .args(["-O", "binary"])
        .arg(input)
        .arg(output)
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("llvm-objcopy failed for {}", input.display()).into())
    }
}

pub(crate) fn llvm_tool(env_var: &str, binary: &str) -> PathBuf {
    env::var_os(env_var)
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .or_else(|| find_in_path(binary))
        .or_else(|| {
            [
                format!("/usr/bin/{binary}"),
                format!("/usr/sbin/{binary}"),
                format!("/usr/lib/llvm-18/bin/{binary}"),
                format!("/usr/lib/llvm-17/bin/{binary}"),
                format!("/usr/lib/llvm-16/bin/{binary}"),
            ]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| path.exists())
        })
        .unwrap_or_else(|| PathBuf::from(binary))
}

pub(crate) fn find_in_path(binary: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|dir| dir.join(binary))
            .find(|candidate| candidate.exists())
    })
}

pub(crate) fn parse_hex(value: &str) -> Result<u64, Box<dyn Error>> {
    Ok(u64::from_str_radix(value.trim_start_matches("0x"), 16)?)
}

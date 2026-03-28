use std::{
    error::Error,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use crate::platform::PlatformSpec;

const USERSPACE_CATALOG_PACKAGE: &str = "serviceos-userspace-catalog";

pub struct BuildArtifacts {
    pub spec: PlatformSpec,
    pub release: bool,
    pub bootstore_binary: PathBuf,
    pub kernel_binary: Option<PathBuf>,
    pub image_root: PathBuf,
}

pub fn build_for_platform(
    spec: PlatformSpec,
    release: bool,
) -> Result<BuildArtifacts, Box<dyn Error>> {
    let profile = if release { "release" } else { "debug" };
    let workspace_root = workspace_root();
    let userspace_profile_dir = workspace_root
        .join("target")
        .join("userspace-programs")
        .join(profile);

    build_package(
        &workspace_root,
        spec.arch_package,
        spec.rust_target,
        release,
        false,
    )?;
    build_package(
        &workspace_root,
        spec.platform_package,
        spec.rust_target,
        release,
        false,
    )?;
    if let Some(kernel_package) = spec.kernel_package {
        build_package(
            &workspace_root,
            kernel_package,
            spec.rust_target,
            release,
            true,
        )?;
    }
    build_package(
        &workspace_root,
        USERSPACE_CATALOG_PACKAGE,
        None,
        release,
        false,
    )?;

    let kernel_binary = match (spec.kernel_package, spec.rust_target) {
        (Some(kernel_package), Some(target)) => Some(
            workspace_root
                .join("target")
                .join(target)
                .join(profile)
                .join(format!("{kernel_package}.efi")),
        ),
        _ => None,
    };

    Ok(BuildArtifacts {
        spec,
        release,
        bootstore_binary: userspace_profile_dir.join("bootstore.bin"),
        kernel_binary,
        image_root: spec.image_root(&workspace_root, profile),
    })
}

fn build_package(
    workspace_root: &Path,
    package: &str,
    target: Option<&str>,
    release: bool,
    binary_target: bool,
) -> Result<(), Box<dyn Error>> {
    let mut command = Command::new("cargo");
    command.current_dir(workspace_root);
    command.args(["build", "-p", package]);
    if let Some(target) = target {
        command.args(["--target", target]);
    }
    if release {
        command.arg("--release");
    }
    if !binary_target {
        command.arg("--lib");
    }

    let status = command.status()?;
    ensure_success(status, &format!("cargo build failed for {package}"))
}

pub fn ensure_success(status: ExitStatus, context: &str) -> Result<(), Box<dyn Error>> {
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

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
    let userspace_target = userspace_target(spec);
    let userspace_profile_dir = workspace_root
        .join("target")
        .join("userspace-programs")
        .join(userspace_target)
        .join(profile);

    build_package(
        &workspace_root,
        spec.arch_package,
        spec.rust_target,
        release,
        false,
        &[("SERVICEOS_USER_TARGET", userspace_target)],
    )?;
    build_package(
        &workspace_root,
        spec.platform_package,
        spec.rust_target,
        release,
        false,
        &[("SERVICEOS_USER_TARGET", userspace_target)],
    )?;
    if let Some(kernel_package) = spec.kernel_package {
        build_package(
            &workspace_root,
            kernel_package,
            spec.rust_target,
            release,
            true,
            &[("SERVICEOS_USER_TARGET", userspace_target)],
        )?;
    }
    build_userspace_catalog_if_needed(spec, release)?;

    let kernel_binary = spec.kernel_binary_path(&workspace_root, profile);

    Ok(BuildArtifacts {
        spec,
        release,
        bootstore_binary: userspace_profile_dir.join("bootstore.bin"),
        kernel_binary,
        image_root: spec.image_root(&workspace_root, profile),
    })
}

/// Skeleton platforms (userspace_catalog = false) skip the userspace graph
/// entirely; their staged bundles contain no bootstore.bin.
fn build_userspace_catalog_if_needed(spec: PlatformSpec, release: bool) -> Result<(), Box<dyn Error>> {
    if !spec.userspace_catalog {
        println!(
            "Platform '{}' is a skeleton target; skipping userspace catalog build",
            spec.name
        );
        return Ok(());
    }
    build_userspace_catalog(spec, release)
}

/// Build the userspace catalog (which drives the nested userspace program
/// builds and produces bootstore.bin).
pub fn build_userspace_catalog(spec: PlatformSpec, release: bool) -> Result<(), Box<dyn Error>> {
    build_package(
        &workspace_root(),
        USERSPACE_CATALOG_PACKAGE,
        None,
        release,
        false,
        &[("SERVICEOS_USER_TARGET", userspace_target(spec))],
    )
}

/// Path of the bootstore.bin produced for the given platform/profile.
pub fn userspace_bootstore_path(spec: PlatformSpec, release: bool) -> PathBuf {
    let profile = if release { "release" } else { "debug" };
    workspace_root()
        .join("target")
        .join("userspace-programs")
        .join(userspace_target(spec))
        .join(profile)
        .join("bootstore.bin")
}

fn build_package(
    workspace_root: &Path,
    package: &str,
    target: Option<&str>,
    release: bool,
    binary_target: bool,
    extra_env: &[(&str, &str)],
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
    for (key, value) in extra_env {
        command.env(key, value);
    }

    let status = command.status()?;
    ensure_success(status, &format!("cargo build failed for {package}"))
}

fn userspace_target(spec: PlatformSpec) -> &'static str {
    spec.userspace_rust_target()
}

pub fn ensure_success(status: ExitStatus, context: &str) -> Result<(), Box<dyn Error>> {
    if status.success() {
        Ok(())
    } else {
        Err(format!("{context}: {status}").into())
    }
}

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

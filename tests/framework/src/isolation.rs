//! Slot isolation: per-case stage dirs with copied disk images, fresh or
//! seeded data volumes, and atomically-copied throwaway OVMF vars overlays
//! (temp-name + rename rather than xtask's direct copy, per the §6.5 note),
//! so killed boots can never poison shared assets.

use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use xtask_core::run::find_ovmf_vars_template;

pub const DEFAULT_DATA_IMAGE_MIB: u64 = 128;

/// Guest RAM footprint per platform (MiB); drives future `-j` slot budgets.
pub fn platform_mem_mib(platform: &str) -> u64 {
    match platform {
        "qemu-virtio" => 1048,
        "virt" => 1024,
        "qemu-isa" => 1024,
        "riscv64-virt" => 128,
        "raspi5" => 0,
        _ => 0,
    }
}

/// True for platforms whose boot graph mounts a writable block device.
fn uses_data_volume(platform: &str) -> bool {
    matches!(platform, "qemu-virtio" | "virt")
}

#[derive(Debug, Clone)]
pub struct SlotPaths {
    pub dir: PathBuf,
    pub disk_image: PathBuf,
    /// None on platforms without writable storage.
    pub data_image: Option<PathBuf>,
    /// qemu-virtio only: slot-private OVMF vars overlay path.
    pub ovmf_vars: Option<PathBuf>,
}

pub fn sanitize_case_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// Slot directory for a case name (pruning + diagnostics reuse).
pub fn slot_dir(workspace_root: &Path, case_name: &str, slot: u32) -> PathBuf {
    workspace_root
        .join("target")
        .join("e2e")
        .join(sanitize_case_name(case_name))
        .join(format!("slot{slot}"))
}

/// Stage `target/e2e/<case-name>/slot<N>/` contents. `built_disk` is the
/// freshly created platform image (never the long-lived dev image handle);
/// None for platforms whose boot argv mounts no disk (kernel-boot targets).
pub fn stage_case_images(
    workspace_root: &Path,
    case: &crate::case::CaseDef,
    platform: &str,
    built_disk: Option<&Path>,
    slot: u32,
) -> Result<SlotPaths, Box<dyn Error>> {
    let dir = slot_dir(workspace_root, &case.name, slot);
    recreate_dir(&dir)?;

    let disk_image = dir.join(disk_file_name(platform));
    if let Some(source) = built_disk {
        fs::copy(source, &disk_image)?;
    } else {
        // Kernel-boot platforms still keep the reserved slot filename so
        // diagnostics and future WP4 parallel slots have a stable shape.
        drop(fs::File::create(&disk_image));
    }

    let data_image = if uses_data_volume(platform) {
        let data_path = dir.join("serviceos-data.img");
        if case.data_fresh {
            create_zeroed_image(&data_path, DEFAULT_DATA_IMAGE_MIB)?;
        } else if let Some(source) = built_disk {
            seed_or_zero_data(&data_path, source)?;
        } else {
            create_zeroed_image(&data_path, DEFAULT_DATA_IMAGE_MIB)?;
        }
        Some(data_path)
    } else {
        None
    };

    let ovmf_vars = if platform == "qemu-virtio" {
        Some(stage_ovmf_vars_atomic(&dir)?)
    } else {
        None
    };

    Ok(SlotPaths {
        dir,
        disk_image,
        data_image,
        ovmf_vars,
    })
}

fn disk_file_name(platform: &str) -> &'static str {
    match platform {
        "qemu-isa" => "serviceos-isa.img",
        _ => "serviceos.img",
    }
}

fn recreate_dir(dir: &Path) -> Result<(), Box<dyn Error>> {
    if dir.exists() {
        fs::remove_dir_all(dir)?;
    }
    fs::create_dir_all(dir)?;
    Ok(())
}

/// Recursive directory copy for bundle-style build outputs (raspi5 staged
/// bundle, virt kernel bundle); std-only, no symlink following.
pub fn copy_tree(source: &Path, destination: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn create_zeroed_image(path: &Path, size_mib: u64) -> Result<(), Box<dyn Error>> {
    if path.exists() {
        fs::remove_file(path)?;
    }
    let file = fs::File::create(path)?;
    file.set_len(size_mib * 1024 * 1024)?;
    Ok(())
}

/// Mirror upgrade.rs's two-phase pattern when `data_fresh = false`: reuse an
/// existing slot volume across boots, else start from the builder's staged
/// seed volume, else zero-fill.
fn seed_or_zero_data(slot_data: &Path, built_disk: &Path) -> Result<(), Box<dyn Error>> {
    if !slot_data.exists() {
        if let Some(parent) = built_disk.parent() {
            let seed = parent.join("serviceos-data.img");
            if seed.exists() {
                fs::copy(&seed, slot_data)?;
                return Ok(());
            }
        }
        create_zeroed_image(slot_data, DEFAULT_DATA_IMAGE_MIB)?;
    }
    Ok(())
}

/// Throwaway vars overlay written atomically (tmp + rename), immune to the
/// template-copy race called out for j>2 concurrency.
fn stage_ovmf_vars_atomic(slot_dir: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let source = find_ovmf_vars_template().ok_or("no OVMF variables template found")?;
    let destination = slot_dir.join("OVMF_VARS.fd");
    let staging = slot_dir.join("OVMF_VARS.fd.tmp");
    fs::copy(&source, &staging)?;
    if destination.exists() {
        fs::remove_file(&destination)?;
    }
    fs::rename(&staging, &destination)?;
    Ok(destination)
}

/// WP4 pruning policy: PASSing cases shed their staged images so parallel
/// batches never accumulate `N × slots × ~1.2 GiB` of disk; failures keep
/// everything for postmortem (override with the runner's `--keep-all`).
pub fn discard_stage_dir(
    workspace_root: &Path,
    case_name: &str,
    slot: u32,
) -> Result<(), Box<dyn Error>> {
    match fs::remove_dir_all(slot_dir(workspace_root, case_name, slot)) {
        // Idempotent: rows that never staged (blocked skips, build-only
        // raspi5 assertions) prune as no-ops.
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizer_preserves_realistic_names() {
        assert_eq!(
            sanitize_case_name("regress.dhcp-rx-delivery"),
            "regress.dhcp-rx-delivery"
        );
        assert_eq!(sanitize_case_name("boot/evil"), "boot_evil");
        assert_eq!(sanitize_case_name("sp ace"), "sp_ace");
    }

    #[test]
    fn memory_budget_table_matches_plan_caps() {
        assert_eq!(platform_mem_mib("qemu-virtio"), 1048);
        assert_eq!(platform_mem_mib("riscv64-virt"), 128);
        assert_eq!(platform_mem_mib("raspi5"), 0);
    }

    #[test]
    fn zeroed_images_have_requested_length() {
        let dir = std::env::temp_dir().join(format!("e2e-zero-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("mkdir");
        let target = dir.join("zero.img");
        create_zeroed_image(&target, 8).expect("create");
        assert_eq!(target.metadata().expect("stat").len(), 8 * 1024 * 1024);
        let _ = fs::remove_dir_all(&dir);
    }
}

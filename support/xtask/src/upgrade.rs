use std::{error::Error, fs, path::PathBuf};

use crate::{
    bootlog,
    build::{build_for_platform, workspace_root},
    image::create_platform_image,
    platform::PlatformSpec,
};

const MARKER_FILE_WRITTEN: &str = "selftest file-written bytes=";
const MARKER_PERSIST_FAILED: &str = "selftest persist FAILED";
const MARKER_RESTORED: &str = "selftest mount-present restored=1";

pub fn run_test_upgrade() -> Result<(), Box<dyn Error>> {
    let root = workspace_root();
    let stage_dir = root.join("target").join("upgrade-test");
    fs::create_dir_all(&stage_dir)?;
    let stage_disk: PathBuf = stage_dir.join("serviceos.img");
    let stage_data: PathBuf = stage_dir.join("serviceos-data.img");

    println!("=== upgrade matrix phase 1/2: boot v-current ===");
    let spec = PlatformSpec::qemu_virtio();
    let artifacts = build_for_platform(spec, false)?;
    let built_disk = create_platform_image(&artifacts)?;
    fs::copy(&built_disk, &stage_disk)?;
    // Factory-fresh zeroed data volume for the first boot so the second boot
    // can prove state actually persisted across a rebuild + reboot.
    if stage_data.exists() {
        fs::remove_file(&stage_data)?;
    }
    let fresh = fs::File::create(&stage_data)?;
    fresh.set_len(128 * 1024 * 1024)?;

    let boot_one_markers = vec![MARKER_FILE_WRITTEN.to_string()];
    let boot_one = bootlog::bounded_qemu_virtio_boot(&stage_disk, &stage_data, &boot_one_markers)?;
    let first_written = storage_selftest_ok(&boot_one.output);
    println!(
        "phase 1 result: storage selftest evidence={} markers_seen={} timed_out={}",
        first_written, boot_one.markers_seen, boot_one.timed_out
    );
    if !first_written {
        return Err(
            "upgrade test phase 1 failed: no positive storage selftest evidence on v-current boot"
                .into(),
        );
    }

    println!("=== upgrade matrix phase 2/2: rebuild + reboot over persisted state ===");
    let artifacts_two = build_for_platform(spec, false)?;
    let rebuilt_disk = create_platform_image(&artifacts_two)?;
    fs::copy(&rebuilt_disk, &stage_disk)?;
    // stage_data intentionally reused: it carries phase-1 persisted state.

    let boot_two = bootlog::bounded_qemu_virtio_boot(&stage_disk, &stage_data, &boot_one_markers)?;
    let second_written = storage_selftest_ok(&boot_two.output);
    let persisted_restored = boot_two.output.contains(MARKER_RESTORED);
    println!(
        "phase 2 result: storage selftest evidence={} persistence-restored-marker={} timed_out={}",
        second_written, persisted_restored, boot_two.timed_out
    );

    println!("\n== upgrade test matrix (qemu-virtio) ==");
    println!(
        "boot v-current storage selftest ....... {}",
        pass_fail(first_written)
    );
    println!("rebuild ................................ PASS (cargo rebuild completed)");
    println!(
        "boot upgraded image storage selftest .. {}",
        pass_fail(second_written)
    );
    println!(
        "persistence marker restored=1 ......... {}",
        pass_fail(persisted_restored)
    );

    if !second_written || !persisted_restored {
        return Err(
            "upgrade test matrix FAILED: persistence not proven across rebuild+reboot".into(),
        );
    }
    println!("result: PASS");
    Ok(())
}

/// Positive storage selftest evidence: the persisted file-write line with no
/// accompanying persist failure.
fn storage_selftest_ok(output: &str) -> bool {
    output.contains(MARKER_FILE_WRITTEN) && !output.contains(MARKER_PERSIST_FAILED)
}

fn pass_fail(ok: bool) -> &'static str {
    if ok { "PASS" } else { "FAIL" }
}

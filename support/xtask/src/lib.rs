//! ServiceOS development-tool library.
//!
//! The interactive binary (`main.rs`) and the e2e runner framework
//! (`tests/framework`) share canonical QEMU builders and boot-log driver via
//! `xtask-core`; the modules are re-exported under their historical paths so
//! internal call sites keep resolving (docs/test-plan.md §2.3).

pub mod ci;
pub mod cli;
pub mod e2e;
pub mod release;
pub mod upgrade;
pub mod validate;

// Shared core, re-exported at its historical module names.
pub use xtask_core::bootlog;
pub use xtask_core::build;
pub use xtask_core::bundle;
pub use xtask_core::image;
pub use xtask_core::platform;
pub use xtask_core::run;

/// Entry point moved verbatim from the historical binary root so behavior is
/// unchanged; `main.rs` is now a thin wrapper over this call.
pub fn try_main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = std::env::args().skip(1).collect::<Vec<String>>();
    let options = cli::Options::parse(arguments)?;
    match options.command {
        cli::CommandKind::CiMatrix => {
            ci::print_github_matrix();
            return Ok(());
        }
        cli::CommandKind::Release => return release::run_release(),
        cli::CommandKind::ReleaseVerify => {
            return release::run_release_verify(
                options.release_verify_manifest.as_deref(),
                options.release_verify_key.as_deref(),
            );
        }
        cli::CommandKind::TestUpgrade => return upgrade::run_test_upgrade(),
        cli::CommandKind::Validate => return validate::run_validate(),
        cli::CommandKind::TestE2e => {
            return e2e::run_test_e2e(options.platform, options.release, &options.e2e_extra);
        }
        _ => {}
    }
    if matches!(options.command, cli::CommandKind::Recover) {
        // Recovery boots build with the boot-mode flag baked into the platform
        // loader's root-manager startup word (and stage a bootmode.txt note).
        // SAFETY: single-threaded process; no other threads read env yet.
        unsafe {
            std::env::set_var("SERVICEOS_BOOT_MODE", "recovery");
        }
    }
    let spec = platform::PlatformSpec::resolve(options.platform)?;
    let artifacts = build::build_for_platform(spec, options.release)?;

    match options.command {
        cli::CommandKind::Build => {}
        cli::CommandKind::Image => {
            let _ = image::create_platform_image(&artifacts)?;
        }
        cli::CommandKind::Run | cli::CommandKind::Recover => {
            if matches!(options.command, cli::CommandKind::Recover) {
                println!("Recovery boot: SERVICEOS_BOOT_MODE=recovery");
            }
            let image = image::create_platform_image(&artifacts)?;
            run::run_platform(&artifacts, &image)?;
        }
        cli::CommandKind::CiMatrix => unreachable!("ci-matrix returns before platform resolution"),
        cli::CommandKind::Release
        | cli::CommandKind::ReleaseVerify
        | cli::CommandKind::TestUpgrade
        | cli::CommandKind::Validate => {
            unreachable!("release commands return before platform resolution")
        }
        cli::CommandKind::TestE2e => unreachable!("test-e2e returns before platform resolution"),
    }

    Ok(())
}

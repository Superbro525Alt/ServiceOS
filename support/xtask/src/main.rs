mod build;
mod bundle;
mod ci;
mod cli;
mod image;
mod platform;
mod run;

use std::error::Error;

use build::build_for_platform;
use ci::print_github_matrix;
use cli::{CommandKind, Options};
use image::create_platform_image;
use platform::PlatformSpec;
use run::run_platform;

fn main() -> Result<(), Box<dyn Error>> {
    let options = Options::parse(std::env::args().skip(1).collect())?;
    if matches!(options.command, CommandKind::CiMatrix) {
        print_github_matrix();
        return Ok(());
    }
    let spec = PlatformSpec::resolve(options.platform)?;
    let artifacts = build_for_platform(spec, options.release)?;

    match options.command {
        CommandKind::Build => {}
        CommandKind::Image => {
            let _ = create_platform_image(&artifacts)?;
        }
        CommandKind::Run => {
            let image = create_platform_image(&artifacts)?;
            run_platform(&artifacts, &image)?;
        }
        CommandKind::CiMatrix => unreachable!("ci-matrix returns before platform resolution"),
    }

    Ok(())
}

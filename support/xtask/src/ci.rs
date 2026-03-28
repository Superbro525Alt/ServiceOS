use crate::platform::PlatformSpec;

pub fn print_github_matrix() {
    let mut first = true;
    print!("{{\"include\":[");
    for spec in PlatformSpec::all().iter().copied() {
        for release in [false, true] {
            if !first {
                print!(",");
            }
            first = false;
            let profile = if release { "release" } else { "debug" };
            let mut rust_targets = [""; 2];
            let mut target_count = 0usize;
            rust_targets[target_count] = spec.userspace_rust_target();
            target_count += 1;
            if let Some(kernel_target) = spec.rust_target {
                if kernel_target != spec.userspace_rust_target() {
                    rust_targets[target_count] = kernel_target;
                    target_count += 1;
                }
            }
            print!(
                "{{\"platform\":\"{}\",\"profile\":\"{}\",\"release\":{},\"artifact\":\"{}-{}\",\"rust_targets\":\"{}\"}}",
                spec.name,
                profile,
                if release { "true" } else { "false" },
                spec.name,
                profile,
                rust_targets[..target_count].join(","),
            );
        }
    }
    println!("]}}");
}

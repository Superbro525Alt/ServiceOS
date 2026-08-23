use std::{env, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let link_script = manifest_dir.join("link.ld");

    println!("cargo:rerun-if-changed={}", link_script.display());
    println!("cargo:rustc-link-arg=-T{}", link_script.display());
}

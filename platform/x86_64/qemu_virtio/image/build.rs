fn main() {
    println!("cargo:rerun-if-env-changed=SERVICEOS_BOOT_MODE");
}

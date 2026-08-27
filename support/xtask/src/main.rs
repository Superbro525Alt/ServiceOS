fn main() {
    if let Err(err) = xtask::try_main() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

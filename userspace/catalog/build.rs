use std::{
    env,
    error::Error,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const IMAGE_BASE: u64 = 0x0000_4000_0000_0000;
const USER_STACK_TOP: u64 = 0x0000_7fff_ffff_0000;

const PROGRAMS: &[Program] = &[
    Program {
        package: "serviceos-root-service-manager",
        bin_name: "serviceos-root-service-manager",
        image_id: 1,
        include_name: "ROOT_MANAGER_IMAGE",
    },
    Program {
        package: "serviceos-log-service",
        bin_name: "serviceos-log-service",
        image_id: 2,
        include_name: "LOG_SERVICE_IMAGE",
    },
    Program {
        package: "serviceos-echo-service",
        bin_name: "serviceos-echo-service",
        image_id: 3,
        include_name: "ECHO_SERVICE_IMAGE",
    },
    Program {
        package: "serviceos-probe-service",
        bin_name: "serviceos-probe-service",
        image_id: 4,
        include_name: "PROBE_SERVICE_IMAGE",
    },
];

struct Program {
    package: &'static str,
    bin_name: &'static str,
    image_id: u32,
    include_name: &'static str,
}

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let repo_root = manifest_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let programs_root = repo_root.join("userspace").join("programs");
    let target_dir = repo_root.join("target").join("userspace-programs");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    println!("cargo:rerun-if-changed={}", programs_root.display());

    fs::create_dir_all(&target_dir)?;
    fs::create_dir_all(&out_dir)?;

    let mut generated = String::new();
    for program in PROGRAMS {
        build_program(&programs_root, &target_dir, program)?;
        let elf = target_dir
            .join("x86_64-unknown-none")
            .join("debug")
            .join(program.bin_name);
        let raw = out_dir.join(format!("{}.bin", program.bin_name));
        let image = out_dir.join(format!("{}.img", program.bin_name));
        objcopy_binary(&elf, &raw)?;
        wrap_flat_image(&raw, &image)?;

        writeln!(
            generated,
            "pub static {}: &[u8] = include_bytes!(r#\"{}\"#);",
            program.include_name,
            image.display()
        )?;
    }

    generated.push_str("pub fn resolve_image(image_id: u32) -> Option<&'static [u8]> {\n");
    generated.push_str("    match image_id {\n");
    for program in PROGRAMS {
        writeln!(
            generated,
            "        {} => Some({}),",
            program.image_id, program.include_name
        )?;
    }
    generated.push_str("        _ => None,\n");
    generated.push_str("    }\n");
    generated.push_str("}\n");

    fs::write(out_dir.join("catalog.rs"), generated)?;

    Ok(())
}

fn build_program(
    programs_root: &Path,
    target_dir: &Path,
    program: &Program,
) -> Result<(), Box<dyn Error>> {
    let link_script = programs_root.join("link.ld");
    let status = Command::new("cargo")
        .current_dir(programs_root)
        .env("CARGO_TARGET_DIR", target_dir)
        .args([
            "rustc",
            "--target",
            "x86_64-unknown-none",
            "-p",
            program.package,
            "--bin",
            program.bin_name,
            "--",
            "-C",
            "relocation-model=static",
            "-C",
            "code-model=large",
            "-C",
            &format!("link-arg=-T{}", link_script.display()),
            "-C",
            "link-arg=--gc-sections",
        ])
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("failed to build {}", program.package).into())
    }
}

fn objcopy_binary(input: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    let status = Command::new("/usr/sbin/llvm-objcopy")
        .args(["-O", "binary"])
        .arg(input)
        .arg(output)
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("llvm-objcopy failed for {}", input.display()).into())
    }
}

fn wrap_flat_image(raw: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    let code = fs::read(raw)?;
    let mut image = Vec::with_capacity(48 + code.len());
    image.extend_from_slice(b"SOSUIMG\0");
    image.extend_from_slice(&1u32.to_le_bytes());
    image.extend_from_slice(&48u32.to_le_bytes());
    image.extend_from_slice(&IMAGE_BASE.to_le_bytes());
    image.extend_from_slice(&0u64.to_le_bytes());
    image.extend_from_slice(&(code.len() as u64).to_le_bytes());
    image.extend_from_slice(&USER_STACK_TOP.to_le_bytes());
    image.extend_from_slice(&code);
    fs::write(output, image)?;
    Ok(())
}

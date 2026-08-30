use std::{
    error::Error,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use crate::{
    build::{
        BuildArtifacts, build_for_platform, build_userspace_catalog, userspace_bootstore_path,
        workspace_root,
    },
    image::create_platform_image,
    platform::{ImageKind, PlatformSpec},
};

const HASH_ALGORITHM: &str = "fnv1a64";
const INTEGRITY_HASH_ALGORITHM: &str = "sha256";

#[derive(Debug)]
enum ReleaseStatus {
    /// Every artifact built with the release profile.
    FullRelease,
    /// Kernel/platform packages are release; the embedded userspace programs
    /// (bootstore.bin) fell back to debug because release-profile userspace
    /// linking currently fails upstream (lld .text/.got overlap under the
    /// PIC + large code-model userspace flags).
    MixedUserspaceDebug,
    /// The platform could not build with the release profile at all; the
    /// recorded artifacts are a full debug-profile build instead.
    DebugFallback(String),
    /// The platform produced no artifacts whatsoever.
    Failed(String),
}

struct PlatformEntry {
    spec: PlatformSpec,
    status: ReleaseStatus,
    artifacts: Vec<ArtifactRecord>,
}

impl PlatformEntry {
    fn status_name(&self) -> &'static str {
        match self.status {
            ReleaseStatus::FullRelease => "release",
            ReleaseStatus::MixedUserspaceDebug => "release+debug-userspace",
            ReleaseStatus::DebugFallback(_) => "debug-fallback",
            ReleaseStatus::Failed(_) => "failed",
        }
    }
}

struct ArtifactRecord {
    relative_path: String,
    size: u64,
    hash: u64,
    sha256: String,
}

pub fn run_release() -> Result<(), Box<dyn Error>> {
    let root = workspace_root();
    let out_dir = root.join("target").join("release");
    fs::create_dir_all(&out_dir)?;

    let mut entries: Vec<PlatformEntry> = Vec::new();
    for spec in PlatformSpec::all().iter().copied() {
        println!("\n=== release build: {} ===", spec.name);
        entries.push(release_platform(spec));
    }

    let total_artifacts: usize = entries.iter().map(|entry| entry.artifacts.len()).sum();
    let manifest_path = out_dir.join("RELEASE-MANIFEST.json");
    write_manifest(&manifest_path, &entries)?;
    println!(
        "\nWrote artifact manifest: {} ({} files across {} platforms)",
        manifest_path.display(),
        total_artifacts,
        entries.len()
    );

    let mut any_failed = false;
    for entry in &entries {
        println!("  {:<12} {}", entry.spec.name, entry.status_name());
        if matches!(entry.status, ReleaseStatus::Failed(_)) {
            any_failed = true;
        }
    }
    // A debug-fallback entry is still a produced, installable image set and
    // is reported in the manifest; only total failure blocks success.
    if any_failed {
        return Err("one or more platforms failed to produce release artifacts".into());
    }
    Ok(())
}

fn release_platform(spec: PlatformSpec) -> PlatformEntry {
    match build_for_platform(spec, true) {
        Ok(artifacts) => match create_platform_image(&artifacts) {
            Ok(bundle) => {
                println!(
                    "release bundle for {} ready at {}",
                    spec.name,
                    bundle.display()
                );
                collect_artifacts(&artifacts, ReleaseStatus::FullRelease)
            }
            Err(error) => failed_entry(spec, error.to_string()),
        },
        Err(build_error) => {
            let message = build_error.to_string();
            if spec.image_kind == ImageKind::RawDisk
                && message.contains("serviceos-userspace-catalog")
            {
                match release_with_debug_userspace(spec) {
                    Ok(artifacts) => {
                        println!(
                            "NOTE: {} released with debug-built userspace programs \
                             (upstream release-link regression in nested userspace builds)",
                            spec.name
                        );
                        match create_platform_image(&artifacts) {
                            Ok(bundle) => {
                                println!(
                                    "release bundle for {} ready at {}",
                                    spec.name,
                                    bundle.display()
                                );
                                collect_artifacts(&artifacts, ReleaseStatus::MixedUserspaceDebug)
                            }
                            Err(error) => failed_entry(spec, error.to_string()),
                        }
                    }
                    Err(fallback_error) => {
                        let combined = format!("{message}; fallback also failed: {fallback_error}");
                        full_debug_fallback(spec, combined)
                    }
                }
            } else {
                full_debug_fallback(spec, message)
            }
        }
    }
}

/// Last resort for platforms whose release profile is currently broken
/// upstream: produce a bootable debug-profile image set and record why.
fn full_debug_fallback(spec: PlatformSpec, reason: String) -> PlatformEntry {
    match build_for_platform(spec, false) {
        Ok(artifacts) => match create_platform_image(&artifacts) {
            Ok(bundle) => {
                println!(
                    "NOTE: {} fell back to a full debug-profile image \
                     (release profile is currently broken)",
                    spec.name
                );
                println!(
                    "debug-fallback bundle for {} ready at {}",
                    spec.name,
                    bundle.display()
                );
                collect_artifacts(&artifacts, ReleaseStatus::DebugFallback(reason))
            }
            Err(image_error) => failed_entry(spec, format!("{reason}; {image_error}")),
        },
        Err(debug_error) => failed_entry(
            spec,
            format!("{reason}; debug fallback failed: {debug_error}"),
        ),
    }
}

/// Recovery path for file-bootstore platforms: rebuild the userspace catalog
/// in debug (nested user program builds succeed there), transplant the
/// resulting bootstore.bin into the release profile directory, and continue.
fn release_with_debug_userspace(spec: PlatformSpec) -> Result<BuildArtifacts, Box<dyn Error>> {
    build_userspace_catalog(spec, false)?;
    let debug_bootstore = userspace_bootstore_path(spec, false);
    let release_bootstore = userspace_bootstore_path(spec, true);
    fs::create_dir_all(release_bootstore.parent().unwrap())?;
    fs::copy(&debug_bootstore, &release_bootstore)?;

    let root = workspace_root();
    let kernel_binary = spec.kernel_binary_path(&root, "release");
    Ok(BuildArtifacts {
        spec,
        release: true,
        bootstore_binary: release_bootstore,
        kernel_binary,
        image_root: spec.image_root(&root, "release"),
    })
}

fn failed_entry(spec: PlatformSpec, reason: String) -> PlatformEntry {
    eprintln!("release build FAILED for {}: {}", spec.name, reason);
    PlatformEntry {
        spec,
        status: ReleaseStatus::Failed(reason),
        artifacts: Vec::new(),
    }
}

fn collect_artifacts(artifacts: &BuildArtifacts, status: ReleaseStatus) -> PlatformEntry {
    let root = workspace_root();
    let mut files: Vec<PathBuf> = Vec::new();
    let _ = collect_files(&artifacts.image_root, &mut files);
    files.sort();
    let records = files
        .into_iter()
        .filter_map(|file| {
            let metadata = file.metadata().ok()?;
            let size = metadata.len();
            let hash = fnv1a64_file(&file).ok()?;
            let sha256 = sha256_file(&file).ok()?;
            let relative_path = file
                .strip_prefix(&root)
                .unwrap_or(&file)
                .to_string_lossy()
                .into_owned();
            println!(
                "  {:<48} {:>10} B fnv1a64={:016x} sha256={}",
                relative_path,
                size,
                hash,
                sha256_hex(&sha256)
            );
            Some(ArtifactRecord {
                relative_path,
                size,
                hash,
                sha256: sha256_hex(&sha256),
            })
        })
        .collect();
    PlatformEntry {
        spec: artifacts.spec,
        status,
        artifacts: records,
    }
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    if !dir.exists() {
        return Ok(());
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    for entry in entries {
        if entry.is_dir() {
            collect_files(&entry, out)?;
        } else {
            out.push(entry);
        }
    }
    Ok(())
}

/// FNV-1a 64-bit over the file bytes. Retained alongside sha256 for
/// backcompat with existing manifest readers; the manifest names both
/// algorithms explicitly.
fn fnv1a64_file(path: &Path) -> Result<u64, Box<dyn Error>> {
    let mut file = fs::File::open(path)?;
    let mut buffer = [0u8; 64 * 1024];
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        for byte in &buffer[..read] {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    Ok(hash)
}

/// SHA-256 over the file bytes, streamed in fixed chunks through the
/// in-repo `serviceos-crypto` implementation (no external dependencies).
fn sha256_file(path: &Path) -> Result<[u8; 32], Box<dyn Error>> {
    let mut file = fs::File::open(path)?;
    let mut buffer = [0u8; 64 * 1024];
    let mut hasher = serviceos_crypto::sha256::Sha256::new();
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize())
}

fn sha256_hex(digest: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                escaped.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => escaped.push(other),
        }
    }
    escaped
}

fn write_manifest(path: &Path, entries: &[PlatformEntry]) -> Result<(), Box<dyn Error>> {
    let mut json = String::from("{\n");
    json.push_str("  \"manifest\": \"serviceos-release\",\n");
    json.push_str("  \"profile\": \"release\",\n");
    json.push_str(format!("  \"hash_algorithm\": \"{HASH_ALGORITHM}\",\n").as_str());
    json.push_str(
        format!("  \"integrity_hash_algorithm\": \"{INTEGRITY_HASH_ALGORITHM}\",\n").as_str(),
    );
    // Deliberately timestamp-free: the manifest itself stays byte-stable
    // across rebuilds of identical inputs.
    json.push_str("  \"reproducibility\": {\n");
    json.push_str("    \"status\": \"partial\",\n");
    json.push_str("    \"not_yet_reproducible\": [\n");
    json.push_str(
        "      \"FAT disk images embed creation/modification timestamps written by mformat/mcopy\",\n",
    );
    json.push_str(
        "      \"staged bundle files inherit filesystem timestamps from the build host\",\n",
    );
    json.push_str(
        "      \"toolchain and host tool versions (rustc, llvm-objcopy, mtools, QEMU) are not pinned\"\n",
    );
    json.push_str("    ],\n");
    json.push_str(
        "    \"notes\": \"every artifact carries a 64-bit FNV-1a hash (legacy, kept for backcompat with existing manifest readers) plus a sha256 integrity hash computed in-repo by serviceos-crypto; the sha256 hash is an integrity digest, not a signature; platforms marked release+debug-userspace embed debug-built bootstore.bin because release-profile nested userspace links currently fail (rust-lld .text/.got overlap)\"\n",
    );
    json.push_str("  },\n");
    json.push_str("  \"platforms\": [\n");

    for (index, entry) in entries.iter().enumerate() {
        json.push_str("    {\n");
        json.push_str(&format!("      \"name\": \"{}\",\n", entry.spec.name));
        json.push_str(&format!("      \"status\": \"{}\",\n", entry.status_name()));
        if let ReleaseStatus::Failed(reason) | ReleaseStatus::DebugFallback(reason) = &entry.status
        {
            json.push_str(&format!(
                "      \"failure_reason\": \"{}\",\n",
                json_escape(reason)
            ));
        }
        json.push_str("      \"artifacts\": [");
        if entry.artifacts.is_empty() {
            json.push_str("]\n");
        } else {
            json.push('\n');
            for (artifact_index, record) in entry.artifacts.iter().enumerate() {
                json.push_str(&format!(
                    "        {{\"path\": \"{}\", \"size\": {}, \"hash\": \"{:016x}\", \"sha256\": \"{}\"}}{}\n",
                    json_escape(&record.relative_path),
                    record.size,
                    record.hash,
                    record.sha256,
                    if artifact_index + 1 < entry.artifacts.len() {
                        ","
                    } else {
                        ""
                    }
                ));
            }
            json.push_str("      ]\n");
        }
        json.push_str(&format!(
            "    }}{}\n",
            if index + 1 < entries.len() { "," } else { "" }
        ));
    }
    json.push_str("  ]\n}\n");

    fs::write(path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn sha256_hex_matches_known_vector() {
        let digest = serviceos_crypto::sha256::digest(&[b"abc"]);
        assert_eq!(
            sha256_hex(&digest),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_file_hashes_fixture_like_the_manifest_writer() {
        // Same streaming code path collect_artifacts uses for manifest
        // entries, exercised on a real on-disk fixture.
        let payload = b"serviceos release fixture 0123456789abcdef\n";
        let path = env::temp_dir().join(format!(
            "serviceos-xtask-sha256-{}",
            serviceos_crypto::sha256::digest(&[&payload[..]])[0]
        ));
        fs::write(&path, payload).expect("write fixture");
        let digest = sha256_file(&path).expect("hash fixture");
        let _ = fs::remove_file(&path);

        // Cross-checked against coreutils sha256sum on the same bytes.
        assert_eq!(
            sha256_hex(&digest),
            "105679abc79909b86e9a967caf1580e0668e1e4bbeff5c0755b85ccb39337e3c"
        );
    }

    #[test]
    fn sha256_file_streams_large_fixture() {
        let path = env::temp_dir().join("serviceos-xtask-sha256-large");
        fs::write(&path, vec![b'a'; 200_000]).expect("write fixture");
        let digest = sha256_file(&path).expect("hash fixture");
        let _ = fs::remove_file(&path);
        assert_eq!(
            sha256_hex(&digest),
            // sha256("a" x 200000), cross-checked against coreutils.
            "2287d207f24a941ff3b56c04c8a25ad56b63e3023207b3bb5b4ac0c9869d74be"
        );
    }
}

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
const SIGNATURE_ALGORITHM: &str = "ed25519";
/// Env var pointing at a file holding the hex-encoded 32-byte ed25519 seed.
/// When set, `cargo xtask release` signs RELEASE-MANIFEST.json; when unset
/// (the default) the manifest is byte-identical to the unsigned format.
const SIGNING_KEY_ENV: &str = "SERVICEOS_RELEASE_SIGNING_KEY";
/// Env var fallback for the 64-hex-character ed25519 public key used by
/// `cargo xtask release-verify` when `--key` is not passed.
const VERIFY_KEY_ENV: &str = "SERVICEOS_RELEASE_VERIFY_KEY";
/// Documented signing scheme, embedded in the manifest notes whenever the
/// manifest is actually signed (the unsigned manifest stays byte-identical
/// to the pre-signing format and carries no signing prose).
const SIGNING_SCHEME_NOTE: &str = "; when SERVICEOS_RELEASE_SIGNING_KEY is set the manifest gains a trailing signature member computed as ed25519 (RFC 8032) over the sha256 of this exact manifest text with the signature member removed (the canonical pre-signature serialization, including this notes text); the signature key_id is the fnv1a64 of the 32-byte ed25519 public key rendered as 16 hex characters; installer image entries are emitted inside the canonical bytes and are covered by the signature when present";

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

/// Data-volume size for the composed installer image, mirroring the fresh
/// data image `create_qemu_disk_image` stages beside the boot disk.
const INSTALLER_DATA_IMAGE_SIZE_MB: u64 = 128;

const INSTALLER_FIRST_BOOT_NOTE: &str = "fresh empty data volume; first boot runs the setup wizard with headless silent defaults, and the admin marker persists on the data volume";

struct InstallerRecord {
    platform: String,
    directory: String,
    first_boot: &'static str,
    files: Vec<ArtifactRecord>,
}

pub fn run_release() -> Result<(), Box<dyn Error>> {
    let root = workspace_root();
    let out_dir = root.join("target").join("release");
    fs::create_dir_all(&out_dir)?;

    let mut entries: Vec<PlatformEntry> = Vec::new();
    let mut installers: Vec<InstallerRecord> = Vec::new();
    let mut installer_error: Option<String> = None;
    for spec in PlatformSpec::all().iter().copied() {
        println!("\n=== release build: {} ===", spec.name);
        let entry = release_platform(spec);
        match stage_installer(&root, &entry) {
            Ok(Some(record)) => installers.push(record),
            Ok(None) => {}
            Err(error) => {
                // Degrade like the per-platform build path does: keep
                // building and manifesting the remaining platforms, but
                // still fail the release loudly at the end.
                installer_error = Some(error.to_string());
                println!("  installer staging FAILED: {}", error);
            }
        }
        entries.push(entry);
    }

    let total_artifacts: usize = entries.iter().map(|entry| entry.artifacts.len()).sum();
    let manifest_path = out_dir.join("RELEASE-MANIFEST.json");
    write_manifest(&manifest_path, &entries, &installers)?;
    println!(
        "\nWrote artifact manifest: {} ({} files across {} platforms, {} installer image{})",
        manifest_path.display(),
        total_artifacts,
        entries.len(),
        installers.len(),
        if installers.len() == 1 { "" } else { "s" }
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
    if let Some(error) = installer_error {
        return Err(format!("installer staging failed: {error}").into());
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

/// Stage the installer artifact for a released platform: the existing
/// bootable composition (kernel + boot store + services inside the boot
/// disk) paired with a freshly composed EMPTY data volume, so first boot on
/// a new machine runs the documented setup-wizard chain. Reuses the
/// artifacts the release build already staged — zero rebuilds. Only the
/// qemu-virtio platform is staged this round; other platforms' boot media
/// shapes differ and remain open.
fn stage_installer(
    root: &Path,
    entry: &PlatformEntry,
) -> Result<Option<InstallerRecord>, Box<dyn Error>> {
    if entry.spec.name != "qemu-virtio" {
        return Ok(None);
    }
    if !matches!(entry.status, ReleaseStatus::FullRelease) {
        println!(
            "  installer staging skipped for {}: platform status is '{}'",
            entry.spec.name,
            entry.status_name()
        );
        return Ok(None);
    }
    let image_root = entry.spec.image_root(root, "release");
    let boot_image = image_root.join("serviceos.img");
    if !boot_image.exists() {
        return Err(format!(
            "qemu-virtio installer staging: boot image missing: {}",
            boot_image.display()
        )
        .into());
    }
    let installer_dir = root
        .join("target")
        .join("release")
        .join("installer")
        .join(entry.spec.name);
    let files = compose_installer(root, &installer_dir, &boot_image)?;
    let directory = installer_dir
        .strip_prefix(root)
        .unwrap_or(&installer_dir)
        .to_string_lossy()
        .into_owned();
    println!(
        "  installer image staged at {} ({} files, fresh data volume)",
        installer_dir.display(),
        files.len()
    );
    Ok(Some(InstallerRecord {
        platform: entry.spec.name.to_string(),
        directory,
        first_boot: INSTALLER_FIRST_BOOT_NOTE,
        files,
    }))
}

/// Compose the installer directory: wipe any previous composition (a stale
/// data volume may have been mutated by earlier boots — the installer must
/// always ship data-fresh), copy the boot disk, create a fresh sparse data
/// volume, and write the README. Returns manifest records for every file in
/// the directory.
fn compose_installer(
    root: &Path,
    installer_dir: &Path,
    boot_image: &Path,
) -> Result<Vec<ArtifactRecord>, Box<dyn Error>> {
    if installer_dir.exists() {
        fs::remove_dir_all(installer_dir)?;
    }
    fs::create_dir_all(installer_dir)?;

    let staged_boot = installer_dir.join("serviceos.img");
    fs::copy(boot_image, &staged_boot)?;

    let data_image = installer_dir.join("serviceos-data.img");
    let file = fs::File::create(&data_image)?;
    file.set_len(INSTALLER_DATA_IMAGE_SIZE_MB * 1024 * 1024)?;
    drop(file);

    fs::write(installer_dir.join("README.txt"), installer_readme())?;

    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(installer_dir, &mut files)?;
    files.sort();
    let mut records = Vec::new();
    for file in files {
        let metadata = file.metadata()?;
        let sha256 = sha256_hex(&sha256_file(&file)?);
        let relative_path = file
            .strip_prefix(root)
            .unwrap_or(&file)
            .to_string_lossy()
            .into_owned();
        println!(
            "  installer {:<48} {:>10} B sha256={}",
            relative_path,
            metadata.len(),
            sha256
        );
        records.push(ArtifactRecord {
            relative_path,
            size: metadata.len(),
            hash: fnv1a64_file(&file)?,
            sha256,
        });
    }
    Ok(records)
}

/// README shipped inside the installer directory, telling a fresh machine
/// what it is looking at and what first boot does.
fn installer_readme() -> String {
    let mut text = String::new();
    text.push_str("ServiceOS installer image (qemu-virtio)\n");
    text.push_str("========================================\n\n");
    text.push_str("Contents\n");
    text.push_str("  serviceos.img       boot disk (UEFI ESP: kernel + boot store + services)\n");
    text.push_str("  serviceos-data.img  empty data volume, freshly composed at release time\n\n");
    text.push_str("First boot\n");
    text.push_str("  Attach the boot disk and the data volume to the machine and boot.\n");
    text.push_str("  The empty data volume triggers the documented first-boot chain:\n");
    text.push_str("  the setup wizard runs with headless silent defaults, records the\n");
    text.push_str("  hostname and timezone, creates the admin account, and writes\n");
    text.push_str("  state/setup-wizard/firstboot.done on the data volume. The admin\n");
    text.push_str("  marker persists on the data volume; later boots skip setup and go\n");
    text.push_str("  straight to the desktop.\n\n");
    text.push_str("QEMU smoke\n");
    text.push_str("  cargo xtask run --platform qemu-virtio --release boots this exact\n");
    text.push_str("  composition (delete target/images/release/qemu-virtio/serviceos-data.img\n");
    text.push_str("  first to reproduce the fresh-data condition). Manual QEMU runs need\n");
    text.push_str("  UEFI firmware (OVMF) and two virtio-blk drives: boot disk, then data\n");
    text.push_str("  volume, in that order.\n");
    text
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

fn write_manifest(
    path: &Path,
    entries: &[PlatformEntry],
    installers: &[InstallerRecord],
) -> Result<(), Box<dyn Error>> {
    let signing_key = match std::env::var(SIGNING_KEY_ENV) {
        Ok(value) if !value.trim().is_empty() => Some(PathBuf::from(value.trim())),
        Ok(_) => None,
        Err(std::env::VarError::NotPresent) => None,
        Err(err) => return Err(format!("{SIGNING_KEY_ENV}: {err}").into()),
    };
    write_manifest_with_signing(path, entries, installers, signing_key.as_deref())
}

/// Write the manifest, optionally signed. `signing_key` (when given) is a
/// file holding the hex-encoded 32-byte ed25519 seed; the key file is read
/// from disk and never echoed: error messages name the env var, never the
/// key material.
fn write_manifest_with_signing(
    path: &Path,
    entries: &[PlatformEntry],
    installers: &[InstallerRecord],
    signing_key: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    let seed = signing_key
        .map(load_signing_seed_file)
        .transpose()
        .map_err(|reason| -> Box<dyn Error> { reason.into() })?;

    let unsigned_json = build_manifest_json(
        entries,
        installers,
        seed.is_some().then_some(SIGNING_SCHEME_NOTE),
    );
    let manifest_text = match &seed {
        Some(seed) => {
            let public = serviceos_crypto::ed25519::public_key(seed);
            let text = append_manifest_signature(&unsigned_json, seed);
            println!(
                "Release manifest signed: {SIGNATURE_ALGORITHM} key_id {}",
                manifest_key_id(&public)
            );
            text
        }
        None => unsigned_json,
    };
    fs::write(path, manifest_text)?;
    Ok(())
}

/// Build the canonical manifest JSON. `signing_scheme_note` (when given) is
/// appended to the reproducibility notes so the signed manifest documents
/// its own verification scheme; the unsigned manifest omits it, keeping the
/// default output byte-identical to the pre-signing format. `installers`
/// (when non-empty) is emitted as an additive trailing array; the empty
/// case keeps the output byte-identical to the pre-installer format.
fn build_manifest_json(
    entries: &[PlatformEntry],
    installers: &[InstallerRecord],
    signing_scheme_note: Option<&str>,
) -> String {
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
    let mut notes = String::from(
        "every artifact carries a 64-bit FNV-1a hash (legacy, kept for backcompat with existing manifest readers) plus a sha256 integrity hash computed in-repo by serviceos-crypto; the sha256 hash is an integrity digest, not a signature; platforms marked release+debug-userspace embed debug-built bootstore.bin because release-profile nested userspace links currently fail (rust-lld .text/.got overlap)",
    );
    if let Some(note) = signing_scheme_note {
        notes.push_str(note);
    }
    json.push_str(&format!("    \"notes\": \"{}\"\n", json_escape(&notes)));
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
    if installers.is_empty() {
        json.push_str("  ]\n}\n");
        return json;
    }
    json.push_str("  ],\n");
    json.push_str("  \"installers\": [\n");
    for (index, installer) in installers.iter().enumerate() {
        json.push_str("    {\n");
        json.push_str(&format!(
            "      \"platform\": \"{}\",\n",
            json_escape(&installer.platform)
        ));
        json.push_str(&format!(
            "      \"directory\": \"{}\",\n",
            json_escape(&installer.directory)
        ));
        json.push_str(&format!(
            "      \"first_boot\": \"{}\",\n",
            json_escape(installer.first_boot)
        ));
        json.push_str("      \"artifacts\": [");
        if installer.files.is_empty() {
            json.push_str("]\n");
        } else {
            json.push('\n');
            for (artifact_index, record) in installer.files.iter().enumerate() {
                json.push_str(&format!(
                    "        {{\"path\": \"{}\", \"size\": {}, \"hash\": \"{:016x}\", \"sha256\": \"{}\"}}{}\n",
                    json_escape(&record.relative_path),
                    record.size,
                    record.hash,
                    record.sha256,
                    if artifact_index + 1 < installer.files.len() {
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
            if index + 1 < installers.len() {
                ","
            } else {
                ""
            }
        ));
    }
    json.push_str("  ]\n}\n");
    json
}

fn load_signing_seed_file(path: &Path) -> Result<[u8; 32], String> {
    let raw = fs::read_to_string(path).map_err(|err| {
        format!(
            "{SIGNING_KEY_ENV}: cannot read signing key file {}: {err}",
            path.display()
        )
    })?;
    parse_seed_hex(raw.trim())
}

fn parse_seed_hex(text: &str) -> Result<[u8; 32], String> {
    let bytes = parse_hex_strict(text).ok_or_else(|| {
        format!("{SIGNING_KEY_ENV}: signing key must be 64 hex characters (32 bytes)")
    })?;
    if bytes.len() != 32 {
        return Err(format!(
            "{SIGNING_KEY_ENV}: signing key must be 64 hex characters (32 bytes), got {} bytes",
            bytes.len()
        ));
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&bytes);
    Ok(seed)
}

/// Strict hex decode: even length, hex digits only, no sign/whitespace
/// tolerance (the key file may be newline-terminated; callers trim first).
fn parse_hex_strict(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if bytes.is_empty() || bytes.len() % 2 != 0 {
        return None;
    }
    (0..bytes.len() / 2)
        .map(|i| Some(hex_nibble(bytes[i * 2])? << 4 | hex_nibble(bytes[i * 2 + 1])?))
        .collect()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn fnv1a64_bytes(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Signature key_id: fnv1a64 over the 32-byte ed25519 public key, 16 hex.
fn manifest_key_id(public: &[u8; 32]) -> String {
    format!("{:016x}", fnv1a64_bytes(public))
}

/// The trailing `signature` member (without its leading comma), formatted
/// exactly as `split_manifest_signature` expects to find it.
fn signature_member(public: &[u8; 32], signature: &[u8; 64]) -> String {
    let mut member = String::from("  \"signature\": {\n");
    member.push_str(&format!("    \"algorithm\": \"{SIGNATURE_ALGORITHM}\",\n"));
    member.push_str(&format!(
        "    \"key_id\": \"{}\",\n",
        manifest_key_id(public)
    ));
    member.push_str("    \"signature\": \"");
    for byte in signature {
        member.push_str(&format!("{byte:02x}"));
    }
    member.push_str("\"\n  }");
    member
}

/// Sign the canonical pre-signature manifest bytes and append the trailing
/// `signature` member. The signed message is the sha256 of the exact
/// unsigned serialization (including any signing scheme note).
fn append_manifest_signature(unsigned_json: &str, seed: &[u8; 32]) -> String {
    let public = serviceos_crypto::ed25519::public_key(seed);
    let digest = serviceos_crypto::sha256::digest(&[unsigned_json.as_bytes()]);
    let signature = serviceos_crypto::ed25519::sign(seed, &digest);
    debug_assert!(unsigned_json.ends_with("\n}\n"));
    let body = &unsigned_json[..unsigned_json.len() - 3];
    format!("{body},\n{}\n}}\n", signature_member(&public, &signature))
}

/// Split a signed manifest into its `signature` member text and the exact
/// canonical bytes the signature was computed over (the manifest with the
/// trailing signature member removed). Returns `None` when the manifest
/// carries no signature member.
fn split_manifest_signature(manifest: &str) -> Option<(String, String)> {
    if !manifest.ends_with("\n}\n") {
        return None;
    }
    let comma = manifest.rfind(",\n  \"signature\": {")?;
    let member = manifest[comma + 2..manifest.len() - 3].to_string();
    let canonical = format!("{}\n}}\n", &manifest[..comma]);
    Some((member, canonical))
}

/// Pull a `"field": "value"` scalar out of the signature member text.
fn member_field(member: &str, field: &str) -> Option<String> {
    let marker = format!("\"{field}\": \"");
    let start = member.find(&marker)? + marker.len();
    let rest = &member[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Verify a manifest's trailing signature against a 32-byte ed25519 public
/// key. The scheme is the one documented in SIGNING_SCHEME_NOTE: ed25519
/// over the sha256 of the exact manifest bytes with the signature member
/// removed.
fn verify_manifest_signature(manifest: &str, public: &[u8; 32]) -> Result<(), String> {
    let Some((member, canonical)) = split_manifest_signature(manifest) else {
        return Err("manifest carries no signature member".to_string());
    };
    let algorithm =
        member_field(&member, "algorithm").ok_or("signature member missing algorithm")?;
    if algorithm != SIGNATURE_ALGORITHM {
        return Err(format!("unsupported signature algorithm: {algorithm}"));
    }
    let key_id = member_field(&member, "key_id").ok_or("signature member missing key_id")?;
    let expected_key_id = manifest_key_id(public);
    if key_id != expected_key_id {
        return Err(format!(
            "key_id mismatch: manifest signed by {key_id}, supplied key is {expected_key_id}"
        ));
    }
    let signature_hex =
        member_field(&member, "signature").ok_or("signature member missing signature")?;
    let signature_bytes =
        parse_hex_strict(&signature_hex).ok_or("signature field is not valid hex")?;
    let signature: [u8; 64] = signature_bytes
        .try_into()
        .map_err(|_| "signature field must be 128 hex characters (64 bytes)".to_string())?;
    let digest = serviceos_crypto::sha256::digest(&[canonical.as_bytes()]);
    if !serviceos_crypto::ed25519::verify(public, &digest, &signature) {
        return Err(
            "ed25519 verification failed: manifest content does not match signature".to_string(),
        );
    }
    Ok(())
}

fn resolve_verify_key(key_hex: Option<&str>) -> Result<[u8; 32], Box<dyn Error>> {
    let text = match key_hex {
        Some(hex) => hex.to_string(),
        None => std::env::var(VERIFY_KEY_ENV).map_err(|_| {
            format!("no verification key: pass --key <64-hex public key> or set {VERIFY_KEY_ENV}")
        })?,
    };
    let bytes = parse_hex_strict(text.trim())
        .ok_or("verification key must be 64 hex characters (32 bytes)")?;
    if bytes.len() != 32 {
        return Err("verification key must be 64 hex characters (32 bytes)".into());
    }
    let mut public = [0u8; 32];
    public.copy_from_slice(&bytes);
    Ok(public)
}

/// `cargo xtask release-verify [manifest] [--key <64-hex public key>]`.
/// Checks a signed RELEASE-MANIFEST.json against a supplied ed25519 public
/// key (arg or SERVICEOS_RELEASE_VERIFY_KEY). An unsigned manifest is a
/// graceful no-op (reported, exit 0); a signed manifest that fails
/// verification is an error (exit nonzero).
pub fn run_release_verify(
    manifest: Option<&str>,
    key_hex: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    let manifest_path = match manifest {
        Some(path) => PathBuf::from(path),
        None => workspace_root().join("target/release/RELEASE-MANIFEST.json"),
    };
    let text = fs::read_to_string(&manifest_path)
        .map_err(|err| format!("cannot read manifest {}: {err}", manifest_path.display()))?;
    if !text.contains("\"manifest\": \"serviceos-release\"") {
        return Err(format!(
            "{} is not a serviceos release manifest",
            manifest_path.display()
        )
        .into());
    }
    // Unsigned manifests are a graceful no-op: reported, exit 0, no key
    // needed. Key resolution errors only matter once there is a signature.
    if split_manifest_signature(&text).is_none() {
        println!(
            "{}: unsigned (no signature member); nothing to verify",
            manifest_path.display()
        );
        return Ok(());
    }
    let public = resolve_verify_key(key_hex)?;
    match verify_manifest_signature(&text, &public) {
        Ok(()) => println!(
            "{}: signature OK ({SIGNATURE_ALGORITHM}, key_id {})",
            manifest_path.display(),
            manifest_key_id(&public)
        ),
        Err(reason) => {
            return Err(format!("{}: signature INVALID: {reason}", manifest_path.display()).into());
        }
    }
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

    // --- release manifest signing ---

    fn fixture_entries() -> Vec<PlatformEntry> {
        vec![
            PlatformEntry {
                spec: PlatformSpec::all()[0],
                status: ReleaseStatus::FullRelease,
                artifacts: vec![ArtifactRecord {
                    relative_path: "images/qemu-virtio/serviceos.img".to_string(),
                    size: 1234,
                    hash: 0x0123_4567_89ab_cdef,
                    sha256: "aa".repeat(32),
                }],
            },
            PlatformEntry {
                spec: PlatformSpec::all()[1],
                status: ReleaseStatus::MixedUserspaceDebug,
                artifacts: Vec::new(),
            },
        ]
    }

    fn fixture_seed() -> [u8; 32] {
        [0x42u8; 32]
    }

    #[test]
    fn fnv1a64_bytes_matches_known_vectors() {
        // FNV-1a 64 reference vectors (empty string = offset basis, "a").
        assert_eq!(fnv1a64_bytes(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64_bytes(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn signature_roundtrip_on_fixture_manifest() {
        let seed = fixture_seed();
        let public = serviceos_crypto::ed25519::public_key(&seed);
        let unsigned = build_manifest_json(&fixture_entries(), &[], Some(SIGNING_SCHEME_NOTE));
        let signed = append_manifest_signature(&unsigned, &seed);

        assert!(signed.contains("\"signature\": {"));
        assert!(signed.contains(&format!("\"key_id\": \"{}\"", manifest_key_id(&public))));
        // Canonical bytes are exactly the signed manifest minus the
        // trailing signature member.
        let (member, canonical) = split_manifest_signature(&signed).expect("split signed");
        assert_eq!(canonical, unsigned);
        assert_eq!(
            member_field(&member, "algorithm").as_deref(),
            Some(SIGNATURE_ALGORITHM)
        );
        verify_manifest_signature(&signed, &public).expect("signature verifies");
    }

    #[test]
    fn tampered_manifest_rejected() {
        let seed = fixture_seed();
        let public = serviceos_crypto::ed25519::public_key(&seed);
        let unsigned = build_manifest_json(&fixture_entries(), &[], Some(SIGNING_SCHEME_NOTE));
        let signed = append_manifest_signature(&unsigned, &seed);

        // Flip one content byte (a platform name character).
        let tampered = signed.replacen("\"qemu-virtio\"", "\"qemu-virtij\"", 1);
        assert_ne!(tampered, signed);
        let err = verify_manifest_signature(&tampered, &public)
            .expect_err("tampered manifest must not verify");
        assert!(err.contains("ed25519 verification failed"), "{err}");

        // A different key must be rejected before any crypto runs.
        let other_public = serviceos_crypto::ed25519::public_key(&[0x43u8; 32]);
        let err = verify_manifest_signature(&signed, &other_public)
            .expect_err("wrong key must not verify");
        assert!(err.contains("key_id mismatch"), "{err}");
    }

    #[test]
    fn unsigned_manifest_has_no_signature_member() {
        let unsigned = build_manifest_json(&fixture_entries(), &[], None);
        assert!(!unsigned.contains("\"signature\": {"));
        assert!(!unsigned.contains("\"key_id\""));
        assert!(split_manifest_signature(&unsigned).is_none());
        // The notes line is byte-identical to the pre-signing format (the
        // notes text routes through json_escape, which must be an identity
        // for this literal).
        assert!(unsigned.contains(
            "    \"notes\": \"every artifact carries a 64-bit FNV-1a hash (legacy, kept for backcompat with existing manifest readers) plus a sha256 integrity hash computed in-repo by serviceos-crypto; the sha256 hash is an integrity digest, not a signature; platforms marked release+debug-userspace embed debug-built bootstore.bin because release-profile nested userspace links currently fail (rust-lld .text/.got overlap)\"\n",
        ));
        let err = verify_manifest_signature(&unsigned, &[0u8; 32])
            .expect_err("unsigned manifest has nothing to verify");
        assert!(err.contains("no signature member"), "{err}");
    }

    #[test]
    fn canonical_scheme_is_stable() {
        let unsigned = build_manifest_json(&fixture_entries(), &[], Some(SIGNING_SCHEME_NOTE));
        let seed = fixture_seed();
        // Same key and content sign deterministically.
        let signed_a = append_manifest_signature(&unsigned, &seed);
        let signed_b = append_manifest_signature(&unsigned, &seed);
        assert_eq!(signed_a, signed_b);
        // Split inverts append byte-for-byte.
        let (_, canonical) = split_manifest_signature(&signed_a).expect("split");
        assert_eq!(canonical, unsigned);
    }

    #[test]
    fn signing_seed_file_parsing_is_strict_and_error_hygiene_holds() {
        let seed = fixture_seed();
        let mut hex = String::new();
        for byte in seed {
            hex.push_str(&format!("{byte:02x}"));
        }
        let dir = env::temp_dir();

        // Valid 64-hex file (with trailing newline, like `openssl rand -hex`).
        let good = dir.join("serviceos-xtask-seed-good");
        fs::write(&good, format!("{hex}\n")).expect("write key fixture");
        assert_eq!(load_signing_seed_file(&good).expect("valid seed"), seed);
        let _ = fs::remove_file(&good);

        // Wrong length, non-hex, and missing files all error naming the env
        // var and never echo key material.
        let cases: Vec<(&str, String)> = vec![
            ("short", hex[..hex.len() - 2].to_string()),
            ("odd", hex[..hex.len() - 1].to_string()),
            ("nonhex", "z".repeat(64)),
        ];
        for (name, content) in cases {
            let path = dir.join(format!("serviceos-xtask-seed-{name}"));
            fs::write(&path, content).expect("write bad key fixture");
            let err = load_signing_seed_file(&path).expect_err("bad seed must error");
            assert!(err.contains(SIGNING_KEY_ENV), "{err}");
            assert!(
                !err.contains(&hex),
                "error must not echo key material: {err}"
            );
            let _ = fs::remove_file(&path);
        }
        let err = load_signing_seed_file(&dir.join("serviceos-xtask-seed-missing"))
            .expect_err("missing seed must error");
        assert!(err.contains(SIGNING_KEY_ENV), "{err}");
    }

    /// Manual-verification fixture generator (not part of the normal suite):
    /// writes a signed manifest from the REAL writer path to /tmp using a
    /// fixed seed, and prints the matching public key for
    /// `cargo xtask release-verify`. Run with:
    ///   cargo test -p xtask --lib generate_signed_manifest_fixture -- --ignored --nocapture
    #[test]
    #[ignore]
    fn generate_signed_manifest_fixture() {
        let seed = fixture_seed();
        let public = serviceos_crypto::ed25519::public_key(&seed);
        let mut key_hex = String::new();
        for byte in seed {
            key_hex.push_str(&format!("{byte:02x}"));
        }
        let mut pub_hex = String::new();
        for byte in public {
            pub_hex.push_str(&format!("{byte:02x}"));
        }
        let key_path = env::temp_dir().join("serviceos-artsign-seed.hex");
        fs::write(&key_path, format!("{key_hex}\n")).expect("write key fixture");
        let pub_path = env::temp_dir().join("serviceos-artsign-pubkey.hex");
        fs::write(&pub_path, format!("{pub_hex}\n")).expect("write pubkey fixture");
        let manifest_path = env::temp_dir().join("serviceos-artsign-manifest.json");
        write_manifest_with_signing(&manifest_path, &fixture_entries(), &[], Some(&key_path))
            .expect("write signed manifest fixture");
        println!("FIXTURE-KEYFILE {}", key_path.display());
        println!("FIXTURE-MANIFEST {}", manifest_path.display());
        println!("FIXTURE-PUBKEY-FILE {}", pub_path.display());
        println!("FIXTURE-PUBKEY {pub_hex}");
    }

    // --- installer images ---

    #[test]
    fn installer_composition_is_self_contained_with_fresh_data() {
        let root =
            env::temp_dir().join(format!("serviceos-xtask-installer-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let image_root = root.join("target/images/release/qemu-virtio");
        fs::create_dir_all(&image_root).expect("create image root");
        let boot_image = image_root.join("serviceos.img");
        fs::write(&boot_image, b"boot-disk-bytes").expect("write boot fixture");

        let installer_dir = root.join("target/release/installer/qemu-virtio");
        let records = compose_installer(&root, &installer_dir, &boot_image).expect("compose");

        let names: Vec<&str> = records
            .iter()
            .map(|record| record.relative_path.rsplit('/').next().unwrap())
            .collect();
        assert_eq!(
            names,
            vec!["README.txt", "serviceos-data.img", "serviceos.img"]
        );
        let boot_record = &records[2];
        assert_eq!(boot_record.size, b"boot-disk-bytes".len() as u64);
        assert_eq!(boot_record.sha256.len(), 64);
        assert_ne!(boot_record.hash, 0);

        // The data volume ships fresh: right size, zeroed content, even when
        // a previous composition had been mutated by an earlier boot.
        let data_image = installer_dir.join("serviceos-data.img");
        fs::write(&data_image, [0xffu8; 64]).expect("dirty the data volume");
        let _ = compose_installer(&root, &installer_dir, &boot_image).expect("recompose");
        let metadata = data_image.metadata().expect("data image metadata");
        assert_eq!(metadata.len(), INSTALLER_DATA_IMAGE_SIZE_MB * 1024 * 1024);
        // The full dirtied region must read back as zeros, pinning the
        // recreate-from-scratch behavior (not an in-place truncate).
        let mut dirty_region = [0xffu8; 64];
        let mut opened = fs::File::open(&data_image).expect("open data image");
        std::io::Read::read_exact(&mut opened, &mut dirty_region).expect("read dirty region");
        assert!(dirty_region.iter().all(|byte| *byte == 0));

        let readme = fs::read_to_string(installer_dir.join("README.txt")).expect("read README");
        assert!(readme.contains("setup wizard"), "{readme}");
        assert!(readme.contains("serviceos-data.img"), "{readme}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn installer_manifest_records_are_additive_and_signable() {
        let installers = vec![InstallerRecord {
            platform: "qemu-virtio".to_string(),
            directory: "target/release/installer/qemu-virtio".to_string(),
            first_boot: INSTALLER_FIRST_BOOT_NOTE,
            files: vec![ArtifactRecord {
                relative_path: "target/release/installer/qemu-virtio/serviceos.img".to_string(),
                size: 4096,
                hash: 0xdead_beef_cafe_babe,
                sha256: "bb".repeat(32),
            }],
        }];

        let with_installers = build_manifest_json(&fixture_entries(), &installers, None);
        assert!(with_installers.contains("\"installers\": ["));
        assert!(with_installers.contains("\"platform\": \"qemu-virtio\""));
        assert!(
            with_installers.contains("\"directory\": \"target/release/installer/qemu-virtio\"")
        );
        assert!(with_installers.contains(INSTALLER_FIRST_BOOT_NOTE));
        // Installers ride inside the signed canonical bytes, not after them.
        let seed = fixture_seed();
        let public = serviceos_crypto::ed25519::public_key(&seed);
        let signed = append_manifest_signature(&with_installers, &seed);
        let (_, canonical) = split_manifest_signature(&signed).expect("split signed");
        assert_eq!(canonical, with_installers);
        verify_manifest_signature(&signed, &public)
            .expect("signed manifest with installers verifies");

        // No installers -> byte-identical to the pre-installer format.
        let without = build_manifest_json(&fixture_entries(), &[], None);
        assert!(!without.contains("installers"));
    }
}

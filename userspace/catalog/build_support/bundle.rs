use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use serviceos_bundle::parse_package_manifest;

fn update_fnv64(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes.iter().copied() {
        *hash ^= byte as u64;
        *hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
}

fn compute_package_integrity(
    bundles_root: &Path,
    manifest: &serviceos_bundle::PackageManifest,
) -> Result<u64, Box<dyn Error>> {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for content in manifest.contents[..manifest.content_count].iter() {
        let path = content
            .as_str()
            .map_err(|_| "package content path is not valid utf-8")?;
        update_fnv64(&mut hash, path.as_bytes());
        let bytes = fs::read(bundles_root.join(path))?;
        update_fnv64(&mut hash, &bytes);
    }
    Ok(hash)
}

pub(crate) fn validate_package_manifests(bundles_root: &Path) -> Result<(), Box<dyn Error>> {
    for path in collect_bundle_files(bundles_root)? {
        if path.extension().and_then(|value| value.to_str()) != Some("pkg") {
            continue;
        }
        let bytes = fs::read(&path)?;
        let manifest = parse_package_manifest(&bytes)
            .map_err(|error| format!("invalid package manifest {}: {:?}", path.display(), error))?;
        let actual = compute_package_integrity(bundles_root, &manifest)?;
        if manifest.integrity != actual {
            return Err(format!(
                "package integrity mismatch for {}: manifest=0x{:016x} actual=0x{:016x}",
                path.display(),
                manifest.integrity,
                actual
            )
            .into());
        }
    }
    Ok(())
}

pub(crate) fn collect_bundle_files(root: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut files = Vec::new();
    visit_bundle_dir(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn visit_bundle_dir(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();
        if entry.file_type()?.is_dir() {
            visit_bundle_dir(&entry_path, files)?;
        } else {
            files.push(entry_path);
        }
    }
    Ok(())
}

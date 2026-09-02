//! Backup blob format, scope model, and wire tags for the backup service.
//! Pure logic shared between the `no_std` service binary and host unit
//! tests, mirroring the account-service layout.
//!
//! Blob layout (little-endian):
//! ```text
//! [0..4]   magic "SBV1"
//! [4..8]   format version (1)
//! [8..12]  scope mask captured at export time
//! [12..16] record count
//! [16..24] FNV-1a 64 checksum over the payload bytes ([24..len])
//! [24..]   records: scope u32 | name_len u32 | data_len u32 |
//!                   name bytes | data bytes
//! ```
//!
//! FNV-1a is NOT a cryptographic hash; like account-service's KDF this is an
//! honest placeholder integrity check for a pragmatic operator-level
//! foundation until real digest/signing machinery lands.
//!
//! Snapshot signing (roadmap row "cryptographic signing of blobs"): every
//! export is signed with the service's Ed25519 identity and the detached
//! signature is stored beside the blob as `backups/<name>.sig`. Restore
//! verifies the signature over the exact blob bytes (direct bytes, no hash
//! indirection - blobs are capped at MAX_BLOB_BYTES so there is nothing to
//! save) before planning or applying anything. Policy: unsigned or invalid
//! signatures REFUSE restore (status 9 / 10); there is deliberately no
//! allow-unsigned escape hatch - re-export to recover a trusted snapshot.
//!
//! Signing identity: loaded from / persisted to
//! `state/backup/signing.cfg` in the service's own namespace (outside every
//! backup scope, so a restore can never rewrite the verifier's key).
//! First-boot generation prefers the kernel entropy contract (RngRequest —
//! hardware-seeded DRBG); when that is unavailable it mirrors
//! package-service's guest entropy substitute and carries the same honest
//! limits: the tick may stand still, so the fallback seed is UNIQUE-ISH,
//! not cryptographically random. v0 keeps one fixed identity per store; key
//! rotation is out of scope (regenerating the config invalidates every
//! existing signature by design).

#![cfg_attr(not(test), no_std)]

pub const MAX_BLOB_BYTES: usize = 16384;
pub const MAX_RECORDS: usize = 32;
pub const MAX_RECORD_NAME: usize = 64;
pub const MAX_BACKUP_NAME: usize = 32;

pub const BACKUPS_DIR: &str = "backups/";
pub const ACCOUNTS_PATH: &str = "state/account/accounts.cfg";
pub const PACKAGES_DIR: &str = "state/packages/";
pub const CONFIG_DIR: &str = "state/config/";

pub const MAGIC: u32 = 0x3156_4253; // "SBV1" little-endian
pub const FORMAT_VERSION: u32 = 1;

/// Scope bits: which parts of system state a backup covers.
pub mod scope {
    pub const CONFIG: u32 = 1 << 0;
    pub const ACCOUNTS: u32 = 1 << 1;
    pub const PACKAGES: u32 = 1 << 2;
    pub const KNOWN_MASK: u32 = CONFIG | ACCOUNTS | PACKAGES;
}

/// Wire tag base chosen away from existing service ranges. INFO (0x238+) is
/// the additive signing-introspection tail of the contract: [status, key id,
/// packed public key].
pub mod backup_tag {
    pub const EXPORT_REQUEST: u32 = 0x230;
    pub const EXPORT_REPLY: u32 = 0x231;
    pub const RESTORE_REQUEST: u32 = 0x232;
    pub const RESTORE_REPLY: u32 = 0x233;
    pub const LIST_REQUEST: u32 = 0x234;
    pub const LIST_REPLY: u32 = 0x235;
    pub const DELETE_REQUEST: u32 = 0x236;
    pub const DELETE_REPLY: u32 = 0x237;
    pub const INFO_REQUEST: u32 = 0x238;
    pub const INFO_REPLY: u32 = 0x239;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackupError {
    InvalidArgument,
    UnknownScope,
    CapacityExceeded,
    NotFound,
    BadMagic,
    UnsupportedVersion,
    Corrupt,
    StorageFailure,
    /// Snapshot has no detached signature; restore refuses by policy.
    Unsigned,
    /// Detached signature is malformed, names another key, or fails
    /// verification over the blob bytes.
    BadSignature,
}

impl BackupError {
    /// Wire status code: 0 = Ok, errors count up from 1. New codes append
    /// at the end (additive-by-default).
    pub fn to_code(self) -> u32 {
        match self {
            BackupError::InvalidArgument => 1,
            BackupError::UnknownScope => 2,
            BackupError::CapacityExceeded => 3,
            BackupError::NotFound => 4,
            BackupError::BadMagic => 5,
            BackupError::UnsupportedVersion => 6,
            BackupError::Corrupt => 7,
            BackupError::StorageFailure => 8,
            BackupError::Unsigned => 9,
            BackupError::BadSignature => 10,
        }
    }
}

pub const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
pub const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn le_u64(bytes: &[u8]) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[..8]);
    u64::from_le_bytes(raw)
}

/// Incremental blob builder. Records are appended into a fixed buffer; the
/// header (magic/version/scope mask/count/checksum) is patched by `finish`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlobWriter {
    buf: [u8; MAX_BLOB_BYTES],
    len: usize,
    record_count: u32,
    scope_mask: u32,
}

impl BlobWriter {
    pub const fn new() -> Self {
        Self {
            buf: [0; MAX_BLOB_BYTES],
            len: 0,
            record_count: 0,
            scope_mask: 0,
        }
    }

    pub fn scope_mask(&self) -> u32 {
        self.scope_mask
    }

    pub fn record_count(&self) -> u32 {
        self.record_count
    }

    pub fn push_record(&mut self, scope: u32, name: &str, data: &[u8]) -> Result<(), BackupError> {
        if scope & !scope::KNOWN_MASK != 0 || scope.count_ones() != 1 || name.is_empty() {
            return Err(BackupError::InvalidArgument);
        }
        if name.len() > MAX_RECORD_NAME || data.len() > MAX_BLOB_BYTES {
            return Err(BackupError::InvalidArgument);
        }
        let needed = 12 + name.len() + data.len();
        if self.len + needed > MAX_BLOB_BYTES || self.record_count as usize >= MAX_RECORDS {
            return Err(BackupError::CapacityExceeded);
        }
        let mut cursor = self.len;
        self.buf[cursor..cursor + 4].copy_from_slice(&scope.to_le_bytes());
        cursor += 4;
        self.buf[cursor..cursor + 4].copy_from_slice(&(name.len() as u32).to_le_bytes());
        cursor += 4;
        self.buf[cursor..cursor + 4].copy_from_slice(&(data.len() as u32).to_le_bytes());
        cursor += 4;
        self.buf[cursor..cursor + name.len()].copy_from_slice(name.as_bytes());
        cursor += name.len();
        self.buf[cursor..cursor + data.len()].copy_from_slice(data);
        self.len = cursor + data.len();
        self.record_count += 1;
        self.scope_mask |= scope;
        Ok(())
    }

    /// Patch the header over the accumulated payload in place and return
    /// the full serialized blob length.
    pub fn finish(&mut self) -> usize {
        let payload = self.len;
        self.buf.copy_within(0..payload, HEADER_LEN);
        let checksum = fnv1a64(&self.buf[HEADER_LEN..HEADER_LEN + payload]);
        self.buf[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        self.buf[4..8].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        self.buf[8..12].copy_from_slice(&self.scope_mask.to_le_bytes());
        self.buf[12..16].copy_from_slice(&self.record_count.to_le_bytes());
        self.buf[16..24].copy_from_slice(&checksum.to_le_bytes());
        self.len = HEADER_LEN + payload;
        self.len
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub fn into_parts(self) -> ([u8; MAX_BLOB_BYTES], usize) {
        (self.buf, self.len)
    }
}

impl Default for BlobWriter {
    fn default() -> Self {
        Self::new()
    }
}

pub const HEADER_LEN: usize = 24;

/// One decoded record borrowing from the blob buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecordRef<'a> {
    pub scope: u32,
    pub name: &'a str,
    pub data: &'a [u8],
}

/// Validated read-only view over a serialized backup blob.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlobView<'a> {
    data: &'a [u8],
    scope_mask: u32,
    record_count: u32,
}

impl<'a> BlobView<'a> {
    /// Validate magic, version, checksum, and record bounds. Returns the view
    /// or the first validation failure.
    pub fn open(data: &'a [u8]) -> Result<Self, BackupError> {
        if data.len() < HEADER_LEN {
            return Err(BackupError::Corrupt);
        }
        if le_u32(&data[0..4]) != MAGIC {
            return Err(BackupError::BadMagic);
        }
        if le_u32(&data[4..8]) != FORMAT_VERSION {
            return Err(BackupError::UnsupportedVersion);
        }
        let scope_mask = le_u32(&data[8..12]);
        let record_count = le_u32(&data[12..16]);
        let checksum = le_u64(&data[16..24]);
        let payload = &data[HEADER_LEN..];
        if fnv1a64(payload) != checksum {
            return Err(BackupError::Corrupt);
        }
        // Walk records once so bounds errors surface at validation time and
        // iteration later can be trusted.
        let mut cursor = 0usize;
        for _ in 0..record_count {
            if cursor + 12 > payload.len() {
                return Err(BackupError::Corrupt);
            }
            let name_len = le_u32(&payload[cursor + 4..cursor + 8]) as usize;
            let data_len = le_u32(&payload[cursor + 8..cursor + 12]) as usize;
            cursor += 12;
            if cursor + name_len + data_len > payload.len() {
                return Err(BackupError::Corrupt);
            }
            cursor += name_len + data_len;
        }
        Ok(Self {
            data,
            scope_mask,
            record_count,
        })
    }

    pub fn scope_mask(&self) -> u32 {
        self.scope_mask
    }

    pub fn record_count(&self) -> u32 {
        self.record_count
    }

    /// Decode the record starting at `*cursor`, advancing it past the record.
    /// Returns `Ok(None)` once records are exhausted.
    pub fn next_record(&self, cursor: &mut usize) -> Result<Option<RecordRef<'a>>, BackupError> {
        let consumed = *cursor;
        let payload = &self.data[HEADER_LEN..];
        if consumed >= payload.len() {
            return Ok(None);
        }
        if consumed + 12 > payload.len() {
            return Err(BackupError::Corrupt);
        }
        let record_scope = le_u32(&payload[consumed..consumed + 4]);
        let name_len = le_u32(&payload[consumed + 4..consumed + 8]) as usize;
        let data_len = le_u32(&payload[consumed + 8..consumed + 12]) as usize;
        let name_start = consumed + 12;
        let data_start = name_start + name_len;
        if data_start + data_len > payload.len() {
            return Err(BackupError::Corrupt);
        }
        let name_bytes = &payload[name_start..name_start + name_len];
        let name = core::str::from_utf8(name_bytes).map_err(|_| BackupError::Corrupt)?;
        *cursor = data_start + data_len;
        Ok(Some(RecordRef {
            scope: record_scope,
            name,
            data: &payload[data_start..data_start + data_len],
        }))
    }
}

/// Dry-run restore report: what a restore of `filter_mask` would write.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub struct RestoreReport {
    pub selected_scope_mask: u32,
    pub selected_records: u32,
    pub total_bytes: u64,
}

/// Compute the dry-run report for restoring `filter_mask` from `view`.
/// Rejects filter masks with unknown scope bits.
pub fn plan_restore(view: &BlobView<'_>, filter_mask: u32) -> Result<RestoreReport, BackupError> {
    if filter_mask & !scope::KNOWN_MASK != 0 {
        return Err(BackupError::UnknownScope);
    }
    let mut report = RestoreReport {
        selected_scope_mask: 0,
        selected_records: 0,
        total_bytes: 0,
    };
    let mut cursor = 0usize;
    while let Some(record) = view.next_record(&mut cursor)? {
        if record.scope & filter_mask == 0 {
            continue;
        }
        report.selected_scope_mask |= record.scope;
        report.selected_records += 1;
        report.total_bytes += record.data.len() as u64;
    }
    Ok(report)
}

/// Map a record's logical name onto its persistent storage path. Returns the
/// path length written into `out`. Names are validated to stay inside their
/// scope directory (no separators beyond the trailing file name).
pub fn record_storage_path(record: RecordRef<'_>, out: &mut [u8]) -> Result<usize, BackupError> {
    if record.name.is_empty() || record.name.len() > MAX_RECORD_NAME || record.name.starts_with('/')
    {
        return Err(BackupError::InvalidArgument);
    }
    match record.scope {
        x if x == scope::CONFIG => copy_path(CONFIG_DIR, record.name, out),
        x if x == scope::ACCOUNTS => copy_path("state/account/", record.name, out),
        x if x == scope::PACKAGES => copy_path(PACKAGES_DIR, record.name, out),
        _ => Err(BackupError::UnknownScope),
    }
}

fn copy_path(directory: &str, name: &str, out: &mut [u8]) -> Result<usize, BackupError> {
    if directory.len() + name.len() > out.len() {
        return Err(BackupError::CapacityExceeded);
    }
    out[..directory.len()].copy_from_slice(directory.as_bytes());
    out[directory.len()..directory.len() + name.len()].copy_from_slice(name.as_bytes());
    Ok(directory.len() + name.len())
}

/// Format `backup-<tick>` into `out`; returns the name length.
pub fn format_backup_name(tick: u64, out: &mut [u8]) -> usize {
    let prefix = b"backup-";
    out[..prefix.len()].copy_from_slice(prefix);
    let mut digits = [0u8; 20];
    let mut count = 0usize;
    let mut value = tick;
    loop {
        digits[count] = b'0' + (value % 10) as u8;
        count += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    for index in 0..count {
        out[prefix.len() + index] = digits[count - 1 - index];
    }
    prefix.len() + count
}

/// Parse names previously produced by `format_backup_name`.
pub fn parse_backup_name(name: &str) -> Option<u64> {
    let index = name.strip_prefix("backup-")?;
    if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    index.parse::<u64>().ok()
}

// ---------------------------------------------------------------- signing

/// Storage path of the service-local signing identity (service's own
/// namespace; no backup scope ever covers it).
pub const SIGNING_CONFIG_PATH: &str = "state/backup/signing.cfg";
/// Detached signatures live beside the blob, name-confined under backups/.
pub const SIGNATURE_SUFFIX: &str = ".sig";
/// Upper bound for both signing.cfg and `<name>.sig` sidecar files.
pub const MAX_SIGNING_TEXT_BYTES: usize = 256;
/// List scan cap when skipping sidecar entries (callers page at most 16/32
/// rows; 64 storage probes bound the walk either way).
pub const LIST_SCAN_CAP: usize = 64;

/// Is this a detached-signature sidecar name (`<snapshot>.sig`)? Such names
/// never parse as snapshot names, but list enumeration must skip them.
pub fn is_signature_name(name: &str) -> bool {
    name.ends_with(SIGNATURE_SUFFIX)
}

/// Sidecar path for a snapshot name: `backups/<name>.sig`. Returns the
/// length written into `out`.
pub fn signature_path(name: &str, out: &mut [u8]) -> Result<usize, BackupError> {
    if name.is_empty() || name.len() > MAX_BACKUP_NAME || parse_backup_name(name).is_none() {
        return Err(BackupError::InvalidArgument);
    }
    let total = BACKUPS_DIR.len() + name.len() + SIGNATURE_SUFFIX.len();
    if total > out.len() {
        return Err(BackupError::CapacityExceeded);
    }
    out[..BACKUPS_DIR.len()].copy_from_slice(BACKUPS_DIR.as_bytes());
    out[BACKUPS_DIR.len()..BACKUPS_DIR.len() + name.len()].copy_from_slice(name.as_bytes());
    out[BACKUPS_DIR.len() + name.len()..total].copy_from_slice(SIGNATURE_SUFFIX.as_bytes());
    Ok(total)
}

/// The signing identity: Ed25519 seed (secret), compressed public key, and
/// the derived key id (FNV-1a64 over the public key - a stable handle for
/// wire tails, not a security property).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SigningIdentity {
    pub seed: [u8; 32],
    pub public: [u8; 32],
    pub key_id: u64,
}

pub fn key_id_for(public: &[u8; 32]) -> u64 {
    fnv1a64(public)
}

/// Derive one Ed25519 identity from guest-local entropy substitutes: SHA-512
/// over (source, monotonic tick, counter, store fingerprint). HONEST LIMITS:
/// the tick may stand still - the seed is UNIQUE-ISH, not cryptographically
/// random. Fine for a boot-local signing identity on a test OS; production
/// keys would be enrolled from the host. This is the FALLBACK behind
/// `fresh_signing_identity`, kept for tests and contract-less platforms.
pub fn derive_signing_identity(
    source: &[u8],
    tick: u64,
    counter: u64,
    store_fingerprint: u64,
) -> SigningIdentity {
    let mut block = [0u8; 64];
    let source_len = source.len().min(24);
    block[..source_len].copy_from_slice(&source[..source_len]);
    block[24..32].copy_from_slice(&tick.to_le_bytes());
    block[32..40].copy_from_slice(&counter.to_le_bytes());
    block[40..48].copy_from_slice(&store_fingerprint.to_le_bytes());
    let digest = serviceos_crypto::sha512::digest(&[&block]);
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&digest[..32]);
    identity_from_seed(seed)
}

pub fn identity_from_seed(seed: [u8; 32]) -> SigningIdentity {
    let public = serviceos_crypto::ed25519::public_key(&seed);
    SigningIdentity {
        seed,
        public,
        key_id: key_id_for(&public),
    }
}

/// Signing identity for first-boot generation, entropy-honest: draw the
/// Ed25519 seed from the kernel entropy contract (RngRequest, syscall 55)
/// when it is reachable, and only fall back to `derive_signing_identity`'s
/// boot-local substitute otherwise. Returns `(identity, kernel_entropy)` so
/// the caller can log which class of seed it got. Host tests compile the
/// contract attempt out and stay deterministic.
pub fn fresh_signing_identity(
    source: &[u8],
    tick: u64,
    counter: u64,
    store_fingerprint: u64,
) -> (SigningIdentity, bool) {
    #[cfg(not(test))]
    {
        let mut seed = [0u8; 32];
        if serviceos_userspace_runtime::entropy(&mut seed) == Ok(32) {
            return (identity_from_seed(seed), true);
        }
    }
    (
        derive_signing_identity(source, tick, counter, store_fingerprint),
        false,
    )
}

// ---- hex helpers (fixed-capacity, lowercase) ----

/// Emit lowercase hex pairs; returns bytes written.
pub fn emit_hex(bytes: &[u8], out: &mut [u8]) -> usize {
    let pairs = bytes.len().min(out.len() / 2);
    for index in 0..pairs {
        let byte = bytes[index];
        out[index * 2] = hex_nibble(byte >> 4);
        out[index * 2 + 1] = hex_nibble(byte & 0xf);
    }
    pairs * 2
}

fn hex_nibble(nibble: u8) -> u8 {
    if nibble < 10 {
        nibble + b'0'
    } else {
        nibble + b'a' - 10
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Parse exactly `out.len()` hex digits into `out`.
fn parse_hex_into(text: &[u8], out: &mut [u8]) -> Option<()> {
    if text.len() != out.len() * 2 {
        return None;
    }
    for index in 0..out.len() {
        let high = hex_value(text[index * 2])?;
        let low = hex_value(text[index * 2 + 1])?;
        out[index] = (high << 4) | low;
    }
    Some(())
}

/// Parse 16 hex digits as one u64 (key ids).
pub fn parse_hex_u64(text: &[u8]) -> Option<u64> {
    if text.len() != 16 {
        return None;
    }
    let mut word = 0u64;
    for byte in text {
        word = (word << 4) | u64::from(hex_value(*byte)?);
    }
    Some(word)
}

// ---- config / sidecar file formats (plain key=value lines) ----

/// Serialize the identity as `state/backup/signing.cfg`. The seed is stored
/// in the service's own namespace so exports keep signing across restarts;
/// the file lives outside every backup scope, so restores cannot rewrite it.
pub fn format_signing_config(
    identity: &SigningIdentity,
    out: &mut [u8],
) -> Result<usize, BackupError> {
    let mut hex = [0u8; 64];
    let mut id_hex = [0u8; 16];
    let _ = emit_hex(&identity.key_id.to_be_bytes(), &mut id_hex);
    let mut cursor = 0usize;
    let line = |text: &[u8], out: &mut [u8], cursor: &mut usize| -> Result<(), BackupError> {
        if *cursor + text.len() > out.len() {
            return Err(BackupError::CapacityExceeded);
        }
        out[*cursor..*cursor + text.len()].copy_from_slice(text);
        *cursor += text.len();
        Ok(())
    };
    line(b"alg=ed25519\n", out, &mut cursor)?;
    line(b"key-id=", out, &mut cursor)?;
    line(&id_hex, out, &mut cursor)?;
    line(b"\npublic=", out, &mut cursor)?;
    let public_len = emit_hex(&identity.public, &mut hex);
    line(&hex[..public_len], out, &mut cursor)?;
    line(b"\nseed=", out, &mut cursor)?;
    let seed_len = emit_hex(&identity.seed, &mut hex);
    line(&hex[..seed_len], out, &mut cursor)?;
    line(b"\n", out, &mut cursor)?;
    Ok(cursor)
}

/// One `key=value` line view within a text buffer.
struct Line<'a> {
    key: &'a [u8],
    value: &'a [u8],
}

fn lines_of(text: &str) -> impl Iterator<Item = Line<'_>> {
    text.lines().filter_map(|line| {
        let split = line.find('=')?;
        let bytes = line.as_bytes();
        Some(Line {
            key: &bytes[..split],
            value: &bytes[split + 1..],
        })
    })
}

/// Parse `state/backup/signing.cfg` back into an identity. Self-validating:
/// the stored public key must match the stored seed, and the key id must
/// match the public key; anything else is treated as corruption (None), so
/// the service regenerates rather than signs under a broken identity.
pub fn parse_signing_config(text: &str) -> Option<SigningIdentity> {
    let mut alg_ok = false;
    let mut seed = [0u8; 32];
    let mut seed_seen = false;
    let mut public = [0u8; 32];
    let mut public_seen = false;
    let mut key_id = 0u64;
    for line in lines_of(text) {
        match line.key {
            b"alg" => alg_ok = line.value == b"ed25519",
            b"key-id" => key_id = parse_hex_u64(line.value)?,
            b"public" => {
                parse_hex_into(line.value, &mut public)?;
                public_seen = true;
            }
            b"seed" => {
                parse_hex_into(line.value, &mut seed)?;
                seed_seen = true;
            }
            _ => {}
        }
    }
    if !alg_ok || !seed_seen || !public_seen {
        return None;
    }
    if serviceos_crypto::ed25519::public_key(&seed) != public || key_id != key_id_for(&public) {
        return None;
    }
    Some(SigningIdentity {
        seed,
        public,
        key_id,
    })
}

/// Serialize a detached signature sidecar (`<name>.sig`).
pub fn format_signature_file(
    key_id: u64,
    signature: &[u8; 64],
    out: &mut [u8],
) -> Result<usize, BackupError> {
    let mut sig_hex = [0u8; 128];
    let sig_len = emit_hex(signature, &mut sig_hex);
    let mut id_hex = [0u8; 16];
    let _ = emit_hex(&key_id.to_be_bytes(), &mut id_hex);
    let mut cursor = 0usize;
    let line = |text: &[u8], out: &mut [u8], cursor: &mut usize| -> Result<(), BackupError> {
        if *cursor + text.len() > out.len() {
            return Err(BackupError::CapacityExceeded);
        }
        out[*cursor..*cursor + text.len()].copy_from_slice(text);
        *cursor += text.len();
        Ok(())
    };
    line(b"alg=ed25519\n", out, &mut cursor)?;
    line(b"key-id=", out, &mut cursor)?;
    line(&id_hex, out, &mut cursor)?;
    line(b"\nsig=", out, &mut cursor)?;
    line(&sig_hex[..sig_len], out, &mut cursor)?;
    line(b"\n", out, &mut cursor)?;
    Ok(cursor)
}

/// A parsed signature sidecar.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignatureRecord {
    pub key_id: u64,
    pub sig: [u8; 64],
}

/// Parse a `<name>.sig` sidecar. Anything malformed is None (caller maps
/// that to BadSignature - an unreadable signature must not restore).
pub fn parse_signature_file(text: &str) -> Option<SignatureRecord> {
    let mut alg_ok = false;
    let mut key_id = 0u64;
    let mut key_id_seen = false;
    let mut sig = [0u8; 64];
    let mut sig_seen = false;
    for line in lines_of(text) {
        match line.key {
            b"alg" => alg_ok = line.value == b"ed25519",
            b"key-id" => {
                key_id = parse_hex_u64(line.value)?;
                key_id_seen = true;
            }
            b"sig" => {
                parse_hex_into(line.value, &mut sig)?;
                sig_seen = true;
            }
            _ => {}
        }
    }
    if !alg_ok || !key_id_seen || !sig_seen {
        return None;
    }
    Some(SignatureRecord { key_id, sig })
}

/// Verify a snapshot signature over the exact blob bytes against `public`.
pub fn verify_blob_signature(
    public: &[u8; 32],
    blob: &[u8],
    record: &SignatureRecord,
    expected_key_id: u64,
) -> Result<(), BackupError> {
    if record.key_id != expected_key_id {
        return Err(BackupError::BadSignature);
    }
    if !serviceos_crypto::ed25519::verify(public, blob, &record.sig) {
        return Err(BackupError::BadSignature);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CONFIG: &[u8] = b"key=value\nother=1\n";

    const SAMPLE_ACCOUNTS: &[u8] = b"account=1,admin,Admin,0001,0002,0000_00ff\n";
    const SAMPLE_PACKAGES: &[u8] = b"pkg=demo,1.0\n";

    fn sample_blob() -> ([u8; MAX_BLOB_BYTES], usize) {
        let mut writer = BlobWriter::new();
        writer
            .push_record(scope::CONFIG, "desktop/settings.cfg", SAMPLE_CONFIG)
            .unwrap();
        writer
            .push_record(scope::ACCOUNTS, "accounts.cfg", SAMPLE_ACCOUNTS)
            .unwrap();
        writer
            .push_record(scope::PACKAGES, "installed.cfg", SAMPLE_PACKAGES)
            .unwrap();
        let length = writer.finish();
        assert_eq!(
            writer.scope_mask(),
            scope::CONFIG | scope::ACCOUNTS | scope::PACKAGES
        );
        assert_eq!(writer.record_count(), 3);
        (writer.into_parts().0, length)
    }

    #[test]
    fn blob_header_roundtrip() {
        let (buffer, length) = sample_blob();
        assert_eq!(le_u32(&buffer[0..4]), MAGIC);
        assert_eq!(le_u32(&buffer[4..8]), FORMAT_VERSION);
        assert_eq!(
            le_u32(&buffer[8..12]),
            scope::CONFIG | scope::ACCOUNTS | scope::PACKAGES
        );
        assert_eq!(le_u32(&buffer[12..16]), 3);
        assert_eq!(
            le_u64(&buffer[16..24]),
            fnv1a64(&buffer[HEADER_LEN..length])
        );

        let view = BlobView::open(&buffer[..length]).expect("valid blob");
        assert_eq!(view.record_count(), 3);

        let mut cursor = 0usize;
        let first = view.next_record(&mut cursor).unwrap().unwrap();
        assert_eq!(first.scope, scope::CONFIG);
        assert_eq!(first.name, "desktop/settings.cfg");
        assert_eq!(first.data, SAMPLE_CONFIG);
        let second = view.next_record(&mut cursor).unwrap().unwrap();
        assert_eq!(second.scope, scope::ACCOUNTS);
        assert_eq!(second.data, SAMPLE_ACCOUNTS);
        let third = view.next_record(&mut cursor).unwrap().unwrap();
        assert_eq!(third.scope, scope::PACKAGES);
        assert_eq!(third.data, SAMPLE_PACKAGES);
        assert!(view.next_record(&mut cursor).unwrap().is_none());

        // Empty blob is still a valid header with zero records.
        let mut empty = BlobWriter::new();
        let length = empty.finish();
        let (empty_buffer, _) = empty.into_parts();
        let view = BlobView::open(&empty_buffer[..length]).unwrap();
        assert_eq!(view.record_count(), 0);
        assert!(view.next_record(&mut 0usize).unwrap().is_none());
    }

    #[test]
    fn checksum_and_header_validation_rejects_damage() {
        let (mut buffer, length) = sample_blob();

        // Flip one payload byte: checksum must catch it.
        buffer[length - 1] ^= 0x01;
        assert_eq!(BlobView::open(&buffer[..length]), Err(BackupError::Corrupt));

        // Wrong magic.
        buffer[0] = b'X';
        assert_eq!(
            BlobView::open(&buffer[..length]),
            Err(BackupError::BadMagic)
        );

        // Restore magic, break the version instead.
        buffer[0] = MAGIC as u8;
        buffer[5] = 9;
        assert_eq!(
            BlobView::open(&buffer[..length]),
            Err(BackupError::UnsupportedVersion)
        );

        // Truncated header.
        assert_eq!(BlobView::open(&buffer[..8]), Err(BackupError::Corrupt));
    }

    #[test]
    fn scope_filter_selects_only_matching_records() {
        let (buffer, length) = sample_blob();
        let view = BlobView::open(&buffer[..length]).unwrap();

        assert_eq!(
            plan_restore(&view, scope::CONFIG | scope::PACKAGES),
            Ok(RestoreReport {
                selected_scope_mask: scope::CONFIG | scope::PACKAGES,
                selected_records: 2,
                total_bytes: (SAMPLE_CONFIG.len() + SAMPLE_PACKAGES.len()) as u64,
            })
        );
        assert_eq!(
            plan_restore(&view, scope::ACCOUNTS),
            Ok(RestoreReport {
                selected_scope_mask: scope::ACCOUNTS,
                selected_records: 1,
                total_bytes: SAMPLE_ACCOUNTS.len() as u64,
            })
        );
        // Filter matching nothing in the blob is valid but selects zero records.
        assert!(plan_restore(&view, 0).is_ok());
        // Unknown scope bits are rejected outright.
        assert_eq!(plan_restore(&view, 0x40), Err(BackupError::UnknownScope));

        // Record-to-path mapping stays inside its scope directory.
        let mut path = [0u8; 96];
        let used = record_storage_path(
            RecordRef {
                scope: scope::CONFIG,
                name: "desktop/settings.cfg",
                data: &[],
            },
            &mut path,
        )
        .unwrap();
        assert_eq!(&path[..used], b"state/config/desktop/settings.cfg");
        let used = record_storage_path(
            RecordRef {
                scope: scope::ACCOUNTS,
                name: "accounts.cfg",
                data: &[],
            },
            &mut path,
        )
        .unwrap();
        assert_eq!(&path[..used], ACCOUNTS_PATH.as_bytes());
        assert_eq!(
            record_storage_path(
                RecordRef {
                    scope: 0x80,
                    name: "escape",
                    data: &[],
                },
                &mut path
            ),
            Err(BackupError::UnknownScope)
        );
    }

    #[test]
    fn backup_names_format_and_parse_roundtrip() {
        let mut name = [0u8; MAX_BACKUP_NAME];
        let used = format_backup_name(1724600000123, &mut name);
        let text = core::str::from_utf8(&name[..used]).unwrap();
        assert_eq!(text, "backup-1724600000123");
        assert_eq!(parse_backup_name(text), Some(1724600000123));
        assert_eq!(parse_backup_name("backups/x"), None);
        assert_eq!(parse_backup_name("backup-"), None);
        assert_eq!(parse_backup_name("backup-12ab"), None);
        assert_eq!(parse_backup_name("snapshot-7"), None);
    }

    #[test]
    fn writer_rejects_bad_scopes_and_overflow() {
        let mut writer = BlobWriter::new();
        assert_eq!(
            writer.push_record(0, "x", b"1"),
            Err(BackupError::InvalidArgument)
        );
        assert_eq!(
            writer.push_record(0xFFFF, "x", b"1"),
            Err(BackupError::InvalidArgument)
        );
        assert_eq!(
            writer.push_record(scope::CONFIG, "", b"1"),
            Err(BackupError::InvalidArgument)
        );
        let big = [0u8; MAX_BLOB_BYTES];
        assert_eq!(
            writer.push_record(scope::CONFIG, "huge", &big),
            Err(BackupError::CapacityExceeded)
        );
        for index in 0..MAX_RECORDS {
            writer.push_record(scope::CONFIG, "f", b"x").expect("fits");
            let _ = index;
        }
        assert_eq!(
            writer.push_record(scope::CONFIG, "extra", b"x"),
            Err(BackupError::CapacityExceeded)
        );
    }
    fn test_identity(byte: u8) -> SigningIdentity {
        let mut seed = [0u8; 32];
        seed[..8].copy_from_slice(&[byte; 8]);
        identity_from_seed(seed)
    }

    #[test]
    fn error_codes_append_additively() {
        assert_eq!(BackupError::StorageFailure.to_code(), 8);
        assert_eq!(BackupError::Unsigned.to_code(), 9);
        assert_eq!(BackupError::BadSignature.to_code(), 10);
    }

    #[test]
    fn signature_names_are_sidecars_never_snapshots() {
        assert!(is_signature_name("backup-123.sig"));
        assert!(!is_signature_name("backup-123"));
        // Sidecars never parse as snapshot names: delete/restore stay
        // name-confined to real snapshots.
        assert_eq!(parse_backup_name("backup-123.sig"), None);
        let mut path = [0u8; 64];
        let used = signature_path("backup-42", &mut path).unwrap();
        assert_eq!(&path[..used], b"backups/backup-42.sig");
        assert_eq!(
            signature_path("", &mut path),
            Err(BackupError::InvalidArgument)
        );
        assert_eq!(
            signature_path("backups/backup-1", &mut path),
            Err(BackupError::InvalidArgument)
        );
    }

    #[test]
    fn signing_config_roundtrip_and_self_validation() {
        let identity = test_identity(9);
        let mut buffer = [0u8; MAX_SIGNING_TEXT_BYTES];
        let used = format_signing_config(&identity, &mut buffer).unwrap();
        let text = core::str::from_utf8(&buffer[..used]).unwrap();
        assert_eq!(parse_signing_config(text), Some(identity));

        // Corrupted public key must not parse (seed/public mismatch).
        let mut damaged = text.to_string();
        let position = damaged.find("public=").unwrap() + 7;
        damaged.replace_range(position..position + 2, "ff");
        assert_eq!(parse_signing_config(&damaged), None);

        // Key id inconsistent with the public key must not parse either.
        let mut swapped = text.to_string();
        let position = swapped.find("key-id=").unwrap() + 7;
        swapped.replace_range(position..position + 16, "0123456789abcdef");
        assert_eq!(parse_signing_config(&swapped), None);

        // Truncated buffer is a capacity error, not a panic.
        let mut tiny = [0u8; 16];
        assert_eq!(
            format_signing_config(&identity, &mut tiny),
            Err(BackupError::CapacityExceeded)
        );
    }

    #[test]
    fn derive_signing_identity_is_deterministic_per_input() {
        let a = derive_signing_identity(b"backup-service-signing", 1234, 0, 77);
        let b = derive_signing_identity(b"backup-service-signing", 1234, 0, 77);
        assert_eq!(a, b);
        let c = derive_signing_identity(b"backup-service-signing", 1235, 0, 77);
        assert_ne!(a.seed, c.seed);
        assert_ne!(a.public, c.public);
        assert_ne!(a.key_id, c.key_id);
        // Key id is the FNV-1a64 of the public key, stable across readers.
        assert_eq!(a.key_id, key_id_for(&a.public));
    }

    #[test]
    fn signature_file_roundtrip_and_verification() {
        let identity = test_identity(3);
        let mut writer = BlobWriter::new();
        writer
            .push_record(scope::CONFIG, "x/y.cfg", b"v=1")
            .unwrap();
        let blob_len = writer.finish();
        let (blob, _) = writer.into_parts();

        let signature = serviceos_crypto::ed25519::sign(&identity.seed, &blob[..blob_len]);
        let mut buffer = [0u8; MAX_SIGNING_TEXT_BYTES];
        let used = format_signature_file(identity.key_id, &signature, &mut buffer).unwrap();
        let text = core::str::from_utf8(&buffer[..used]).unwrap();
        let record = parse_signature_file(text).expect("sidecar parses");
        assert_eq!(record.key_id, identity.key_id);
        assert_eq!(
            verify_blob_signature(
                &identity.public,
                &blob[..blob_len],
                &record,
                identity.key_id
            ),
            Ok(())
        );

        // Tampered blob byte fails verification over the exact bytes (the
        // FNV checksum is the first gate; the signature is the second).
        let mut tampered = blob;
        tampered[HEADER_LEN] ^= 0x01;
        assert_eq!(
            verify_blob_signature(
                &identity.public,
                &tampered[..blob_len],
                &record,
                identity.key_id
            ),
            Err(BackupError::BadSignature)
        );

        // Tampered signature text fails.
        let mut damaged_sig = text.to_string();
        let position = damaged_sig.find("sig=").unwrap() + 4;
        damaged_sig.replace_range(position..position + 2, "00");
        let broken = parse_signature_file(&damaged_sig).unwrap();
        assert_eq!(
            verify_blob_signature(
                &identity.public,
                &blob[..blob_len],
                &broken,
                identity.key_id
            ),
            Err(BackupError::BadSignature)
        );

        // A different identity's key never verifies.
        let other = test_identity(4);
        assert_eq!(
            verify_blob_signature(&other.public, &blob[..blob_len], &record, other.key_id),
            Err(BackupError::BadSignature)
        );

        // Key-id mismatch inside the record is refused before crypto.
        let mut renamed = record;
        renamed.key_id = 0xdead_beef;
        assert_eq!(
            verify_blob_signature(
                &identity.public,
                &blob[..blob_len],
                &renamed,
                identity.key_id
            ),
            Err(BackupError::BadSignature)
        );

        // Malformed sidecar text is None.
        assert_eq!(parse_signature_file("no equals sign\n"), None);
        assert_eq!(parse_signature_file("alg=ed25519\n"), None);
        assert_eq!(
            parse_signature_file("alg=sha1\nkey-id=0000000000000000\nsig=00\n"),
            None
        );
    }

    #[test]
    fn hex_helpers_roundtrip() {
        let mut out = [0u8; 64];
        let used = emit_hex(&[0xde, 0xad, 0xbe, 0xef], &mut out);
        assert_eq!(&out[..used], b"deadbeef");
        let mut bytes = [0u8; 4];
        assert_eq!(parse_hex_into(b"deadbeef", &mut bytes), Some(()));
        assert_eq!(bytes, [0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(parse_hex_into(b"deadbee", &mut bytes), None);
        assert_eq!(parse_hex_into(b"deadbeeg", &mut bytes), None);
        assert_eq!(
            parse_hex_u64(b"0123456789abcdef"),
            Some(0x0123_4567_89ab_cdef)
        );
        assert_eq!(parse_hex_u64(b"0123"), None);
    }
}

#[cfg(test)]
mod debug_tests {
    use super::*;
    #[test]
    fn debug_walk() {
        let mut writer = BlobWriter::new();
        writer
            .push_record(scope::CONFIG, "desktop/settings.cfg", b"key=value\n")
            .unwrap();
        let length = writer.finish();
        let (buffer, _) = writer.into_parts();
        eprintln!(
            "length={} magic={:#x} ver={:#x} mask={:#x} count={:#x} ck={:#x}",
            length,
            le_u32(&buffer[0..4]),
            le_u32(&buffer[4..8]),
            le_u32(&buffer[8..12]),
            le_u32(&buffer[12..16]),
            le_u64(&buffer[16..24])
        );
        eprintln!("computed_ck={:#x}", fnv1a64(&buffer[HEADER_LEN..length]));
        eprintln!(
            "first_payload_bytes={:?}",
            &buffer[HEADER_LEN..HEADER_LEN + 16]
        );
        let _ = BlobView::open(&buffer[..length]);
    }
}

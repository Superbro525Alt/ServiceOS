//! Signed-feed integrity model shared between the `no_std` service binary
//! and host unit tests: a keyed double-FNV digest over the sorted canonical
//! form of a repository feed, per-source pinned keys in a local keystore,
//! key rotation with an old-key acceptance window, feed verification
//! verdicts, and a bounded journal of rejected feeds.
//!
//! HONEST LIMITS: the keyed FNV-1a construction is an integrity checksum
//! bound to a locally pinned secret. It detects accidental or naive
//! tampering and makes unsigned modification of a signed feed detectable
//! as long as the keystore stays secret, but it is NOT cryptography: FNV
//! has no collision or preimage resistance guarantees, offers no
//! unforgeability against an adversary that can study many message/digest
//! pairs, and provides no identity binding beyond "whoever holds the key".
//! Real Ed25519-style signatures, key provenance, and a proper trust root
//! remain open roadmap work.

pub const KEY_ID_MAX: usize = 24;
/// Longest pinned-key hex: 32 chars for the legacy FNV secret, 64 for an
/// Ed25519 compressed public key.
pub const KEY_HEX_MAX: usize = 64;
pub const SOURCE_NAME_MAX: usize = 32;
pub const MAX_KEYS_PER_SOURCE: usize = 4;
pub const MAX_SIGNED_SOURCES: usize = 4;
pub const MAX_CANON_LINES: usize = 256;
pub const REJECT_RECORDS_MAX: usize = 8;
/// Hard cap on the canonical byte stream an Ed25519 signature may cover.
pub const ED25519_MSG_MAX: usize = 8192;

pub const SIG_KEY_PREFIX: &str = "sig-key=";
pub const SIG_DIGEST_PREFIX: &str = "sig-digest=";
pub const SIG_ALG_PREFIX: &str = "sig-alg=";
pub const SIG_SIG_PREFIX: &str = "sig-sig=";
pub const ALG_ED25519: &str = "ed25519";

/// Rejection reason words persisted in the feed-reject journal.
pub const REJECT_UNSIGNED_REQUIRED: u64 = 1;
pub const REJECT_TAMPERED: u64 = 2;
pub const REJECT_STALE_SIGNATURE: u64 = 3;
pub const REJECT_UNKNOWN_KEY: u64 = 4;
pub const REJECT_WRONG_KEY_BINDING: u64 = 5;

pub fn reject_reason(verdict: FeedVerdict) -> u64 {
    match verdict {
        FeedVerdict::RejectedUnsignedRequired => REJECT_UNSIGNED_REQUIRED,
        FeedVerdict::RejectedTampered => REJECT_TAMPERED,
        FeedVerdict::RejectedStaleSignature => REJECT_STALE_SIGNATURE,
        FeedVerdict::UnknownKey => REJECT_UNKNOWN_KEY,
        FeedVerdict::RejectedWrongKeyBinding => REJECT_WRONG_KEY_BINDING,
        _ => 0,
    }
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x1000_0000_01b3;

#[allow(dead_code)]
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for byte in bytes.iter().copied() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyState {
    Active,
    Retired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FixedText<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> FixedText<N> {
    pub const fn empty() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    pub fn set(&mut self, value: &str) -> bool {
        if value.len() > N {
            return false;
        }
        self.bytes = [0; N];
        self.bytes[..value.len()].copy_from_slice(value.as_bytes());
        self.len = value.len();
        true
    }

    pub fn as_str(&self) -> &str {
        match core::str::from_utf8(&self.bytes[..self.len]) {
            Ok(text) => text,
            Err(_) => "",
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyAlg {
    /// Legacy keyed double-FNV integrity digest (secret key material).
    Fnv,
    /// Real Ed25519 signature verification (compressed public key).
    Ed25519,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrustedKey {
    pub key_id: FixedText<KEY_ID_MAX>,
    pub key_hex: FixedText<KEY_HEX_MAX>,
    pub alg: KeyAlg,
    pub state: KeyState,
    pub retired_tick: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoundKeyIdentity {
    pub key_id: FixedText<KEY_ID_MAX>,
    pub fingerprint: u64,
}

impl BoundKeyIdentity {
    pub const fn empty() -> Self {
        Self {
            key_id: FixedText::empty(),
            fingerprint: 0,
        }
    }

    pub fn matches(&self, key_id: &str, fingerprint: u64) -> bool {
        !self.key_id.is_empty()
            && self.key_id.as_str() == key_id
            && self.fingerprint != 0
            && self.fingerprint == fingerprint
    }
}

impl TrustedKey {
    pub const fn empty() -> Self {
        Self {
            key_id: FixedText::empty(),
            key_hex: FixedText::empty(),
            alg: KeyAlg::Fnv,
            state: KeyState::Retired,
            retired_tick: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceKeys {
    pub source: FixedText<SOURCE_NAME_MAX>,
    pub keys: [TrustedKey; MAX_KEYS_PER_SOURCE],
    pub key_count: usize,
    pub accept_retired_ticks: u64,
}

#[derive(Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub enum KeystoreError {
    UnknownSource,
    SourceFull,
    DuplicateKey,
    InvalidKeyId,
    InvalidKeyHex,
    UnknownKey,
    SameKeyActive,
    NoActiveKey,
}

impl SourceKeys {
    pub const fn empty() -> Self {
        Self {
            source: FixedText::empty(),
            keys: [TrustedKey::empty(); MAX_KEYS_PER_SOURCE],
            key_count: 0,
            accept_retired_ticks: 0,
        }
    }

    pub fn find_key(&self, key_id: &str) -> Option<&TrustedKey> {
        self.keys[..self.key_count]
            .iter()
            .find(|key| key.key_id.as_str() == key_id)
    }

    /// Enroll a new key for this source. The first enrolled key bootstraps
    /// as the active anchor; later keys start retired so they only verify
    /// once rotation promotes them.
    #[allow(dead_code)]
    pub fn enroll(&mut self, key_id: &str, key_hex: &str) -> Result<(), KeystoreError> {
        if key_id.is_empty() || key_id.len() > KEY_ID_MAX {
            return Err(KeystoreError::InvalidKeyId);
        }
        if decode_key_hex(key_hex).is_none() {
            return Err(KeystoreError::InvalidKeyHex);
        }
        if self.find_key(key_id).is_some() {
            return Err(KeystoreError::DuplicateKey);
        }
        let has_active = self.keys[..self.key_count]
            .iter()
            .any(|key| key.state == KeyState::Active);
        let slot = match self.keys[..self.key_count]
            .iter_mut()
            .find(|key| key.key_id.is_empty())
        {
            Some(slot) => slot,
            None if self.key_count < MAX_KEYS_PER_SOURCE => {
                self.key_count += 1;
                &mut self.keys[self.key_count - 1]
            }
            None => return Err(KeystoreError::SourceFull),
        };
        slot.key_id.set(key_id);
        slot.key_hex.set(key_hex);
        slot.alg = KeyAlg::Fnv;
        slot.state = if has_active {
            KeyState::Retired
        } else {
            KeyState::Active
        };
        slot.retired_tick = 0;
        Ok(())
    }

    /// Enroll an Ed25519 verification key (64-hex compressed public key).
    /// The first enrolled key bootstraps as the active anchor; later keys
    /// start retired so they only verify once rotation promotes them.
    #[allow(dead_code)]
    pub fn enroll_ed25519(&mut self, key_id: &str, pubkey_hex: &str) -> Result<(), KeystoreError> {
        if key_id.is_empty() || key_id.len() > KEY_ID_MAX {
            return Err(KeystoreError::InvalidKeyId);
        }
        if decode_pubkey_hex(pubkey_hex).is_none() {
            return Err(KeystoreError::InvalidKeyHex);
        }
        if self.find_key(key_id).is_some() {
            return Err(KeystoreError::DuplicateKey);
        }
        let has_active = self.keys[..self.key_count]
            .iter()
            .any(|key| key.state == KeyState::Active);
        let slot = match self.keys[..self.key_count]
            .iter_mut()
            .find(|key| key.key_id.is_empty())
        {
            Some(slot) => slot,
            None if self.key_count < MAX_KEYS_PER_SOURCE => {
                self.key_count += 1;
                &mut self.keys[self.key_count - 1]
            }
            None => return Err(KeystoreError::SourceFull),
        };
        slot.key_id.set(key_id);
        slot.key_hex.set(pubkey_hex);
        slot.alg = KeyAlg::Ed25519;
        slot.state = if has_active {
            KeyState::Retired
        } else {
            KeyState::Active
        };
        slot.retired_tick = 0;
        Ok(())
    }

    /// Promote an already-enrolled key to active and retire the currently
    /// active one at `now`. This is the rotation operation: callers persist
    /// the keystore afterwards, which re-signs the verification config.
    pub fn rotate_active(&mut self, new_key_id: &str, now: u64) -> Result<(), KeystoreError> {
        if self.find_key(new_key_id).is_none() {
            return Err(KeystoreError::UnknownKey);
        }
        let Some(current_active) = self.keys[..self.key_count]
            .iter()
            .position(|key| key.state == KeyState::Active)
        else {
            return Err(KeystoreError::NoActiveKey);
        };
        if self.keys[current_active].key_id.as_str() == new_key_id {
            return Err(KeystoreError::SameKeyActive);
        }
        self.keys[current_active].state = KeyState::Retired;
        self.keys[current_active].retired_tick = now;
        let promoted = self.keys[..self.key_count]
            .iter_mut()
            .find(|key| key.key_id.as_str() == new_key_id)
            .map(|key| {
                key.state = KeyState::Active;
                key.retired_tick = 0;
            });
        match promoted {
            Some(()) => Ok(()),
            None => Err(KeystoreError::UnknownKey),
        }
    }

    /// Whole-source rotation: promote the MOST RECENTLY enrolled retired
    /// key (the just-provisioned standby) to active and retire the current
    /// active key at `now`. Returns the promoted key's slot index so the
    /// caller can report which id became active.
    pub fn rotate_source(&mut self, now: u64) -> Result<usize, KeystoreError> {
        let Some(new_slot) = self.keys[..self.key_count]
            .iter()
            .rposition(|key| key.state == KeyState::Retired)
        else {
            return Err(KeystoreError::UnknownKey);
        };
        // Copy the id out first so the immutable slot borrow ends before
        // rotate_active takes the entry mutably again.
        let id_bytes = self.keys[new_slot].key_id.as_str().as_bytes();
        if id_bytes.is_empty() || id_bytes.len() > KEY_ID_MAX {
            return Err(KeystoreError::UnknownKey);
        }
        let mut id_buffer = [0u8; KEY_ID_MAX];
        id_buffer[..id_bytes.len()].copy_from_slice(id_bytes);
        let key_id = core::str::from_utf8(&id_buffer[..id_bytes.len()])
            .map_err(|_| KeystoreError::InvalidKeyId)?;
        self.rotate_active(key_id, now)?;
        Ok(new_slot)
    }

    pub fn retired_within_window(&self, key: &TrustedKey, now: u64) -> bool {
        if self.accept_retired_ticks == 0 || key.state != KeyState::Retired {
            return false;
        }
        now.saturating_sub(key.retired_tick) <= self.accept_retired_ticks
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Keystore {
    pub sources: [SourceKeys; MAX_SIGNED_SOURCES],
    pub source_count: usize,
}

impl Keystore {
    pub const fn empty() -> Self {
        Self {
            sources: [SourceKeys::empty(); MAX_SIGNED_SOURCES],
            source_count: 0,
        }
    }

    pub fn source_keys(&self, source: &str) -> Option<&SourceKeys> {
        self.sources[..self.source_count]
            .iter()
            .find(|entry| entry.source.as_str() == source)
    }

    pub fn source_keys_mut(&mut self, source: &str) -> Option<&mut SourceKeys> {
        self.sources[..self.source_count]
            .iter_mut()
            .find(|entry| entry.source.as_str() == source)
    }

    pub fn ensure_source(&mut self, source: &str) -> Result<&mut SourceKeys, KeystoreError> {
        if source.is_empty() || source.len() > SOURCE_NAME_MAX {
            return Err(KeystoreError::InvalidKeyId);
        }
        if let Some(index) = self.sources[..self.source_count]
            .iter()
            .position(|entry| entry.source.as_str() == source)
        {
            return Ok(&mut self.sources[index]);
        }
        if self.source_count == MAX_SIGNED_SOURCES {
            return Err(KeystoreError::SourceFull);
        }
        self.sources[self.source_count].source.set(source);
        self.source_count += 1;
        Ok(&mut self.sources[self.source_count - 1])
    }
}

/// Wire encoding for key algorithm words: 1 = keyed FNV digest, 2 =
/// Ed25519 verification key.
pub const ALG_WORD_FNV: u64 = 1;
pub const ALG_WORD_ED25519: u64 = 2;
/// Wire encoding for key state words: 1 = active, 2 = retired.
pub const STATE_WORD_ACTIVE: u64 = 1;
pub const STATE_WORD_RETIRED: u64 = 2;

pub fn alg_word(alg: KeyAlg) -> u64 {
    match alg {
        KeyAlg::Fnv => ALG_WORD_FNV,
        KeyAlg::Ed25519 => ALG_WORD_ED25519,
    }
}

pub fn state_word(state: KeyState) -> u64 {
    match state {
        KeyState::Active => STATE_WORD_ACTIVE,
        KeyState::Retired => STATE_WORD_RETIRED,
    }
}

/// Encode `bytes` as lowercase hex into `out`; returns the written length
/// (2 * bytes.len(), capped by the caller's buffer).
pub fn encode_hex(bytes: &[u8], out: &mut [u8]) -> usize {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut len = 0usize;
    for byte in bytes {
        if len + 2 > out.len() {
            break;
        }
        out[len] = DIGITS[(byte >> 4) as usize];
        out[len + 1] = DIGITS[(byte & 0xf) as usize];
        len += 2;
    }
    len
}

/// Deterministic short key id derived from the key material itself:
/// `k-` plus 16 lowercase hex digits of FNV-1a64 over the decoded key
/// bytes. Returned as (`bytes`, `len`) ready for FixedText::set via
/// core::str::from_utf8.
pub fn auto_key_id(key_bytes: &[u8]) -> ([u8; KEY_ID_MAX + 2], usize) {
    let mut buffer = [0u8; KEY_ID_MAX + 2];
    let prefix = b"k-";
    buffer[..prefix.len()].copy_from_slice(prefix);
    let mut hex_out = [0u8; 16];
    let mut word = FNV_OFFSET;
    for byte in key_bytes {
        word ^= u64::from(*byte);
        word = word.wrapping_mul(FNV_PRIME);
    }
    // Big-endian nibble emit keeps the id stable across readers.
    for index in 0..16 {
        hex_out[index] = ((word >> (60 - 4 * index)) & 0xf) as u8;
        if hex_out[index] < 10 {
            hex_out[index] += b'0';
        } else {
            hex_out[index] += b'a' - 10;
        }
    }
    buffer[prefix.len()..prefix.len() + 16].copy_from_slice(&hex_out);
    let len = prefix.len() + 16;
    (buffer, len)
}

/// Candidate struct returned by [`derive_generated_identity`]: a fresh
/// Ed25519 seed, its compressed public key, both in hex, and the derived
/// auto key id.
#[derive(Clone, Copy)]
pub struct GeneratedKey {
    pub seed: [u8; 32],
    pub public: [u8; 32],
    pub id_bytes: [u8; KEY_ID_MAX + 2],
    pub id_len: usize,
}

impl GeneratedKey {
    pub fn seed_hex(&self) -> [u8; 64] {
        let mut out = [0u8; 64];
        let _ = encode_hex(&self.seed, &mut out);
        out
    }

    pub fn public_hex(&self) -> [u8; 64] {
        let mut out = [0u8; 64];
        let _ = encode_hex(&self.public, &mut out);
        out
    }

    pub fn id_str(&self) -> Option<&str> {
        core::str::from_utf8(&self.id_bytes[..self.id_len]).ok()
    }
}

/// Build one fresh Ed25519 identity from guest-local entropy substitutes:
/// SHA-512 over (source, monotonic tick, per-call counter, store fingerprint).
///
/// HONEST LIMITS: this kernel exposes no hardware RNG yet and its monotonic
/// tick may stand still on some builds, so the seed is UNIQUE-ISH, not
/// cryptographically random. Suitable for test/tooling flows; production
/// keys should be generated on the host and enrolled with their hex pubkey.
pub fn derive_generated_identity(
    source: &[u8],
    tick: u64,
    counter: u64,
    store_fingerprint: u64,
) -> GeneratedKey {
    let mut block = [0u8; 64];
    let source_len = source.len().min(24);
    block[..source_len].copy_from_slice(&source[..source_len]);
    block[24..32].copy_from_slice(&tick.to_le_bytes());
    block[32..40].copy_from_slice(&counter.to_le_bytes());
    block[40..48].copy_from_slice(&store_fingerprint.to_le_bytes());
    let digest = serviceos_crypto::sha512::digest(&[&block]);
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&digest[..32]);
    let public = serviceos_crypto::ed25519::public_key(&seed);
    let (id_bytes, id_len) = auto_key_id(&public);
    GeneratedKey {
        seed,
        public,
        id_bytes,
        id_len,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SigKind {
    /// Legacy keyed double-FNV integrity checksum.
    FnvDigest,
    /// Real Ed25519 signature over the canonical content stream.
    Ed25519,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FeedSignature {
    pub key_id: FixedText<KEY_ID_MAX>,
    pub kind: SigKind,
    /// FNV digest (meaningful only for `SigKind::FnvDigest`).
    pub digest: u64,
    /// R||S signature bytes (meaningful only for `SigKind::Ed25519`).
    pub sig: [u8; 64],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FeedVerification {
    pub verdict: FeedVerdict,
    pub key_id: FixedText<KEY_ID_MAX>,
    pub key_alg: Option<KeyAlg>,
    pub key_fingerprint: u64,
}

impl FeedVerification {
    pub const fn empty(verdict: FeedVerdict) -> Self {
        Self {
            verdict,
            key_id: FixedText::empty(),
            key_alg: None,
            key_fingerprint: 0,
        }
    }
}

pub fn parse_hex_u64(value: &str) -> Option<u64> {
    if value.is_empty() || value.len() > 16 {
        return None;
    }
    let mut digest: u64 = 0;
    for byte in value.bytes() {
        let nibble = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        digest = (digest << 4) | nibble as u64;
    }
    Some(digest)
}

pub fn decode_key_hex(value: &str) -> Option<[u8; 16]> {
    if value.is_empty() || value.len() > 32 || value.len() % 2 != 0 {
        return None;
    }
    let mut key = [0u8; 16];
    let byte_len = value.len() / 2;
    for index in 0..byte_len {
        let high = parse_hex_nibble(value.as_bytes()[index * 2])?;
        let low = parse_hex_nibble(value.as_bytes()[index * 2 + 1])?;
        key[index] = (high << 4) | low;
    }
    Some(key)
}

fn parse_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn decode_hex_bytes<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 {
        return None;
    }
    let bytes = value.as_bytes();
    let mut out = [0u8; N];
    for index in 0..N {
        let hi = parse_hex_nibble(bytes[index * 2])?;
        let lo = parse_hex_nibble(bytes[index * 2 + 1])?;
        out[index] = (hi << 4) | lo;
    }
    Some(out)
}

/// Decode an Ed25519 compressed public key from its 64-hex encoding.
pub fn decode_pubkey_hex(value: &str) -> Option<[u8; 32]> {
    decode_hex_bytes::<32>(value)
}

pub fn ed25519_key_fingerprint(public: &[u8; 32]) -> u64 {
    let digest = serviceos_crypto::sha512::digest(&[public]);
    u64::from_be_bytes(digest[..8].try_into().unwrap_or([0; 8]))
}

pub fn ed25519_key_fingerprint_hex(value: &str) -> Option<u64> {
    decode_pubkey_hex(value).map(|public| ed25519_key_fingerprint(&public))
}

pub fn active_ed25519_binding(entry: &SourceKeys) -> Option<BoundKeyIdentity> {
    entry.keys[..entry.key_count]
        .iter()
        .find(|key| key.state == KeyState::Active && key.alg == KeyAlg::Ed25519)
        .and_then(|key| {
            ed25519_key_fingerprint_hex(key.key_hex.as_str()).map(|fingerprint| {
                let mut key_id = FixedText::empty();
                let _ = key_id.set(key.key_id.as_str());
                BoundKeyIdentity {
                    key_id,
                    fingerprint,
                }
            })
        })
}

fn is_signature_line(line: &str) -> bool {
    line.starts_with(SIG_KEY_PREFIX)
        || line.starts_with(SIG_DIGEST_PREFIX)
        || line.starts_with(SIG_ALG_PREFIX)
        || line.starts_with(SIG_SIG_PREFIX)
}

/// Extract the trailing signature lines from a feed, if present. Two
/// trailer grammars are understood:
/// - legacy integrity checksum: `sig-key=` + `sig-digest=<u64 hex>`
/// - Ed25519 signature: `sig-alg=ed25519` + `sig-key=` + `sig-sig=<128 hex>`
pub fn parse_feed_signature(feed: &str) -> Option<FeedSignature> {
    let mut key_id = FixedText::<KEY_ID_MAX>::empty();
    let mut digest: Option<u64> = None;
    let mut ed_sig: Option<[u8; 64]> = None;
    let mut alg_ed25519 = false;
    for line in feed.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(value) = line.strip_prefix(SIG_KEY_PREFIX) {
            let _ = key_id.set(value.trim());
        } else if let Some(value) = line.strip_prefix(SIG_DIGEST_PREFIX) {
            digest = parse_hex_u64(value.trim());
        } else if let Some(value) = line.strip_prefix(SIG_ALG_PREFIX) {
            alg_ed25519 |= value.trim() == ALG_ED25519;
        } else if let Some(value) = line.strip_prefix(SIG_SIG_PREFIX) {
            ed_sig = decode_hex_bytes::<64>(value.trim());
        }
    }
    if alg_ed25519
        && !key_id.is_empty()
        && let Some(sig) = ed_sig
    {
        return Some(FeedSignature {
            key_id,
            kind: SigKind::Ed25519,
            digest: 0,
            sig,
        });
    }
    if !alg_ed25519 && ed_sig.is_none() && !key_id.is_empty() {
        if let Some(digest) = digest {
            return Some(FeedSignature {
                key_id,
                kind: SigKind::FnvDigest,
                digest,
                sig: [0u8; 64],
            });
        }
    }
    None
}

/// Canonical feed content: every non-signature line, trimmed, empties
/// dropped, then sorted so equivalent manifests hash identically.
/// Returns the number of canonical lines written into `lines`.
pub fn canonical_lines<'a>(feed: &'a str, lines: &mut [&'a str; MAX_CANON_LINES]) -> usize {
    let mut count = 0usize;
    for line in feed.lines().map(str::trim) {
        if line.is_empty() || is_signature_line(line) {
            continue;
        }
        if count == lines.len() {
            break;
        }
        lines[count] = line;
        count += 1;
    }
    for index in 1..count {
        let mut position = index;
        while position > 0 && lines[position - 1] > lines[position] {
            lines.swap(position - 1, position);
            position -= 1;
        }
    }
    count
}

/// Keyed digest over the canonical form of `lines`: inner FNV-1a over
/// (key bytes ++ each canonical line ++ '\n'), outer FNV-1a over
/// (key bytes ++ inner digest). Integrity checksum only — see module docs.
pub fn compute_feed_digest(key_hex: &str, lines: &[&str], count: usize) -> u64 {
    let key_bytes = decode_key_hex(key_hex).unwrap_or([0u8; 16]);
    let mut inner = FNV_OFFSET;
    update_hash(&mut inner, &key_bytes);
    for line in lines[..count].iter() {
        update_hash(&mut inner, line.as_bytes());
        update_hash(&mut inner, b"\n");
    }
    let mut outer = FNV_OFFSET;
    update_hash(&mut outer, &key_bytes);
    update_hash(&mut outer, &inner.to_le_bytes());
    outer
}

fn update_hash(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes.iter().copied() {
        *hash ^= byte as u64;
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

/// Produce a signed feed by appending `sig-key=`/`sig-digest=` lines over
/// the canonical content already present in `feed`.
#[allow(dead_code)]
pub fn sign_feed_text(
    feed: &str,
    key_id: &str,
    key_hex: &str,
    append: &mut dyn core::fmt::Write,
) -> Option<u64> {
    let mut lines = [""; MAX_CANON_LINES];
    let count = canonical_lines(feed, &mut lines);
    let digest = compute_feed_digest(key_hex, &lines, count);
    write!(append, "{}{}\n", feed.trim_end(), "\n").ok()?;
    write!(append, "{}{}\n", SIG_KEY_PREFIX, key_id).ok()?;
    write!(append, "{}{:016x}\n", SIG_DIGEST_PREFIX, digest).ok()?;
    Some(digest)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeedVerdict {
    /// Valid signature from the active key.
    Accepted,
    /// Valid signature from a recently retired key inside the window.
    AcceptedRetired,
    /// No keys pinned for this source: honor-system behavior applies.
    UnsignedNoKeysPinned,
    /// Feed carries a signature from a key we do not pin: untrusted until
    /// an operator explicitly accepts it through the existing trust flow.
    UnknownKey,
    /// Keys are pinned but the feed carries no signature: hard fail.
    RejectedUnsignedRequired,
    /// Signature does not cover the observed content: hard fail.
    RejectedTampered,
    /// Signed by a retired key whose acceptance window has closed: hard fail.
    RejectedStaleSignature,
    /// Signature verifies, but not against the repository's bound key identity.
    RejectedWrongKeyBinding,
}

/// Rebuild the canonical content byte stream (sorted lines, each followed
/// by '\n') that an Ed25519 signature covers. Returns `None` when the
/// stream would exceed `ED25519_MSG_MAX`.
pub fn canonical_message(feed: &str, message: &mut [u8; ED25519_MSG_MAX]) -> Option<usize> {
    let mut lines = [""; MAX_CANON_LINES];
    let count = canonical_lines(feed, &mut lines);
    let mut len = 0usize;
    for line in lines[..count].iter() {
        let bytes = line.as_bytes();
        if len + bytes.len() + 1 > message.len() {
            return None;
        }
        message[len..len + bytes.len()].copy_from_slice(bytes);
        len += bytes.len();
        message[len] = b'\n';
        len += 1;
    }
    Some(len)
}

/// Produce an Ed25519-signed feed: the seed signs the canonical stream and
/// `sig-alg=`/`sig-key=`/`sig-sig=` trailers are appended. Host-side only;
/// requires the seed rather than the public key.
#[allow(dead_code)]
pub fn sign_feed_text_ed25519(
    feed: &str,
    key_id: &str,
    seed: &[u8; 32],
    append: &mut dyn core::fmt::Write,
) -> Option<[u8; 32]> {
    let public = serviceos_crypto::ed25519::public_key(seed);
    let mut message = [0u8; ED25519_MSG_MAX];
    let len = canonical_message(feed, &mut message)?;
    let signature = serviceos_crypto::ed25519::sign(seed, &message[..len]);
    write!(append, "{}{}\n", feed.trim_end(), "\n").ok()?;
    write!(append, "{}{}\n", SIG_ALG_PREFIX, ALG_ED25519).ok()?;
    write!(append, "{}{}\n", SIG_KEY_PREFIX, key_id).ok()?;
    for byte in signature.iter() {
        write!(append, "{:02x}", byte).ok()?;
    }
    writeln!(append).ok()?;
    Some(public)
}

/// Verify a feed against the keys pinned for its source (`None` when the
/// keystore holds no entry for the source).
pub fn verify_signed_feed_report(
    feed: &str,
    entry: Option<&SourceKeys>,
    now: u64,
) -> FeedVerification {
    let entry = match entry {
        Some(entry) if entry.key_count > 0 => entry,
        _ => return FeedVerification::empty(FeedVerdict::UnsignedNoKeysPinned),
    };
    let Some(signature) = parse_feed_signature(feed) else {
        return FeedVerification::empty(FeedVerdict::RejectedUnsignedRequired);
    };
    let mut report = FeedVerification::empty(FeedVerdict::UnknownKey);
    report.key_id = signature.key_id;
    let Some(key) = entry.find_key(signature.key_id.as_str()) else {
        return report;
    };
    report.key_alg = Some(key.alg);
    report.key_fingerprint = if key.alg == KeyAlg::Ed25519 {
        ed25519_key_fingerprint_hex(key.key_hex.as_str()).unwrap_or(0)
    } else {
        0
    };
    let accepted = match key.state {
        KeyState::Active => true,
        KeyState::Retired => entry.retired_within_window(key, now),
    };
    if !accepted {
        report.verdict = FeedVerdict::RejectedStaleSignature;
        return report;
    }
    let verified = match signature.kind {
        SigKind::FnvDigest => {
            let alg_ok = key.alg == KeyAlg::Fnv;
            let mut lines = [""; MAX_CANON_LINES];
            let count = canonical_lines(feed, &mut lines);
            alg_ok && compute_feed_digest(key.key_hex.as_str(), &lines, count) == signature.digest
        }
        SigKind::Ed25519 => {
            if key.alg != KeyAlg::Ed25519 {
                false
            } else {
                match (decode_pubkey_hex(key.key_hex.as_str()), ()) {
                    (Some(public), ()) => {
                        let mut message = [0u8; ED25519_MSG_MAX];
                        match canonical_message(feed, &mut message) {
                            Some(len) => serviceos_crypto::ed25519::verify(
                                &public,
                                &message[..len],
                                &signature.sig,
                            ),
                            None => false,
                        }
                    }
                    _ => false,
                }
            }
        }
    };
    report.verdict = if verified {
        if key.state == KeyState::Active {
            FeedVerdict::Accepted
        } else {
            FeedVerdict::AcceptedRetired
        }
    } else {
        FeedVerdict::RejectedTampered
    };
    report
}

pub fn verify_bound_feed(
    feed: &str,
    entry: Option<&SourceKeys>,
    binding: BoundKeyIdentity,
    now: u64,
) -> FeedVerification {
    let mut report = verify_signed_feed_report(feed, entry, now);
    if !matches!(
        report.verdict,
        FeedVerdict::Accepted | FeedVerdict::AcceptedRetired
    ) {
        return report;
    }
    if report.key_alg != Some(KeyAlg::Ed25519)
        || !binding.matches(report.key_id.as_str(), report.key_fingerprint)
    {
        report.verdict = FeedVerdict::RejectedWrongKeyBinding;
    }
    report
}

pub fn verify_signed_feed(feed: &str, entry: Option<&SourceKeys>, now: u64) -> FeedVerdict {
    verify_signed_feed_report(feed, entry, now).verdict
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RejectRecord {
    pub source: FixedText<SOURCE_NAME_MAX>,
    pub reason: u64,
    pub digest: u64,
    pub tick: u64,
    pub occupied: bool,
}

impl RejectRecord {
    pub const fn empty() -> Self {
        Self {
            source: FixedText::empty(),
            reason: 0,
            digest: 0,
            tick: 0,
            occupied: false,
        }
    }
}

/// Bounded ring of rejected-feed records persisted next to the keystore so
/// rejections survive restarts and stay queryable after boot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RejectJournal {
    pub records: [RejectRecord; REJECT_RECORDS_MAX],
    pub cursor: usize,
    pub total: u64,
}

impl RejectJournal {
    pub const fn empty() -> Self {
        Self {
            records: [RejectRecord::empty(); REJECT_RECORDS_MAX],
            cursor: 0,
            total: 0,
        }
    }

    pub fn record(&mut self, source: &str, reason: u64, digest: u64, tick: u64) {
        let slot = self.cursor % REJECT_RECORDS_MAX;
        self.records[slot].source.set(source);
        self.records[slot].reason = reason;
        self.records[slot].digest = digest;
        self.records[slot].tick = tick;
        self.records[slot].occupied = true;
        self.cursor = (self.cursor + 1) % REJECT_RECORDS_MAX;
        self.total = self.total.saturating_add(1);
    }

    /// Most recent record first; returns how many were written.
    pub fn recent(&self, out: &mut [RejectRecord; REJECT_RECORDS_MAX]) -> usize {
        let mut written = 0usize;
        let filled = self.total.min(REJECT_RECORDS_MAX as u64) as usize;
        for offset in 0..filled {
            let index = (self.cursor + REJECT_RECORDS_MAX - 1 - offset) % REJECT_RECORDS_MAX;
            if self.records[index].occupied {
                out[written] = self.records[index];
                written += 1;
            }
        }
        written
    }

    pub fn serialize(&self, append: &mut dyn core::fmt::Write) {
        let _ = core::writeln!(append, "pfj1");
        let _ = core::writeln!(append, "total={}", self.total);
        let mut ordered = [RejectRecord::empty(); REJECT_RECORDS_MAX];
        let written = self.recent(&mut ordered);
        for record in ordered[..written].iter() {
            let _ = core::writeln!(
                append,
                "reject {} {} {:016x} {}",
                record.source.as_str(),
                record.reason,
                record.digest,
                record.tick
            );
        }
    }

    pub fn parse(text: &str) -> Self {
        let mut journal = Self::empty();
        let mut persisted_total: Option<u64> = None;
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
            if let Some(value) = line.strip_prefix("total=") {
                persisted_total = value.parse::<u64>().ok();
                continue;
            }
            let Some(payload) = line.strip_prefix("reject ") else {
                continue;
            };
            let mut parts = payload.split(' ');
            let Some(source) = parts.next() else { continue };
            let Some(reason) = parts.next().and_then(|value| value.parse::<u64>().ok()) else {
                continue;
            };
            let Some(digest) = parts.next().and_then(parse_hex_u64) else {
                continue;
            };
            let tick = parts
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            journal.record(source, reason, digest, tick);
        }
        // Rebuild recency from tick order: `recent()` derives order from the
        // write cursor, which a serialized dump cannot preserve.
        let mut kept = [RejectRecord::empty(); REJECT_RECORDS_MAX];
        let mut count = 0usize;
        for record in journal
            .records
            .iter()
            .copied()
            .filter(|record| record.occupied)
        {
            let mut position = count;
            while position > 0 && kept[position - 1].tick > record.tick {
                kept[position] = kept[position - 1];
                position -= 1;
            }
            kept[position] = record;
            count += 1;
        }
        journal.records = kept;
        journal.cursor = count % REJECT_RECORDS_MAX;
        if let Some(total) = persisted_total {
            journal.total = total;
        }
        journal
    }
}

/// Serialize the keystore (the verification config). Rotation flows call
/// this after mutating entries so the on-disk config matches the promoted
/// active key. Key state packs as a word: 1=active, 2|tick<<2=retired.
pub fn serialize_keystore(keystore: &Keystore, append: &mut dyn core::fmt::Write) {
    let _ = core::writeln!(append, "pks1");
    for source in keystore.sources[..keystore.source_count].iter() {
        let _ = core::writeln!(
            append,
            "window {} {}",
            source.source.as_str(),
            source.accept_retired_ticks
        );
        for key in source.keys[..source.key_count].iter() {
            if key.key_id.is_empty() {
                continue;
            }
            if key.alg == KeyAlg::Ed25519 {
                let _ = core::writeln!(
                    append,
                    "ekey {} {} {}",
                    key.key_id.as_str(),
                    key.key_hex.as_str(),
                    key_state_word(key.state, key.retired_tick)
                );
            } else {
                let _ = core::writeln!(
                    append,
                    "key {} {} {}",
                    key.key_id.as_str(),
                    key.key_hex.as_str(),
                    key_state_word(key.state, key.retired_tick)
                );
            }
        }
    }
}

fn key_state_word(state: KeyState, retired_tick: u64) -> u64 {
    match state {
        KeyState::Active => 1,
        KeyState::Retired => (retired_tick.min(u64::MAX >> 2) << 2) | 2,
    }
}

fn key_state_from_word(word: u64) -> (KeyState, u64) {
    if word & 0b11 == 1 {
        (KeyState::Active, 0)
    } else {
        (KeyState::Retired, word >> 2)
    }
}

#[test]
fn key_state_words_roundtrip() {
    assert_eq!(
        key_state_from_word(key_state_word(KeyState::Active, 0)),
        (KeyState::Active, 0)
    );
    assert_eq!(
        key_state_from_word(key_state_word(KeyState::Retired, 9_999)),
        (KeyState::Retired, 9_999)
    );
}

/// Parse a serialized keystore; malformed lines are skipped so a partial
/// write can never lock every key out.
pub fn parse_keystore(text: &str) -> Keystore {
    let mut keystore = Keystore::empty();
    let mut current: Option<usize> = None;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if line == "pks1" {
            continue;
        }
        if let Some(payload) = line.strip_prefix("window ") {
            let mut parts = payload.split(' ');
            if let (Some(name), Some(window)) = (parts.next(), parts.next()) {
                current = keystore.sources[..keystore.source_count]
                    .iter()
                    .position(|entry| entry.source.as_str() == name);
                if current.is_none() && keystore.ensure_source(name).is_ok() {
                    current = Some(keystore.source_count - 1);
                }
                if let (Some(index), Ok(ticks)) = (current, window.parse::<u64>()) {
                    keystore.sources[index].accept_retired_ticks = ticks;
                }
            }
            continue;
        }
        let (line_kind, payload) = if let Some(payload) = line.strip_prefix("ekey ") {
            ("ekey", payload)
        } else if let Some(payload) = line.strip_prefix("key ") {
            ("key", payload)
        } else {
            continue;
        };
        let mut parts = payload.split(' ');
        let (Some(key_id), Some(key_hex), Some(state_word)) = (
            parts.next(),
            parts.next(),
            parts.next().and_then(|v| v.parse::<u64>().ok()),
        ) else {
            continue;
        };
        let Some(index) = current else { continue };
        let entry = &mut keystore.sources[index];
        let alg = match line_kind {
            "ekey" => match decode_pubkey_hex(key_hex) {
                Some(_) => KeyAlg::Ed25519,
                None => continue,
            },
            _ => {
                if key_hex.len() > 32 || decode_key_hex(key_hex).is_none() {
                    continue;
                }
                KeyAlg::Fnv
            }
        };
        if key_id.is_empty() || key_id.len() > KEY_ID_MAX || entry.key_count == MAX_KEYS_PER_SOURCE
        {
            continue;
        }
        let slot = &mut entry.keys[entry.key_count];
        slot.key_id.set(key_id);
        slot.key_hex.set(key_hex);
        slot.alg = alg;
        let (state, retired_tick) = key_state_from_word(state_word);
        slot.state = state;
        slot.retired_tick = retired_tick;
        entry.key_count += 1;
    }
    keystore
}

#[cfg(test)]
mod tests {
    // Host-test shims: the service binary is `#![no_std]`, but these unit
    // tests build against std for String/format! convenience.
    extern crate std;
    use super::*;
    use std::{
        format,
        string::{String, ToString},
        vec::Vec,
    };

    const KEY_A: &str = "00112233445566778899aabbccddeeff";
    const KEY_B: &str = "ffeeddccbbaa99887766554433221100";

    fn sample_feed() -> &'static str {
        "version=1\n\
         entry=alpha|storage-service|1.2.0|serviceos.bootstore.v1|m/alpha.manifest|tool|Alpha package\n\
         entry=beta|network-service|0.9.1|serviceos.bootstore.v1|m/beta.manifest|net|Beta package\n"
    }

    fn signed_feed(key_hex: &str) -> String {
        let mut text = String::new();
        sign_feed_text(sample_feed(), "k1", key_hex, &mut text).expect("sign");
        text
    }

    fn source_with(keys: &[(&str, &str)], accept_retired_ticks: u64) -> SourceKeys {
        let mut entry = SourceKeys::empty();
        entry.source.set("extra");
        for (key_id, key_hex) in keys {
            let _ = entry.enroll(key_id, key_hex);
        }
        entry.accept_retired_ticks = accept_retired_ticks;
        entry
    }

    fn ed_source_with(key_id: &str, pubkey_hex: &str, accept_retired_ticks: u64) -> SourceKeys {
        let mut entry = SourceKeys::empty();
        entry.source.set("extra");
        let _ = entry.enroll_ed25519(key_id, pubkey_hex);
        entry.accept_retired_ticks = accept_retired_ticks;
        entry
    }

    const ED_SEED_HEX: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";

    fn ed_seed() -> [u8; 32] {
        decode_pubkey_hex(ED_SEED_HEX).unwrap().into()
    }

    fn ed_feed() -> String {
        let pair = serviceos_crypto::host::KeyPair::from_seed(ed_seed());
        serviceos_crypto::host::sign_feed_fixture(&pair, "ed1", &[
            "version=1",
            "entry=alpha|storage-service|1.2.0|serviceos.bootstore.v1|m/alpha.manifest|tool|Alpha package",
            "entry=beta|network-service|0.9.1|serviceos.bootstore.v1|m/beta.manifest|net|Beta package",
        ])
        .expect("fixture")
    }

    fn ed_pair_for(byte: u8) -> serviceos_crypto::host::KeyPair {
        let mut seed = [byte; 32];
        seed[0] = byte.wrapping_add(1);
        seed[31] = byte.wrapping_mul(7).wrapping_add(3);
        serviceos_crypto::host::KeyPair::from_seed(seed)
    }

    fn ed_feed_with_pair(pair: &serviceos_crypto::host::KeyPair, key_id: &str) -> String {
        serviceos_crypto::host::sign_feed_fixture(pair, key_id, &[
            "version=1",
            "entry=alpha|storage-service|1.2.0|serviceos.bootstore.v1|m/alpha.manifest|tool|Alpha package",
            "entry=beta|network-service|0.9.1|serviceos.bootstore.v1|m/beta.manifest|net|Beta package",
        ])
        .expect("fixture")
    }

    fn bound_identity_for(key_id: &str, pubkey_hex: &str) -> BoundKeyIdentity {
        let mut bound = BoundKeyIdentity::empty();
        let _ = bound.key_id.set(key_id);
        bound.fingerprint = ed25519_key_fingerprint_hex(pubkey_hex).expect("fingerprint");
        bound
    }

    #[test]
    fn ed25519_signature_roundtrip_verifies_and_rejects_tamper() {
        let feed = ed_feed();
        assert_eq!(
            parse_feed_signature(&feed).map(|s| s.kind),
            Some(SigKind::Ed25519)
        );
        let pk_hex = hex_of(&serviceos_crypto::host::KeyPair::from_seed(ed_seed()).public);
        let entry = ed_source_with("ed1", &pk_hex, 0);
        assert_eq!(
            verify_signed_feed(&feed, Some(&entry), 0),
            FeedVerdict::Accepted
        );

        // Content tamper must invalidate the signature even if order matches.
        let mut tampered = feed.clone();
        tampered = tampered.replace("1.2.0", "9.9.9");
        assert_eq!(
            verify_signed_feed(&tampered, Some(&entry), 0),
            FeedVerdict::RejectedTampered
        );

        // Line reordering is canonicalized away and still verifies.
        let reordered = {
            let mut lines: Vec<String> = feed
                .lines()
                .filter(|line| !line.starts_with("sig-"))
                .map(|line| line.to_string())
                .collect();
            lines.reverse();
            let joined = lines.join("\n");
            format!("{}\n", joined.trim_end())
                + "\nsig-alg=ed25519\nsig-key=ed1\n"
                + &feed
                    .rsplit_once("sig-sig=")
                    .map(|(_, rest)| format!("sig-sig={}", rest))
                    .expect("trailer")
        };
        assert_eq!(
            verify_signed_feed(&reordered, Some(&entry), 0),
            FeedVerdict::Accepted
        );
    }

    fn hex_of(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }

    #[test]
    fn ed25519_unknown_key_and_alg_mismatch_paths() {
        let feed = ed_feed();
        // Different pinned Ed25519 identity under the same key id: lookup
        // succeeds, signature check fails -> tamper verdict.
        let other_pair = serviceos_crypto::host::KeyPair::from_seed({
            let mut seed = [7u8; 32];
            seed[0] = 42;
            seed
        });
        let entry = ed_source_with("ed1", &hex_of(&other_pair.public), 0);
        assert_eq!(
            verify_signed_feed(&feed, Some(&entry), 0),
            FeedVerdict::RejectedTampered
        );

        // Same key id but pinned through the FNV path -> alg mismatch is Tampered.
        let mut legacy = source_with(&[("ed1", KEY_A)], 0);
        let _ = &mut legacy;
        assert_eq!(
            verify_signed_feed(&feed, Some(&legacy), 0),
            FeedVerdict::RejectedTampered
        );
    }

    #[test]
    fn bound_ed25519_trust_accepts_only_the_bound_identity() {
        let active = serviceos_crypto::host::KeyPair::from_seed(ed_seed());
        let active_hex = hex_of(&active.public);
        let other = ed_pair_for(7);
        let other_hex = hex_of(&other.public);

        let mut entry = SourceKeys::empty();
        entry.source.set("extra");
        let _ = entry.enroll_ed25519("ed1", &active_hex);
        let _ = entry.enroll_ed25519("ed2", &other_hex);

        let accepted = verify_bound_feed(
            &ed_feed(),
            Some(&entry),
            bound_identity_for("ed1", &active_hex),
            0,
        );
        assert_eq!(accepted.verdict, FeedVerdict::Accepted);
        assert_eq!(
            accepted.key_fingerprint,
            ed25519_key_fingerprint_hex(&active_hex).unwrap()
        );

        let wrong = verify_bound_feed(
            &ed_feed(),
            Some(&entry),
            bound_identity_for("ed2", &other_hex),
            0,
        );
        assert_eq!(wrong.verdict, FeedVerdict::RejectedWrongKeyBinding);

        let unknown = verify_bound_feed(
            &ed_feed_with_pair(&ed_pair_for(9), "ed9"),
            Some(&entry),
            bound_identity_for("ed1", &active_hex),
            0,
        );
        assert_eq!(unknown.verdict, FeedVerdict::UnknownKey);

        let tampered = verify_bound_feed(
            &ed_feed().replace("1.2.0", "9.9.9"),
            Some(&entry),
            bound_identity_for("ed1", &active_hex),
            0,
        );
        assert_eq!(tampered.verdict, FeedVerdict::RejectedTampered);
    }

    #[test]
    fn bound_ed25519_respects_retired_key_window() {
        let first = serviceos_crypto::host::KeyPair::from_seed(ed_seed());
        let first_hex = hex_of(&first.public);
        let second = ed_pair_for(11);
        let second_hex = hex_of(&second.public);
        let mut entry = SourceKeys::empty();
        entry.source.set("extra");
        let _ = entry.enroll_ed25519("ed1", &first_hex);
        let _ = entry.enroll_ed25519("ed2", &second_hex);
        entry.accept_retired_ticks = 500;
        let _ = entry.rotate_active("ed2", 1_000);

        let bound = bound_identity_for("ed1", &first_hex);
        assert_eq!(
            verify_bound_feed(&ed_feed(), Some(&entry), bound, 1_400).verdict,
            FeedVerdict::AcceptedRetired
        );
        assert_eq!(
            verify_bound_feed(&ed_feed(), Some(&entry), bound, 1_501).verdict,
            FeedVerdict::RejectedStaleSignature
        );
    }

    #[test]
    fn keystore_roundtrips_ed25519_key_entries() {
        let pair = serviceos_crypto::host::KeyPair::from_seed(ed_seed());
        let pk_hex = hex_of(&pair.public);
        let mut entry = SourceKeys::empty();
        entry.source.set("extra");
        let _ = entry.enroll_ed25519("ed1", &pk_hex);
        let _ = entry.enroll("k-fnv", KEY_A);

        let mut text = String::new();
        serialize_keystore(
            &{
                let mut ks = Keystore::empty();
                let _ = ks.ensure_source("extra");
                ks.sources[0] = entry;
                ks.source_count = 1;
                ks
            },
            &mut text,
        );
        assert!(text.contains("ekey ed1 "));
        assert!(text.contains("key k-fnv "));

        let parsed = parse_keystore(&text);
        let back = &parsed.sources[0];
        assert_eq!(back.key_count, 2);
        assert_eq!(back.keys[0].alg, KeyAlg::Ed25519);
        assert_eq!(back.keys[0].key_hex.as_str(), pk_hex.as_str());
        assert_eq!(back.keys[0].state, KeyState::Active);
        assert_eq!(back.keys[1].alg, KeyAlg::Fnv);

        // The reparsed Ed25519 entry verifies a freshly signed fixture too.
        let feed = ed_feed();
        assert_eq!(
            verify_signed_feed(&feed, Some(back), 0),
            FeedVerdict::Accepted
        );
    }

    #[test]
    fn digest_is_stable_and_order_insensitive_but_key_bound() {
        let mut lines = [""; MAX_CANON_LINES];
        let count = canonical_lines(sample_feed(), &mut lines);
        let first = compute_feed_digest(KEY_A, &lines, count);
        let again = compute_feed_digest(KEY_A, &lines, count);
        assert_eq!(first, again);

        let reordered = "entry=beta|network-service|0.9.1|serviceos.bootstore.v1|m/beta.manifest|net|Beta package\n\
         version=1\n\
         entry=alpha|storage-service|1.2.0|serviceos.bootstore.v1|m/alpha.manifest|tool|Alpha package\n";
        let mut other_lines = [""; MAX_CANON_LINES];
        let other_count = canonical_lines(reordered, &mut other_lines);
        assert_eq!(count, other_count);
        assert_eq!(first, compute_feed_digest(KEY_A, &other_lines, other_count));

        assert_ne!(first, compute_feed_digest(KEY_B, &lines, count));
        // Tampering changes the digest.
        lines[1] = "entry=alpha|storage-service|9.9.9|serviceos.bootstore.v1|m/alpha.manifest|tool|Alpha package";
        assert_ne!(first, compute_feed_digest(KEY_A, &lines, count));
    }

    #[test]
    fn signature_lines_are_excluded_from_canonical_form() {
        let feed = signed_feed(KEY_A);
        assert_eq!(
            parse_feed_signature(&feed).map(|signature| signature.digest),
            {
                let mut lines = [""; MAX_CANON_LINES];
                let count = canonical_lines(sample_feed(), &mut lines);
                Some(compute_feed_digest(KEY_A, &lines, count))
            }
        );
        assert_eq!(
            parse_feed_signature("no signatures here"),
            None,
            "unsigned feed parses to none"
        );
    }

    #[test]
    fn tampered_feed_is_rejected() {
        let entry = source_with(&[("k1", KEY_A)], 0);
        let good = signed_feed(KEY_A);
        assert_eq!(
            verify_signed_feed(&good, Some(&entry), 100),
            FeedVerdict::Accepted
        );

        // Flip a manifest field without updating the digest.
        let tampered = good.replace("1.2.0", "1.2.1");
        assert_eq!(
            verify_signed_feed(&tampered, Some(&entry), 100),
            FeedVerdict::RejectedTampered
        );

        // Corrupt the recorded digest itself (still parseable hex).
        let good_digest = parse_feed_signature(&good)
            .map(|signature| signature.digest)
            .unwrap_or(0);
        let corrupted = good.replace(
            &format!("{:016x}", good_digest),
            &format!("{:016x}", good_digest ^ 1),
        );
        assert_eq!(
            verify_signed_feed(&corrupted, Some(&entry), 100),
            FeedVerdict::RejectedTampered
        );

        // Signed with a different key than pinned.
        let wrong_key = signed_feed(KEY_B);
        assert_eq!(
            verify_signed_feed(&wrong_key, Some(&entry), 100),
            FeedVerdict::RejectedTampered
        );
    }

    #[test]
    fn rotation_window_gates_retired_keys() {
        let mut entry = source_with(&[("k1", KEY_A)], 500);
        let _ = entry.enroll("k2", KEY_B);
        let _ = entry.rotate_active("k2", 1_000);
        assert_eq!(
            entry.find_key("k1").map(|key| key.state),
            Some(KeyState::Retired)
        );
        assert_eq!(
            entry.find_key("k1").map(|key| key.retired_tick),
            Some(1_000)
        );

        // Old key signs still valid inside the window...
        let old_sig = signed_feed(KEY_A);
        assert_eq!(
            verify_signed_feed(&old_sig, Some(&entry), 1_400),
            FeedVerdict::AcceptedRetired
        );
        // ...but hard-fails once the window closes.
        assert_eq!(
            verify_signed_feed(&old_sig, Some(&entry), 1_501),
            FeedVerdict::RejectedStaleSignature
        );

        // New key verifies as active across the whole span.
        let new_sig = signed_feed_with_id("k2", KEY_B);
        assert_eq!(
            verify_signed_feed(&new_sig, Some(&entry), 5_000),
            FeedVerdict::Accepted
        );

        // A zero window disables old-key acceptance entirely.
        entry.accept_retired_ticks = 0;
        assert_eq!(
            verify_signed_feed(&old_sig, Some(&entry), 1_001),
            FeedVerdict::RejectedStaleSignature
        );
    }

    #[test]
    fn rotation_errors_are_distinct() {
        let mut entry = source_with(&[("k1", KEY_A)], 0);
        assert_eq!(
            entry.rotate_active("missing", 10),
            Err(KeystoreError::UnknownKey)
        );
        assert_eq!(
            entry.rotate_active("k1", 10),
            Err(KeystoreError::SameKeyActive)
        );
        let _ = entry.enroll("k2", KEY_B);
        assert_eq!(entry.rotate_active("k2", 20), Ok(()));
        assert_eq!(
            entry.rotate_active("k2", 30),
            Err(KeystoreError::SameKeyActive)
        );
        assert_eq!(entry.enroll("k2", KEY_A), Err(KeystoreError::DuplicateKey));
        assert_eq!(entry.enroll("bad", "zz"), Err(KeystoreError::InvalidKeyHex));
    }

    #[test]
    fn missing_key_gates_to_untrusted_without_hard_failure() {
        let signed = signed_feed(KEY_A);
        // Nothing pinned: honor-system unchanged.
        assert_eq!(
            verify_signed_feed(&signed, None, 0),
            FeedVerdict::UnsignedNoKeysPinned
        );
        assert_eq!(
            verify_signed_feed(sample_feed(), None, 0),
            FeedVerdict::UnsignedNoKeysPinned
        );

        // Keys pinned elsewhere, unknown signer: operator accept required.
        let entry = source_with(&[("k1", KEY_A)], 0);
        assert_eq!(
            verify_signed_feed(&signed, Some(&entry), 0),
            FeedVerdict::Accepted
        );
        let stranger = signed_feed_with_id("stranger", KEY_A);
        assert_eq!(
            verify_signed_feed(&stranger, Some(&entry), 0),
            FeedVerdict::UnknownKey
        );

        // Keys pinned but feed unsigned: hard fail (trust-root upgrade).
        let unsigned_entry = source_with(&[("k1", KEY_A)], 0);
        assert_eq!(
            verify_signed_feed(sample_feed(), Some(&unsigned_entry), 0),
            FeedVerdict::RejectedUnsignedRequired
        );
    }

    fn signed_feed_with_id(key_id: &str, key_hex: &str) -> String {
        let mut text = String::new();
        sign_feed_text(sample_feed(), key_id, key_hex, &mut text).expect("sign");
        text
    }

    #[test]
    fn keystore_roundtrip_preserves_verdicts() {
        let mut keystore = Keystore::empty();
        let entry = keystore.ensure_source("extra").expect("source");
        let _ = entry.enroll("k1", KEY_A);
        let _ = entry.enroll("k2", KEY_B);
        entry.accept_retired_ticks = 500;
        let _ = entry.rotate_active("k2", 1_000);

        let mut text = String::new();
        serialize_keystore(&keystore, &mut text);
        let parsed = parse_keystore(&text);
        assert_eq!(parsed.source_count, 1);
        let parsed_entry = parsed.source_keys("extra").expect("entry");
        assert_eq!(parsed_entry.key_count, 2);
        assert_eq!(
            parsed_entry.find_key("k2").map(|key| key.state),
            Some(KeyState::Active)
        );
        assert_eq!(
            parsed_entry.find_key("k1").map(|key| key.retired_tick),
            Some(1_000)
        );
        assert_eq!(parsed_entry.accept_retired_ticks, 500);

        // Verdicts survive the roundtrip.
        let new_sig = signed_feed_with_id("k2", KEY_B);
        let old_sig = signed_feed(KEY_A);
        assert_eq!(
            verify_signed_feed(&new_sig, parsed.source_keys("extra"), 2_000),
            FeedVerdict::Accepted
        );
        assert_eq!(
            verify_signed_feed(&old_sig, parsed.source_keys("extra"), 1_200),
            FeedVerdict::AcceptedRetired
        );
        assert_eq!(
            verify_signed_feed(&old_sig, parsed.source_keys("extra"), 2_000),
            FeedVerdict::RejectedStaleSignature
        );
    }

    #[test]
    fn reject_journal_wraps_and_reports_newest_first() {
        let mut journal = RejectJournal::empty();
        for index in 0u64..REJECT_RECORDS_MAX as u64 + 3 {
            journal.record("extra", REJECT_TAMPERED, index * 7, index * 11);
        }
        assert_eq!(journal.total, (REJECT_RECORDS_MAX + 3) as u64);
        let mut out = [RejectRecord::empty(); REJECT_RECORDS_MAX];
        let written = journal.recent(&mut out);
        assert_eq!(written, REJECT_RECORDS_MAX);
        assert_eq!(out[0].digest, ((REJECT_RECORDS_MAX + 2) as u64) * 7);
        assert_eq!(out[REJECT_RECORDS_MAX - 1].digest, 3 * 7);

        let mut text = String::new();
        journal.serialize(&mut text);
        let parsed = RejectJournal::parse(&text);
        assert_eq!(parsed.total, journal.total);
        let mut reparsed = [RejectRecord::empty(); REJECT_RECORDS_MAX];
        assert_eq!(parsed.recent(&mut reparsed), written);
        assert_eq!(reparsed[0].digest, out[0].digest);
        assert_eq!(reparsed[0].reason, REJECT_TAMPERED);
    }

    #[test]
    fn hex_helpers_reject_malformed_input() {
        assert_eq!(parse_hex_u64(""), None);
        assert_eq!(parse_hex_u64("00000000000000000"), None);
        assert_eq!(parse_hex_u64("deadbeef"), Some(0xdeadbeef));
        assert_eq!(decode_key_hex("abc"), None);
        assert_eq!(decode_key_hex("zz"), None);
        assert_eq!(
            decode_key_hex(KEY_A),
            Some([
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff
            ])
        );
    }

    fn pubkey_hex_for(byte: u8) -> String {
        let mut seed = [byte; 32];
        seed[31] = byte.wrapping_mul(7);
        hex_of(&serviceos_crypto::ed25519::public_key(&seed))
    }

    #[test]
    fn auto_key_id_is_stable_prefixed_and_unique_enough() {
        let (a, len_a) = auto_key_id(b"\x01\x02\x03");
        let (b, len_b) = auto_key_id(b"\x01\x02\x03");
        assert_eq!((&a[..len_a], &b[..len_b]), (&b[..len_b], &a[..len_a]));
        assert_eq!(&a[..2], b"k-");
        assert_eq!(len_a, 18);
        let (c, _) = auto_key_id(b"different");
        assert_ne!(&a[2..len_a], &c[2..18]);
    }

    #[test]
    fn encode_hex_emits_lowercase_pairs() {
        let mut out = [0u8; 8];
        assert_eq!(encode_hex(&[0xde, 0xad, 0xbe, 0xef], &mut out), 8);
        assert_eq!(&out, b"deadbeef");
        let mut small = [0u8; 3];
        // Odd-length buffers cap at whole pairs.
        assert_eq!(encode_hex(&[0xaa, 0xbb], &mut small), 2);
        assert_eq!(&small[..2], b"aa");
    }

    #[test]
    fn generated_identity_varies_and_enrolls_as_valid_pubkey() {
        let first = derive_generated_identity(b"boot", 7, 1, 0);
        let second = derive_generated_identity(b"boot", 7, 2, 0);
        assert_ne!(first.seed, second.seed);
        let mut entry = SourceKeys::empty();
        let pub_binding = first.public_hex();
        let pub_text = core::str::from_utf8(&pub_binding).unwrap();
        let id_text = first.id_str().unwrap();
        assert_eq!(entry.enroll_ed25519(id_text, pub_text), Ok(()));
        assert_eq!(
            entry.find_key(id_text).map(|key| key.state),
            Some(KeyState::Active)
        );
    }

    #[test]
    fn rotate_source_promotes_latest_retired_standby() {
        let mut entry = SourceKeys::empty();
        entry.enroll_ed25519("k1", &pubkey_hex_for(1)).unwrap();
        entry.enroll_ed25519("k2", &pubkey_hex_for(2)).unwrap();
        entry.enroll_ed25519("k3", &pubkey_hex_for(3)).unwrap();
        let slot = entry.rotate_source(500).unwrap();
        assert_eq!(entry.keys[slot].key_id.as_str(), "k3");
        assert_eq!(entry.keys[slot].state, KeyState::Active);
        assert_eq!(entry.keys[0].state, KeyState::Retired);
        assert_eq!(entry.keys[0].retired_tick, 500);
        // k2 stays retired with no tick stamped by THIS rotation.
        assert_eq!(entry.keys[1].retired_tick, 0);
    }

    #[test]
    fn rotate_source_fails_without_any_standby() {
        let mut entry = SourceKeys::empty();
        assert!(matches!(
            entry.rotate_source(1),
            Err(KeystoreError::UnknownKey)
        ));
        entry.enroll_ed25519("only", &pubkey_hex_for(4)).unwrap();
        assert!(matches!(
            entry.rotate_source(2),
            Err(KeystoreError::UnknownKey)
        ));
    }
}

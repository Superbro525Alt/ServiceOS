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
pub const KEY_HEX_MAX: usize = 32;
pub const SOURCE_NAME_MAX: usize = 32;
pub const MAX_KEYS_PER_SOURCE: usize = 4;
pub const MAX_SIGNED_SOURCES: usize = 4;
pub const MAX_CANON_LINES: usize = 256;
pub const REJECT_RECORDS_MAX: usize = 8;

pub const SIG_KEY_PREFIX: &str = "sig-key=";
pub const SIG_DIGEST_PREFIX: &str = "sig-digest=";

/// Rejection reason words persisted in the feed-reject journal.
pub const REJECT_UNSIGNED_REQUIRED: u64 = 1;
pub const REJECT_TAMPERED: u64 = 2;
pub const REJECT_STALE_SIGNATURE: u64 = 3;
pub const REJECT_UNKNOWN_KEY: u64 = 4;

pub fn reject_reason(verdict: FeedVerdict) -> u64 {
    match verdict {
        FeedVerdict::RejectedUnsignedRequired => REJECT_UNSIGNED_REQUIRED,
        FeedVerdict::RejectedTampered => REJECT_TAMPERED,
        FeedVerdict::RejectedStaleSignature => REJECT_STALE_SIGNATURE,
        FeedVerdict::UnknownKey => REJECT_UNKNOWN_KEY,
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
pub struct TrustedKey {
    pub key_id: FixedText<KEY_ID_MAX>,
    pub key_hex: FixedText<KEY_HEX_MAX>,
    pub state: KeyState,
    pub retired_tick: u64,
}

impl TrustedKey {
    pub const fn empty() -> Self {
        Self {
            key_id: FixedText::empty(),
            key_hex: FixedText::empty(),
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
        let has_active = self
            .keys[..self.key_count]
            .iter()
            .any(|key| key.state == KeyState::Active);
        let slot = match self.keys[..self.key_count].iter_mut().find(|key| key.key_id.is_empty()) {
            Some(slot) => slot,
            None if self.key_count < MAX_KEYS_PER_SOURCE => {
                self.key_count += 1;
                &mut self.keys[self.key_count - 1]
            }
            None => return Err(KeystoreError::SourceFull),
        };
        slot.key_id.set(key_id);
        slot.key_hex.set(key_hex);
        slot.state = if has_active { KeyState::Retired } else { KeyState::Active };
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
        let Some(current_active) = self
            .keys[..self.key_count]
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
        let promoted = self
            .keys[..self.key_count]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FeedSignature {
    pub key_id: FixedText<KEY_ID_MAX>,
    pub digest: u64,
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
    if value.is_empty() || value.len() > KEY_HEX_MAX || value.len() % 2 != 0 {
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

fn is_signature_line(line: &str) -> bool {
    line.starts_with(SIG_KEY_PREFIX) || line.starts_with(SIG_DIGEST_PREFIX)
}

/// Extract the trailing signature lines from a feed, if present.
pub fn parse_feed_signature(feed: &str) -> Option<FeedSignature> {
    let mut key_id = FixedText::<KEY_ID_MAX>::empty();
    let mut digest: Option<u64> = None;
    for line in feed.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(value) = line.strip_prefix(SIG_KEY_PREFIX) {
            let _ = key_id.set(value.trim());
        } else if let Some(value) = line.strip_prefix(SIG_DIGEST_PREFIX) {
            digest = parse_hex_u64(value.trim());
        }
    }
    if key_id.is_empty() || digest.is_none() {
        return None;
    }
    Some(FeedSignature {
        key_id,
        digest: digest.unwrap_or(0),
    })
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
}

/// Verify a feed against the keys pinned for its source (`None` when the
/// keystore holds no entry for the source).
pub fn verify_signed_feed(feed: &str, entry: Option<&SourceKeys>, now: u64) -> FeedVerdict {
    let entry = match entry {
        Some(entry) if entry.key_count > 0 => entry,
        _ => return FeedVerdict::UnsignedNoKeysPinned,
    };
    let Some(signature) = parse_feed_signature(feed) else {
        return FeedVerdict::RejectedUnsignedRequired;
    };
    let Some(key) = entry.find_key(signature.key_id.as_str()) else {
        return FeedVerdict::UnknownKey;
    };
    let accepted = match key.state {
        KeyState::Active => true,
        KeyState::Retired => entry.retired_within_window(key, now),
    };
    if !accepted {
        return FeedVerdict::RejectedStaleSignature;
    }
    let mut lines = [""; MAX_CANON_LINES];
    let count = canonical_lines(feed, &mut lines);
    if compute_feed_digest(key.key_hex.as_str(), &lines, count) == signature.digest {
        if key.state == KeyState::Active {
            FeedVerdict::Accepted
        } else {
            FeedVerdict::AcceptedRetired
        }
    } else {
        FeedVerdict::RejectedTampered
    }
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
            let tick = parts.next().and_then(|value| value.parse::<u64>().ok()).unwrap_or(0);
            journal.record(source, reason, digest, tick);
        }
        // Rebuild recency from tick order: `recent()` derives order from the
        // write cursor, which a serialized dump cannot preserve.
        let mut kept = [RejectRecord::empty(); REJECT_RECORDS_MAX];
        let mut count = 0usize;
        for record in journal.records.iter().copied().filter(|record| record.occupied) {
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
    assert_eq!(key_state_from_word(key_state_word(KeyState::Active, 0)), (KeyState::Active, 0));
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
                current = keystore
                    .sources[..keystore.source_count]
                    .iter()
                    .position(|entry| entry.source.as_str() == name);
                if current.is_none() && keystore.ensure_source(name).is_ok() {
                    current = Some(keystore.source_count - 1);
                }
                if let (Some(index), Ok(ticks)) =
                    (current, window.parse::<u64>())
                {
                    keystore.sources[index].accept_retired_ticks = ticks;
                }
            }
            continue;
        }
        let Some(payload) = line.strip_prefix("key ") else {
            continue;
        };
        let mut parts = payload.split(' ');
        let (Some(key_id), Some(key_hex), Some(state_word)) =
            (parts.next(), parts.next(), parts.next().and_then(|v| v.parse::<u64>().ok()))
        else {
            continue;
        };
        let Some(index) = current else { continue };
        let entry = &mut keystore.sources[index];
        if decode_key_hex(key_hex).is_none() {
            continue;
        }
        if key_id.is_empty() || key_id.len() > KEY_ID_MAX || entry.key_count == MAX_KEYS_PER_SOURCE
        {
            continue;
        }
        let slot = &mut entry.keys[entry.key_count];
        slot.key_id.set(key_id);
        slot.key_hex.set(key_hex);
        let (state, retired_tick) = key_state_from_word(state_word);
        slot.state = state;
        slot.retired_tick = retired_tick;
        entry.key_count += 1;
    }
    keystore
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(parse_feed_signature(&feed).map(|signature| signature.digest), {
            let mut lines = [""; MAX_CANON_LINES];
            let count = canonical_lines(sample_feed(), &mut lines);
            Some(compute_feed_digest(KEY_A, &lines, count))
        });
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
        assert_eq!(verify_signed_feed(&good, Some(&entry), 100), FeedVerdict::Accepted);

        // Flip a manifest field without updating the digest.
        let tampered = good.replace("1.2.0", "1.2.1");
        assert_eq!(
            verify_signed_feed(&tampered, Some(&entry), 100),
            FeedVerdict::RejectedTampered
        );

        // Corrupt the recorded digest itself (still parseable hex).
        let good_digest = parse_feed_signature(&good).map(|signature| signature.digest).unwrap_or(0);
        let corrupted = good.replace(&format!("{:016x}", good_digest), &format!("{:016x}", good_digest ^ 1));
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
        assert_eq!(entry.find_key("k1").map(|key| key.retired_tick), Some(1_000));

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
        assert_eq!(verify_signed_feed(&new_sig, Some(&entry), 5_000), FeedVerdict::Accepted);

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
        assert_eq!(entry.rotate_active("missing", 10), Err(KeystoreError::UnknownKey));
        assert_eq!(entry.rotate_active("k1", 10), Err(KeystoreError::SameKeyActive));
        let _ = entry.enroll("k2", KEY_B);
        assert_eq!(entry.rotate_active("k2", 20), Ok(()));
        assert_eq!(entry.rotate_active("k2", 30), Err(KeystoreError::SameKeyActive));
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
        assert_eq!(parsed_entry.find_key("k2").map(|key| key.state), Some(KeyState::Active));
        assert_eq!(parsed_entry.find_key("k1").map(|key| key.retired_tick), Some(1_000));
        assert_eq!(parsed_entry.accept_retired_ticks, 500);

        // Verdicts survive the roundtrip.
        let new_sig = signed_feed_with_id("k2", KEY_B);
        let old_sig = signed_feed(KEY_A);
        assert_eq!(verify_signed_feed(&new_sig, parsed.source_keys("extra"), 2_000), FeedVerdict::Accepted);
        assert_eq!(verify_signed_feed(&old_sig, parsed.source_keys("extra"), 1_200), FeedVerdict::AcceptedRetired);
        assert_eq!(verify_signed_feed(&old_sig, parsed.source_keys("extra"), 2_000), FeedVerdict::RejectedStaleSignature);
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
        assert_eq!(decode_key_hex(KEY_A), Some([0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]));
    }
}

//! Host-only helpers for tooling and test fixtures. Requires the
//! `std-helper` feature (not enabled for OS targets).

use crate::ed25519;

/// A generated signing identity: 32-byte seed (keep secret) and the
/// matching compressed public key.
pub struct KeyPair {
    pub seed: [u8; 32],
    pub public: [u8; 32],
}

impl KeyPair {
    /// Generate a keypair from the host CSPRNG (/dev/urandom).
    pub fn generate() -> std::io::Result<KeyPair> {
        let mut seed = [0u8; 32];
        fill_random(&mut seed)?;
        Ok(KeyPair::from_seed(seed))
    }

    pub fn from_seed(seed: [u8; 32]) -> KeyPair {
        KeyPair {
            public: ed25519::public_key(&seed),
            seed,
        }
    }

    /// Sign raw bytes.
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        ed25519::sign(&self.seed, message)
    }
}

/// Build a signed fixture feed: `content_lines` are sorted exactly like
/// package-service's canonical form (so the signature covers the same
/// stream a verifier rebuilds), then `sig-alg=ed25519`, `sig-key=<key_id>`,
/// and `sig-sig=<128 hex>` trailers are appended.
pub fn sign_feed_fixture(
    key: &KeyPair,
    key_id: &str,
    content_lines: &[&str],
) -> std::io::Result<String> {
    let mut sorted: Vec<&str> = content_lines.to_vec();
    sorted.sort_unstable();
    let mut message = Vec::new();
    for line in &sorted {
        message.extend_from_slice(line.as_bytes());
        message.push(b'\n');
    }
    let signature = key.sign(&message);
    let mut feed = String::new();
    for line in sorted {
        feed.push_str(line);
        feed.push('\n');
    }
    feed.push_str("sig-alg=ed25519\n");
    feed.push_str("sig-key=");
    feed.push_str(key_id);
    feed.push('\n');
    feed.push_str("sig-sig=");
    for byte in signature {
        use core::fmt::Write as _;
        let _ = write!(feed, "{:02x}", byte);
    }
    feed.push('\n');
    Ok(feed)
}

fn fill_random(buf: &mut [u8]) -> std::io::Result<()> {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom")?;
    f.read_exact(buf)
}

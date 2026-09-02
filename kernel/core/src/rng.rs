//! Kernel random-number subsystem: a jitter-conditioned ChaCha20 DRBG.
//!
//! Design (honest classification):
//! - Primary seed material comes from a hardware entropy source when the
//!   platform provides one (virtio-rng on x86 qemu-virtio; see the platform
//!   probe that hands an [`EntropySource`] to [`initialize`]). Reseeding
//!   pulls fresh bytes from the same source when it is still attached.
//! - Jitter conditioning samples the monotonic timer across a short busy
//!   loop and folds the observed deltas into the seed material. This is
//!   NOT a certified TRNG and on platforms without a hardware source the
//!   output is only integrity-grade-plus (unpredictable against off-host
//!   observers, not a cryptographic root of trust). The boot log line
//!   names the seed source so operators can tell which class they got.
//! - The generator itself is a ChaCha20 stream (shared `serviceos-crypto`
//!   primitive) keyed by SHA-512-extracted seed material; the stream
//!   counter is a 64-bit block index so draws never repeat output blocks.
//!
//! Reseed policy: after [`RESEED_DRAWS`] fills the DRBG mixes fresh jitter
//! plus (when attached) up to [`RESEED_SOURCE_BYTES`] new hardware bytes.

use core::sync::atomic::{AtomicU64, Ordering};

use alloc::sync::Arc;
use serviceos_crypto::{chacha20, sha512};
use spin::Mutex;

/// Largest single RngRequest the kernel will fill (syscall contract bound).
pub const MAX_REQUEST_BYTES: usize = 4096;
/// Bytes requested from the hardware source for the initial seed.
const SEED_SOURCE_BYTES: usize = 64;
/// Jitter samples folded into every seed/reseed.
const JITTER_SAMPLES: usize = 32;
/// Reseed after this many `fill` calls.
const RESEED_DRAWS: u32 = 256;
/// Fresh hardware bytes mixed into each reseed.
const RESEED_SOURCE_BYTES: usize = 32;

/// A hardware entropy source handed to the kernel by the platform layer
/// (virtio-rng probe on x86; absent on platforms without such a device).
/// The source is shared kernel-wide; implementations provide their own
/// interior mutability (the virtio probe wraps its device in a spin lock).
pub trait EntropySource: Send + Sync {
    /// Poll the device for entropy into `dst`. Returns the number of bytes
    /// actually written, or None when the device failed/withdrew.
    fn request_entropy(&self, dst: &mut [u8]) -> Option<usize>;
}

/// Where the current DRBG seed came from (boot-log + caller diagnostics).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeedSource {
    /// Hardware entropy source present and responsive.
    Hardware,
    /// No hardware source: jitter-only conditioning.
    Jitter,
}

impl SeedSource {
    pub fn as_str(self) -> &'static str {
        match self {
            SeedSource::Hardware => "virtio-rng",
            SeedSource::Jitter => "jitter",
        }
    }
}

/// Result of [`initialize`] for the platform boot log line.
#[derive(Clone, Copy, Debug)]
pub struct SeedSummary {
    pub source: SeedSource,
    /// Hardware bytes mixed into the initial seed (0 when jitter-only).
    pub hardware_bytes: usize,
}

struct Drbg {
    key: [u8; 32],
    /// 64-bit ChaCha20 block index (nonce carries the high 64 bits,
    /// counter the low 32) so output blocks never repeat for one key.
    block_index: u64,
    draws_since_reseed: u32,
    source: Option<Arc<dyn EntropySource>>,
}

impl Drbg {
    fn new(material: &[u8], source: Option<Arc<dyn EntropySource>>) -> Self {
        let digest = sha512::digest(&[b"serviceos-kernel-rng-seed", material]);
        let mut key = [0u8; 32];
        key.copy_from_slice(&digest[..32]);
        Self {
            key,
            block_index: 0,
            draws_since_reseed: 0,
            source,
        }
    }

    /// Extractor-refresh: compress material into a new key, keeping the
    /// old key folded in so a degenerate reseed cannot weaken the stream.
    fn reseed(&mut self) {
        let mut material = [0u8; 64];
        material[..32].copy_from_slice(&self.key);
        let jitter = collect_jitter();
        let tail = 32 + jitter.len();
        material[32..tail].copy_from_slice(&jitter);
        let mut hardware = [0u8; RESEED_SOURCE_BYTES];
        let mut fetched = 0usize;
        if let Some(source) = self.source.as_ref() {
            if let Some(n) = source.request_entropy(&mut hardware) {
                fetched = n.min(RESEED_SOURCE_BYTES);
            }
        }
        let digest = sha512::digest(&[
            b"serviceos-kernel-rng-reseed",
            &material[..tail],
            &hardware[..fetched],
        ]);
        self.key.copy_from_slice(&digest[..32]);
        self.draws_since_reseed = 0;
    }

    fn fill(&mut self, dst: &mut [u8]) {
        if self.draws_since_reseed >= RESEED_DRAWS {
            self.reseed();
        }
        let mut offset = 0;
        while offset < dst.len() {
            let mut nonce = [0u8; 12];
            nonce[..8].copy_from_slice(&self.block_index.to_le_bytes());
            let counter = (self.block_index & 0xffff_ffff) as u32;
            let block = chacha20::block(&self.key, counter, &nonce);
            let take = (dst.len() - offset).min(block.len());
            dst[offset..offset + take].copy_from_slice(&block[..take]);
            offset += take;
            self.block_index += 1;
        }
        self.draws_since_reseed += 1;
    }
}

static RNG: Mutex<Option<Drbg>> = Mutex::new(None);
static DRAW_BYTES: AtomicU64 = AtomicU64::new(0);

/// Sample the monotonic timer across a busy loop and return the observed
/// deltas (weak but real conditioning; the honest classification lives in
/// the module docs).
fn collect_jitter() -> [u8; JITTER_SAMPLES] {
    let mut deltas = [0u8; JITTER_SAMPLES];
    let Some(manager) = crate::time::manager() else {
        return deltas;
    };
    let mut previous = manager.now().0;
    for slot in deltas.iter_mut() {
        let mut sink = 0u64;
        for _ in 0..4 {
            sink = sink.wrapping_add(manager.now().0);
        }
        core::hint::black_box(sink);
        let now = manager.now().0;
        *slot = now.wrapping_sub(previous) as u8;
        previous = now;
    }
    deltas
}

/// Seed the DRBG once at boot. `source` is the platform's hardware entropy
/// source when one probed successfully. Repeated calls are ignored (the
/// first seed wins); consumers draw via the RngRequest syscall.
pub fn initialize(source: Option<Arc<dyn EntropySource>>) -> SeedSummary {
    let mut hardware_bytes = 0usize;
    let mut hardware = [0u8; SEED_SOURCE_BYTES];
    if let Some(backend) = source.as_ref() {
        if let Some(n) = backend.request_entropy(&mut hardware) {
            hardware_bytes = n.min(SEED_SOURCE_BYTES);
        }
    }

    let mut material = [0u8; 128];
    material[..hardware_bytes].copy_from_slice(&hardware[..hardware_bytes]);
    let jitter = collect_jitter();
    material[64..64 + jitter.len()].copy_from_slice(&jitter);
    let drbg = Drbg::new(&material[..64 + jitter.len()], source);

    let mut guard = RNG.lock();
    let summary = SeedSummary {
        source: if hardware_bytes > 0 {
            SeedSource::Hardware
        } else {
            SeedSource::Jitter
        },
        hardware_bytes,
    };
    if guard.is_none() {
        *guard = Some(drbg);
    }
    summary
}

/// Draw kernel DRBG bytes. Returns false when the subsystem was never
/// seeded (callers fall back to their documented substitutes).
pub fn fill(dst: &mut [u8]) -> bool {
    if dst.is_empty() {
        return true;
    }
    let mut guard = RNG.lock();
    match guard.as_mut() {
        Some(drbg) => {
            drbg.fill(dst);
            DRAW_BYTES.fetch_add(dst.len() as u64, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

/// Total bytes drawn since boot (diagnostics).
pub fn drawn_bytes() -> u64 {
    DRAW_BYTES.load(Ordering::Relaxed)
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// Test-only: replace the DRBG with one keyed by `material` and no
    /// hardware source, so host tests can assert determinism.
    pub fn install_for_test(material: &[u8]) {
        *RNG.lock() = Some(Drbg::new(material, None));
    }

    /// Test-only: clear the DRBG (NotInitialized path).
    pub fn clear_for_test() {
        *RNG.lock() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_seed_deterministic() {
        crate::rng::testing::install_for_test(b"fixed-seed-material");
        let mut a = [0u8; 64];
        let mut b = [0u8; 64];
        assert!(fill(&mut a));
        crate::rng::testing::install_for_test(b"fixed-seed-material");
        assert!(fill(&mut b));
        assert_eq!(a, b);
        crate::rng::testing::clear_for_test();
    }

    #[test]
    fn distinct_draws_differ() {
        crate::rng::testing::install_for_test(b"distinct-draw-seed");
        let mut a = [0u8; 64];
        let mut b = [0u8; 64];
        assert!(fill(&mut a));
        assert!(fill(&mut b));
        assert_ne!(a, b);
        crate::rng::testing::clear_for_test();
    }

    #[test]
    fn unseeded_fills_fail() {
        crate::rng::testing::clear_for_test();
        let mut out = [0u8; 16];
        assert!(!fill(&mut out));
        assert!(fill(&mut []));
    }

    #[test]
    fn reseed_keeps_stream_alive_and_changes_output() {
        crate::rng::testing::install_for_test(b"reseed-path-seed");
        let mut before = [0u8; 64];
        assert!(fill(&mut before));
        // Force a reseed by exceeding the draw budget with tiny fills.
        let mut chunk = [0u8; 8];
        for _ in 0..RESEED_DRAWS {
            assert!(fill(&mut chunk));
        }
        let mut after = [0u8; 64];
        assert!(fill(&mut after));
        assert_ne!(before, after);
        crate::rng::testing::clear_for_test();
    }

    #[test]
    fn multi_block_draw_spans_block_index() {
        crate::rng::testing::install_for_test(b"multi-block-seed");
        let mut out = [0u8; 300]; // spans >4 ChaCha20 blocks
        assert!(fill(&mut out));
        assert!(out.iter().any(|b| *b != 0));
        crate::rng::testing::clear_for_test();
    }
}

//! TTL-honoring positive/negative resolver cache for A / AAAA / CNAME
//! records. Pure logic over an injected millisecond clock so expiry is
//! host-unit-testable.

use crate::consts::{
    MAX_CACHED_A_RECORDS, MAX_RESOLVER_CACHE_ENTRIES, NEGATIVE_TTL_MS_CAP,
};
use crate::dnsmsg::{NameBuf, QueryType};

/// Result of a cache probe.
#[derive(Clone, Copy, Debug)]
pub(crate) enum CacheLookup {
    /// Positive hit carrying up to four A addresses (host order BE words).
    HitA([u32; MAX_CACHED_A_RECORDS], usize),
    /// Positive hit carrying one AAAA address.
    HitAaaa([u8; 16]),
    /// Negative hit: name known not to resolve.
    Negative,
    Miss,
}

/// Slot key: either a typed record set or a CNAME mapping (a CNAME entry for
/// a name coexists with A/AAAA entries for other names).
#[derive(Clone, Copy, Eq, PartialEq)]
enum Key {
    Typed(QueryType),
    Cname,
}

#[derive(Clone, Copy)]
struct Entry {
    active: bool,
    key: Option<Key>,
    negative: bool,
    name: NameBuf,
    /// A: up to MAX_CACHED_A_RECORDS big-endian u32s (count*4 bytes).
    /// AAAA: one 16-byte address.
    /// CNAME: target name in dotted text form.
    data: [u8; MAX_CACHED_A_RECORDS * 4],
    data_len: usize,
    expires_ms: u64,
}

impl Entry {
    const fn empty() -> Self {
        Self {
            active: false,
            key: None,
            negative: false,
            name: NameBuf::empty(),
            data: [0; MAX_CACHED_A_RECORDS * 4],
            data_len: 0,
            expires_ms: 0,
        }
    }

    fn expired(&self, now_ms: u64) -> bool {
        now_ms >= self.expires_ms
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ResolverCache {
    entries: [Entry; MAX_RESOLVER_CACHE_ENTRIES],
    pub(crate) hits: u64,
    pub(crate) misses: u64,
    pub(crate) negative_hits: u64,
}

impl ResolverCache {
    pub(crate) const fn new() -> Self {
        Self {
            entries: [const { Entry::empty() }; MAX_RESOLVER_CACHE_ENTRIES],
            hits: 0,
            misses: 0,
            negative_hits: 0,
        }
    }

    fn find_slot(&self, name: &NameBuf, key: Key) -> Option<usize> {
        self.entries.iter().position(|entry| {
            entry.active
                && entry.key == Some(key)
                && entry.name.len == name.len
                && entry.name.bytes[..name.len] == name.bytes[..name.len]
        })
    }

    fn free_slot(&mut self) -> Option<usize> {
        self.entries.iter().position(|entry| !entry.active)
    }

    fn evict_oldest(&mut self) -> usize {
        let index = self
            .entries
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| entry.expires_ms)
            .map(|(index, _)| index)
            .unwrap_or(0);
        self.entries[index] = Entry::empty();
        index
    }

    pub(crate) fn lookup(&mut self, name: &str, qtype: QueryType, now_ms: u64) -> CacheLookup {
        let Some(encoded) = NameBuf::parse(name) else {
            return CacheLookup::Miss;
        };
        let Some(index) = self.find_slot(&encoded, Key::Typed(qtype)) else {
            self.misses = self.misses.saturating_add(1);
            return CacheLookup::Miss;
        };
        if self.entries[index].expired(now_ms) {
            self.entries[index] = Entry::empty();
            self.misses = self.misses.saturating_add(1);
            return CacheLookup::Miss;
        }
        let entry = &self.entries[index];
        if entry.negative {
            self.negative_hits = self.negative_hits.saturating_add(1);
            return CacheLookup::Negative;
        }
        match qtype {
            QueryType::A => {
                let count = entry.data_len / 4;
                debug_assert_eq!(entry.data_len % 4, 0);
                let mut addresses = [0u32; MAX_CACHED_A_RECORDS];
                for (index, address) in addresses.iter_mut().enumerate().take(count) {
                    let start = index * 4;
                    *address = u32::from_be_bytes([
                        entry.data[start],
                        entry.data[start + 1],
                        entry.data[start + 2],
                        entry.data[start + 3],
                    ]);
                }
                self.hits = self.hits.saturating_add(1);
                CacheLookup::HitA(addresses, count)
            }
            QueryType::Aaaa => {
                let mut address = [0u8; 16];
                address.copy_from_slice(&entry.data[..16]);
                self.hits = self.hits.saturating_add(1);
                CacheLookup::HitAaaa(address)
            }
            QueryType::Txt => CacheLookup::Miss,
        }
    }

    /// CNAME probe separate from typed lookups (a CNAME entry for one name
    /// coexists with A/AAAA entries for other names).
    pub(crate) fn lookup_cname(&mut self, name: &str, now_ms: u64) -> Option<NameBuf> {
        let encoded = NameBuf::parse(name)?;
        let index = self.find_slot(&encoded, Key::Cname)?;
        let entry = &self.entries[index];
        if entry.negative || entry.expired(now_ms) {
            if entry.expired(now_ms) {
                self.entries[index] = Entry::empty();
            }
            return None;
        }
        let mut target = NameBuf::empty();
        if entry.data_len > target.bytes.len() {
            return None;
        }
        target.bytes[..entry.data_len].copy_from_slice(&entry.data[..entry.data_len]);
        target.len = entry.data_len;
        self.hits = self.hits.saturating_add(1);
        Some(target)
    }

    pub(crate) fn store_positive_a(
        &mut self,
        name: &str,
        addresses: &[u32],
        ttl_ms: u64,
        now_ms: u64,
    ) {
        let Some(encoded) = NameBuf::parse(name) else {
            return;
        };
        let count = addresses.len().min(MAX_CACHED_A_RECORDS);
        if count == 0 {
            return;
        }
        let index = self.upsert_slot(&encoded, Key::Typed(QueryType::A), ttl_ms, now_ms, false);
        let entry = &mut self.entries[index];
        entry.data.fill(0);
        for (offset, address) in addresses.iter().take(count).enumerate() {
            let start = offset * 4;
            entry.data[start..start + 4].copy_from_slice(&address.to_be_bytes());
        }
        entry.data_len = count * 4;
    }

    pub(crate) fn store_positive_aaaa(
        &mut self,
        name: &str,
        address: &[u8; 16],
        ttl_ms: u64,
        now_ms: u64,
    ) {
        let Some(encoded) = NameBuf::parse(name) else {
            return;
        };
        let index = self.upsert_slot(&encoded, Key::Typed(QueryType::Aaaa), ttl_ms, now_ms, false);
        let entry = &mut self.entries[index];
        entry.data[..16].copy_from_slice(address);
        entry.data_len = 16;
    }

    pub(crate) fn store_positive_cname(
        &mut self,
        name: &str,
        target: &NameBuf,
        ttl_ms: u64,
        now_ms: u64,
    ) {
        let Some(encoded) = NameBuf::parse(name) else {
            return;
        };
        if target.len == 0 || target.len > MAX_CACHED_A_RECORDS * 4 {
            return;
        }
        let index = self.upsert_slot(&encoded, Key::Cname, ttl_ms, now_ms, false);
        let entry = &mut self.entries[index];
        entry.data[..target.len].copy_from_slice(&target.bytes[..target.len]);
        entry.data_len = target.len;
    }

    /// Store a negative result; TTL is capped at NEGATIVE_TTL_MS_CAP.
    pub(crate) fn store_negative(&mut self, name: &str, qtype: QueryType, ttl_ms: u64, now_ms: u64) {
        let Some(encoded) = NameBuf::parse(name) else {
            return;
        };
        let capped = ttl_ms.clamp(1, NEGATIVE_TTL_MS_CAP);
        let index = self.upsert_slot(&encoded, Key::Typed(qtype), capped, now_ms, true);
        self.entries[index].data_len = 0;
    }

    fn upsert_slot(
        &mut self,
        name: &NameBuf,
        key: Key,
        ttl_ms: u64,
        now_ms: u64,
        negative: bool,
    ) -> usize {
        let existing = self.find_slot(name, key);
        let index = match existing {
            Some(index) => index,
            None => match self.free_slot() {
                Some(index) => index,
                None => self.evict_oldest(),
            },
        };
        self.entries[index] = Entry {
            active: true,
            key: Some(key),
            negative,
            name: *name,
            data: [0; MAX_CACHED_A_RECORDS * 4],
            data_len: 0,
            expires_ms: now_ms.saturating_add(ttl_ms.max(1)),
        };
        index
    }

    /// Drop every expired entry (called opportunistically from status paths).
    pub(crate) fn prune(&mut self, now_ms: u64) {
        for entry in &mut self.entries {
            if entry.active && entry.expired(now_ms) {
                *entry = Entry::empty();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_ttl_expiry() {
        let mut cache = ResolverCache::new();
        cache.store_positive_a("host.test", &[0x0A00_0001], 100, 1_000);
        assert!(matches!(
            cache.lookup("host.test", QueryType::A, 1_050),
            CacheLookup::HitA(_, 1)
        ));
        // Past expiry: miss and entry dropped.
        assert!(matches!(
            cache.lookup("host.test", QueryType::A, 1_101),
            CacheLookup::Miss
        ));
        assert_eq!(cache.hits, 1);
        assert_eq!(cache.misses, 1);
    }

    #[test]
    fn negative_cache_hit_then_expiry() {
        let mut cache = ResolverCache::new();
        cache.store_negative("gone.test", QueryType::A, 60_000, 100);
        assert!(
            matches!(cache.lookup("gone.test", QueryType::A, 200), CacheLookup::Negative),
            "negative hit before expiry"
        );
        assert_eq!(cache.negative_hits, 1);
        // TTL capped at NEGATIVE_TTL_MS_CAP: expires at 100 + 30_000 = 30_100.
        assert!(
            matches!(cache.lookup("gone.test", QueryType::A, 30_000), CacheLookup::Negative),
            "capped TTL still live just before expiry"
        );
        assert!(
            matches!(cache.lookup("gone.test", QueryType::A, 30_200), CacheLookup::Miss),
            "expired after capped 30s TTL"
        );
    }

    #[test]
    fn negative_ttl_is_capped() {
        let mut cache = ResolverCache::new();
        cache.store_negative("big.test", QueryType::A, 3_600_000, 0);
        assert!(matches!(
            cache.lookup("big.test", QueryType::A, NEGATIVE_TTL_MS_CAP - 1),
            CacheLookup::Negative
        ));
        assert!(matches!(
            cache.lookup("big.test", QueryType::A, NEGATIVE_TTL_MS_CAP),
            CacheLookup::Miss
        ));
    }

    #[test]
    fn cname_store_and_probe_with_expiry() {
        let mut cache = ResolverCache::new();
        let target = NameBuf::parse("cdn.example.net").unwrap();
        cache.store_positive_cname("www.example.com", &target, 5_000, 10);
        assert_eq!(
            cache
                .lookup_cname("www.example.com", 20)
                .and_then(|t| t.as_str().map(str::len)),
            Some("cdn.example.net".len())
        );
        assert!(cache.lookup_cname("www.example.com", 10_011).is_none());
    }

    #[test]
    fn cname_chain_bound_via_repeated_probes() {
        // Simulates chasing: each hop consumes one cached CNAME; bound check
        // lives in dnsresolv, here we prove probes terminate on missing link.
        let mut cache = ResolverCache::new();
        let a = NameBuf::parse("b.test").unwrap();
        let b = NameBuf::parse("c.test").unwrap();
        cache.store_positive_cname("a.test", &a, 60_000, 0);
        cache.store_positive_cname("b.test", &b, 60_000, 0);
        let mut current: NameBuf = NameBuf::parse("a.test").unwrap();
        for _ in 0..crate::consts::MAX_CNAME_CHAIN {
            match cache.lookup_cname(current.as_str().unwrap_or(""), 1) {
                Some(next) => current = next,
                None => break,
            }
        }
        assert!(current.matches("c.test"));
    }

    #[test]
    fn upsert_overwrites_and_capacity_evicts_oldest() {
        const NAMES: [&str; MAX_RESOLVER_CACHE_ENTRIES] = [
            "n0.test", "n1.test", "n2.test", "n3.test", "n4.test", "n5.test", "n6.test",
            "n7.test", "n8.test", "n9.test", "n10.test", "n11.test", "n12.test", "n13.test",
            "n14.test", "n15.test",
        ];
        let mut cache = ResolverCache::new();
        cache.store_positive_a("h.test", &[1, 2], 50_000, 0);
        cache.store_positive_a("h.test", &[3], 50_000, 0);
        assert!(matches!(
            cache.lookup("h.test", QueryType::A, 1),
            CacheLookup::HitA(addrs, 1) if addrs[0] == 3
        ));

        for (index, name) in NAMES.iter().enumerate() {
            cache.store_positive_a(name, &[index as u32], 1_000 + index as u64, 0);
        }
        // All slots full; inserting one more evicts the earliest expiring
        // entry (n0.test, expires at 1000).
        cache.store_positive_a("fresh.test", &[99], 500, 0);
        assert!(matches!(
            cache.lookup("n0.test", QueryType::A, 1),
            CacheLookup::Miss
        ));
        assert!(matches!(
            cache.lookup("fresh.test", QueryType::A, 1),
            CacheLookup::HitA(addrs, 1) if addrs[0] == 99
        ));
    }

    #[test]
    fn aaaa_round_trip() {
        let mut cache = ResolverCache::new();
        let address: [u8; 16] = core::array::from_fn(|i| i as u8);
        cache.store_positive_aaaa("v6.test", &address, 1_000, 5);
        match cache.lookup("v6.test", QueryType::Aaaa, 6) {
            CacheLookup::HitAaaa(seen) => assert_eq!(seen, address),
            other => panic!("expected AAAA hit, got {other:?}"),
        }
    }

    #[test]
    fn prune_clears_expired_only() {
        let mut cache = ResolverCache::new();
        cache.store_positive_a("live.test", &[1], 10_000, 0);
        cache.store_positive_a("dead.test", &[2], 10, 0);
        cache.prune(100);
        assert!(matches!(
            cache.lookup("dead.test", QueryType::A, 100),
            CacheLookup::Miss
        ));
        assert!(matches!(
            cache.lookup("live.test", QueryType::A, 100),
            CacheLookup::HitA(_, 1)
        ));
    }
}

//! Resolver front-end: bounded CNAME chasing over the cache, plus the real
//! UDP DNS transport. The chase core takes the query as a closure so it is
//! host-unit-testable without sockets.

use smoltcp::{
    iface::{Interface, SocketHandle, SocketSet},
    socket::udp,
    wire::{IpAddress, Ipv4Address},
};

use serviceos_userspace_runtime as rt;

use crate::{
    cache::{CacheLookup, ResolverCache},
    consts::{
        DNS_RETRANSMIT_MS, DNS_SERVER_PORT, DNS_UDP_BUFFER_BYTES, MAX_CNAME_CHAIN, MAX_TXT_BYTES,
        NEGATIVE_TTL_MS_CAP, NODATA_TTL_MS,
    },
    device::KernelPacketDevice,
    dnsmsg::{self, DnsRecords, NameBuf, QueryType, RCODE_NXDOMAIN, RCODE_OK, RCODE_SERVFAIL},
    types::HostEntry,
    util::{now_instant, ticks_to_millis},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ChaseDetail {
    Fresh,
    PositiveCache,
    NegativeCache,
    NxDomain,
    ServFail,
    NoData,
    Timeout,
    ChainTooLong,
    Malformed,
}

impl ChaseDetail {
    /// Maps onto the RESOLVE_DETAIL_* words sent in replies.
    pub(crate) fn word(self) -> u64 {
        match self {
            ChaseDetail::Fresh => crate::consts::RESOLVE_DETAIL_FRESH,
            ChaseDetail::NxDomain => crate::consts::RESOLVE_DETAIL_NXDOMAIN,
            ChaseDetail::ServFail => crate::consts::RESOLVE_DETAIL_SERVFAIL,
            ChaseDetail::NoData => crate::consts::RESOLVE_DETAIL_NODATA,
            ChaseDetail::Timeout => crate::consts::RESOLVE_DETAIL_TIMEOUT,
            ChaseDetail::NegativeCache => crate::consts::RESOLVE_DETAIL_NEGATIVE_CACHE,
            ChaseDetail::PositiveCache => crate::consts::RESOLVE_DETAIL_POSITIVE_CACHE,
            ChaseDetail::ChainTooLong => crate::consts::RESOLVE_DETAIL_CHAIN_TOO_LONG,
            ChaseDetail::Malformed => crate::consts::RESOLVE_DETAIL_MALFORMED,
        }
    }

    pub(crate) fn is_success(self) -> bool {
        matches!(self, ChaseDetail::Fresh | ChaseDetail::PositiveCache)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ChaseOutcome {
    pub(crate) address: Option<u32>,
    pub(crate) aaaa: [[u8; 16]; 2],
    pub(crate) aaaa_count: usize,
    pub(crate) txt: [u8; MAX_TXT_BYTES],
    pub(crate) txt_len: usize,
    pub(crate) detail: ChaseDetail,
}

impl ChaseOutcome {
    pub(crate) const fn with(detail: ChaseDetail) -> Self {
        Self {
            address: None,
            aaaa: [[0; 16]; 2],
            aaaa_count: 0,
            txt: [0; MAX_TXT_BYTES],
            txt_len: 0,
            detail,
        }
    }
}

fn host_lookup(hosts: &[HostEntry], name: &str) -> Option<u32> {
    hosts
        .iter()
        .find(|entry| entry.name_len != 0 && entry.matches(name))
        .map(|entry| crate::util::ipv4_to_u32(entry.address))
}

/// Bounded CNAME-chasing resolution. Each hop probes the cache first, then
/// consults the local hosts table, then issues one query via `query`
/// (`None` = transport timeout/unreachable). Depth is hard-capped at
/// MAX_CNAME_CHAIN hops regardless of cache shortcuts.
pub(crate) fn chase(
    start: &str,
    qtype: QueryType,
    cache: &mut ResolverCache,
    hosts: &[HostEntry],
    now_ms: u64,
    mut query: impl FnMut(&str) -> Option<DnsRecords>,
) -> ChaseOutcome {
    let mut current = match NameBuf::parse(start) {
        Some(encoded) => encoded,
        None => return ChaseOutcome::with(ChaseDetail::Malformed),
    };

    for _ in 0..MAX_CNAME_CHAIN {
        let Some(name) = current.as_str() else {
            return ChaseOutcome::with(ChaseDetail::Malformed);
        };

        // Cache probe for the queried record type.
        match qtype {
            QueryType::A => match cache.lookup(name, QueryType::A, now_ms) {
                CacheLookup::HitA(addresses, count) if count > 0 => {
                    let mut outcome = ChaseOutcome::with(ChaseDetail::PositiveCache);
                    outcome.address = Some(addresses[0]);
                    return outcome;
                }
                CacheLookup::Negative => {
                    return ChaseOutcome::with(ChaseDetail::NegativeCache);
                }
                _ => {}
            },
            QueryType::Aaaa => match cache.lookup(name, QueryType::Aaaa, now_ms) {
                CacheLookup::HitAaaa(address) => {
                    let mut outcome = ChaseOutcome::with(ChaseDetail::PositiveCache);
                    outcome.aaaa[0] = address;
                    outcome.aaaa_count = 1;
                    return outcome;
                }
                CacheLookup::Negative => {
                    return ChaseOutcome::with(ChaseDetail::NegativeCache);
                }
                _ => {}
            },
            QueryType::Txt => {}
        }

        // Cached CNAME shortcut (counts against the same depth bound).
        if let Some(target) = cache.lookup_cname(name, now_ms) {
            current = target;
            continue;
        }

        // Local hosts table answers A lookups authoritatively.
        if qtype == QueryType::A
            && let Some(address) = host_lookup(hosts, name)
        {
            let mut outcome = ChaseOutcome::with(ChaseDetail::Fresh);
            outcome.address = Some(address);
            return outcome;
        }

        let Some(records) = query(name) else {
            return ChaseOutcome::with(ChaseDetail::Timeout);
        };
        if records.qtype_matched != qtype {
            return ChaseOutcome::with(ChaseDetail::Malformed);
        }

        match records.rcode {
            RCODE_OK => {}
            RCODE_NXDOMAIN => {
                let ttl = records.min_ttl_ms.clamp(1, NEGATIVE_TTL_MS_CAP);
                cache.store_negative(name, qtype, ttl, now_ms);
                return ChaseOutcome::with(ChaseDetail::NxDomain);
            }
            RCODE_SERVFAIL => return ChaseOutcome::with(ChaseDetail::ServFail),
            _ => {
                // Other rcodes (NOTIMP/REFUSED/...) behave like server-side
                // failures and are not cached.
                return ChaseOutcome::with(ChaseDetail::ServFail);
            }
        }

        // Records may be keyed to the end of an in-packet CNAME chain rather
        // than the queried name itself.
        let owner_name = records
            .resolved_owner
            .as_ref()
            .and_then(|owner| owner.as_str())
            .unwrap_or(name);

        let mut outcome = ChaseOutcome::with(ChaseDetail::Fresh);
        match qtype {
            QueryType::A if records.a_count > 0 => {
                cache.store_positive_a(
                    owner_name,
                    &records.a[..records.a_count],
                    records.min_ttl_ms,
                    now_ms,
                );
                if let Some(owner) = records.resolved_owner.as_ref() {
                    if owner.bytes[..owner.len] != current.bytes[..current.len] {
                        cache.store_positive_cname(name, owner, records.min_ttl_ms, now_ms);
                    }
                } else if let Some(target) = records.cname.as_ref() {
                    // Same-packet single-link CNAME alongside the answer.
                    cache.store_positive_cname(name, target, records.min_ttl_ms, now_ms);
                }
                outcome.address = Some(records.a[0]);
                return outcome;
            }
            QueryType::Aaaa if records.aaaa_count > 0 => {
                cache.store_positive_aaaa(owner_name, &records.aaaa[0], records.min_ttl_ms, now_ms);
                outcome.aaaa = records.aaaa;
                outcome.aaaa_count = records.aaaa_count;
                return outcome;
            }
            QueryType::Txt if records.txt_len > 0 => {
                outcome.txt = records.txt;
                outcome.txt_len = records.txt_len;
                return outcome;
            }
            _ => {}
        }

        if let Some(target) = records.resolved_owner.or(records.cname) {
            cache.store_positive_cname(name, &target, records.min_ttl_ms.max(1), now_ms);
            current = target;
            continue;
        }

        // RCODE_OK without data: NODATA negative answer.
        cache.store_negative(name, qtype, NODATA_TTL_MS, now_ms);
        return ChaseOutcome::with(ChaseDetail::NoData);
    }

    ChaseOutcome::with(ChaseDetail::ChainTooLong)
}

/// Real DNS-over-UDP transport on a pre-added pool UDP socket.
pub(crate) struct DnsTransport<'i, 's> {
    pub(crate) server: Ipv4Address,
    pub(crate) id: u16,
    pub(crate) timeout_ticks: u64,
    pub(crate) handle: SocketHandle,
    pub(crate) iface: &'i mut Interface,
    pub(crate) device: &'i mut KernelPacketDevice,
    pub(crate) sockets: &'i mut SocketSet<'s>,
}

impl DnsTransport<'_, '_> {
    fn send_query(&mut self, packet: &[u8], destination: smoltcp::wire::IpEndpoint) -> bool {
        let _ = self.iface.poll(now_instant(), self.device, self.sockets);
        self.sockets
            .get_mut::<udp::Socket>(self.handle)
            .send_slice(packet, destination)
            .is_ok()
    }

    /// One best-effort exchange: send, retransmit every DNS_RETRANSMIT_MS,
    /// until a matching parsed response arrives or the tick budget runs out.
    pub(crate) fn exchange(&mut self, name: &str, qtype: QueryType) -> Option<DnsRecords> {
        if self.server == Ipv4Address::UNSPECIFIED {
            return None;
        }
        let mut buffer = [0u8; DNS_UDP_BUFFER_BYTES];
        let length = dnsmsg::build_query(&mut buffer, self.id, name, qtype)?;
        let destination = smoltcp::wire::IpEndpoint {
            addr: IpAddress::Ipv4(self.server),
            port: DNS_SERVER_PORT,
        };
        if !self.send_query(&buffer[..length], destination) {
            return None;
        }
        let started_ms = ticks_to_millis(rt::monotonic_now().ok()?);
        let mut last_send_ms = started_ms;
        loop {
            let _ = self.iface.poll(now_instant(), self.device, self.sockets);
            let mut scratch = [0u8; DNS_UDP_BUFFER_BYTES];
            if let Ok((count, _meta)) = self
                .sockets
                .get_mut::<udp::Socket>(self.handle)
                .recv_slice(&mut scratch)
            {
                if let Some(records) =
                    dnsmsg::parse_response(&scratch[..count], self.id, name, qtype)
                {
                    return Some(records);
                }
                // Stale/mismatched datagram: keep waiting.
            }

            let now_ms = ticks_to_millis(rt::monotonic_now().ok()?);
            if now_ms.saturating_sub(started_ms) >= ticks_to_millis(self.timeout_ticks) {
                return None;
            }
            if now_ms.saturating_sub(last_send_ms) >= DNS_RETRANSMIT_MS {
                if !self.send_query(&buffer[..length], destination) {
                    return None;
                }
                last_send_ms = now_ms;
            }
            rt::yield_current().ok()?;
        }
    }
}

/// Typed resolution over the real transport (A / AAAA / TXT).
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_typed(
    target: &str,
    qtype: QueryType,
    hosts: &[HostEntry],
    cache: &mut ResolverCache,
    timeout_ticks: u64,
    iface: &mut Interface,
    device: &mut KernelPacketDevice,
    sockets: &mut SocketSet<'_>,
    dns_handle: SocketHandle,
    next_query_id: &mut u16,
    server: Ipv4Address,
) -> rt::Result<ChaseOutcome> {
    let now_ms = ticks_to_millis(rt::monotonic_now().unwrap_or(0));
    let id = *next_query_id;
    *next_query_id = next_query_id.wrapping_add(1).max(1);
    let mut transport = DnsTransport {
        server,
        id,
        timeout_ticks,
        handle: dns_handle,
        iface,
        device,
        sockets,
    };
    Ok(chase(target, qtype, cache, hosts, now_ms, |name| {
        transport.exchange(name, qtype)
    }))
}

/// Convenience: full IPv4 resolution path used by Resolve/Ping/SocketOpen.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_ipv4(
    target: &str,
    hosts: &[HostEntry],
    cache: &mut ResolverCache,
    timeout_ticks: u64,
    iface: &mut Interface,
    device: &mut KernelPacketDevice,
    sockets: &mut SocketSet<'_>,
    dns_handle: SocketHandle,
    next_query_id: &mut u16,
    server: Ipv4Address,
) -> rt::Result<ChaseOutcome> {
    if let Some(address) = crate::config::parse_ipv4(target) {
        let mut outcome = ChaseOutcome::with(ChaseDetail::Fresh);
        outcome.address = Some(crate::util::ipv4_to_u32(address));
        return Ok(outcome);
    }
    resolve_typed(
        target,
        QueryType::A,
        hosts,
        cache,
        timeout_ticks,
        iface,
        device,
        sockets,
        dns_handle,
        next_query_id,
        server,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dnsmsg::RTYPE_A;

    fn records(
        rcode: u8,
        cname: Option<&'static str>,
        a: &[u32],
        aaaa: &[[u8; 16]],
        ttl_s: u64,
        qtype: QueryType,
    ) -> DnsRecords {
        let mut parsed = DnsRecords {
            rcode,
            qtype_matched: qtype,
            a: [0; crate::consts::MAX_CACHED_A_RECORDS],
            a_count: 0,
            aaaa: [[0; 16]; 2],
            aaaa_count: 0,
            cname: cname.and_then(NameBuf::parse),
            resolved_owner: None,
            txt: [0; MAX_TXT_BYTES],
            txt_len: 0,
            min_ttl_ms: ttl_s * 1000,
        };
        for (index, address) in a.iter().enumerate().take(parsed.a.len()) {
            parsed.a[index] = *address;
            parsed.a_count += 1;
        }
        for (index, address) in aaaa.iter().enumerate().take(2) {
            parsed.aaaa[index] = *address;
            parsed.aaaa_count += 1;
        }
        parsed
    }

    fn ok_a(a: &[u32]) -> Option<DnsRecords> {
        Some(records(RCODE_OK, None, a, &[], 60, QueryType::A))
    }

    const EDGE_A: u32 = u32::from_be_bytes([203, 0, 113, 9]);

    #[test]
    fn direct_a_answer_is_fresh_and_cached() {
        let mut cache = ResolverCache::new();
        let outcome = chase("dns.test", QueryType::A, &mut cache, &[], 0, |_name| {
            ok_a(&[0x0808_0808])
        });
        assert_eq!(outcome.detail, ChaseDetail::Fresh);
        assert_eq!(outcome.address, Some(0x0808_0808));
        // Second resolution is served from cache without any new exchange.
        let mut exchanges = 0usize;
        let cached = chase("dns.test", QueryType::A, &mut cache, &[], 1, |name| {
            exchanges += 1;
            let _ = name;
            None
        });
        assert_eq!(cached.detail, ChaseDetail::PositiveCache);
        assert_eq!(cached.address, Some(0x0808_0808));
        assert_eq!(exchanges, 0, "replay must not touch the network");
    }

    #[test]
    fn nxdomain_negative_cached_then_short_circuits() {
        let mut cache = ResolverCache::new();
        let first = chase("missing.test", QueryType::A, &mut cache, &[], 0, |_name| {
            Some(records(RCODE_NXDOMAIN, None, &[], &[], 120, QueryType::A))
        });
        assert_eq!(first.detail, ChaseDetail::NxDomain);
        let second = chase("missing.test", QueryType::A, &mut cache, &[], 1, |_name| {
            panic!("network queried despite negative cache")
        });
        assert_eq!(second.detail, ChaseDetail::NegativeCache);
    }

    #[test]
    fn servfail_distinct_from_nxdomain_and_not_cached() {
        let mut cache = ResolverCache::new();
        let first = chase("flaky.test", QueryType::A, &mut cache, &[], 0, |_name| {
            Some(records(
                2, /* SERVFAIL */
                None,
                &[],
                &[],
                60,
                QueryType::A,
            ))
        });
        assert_eq!(first.detail, ChaseDetail::ServFail);
        // Not negatively cached: retry goes back to the network.
        let second = chase("flaky.test", QueryType::A, &mut cache, &[], 1, |_name| {
            ok_a(&[1])
        });
        assert_eq!(second.detail, ChaseDetail::Fresh);
    }

    #[test]
    fn cname_chain_followed_across_queries() {
        const MID_CNAME: &str = "mid.test";
        const EDGE_CNAME: &str = "edge.test";
        let mut cache = ResolverCache::new();
        let fake = |name: &str| match name {
            "start.test" => Some(records(
                RCODE_OK,
                Some(MID_CNAME),
                &[],
                &[],
                30,
                QueryType::A,
            )),
            MID_CNAME => Some(records(
                RCODE_OK,
                Some(EDGE_CNAME),
                &[],
                &[],
                30,
                QueryType::A,
            )),
            EDGE_CNAME => ok_a(&[EDGE_A]),
            _ => None,
        };
        let outcome = chase("start.test", QueryType::A, &mut cache, &[], 0, fake);
        assert_eq!(outcome.detail, ChaseDetail::Fresh);
        assert_eq!(outcome.address, Some(EDGE_A));
        // Chain links are memoized; a replay walks cache shortcuts to the
        // final A entry without any network exchange.
        let replay = chase("start.test", QueryType::A, &mut cache, &[], 1, |_name| {
            panic!("chain links should be cached")
        });
        assert_eq!(replay.detail, ChaseDetail::PositiveCache);
        assert_eq!(replay.address, Some(EDGE_A));
    }

    #[test]
    fn in_packet_chain_resolves_in_one_exchange() {
        // Server answers start.test with CNAME start->edge AND A edge in one
        // packet (resolved_owner set by the parser).
        let mut cache = ResolverCache::new();
        let mut answer = records(RCODE_OK, None, &[EDGE_A], &[], 30, QueryType::A);
        answer.cname = NameBuf::parse("edge.test");
        answer.resolved_owner = NameBuf::parse("edge.test");
        let mut hops = 0usize;
        let outcome = chase("start.test", QueryType::A, &mut cache, &[], 0, |_name| {
            hops += 1;
            Some(answer)
        });
        assert_eq!(hops, 1);
        assert_eq!(outcome.detail, ChaseDetail::Fresh);
        assert_eq!(outcome.address, Some(EDGE_A));
    }

    #[test]
    fn chain_bound_enforced_on_cname_loop() {
        let mut cache = ResolverCache::new();
        // A <-> B loop: every hop yields another CNAME forever.
        let fake = |name: &str| {
            let other = if name == "a.test" { "b.test" } else { "a.test" };
            Some(records(RCODE_OK, Some(other), &[], &[], 30, QueryType::A))
        };
        let outcome = chase("a.test", QueryType::A, &mut cache, &[], 0, fake);
        assert_eq!(outcome.detail, ChaseDetail::ChainTooLong);
        assert_eq!(outcome.address, None);
    }

    #[test]
    fn hosts_table_wins_per_hop_without_queries() {
        let mut cache = ResolverCache::new();
        let mut hosts = [HostEntry::empty(); 1];
        hosts[0].name_len = "pinned.test".len();
        hosts[0].name[..hosts[0].name_len].copy_from_slice(b"pinned.test");
        hosts[0].address = smoltcp::wire::Ipv4Address::new(192, 168, 7, 7);
        let outcome = chase(
            "pinned.test",
            QueryType::A,
            &mut cache,
            &hosts,
            0,
            |_name| panic!("hosts table should answer"),
        );
        assert_eq!(outcome.detail, ChaseDetail::Fresh);
        assert_eq!(outcome.address, Some(u32::from_be_bytes([192, 168, 7, 7])));
    }

    #[test]
    fn nodata_returns_no_data_and_negative_caches() {
        let mut cache = ResolverCache::new();
        let first = chase("empty.test", QueryType::A, &mut cache, &[], 0, |_name| {
            ok_a(&[])
        });
        assert_eq!(first.detail, ChaseDetail::NoData);
        let again = chase("empty.test", QueryType::A, &mut cache, &[], 1, |_name| {
            panic!("network queried despite nodata cache")
        });
        assert_eq!(again.detail, ChaseDetail::NegativeCache);
    }

    #[test]
    fn timeout_when_transport_unreachable() {
        let mut cache = ResolverCache::new();
        let outcome = chase(
            "blackhole.test",
            QueryType::A,
            &mut cache,
            &[],
            0,
            |_name| None,
        );
        assert_eq!(outcome.detail, ChaseDetail::Timeout);
    }

    #[test]
    fn malformed_name_rejected_without_queries() {
        let mut cache = ResolverCache::new();
        let outcome = chase("", QueryType::A, &mut cache, &[], 0, |_name| {
            panic!("no query for malformed name")
        });
        assert_eq!(outcome.detail, ChaseDetail::Malformed);
    }

    #[test]
    fn aaaa_resolution_populates_outcome_and_cache() {
        let address: [u8; 16] = core::array::from_fn(|i| i as u8);
        let mut cache = ResolverCache::new();
        let outcome = chase("v6.test", QueryType::Aaaa, &mut cache, &[], 0, |_name| {
            Some(records(
                RCODE_OK,
                None,
                &[],
                core::slice::from_ref(&address),
                45,
                QueryType::Aaaa,
            ))
        });
        assert_eq!(outcome.detail, ChaseDetail::Fresh);
        assert_eq!(outcome.aaaa_count, 1);
        assert_eq!(outcome.aaaa[0], address);
        let cached = chase("v6.test", QueryType::Aaaa, &mut cache, &[], 1, |_name| {
            panic!("network queried despite AAAA cache")
        });
        assert_eq!(cached.detail, ChaseDetail::PositiveCache);
        assert_eq!(cached.aaaa[0], address);
    }

    #[test]
    fn single_hop_chain_with_appended_answer_resolves() {
        let mut cache = ResolverCache::new();
        let outcome = chase("alias.test", QueryType::A, &mut cache, &[], 0, |name| {
            if name == "alias.test" {
                Some(records(
                    RCODE_OK,
                    Some("real.test"),
                    &[],
                    &[],
                    30,
                    QueryType::A,
                ))
            } else {
                ok_a(&[0x0102_0304])
            }
        });
        assert_eq!(outcome.detail, ChaseDetail::Fresh);
        assert_eq!(outcome.address, Some(0x0102_0304));
    }

    #[test]
    fn detail_codes_are_stable() {
        assert_eq!(
            ChaseDetail::Fresh.word(),
            crate::consts::RESOLVE_DETAIL_FRESH
        );
        assert!(ChaseDetail::Fresh.is_success());
        assert!(ChaseDetail::PositiveCache.is_success());
        assert!(!ChaseDetail::NxDomain.is_success());
        assert!(!ChaseDetail::ServFail.is_success());
        let _ = RTYPE_A; // touch import used by wire tests elsewhere
    }
}

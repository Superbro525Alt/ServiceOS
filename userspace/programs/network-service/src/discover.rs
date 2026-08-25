//! Local service registry and discovered-peer table for the beacon protocol.
//!
//! The registry holds *this host's* services (populated via the
//! DISCOVERY_REGISTER control tag); the peer table caches *remote* hosts seen
//! in recent announce frames so a peers query can answer "who advertised
//! within the last N ms". Pure fixed-capacity logic, host-unit-testable.

use crate::consts::{
    MAX_BEACON_PEERS, MAX_BEACON_NAME_BYTES, MAX_LOCAL_SERVICES, MAX_SERVICE_NAME_BYTES,
};
use serviceos_userspace_runtime as rt;

#[derive(Clone, Copy)]
pub(crate) struct LocalService {
    pub(crate) name_len: usize,
    pub(crate) name: [u8; MAX_SERVICE_NAME_BYTES],
    pub(crate) port: u16,
}

#[derive(Clone, Copy)]
pub(crate) struct Registry {
    pub(crate) count: usize,
    pub(crate) services: [LocalService; MAX_LOCAL_SERVICES],
}

impl Registry {
    pub(crate) const fn new() -> Self {
        Self {
            count: 0,
            services: [LocalService {
                name_len: 0,
                name: [0; MAX_SERVICE_NAME_BYTES],
                port: 0,
            }; MAX_LOCAL_SERVICES],
        }
    }

    /// Register or update by name. Returns Ok on insert/update.
    pub(crate) fn register(&mut self, name: &[u8], port: u16) -> rt::Result<()> {
        if name.is_empty() || name.len() > MAX_SERVICE_NAME_BYTES {
            return Err(rt::Error::InvalidArgument);
        }
        for index in 0..self.count {
            let service = &mut self.services[index];
            if service.name_len == name.len() && service.name[..name.len()] == *name {
                service.port = port;
                return Ok(());
            }
        }
        if self.count == self.services.len() {
            return Err(rt::Error::CapacityExceeded);
        }
        let service = &mut self.services[self.count];
        service.name_len = name.len();
        service.name[..name.len()].copy_from_slice(name);
        service.port = port;
        self.count += 1;
        Ok(())
    }

    pub(crate) fn snapshot(&self) -> &[LocalService] {
        &self.services[..self.count]
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Peer {
    pub(crate) name_len: usize,
    pub(crate) name: [u8; MAX_BEACON_NAME_BYTES],
    pub(crate) address: [u8; 4],
    pub(crate) last_seen_ms: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct PeerTable {
    pub(crate) count: usize,
    pub(crate) peers: [Peer; MAX_BEACON_PEERS],
}

impl PeerTable {
    pub(crate) const fn new() -> Self {
        Self {
            count: 0,
            peers: [Peer {
                name_len: 0,
                name: [0; MAX_BEACON_NAME_BYTES],
                address: [0; 4],
                last_seen_ms: 0,
            }; MAX_BEACON_PEERS],
        }
    }

    /// Record an announce from `address` advertising `name`. Same-name peers
    /// refresh in place (address update); unknown names evict the
    /// least-recently-seen entry when full.
    pub(crate) fn note(&mut self, name: &[u8], address: [u8; 4], now_ms: u64) -> rt::Result<()> {
        if name.is_empty() || name.len() > MAX_BEACON_NAME_BYTES {
            return Err(rt::Error::InvalidArgument);
        }
        for index in 0..self.count {
            let peer = &mut self.peers[index];
            if peer.name_len == name.len() && peer.name[..name.len()] == *name {
                peer.address = address;
                peer.last_seen_ms = now_ms;
                return Ok(());
            }
        }
        if self.count == self.peers.len() {
            let mut oldest = 0usize;
            for index in 1..self.count {
                if self.peers[index].last_seen_ms < self.peers[oldest].last_seen_ms {
                    oldest = index;
                }
            }
            // Refreshing an existing address under a stale name is fine: the
            // table is keyed by advertised hostname.
            let peer = &mut self.peers[oldest];
            peer.name_len = name.len();
            peer.name[..name.len()].copy_from_slice(name);
            peer.address = address;
            peer.last_seen_ms = now_ms;
            return Ok(());
        }
        let peer = &mut self.peers[self.count];
        peer.name_len = name.len();
        peer.name[..name.len()].copy_from_slice(name);
        peer.address = address;
        peer.last_seen_ms = now_ms;
        self.count += 1;
        Ok(())
    }

    /// Peers seen at most `window_ms` before `now_ms`, packed into `out`.
    /// Returns the number of entries written.
    pub(crate) fn recent(&self, now_ms: u64, window_ms: u64, out: &mut [Peer]) -> usize {
        let mut written = 0usize;
        for index in 0..self.count {
            let peer = self.peers[index];
            if now_ms.saturating_sub(peer.last_seen_ms) > window_ms {
                continue;
            }
            if written == out.len() {
                break;
            }
            out[written] = peer;
            written += 1;
        }
        written
    }

    /// Forget entries older than `window_ms`; returns how many were dropped.
    pub(crate) fn expire(&mut self, now_ms: u64, window_ms: u64) -> usize {
        let mut kept = 0usize;
        for index in 0..self.count {
            if now_ms.saturating_sub(self.peers[index].last_seen_ms) <= window_ms {
                self.peers.swap(kept, index);
                kept += 1;
            }
        }
        let dropped = self.count - kept;
        self.count = kept;
        dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_updates_by_name_and_enforces_capacity() {
        let mut registry = Registry::new();
        assert!(registry.register(b"shell", 4021).is_ok());
        assert!(registry.register(b"logs", 4022).is_ok());
        assert!(registry.register(b"shell", 4099).is_ok()); // update port
        assert_eq!(registry.count, 2);
        assert_eq!(registry.snapshot()[0].port, 4099);
        assert_eq!(registry.snapshot()[1].port, 4022);

        let mut fill = 0usize;
        while registry.count < MAX_LOCAL_SERVICES {
            let name = [b's', b'v', b'0' + fill as u8];
            assert!(registry.register(&name, 1000 + fill as u16).is_ok());
            fill += 1;
        }
        assert!(registry.register(b"overflow", 1).is_err());
        assert!(registry.register(b"", 1).is_err());
    }

    #[test]
    fn peer_table_refreshes_and_expires() {
        let mut peers = PeerTable::new();
        assert!(peers.note(b"alpha", [10, 0, 0, 1], 100).is_ok());
        assert!(peers.note(b"beta", [10, 0, 0, 2], 200).is_ok());
        assert!(peers.note(b"alpha", [10, 0, 0, 3], 300).is_ok()); // refresh
        assert_eq!(peers.count, 2);
        assert_eq!(peers.peers[0].address, [10, 0, 0, 3]);
        assert_eq!(peers.peers[0].last_seen_ms, 300);

        let mut recent = [Peer {
            name_len: 0,
            name: [0; MAX_BEACON_NAME_BYTES],
            address: [0; 4],
            last_seen_ms: 0,
        }; MAX_BEACON_PEERS];
        // Wide window keeps both; narrow window drops both (alpha at 300 is
        // 100ms old, beta at 200 is 200ms old).
        assert_eq!(peers.recent(400, 500, &mut recent), 2);
        assert_eq!(peers.recent(400, 50, &mut recent), 0);

        assert_eq!(peers.expire(100_000, 30_000), 2);
        assert_eq!(peers.count, 0);
    }

    #[test]
    fn peer_table_evicts_least_recently_seen_when_full() {
        let mut peers = PeerTable::new();
        for (index, name) in [b"p1".as_slice(), &b"p2"[..], &b"p3"[..], &b"p4"[..]]
            .iter()
            .enumerate()
        {
            peers
                .note(name, [10, 0, 0, index as u8 + 1], 1000 + index as u64)
                .ok();
        }
        assert_eq!(peers.count, MAX_BEACON_PEERS);
        // p1 is oldest (seen at 1000): a fifth peer must evict it.
        peers.note(b"p5", [10, 0, 0, 5], 9000).ok();
        assert_eq!(peers.count, MAX_BEACON_PEERS);
        let mut recent = [Peer {
            name_len: 0,
            name: [0; MAX_BEACON_NAME_BYTES],
            address: [0; 4],
            last_seen_ms: 0,
        }; MAX_BEACON_PEERS];
        let written = peers.recent(9001, 60_000, &mut recent);
        assert_eq!(written, MAX_BEACON_PEERS);
        assert!(written == MAX_BEACON_PEERS && recent[..written].iter().all(|peer| {
            peer.name[..peer.name_len] != *b"p1"
        }));
    }
}

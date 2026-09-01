//! Firewall rule table: ordered allow/deny rules over (protocol, direction,
//! port, interface) with per-rule hit counters, an optional remote-address
//! qualifier backed by named CIDR address sets, and a configurable default
//! inbound policy. Pure decision logic, host-unit-testable.

use crate::consts::{MAX_FIREWALL_ADDR_SET_ENTRIES, MAX_FIREWALL_ADDR_SETS, MAX_FIREWALL_RULES};

/// The remote endpoint address of the connection under decision, family
/// tagged. Address sets match family-strictly: a v4 CIDR only contains v4
/// remotes, a v6 prefix only v6 remotes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RemoteAddress {
    V4([u8; 4]),
    V6([u8; 16]),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CidrFamily {
    V4,
    V6,
}

/// One CIDR entry of an address set. The address is stored zero-extended to
/// 16 bytes (v4 uses the first 4 bytes); `prefix` is bounded by the family
/// (<= 32 for v4, <= 128 for v6).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Cidr {
    family: CidrFamily,
    prefix: u8,
    address: [u8; 16],
}

impl Cidr {
    pub(crate) const fn v4(address: [u8; 4], prefix: u8) -> Option<Self> {
        if prefix > 32 {
            return None;
        }
        let mut bytes = [0u8; 16];
        bytes[0] = address[0];
        bytes[1] = address[1];
        bytes[2] = address[2];
        bytes[3] = address[3];
        Some(Self {
            family: CidrFamily::V4,
            prefix,
            address: bytes,
        })
    }

    pub(crate) const fn v6(address: [u8; 16], prefix: u8) -> Option<Self> {
        if prefix > 128 {
            return None;
        }
        Some(Self {
            family: CidrFamily::V6,
            prefix,
            address,
        })
    }

    /// Family-strict membership: prefix 0 matches every address of the
    /// family, prefix 32/128 a single host, and the other family never.
    pub(crate) fn contains(&self, remote: RemoteAddress) -> bool {
        match (self.family, remote) {
            (CidrFamily::V4, RemoteAddress::V4(remote)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - self.prefix.min(32))
                };
                let base = u32::from_be_bytes([
                    self.address[0],
                    self.address[1],
                    self.address[2],
                    self.address[3],
                ]);
                u32::from_be_bytes(remote) & mask == base & mask
            }
            (CidrFamily::V6, RemoteAddress::V6(remote)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - self.prefix.min(128))
                };
                u128::from_be_bytes(remote) & mask == u128::from_be_bytes(self.address) & mask
            }
            _ => false,
        }
    }

    /// Encode to the SetDefine entry layout: [family | prefix<<8] then the
    /// address as two big-endian words (v4: the address as a big-endian
    /// u32 in the LOW 32 bits of word A, word B stays 0; v6: bytes 0..8 /
    /// 8..16, matching ipv6_addr_words order).
    pub(crate) fn to_words(&self) -> [u64; 3] {
        let family = match self.family {
            CidrFamily::V4 => 0u64,
            CidrFamily::V6 => 1u64,
        };
        let value = u128::from_be_bytes(self.address);
        let (word_a, word_b) = match self.family {
            CidrFamily::V4 => ((value >> 96) as u64, 0),
            CidrFamily::V6 => ((value >> 64) as u64, value as u64),
        };
        [family | ((self.prefix as u64) << 8), word_a, word_b]
    }

    /// Decode one SetDefine entry; malformed families or out-of-range
    /// prefixes are rejected.
    pub(crate) fn from_words(words: [u64; 3]) -> Option<Self> {
        let family = match words[0] & 0xff {
            0 => CidrFamily::V4,
            1 => CidrFamily::V6,
            _ => return None,
        };
        let prefix = ((words[0] >> 8) & 0xff) as u8;
        match family {
            CidrFamily::V4 => {
                if prefix > 32 {
                    return None;
                }
                let be = (words[1] & 0xffff_ffff) as u32;
                Self::v4(be.to_be_bytes(), prefix)
            }
            CidrFamily::V6 => {
                if prefix > 128 {
                    return None;
                }
                let value = ((words[1] as u128) << 64) | words[2] as u128;
                Self::v6(value.to_be_bytes(), prefix)
            }
        }
    }
}

/// A named set of CIDR entries rules reference for remote-address
/// qualification. A set that is defined but empty matches nothing.
#[derive(Clone, Copy)]
pub(crate) struct AddrSet {
    defined: bool,
    entry_count: usize,
    entries: [Cidr; MAX_FIREWALL_ADDR_SET_ENTRIES],
}

impl AddrSet {
    pub(crate) const EMPTY: Self = Self {
        defined: false,
        entry_count: 0,
        entries: [const { Cidr::EMPTY_SLOT }; MAX_FIREWALL_ADDR_SET_ENTRIES],
    };

    /// Replace the entries wholesale and mark the set defined; `false` when
    /// the list overflows the bounded capacity.
    pub(crate) fn define(&mut self, cidrs: &[Cidr]) -> bool {
        if cidrs.len() > MAX_FIREWALL_ADDR_SET_ENTRIES {
            return false;
        }
        for (slot, cidr) in self.entries.iter_mut().zip(cidrs.iter()) {
            *slot = *cidr;
        }
        self.entry_count = cidrs.len();
        self.defined = true;
        true
    }

    pub(crate) fn is_defined(&self) -> bool {
        self.defined
    }

    pub(crate) fn contains(&self, remote: RemoteAddress) -> bool {
        self.entries[..self.entry_count]
            .iter()
            .any(|cidr| cidr.contains(remote))
    }
}

impl Cidr {
    pub(crate) const EMPTY_SLOT: Self = Self {
        family: CidrFamily::V4,
        prefix: 0,
        address: [0; 16],
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RuleAction {
    Allow,
    Deny,
}

impl RuleAction {
    pub(crate) fn from_word(value: u64) -> Option<Self> {
        match value {
            0 => Some(RuleAction::Allow),
            1 => Some(RuleAction::Deny),
            _ => None,
        }
    }

    fn word(self) -> u64 {
        match self {
            RuleAction::Allow => 0,
            RuleAction::Deny => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Proto {
    Any,
    Tcp,
    Udp,
    Icmp,
}

impl Proto {
    pub(crate) fn from_word(value: u64) -> Option<Self> {
        match value {
            0 => Some(Proto::Any),
            1 => Some(Proto::Tcp),
            2 => Some(Proto::Udp),
            3 => Some(Proto::Icmp),
            _ => None,
        }
    }

    fn word(self) -> u64 {
        match self {
            Proto::Any => 0,
            Proto::Tcp => 1,
            Proto::Udp => 2,
            Proto::Icmp => 3,
        }
    }

    fn matches(self, other: Proto) -> bool {
        self == Proto::Any || self == other
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Direction {
    Inbound,
    Outbound,
}

impl Direction {
    fn word(self) -> u64 {
        match self {
            Direction::Inbound => 0,
            Direction::Outbound => 1,
        }
    }

    fn from_word(value: u64) -> Option<Self> {
        match value {
            0 => Some(Direction::Inbound),
            1 => Some(Direction::Outbound),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FirewallRule {
    pub(crate) action: RuleAction,
    pub(crate) proto: Proto,
    pub(crate) direction: Direction,
    /// 0 = any port. Inbound rules compare against the local service port,
    /// outbound rules against the remote port.
    pub(crate) port: u16,
    /// Interface qualifier: `Some(index)` restricts the rule to that
    /// interface (0-based, as reported by InterfaceStatusRequest; the
    /// single boot interface is eth0 = index 0), `None` matches every
    /// interface (legacy behavior, byte-compatible with old rule words).
    pub(crate) interface: Option<u16>,
    /// Remote-address qualifier: `Some(id)` restricts the rule to
    /// connections whose remote address is a member of address set `id`
    /// (1-based, defined via FirewallAddrSetDefineRequest), `None` matches
    /// every remote address (legacy behavior).
    pub(crate) addr_set: Option<u8>,
    pub(crate) enabled: bool,
    /// Times this rule produced a decision.
    pub(crate) hits: u64,
}

impl FirewallRule {
    pub(crate) const fn empty() -> Self {
        Self {
            action: RuleAction::Allow,
            proto: Proto::Any,
            direction: Direction::Inbound,
            port: 0,
            interface: None,
            addr_set: None,
            enabled: false,
            hits: 0,
        }
    }

    fn matches(
        &self,
        direction: Direction,
        proto: Proto,
        local_port: u16,
        remote_port: u16,
        iface_index: u16,
        remote: RemoteAddress,
        addr_sets: &[AddrSet; MAX_FIREWALL_ADDR_SETS],
    ) -> bool {
        if !self.enabled
            || self.direction != direction
            || !self.proto.matches(proto)
            || self
                .interface
                .map_or(false, |interface| interface != iface_index)
        {
            return false;
        }
        if let Some(set_id) = self.addr_set {
            let Some(set) = addr_sets.get(set_id as usize - 1) else {
                return false;
            };
            if !set.contains(remote) {
                return false;
            }
        }
        let subject_port = match direction {
            Direction::Inbound => local_port,
            Direction::Outbound => remote_port,
        };
        self.port == 0 || self.port == subject_port
    }

    /// Pack for IPC reply: action | proto<<8 | direction<<16 | enabled<<24 |
    /// port<<32 | qualifier<<48; hits ride in the following word. The
    /// qualifier field holds one of: 0 = any (legacy), `index + 1` for a
    /// rule pinned to one interface, or `0x0100 | set-id` for a rule
    /// qualified by address set `set-id` (a rule carries at most one of the
    /// two qualifiers; the set wins if both were somehow set).
    pub(crate) fn pack(&self) -> [u64; 2] {
        let qualifier = if let Some(set) = self.addr_set {
            0x0100 | set as u64
        } else {
            self.interface.map_or(0, |interface| interface as u64 + 1)
        };
        [
            self.action.word()
                | (self.proto.word() << 8)
                | (self.direction.word() << 16)
                | ((self.enabled as u64) << 24)
                | ((self.port as u64) << 32)
                | (qualifier << 48),
            self.hits,
        ]
    }

    /// Decode from one packed request word (action/proto/direction/port and
    /// the trailing qualifier at bits [48..64): 0 = any, 1..=0x00ff =
    /// `interface index + 1`, 0x0100..=0x01ff = `0x0100 | set-id`; higher
    /// namespaces are reserved and rejected); `enabled` comes from the
    /// caller's flag word.
    pub(crate) fn unpack(word: u64, enabled: bool) -> Option<Self> {
        let qualifier = (word >> 48) as u16;
        let (interface, addr_set) = match qualifier {
            0 => (None, None),
            1..=0x00ff => (Some(qualifier - 1), None),
            set if (0x0100..=0x01ff).contains(&set) => (None, Some((set & 0xff) as u8)),
            _ => return None,
        };
        Some(Self {
            action: RuleAction::from_word(word & 0xff)?,
            proto: Proto::from_word((word >> 8) & 0xff)?,
            direction: Direction::from_word((word >> 16) & 0xff)?,
            port: (word >> 32) as u16,
            interface,
            addr_set,
            enabled,
            hits: 0,
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) struct FirewallState {
    pub(crate) rules: [FirewallRule; MAX_FIREWALL_RULES],
    pub(crate) rule_count: usize,
    /// Address sets referenced by rule qualifiers (index = id - 1).
    pub(crate) addr_sets: [AddrSet; MAX_FIREWALL_ADDR_SETS],
    /// Policy applied to inbound traffic that matched no rule. Outbound
    /// traffic unmatched by any rule is always allowed.
    pub(crate) default_inbound_allow: bool,
    pub(crate) inbound_denied_total: u64,
    pub(crate) outbound_denied_total: u64,
}

impl FirewallState {
    pub(crate) const fn new() -> Self {
        Self {
            rules: [const { FirewallRule::empty() }; MAX_FIREWALL_RULES],
            rule_count: 0,
            addr_sets: [const { AddrSet::EMPTY }; MAX_FIREWALL_ADDR_SETS],
            default_inbound_allow: true,
            inbound_denied_total: 0,
            outbound_denied_total: 0,
        }
    }

    /// `true` when set `id` (1-based) exists. Ids beyond the cap are never
    /// defined.
    pub(crate) fn addr_set_defined(&self, id: u8) -> bool {
        (id as usize)
            .checked_sub(1)
            .and_then(|index| self.addr_sets.get(index))
            .is_some_and(AddrSet::is_defined)
    }

    /// Define/replace address set `id` (1-based). `false` on an id beyond
    /// the cap or an entry-list overflow.
    pub(crate) fn define_addr_set(&mut self, id: u8, cidrs: &[Cidr]) -> bool {
        let Some(slot) = self.addr_sets.get_mut(id as usize - 1) else {
            return false;
        };
        slot.define(cidrs)
    }

    /// Drop every address set. Refused while any rule still references a
    /// set (dangling references would silently never match); the caller
    /// clears rules first.
    pub(crate) fn clear_addr_sets(&mut self) -> bool {
        if self.rules.iter().any(|rule| rule.addr_set.is_some()) {
            return false;
        }
        self.addr_sets = [const { AddrSet::EMPTY }; MAX_FIREWALL_ADDR_SETS];
        true
    }

    /// First-match-wins decision over enabled rules; falls back to the
    /// default policy (deny-able only for inbound). Increments counters.
    /// `iface_index` is the 0-based interface the traffic rides on; rules
    /// pinned to another interface (or unqualified rules) are the only
    /// candidates besides it. `remote` is the connection's remote address;
    /// rules qualified by an address set apply only when the set contains
    /// it (family-strict).
    pub(crate) fn decide(
        &mut self,
        direction: Direction,
        proto: Proto,
        local_port: u16,
        remote_port: u16,
        iface_index: u16,
        remote: RemoteAddress,
    ) -> bool {
        let index = self.rules[..self.rule_count].iter().position(|rule| {
            rule.matches(
                direction,
                proto,
                local_port,
                remote_port,
                iface_index,
                remote,
                &self.addr_sets,
            )
        });
        let allowed = match index {
            Some(index) => {
                self.rules[index].hits = self.rules[index].hits.saturating_add(1);
                self.rules[index].action == RuleAction::Allow
            }
            None => match direction {
                Direction::Inbound => self.default_inbound_allow,
                Direction::Outbound => true,
            },
        };
        if !allowed {
            match direction {
                Direction::Inbound => {
                    self.inbound_denied_total = self.inbound_denied_total.saturating_add(1)
                }
                Direction::Outbound => {
                    self.outbound_denied_total = self.outbound_denied_total.saturating_add(1)
                }
            }
        }
        allowed
    }

    /// Replace the whole table from packed request words
    /// (words[i*2] = rule fields incl. the trailing interface qualifier,
    /// words[i*2+1] = enable flag).
    /// Returns InvalidArgument-shaped error via Ok(None).
    pub(crate) fn replace_all(&mut self, words: &[u64], count: usize) -> Option<()> {
        if count > MAX_FIREWALL_RULES || count * 2 > words.len() {
            return None;
        }
        for slot in &mut self.rules {
            *slot = FirewallRule::empty();
        }
        for index in 0..count {
            let enabled = words[index * 2 + 1] != 0;
            let Some(rule) = FirewallRule::unpack(words[index * 2], enabled) else {
                // Roll back to a clean table on malformed input.
                for slot in &mut self.rules {
                    *slot = FirewallRule::empty();
                }
                self.rule_count = 0;
                return None;
            };
            // A rule may only reference an address set that is currently
            // defined; dangling references are rejected wholesale.
            if let Some(set_id) = rule.addr_set {
                if !self.addr_set_defined(set_id) {
                    for slot in &mut self.rules {
                        *slot = FirewallRule::empty();
                    }
                    self.rule_count = 0;
                    return None;
                }
            }
            self.rules[index] = rule;
        }
        self.rule_count = count;
        Some(())
    }

    pub(crate) fn set_default_inbound_allow(&mut self, allow: bool) {
        self.default_inbound_allow = allow;
    }

    pub(crate) fn clear_rules(&mut self) {
        for slot in &mut self.rules {
            *slot = FirewallRule::empty();
        }
        self.rule_count = 0;
    }

    /// Encode the table + summary into reply words starting at `words[0]`.
    /// Layout: [status][count][default_inbound_allow][inbound_denied<<32|outbound_denied]
    /// then 2 words per rule (the rule word carries the qualifier at bits
    /// [48..64) — interface pin or address-set id — so qualified rules
    /// round-trip additively; the global deny counters stay summary-wide —
    /// no trailing budget exists for per-interface counters at
    /// IPC_MAX_WORDS=16 with 6 rules).
    pub(crate) fn encode_reply(&self, reply_words: &mut [u64]) -> usize {
        reply_words[1] = self.rule_count as u64;
        reply_words[2] = self.default_inbound_allow as u64;
        reply_words[3] = (self.inbound_denied_total.min(u32::MAX as u64) << 32)
            | self.outbound_denied_total.min(u32::MAX as u64);
        let mut used = 4usize;
        for rule in &self.rules[..self.rule_count] {
            let packed = rule.pack();
            reply_words[used] = packed[0];
            reply_words[used + 1] = packed[1];
            used += 2;
        }
        used
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(action: RuleAction, proto: Proto, direction: Direction, port: u16) -> FirewallRule {
        FirewallRule {
            action,
            proto,
            direction,
            port,
            interface: None,
            addr_set: None,
            enabled: true,
            hits: 0,
        }
    }

    fn load(state: &mut FirewallState, rules: &[FirewallRule]) {
        let mut packed = Vec::new();
        for rule in rules {
            let words = rule.pack();
            packed.push(words[0]);
            packed.push(rule.enabled as u64);
        }
        assert!(state.replace_all(&packed, rules.len()).is_some());
    }

    #[test]
    fn empty_table_default_inbound_allow() {
        let mut state = FirewallState::new();
        assert!(state.decide(
            Direction::Inbound,
            Proto::Tcp,
            80,
            44_000,
            0,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert!(state.decide(
            Direction::Outbound,
            Proto::Tcp,
            0,
            80,
            0,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert_eq!(state.inbound_denied_total, 0);
    }

    #[test]
    fn empty_table_default_inbound_deny() {
        let mut state = FirewallState::new();
        state.set_default_inbound_allow(false);
        assert!(!state.decide(
            Direction::Inbound,
            Proto::Tcp,
            22,
            44_000,
            0,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert_eq!(state.inbound_denied_total, 1);
        assert!(
            state.decide(
                Direction::Outbound,
                Proto::Udp,
                53_000,
                53,
                0,
                RemoteAddress::V4([10, 0, 0, 1])
            ),
            "outbound unaffected by default-inbound deny"
        );
    }

    #[test]
    fn matrix_inbound_deny_tcp_80_only() {
        let mut state = FirewallState::new();
        load(
            &mut state,
            &[rule(RuleAction::Deny, Proto::Tcp, Direction::Inbound, 80)],
        );
        let cases: [(Proto, u16, bool); 5] = [
            (Proto::Tcp, 80, false),
            (Proto::Tcp, 81, true),
            (Proto::Udp, 80, true),
            (Proto::Icmp, 80, true),
            (Proto::Tcp, 443, true),
        ];
        for (proto, port, expected) in cases {
            assert_eq!(
                state.decide(
                    Direction::Inbound,
                    proto,
                    port,
                    40_000,
                    0,
                    RemoteAddress::V4([10, 0, 0, 1])
                ),
                expected,
                "proto={proto:?} port={port}"
            );
        }
        // Outbound same port/proto passes (direction mismatch).
        assert!(state.decide(
            Direction::Outbound,
            Proto::Tcp,
            0,
            80,
            0,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert_eq!(
            state.rules[0].hits, 1,
            "only the matching case hit the rule"
        );
    }

    #[test]
    fn matrix_outbound_deny_udp_any_port() {
        let mut state = FirewallState::new();
        load(
            &mut state,
            &[rule(RuleAction::Deny, Proto::Udp, Direction::Outbound, 0)],
        );
        assert!(!state.decide(
            Direction::Outbound,
            Proto::Udp,
            50_000,
            53,
            0,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert!(!state.decide(
            Direction::Outbound,
            Proto::Udp,
            50_000,
            123,
            0,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert!(state.decide(
            Direction::Outbound,
            Proto::Tcp,
            0,
            80,
            0,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert!(state.decide(
            Direction::Inbound,
            Proto::Udp,
            53,
            40_000,
            0,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert_eq!(state.outbound_denied_total, 2);
    }

    #[test]
    fn first_match_wins_ordering() {
        let mut state = FirewallState::new();
        load(
            &mut state,
            &[
                rule(RuleAction::Allow, Proto::Tcp, Direction::Inbound, 8080),
                rule(RuleAction::Deny, Proto::Tcp, Direction::Inbound, 0),
            ],
        );
        assert!(state.decide(
            Direction::Inbound,
            Proto::Tcp,
            8080,
            9,
            0,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert!(!state.decide(
            Direction::Inbound,
            Proto::Tcp,
            9999,
            9,
            0,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert_eq!(state.rules[0].hits, 1);
        assert_eq!(state.rules[1].hits, 1);
    }

    #[test]
    fn disabled_rules_skipped() {
        let mut state = FirewallState::new();
        let mut denied = rule(RuleAction::Deny, Proto::Tcp, Direction::Inbound, 80);
        denied.enabled = false;
        load(&mut state, &[denied]);
        assert!(state.decide(
            Direction::Inbound,
            Proto::Tcp,
            80,
            9,
            0,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert_eq!(state.rules[0].hits, 0);
    }

    #[test]
    fn icmp_rules_need_any_port() {
        let mut state = FirewallState::new();
        load(
            &mut state,
            &[rule(RuleAction::Deny, Proto::Icmp, Direction::Outbound, 0)],
        );
        assert!(!state.decide(
            Direction::Outbound,
            Proto::Icmp,
            0,
            0,
            0,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert!(!state.decide(
            Direction::Outbound,
            Proto::Icmp,
            0,
            0,
            0,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert_eq!(state.rules[0].hits, 2);
        assert_eq!(state.outbound_denied_total, 2);
    }

    #[test]
    fn replace_all_rejects_overflow_and_malformed() {
        let mut state = FirewallState::new();
        load(
            &mut state,
            &[rule(RuleAction::Allow, Proto::Any, Direction::Inbound, 1)],
        );
        let too_many = [0u64; (MAX_FIREWALL_RULES + 2) * 2];
        assert!(
            state
                .replace_all(&too_many, MAX_FIREWALL_RULES + 1)
                .is_none()
        );
        let bad_proto = [3 | (9u64 << 8), 1]; // proto word 9 invalid
        assert!(state.replace_all(&bad_proto, 1).is_none());
        assert_eq!(state.rule_count, 0, "malformed input clears the table");
    }

    #[test]
    fn pack_unpack_round_trip() {
        let original = FirewallRule {
            action: RuleAction::Deny,
            proto: Proto::Udp,
            direction: Direction::Outbound,
            port: 5353,
            interface: Some(1),
            addr_set: None,
            enabled: true,
            hits: 77,
        };
        let packed = original.pack();
        let unpacked = FirewallRule::unpack(packed[0], true).unwrap();
        assert_eq!(unpacked.action, original.action);
        assert_eq!(unpacked.proto, original.proto);
        assert_eq!(unpacked.direction, original.direction);
        assert_eq!(unpacked.port, original.port);
        assert_eq!(unpacked.interface, original.interface);
        assert_eq!(packed[1], 77);
    }

    #[test]
    fn encode_reply_shape() {
        let mut state = FirewallState::new();
        load(
            &mut state,
            &[
                rule(RuleAction::Allow, Proto::Tcp, Direction::Inbound, 80),
                rule(RuleAction::Deny, Proto::Any, Direction::Outbound, 0),
            ],
        );
        state.set_default_inbound_allow(false);
        assert!(!state.decide(
            Direction::Outbound,
            Proto::Tcp,
            0,
            25,
            0,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        let mut words = [0u64; 32];
        let used = state.encode_reply(&mut words);
        assert_eq!(used, 4 + 2 * 2);
        assert_eq!(words[1], 2);
        assert_eq!(words[2], 0);
        assert_eq!(words[3] >> 32, 0);
        assert_eq!(words[3] & 0xffff_ffff, 1);
    }

    fn qualified_rule(
        action: RuleAction,
        proto: Proto,
        direction: Direction,
        port: u16,
        interface: u16,
    ) -> FirewallRule {
        let mut rule = rule(action, proto, direction, port);
        rule.interface = Some(interface);
        rule
    }

    #[test]
    fn legacy_words_decode_unqualified() {
        // A pre-qualifier rule word must decode to `interface: None`.
        let legacy = rule(RuleAction::Deny, Proto::Tcp, Direction::Inbound, 80);
        let word = legacy.pack()[0];
        assert_eq!(word >> 48, 0, "legacy words leave the qualifier field zero");
        let decoded = FirewallRule::unpack(word, true).unwrap();
        assert_eq!(decoded.interface, None);
        assert_eq!(decoded.port, 80);
    }

    #[test]
    fn qualified_rule_matches_only_named_interface() {
        let mut state = FirewallState::new();
        load(
            &mut state,
            &[qualified_rule(
                RuleAction::Deny,
                Proto::Tcp,
                Direction::Inbound,
                80,
                1,
            )],
        );
        // eth0 (index 0): unqualified -> default allow.
        assert!(state.decide(
            Direction::Inbound,
            Proto::Tcp,
            80,
            9,
            0,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        // eth1 (index 1): the rule matches -> deny.
        assert!(!state.decide(
            Direction::Inbound,
            Proto::Tcp,
            80,
            9,
            1,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        // eth1, other port/proto: rule misses, default allows.
        assert!(state.decide(
            Direction::Inbound,
            Proto::Tcp,
            81,
            9,
            1,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert!(state.decide(
            Direction::Inbound,
            Proto::Udp,
            80,
            9,
            1,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        // eth1 outbound: direction mismatch, default (outbound always allow).
        assert!(state.decide(
            Direction::Outbound,
            Proto::Tcp,
            0,
            80,
            1,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert_eq!(state.rules[0].hits, 1, "only the eth1 inbound :80 case hit");
    }

    #[test]
    fn qualified_rule_isolation_between_interfaces() {
        // eth0 denies inbound TCP :22; eth1 denies inbound UDP any port.
        let mut state = FirewallState::new();
        load(
            &mut state,
            &[
                qualified_rule(RuleAction::Deny, Proto::Tcp, Direction::Inbound, 22, 0),
                qualified_rule(RuleAction::Deny, Proto::Udp, Direction::Inbound, 0, 1),
            ],
        );
        // Cross-traffic flows: each deny applies only to its own interface.
        assert!(state.decide(
            Direction::Inbound,
            Proto::Tcp,
            22,
            9,
            1,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert!(state.decide(
            Direction::Inbound,
            Proto::Udp,
            53,
            9,
            0,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert!(!state.decide(
            Direction::Inbound,
            Proto::Tcp,
            22,
            9,
            0,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert!(!state.decide(
            Direction::Inbound,
            Proto::Udp,
            53,
            9,
            1,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert_eq!(state.inbound_denied_total, 2);
    }

    #[test]
    fn qualified_first_match_wins_across_interfaces() {
        // Per-interface scoping re-enables traffic the global deny would
        // block, when an earlier allow rule names the receiving interface.
        let mut state = FirewallState::new();
        load(
            &mut state,
            &[
                qualified_rule(RuleAction::Allow, Proto::Tcp, Direction::Inbound, 8080, 1),
                rule(RuleAction::Deny, Proto::Tcp, Direction::Inbound, 0),
            ],
        );
        // eth1 :8080 hits the qualified allow (first match wins).
        assert!(state.decide(
            Direction::Inbound,
            Proto::Tcp,
            8080,
            9,
            1,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        // eth0 :8080 skips the qualified allow, falls to the global deny.
        assert!(!state.decide(
            Direction::Inbound,
            Proto::Tcp,
            8080,
            9,
            0,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        // eth1 other port falls to the global deny too.
        assert!(!state.decide(
            Direction::Inbound,
            Proto::Tcp,
            9999,
            9,
            1,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert_eq!(state.rules[0].hits, 1);
        assert_eq!(state.rules[1].hits, 2);
    }

    #[test]
    fn qualified_replace_all_and_reply_round_trip() {
        let mut state = FirewallState::new();
        load(
            &mut state,
            &[
                qualified_rule(RuleAction::Deny, Proto::Udp, Direction::Outbound, 53, 0),
                rule(RuleAction::Allow, Proto::Any, Direction::Inbound, 443),
            ],
        );
        let mut words = [0u64; 32];
        let used = state.encode_reply(&mut words);
        assert_eq!(used, 4 + 2 * 2);
        // Qualified rule word round-trips through the reply word.
        let reply_rule = FirewallRule::unpack(words[4], true).unwrap();
        assert_eq!(reply_rule.interface, Some(0));
        assert_eq!(reply_rule.proto, Proto::Udp);
        // Unqualified rule word stays zero in the qualifier field.
        let plain_rule = FirewallRule::unpack(words[6], true).unwrap();
        assert_eq!(plain_rule.interface, None);
        // Re-feeding the reply words back must reproduce the same table.
        let mut replay = FirewallState::new();
        let request_words = [words[4], 1, words[6], 1];
        assert!(replay.replace_all(&request_words, 2).is_some());
        assert_eq!(replay.rules[0].interface, Some(0));
        assert_eq!(replay.rules[1].interface, None);
        assert!(!replay.decide(
            Direction::Outbound,
            Proto::Udp,
            0,
            53,
            0,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert!(replay.decide(
            Direction::Outbound,
            Proto::Udp,
            0,
            53,
            1,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
    }

    #[test]
    fn qualified_rule_for_unknown_interface_never_matches() {
        // Rules may name interfaces that do not exist yet; they simply never
        // match (mirrors "interface not present" semantics, not an error).
        let mut state = FirewallState::new();
        load(
            &mut state,
            &[qualified_rule(
                RuleAction::Deny,
                Proto::Any,
                Direction::Inbound,
                0,
                7,
            )],
        );
        assert!(state.decide(
            Direction::Inbound,
            Proto::Tcp,
            22,
            9,
            0,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
        assert!(state.decide(
            Direction::Inbound,
            Proto::Tcp,
            22,
            9,
            1,
            RemoteAddress::V4([10, 0, 0, 1])
        ));
    }

    // --- address sets ---

    const FD00_BASE: [u8; 16] = {
        let mut octets = [0u8; 16];
        octets[0] = 0xfd;
        octets
    };

    fn cidr4(address: [u8; 4], prefix: u8) -> Cidr {
        Cidr::v4(address, prefix).unwrap()
    }

    fn cidr6(address: [u8; 16], prefix: u8) -> Cidr {
        Cidr::v6(address, prefix).unwrap()
    }

    fn rule_with_set(
        action: RuleAction,
        proto: Proto,
        direction: Direction,
        port: u16,
        set: u8,
    ) -> FirewallRule {
        FirewallRule {
            addr_set: Some(set),
            ..rule(action, proto, direction, port)
        }
    }

    #[test]
    fn addr_set_v4_cidr_contains_matrix() {
        let network = cidr4([10, 0, 0, 0], 8);
        assert!(network.contains(RemoteAddress::V4([10, 1, 2, 3])));
        assert!(!network.contains(RemoteAddress::V4([11, 0, 0, 1])));
        let host = cidr4([10, 1, 2, 3], 32);
        assert!(host.contains(RemoteAddress::V4([10, 1, 2, 3])));
        assert!(!host.contains(RemoteAddress::V4([10, 1, 2, 4])));
        let everything = cidr4([77, 77, 77, 77], 0);
        assert!(everything.contains(RemoteAddress::V4([0, 0, 0, 0])));
        assert!(everything.contains(RemoteAddress::V4([255, 255, 255, 255])));
        // Family mismatch never matches, even for a /0.
        assert!(!everything.contains(RemoteAddress::V6([0x20; 16])));
        // Prefix beyond 32 is invalid for v4.
        assert!(Cidr::v4([10, 0, 0, 0], 33).is_none());
    }

    #[test]
    fn addr_set_v6_cidr_contains_matrix() {
        let network = cidr6(FD00_BASE, 8);
        let mut inside = FD00_BASE;
        inside[15] = 1;
        assert!(network.contains(RemoteAddress::V6(inside)));
        let mut outside = FD00_BASE;
        outside[0] = 0xfe;
        assert!(!network.contains(RemoteAddress::V6(outside)));
        let host = cidr6(inside, 128);
        assert!(host.contains(RemoteAddress::V6(inside)));
        assert!(!host.contains(RemoteAddress::V6(FD00_BASE)));
        let everything = cidr6([0u8; 16], 0);
        assert!(everything.contains(RemoteAddress::V6(inside)));
        // Family mismatch never matches, even for a /0.
        assert!(!everything.contains(RemoteAddress::V4([10, 0, 0, 1])));
        // Prefix beyond 128 is invalid for v6.
        assert!(Cidr::v6([0u8; 16], 129).is_none());
    }

    #[test]
    fn cidr_word_codec_roundtrip() {
        let v4 = cidr4([10, 1, 2, 3], 24);
        assert_eq!(Cidr::from_words(v4.to_words()), Some(v4));
        let v6 = cidr6(FD00_BASE, 64);
        assert_eq!(Cidr::from_words(v6.to_words()), Some(v6));
        // Family byte outside {0, 1} is malformed.
        assert!(Cidr::from_words([2, 0x0a01_0203, 0]).is_none());
        // v4 prefix 33 and v6 prefix 129 are malformed.
        assert!(Cidr::from_words([33 << 8, 0x0a01_0203, 0]).is_none());
        assert!(Cidr::from_words([1 | (129 << 8), 0, 1]).is_none());
    }

    #[test]
    fn addr_set_define_replace_clear_roundtrip() {
        let mut state = FirewallState::new();
        assert!(!state.addr_set_defined(1));
        assert!(state.define_addr_set(1, &[cidr4([10, 0, 0, 0], 8)]));
        assert!(state.addr_set_defined(1));
        // Replace overwrites the previous entries wholesale.
        assert!(state.define_addr_set(1, &[cidr4([192, 168, 0, 0], 16)]));
        // Ids beyond the cap and entry overflow are rejected.
        assert!(!state.define_addr_set(9, &[]));
        assert!(!state.define_addr_set(
            1,
            &[
                cidr4([10, 0, 0, 0], 8),
                cidr4([11, 0, 0, 0], 8),
                cidr4([12, 0, 0, 0], 8),
                cidr4([13, 0, 0, 0], 8),
                cidr4([14, 0, 0, 0], 8),
            ],
        ));
        // A zero-entry define is valid (defined-but-empty set).
        assert!(state.define_addr_set(2, &[]));
        assert!(state.addr_set_defined(2));
        // Clear-all succeeds while no rule references the sets.
        assert!(state.clear_addr_sets());
        assert!(!state.addr_set_defined(1));
        assert!(!state.addr_set_defined(2));
    }

    #[test]
    fn rule_with_addr_set_match_matrix() {
        let mut state = FirewallState::new();
        assert!(state.define_addr_set(2, &[cidr4([10, 0, 0, 0], 8), cidr6(FD00_BASE, 8)],));
        load(
            &mut state,
            &[rule_with_set(
                RuleAction::Deny,
                Proto::Tcp,
                Direction::Outbound,
                80,
                2,
            )],
        );
        // v4 set member with matching port -> denied.
        assert!(!state.decide(
            Direction::Outbound,
            Proto::Tcp,
            0,
            80,
            0,
            RemoteAddress::V4([10, 1, 2, 3]),
        ));
        // v4 non-member -> skips the rule (allowed).
        assert!(state.decide(
            Direction::Outbound,
            Proto::Tcp,
            0,
            80,
            0,
            RemoteAddress::V4([11, 0, 0, 1]),
        ));
        // v6 set member -> denied.
        let mut member6 = FD00_BASE;
        member6[15] = 9;
        assert!(!state.decide(
            Direction::Outbound,
            Proto::Tcp,
            0,
            80,
            0,
            RemoteAddress::V6(member6),
        ));
        // v6 non-member -> allowed.
        let mut other6 = FD00_BASE;
        other6[0] = 0xfe;
        assert!(state.decide(
            Direction::Outbound,
            Proto::Tcp,
            0,
            80,
            0,
            RemoteAddress::V6(other6),
        ));
        // Port mismatch -> rule does not apply.
        assert!(state.decide(
            Direction::Outbound,
            Proto::Tcp,
            0,
            81,
            0,
            RemoteAddress::V4([10, 1, 2, 3]),
        ));
        assert_eq!(state.rules[0].hits, 2);
    }

    #[test]
    fn rule_without_addr_set_ignores_remote_address() {
        // Legacy unqualified rules match any remote address.
        let mut state = FirewallState::new();
        load(
            &mut state,
            &[rule(RuleAction::Deny, Proto::Tcp, Direction::Inbound, 22)],
        );
        assert!(!state.decide(
            Direction::Inbound,
            Proto::Tcp,
            22,
            44_000,
            0,
            RemoteAddress::V4([203, 0, 113, 9]),
        ));
        assert!(!state.decide(
            Direction::Inbound,
            Proto::Tcp,
            22,
            44_000,
            0,
            RemoteAddress::V6(FD00_BASE),
        ));
    }

    #[test]
    fn rule_with_empty_set_never_matches() {
        let mut state = FirewallState::new();
        assert!(state.define_addr_set(1, &[]));
        load(
            &mut state,
            &[rule_with_set(
                RuleAction::Deny,
                Proto::Tcp,
                Direction::Inbound,
                22,
                1,
            )],
        );
        assert!(state.decide(
            Direction::Inbound,
            Proto::Tcp,
            22,
            44_000,
            0,
            RemoteAddress::V4([10, 0, 0, 1]),
        ));
        assert_eq!(state.inbound_denied_total, 0);
    }

    #[test]
    fn replace_all_rejects_rule_referencing_undefined_set() {
        let mut state = FirewallState::new();
        assert!(state.define_addr_set(1, &[cidr4([10, 0, 0, 0], 8)]));
        // Set id 5 was never defined -> the whole replace is rejected and
        // the table rolls back to clean (defaults apply).
        let mut packed = Vec::new();
        let words = rule_with_set(RuleAction::Deny, Proto::Tcp, Direction::Inbound, 22, 5).pack();
        packed.push(words[0]);
        packed.push(1);
        assert!(state.replace_all(&packed, 1).is_none());
        assert_eq!(state.rule_count, 0);
        assert!(state.decide(
            Direction::Inbound,
            Proto::Tcp,
            22,
            44_000,
            0,
            RemoteAddress::V4([10, 0, 0, 1]),
        ));
        // With the set defined the same table loads.
        assert!(state.define_addr_set(5, &[]));
        assert!(state.replace_all(&packed, 1).is_some());
        assert_eq!(state.rule_count, 1);
    }

    #[test]
    fn qualifier_set_id_roundtrips_and_reserved_namespaces_rejected() {
        let mut state = FirewallState::new();
        assert!(state.define_addr_set(3, &[]));
        let qualified = rule_with_set(RuleAction::Allow, Proto::Udp, Direction::Inbound, 53, 3);
        let packed = qualified.pack();
        assert_eq!(packed[0] >> 48, 0x0100 | 3);
        let unpacked = FirewallRule::unpack(packed[0], true).unwrap();
        assert_eq!(unpacked.addr_set, Some(3));
        assert_eq!(unpacked.interface, None);
        // Reserved qualifier namespaces are malformed.
        assert!(FirewallRule::unpack(0x0200_0000_0000_0000, true).is_none());
        assert!(FirewallRule::unpack(0x1234_0000_0000_0000, true).is_none());
        // Legacy interface qualifier keeps decoding unchanged.
        let legacy = FirewallRule::unpack(0x0001_0000_0000_0000, true).unwrap();
        assert_eq!(legacy.interface, Some(0));
        assert_eq!(legacy.addr_set, None);
        // Set ids beyond the service cap are rejected at replace time.
        let over = rule_with_set(RuleAction::Allow, Proto::Udp, Direction::Inbound, 53, 9);
        let mut packed_over = Vec::new();
        packed_over.push(over.pack()[0]);
        packed_over.push(1);
        assert!(state.replace_all(&packed_over, 1).is_none());
    }

    #[test]
    fn clear_all_addr_sets_rejected_while_referenced() {
        let mut state = FirewallState::new();
        assert!(state.define_addr_set(1, &[]));
        load(
            &mut state,
            &[rule_with_set(
                RuleAction::Deny,
                Proto::Tcp,
                Direction::Inbound,
                22,
                1,
            )],
        );
        assert!(!state.clear_addr_sets());
        assert!(state.addr_set_defined(1));
        // After the referencing rules are gone the clear succeeds.
        state.clear_rules();
        assert!(state.clear_addr_sets());
        assert!(!state.addr_set_defined(1));
    }
}

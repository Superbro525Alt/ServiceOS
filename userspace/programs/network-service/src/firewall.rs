//! Firewall rule table: ordered allow/deny rules over (protocol, direction,
//! port, interface) with per-rule hit counters and a configurable default
//! inbound policy. Pure decision logic, host-unit-testable.

use crate::consts::MAX_FIREWALL_RULES;

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
        let subject_port = match direction {
            Direction::Inbound => local_port,
            Direction::Outbound => remote_port,
        };
        self.port == 0 || self.port == subject_port
    }

    /// Pack for IPC reply: action | proto<<8 | direction<<16 | enabled<<24 |
    /// port<<32 | interface-qualifier<<48; hits ride in the following word.
    /// The qualifier field holds 0 for "any interface" or `index + 1` for a
    /// rule pinned to one interface, so legacy zero words decode as
    /// unqualified.
    pub(crate) fn pack(&self) -> [u64; 2] {
        [
            self.action.word()
                | (self.proto.word() << 8)
                | (self.direction.word() << 16)
                | ((self.enabled as u64) << 24)
                | ((self.port as u64) << 32)
                | (self.interface.map_or(0, |interface| interface as u64 + 1) << 48),
            self.hits,
        ]
    }

    /// Decode from one packed request word (action/proto/direction/port and
    /// the trailing interface qualifier at bits [48..64); 0 = any interface,
    /// else `index + 1`); `enabled` comes from the caller's flag word.
    pub(crate) fn unpack(word: u64, enabled: bool) -> Option<Self> {
        let qualifier = (word >> 48) as u16;
        Some(Self {
            action: RuleAction::from_word(word & 0xff)?,
            proto: Proto::from_word((word >> 8) & 0xff)?,
            direction: Direction::from_word((word >> 16) & 0xff)?,
            port: (word >> 32) as u16,
            interface: qualifier.checked_sub(1),
            enabled,
            hits: 0,
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) struct FirewallState {
    pub(crate) rules: [FirewallRule; MAX_FIREWALL_RULES],
    pub(crate) rule_count: usize,
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
            default_inbound_allow: true,
            inbound_denied_total: 0,
            outbound_denied_total: 0,
        }
    }

    /// First-match-wins decision over enabled rules; falls back to the
    /// default policy (deny-able only for inbound). Increments counters.
    /// `iface_index` is the 0-based interface the traffic rides on; rules
    /// pinned to another interface (or unqualified rules) are the only
    /// candidates besides it.
    pub(crate) fn decide(
        &mut self,
        direction: Direction,
        proto: Proto,
        local_port: u16,
        remote_port: u16,
        iface_index: u16,
    ) -> bool {
        let index = self.rules[..self.rule_count]
            .iter()
            .position(|rule| rule.matches(direction, proto, local_port, remote_port, iface_index));
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
    /// then 2 words per rule (the rule word carries the interface qualifier
    /// at bits [48..64), so qualified rules round-trip additively; the
    /// global deny counters stay summary-wide — no trailing budget exists
    /// for per-interface counters at IPC_MAX_WORDS=16 with 6 rules).
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
        assert!(state.decide(Direction::Inbound, Proto::Tcp, 80, 44_000, 0));
        assert!(state.decide(Direction::Outbound, Proto::Tcp, 0, 80, 0));
        assert_eq!(state.inbound_denied_total, 0);
    }

    #[test]
    fn empty_table_default_inbound_deny() {
        let mut state = FirewallState::new();
        state.set_default_inbound_allow(false);
        assert!(!state.decide(Direction::Inbound, Proto::Tcp, 22, 44_000, 0));
        assert_eq!(state.inbound_denied_total, 1);
        assert!(
            state.decide(Direction::Outbound, Proto::Udp, 53_000, 53, 0),
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
                state.decide(Direction::Inbound, proto, port, 40_000, 0),
                expected,
                "proto={proto:?} port={port}"
            );
        }
        // Outbound same port/proto passes (direction mismatch).
        assert!(state.decide(Direction::Outbound, Proto::Tcp, 0, 80, 0));
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
        assert!(!state.decide(Direction::Outbound, Proto::Udp, 50_000, 53, 0));
        assert!(!state.decide(Direction::Outbound, Proto::Udp, 50_000, 123, 0));
        assert!(state.decide(Direction::Outbound, Proto::Tcp, 0, 80, 0));
        assert!(state.decide(Direction::Inbound, Proto::Udp, 53, 40_000, 0));
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
        assert!(state.decide(Direction::Inbound, Proto::Tcp, 8080, 9, 0));
        assert!(!state.decide(Direction::Inbound, Proto::Tcp, 9999, 9, 0));
        assert_eq!(state.rules[0].hits, 1);
        assert_eq!(state.rules[1].hits, 1);
    }

    #[test]
    fn disabled_rules_skipped() {
        let mut state = FirewallState::new();
        let mut denied = rule(RuleAction::Deny, Proto::Tcp, Direction::Inbound, 80);
        denied.enabled = false;
        load(&mut state, &[denied]);
        assert!(state.decide(Direction::Inbound, Proto::Tcp, 80, 9, 0));
        assert_eq!(state.rules[0].hits, 0);
    }

    #[test]
    fn icmp_rules_need_any_port() {
        let mut state = FirewallState::new();
        load(
            &mut state,
            &[rule(RuleAction::Deny, Proto::Icmp, Direction::Outbound, 0)],
        );
        assert!(!state.decide(Direction::Outbound, Proto::Icmp, 0, 0, 0));
        assert!(!state.decide(Direction::Outbound, Proto::Icmp, 0, 0, 0));
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
        assert!(!state.decide(Direction::Outbound, Proto::Tcp, 0, 25, 0));
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
        assert!(state.decide(Direction::Inbound, Proto::Tcp, 80, 9, 0));
        // eth1 (index 1): the rule matches -> deny.
        assert!(!state.decide(Direction::Inbound, Proto::Tcp, 80, 9, 1));
        // eth1, other port/proto: rule misses, default allows.
        assert!(state.decide(Direction::Inbound, Proto::Tcp, 81, 9, 1));
        assert!(state.decide(Direction::Inbound, Proto::Udp, 80, 9, 1));
        // eth1 outbound: direction mismatch, default (outbound always allow).
        assert!(state.decide(Direction::Outbound, Proto::Tcp, 0, 80, 1));
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
        assert!(state.decide(Direction::Inbound, Proto::Tcp, 22, 9, 1));
        assert!(state.decide(Direction::Inbound, Proto::Udp, 53, 9, 0));
        assert!(!state.decide(Direction::Inbound, Proto::Tcp, 22, 9, 0));
        assert!(!state.decide(Direction::Inbound, Proto::Udp, 53, 9, 1));
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
        assert!(state.decide(Direction::Inbound, Proto::Tcp, 8080, 9, 1));
        // eth0 :8080 skips the qualified allow, falls to the global deny.
        assert!(!state.decide(Direction::Inbound, Proto::Tcp, 8080, 9, 0));
        // eth1 other port falls to the global deny too.
        assert!(!state.decide(Direction::Inbound, Proto::Tcp, 9999, 9, 1));
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
        assert!(!replay.decide(Direction::Outbound, Proto::Udp, 0, 53, 0));
        assert!(replay.decide(Direction::Outbound, Proto::Udp, 0, 53, 1));
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
        assert!(state.decide(Direction::Inbound, Proto::Tcp, 22, 9, 0));
        assert!(state.decide(Direction::Inbound, Proto::Tcp, 22, 9, 1));
    }
}

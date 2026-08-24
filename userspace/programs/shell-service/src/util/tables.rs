use core::fmt::Write;

use serviceos_userspace_runtime as rt;

use rt::{LogDomain, LogSeverity, ServiceId, StatusHealth};

use crate::util::FixedValueText;

/// Tick frequency of the kernel monotonic clock (100 Hz).
pub(crate) const TICK_HZ: u64 = 100;
/// `logs follow` gives up after this much silence so a graphical-terminal
/// session cannot wedge a pane forever when no interrupt can be delivered.
pub(crate) const FOLLOW_IDLE_TIMEOUT_TICKS: u64 = 30 * TICK_HZ;
/// Hard cap on streamed records per `logs follow` invocation.
pub(crate) const FOLLOW_MAX_RECORDS: usize = 512;
/// Hard cap on crash rows listed by one `logs crashes` invocation.
pub(crate) const MAX_CRASH_ROWS: usize = 16;
/// Words in the status-service snapshot rollup reply.
pub(crate) const ROLLUP_WORDS: usize = 16;
const MAX_ROLLUP_LISTED: usize = 2;

pub(crate) fn health_name(health: StatusHealth) -> &'static str {
    match health {
        StatusHealth::Healthy => "healthy",
        StatusHealth::Degraded => "degraded",
        StatusHealth::Failing => "failing",
        StatusHealth::Recovering => "recovering",
        StatusHealth::Dormant => "dormant",
        StatusHealth::Unknown => "unknown",
    }
}

pub(crate) fn detail_kind_name(kind: u32) -> &'static str {
    match kind {
        x if x == rt::status_detail_kind::LIFECYCLE => "lifecycle",
        x if x == rt::status_detail_kind::BLOCKED_DEPENDENCY => "blocked",
        x if x == rt::status_detail_kind::RESTART_BACKOFF => "backoff",
        x if x == rt::status_detail_kind::HEARTBEAT => "heartbeat",
        _ => "none",
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CrashShape {
    pub severity_word: u64,
    pub event_word: u64,
}

impl CrashShape {
    /// Mirrors log-service's own crash predicate (`CrashRecord::is_crash`):
    /// error-or-worse severity, kernel traps, or service-failure events.
    pub(crate) fn is_crash(self) -> bool {
        self.severity_word >= LogSeverity::Error as u64
            || self.event_word == 70 // LogEvent::KernelTrap
            || self.event_word == 3 // LogEvent::ServiceFailed
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct HealthRollup {
    pub heartbeats: u64,
    pub last_tick: u64,
    pub total: usize,
    pub unknown: usize,
    pub healthy: usize,
    pub degraded: usize,
    pub failing: usize,
    pub recovering: usize,
    pub dormant: usize,
    pub restarting_count: usize,
    pub degraded_ids: [u32; MAX_ROLLUP_LISTED],
    pub degraded_len: usize,
    pub restarting_ids: [u32; MAX_ROLLUP_LISTED],
    pub restarting_len: usize,
    pub offenders: [(u32, u64); MAX_ROLLUP_LISTED],
}

impl HealthRollup {
    pub(crate) const fn problem_count(&self) -> usize {
        self.degraded + self.failing
    }

    pub(crate) fn worst_offender_label(&self) -> FixedValueText {
        let mut label = FixedValueText::empty();
        let Some((worst_id, worst_restarts)) =
            self.offenders.iter().copied().find(|(id, _)| *id != 0)
        else {
            let _ = write!(&mut label, "-");
            return label;
        };
        let _ = write!(
            &mut label,
            "{}({} restarts)",
            service_cell(worst_id as u64),
            worst_restarts,
        );
        label
    }
}

/// Parses the 16-word snapshot rollup reply published by status-service
/// (`fill_snapshot_reply`). Returns `None` when the reply is too short to be
/// a rollup-era snapshot.
pub(crate) fn parse_health_rollup(words: &[u64]) -> Option<HealthRollup> {
    if words.len() < ROLLUP_WORDS {
        return None;
    }
    let count_at = |slot: usize| words[3 + slot] as usize;
    Some(HealthRollup {
        heartbeats: words[0],
        last_tick: words[1],
        total: words[2] as usize,
        unknown: count_at(0),
        healthy: count_at(1),
        degraded: count_at(2),
        failing: count_at(3),
        recovering: count_at(4),
        dormant: count_at(5),
        restarting_count: words[9] as usize,
        degraded_ids: unpack_id_pair(words[11]),
        degraded_len: words[10] as usize,
        restarting_ids: unpack_id_pair(words[13]),
        restarting_len: words[12] as usize,
        offenders: [unpack_pair(words[14]), unpack_pair(words[15])],
    })
}

fn unpack_pair(word: u64) -> (u32, u64) {
    (word as u32, word >> 32)
}

fn unpack_id_pair(word: u64) -> [u32; MAX_ROLLUP_LISTED] {
    [word as u32, (word >> 32) as u32]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FollowStop {
    Continue,
    Interrupted,
    OperatorLine,
    IdleTimeout,
    RecordCap,
}

/// Pure follow-loop stop decision given whether anything happened this pass.
pub(crate) fn follow_stop(
    input_event: Option<FollowStop>,
    records_seen: usize,
    idle_ticks: u64,
) -> FollowStop {
    if let Some(event) = input_event {
        return event;
    }
    if records_seen >= FOLLOW_MAX_RECORDS {
        return FollowStop::RecordCap;
    }
    if idle_ticks >= FOLLOW_IDLE_TIMEOUT_TICKS {
        return FollowStop::IdleTimeout;
    }
    FollowStop::Continue
}

pub(crate) fn severity_from_word(value: u64) -> LogSeverity {
    match value {
        1 => LogSeverity::Trace,
        2 => LogSeverity::Debug,
        4 => LogSeverity::Warn,
        5.. => LogSeverity::Error,
        _ => LogSeverity::Info,
    }
}

pub(crate) fn domain_from_word(value: u64) -> Option<LogDomain> {
    let domain = match value {
        1 => LogDomain::Bootstrap,
        2 => LogDomain::ServiceManager,
        3 => LogDomain::Service,
        4 => LogDomain::Storage,
        5 => LogDomain::Log,
        6 => LogDomain::Config,
        7 => LogDomain::Console,
        8 => LogDomain::Status,
        9 => LogDomain::Ipc,
        10 => LogDomain::Shell,
        11 => LogDomain::Package,
        12 => LogDomain::Network,
        13 => LogDomain::Graphics,
        14 => LogDomain::Session,
        15 => LogDomain::Desktop,
        16 => LogDomain::App,
        17 => LogDomain::Audio,
        18 => LogDomain::Runtime,
        19 => LogDomain::Developer,
        20 => LogDomain::Security,
        21 => LogDomain::Kernel,
        _ => return None,
    };
    Some(domain)
}

/// Event names keyed by their ABI word values (LogEvent is `repr(u32)` and
/// contiguous). Unknown words render as `event-<n>` by the caller.
pub(crate) fn event_name_from_word(value: u64) -> Option<&'static str> {
    Some(match value {
        1 => "service-started",
        2 => "service-ready",
        3 => "service-failed",
        4 => "service-restarting",
        5 => "config-loaded",
        6 => "config-read",
        7 => "console-write",
        8 => "status-started",
        9 => "status-heartbeat",
        10 => "lookup-granted",
        11 => "storage-mounted",
        12 => "manifest-loaded",
        13 => "resource-opened",
        14 => "session-opened",
        15 => "shell-command",
        16 => "tool-launched",
        17 => "package-catalog-loaded",
        18 => "package-installed",
        19 => "package-updated",
        20 => "package-removed",
        21 => "package-rolled-back",
        22 => "package-activation-failed",
        23 => "network-interface-ready",
        24 => "network-address-configured",
        25 => "network-resolve-completed",
        26 => "network-probe-completed",
        27 => "network-link-changed",
        28 => "display-output-ready",
        29 => "surface-created",
        30 => "surface-updated",
        31 => "compositor-presented",
        32 => "session-ready",
        33 => "session-focus-changed",
        34 => "desktop-ready",
        35 => "desktop-app-launched",
        36 => "desktop-app-exited",
        37 => "desktop-focus-changed",
        38 => "app-rendered",
        39 => "input-source-ready",
        40 => "input-key-delivered",
        41 => "network-lease-changed",
        42 => "network-socket-opened",
        43 => "network-socket-closed",
        44 => "terminal-session-opened",
        45 => "terminal-session-closed",
        46 => "audio-endpoint-ready",
        47 => "audio-stream-opened",
        48 => "audio-stream-started",
        49 => "audio-stream-stopped",
        50 => "audio-stream-closed",
        51 => "runtime-environment-created",
        52 => "runtime-environment-destroyed",
        53 => "runtime-launch-started",
        54 => "runtime-launch-exited",
        55 => "runtime-mapped-read",
        56 => "developer-catalog-loaded",
        57 => "developer-build-started",
        58 => "developer-build-finished",
        59 => "developer-build-failed",
        60 => "developer-artifact-opened",
        61 => "package-repository-added",
        62 => "package-repository-synced",
        63 => "package-repository-sync-failed",
        64 => "package-repair-completed",
        65 => "package-garbage-collected",
        66 => "security-policy-changed",
        67 => "security-launch-denied",
        68 => "runtime-approval-pending",
        69 => "runtime-approval-changed",
        70 => "kernel-trap",
        _ => return None,
    })
}

pub(crate) fn service_cell(word: u64) -> FixedValueText {
    let mut text = FixedValueText::empty();
    let name = match word {
        1 => Some("root-manager"),
        2 => Some("storage-service"),
        3 => Some("console-service"),
        4 => Some("config-service"),
        5 => Some("log-service"),
        6 => Some("status-service"),
        7 => Some("shell-service"),
        8 => Some("package-service"),
        9 => Some("announce-service"),
        10 => Some("network-service"),
        11 => Some("graphics-service"),
        12 => Some("session-service"),
        13 => Some("desktop-shell-service"),
        14 => Some("terminal-service"),
        15 => Some("audio-service"),
        16 => Some("runtime-service"),
        17 => Some("developer-service"),
        18 => Some("clipboard-service"),
        19 => Some("security-service"),
        _ => None,
    };
    match name {
        Some(name) => {
            let _ = write!(&mut text, "{name}");
        }
        None => {
            let _ = write!(&mut text, "svc-{}", word as u32);
        }
    }
    text
}

pub(crate) fn severity_cell(word: u64) -> &'static str {
    match severity_from_word(word) {
        LogSeverity::Trace => "trace",
        LogSeverity::Debug => "debug",
        LogSeverity::Info => "info",
        LogSeverity::Warn => "warn",
        LogSeverity::Error => "error",
    }
}

pub(crate) fn ps_state_cell(running: bool, focused: bool) -> &'static str {
    match (running, focused) {
        (true, true) => "focused",
        (true, false) => "running",
        (false, _) => "idle",
    }
}

/// Parses a `logs follow <domain|service>` filter word into a log domain.
pub(crate) fn parse_domain_word(word: &str) -> Option<LogDomain> {
    Some(match word {
        "bootstrap" => LogDomain::Bootstrap,
        "manager" | "service-manager" => LogDomain::ServiceManager,
        "service" | "services" => LogDomain::Service,
        "storage" => LogDomain::Storage,
        "log" => LogDomain::Log,
        "config" => LogDomain::Config,
        "console" => LogDomain::Console,
        "status" => LogDomain::Status,
        "ipc" => LogDomain::Ipc,
        "shell" => LogDomain::Shell,
        "package" | "packages" => LogDomain::Package,
        "network" | "net" => LogDomain::Network,
        "graphics" | "gfx" => LogDomain::Graphics,
        "session" => LogDomain::Session,
        "desktop" => LogDomain::Desktop,
        "app" | "apps" => LogDomain::App,
        "audio" => LogDomain::Audio,
        "runtime" => LogDomain::Runtime,
        "developer" | "dev" => LogDomain::Developer,
        "security" => LogDomain::Security,
        "kernel" => LogDomain::Kernel,
        _ => return None,
    })
}

pub(crate) fn service_id_from_word(value: u64) -> Option<ServiceId> {
    let id = match value {
        1 => ServiceId::RootManager,
        2 => ServiceId::Storage,
        3 => ServiceId::Console,
        4 => ServiceId::Config,
        5 => ServiceId::Log,
        6 => ServiceId::Status,
        7 => ServiceId::Shell,
        8 => ServiceId::Package,
        9 => ServiceId::Announce,
        10 => ServiceId::Network,
        11 => ServiceId::Graphics,
        12 => ServiceId::Session,
        13 => ServiceId::DesktopShell,
        14 => ServiceId::Terminal,
        15 => ServiceId::Audio,
        16 => ServiceId::Runtime,
        17 => ServiceId::Developer,
        18 => ServiceId::Clipboard,
        19 => ServiceId::Security,
        _ => return None,
    };
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_shape_matches_log_service_predicate() {
        assert!(
            CrashShape {
                severity_word: 5,
                event_word: 1
            }
            .is_crash()
        );
        assert!(
            CrashShape {
                severity_word: 1,
                event_word: 70
            }
            .is_crash()
        );
        assert!(
            CrashShape {
                severity_word: 1,
                event_word: 3
            }
            .is_crash()
        );
        assert!(
            !CrashShape {
                severity_word: 4,
                event_word: 9
            }
            .is_crash()
        );
        assert!(
            !CrashShape {
                severity_word: 1,
                event_word: 15
            }
            .is_crash()
        );
    }

    #[test]
    fn rollup_parses_full_snapshot_words() {
        let mut words = [0u64; ROLLUP_WORDS];
        words[0] = 7; // heartbeats
        words[1] = 900; // last tick
        words[2] = 3; // total
        words[4] = 2; // healthy
        words[5] = 1; // degraded
        words[9] = 1; // restarting count
        words[10] = 1; // degraded len
        words[11] = pack(2, 13);
        words[13] = pack(14, 0);
        words[14] = pack(16, 4);
        let rollup = parse_health_rollup(&words).expect("rollup");
        assert_eq!(rollup.heartbeats, 7);
        assert_eq!(rollup.total, 3);
        assert_eq!(rollup.healthy, 2);
        assert_eq!(rollup.degraded, 1);
        assert_eq!(rollup.restarting_count, 1);
        assert_eq!(rollup.degraded_ids, [2, 13]);
        assert_eq!(rollup.offenders[0], (16, 4));
        assert_eq!(rollup.problem_count(), 1);
    }

    #[test]
    fn rollup_rejects_short_legacy_snapshots() {
        assert!(parse_health_rollup(&[1, 2, 3]).is_none());
        assert!(parse_health_rollup(&[0u64; 15]).is_none());
    }

    #[test]
    fn worst_offender_skips_empty_slots() {
        let rollup = HealthRollup {
            offenders: [(0, 0), (5, 2)],
            ..HealthRollup::default()
        };
        assert_eq!(
            rollup.worst_offender_label().to_string(),
            "log-service(2 restarts)"
        );

        let none = HealthRollup::default();
        assert_eq!(none.worst_offender_label().to_string(), "-");

        let unknown = HealthRollup {
            offenders: [(99, 1), (0, 0)],
            ..HealthRollup::default()
        };
        assert_eq!(
            unknown.worst_offender_label().to_string(),
            "svc-99(1 restarts)"
        );
    }

    #[test]
    fn follow_stop_precedence() {
        assert_eq!(
            follow_stop(Some(FollowStop::Interrupted), 0, 0),
            FollowStop::Interrupted
        );
        assert_eq!(
            follow_stop(None, FOLLOW_MAX_RECORDS, 0),
            FollowStop::RecordCap
        );
        assert_eq!(
            follow_stop(Some(FollowStop::OperatorLine), FOLLOW_MAX_RECORDS, 0),
            FollowStop::OperatorLine
        );
        assert_eq!(
            follow_stop(None, 3, FOLLOW_IDLE_TIMEOUT_TICKS),
            FollowStop::IdleTimeout
        );
        assert_eq!(
            follow_stop(None, 3, FOLLOW_IDLE_TIMEOUT_TICKS - 1),
            FollowStop::Continue
        );
    }

    #[test]
    fn word_decoders_match_abi_values() {
        assert_eq!(severity_from_word(5), LogSeverity::Error);
        assert_eq!(severity_from_word(3), LogSeverity::Info);
        assert_eq!(domain_from_word(21), Some(LogDomain::Kernel));
        assert_eq!(domain_from_word(22), None);
        assert_eq!(event_name_from_word(70), Some("kernel-trap"));
        assert_eq!(event_name_from_word(3), Some("service-failed"));
        assert_eq!(event_name_from_word(71), None);
        assert_eq!(service_cell(5).to_string(), "log-service");
        assert_eq!(severity_cell(4), "warn");
    }

    #[test]
    fn service_id_decoder_round_trips_known_words() {
        assert_eq!(service_id_from_word(6), Some(ServiceId::Status));
        assert_eq!(service_id_from_word(0), None);
        assert_eq!(service_id_from_word(20), None);
    }

    #[test]
    fn ps_state_reflects_running_and_focus() {
        assert_eq!(ps_state_cell(true, true), "focused");
        assert_eq!(ps_state_cell(true, false), "running");
        assert_eq!(ps_state_cell(false, true), "idle");
    }

    #[test]
    fn domain_words_parse_for_follow_filter() {
        assert_eq!(parse_domain_word("kernel"), Some(LogDomain::Kernel));
        assert_eq!(parse_domain_word("net"), Some(LogDomain::Network));
        assert_eq!(parse_domain_word("services"), Some(LogDomain::Service));
        assert_eq!(parse_domain_word("bogus"), None);
    }

    fn pack(first: u32, second: u32) -> u64 {
        first as u64 | ((second as u64) << 32)
    }
}

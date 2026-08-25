use core::cell::UnsafeCell;

use rt::PackageRepositoryTrustMode;
use serviceos_userspace_runtime as rt;

use crate::util::{ShellOutput, write_output_linef};

use super::parse::{MAX_PACKAGE_TEXT, trust_mode_name};

pub(crate) const HOST_ARCH: &str = "x86_64";
const MAX_ONBOARDED_SOURCES: usize = 16;
const UNIVERSAL_TAGS: [&str; 2] = ["any", "all"];
const FOREIGN_ARCH_TAGS: [&str; 6] = ["x86", "i686", "arm", "arm64", "aarch64", "riscv64"];

#[derive(Clone, Copy, Debug)]
pub(crate) enum CompatVerdict<'a> {
    Undeclared,
    Universal,
    Match,
    Mismatch { declared: &'a str },
}

pub(crate) fn compat_verdict(version: &str) -> CompatVerdict<'_> {
    let Some(tag) = declared_arch_tag(version) else {
        return CompatVerdict::Undeclared;
    };
    if UNIVERSAL_TAGS.contains(&tag) {
        return CompatVerdict::Universal;
    }
    if tag == HOST_ARCH {
        return CompatVerdict::Match;
    }
    if FOREIGN_ARCH_TAGS.contains(&tag) {
        return CompatVerdict::Mismatch { declared: tag };
    }
    CompatVerdict::Undeclared
}

fn declared_arch_tag(version: &str) -> Option<&str> {
    let (_, tag) = version.rsplit_once('+')?;
    if tag.is_empty() || tag.len() > 12 {
        return None;
    }
    if tag.bytes().any(|byte| !(byte.is_ascii_alphanumeric() || byte == b'_')) {
        return None;
    }
    Some(tag)
}

pub(crate) fn compat_requires_override(verdict: &CompatVerdict<'_>) -> bool {
    matches!(verdict, CompatVerdict::Mismatch { .. })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub(crate) enum SideLoadPolicy {
    Allow,
    #[default]
    Warn,
    Deny,
}

pub(crate) fn parse_side_load_policy(text: &str) -> Option<SideLoadPolicy> {
    match text {
        "allow" => Some(SideLoadPolicy::Allow),
        "warn" => Some(SideLoadPolicy::Warn),
        "deny" => Some(SideLoadPolicy::Deny),
        _ => None,
    }
}

pub(crate) fn side_load_policy_name(policy: SideLoadPolicy) -> &'static str {
    match policy {
        SideLoadPolicy::Allow => "allow",
        SideLoadPolicy::Warn => "warn",
        SideLoadPolicy::Deny => "deny",
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SideLoadDecision {
    Allow,
    AllowWithWarning,
    Deny,
}

pub(crate) fn side_load_decision(policy: SideLoadPolicy) -> SideLoadDecision {
    match policy {
        SideLoadPolicy::Allow => SideLoadDecision::Allow,
        SideLoadPolicy::Warn => SideLoadDecision::AllowWithWarning,
        SideLoadPolicy::Deny => SideLoadDecision::Deny,
    }
}

pub(crate) enum SourceGateDecision {
    Proceed,
    BlockedDisabled,
}

pub(crate) fn source_gate_decision(onboarded_state: Option<bool>) -> SourceGateDecision {
    match onboarded_state {
        Some(false) => SourceGateDecision::BlockedDisabled,
        _ => SourceGateDecision::Proceed,
    }
}

pub(crate) fn trust_meaning(mode: PackageRepositoryTrustMode) -> &'static str {
    match mode {
        PackageRepositoryTrustMode::Boot => "packages verify against the boot trust root",
        PackageRepositoryTrustMode::Unsigned => {
            "no signature evidence; package bytes are trusted as-fetched"
        }
        PackageRepositoryTrustMode::PinnedDigest => {
            "feed digest must equal your pinned digest on every sync"
        }
    }
}

pub(crate) fn trust_onboarding_impact(mode: PackageRepositoryTrustMode) -> &'static str {
    match mode {
        PackageRepositoryTrustMode::Boot => {
            "installs from this source run without per-install acknowledgement"
        }
        PackageRepositoryTrustMode::Unsigned => {
            "every install from this source needs --yes and is flagged unverified"
        }
        PackageRepositoryTrustMode::PinnedDigest => {
            "sync fails closed when the fetched digest differs from the pin"
        }
    }
}

pub(crate) struct RepoAddPlan<'a> {
    pub(super) name: &'a str,
    pub(super) url: &'a str,
    pub(super) trust_mode: PackageRepositoryTrustMode,
    pub(super) pinned_digest: u64,
}

pub(super) fn write_repo_review(output: ShellOutput, plan: &RepoAddPlan<'_>) -> rt::Result<()> {
    write_output_linef(
        output,
        format_args!("trust review for third-party repository {}", plan.name),
    )?;
    write_output_linef(
        output,
        format_args!(
            "  endpoint {} trust={} meaning: {}",
            plan.url,
            trust_mode_name(plan.trust_mode),
            trust_meaning(plan.trust_mode),
        ),
    )?;
    if plan.trust_mode == PackageRepositoryTrustMode::PinnedDigest {
        write_output_linef(
            output,
            format_args!("  pinned digest {:016x}", plan.pinned_digest),
        )?;
    }
    write_output_linef(
        output,
        format_args!(
            "  effect once added: {}",
            trust_onboarding_impact(plan.trust_mode)
        ),
    )?;
    write_output_linef(
        output,
        format_args!(
            "  packages from this source become installable and update-visible; manage it with pkg repo <enable|disable|remove|status>",
        ),
    )?;
    write_output_linef(
        output,
        format_args!("not committed; re-run with --yes to accept and register"),
    )
}

#[derive(Clone, Copy)]
pub(crate) struct OnboardedSource {
    name: [u8; MAX_PACKAGE_TEXT],
    name_len: usize,
    enabled: bool,
}

impl OnboardedSource {
    const fn empty() -> Self {
        Self {
            name: [0u8; MAX_PACKAGE_TEXT],
            name_len: 0,
            enabled: false,
        }
    }

    fn matches(&self, name: &[u8]) -> bool {
        self.name_len == name.len() && &self.name[..self.name_len] == name
    }

    fn set(&mut self, name: &[u8], enabled: bool) {
        let len = name.len();
        self.name[..len].copy_from_slice(name);
        self.name_len = len;
        self.enabled = enabled;
    }
}

struct OnboardingLedger {
    sources: [OnboardedSource; MAX_ONBOARDED_SOURCES],
    count: usize,
    side_load: SideLoadPolicy,
}

impl OnboardingLedger {
    const fn new() -> Self {
        Self {
            sources: [OnboardedSource::empty(); MAX_ONBOARDED_SOURCES],
            count: 0,
            side_load: SideLoadPolicy::Warn,
        }
    }

    fn encode_name(name: &str) -> Option<([u8; MAX_PACKAGE_TEXT], usize)> {
        let bytes = name.as_bytes();
        if bytes.is_empty() || bytes.len() > MAX_PACKAGE_TEXT {
            return None;
        }
        let mut buffer = [0u8; MAX_PACKAGE_TEXT];
        buffer[..bytes.len()].copy_from_slice(bytes);
        Some((buffer, bytes.len()))
    }

    fn record(&mut self, name: &str) -> rt::Result<()> {
        let Some((encoded, len)) = Self::encode_name(name) else {
            return Err(rt::Error::InvalidArgument);
        };
        if self.sources[..self.count].iter().any(|source| source.matches(&encoded[..len])) {
            return Err(rt::Error::Busy);
        }
        let slot = self.sources.get_mut(self.count).ok_or(rt::Error::CapacityExceeded)?;
        slot.set(&encoded[..len], true);
        self.count += 1;
        Ok(())
    }

    fn lookup(&self, name: &str) -> Option<bool> {
        let (_, len) = Self::encode_name(name)?;
        let encoded = &name.as_bytes()[..len];
        self.sources[..self.count]
            .iter()
            .find(|source| source.matches(encoded))
            .map(|source| source.enabled)
    }

    fn set_enabled(&mut self, name: &str, enabled: bool) -> bool {
        let Some((_, len)) = Self::encode_name(name) else {
            return false;
        };
        let encoded = &name.as_bytes()[..len];
        match self.sources[..self.count]
            .iter_mut()
            .find(|source| source.matches(encoded))
        {
            Some(source) => {
                source.enabled = enabled;
                true
            }
            None => false,
        }
    }

    fn remove(&mut self, name: &str) -> bool {
        let Some((_, len)) = Self::encode_name(name) else {
            return false;
        };
        let encoded = &name.as_bytes()[..len];
        let Some(position) = self.sources[..self.count]
            .iter()
            .position(|source| source.matches(encoded))
        else {
            return false;
        };
        for index in position..self.count - 1 {
            let moved = self.sources[index + 1];
            self.sources[index] = moved;
        }
        self.sources[self.count - 1] = OnboardedSource::empty();
        self.count -= 1;
        true
    }

    #[cfg(test)]
    fn reset(&mut self) {
        *self = Self::new();
    }
}

struct LedgerSlot(UnsafeCell<OnboardingLedger>);
unsafe impl Sync for LedgerSlot {}
static ONBOARDING_LEDGER: LedgerSlot = LedgerSlot(UnsafeCell::new(OnboardingLedger::new()));

fn ledger() -> &'static mut OnboardingLedger {
    // SAFETY: the shell task is strictly single-threaded (sessions-table
    // precedent); no concurrent access is possible.
    unsafe { &mut *ONBOARDING_LEDGER.0.get() }
}

pub(crate) fn onboard_record(name: &str) -> rt::Result<()> {
    ledger().record(name)
}

pub(crate) fn onboard_lookup(name: &str) -> Option<bool> {
    ledger().lookup(name)
}

pub(crate) fn onboard_set_enabled(name: &str, enabled: bool) -> bool {
    ledger().set_enabled(name, enabled)
}

pub(crate) fn onboard_remove(name: &str) -> bool {
    ledger().remove(name)
}

pub(crate) fn side_load_policy() -> SideLoadPolicy {
    ledger().side_load
}

pub(crate) fn set_side_load_policy(policy: SideLoadPolicy) {
    ledger().side_load = policy;
}

pub(crate) fn onboarded_count() -> usize {
    ledger().count
}

pub(crate) fn for_each_onboarded(mut visit: impl FnMut(&str, bool)) {
    let count = ledger().count;
    for index in 0..count {
        let source = &ledger().sources[index];
        let text = core::str::from_utf8(&source.name[..source.name_len]).unwrap_or("?");
        visit(text, source.enabled);
    }
}

pub(crate) fn sideload_image_gate(output: ShellOutput, path: &str) -> rt::Result<bool> {
    match side_load_decision(side_load_policy()) {
        SideLoadDecision::Allow => Ok(true),
        SideLoadDecision::AllowWithWarning => {
            write_output_linef(
                output,
                format_args!(
                    "warning: {path} runs as local side-loaded code (trust class: side-load); policy=warn",
                ),
            )?;
            Ok(true)
        }
        SideLoadDecision::Deny => {
            write_output_linef(
                output,
                format_args!(
                    "blocked: local-image launch is side-loaded code (trust class: side-load); policy=deny (pkg sideload policy warn)",
                ),
            )?;
            Ok(false)
        }
    }
}

pub(super) fn cmd_pkg_sideload<'a>(
    output: ShellOutput,
    mut parts: impl Iterator<Item = &'a str>,
) -> rt::Result<()> {
    match parts.next() {
        Some("policy") => match parts.next() {
            None => write_output_linef(
                output,
                format_args!(
                    "side-load policy {} (local-file installs/launches: allow=run, warn=prompt-free warning, deny=block)",
                    side_load_policy_name(side_load_policy()),
                ),
            ),
            Some(text) => match parse_side_load_policy(text) {
                Some(policy) => {
                    set_side_load_policy(policy);
                    write_output_linef(
                        output,
                        format_args!(
                            "side-load policy set to {}",
                            side_load_policy_name(policy),
                        ),
                    )
                }
                None => write_output_linef(
                    output,
                    format_args!("usage: pkg sideload policy [allow|warn|deny]"),
                ),
            },
        },
        _ => write_output_linef(output, format_args!("usage: pkg sideload policy [allow|warn|deny]")),
    }
}

pub(super) fn cmd_pkg_repo_set_enabled(
    output: ShellOutput,
    name: &str,
    enable: bool,
) -> rt::Result<()> {
    let verb = if enable { "enabled" } else { "disabled" };
    let found = onboard_set_enabled(name, enable);
    if !found {
        return write_output_linef(
            output,
            format_args!(
                "{name} is not in the onboarding ledger; onboard it with pkg repo add first"
            ),
        );
    }
    write_output_linef(output, format_args!("{verb} source {name}"))?;
    if !enable {
        write_output_linef(
            output,
            format_args!("installs and updates from {name} are now blocked until re-enabled"),
        )?;
    }
    Ok(())
}

pub(super) fn cmd_pkg_repo_remove(output: ShellOutput, name: &str) -> rt::Result<()> {
    if !onboard_remove(name) {
        return write_output_linef(
            output,
            format_args!("{name} is not in the onboarding ledger"),
        );
    }
    write_output_linef(output, format_args!("removed onboarding approval for {name}"))?;
    write_output_linef(
        output,
        format_args!(
            "note: the repository stays registered in package-service until removal support lands there; installs fall back to per-use --yes review",
        ),
    )
}

pub(super) fn cmd_pkg_repo_status(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    write_output_linef(
        output,
        format_args!(
            "side-load policy {} host-arch {} onboarded-sources {}",
            side_load_policy_name(side_load_policy()),
            HOST_ARCH,
            onboarded_count(),
        ),
    )?;
    let package_handle = rt::lookup_service(bootstrap, rt::ServiceId::Package)?;
    let mut matched = 0usize;
    for_each_onboarded(|name, enabled| {
        matched += 1;
        let state = super::mutate::find_source_repo(package_handle, name)
            .ok()
            .flatten();
        match state {
            Some(repo) => {
                let _ = write_output_linef(
                    output,
                    format_args!(
                        "{} {} trust={} service-state={}",
                        name,
                        if enabled { "enabled" } else { "disabled" },
                        trust_mode_name(repo.trust_mode),
                        if repo.enabled { "registered" } else { "service-disabled" },
                    ),
                );
            }
            None => {
                let _ = write_output_linef(
                    output,
                    format_args!(
                        "{} {} not-found-in-service-catalog",
                        name,
                        if enabled { "enabled" } else { "disabled" },
                    ),
                );
            }
        }
    });
    let _ = rt::handle_close(package_handle);
    if matched == 0 {
        write_output_linef(output, format_args!("no onboarded third-party sources"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_review_text_covers_all_trust_levels() {
        for mode in [
            PackageRepositoryTrustMode::Boot,
            PackageRepositoryTrustMode::Unsigned,
            PackageRepositoryTrustMode::PinnedDigest,
        ] {
            assert!(!trust_meaning(mode).is_empty());
            assert!(!trust_onboarding_impact(mode).is_empty());
        }
        assert_eq!(trust_mode_name(PackageRepositoryTrustMode::Boot), "boot");
    }

    #[test]
    fn source_gating_matrix_blocks_only_disabled_entries() {
        assert!(matches!(
            source_gate_decision(Some(true)),
            SourceGateDecision::Proceed
        ));
        assert!(matches!(
            source_gate_decision(None),
            SourceGateDecision::Proceed
        ));
        assert!(matches!(
            source_gate_decision(Some(false)),
            SourceGateDecision::BlockedDisabled
        ));
    }

    #[test]
    fn side_load_policy_matrix_matches_switch_semantics() {
        assert_eq!(side_load_decision(SideLoadPolicy::Allow), SideLoadDecision::Allow);
        assert!(matches!(
            side_load_decision(SideLoadPolicy::Warn),
            SideLoadDecision::AllowWithWarning
        ));
        assert!(matches!(
            side_load_decision(SideLoadPolicy::Deny),
            SideLoadDecision::Deny
        ));
        assert_eq!(SideLoadPolicy::default(), SideLoadPolicy::Warn);
        assert_eq!(parse_side_load_policy("deny"), Some(SideLoadPolicy::Deny));
        assert_eq!(parse_side_load_policy("bogus"), None);
        assert_eq!(side_load_policy_name(SideLoadPolicy::Warn), "warn");
    }

    #[test]
    fn compat_mismatch_detection_covers_tag_forms() {
        assert!(matches!(compat_verdict("1.2.3"), CompatVerdict::Undeclared));
        assert!(matches!(
            compat_verdict("1.2.3+x86_64"),
            CompatVerdict::Match
        ));
        assert!(matches!(compat_verdict("1.2.3+any"), CompatVerdict::Universal));
        assert!(matches!(compat_verdict("2.0+all"), CompatVerdict::Universal));
        assert!(matches!(compat_verdict("1.0+build4"), CompatVerdict::Undeclared));
        match compat_verdict("9.9+aarch64") {
            CompatVerdict::Mismatch { declared } => assert_eq!(declared, "aarch64"),
            other => panic!("expected mismatch, got {other:?}"),
        }
        assert!(compat_requires_override(&CompatVerdict::Mismatch { declared: "arm" }));
        assert!(!compat_requires_override(&CompatVerdict::Match));
        assert!(!compat_requires_override(&CompatVerdict::Undeclared));
        assert!(!compat_requires_override(&CompatVerdict::Universal));
    }

    #[test]
    fn ledger_roundtrip_records_enable_disable_and_remove() {
        ledger().reset();
        assert_eq!(onboarded_count(), 0);
        assert_eq!(onboard_lookup("vendor-main"), None);

        onboard_record("vendor-main").expect("record fresh source");
        assert!(matches!(onboard_lookup("vendor-main"), Some(true)));
        onboard_record("vendor-main").expect_err("duplicate rejected");

        assert!(onboard_set_enabled("vendor-main", false));
        assert!(matches!(onboard_lookup("vendor-main"), Some(false)));
        assert!(!onboard_set_enabled("missing", false));

        onboard_record("vendor-two").expect("second source");
        assert_eq!(onboarded_count(), 2);
        assert!(onboard_remove("vendor-main"));
        assert_eq!(onboard_lookup("vendor-main"), None);
        assert!(matches!(onboard_lookup("vendor-two"), Some(true)));
        assert!(!onboard_remove("vendor-main"));

        ledger().reset();
        assert_eq!(onboarded_count(), 0);
    }
}

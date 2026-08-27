//! SerialScript parsing + execution: `expect:`/`send:` directives against a
//! live [`crate::session::SerialSession`]. Deliberately dumb — regex-on-lines
//! plus typing, no screen scraping (docs/test-plan.md §2.4). Each directive
//! inherits the case timeout proportionally: remaining budget at script
//! start divided evenly across pending expect steps.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::witness::Pattern;

#[derive(Debug)]
pub enum Directive {
    /// `expect: prompt` (special form) or `expect: <pattern>`.
    Expect { raw: String, pattern: Pattern },
    /// `send: <line>` typed verbatim with trailing newline.
    Send { line: String },
    /// `raw: <escaped-bytes>` sent byte-exact with NO trailing newline
    /// (WP3 additive). Supports \\ \n \r \t \xHH escapes — used for the HMP
    /// monitor toggle (Ctrl-A = 0x01) and other control sequences.
    Raw { source: String, bytes: Vec<u8> },
    /// `resend: <line> until <pattern>` — type `<line>`, then re-type it on a
    /// short backoff until `<pattern>` appears or the directive's budget
    /// expires (WP3 additive). Guards against console readline re-arm races
    /// where an entire typed line is silently dropped between commands.
    ResendUntil {
        raw: String,
        line: String,
        pattern: Pattern,
    },
}

#[derive(Debug)]
pub struct SerialScript {
    pub directives: Vec<Directive>,
}

impl SerialScript {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut directives = Vec::new();
        for (index, raw_line) in text.lines().enumerate() {
            let line_no = index + 1;
            let line = strip_comment(raw_line).trim();
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = strip_directive(line, "expect") {
                let pattern = Pattern::new(&rest)
                    .map_err(|error| format!("script line {line_no}: {error}"))?;
                directives.push(Directive::Expect { raw: rest, pattern });
            } else if let Some(rest) = strip_directive(line, "send") {
                directives.push(Directive::Send { line: rest });
            } else if let Some(rest) = strip_directive(line, "raw") {
                let bytes = decode_escapes(&rest)
                    .map_err(|error| format!("script line {line_no}: {error}"))?;
                directives.push(Directive::Raw {
                    source: rest.to_owned(),
                    bytes,
                });
            } else if let Some(rest) = strip_directive(line, "resend") {
                let Some((send_part, expect_part)) = rest.split_once(" until ") else {
                    return Err(format!(
                        "script line {line_no}: resend needs '<line> until <pattern>'"
                    ));
                };
                if send_part.trim().is_empty() {
                    return Err(format!("script line {line_no}: resend line is empty"));
                }
                let pattern = Pattern::new(expect_part.trim())
                    .map_err(|error| format!("script line {line_no}: {error}"))?;
                directives.push(Directive::ResendUntil {
                    raw: rest.to_owned(),
                    line: send_part.trim().to_owned(),
                    pattern,
                });
            } else {
                return Err(format!(
                    "script line {line_no}: expected 'expect:', 'send:', 'raw:' or 'resend:', got {line:?}"
                ));
            }
        }
        Ok(Self { directives })
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("failed to read script {}: {error}", path.display()))?;
        Self::parse(&text)
    }

    fn expect_count(&self) -> usize {
        self.directives
            .iter()
            .filter(|directive| {
                matches!(
                    directive,
                    Directive::Expect { .. } | Directive::ResendUntil { .. }
                )
            })
            .count()
    }

    /// Run against a live session. Anchor semantics honored: `expect: prompt`
    /// waits for [`crate::witness::PROMPT_PATTERN`] before any send.
    pub fn run(
        &self,
        session: &mut crate::session::SerialSession,
        budget: Duration,
    ) -> Result<(), String> {
        let script_started = Instant::now();
        let total_expects = self.expect_count().max(1);
        for directive in &self.directives {
            match directive {
                Directive::Send { line } => {
                    // Scripts always anchor on a prior prompt expectation; if
                    // none preceded, type anyway (runner logs this hazard).
                    session
                        .send_line(line)
                        .map_err(|error| format!("send failed: {error}"))?;
                }
                Directive::Raw { source, bytes } => {
                    session
                        .send_bytes(bytes)
                        .map_err(|error| format!("raw send failed for {source:?}: {error}"))?;
                }
                Directive::ResendUntil { raw, line, pattern } => {
                    let elapsed = Instant::now().saturating_duration_since(script_started);
                    let share = budget / total_expects as u32;
                    let deadline = script_started + budget.min(elapsed + share);
                    if !resend_until(session, line, pattern, deadline)? {
                        return Err(format!(
                            "resend witness not observed before budget share elapsed: {raw:?}"
                        ));
                    }
                }
                Directive::Expect { raw, pattern } => {
                    let elapsed = Instant::now().saturating_duration_since(script_started);
                    let share = budget / total_expects as u32;
                    let deadline = script_started + budget.min(elapsed + share);
                    loop {
                        // Re-check the caller pattern on EVERY wake-up: the
                        // helper below yields on any serial progress instead
                        // of parking until `deadline`, so late-arriving anchor
                        // text can never be starved inside a single wait.
                        if pattern.matches(&session_snapshot_text(session)) {
                            break;
                        }
                        match session_wait(session, deadline) {
                            Ok(()) => continue,
                            Err(crate::session::WaitOutcome::DeadlineExceeded) => {
                                return Err(format!(
                                    "expect not observed before budget share elapsed: {raw:?}"
                                ))
                            }
                            Err(other) => return Err(format!("expect failed: {other}")),
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn strip_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (offset, ch) in line.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => return &line[..offset],
            _ => {}
        }
    }
    line
}

fn strip_directive<'a>(line: &'a str, name: &str) -> Option<String> {
    line.strip_prefix(name)
        .and_then(|rest| rest.strip_prefix(':'))
        .map(|rest| rest.trim().to_owned())
}

/// Decode `raw:` payload escapes. Unknown escapes are rejected loudly.
fn decode_escapes(source: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(source.len());
    let chars: Vec<char> = source.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        let current = chars[index];
        if current != '\\' {
            let mut buffer = [0u8; 4];
            bytes.extend_from_slice(current.encode_utf8(&mut buffer).as_bytes());
            index += 1;
            continue;
        }
        let Some(next) = chars.get(index + 1).copied() else {
            return Err("trailing lone backslash".to_owned());
        };
        match next {
            '\\' => {
                bytes.push(b'\\');
                index += 2;
            }
            'n' => {
                bytes.push(b'\n');
                index += 2;
            }
            'r' => {
                bytes.push(b'\r');
                index += 2;
            }
            't' => {
                bytes.push(b'\t');
                index += 2;
            }
            'x' => {
                let hex: String = chars
                    .get(index + 2..index + 4)
                    .ok_or_else(|| "truncated \\x escape".to_string())?
                    .iter()
                    .collect();
                let value =
                    u8::from_str_radix(&hex, 16).map_err(|_| format!("bad hex digits in \\x{hex}"))?;
                bytes.push(value);
                index += 4;
            }
            other => return Err(format!("unsupported escape \\{other}")),
        }
    }
    Ok(bytes)
}

// Thin indirections so future test doubles can intercept I/O without
// duplicating directive walking.
fn session_snapshot_text(session: &mut crate::session::SerialSession) -> String {
    session.snapshot().text().to_owned()
}

fn session_wait(
    session: &mut crate::session::SerialSession,
    deadline: Instant,
) -> Result<(), crate::session::WaitOutcome> {
    // Yield on ANY serial progress so the outer loop can re-evaluate its own
    // anchor pattern; parking here until `deadline` (the old
    // `wait_witness(NEVER_MATCHES)` shape) silently swallowed every anchor
    // that arrived after the expect was entered.
    session.await_evidence(deadline)
}

/// Backoff between re-transmissions of a dropped typed line.
const RESEND_PERIOD: Duration = Duration::from_secs(3);
/// Hard cap so a genuinely delivered-but-unmatched command cannot be spammed
/// forever (a second delivery of `pkg install` is rejected upstream with a
/// distinct message that fails the witness honestly).
const MAX_RESENDS: usize = 8;

/// Type `line`, re-typing on a short backoff while its response is missing.
/// Returns Ok(true) when `pattern` was observed, Ok(false) on budget
/// exhaustion (caller turns that into the standard failure message), and
/// propagates transport/infra errors.
fn resend_until(
    session: &mut crate::session::SerialSession,
    line: &str,
    pattern: &Pattern,
    deadline: Instant,
) -> Result<bool, String> {
    let mut sends = 0usize;
    let mut next_send_at = Instant::now();
    loop {
        if pattern.matches(&session_snapshot_text(session)) {
            return Ok(true);
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(false);
        }
        if sends < MAX_RESENDS && now >= next_send_at {
            sends += 1;
            session
                .send_line(line)
                .map_err(|error| format!("resend send failed: {error}"))?;
            next_send_at = now + RESEND_PERIOD;
        }
        match session_wait(session, deadline.min(next_send_at)) {
            Ok(()) => continue,
            Err(crate::session::WaitOutcome::DeadlineExceeded) => continue,
            Err(other) => return Err(format!("resend wait failed: {other}")),
        }
    }
}

/// Public entry used by `run_case` when `serial_script` is set.
pub fn run_script(
    script_path: &Path,
    session: &mut crate::session::SerialSession,
    budget: Duration,
) -> Result<(), String> {
    let script = SerialScript::load(script_path)?;
    script.run(session, budget)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_expect_send_alternation() {
        let text = r#"
# comment lines skipped
expect: prompt
send: status health
expect: system health @tick|^commands:
send: help
"#;
        let script = SerialScript::parse(text).expect("parse");
        assert_eq!(script.directives.len(), 4);
        assert!(matches!(script.directives[0], Directive::Expect { .. }));
        assert!(matches!(script.directives[1], Directive::Send { ref line } if line == "status health"));
        assert_eq!(
            script.expect_count(),
            2,
            "prompt expectation counts toward proportional budget"
        );
    }

    #[test]
    fn rejects_unknown_directives_with_line_numbers() {
        let error = SerialScript::parse("expect: x\ntype: y\n").expect_err("rejected");
        assert!(error.contains("line 2"), "{error}");
    }

    #[test]
    fn decodes_raw_escapes_for_hmp_toggle() {
        let script = SerialScript::parse("expect: prompt\nraw: \\x01c\nsend: status\n").expect("parse");
        assert_eq!(script.directives.len(), 3);
        match &script.directives[1] {
            Directive::Raw { bytes, .. } => {
                assert_eq!(bytes, &[0x01, b'c'], "Ctrl-A then 'c' toggles the mux")
            }
            other => panic!("expected Raw, got {other:?}"),
        }
        let bad = SerialScript::parse("raw: \\q\n").expect_err("rejected");
        assert!(bad.contains("unsupported"), "{bad}");
    }

    #[test]
    fn parses_resend_until_with_budget_accounting() {
        let text = "send: status\nresend: pkg install developer until installed|failed\nexpect: done\n";
        let script = SerialScript::parse(text).expect("parse");
        assert_eq!(script.directives.len(), 3);
        match &script.directives[1] {
            Directive::ResendUntil { raw, line, pattern } => {
                assert_eq!(line, "pkg install developer");
                assert_eq!(
                    pattern.raw(),
                    "installed|failed",
                    "pattern keeps alternation"
                );
                assert!(raw.starts_with("pkg install developer until "));
            }
            other => panic!("expected ResendUntil, got {other:?}"),
        }
        // The resend step shares the proportional expect budget.
        assert_eq!(script.expect_count(), 2);
        let no_until = SerialScript::parse("resend: only-send\n").expect_err("rejected");
        assert!(no_until.contains("until"), "{no_until}");
    }

    #[test]
    fn accepts_empty_send_and_alternation_anchor_forms() {
        // Real-script shapes used by tests/cases/live/scripts/*.txt.
        let text = "expect: prompt\nsend:\nsend: status health\nexpect: system health @tick\nsend: pkg install developer\nexpect: installed developer-service \\(|install failed:\n";
        let script = SerialScript::parse(text).expect("parse");
        assert_eq!(script.directives.len(), 6);
        match &script.directives[1] {
            Directive::Send { line } => assert!(line.is_empty(), "blank send = bare Enter"),
            other => panic!("expected Send, got {other:?}"),
        }
        match &script.directives[5] {
            Directive::Expect { raw, .. } => {
                assert!(raw.contains('|'), "alternation preserved through parsing")
            }
            other => panic!("expected Expect, got {other:?}"),
        }
    }
}

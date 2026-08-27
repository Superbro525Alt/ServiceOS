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
            } else {
                return Err(format!(
                    "script line {line_no}: expected 'expect:' or 'send:', got {line:?}"
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
            .filter(|directive| matches!(directive, Directive::Expect { .. }))
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
                Directive::Expect { raw, pattern } => {
                    let elapsed = Instant::now().saturating_duration_since(script_started);
                    let share = budget / total_expects as u32;
                    let deadline = script_started + budget.min(elapsed + share);
                    loop {
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

// Thin indirections so future test doubles can intercept I/O without
// duplicating directive walking.
fn session_snapshot_text(session: &mut crate::session::SerialSession) -> String {
    session.snapshot().text().to_owned()
}

fn session_wait(
    session: &mut crate::session::SerialSession,
    deadline: Instant,
) -> Result<(), crate::session::WaitOutcome> {
    session.wait_witness("\u{0}NEVER_MATCHES\u{0}", deadline)
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
}

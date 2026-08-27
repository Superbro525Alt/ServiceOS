//! Witness matching: a tiny Thompson-NFA pattern engine covering the regex
//! subset the suite needs (literals, `.`, classes `\d \s \w` (+ negations),
//! bracket classes `[abc]` / `[^abc]` (no `-` ranges: `-` is a literal
//! member), groups, alternation `|`, quantifiers `* + ?`, and per-line anchors
//! `^ $` evaluated against `\n` boundaries). Pure-literal patterns take a fast
//! substring path equivalent to bootlog's marker grep.

use std::fmt;

#[derive(Debug)]
pub struct PatternError {
    raw: String,
    message: String,
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid pattern {:?}: {}", self.raw, self.message)
    }
}

impl std::error::Error for PatternError {}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Class {
    Literal(char),
    Any,
    Digit(bool),
    Word(bool),
    Space(bool),
    /// Bracket class: member alternatives + negation flag.
    Set(bool, Vec<Class>),
}

impl Class {
    fn accepts(&self, ch: char) -> bool {
        match self {
            Class::Literal(expected) => *expected == ch,
            Class::Any => true,
            Class::Digit(negated) => ch.is_ascii_digit() != *negated,
            Class::Word(negated) => (ch.is_alphanumeric() || ch == '_') != *negated,
            Class::Space(negated) => ch.is_whitespace() != *negated,
            Class::Set(negated, members) => members.iter().any(|m| m.accepts(ch)) != *negated,
        }
    }
}

#[derive(Debug, Clone)]
enum Inst {
    Char(Class),
    LineStart,
    LineEnd,
    Split(usize, usize),
    Jump(usize),
    Accept,
}

#[derive(Debug, Clone)]
enum Node {
    Atom(Class),
    AnchorStart,
    AnchorEnd,
    Group(Vec<Vec<Node>>),
    Repeat(Box<Node>, u32, Option<u32>),
}

/// A compiled witness pattern. Cloning is cheap; reuse across rows.
#[derive(Debug, Clone)]
pub struct Pattern {
    raw: String,
    program: Vec<Inst>,
    literal: Option<String>,
}

const META_CHARS: [char; 12] = ['\\', '^', '$', '.', '|', '(', ')', '*', '+', '?', '[', ']'];

/// Canonical console-prompt matcher (docs/test-plan.md §6.4: parameterize the
/// glyph in one constant until WP3 inspects the shell draw code).
pub const PROMPT_PATTERN: &str = "[#>$] $";

struct Compiler {
    program: Vec<Inst>,
}

impl Compiler {
    fn new() -> Self {
        Self { program: Vec::new() }
    }

    fn emit(&mut self, inst: Inst) -> usize {
        self.program.push(inst);
        self.program.len() - 1
    }

    fn set_slot(&mut self, at: usize, slot: u8, target: usize) {
        debug_assert!(matches!(self.program[at], Inst::Split(_, _)));
        self.program[at] = match (&self.program[at], slot) {
            (Inst::Split(_, b), 0) => Inst::Split(target, *b),
            (Inst::Split(a, _), 1) => Inst::Split(*a, target),
            _ => unreachable!("patched instruction was not a live Split"),
        };
    }

    /// Compile alternatives; every branch except the last falls through to a
    /// jump to a shared exit so adjacent branches stay isolated.
    fn alternatives(&mut self, alts: &[Vec<Node>]) -> Result<(), PatternError> {
        let mut exit_jumps: Vec<usize> = Vec::new();
        let mut pending_chain: Option<usize> = None;
        for (index, alt) in alts.iter().enumerate() {
            // This branch's entry point; a prior split's slot 1 targets it.
            if let Some(previous) = pending_chain.take() {
                self.set_slot(previous, 1, self.program.len());
            }
            let mut chained_split: Option<usize> = None;
            if index + 1 < alts.len() {
                let split = self.emit(Inst::Split(self.program.len() + 1, usize::MAX));
                chained_split = Some(split);
            }
            self.sequence(alt)?;
            exit_jumps.push(self.emit(Inst::Jump(usize::MAX)));
            pending_chain = chained_split;
        }
        let exit_target = self.program.len();
        for jump in exit_jumps {
            self.program[jump] = Inst::Jump(exit_target);
        }
        Ok(())
    }

    fn sequence(&mut self, nodes: &[Node]) -> Result<(), PatternError> {
        for node in nodes {
            self.node(node)?;
        }
        Ok(())
    }

    fn node(&mut self, node: &Node) -> Result<(), PatternError> {
        match node {
            Node::Atom(class) => {
                self.emit(Inst::Char(class.clone()));
                Ok(())
            }
            Node::AnchorStart => {
                self.emit(Inst::LineStart);
                Ok(())
            }
            Node::AnchorEnd => {
                self.emit(Inst::LineEnd);
                Ok(())
            }
            Node::Group(alts) => self.alternatives(alts),
            Node::Repeat(inner, min, max) => self.repeat(inner, *min, *max),
        }
    }

    fn repeat(&mut self, inner: &Node, min: u32, max: Option<u32>) -> Result<(), PatternError> {
        for _ in 0..min {
            self.node(inner)?;
        }
        match max {
            None => {
                let loop_start = self.program.len();
                let split = self.emit(Inst::Split(loop_start + 1, usize::MAX));
                self.node(inner)?;
                self.emit(Inst::Jump(loop_start));
                let exit_target = self.program.len();
                self.set_slot(split, 1, exit_target);
            }
            Some(maximum) => {
                let optionals = maximum.saturating_sub(min);
                for _ in 0..optionals {
                    let split = self.emit(Inst::Split(usize::MAX, usize::MAX));
                    let body_entry = self.program.len();
                    self.set_slot(split, 0, body_entry);
                    self.node(inner)?;
                    let continue_target = self.program.len();
                    self.set_slot(split, 1, continue_target);
                }
            }
        }
        Ok(())
    }
}

struct Parser<'p> {
    chars: &'p [char],
    source: &'p str,
    position: usize,
}

impl<'p> Parser<'p> {
    fn new(source: &'p str, chars: &'p [char]) -> Self {
        Self {
            chars,
            source,
            position: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.position).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek();
        if ch.is_some() {
            self.position += 1;
        }
        ch
    }

    fn alternatives(&mut self) -> Result<Vec<Vec<Node>>, PatternError> {
        let mut alts = vec![self.sequence()?];
        while self.peek() == Some('|') {
            self.bump();
            alts.push(self.sequence()?);
        }
        Ok(alts)
    }

    fn sequence(&mut self) -> Result<Vec<Node>, PatternError> {
        let mut nodes = Vec::new();
        while let Some(ch) = self.peek() {
            if ch == '|' || ch == ')' {
                break;
            }
            let atom = self.atom()?;
            nodes.push(self.quantified(atom)?);
        }
        Ok(nodes)
    }

    fn atom(&mut self) -> Result<Node, PatternError> {
        let Some(ch) = self.bump() else {
            return Err(self.error("unexpected end of pattern"));
        };
        match ch {
            '(' => {
                let inner = self.alternatives()?;
                if self.bump() != Some(')') {
                    return Err(self.error("missing closing parenthesis"));
                }
                Ok(Node::Group(inner))
            }
            ')' => Err(self.error("unbalanced parenthesis")),
            '[' => self.bracket_class(),
            '.' => Ok(Node::Atom(Class::Any)),
            '^' => Ok(Node::AnchorStart),
            '$' => Ok(Node::AnchorEnd),
            '\\' => self.escape(),
            '*' | '+' | '?' => Err(self.error("quantifier has nothing to repeat")),
            other => Ok(Node::Atom(Class::Literal(other))),
        }
    }

    fn escape(&mut self) -> Result<Node, PatternError> {
        let Some(ch) = self.bump() else {
            return Err(self.error("dangling backslash"));
        };
        Ok(match ch {
            'd' => Node::Atom(Class::Digit(false)),
            'D' => Node::Atom(Class::Digit(true)),
            'w' => Node::Atom(Class::Word(false)),
            'W' => Node::Atom(Class::Word(true)),
            's' => Node::Atom(Class::Space(false)),
            'S' => Node::Atom(Class::Space(true)),
            'n' => Node::Atom(Class::Literal('\n')),
            't' => Node::Atom(Class::Literal('\t')),
            other => Node::Atom(Class::Literal(other)),
        })
    }

    /// `[abc]` / `[^abc]`; members are literals, the shared `\` escapes, and a
    /// literal `-`. Ranges are intentionally unsupported (greps stay dumb);
    /// `]` must be escaped to appear as a member.
    fn bracket_class(&mut self) -> Result<Node, PatternError> {
        let negated = if self.peek() == Some('^') {
            self.bump();
            true
        } else {
            false
        };
        let mut members = Vec::new();
        loop {
            let Some(ch) = self.peek() else {
                return Err(self.error("missing closing bracket"));
            };
            self.bump();
            match ch {
                ']' => break,
                '\\' => {
                    let Some(escaped) = self.bump() else {
                        return Err(self.error("dangling backslash in class"));
                    };
                    members.push(match escaped {
                        'd' => Class::Digit(false),
                        'w' => Class::Word(false),
                        's' => Class::Space(false),
                        'n' => Class::Literal('\n'),
                        't' => Class::Literal('\t'),
                        other => Class::Literal(other),
                    });
                }
                other => members.push(Class::Literal(other)),
            }
        }
        if members.is_empty() {
            return Err(self.error("empty class"));
        }
        Ok(Node::Atom(Class::Set(negated, members)))
    }

    fn quantified(&mut self, atom: Node) -> Result<Node, PatternError> {
        match self.peek() {
            Some('*') => {
                self.bump();
                Ok(Node::Repeat(Box::new(atom), 0, None))
            }
            Some('+') => {
                self.bump();
                Ok(Node::Repeat(Box::new(atom), 1, None))
            }
            Some('?') => {
                self.bump();
                Ok(Node::Repeat(Box::new(atom), 0, Some(1)))
            }
            _ => Ok(atom),
        }
    }

    fn error(&self, message: &str) -> PatternError {
        PatternError {
            raw: self.source.to_owned(),
            message: message.to_owned(),
        }
    }
}

impl Pattern {
    pub fn new(raw: &str) -> Result<Self, PatternError> {
        let literal = if raw.chars().any(|ch| META_CHARS.contains(&ch)) {
            None
        } else {
            Some(raw.to_owned())
        };

        let owned_chars: Vec<char> = raw.chars().collect();
        let mut parser = Parser::new(raw, &owned_chars);
        let alternatives = parser.alternatives()?;
        if parser.position != owned_chars.len() {
            return Err(parser.error("trailing unparsed input"));
        }

        let mut compiler = Compiler::new();
        compiler.alternatives(&alternatives)?;
        compiler.emit(Inst::Accept);

        Ok(Self {
            raw: raw.to_owned(),
            program: compiler.program,
            literal,
        })
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }

    pub fn is_literal(&self) -> bool {
        self.literal.is_some()
    }

    /// Haystack search over text possibly containing newlines. Anchors
    /// evaluate per line: `^` matches at offset 0 or right after `\n`;
    /// `$` matches at string end or right before `\n`.
    pub fn matches(&self, text: &str) -> bool {
        if let Some(literal) = &self.literal {
            return text.contains(literal.as_str());
        }
        let chars: Vec<char> = text.chars().collect();
        (0..=chars.len()).any(|start| self.run_from(&chars, start))
    }

    /// Single-line evaluation used by scripts/prompts; identical rules minus
    /// cross-line boundary contexts.
    pub fn match_line(&self, line: &str) -> bool {
        self.matches(line)
    }

    fn run_from(&self, chars: &[char], start: usize) -> bool {
        let mut visited = vec![false; self.program.len()];
        let mut frontier: Vec<usize> = vec![0];

        let mut position = start;
        loop {
            // Epsilon closure honoring zero-width anchors at this position.
            let mut stack: Vec<usize> = frontier.clone();
            let mut matched = false;
            while let Some(index) = stack.pop() {
                if index >= self.program.len() || visited[index] {
                    continue;
                }
                visited[index] = true;
                match &self.program[index] {
                    Inst::Jump(target) => stack.push(*target),
                    Inst::Split(a, b) => {
                        stack.push(*a);
                        stack.push(*b);
                    }
                    Inst::LineStart => {
                        if position == 0 || chars.get(position.wrapping_sub(1)) == Some(&'\n') {
                            stack.push(index + 1);
                        }
                    }
                    Inst::LineEnd => {
                        if position == chars.len() || chars.get(position) == Some(&'\n') {
                            stack.push(index + 1);
                        }
                    }
                    Inst::Accept => matched = true,
                    Inst::Char(_) => {}
                }
            }
            if matched {
                return true;
            }

            if position >= chars.len() {
                return false;
            }

            let ch = chars[position];
            let mut next_frontier = Vec::new();
            for index in 0..self.program.len() {
                if !visited[index] {
                    continue;
                }
                if let Inst::Char(class) = &self.program[index] {
                    if class.accepts(ch) {
                        next_frontier.push(index + 1);
                    }
                }
            }

            position += 1;
            if next_frontier.is_empty() {
                return false;
            }
            frontier = next_frontier;
            for seen in visited.iter_mut() {
                *seen = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pattern: &str, text: &str) -> bool {
        Pattern::new(pattern)
            .unwrap_or_else(|error| panic!("pattern {pattern:?}: {error}"))
            .matches(text)
    }

    #[test]
    fn literal_fast_path_is_substring_semantics() {
        assert!(m("net-selftest end", "serviceos: net: net-selftest end ok"));
        assert!(!m("missing", "present evidence"));
    }

    #[test]
    fn digit_classes_match_selftest_evidence() {
        assert!(m("bytes=\\d+", "selftest file-written bytes=128 ok"));
        assert!(!m("bytes=\\d+", "bytes=none"));
        assert!(m("timer-hz=\\d+", "boot: timer-hz=100 syscall-vector=49"));
    }

    #[test]
    fn alternation_and_groups_compose() {
        assert!(m("exit code 0|^commands:", "commands:\n status\n"));
        assert!(m("exit code 0|^commands:", "shell said exit code 0 today"));
        assert!(!m("^commands:", "please run commands: now"));
        assert!(m("^second", "first\nsecond\nthird"));
    }

    #[test]
    fn quantifiers_greedy_and_optional() {
        assert!(m("ab*c", "abbbc"));
        assert!(!m("ab+c", "ac"));
        assert!(m("colou?r", "color"));
        assert!(m("colou?r", "colour"));
        assert!(m("\\d+ tasks", "12 tasks alive"));
        assert!(m("a?b", "b"));
    }

    #[test]
    fn negated_classes_respect_boundaries() {
        assert!(m("a\\Dc", "abc"));
        assert!(!m("a\\Dc", "a1c"));
        assert!(m("\\S+@tick", "health@tick"));
        assert!(!m("\\S+@tick", "health @tick"));
    }

    #[test]
    fn dollar_anchors_on_multiline_haystacks() {
        let text = "first phase\nsecond phase ok\ndone";
        assert!(m("phase ok$", text));
        assert!(!m("^phase ok", text));
    }

    #[test]
    fn nested_groups_with_alternation_backtrack() {
        assert!(m("(open|close)\\((\\d+)\\)", "called close(12) now"));
        assert!(m("(xa|xb)+y", "xaxbxay"));
        assert!(!m("(xa|xb)+y", "xaxaby"));
    }

    #[test]
    fn invalid_patterns_error_cleanly() {
        assert!(Pattern::new("*bad").is_err());
        assert!(Pattern::new("trailing\\").is_err());
        assert!(Pattern::new("(unclosed").is_err());
        assert!(Pattern::new("+").is_err());
    }

    #[test]
    fn parenthesized_group_preserves_literal_equivalence() {
        // Regex-group parentheses are structural, never literal: the escaped
        // form \( .. \) is required to match literal parens in guest output.
        let with_parens =
            "AFGHIJKLserviceos: boot: entered x86_64 legacy-BIOS (SeaBIOS/PVH) kernel image";
        let cases: Vec<(&str, &str)> = vec![
            ("entered x86_64 legacy-BIOS \\(SeaBIOS/PVH\\) kernel image", with_parens),
            ("legacy-BIOS \\(SeaBIOS/PVH\\)", "x legacy-BIOS (SeaBIOS/PVH) y"),
            ("x86_64 \\(a\\)", "cpu x86_64 (a) ok"),
            ("SeaBIOS/PVH", with_parens),
        ];
        let mut failures = Vec::new();
        for (pattern, text) in &cases {
            if !m(pattern, text) {
                failures.push(*pattern);
            }
        }
        assert!(
            failures.is_empty(),
            "NFA diverged from substring semantics for: {:?}",
            failures
        );
        assert!(!m("x86_64 \\(a\\)", "cpu x86_64 a"));
    }

    #[test]
    fn prompt_pattern_matches_glyph_terminations() {
        let pattern = Pattern::new(PROMPT_PATTERN).expect("prompt pattern");
        assert!(pattern.match_line("shell> "));
        assert!(pattern.match_line("root# "));
        assert!(pattern.match_line("user$ "));
        assert!(!pattern.match_line("random output line"));
    }
}

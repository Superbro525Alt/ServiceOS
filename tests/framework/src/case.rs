//! Case definitions: a std-only TOML-subset parser plus recursive discovery
//! of `tests/cases/**/*.toml`. Unsupported syntax fails loudly with
//! file:line context so typos surface immediately (docs/test-plan.md §2.2).

use std::{
    collections::BTreeSet,
    fmt,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct CaseError {
    pub file: PathBuf,
    pub message: String,
}

impl fmt::Display for CaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.file.display(), self.message)
    }
}

impl std::error::Error for CaseError {}

fn err(file: &Path, line: usize, message: impl Into<String>) -> Box<CaseError> {
    Box::new(CaseError {
        file: file.to_path_buf(),
        message: format!("line {line}: {}", message.into()),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WitnessMode {
    /// All witnesses must appear (plus nothing adverse). Default.
    Witness,
    /// Require the §2.6 protocol completion line (`E2E SUITE DONE ...`).
    Suite,
}

/// One declarative end-to-end case as loaded from a TOML file.
#[derive(Debug, Clone)]
pub struct CaseDef {
    pub source_path: PathBuf,
    pub name: String,
    pub tier: u8,
    pub platforms: Vec<String>,
    /// None => SERVICEOS_BOOT_TIMEOUT_SECS or DEFAULT_CASE_TIMEOUT_SECS.
    pub timeout_secs: Option<u64>,
    pub witnesses: Vec<String>,
    pub fail_on: Vec<String>,
    /// option_env!-style guest build gates (WP2 plumbs them into builds).
    pub env_build: Vec<(String, String)>,
    pub probes: Vec<String>,
    pub serial_script: Option<PathBuf>,
    pub data_fresh: bool,
    pub tags: Vec<String>,
    pub mode: WitnessMode,
    /// Informational graph depth annotation (e.g. isa declares "minimal").
    pub graph: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Value {
    Str(String),
    Int(i64),
    Bool(bool),
    Array(Vec<Value>),
}

impl Value {
    fn as_string(&self) -> Option<&str> {
        match self {
            Value::Str(text) => Some(text),
            _ => None,
        }
    }

    fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(flag) => Some(*flag),
            _ => None,
        }
    }

    fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Int(int) if *int >= 0 => Some(*int as u64),
            _ => None,
        }
    }

    fn as_strings(&self) -> Option<Vec<&str>> {
        match self {
            Value::Array(items) => items.iter().map(|v| v.as_string()).collect(),
            _ => None,
        }
    }
}

/// Parse a case document. Returns ordered key/value pairs with line numbers.
/// Physical lines are first joined into logical statements while square
/// brackets stay open, so multi-line arrays parse like the canonical sample.
fn parse_document(file: &Path, text: &str) -> Result<Vec<(String, Value, usize)>, Box<dyn std::error::Error>> {
    let mut entries = Vec::new();
    let mut seen_keys: BTreeSet<String> = BTreeSet::new();
    for (line_no, logical) in logical_lines(text) {
        let line = strip_comment(&logical).trim().to_owned();
        if line.is_empty() {
            continue;
        }
        process_entry_line(file, line_no, &line, &mut entries, &mut seen_keys)?;
    }
    Ok(entries)
}

/// Join physical lines into bracket-balanced statements.
fn logical_lines(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    let mut buffer = String::new();
    let mut start_line = 0usize;
    for (index, raw_line) in text.lines().enumerate() {
        let line_no = index + 1;
        if depth == 0 {
            start_line = line_no;
            buffer.clear();
        }
        for ch in scan_chars_excluding_strings(raw_line) {
            match ch {
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => {}
            }
        }
        // Comment tails are stripped per physical line: stripping the joined
        // buffer once would truncate everything after the first trailing
        // comment of a multi-line array.
        buffer.push_str(strip_comment(raw_line));
        buffer.push('\n');
        if depth <= 0 {
            depth = 0.max(depth);
            if !buffer.trim().is_empty() {
                out.push((start_line, buffer.clone()));
            }
            buffer.clear();
        }
    }
    out
}

/// Yield characters with double-quoted spans collapsed and `#` comment
/// tails dropped, so `[`/`]` inside strings or comments never influence
/// balance.
fn scan_chars_excluding_strings(line: &str) -> Vec<char> {
    let mut chars = Vec::new();
    let mut in_quotes = false;
    for ch in line.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => break,
            _ => {
                if !in_quotes {
                    chars.push(ch);
                }
            }
        }
    }
    chars
}

fn process_entry_line(
    file: &Path,
    line_no: usize,
    line: &str,
    entries: &mut Vec<(String, Value, usize)>,
    seen_keys: &mut BTreeSet<String>,
) -> Result<(), Box<dyn std::error::Error>> {
        if line.starts_with('[') {
            // Bracket-balanced joining guarantees headers stay standalone.
            if line != "[case]" {
                return Err(err(
                    file,
                    line_no,
                    format!("unsupported table header {line}; only [case] exists"),
                ));
            }
            return Ok(());
        }
        let Some((key_part, value_part)) = line.split_once('=') else {
            return Err(err(file, line_no, "expected key = value"));
        };
        let key = key_part.trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
        {
            return Err(err(file, line_no, format!("invalid key {key:?}")));
        }
        if !seen_keys.insert(key.to_owned()) {
            return Err(err(file, line_no, format!("duplicate key {key:?}")));
        }
        let value = parse_value(value_part.trim(), file, line_no)?;
        entries.push((key.to_owned(), value, line_no));
    Ok(())
}

fn strip_comment(line: &str) -> &str {
    // Comments start at '#' when outside quotes; scan char-wise honoring "".
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

fn parse_value(raw: &str, file: &Path, line: usize) -> Result<Value, Box<dyn std::error::Error>> {
    if raw.is_empty() {
        return Err(err(file, line, "missing value"));
    }
    if raw.starts_with('"') {
        return parse_string(raw, file, line);
    }
    if raw.starts_with('[') {
        return parse_array(raw, file, line);
    }
    if raw == "true" || raw == "false" {
        return Ok(Value::Bool(raw == "true"));
    }
    if let Ok(int) = raw.parse::<i64>() {
        return Ok(Value::Int(int));
    }
    Err(err(
        file,
        line,
        format!("unsupported value {raw:?} (strings must be quoted)"),
    ))
}

fn parse_string(raw: &str, file: &Path, line: usize) -> Result<Value, Box<dyn std::error::Error>> {
    let bytes: Vec<char> = raw.chars().collect();
    debug_assert_eq!(bytes[0], '"');
    let mut out = String::new();
    let mut index = 1usize;
    while index < bytes.len() {
        let ch = bytes[index];
        if ch == '"' {
            let rest: String = bytes[index + 1..].iter().collect();
            if !rest.trim().is_empty() {
                return Err(err(file, line, "trailing characters after string literal"));
            }
            return Ok(Value::Str(out));
        }
        if ch == '\\' {
            index += 1;
            let Some(escaped) = bytes.get(index) else {
                return Err(err(file, line, "dangling escape in string"));
            };
            out.push(match escaped {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '\\' | '"' => *escaped,
                // Regex metacharacters: witness patterns need \( \) \. etc.
                '(' | ')' | '[' | ']' | '{' | '}' | '.' | '+' | '*' | '?' | '^' | '$' | '|'
                | '-' | '/' => *escaped,
                other => return Err(err(file, line, format!("unsupported escape \\{other}"))),
            });
            index += 1;
            continue;
        }
        out.push(ch);
        index += 1;
    }
    Err(err(file, line, "unterminated string literal"))
}

fn parse_array(raw: &str, file: &Path, line: usize) -> Result<Value, Box<dyn std::error::Error>> {
    let trimmed = raw.trim();
    if !trimmed.ends_with(']') {
        return Err(err(file, line, "arrays must open and close on one line"));
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if inner.is_empty() {
        return Ok(Value::Array(Vec::new()));
    }
    let mut parts = split_top_level(inner, ',');
    // Tolerate Rust-style trailing commas.
    while parts.last().is_some_and(|piece| piece.trim().is_empty()) {
        parts.pop();
    }
    let mut items = Vec::new();
    for piece in &parts {
        let piece = piece.trim();
        if piece.is_empty() {
            return Err(err(file, line, "empty array element"));
        }
        items.push(parse_value(piece, file, line)?);
    }
    Ok(Value::Array(items))
}

fn split_top_level(inner: &str, separator: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in inner.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            c if c == separator && !in_quotes => {
                parts.push(current.clone());
                current.clear();
            }
            c => current.push(c),
        }
    }
    parts.push(current);
    parts
}

const KNOWN_KEYS: [&str; 13] = [
    "name",
    "tier",
    "platforms",
    "timeout_secs",
    "witnesses",
    "fail_on",
    "env_build",
    "probes",
    "serial_script",
    "data_fresh",
    "tags",
    "mode",
    "graph",
];

impl CaseDef {
    pub fn parse_file(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let text = fs::read_to_string(path)?;
        let mut def = Self {
            source_path: path.to_path_buf(),
            name: String::new(),
            tier: 1,
            platforms: vec!["qemu-virtio".to_owned()],
            timeout_secs: None,
            witnesses: Vec::new(),
            fail_on: Vec::new(),
            env_build: Vec::new(),
            probes: Vec::new(),
            serial_script: None,
            data_fresh: true,
            tags: Vec::new(),
            mode: WitnessMode::Witness,
            graph: String::from("full"),
        };

        for (key, value, line) in parse_document(path, &text)? {
            match key.as_str() {
                "name" => {
                    def.name = value
                        .as_string()
                        .ok_or_else(|| err(path, line, "name must be a string"))?
                        .to_owned();
                    if def.name.is_empty() {
                        return Err(err(path, line, "name must not be empty"));
                    }
                }
                "tier" => {
                    let int = value
                        .as_u64()
                        .ok_or_else(|| err(path, line, "tier must be 0..=4"))?;
                    if !(1..=4).contains(&int) {
                        return Err(err(path, line, "tier must be within 1..=4"));
                    }
                    def.tier = int as u8;
                }
                "platforms" => {
                    let strings = value
                        .as_strings()
                        .ok_or_else(|| {
                            err(path, line, "platforms must be an array of platform names")
                        })?;
                    if strings.is_empty() {
                        return Err(err(path, line, "platforms must not be empty"));
                    }
                    for platform in &strings {
                        PlatformList::validate(platform)
                            .map_err(|message| err(path, line, message))?;
                    }
                    def.platforms = strings.into_iter().map(str::to_owned).collect::<Vec<_>>();
                }
                "timeout_secs" => {
                    def.timeout_secs = Some(
                        value
                            .as_u64()
                            .filter(|secs| *secs > 0)
                            .ok_or_else(|| err(path, line, "timeout_secs must be positive"))?,
                    );
                }
                "witnesses" => {
                    let strings = value
                        .as_strings()
                        .ok_or_else(|| err(path, line, "witnesses must be an array of strings"))?
                        .into_iter()
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                    if strings.is_empty() {
                        return Err(err(path, line, "witnesses must not be empty"));
                    }
                    for witness in &strings {
                        // Compile once here to give case-file-level errors.
                        crate::witness::Pattern::new(witness).map_err(|pattern_error| {
                            err(path, line, pattern_error.to_string())
                        })?;
                    }
                    def.witnesses = strings;
                }
                "fail_on" => {
                    def.fail_on = value
                        .as_strings()
                        .ok_or_else(|| err(path, line, "fail_on must be an array of strings"))?
                        .into_iter()
                        .map(|raw| {
                            crate::witness::Pattern::new(raw)
                                .map_err(|pattern_error| err(path, line, pattern_error.to_string()))?;
                            Ok(raw.to_owned())
                        })
                        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
                }
                "env_build" => {
                    let pairs = value
                        .as_strings()
                        .ok_or_else(|| {
                            err(path, line, "env_build must be an array of \"KEY=VALUE\" strings")
                        })?;
                    for pair in pairs {
                        let Some((key, assigned)) = pair.split_once('=') else {
                            return Err(err(path, line, format!("env_build entry {pair:?} lacks '='")));
                        };
                        if key.is_empty() || assigned.is_empty() {
                            return Err(err(path, line, "env_build KEY and VALUE required"));
                        }
                        def.env_build.push((key.to_owned(), assigned.to_owned()));
                    }
                }
                "probes" => {
                    let strings = value
                        .as_strings()
                        .ok_or_else(|| err(path, line, "probes must be an array of strings"))?
                        .into_iter()
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                    for probe in &strings {
                        if probe.contains(' ') || probe != probe.trim() {
                            return Err(err(path, line, format!("invalid probe name {probe:?}")));
                        }
                    }
                    def.probes = strings;
                }
                "serial_script" => {
                    let script = value
                        .as_string()
                        .ok_or_else(|| err(path, line, "serial_script must be a string"))?;
                    if !script.is_empty() {
                        let resolved = path.parent().unwrap_or(Path::new(".")).join(script);
                        if !resolved.exists() {
                            return Err(err(
                                path,
                                line,
                                format!("serial_script not found: {}", resolved.display()),
                            ));
                        }
                        def.serial_script = Some(resolved);
                    }
                }
                "data_fresh" => {
                    def.data_fresh = value
                        .as_bool()
                        .ok_or_else(|| err(path, line, "data_fresh must be a boolean"))?;
                }
                "tags" => {
                    def.tags = value
                        .as_strings()
                        .ok_or_else(|| err(path, line, "tags must be an array of strings"))?
                        .into_iter()
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                }
                "mode" => {
                    def.mode = match value.as_string().unwrap_or_default() {
                        "witness" => WitnessMode::Witness,
                        "suite" => WitnessMode::Suite,
                        other => {
                            return Err(err(
                                path,
                                line,
                                format!("mode must be witness|suite, got {other:?}"),
                            ))
                        }
                    };
                }
                "graph" => {
                    def.graph = value
                        .as_string()
                        .ok_or_else(|| err(path, line, "graph must be a string"))?
                        .to_owned();
                }
                unknown => {
                    return Err(err(
                        path,
                        line,
                        format!(
                            "unknown key {unknown:?} (expected one of: {})",
                            KNOWN_KEYS.join(", ")
                        ),
                    ));
                }
            }
        }

        if def.name.is_empty() {
            return Err(Box::new(CaseError {
                file: path.to_path_buf(),
                message: "missing required key name".to_owned(),
            }));
        }
        if def.witnesses.is_empty() && def.mode == WitnessMode::Witness {
            return Err(Box::new(CaseError {
                file: path.to_path_buf(),
                message: "witnesses must list at least one evidence pattern".to_owned(),
            }));
        }
        for platform in &def.platforms {
            PlatformList::validate(platform).map_err(|message| CaseError {
                file: path.to_path_buf(),
                message: format!("platforms entry invalid: {message}"),
            })?;
        }
        Ok(def)
    }

    pub fn case_dir(&self) -> &Path {
        self.source_path.parent().unwrap_or(Path::new("."))
    }
}

struct PlatformList;

impl PlatformList {
    const VALID: [&str; 5] = [
        "qemu-virtio",
        "raspi5",
        "virt",
        "qemu-isa",
        "riscv64-virt",
    ];

    fn validate(platform: &str) -> Result<(), String> {
        if Self::VALID.contains(&platform) {
            Ok(())
        } else {
            Err(format!(
                "{platform:?} is not one of {}",
                Self::VALID.join(", ")
            ))
        }
    }
}

/// Discover every case under `root`, sorted deterministically by path.
/// Duplicates names are rejected; manifest.toml placeholders are ignored.
pub fn load_cases(root: &Path) -> Result<Vec<CaseDef>, Box<dyn std::error::Error>> {
    if !root.is_dir() {
        return Err(Box::new(CaseError {
            file: root.to_path_buf(),
            message: "case root directory does not exist".to_owned(),
        }));
    }
    let mut cases = Vec::new();
    let mut names: BTreeSet<String> = BTreeSet::new();
    walk_cases(root, root, &mut cases, &mut names)?;
    cases.sort_by(|left, right| left.source_path.cmp(&right.source_path));
    Ok(cases)
}

fn walk_cases(
    root: &Path,
    dir: &Path,
    out: &mut Vec<CaseDef>,
    names: &mut BTreeSet<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut children: Vec<PathBuf> = fs::read_dir(dir)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|entry| entry.path())
        .collect();
    children.sort();
    for child in children {
        if child.is_dir() {
            walk_cases(root, &child, out, names)?;
        } else if child.extension().and_then(|ext| ext.to_str()) == Some("toml") {
            if child.file_name().and_then(|n| n.to_str()) == Some("manifest.toml") {
                continue;
            }
            let case = CaseDef::parse_file(&child)?;
            if !names.insert(case.name.clone()) {
                return Err(Box::new(CaseError {
                    file: child.clone(),
                    message: format!(
                        "duplicate case name {:?} (each name must be unique across all dirs)",
                        case.name
                    ),
                }));
            }
            out.push(case);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_case(root: &Path, rel: &str, contents: &str) -> PathBuf {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, contents).expect("write");
        path
    }

    #[test]
    fn parses_canonical_case_file_schema() {
        let dir = std::env::temp_dir().join(format!(
            "e2e-case-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        write_case(
            &dir,
            "regress/dhcp-rx-delivery.toml",
            r#"
# tests/cases/regress/dhcp-rx-delivery.toml
[case]
name = "regress.dhcp-rx-delivery"
tier = 4
platforms = ["qemu-virtio", "virt"]
timeout_secs = 180
witnesses = [
  "net-selftest end ok",
  "E2E net.address-configured PASS",
]
fail_on = ["FAILED", "E2E net.address-configured FAIL"]
env_build = ["SERVICEOS_E2E_NET=1"]
probes = []
serial_script = ""
data_fresh = true
tags = ["network"]
"#,
        );
        let cases = load_cases(&dir).expect("cases");
        assert_eq!(cases.len(), 1);
        let case = &cases[0];
        assert_eq!(case.name, "regress.dhcp-rx-delivery");
        assert_eq!(case.tier, 4);
        assert_eq!(case.platforms.len(), 2);
        assert_eq!(case.timeout_secs, Some(180));
        assert_eq!(case.env_build, vec![("SERVICEOS_E2E_NET".to_owned(), "1".to_owned())]);
        assert!(case.data_fresh);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_names_are_rejected() {
        let dir = std::env::temp_dir().join(format!(
            "e2e-dup-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let body = r#"
[case]
name = "same"
tier = 1
witnesses = ["boot banner"]
"#;
        write_case(&dir, "smoke/a.toml", body);
        write_case(&dir, "live/b.toml", body);
        let error = load_cases(&dir).expect_err("duplicate rejected");
        assert!(error.to_string().contains("duplicate case name"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_keys_and_bad_values_fail_with_line_context() {
        let dir = std::env::temp_dir().join(format!(
            "e2e-err-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        write_case(
            &dir,
            "smoke/broken.toml",
            r#"
[case]
name = "broken"
witnessses = ["typo key"]
"#,
        );
        let error = load_cases(&dir).expect_err("unknown key");
        assert!(error.to_string().contains("line 4"), "{error}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifest_placeholders_are_ignored() {
        let dir = std::env::temp_dir().join(format!(
            "e2e-manifest-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        write_case(&dir, "manifest.toml", "# reserved metadata\nnot-a-case = true\n");
        write_case(
            &dir,
            "smoke/only.toml",
            "[case]\nname=\"only\"\ntier=2\nwitnesses=[\"marker\"]\n",
        );
        let cases = load_cases(&dir).expect("cases");
        assert_eq!(cases.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }
}

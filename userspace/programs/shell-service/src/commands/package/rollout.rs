//! `pkg rollout` — per-source staged-rollout cohorts and upgrade rules,
//! mirroring the `pkg keys` command shape. The shell speaks the rollout
//! protocol (PackageTag 0x72a..) directly against package-service, so field
//! caps mirror the service-side limits (package-service/src/rollout.rs).

use rt::PackageStatus;
use serviceos_userspace_runtime as rt;

use crate::util::{ShellOutput, write_output_linef};

use super::mutate::{package_status_name, status_from_word};
use super::parse::MAX_PACKAGE_TEXT;

/// Wire limits mirrored from package-service/src/rollout.rs.
const SOURCE_NAME_MAX: usize = 32;
const ARG_MAX: usize = 24;
/// Holds echoed per RolloutGetReply page (service packs one per reply).
const HOLD_PAGE: usize = 1;

/// RolloutSetRequest operation words (package-service/src/rollout.rs).
pub(in crate::commands) const OP_COHORT: u64 = 1;
pub(in crate::commands) const OP_HOLD_ADD: u64 = 2;
pub(in crate::commands) const OP_HOLD_REMOVE: u64 = 3;
pub(in crate::commands) const OP_HOLD_CLEAR: u64 = 4;
pub(in crate::commands) const OP_MIN_RING: u64 = 5;
pub(in crate::commands) const OP_MAX_STEP: u64 = 6;
pub(in crate::commands) const OP_CLEAR: u64 = 7;

const USAGE_ROLLOUT: &str =
    "usage: pkg rollout <list|show|cohort|min-ring|hold|max-step|clear> ...";

/// Gate reason words (RolloutStatusReply.words[2]); mirrors the service's
/// RolloutReason mapping.
pub(in crate::commands) fn reason_name(word: u64) -> &'static str {
    match word {
        1 => "admit",
        2 => "held",
        3 => "cohort",
        4 => "min-ring",
        5 => "max-step",
        _ => "none",
    }
}

fn ring_floor_name(word: u64) -> &'static str {
    match word {
        1 => "production",
        2 => "preview",
        3 => "testing",
        _ => "unknown",
    }
}

fn op_name(op: u64) -> &'static str {
    match op {
        OP_COHORT => "cohort",
        OP_HOLD_ADD => "hold-add",
        OP_HOLD_REMOVE => "hold-remove",
        OP_HOLD_CLEAR => "hold-clear",
        OP_MIN_RING => "min-ring",
        OP_MAX_STEP => "max-step",
        OP_CLEAR => "clear",
        _ => "unknown",
    }
}

/// Fixed-capacity text carrier (no alloc on host or guest).
struct HeapText {
    bytes: [u8; 64],
    len: usize,
}

impl HeapText {
    fn empty() -> Self {
        Self {
            bytes: [0; 64],
            len: 0,
        }
    }

    fn set(&mut self, value: &str) {
        self.bytes = [0; 64];
        let copy = value.len().min(self.bytes.len());
        self.bytes[..copy].copy_from_slice(&value.as_bytes()[..copy]);
        self.len = copy;
    }

    fn push_byte(&mut self, byte: u8) {
        if self.len < self.bytes.len() {
            self.bytes[self.len] = byte;
            self.len += 1;
        }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

/// Cohort display: `none` when fully open, else the percent (the cohort
/// NAME rides the list reply and is shown there as `name:percent`).
fn percent_text(percent: u64) -> HeapText {
    let mut text = HeapText::empty();
    if percent >= 100 {
        text.set("none");
    } else {
        let digits = unsigned_text(percent);
        text.set(digits.as_str());
    }
    text
}

fn unsigned_text(value: u64) -> HeapText {
    let mut digits = [0u8; 20];
    let mut len = 0usize;
    let mut remaining = value;
    loop {
        digits[len] = b'0' + (remaining % 10) as u8;
        len += 1;
        remaining /= 10;
        if remaining == 0 {
            break;
        }
    }
    let mut text = HeapText::empty();
    for index in (0..len).rev() {
        text.push_byte(digits[index]);
    }
    text
}

/// Pack `[len words][inline bytes...]` after the header (keys.rs shape).
fn push_packed(
    message: &mut rt::RawMessage,
    header_words: usize,
    fields: [&[u8]; 2],
    field_count: usize,
) -> rt::Result<()> {
    const COMBINED_MAX: usize = SOURCE_NAME_MAX + ARG_MAX + 1;
    let mut combined = [0u8; COMBINED_MAX];
    let mut cursor = 0usize;
    for field in fields.iter().take(field_count) {
        if cursor + field.len() > COMBINED_MAX {
            return Err(rt::Error::BufferTooSmall);
        }
        combined[cursor..cursor + field.len()].copy_from_slice(field);
        cursor += field.len();
    }
    for (slot, field) in fields.iter().enumerate().take(field_count) {
        message.words[header_words + slot] = field.len() as u64;
    }
    message.word_count = (header_words + field_count) as u32
        + rt::pack_bytes(
            &combined[..cursor],
            &mut message.words[header_words + field_count..],
        )?;
    Ok(())
}

/// Extract one inline string field from `[len][packed bytes..]` at `base`.
fn reply_string<'a>(
    words: &'a [u64],
    word_count: u32,
    base: usize,
    out: &'a mut [u8],
) -> rt::Result<&'a str> {
    if word_count as usize <= base || out.is_empty() {
        return Err(rt::Error::InvalidArgument);
    }
    let len = words[base] as usize;
    if len == 0 || len > out.len() || word_count as usize <= base + 1 {
        return Err(rt::Error::InvalidArgument);
    }
    rt::unpack_bytes(&words[base + 1..word_count as usize], len, &mut out[..len])?;
    core::str::from_utf8(&out[..len]).map_err(|_| rt::Error::InvalidArgument)
}

struct RolloutHandle {
    package_handle: rt::Handle,
}

impl RolloutHandle {
    fn open(bootstrap: rt::Handle) -> rt::Result<Self> {
        Ok(Self {
            package_handle: rt::lookup_service(bootstrap, rt::ServiceId::Package)?,
        })
    }

    /// One rollout request/reply round trip; validates the reply tag.
    fn call(
        &self,
        tag: rt::PackageTag,
        fill: impl FnOnce(&mut rt::RawMessage),
    ) -> rt::Result<rt::RawMessage> {
        let mut request = rt::RawMessage::empty(tag as u32);
        fill(&mut request);
        let response = rt::channel_call(self.package_handle, &mut request)?;
        let expected = match tag {
            rt::PackageTag::RolloutListRequest => rt::PackageTag::RolloutListReply,
            rt::PackageTag::RolloutGetRequest => rt::PackageTag::RolloutGetReply,
            rt::PackageTag::RolloutSetRequest => rt::PackageTag::RolloutSetReply,
            rt::PackageTag::RolloutStatusRequest => rt::PackageTag::RolloutStatusReply,
            _ => return Err(rt::Error::InvalidArgument),
        };
        if response.tag != expected as u32 {
            return Err(rt::Error::InvalidArgument);
        }
        Ok(response)
    }
}

impl Drop for RolloutHandle {
    fn drop(&mut self) {
        let _ = rt::handle_close(self.package_handle);
    }
}

pub(in crate::commands) fn cmd_pkg_rollout<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    mut parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("list") => cmd_rollout_list(bootstrap, output),
        Some("show") => match parts.next() {
            Some(source) if !source.is_empty() && source.len() <= SOURCE_NAME_MAX => {
                cmd_rollout_show(bootstrap, output, source)
            }
            _ => write_output_linef(output, format_args!("usage: pkg rollout show <source>")),
        },
        Some("cohort") => match (parts.next(), parts.next()) {
            (Some(source), Some(spec)) => {
                cmd_rollout_set(bootstrap, output, source, OP_COHORT, 0, spec)
            }
            _ => write_output_linef(
                output,
                format_args!("usage: pkg rollout cohort <source> <0-100|none|name:percent>"),
            ),
        },
        Some("min-ring") => match (parts.next(), parts.next()) {
            (Some(source), Some(ring)) => match ring {
                "production" => cmd_rollout_set(bootstrap, output, source, OP_MIN_RING, 1, ""),
                "preview" => cmd_rollout_set(bootstrap, output, source, OP_MIN_RING, 2, ""),
                "testing" => cmd_rollout_set(bootstrap, output, source, OP_MIN_RING, 3, ""),
                _ => write_output_linef(
                    output,
                    format_args!(
                        "usage: pkg rollout min-ring <source> <production|preview|testing>"
                    ),
                ),
            },
            _ => write_output_linef(
                output,
                format_args!("usage: pkg rollout min-ring <source> <production|preview|testing>"),
            ),
        },
        Some("hold") => match (parts.next(), parts.next(), parts.next()) {
            (Some(source), Some("add"), Some(name)) if !name.is_empty() => {
                cmd_rollout_set(bootstrap, output, source, OP_HOLD_ADD, 0, name)
            }
            (Some(source), Some("remove") | Some("rm"), Some(name)) if !name.is_empty() => {
                cmd_rollout_set(bootstrap, output, source, OP_HOLD_REMOVE, 0, name)
            }
            (Some(source), Some("clear"), None) | (Some(source), Some("clear"), Some("all")) => {
                cmd_rollout_set(bootstrap, output, source, OP_HOLD_CLEAR, 0, "")
            }
            _ => write_output_linef(
                output,
                format_args!("usage: pkg rollout hold <source> <add|remove|clear> [name]"),
            ),
        },
        Some("max-step") => match (parts.next(), parts.next()) {
            (Some(source), Some(value)) if value == "none" => {
                cmd_rollout_set(bootstrap, output, source, OP_MAX_STEP, 0, "")
            }
            (Some(source), Some(value)) => match value.parse::<u32>() {
                Ok(step) => {
                    cmd_rollout_set(bootstrap, output, source, OP_MAX_STEP, u64::from(step), "")
                }
                Err(_) => write_output_linef(
                    output,
                    format_args!("usage: pkg rollout max-step <source> <N|none>"),
                ),
            },
            _ => write_output_linef(
                output,
                format_args!("usage: pkg rollout max-step <source> <N|none>"),
            ),
        },
        Some("clear") => match parts.next() {
            Some(source) if !source.is_empty() => {
                cmd_rollout_set(bootstrap, output, source, OP_CLEAR, 0, "")
            }
            _ => write_output_linef(output, format_args!("usage: pkg rollout clear <source>")),
        },
        _ => write_output_linef(output, format_args!("{USAGE_ROLLOUT}")),
    }
}

/// `pkg rollout list` — one summary row per configured source policy.
fn cmd_rollout_list(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let rollout = RolloutHandle::open(bootstrap)?;
    let mut index = 0u64;
    loop {
        let reply = rollout.call(rt::PackageTag::RolloutListRequest, |request| {
            request.word_count = 1;
            request.words[0] = index;
        })?;
        if reply.word_count < 8 {
            return Err(rt::Error::InvalidArgument);
        }
        let status = status_from_word(reply.words[0]);
        if status == PackageStatus::End {
            break;
        }
        if status != PackageStatus::Ok {
            write_output_linef(
                output,
                format_args!("rollout list failed: {}", package_status_name(status)),
            )?;
            return Ok(());
        }
        let percent = reply.words[2];
        let min_ring = reply.words[3];
        let max_step = reply.words[4];
        let hold_count = reply.words[5];
        let source_len = reply.words[6] as usize;
        let name_len = reply.words[7] as usize;
        if source_len == 0 || source_len > SOURCE_NAME_MAX || name_len > MAX_PACKAGE_TEXT {
            return Err(rt::Error::InvalidArgument);
        }
        let mut names = [0u8; SOURCE_NAME_MAX + MAX_PACKAGE_TEXT];
        let (source_text, cohort_name) =
            reply_two_fields(&reply.words, reply.word_count, 6, 7, &mut names)?;
        let cohort = cohort_line(percent, cohort_name);
        let step = step_text(max_step);
        write_output_linef(
            output,
            format_args!(
                "src={} cohort={} min-ring={} max-step={} holds={}",
                source_text,
                cohort.as_str(),
                ring_floor_name(min_ring),
                step.as_str(),
                hold_count,
            ),
        )?;
        index += 1;
    }
    if index == 0 {
        write_output_linef(
            output,
            format_args!("no rollout policies (all sources unstaged)"),
        )
    } else {
        Ok(())
    }
}

/// `pkg rollout show <source>` — full rules incl. paged hold names.
fn cmd_rollout_show(bootstrap: rt::Handle, output: ShellOutput, source: &str) -> rt::Result<()> {
    let rollout = RolloutHandle::open(bootstrap)?;
    let cohort_name = match resolve_cohort_name(&rollout, source) {
        Ok(text) => text,
        Err(_) => {
            write_output_linef(output, format_args!("rollout show failed: unknown source"))?;
            return Ok(());
        }
    };
    let mut page = 0u64;
    loop {
        let reply = rollout.call(rt::PackageTag::RolloutGetRequest, |request| {
            request.word_count = 1;
            request.words[0] = page;
            let _ = push_packed(request, 1, [source.as_bytes(), b""], 1);
        })?;
        if reply.word_count < 8 {
            return Err(rt::Error::InvalidArgument);
        }
        let status = status_from_word(reply.words[0]);
        if status == PackageStatus::NotFound {
            write_output_linef(output, format_args!("rollout show failed: unknown source"))?;
            return Ok(());
        }
        if status != PackageStatus::Ok {
            write_output_linef(
                output,
                format_args!("rollout show failed: {}", package_status_name(status)),
            )?;
            return Ok(());
        }
        let percent = reply.words[1];
        let min_ring = reply.words[2];
        let max_step = reply.words[3];
        let hold_total = reply.words[4];
        let page_count = reply.words[5] as usize;
        let page_start = reply.words[6] as usize;
        if page_count > HOLD_PAGE {
            return Err(rt::Error::InvalidArgument);
        }
        if page == 0 {
            let cohort = cohort_line(percent, cohort_name.as_str());
            let step = step_text(max_step);
            write_output_linef(
                output,
                format_args!(
                    "{} cohort={} min-ring={} max-step={} holds={}",
                    source,
                    cohort.as_str(),
                    ring_floor_name(min_ring),
                    step.as_str(),
                    hold_total,
                ),
            )?;
        }
        if page_count > 0 {
            let mut holds_out = [0u8; MAX_PACKAGE_TEXT];
            let page_text = reply_string(&reply.words, reply.word_count, 7, &mut holds_out)?;
            let index = page_start;
            write_output_linef(output, format_args!("  hold[{}]: {}", index, page_text))?;
        }
        if page_count < HOLD_PAGE {
            break;
        }
        page += 1;
    }
    Ok(())
}

/// Find a source's row via the flat listing (also yields the cohort name).
fn resolve_cohort_name(rollout: &RolloutHandle, source: &str) -> rt::Result<HeapText> {
    let mut index = 0u64;
    loop {
        let reply = rollout.call(rt::PackageTag::RolloutListRequest, |request| {
            request.word_count = 1;
            request.words[0] = index;
        })?;
        if reply.word_count < 8 {
            return Err(rt::Error::InvalidArgument);
        }
        let status = status_from_word(reply.words[0]);
        if status == PackageStatus::End {
            return Err(rt::Error::InvalidArgument);
        }
        if status != PackageStatus::Ok {
            return Err(rt::Error::InvalidArgument);
        }
        let source_len = reply.words[6] as usize;
        let name_len = reply.words[7] as usize;
        if source_len == 0 || source_len > SOURCE_NAME_MAX || name_len > MAX_PACKAGE_TEXT {
            return Err(rt::Error::InvalidArgument);
        }
        let mut names = [0u8; SOURCE_NAME_MAX + MAX_PACKAGE_TEXT];
        let (row_source, cohort_name) =
            reply_two_fields(&reply.words, reply.word_count, 6, 7, &mut names)?;
        if row_source == source {
            let mut text = HeapText::empty();
            text.set(cohort_name);
            return Ok(text);
        }
        index += 1;
    }
}

/// `pkg rollout cohort|min-ring|hold|max-step|clear` mutations. One rule per
/// request; the service persists the table before replying Ok.
fn cmd_rollout_set(
    bootstrap: rt::Handle,
    output: ShellOutput,
    source: &str,
    op: u64,
    value: u64,
    argument: &str,
) -> rt::Result<()> {
    if source.is_empty() || source.len() > SOURCE_NAME_MAX || argument.len() > ARG_MAX {
        write_output_linef(output, format_args!("rollout set failed: invalid argument"))?;
        return Ok(());
    }
    if op == OP_COHORT && parse_cohort_arg(argument).is_none() {
        write_output_linef(
            output,
            format_args!("usage: pkg rollout cohort <source> <0-100|none|name:percent>"),
        )?;
        return Ok(());
    }
    let rollout = RolloutHandle::open(bootstrap)?;
    // Argument-bearing ops pack [len][len][bytes]; source-only ops pack one
    // field — matching the service-side readers exactly.
    let field_count = if argument.is_empty() { 1 } else { 2 };
    let reply = rollout.call(rt::PackageTag::RolloutSetRequest, |request| {
        request.words[0] = op;
        request.words[1] = value;
        let _ = push_packed(
            request,
            2,
            [source.as_bytes(), argument.as_bytes()],
            field_count,
        );
    })?;
    if reply.word_count < 1 {
        return Err(rt::Error::InvalidArgument);
    }
    let status = status_from_word(reply.words[0]);
    if status == PackageStatus::Ok {
        write_output_linef(output, format_args!("ok src={} op={}", source, op_name(op)))?;
    } else {
        write_output_linef(
            output,
            format_args!(
                "rollout set failed: {} (src={})",
                package_status_name(status),
                source
            ),
        )?;
    }
    Ok(())
}

/// Validate a cohort argument locally: `0-100`, `none`, or `name:percent`
/// where name is at most 24 bytes and free of `|`/`,`.
pub(in crate::commands) fn parse_cohort_arg(value: &str) -> Option<(u32, usize)> {
    if value == "none" {
        return Some((100, 0));
    }
    let (name, percent_text) = match value.split_once(':') {
        Some((name, percent)) => (name, percent),
        None => ("", value),
    };
    if name.len() > 24 || name.contains('|') || name.contains(',') {
        return None;
    }
    let percent = percent_text.parse::<u32>().ok()?;
    if percent > 100 {
        return None;
    }
    Some((percent, name.len()))
}

/// Extract `(first, second)` from packed fields with lens at `base_a`/`base_b`.
fn reply_two_fields<'a>(
    words: &'a [u64],
    word_count: u32,
    base_first: usize,
    base_second: usize,
    out: &'a mut [u8],
) -> rt::Result<(&'a str, &'a str)> {
    let len_a = words.get(base_first).copied().unwrap_or(0) as usize;
    let len_b = words
        .get(base_second)
        .copied()
        .map(|v| v as usize)
        .unwrap_or(0);
    let data_base = base_second + 1;
    if word_count as usize <= data_base || len_a + len_b == 0 || len_a + len_b > out.len() {
        return Err(rt::Error::InvalidArgument);
    }
    rt::unpack_bytes(
        &words[data_base..word_count as usize],
        len_a + len_b,
        &mut out[..],
    )?;
    let first = core::str::from_utf8(&out[..len_a]).map_err(|_| rt::Error::InvalidArgument)?;
    let second =
        core::str::from_utf8(&out[len_a..len_a + len_b]).map_err(|_| rt::Error::InvalidArgument)?;
    Ok((first, second))
}

/// Consult the service's gated update decision for one installed package.
/// Mirrors the automatic `pkg update` path exactly (full gate stack).
/// Returns `(offered, reason_word)`; `(None, 0)` means the gate could not
/// be consulted, so the caller keeps the pre-policy display flag.
pub(in crate::commands) fn gated_update_offered(
    bootstrap: rt::Handle,
    service_id: rt::ServiceId,
) -> (Option<bool>, u64) {
    let consult = || -> rt::Result<(Option<bool>, u64)> {
        let rollout = RolloutHandle::open(bootstrap)?;
        let reply = rollout.call(rt::PackageTag::RolloutStatusRequest, |request| {
            request.word_count = 1;
            request.words[0] = service_id as u32 as u64;
        })?;
        if reply.word_count < 3 {
            return Err(rt::Error::InvalidArgument);
        }
        if status_from_word(reply.words[0]) != PackageStatus::Ok {
            return Ok((None, 0));
        }
        Ok((Some(reply.words[1] == 1), reply.words[2]))
    };
    consult().unwrap_or((None, 0))
}

fn cohort_line(percent: u64, cohort_name: &str) -> HeapText {
    let mut text = HeapText::empty();
    if cohort_name.is_empty() {
        let digits = percent_text(percent);
        text.set(digits.as_str());
    } else {
        text.set(cohort_name);
        if percent < 100 {
            text.push_byte(b':');
            let digits = unsigned_text(percent);
            for byte in digits.as_str().bytes() {
                text.push_byte(byte);
            }
        }
    }
    text
}

fn step_text(value: u64) -> HeapText {
    if value == 0 {
        let mut text = HeapText::empty();
        text.set("none");
        text
    } else {
        unsigned_text(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_and_op_names_cover_wire_words() {
        assert_eq!(reason_name(0), "none");
        assert_eq!(reason_name(1), "admit");
        assert_eq!(reason_name(2), "held");
        assert_eq!(reason_name(3), "cohort");
        assert_eq!(reason_name(4), "min-ring");
        assert_eq!(reason_name(5), "max-step");
        assert_eq!(reason_name(9), "none");
        assert_eq!(op_name(OP_COHORT), "cohort");
        assert_eq!(op_name(OP_HOLD_ADD), "hold-add");
        assert_eq!(op_name(OP_HOLD_CLEAR), "hold-clear");
        assert_eq!(op_name(OP_MIN_RING), "min-ring");
        assert_eq!(op_name(OP_MAX_STEP), "max-step");
        assert_eq!(op_name(OP_CLEAR), "clear");
        assert_eq!(op_name(42), "unknown");
    }

    #[test]
    fn cohort_arg_validation_mirrors_service_grammar() {
        assert_eq!(parse_cohort_arg("none"), Some((100, 0)));
        assert_eq!(parse_cohort_arg("0"), Some((0, 0)));
        assert_eq!(parse_cohort_arg("25"), Some((25, 0)));
        assert_eq!(parse_cohort_arg("100"), Some((100, 0)));
        assert_eq!(parse_cohort_arg("wave:40"), Some((40, 4)));
        assert_eq!(parse_cohort_arg("101"), None);
        assert_eq!(parse_cohort_arg("wave:101"), None);
        assert_eq!(parse_cohort_arg("wave:"), None);
        assert_eq!(parse_cohort_arg("a|b:10"), None);
        assert_eq!(parse_cohort_arg("a,b:10"), None);
        let mut long = std::string::String::new();
        for _ in 0..24 {
            long.push('x');
        }
        let mut too_long = long.clone();
        too_long.push('x');
        assert_eq!(parse_cohort_arg(&std::format!("{}:10", too_long)), None);
        assert_eq!(
            parse_cohort_arg(&std::format!("{}:10", long)),
            Some((10, 24))
        );
    }

    #[test]
    fn percent_and_step_text_render_bounds() {
        assert_eq!(percent_text(100).as_str(), "none");
        assert_eq!(percent_text(0).as_str(), "0");
        assert_eq!(percent_text(37).as_str(), "37");
        assert_eq!(step_text(0).as_str(), "none");
        assert_eq!(step_text(5).as_str(), "5");
        assert_eq!(step_text(10_000).as_str(), "10000");
    }

    #[test]
    fn push_packed_sets_lengths_and_word_count() {
        let mut message = rt::RawMessage::empty(0x72e);
        message.words[0] = 5;
        push_packed(&mut message, 2, [b"boot", b"netd"], 2).unwrap();
        assert_eq!(message.words[2], 4);
        assert_eq!(message.words[3], 4);
        assert_eq!(message.word_count, 4 + 1);
        // Source-only ops pack ONE field: [.. header ..][len][bytes..].
        let mut single = rt::RawMessage::empty(0x72e);
        push_packed(&mut single, 2, [b"boot", b""], 1).unwrap();
        assert_eq!(single.words[2], 4);
        assert_eq!(single.word_count, 3 + 1);
        let mut out = [0u8; 8];
        rt::unpack_bytes(&single.words[3..single.word_count as usize], 4, &mut out).unwrap();
        assert_eq!(&out[..4], b"boot");
    }

    #[test]
    fn reply_two_fields_roundtrips_source_and_cohort_name() {
        // Mirrors the RolloutListReply layout: lens at words[6]/words[7].
        let mut message = rt::RawMessage::empty(0x72b);
        for slot in 0..6 {
            message.words[slot] = 0;
        }
        push_packed_names(&mut message, 6, b"edge", b"wave");
        let mut buffer = [0u8; SOURCE_NAME_MAX + MAX_PACKAGE_TEXT];
        let (source, name) =
            reply_two_fields(&message.words, message.word_count, 6, 7, &mut buffer).unwrap();
        assert_eq!(source, "edge");
        assert_eq!(name, "wave");
    }

    /// Layout helper mirroring the service's two-field packed tail.
    fn push_packed_names(message: &mut rt::RawMessage, base: usize, first: &[u8], second: &[u8]) {
        message.words[base] = first.len() as u64;
        message.words[base + 1] = second.len() as u64;
        let mut combined = [0u8; 128];
        let total = first.len() + second.len();
        combined[..first.len()].copy_from_slice(first);
        combined[first.len()..total].copy_from_slice(second);
        message.word_count = (base + 2) as u32
            + rt::pack_bytes(&combined[..total], &mut message.words[base + 2..]).unwrap();
    }

    #[test]
    fn reply_string_roundtrips_single_field() {
        let mut message = rt::RawMessage::empty(0x72d);
        message.words[7] = 5;
        message.word_count = 8;
        message.word_count += rt::pack_bytes(b"netd,", &mut message.words[8..]).unwrap();
        let mut out = [0u8; MAX_PACKAGE_TEXT];
        assert_eq!(
            reply_string(&message.words, message.word_count, 7, &mut out).unwrap(),
            "netd,"
        );
    }
}

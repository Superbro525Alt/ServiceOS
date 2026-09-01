use serviceos_userspace_runtime as rt;

// Developer-service job surface, mirrored from
// userspace/programs/developer-service (tags shared/abi/src/developer.rs,
// IDE tails protocol.rs). This app holds no developer-service channel grant
// (root-manager launch.rs grants Package only), so the panel is honest
// read-only: decoders stay host-tested but no transport is wired.

pub(crate) const MAX_JOBS: usize = 8;
pub(crate) const MAX_ARTIFACT_NAME: usize = 64;
pub(crate) const MAX_ENDPOINT_BYTES: usize = 96;

pub(crate) const TAG_JOB_LIST_REQUEST: u32 = 0xd0a;
pub(crate) const TAG_JOB_LIST_REPLY: u32 = 0xd0b;
pub(crate) const TAG_JOB_INFO_REQUEST: u32 = 0xd0c;
pub(crate) const TAG_JOB_INFO_REPLY: u32 = 0xd0d;
pub(crate) const TAG_IDE_SNAPSHOT_REQUEST: u32 = 0xd20;
pub(crate) const TAG_IDE_SNAPSHOT_REPLY: u32 = 0xd21;

pub(crate) const STATUS_OK: u32 = 0;
pub(crate) const STATUS_NOT_FOUND: u32 = 1;

pub(crate) const ROUTE_KIND_DIRECT: u32 = 0;
pub(crate) const ROUTE_KIND_RUNTIME_ENV: u32 = 1;
pub(crate) const ROUTE_KIND_REMOTE_FARM: u32 = 2;

pub(crate) const EXPORT_STATE_LOCAL: u32 = 0;
pub(crate) const EXPORT_STATE_PENDING: u32 = 1;

pub(crate) const FARM_STATUS_REGISTERED: u32 = 0;
pub(crate) const FARM_STATUS_NOT_CONFIGURED: u32 = 1;
pub(crate) const FARM_STATUS_UNREACHABLE: u32 = 2;

pub(crate) const TOOLCHAIN_UNRESOLVED: u32 = u32::MAX;

/// "IDE1" self-describing tail magic; field count rides bits 32..40.
pub(crate) const IDE_TAIL_MAGIC: u64 = 0x4944_4531;

pub(crate) fn ide_tail_field_count(word: u64) -> Option<usize> {
    if word & 0xFFFF_FFFF != IDE_TAIL_MAGIC {
        return None;
    }
    Some(((word >> 32) & 0xFF) as usize)
}

/// state | route_kind << 8 | export_state << 16, exactly as
/// developer-service's pack_phase builds it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct JobPhase {
    pub(crate) state: u32,
    pub(crate) route_kind: u32,
    pub(crate) export_state: u32,
}

pub(crate) fn phase_from_word(word: u64) -> JobPhase {
    JobPhase {
        state: (word & 0xFF) as u32,
        route_kind: ((word >> 8) & 0xFF) as u32,
        export_state: ((word >> 16) & 0xFF) as u32,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DevJob {
    pub(crate) job_id: u32,
    pub(crate) workspace_id: u32,
    pub(crate) target: u32,
    pub(crate) state: u32,
    pub(crate) format: u32,
    pub(crate) artifact_size: u64,
    /// IDE1 tail payloads; None when the reply carried no (valid) tail.
    pub(crate) phase: Option<JobPhase>,
    pub(crate) toolchain: Option<u32>,
    pub(crate) has_artifact: bool,
    pub(crate) name: [u8; MAX_ARTIFACT_NAME],
    pub(crate) name_len: usize,
    pub(crate) farm_status: Option<u32>,
    pub(crate) endpoint: [u8; MAX_ENDPOINT_BYTES],
    pub(crate) endpoint_len: usize,
    pub(crate) exec_mode: Option<u64>,
}

impl DevJob {
    pub(crate) const fn empty() -> Self {
        Self {
            job_id: 0,
            workspace_id: 0,
            target: 0,
            state: 0,
            format: 0,
            artifact_size: 0,
            phase: None,
            toolchain: None,
            has_artifact: false,
            name: [0; MAX_ARTIFACT_NAME],
            name_len: 0,
            farm_status: None,
            endpoint: [0; MAX_ENDPOINT_BYTES],
            endpoint_len: 0,
            exec_mode: None,
        }
    }

    pub(crate) fn name_text(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("?")
    }

    pub(crate) fn endpoint_text(&self) -> &str {
        core::str::from_utf8(&self.endpoint[..self.endpoint_len]).unwrap_or("?")
    }
}

/// Unpack `len` little-endian bytes from `words` (rt::unpack_bytes shape).
fn unpack_into(words: &[u64], len: usize, destination: &mut [u8]) {
    let mut copied = 0usize;
    for word in words.iter().copied() {
        if copied >= len {
            break;
        }
        let bytes = word.to_le_bytes();
        let chunk = (len - copied).min(bytes.len());
        destination[copied..copied + chunk].copy_from_slice(&bytes[..chunk]);
        copied += chunk;
    }
}

/// Optional additive IDE tail: [phase][toolchain][flags] with the grammar
/// grown to [..][duration][rate|valid] (field count 5). The panel reads
/// the self-describing prefix and skips the new trailing fields.
fn apply_list_tail(job: &mut DevJob, words: &[u64]) {
    let fields = ide_tail_field_count(words[7]);
    if !matches!(fields, Some(3) | Some(5)) || words.len() < 11 {
        return;
    }
    let tail = &words[8..11];
    job.phase = Some(phase_from_word(tail[0]));
    job.toolchain = Some(tail[1] as u32);
    job.has_artifact = tail[2] & 1 != 0;
    job.name_len = ((tail[2] >> 8) & 0xFF) as usize;
}

/// Core JobListReply/JobInfoReply words: [status][index][workspace][target]
/// [state][format][size] (+[name_len] on info replies).
fn decode_core(tag: u32, words: &[u64], core_words: usize) -> Option<DevJob> {
    if tag != TAG_JOB_LIST_REPLY && tag != TAG_JOB_INFO_REPLY {
        return None;
    }
    if words.len() < core_words || words[0] as u32 != STATUS_OK {
        return None;
    }
    let mut job = DevJob::empty();
    job.job_id = words[1] as u32;
    job.workspace_id = words[2] as u32;
    job.target = words[3] as u32;
    job.state = words[4] as u32;
    job.format = words[5] as u32;
    job.artifact_size = words[6];
    Some(job)
}

pub(crate) fn decode_job_list_reply(tag: u32, words: &[u64]) -> Option<DevJob> {
    if tag != TAG_JOB_LIST_REPLY {
        return None;
    }
    let mut job = decode_core(tag, words, 7)?;
    if words.len() >= 8 {
        apply_list_tail(&mut job, words);
    }
    Some(job)
}

pub(crate) fn decode_job_info_reply(tag: u32, words: &[u64]) -> Option<DevJob> {
    if tag != TAG_JOB_INFO_REPLY {
        return None;
    }
    let mut job = decode_core(tag, words, 8)?;
    let name_len = (words[7] as usize).min(MAX_ARTIFACT_NAME);
    if words[7] as usize > MAX_ARTIFACT_NAME {
        return None;
    }
    let name_words = name_len.div_ceil(8);
    if words.len() < 8 + name_words {
        return None;
    }
    job.name_len = name_len;
    unpack_into(&words[8..8 + name_words], name_len, &mut job.name);
    // 5-field tail [phase][toolchain][flags][farm][exec] only when complete.
    let base = 8 + name_words;
    if ide_tail_field_count(words.get(base).copied().unwrap_or(0)) == Some(5)
        && words.len() >= base + 6
    {
        let tail = &words[base + 1..base + 6];
        job.phase = Some(phase_from_word(tail[0]));
        job.toolchain = Some(tail[1] as u32);
        job.has_artifact = tail[2] & 1 != 0;
        job.farm_status = Some((tail[3] >> 16) as u32 & 0xFF);
        job.exec_mode = Some(tail[4]);
    }
    Some(job)
}

pub(crate) fn decode_ide_snapshot(tag: u32, words: &[u64]) -> Option<DevJob> {
    if tag != TAG_IDE_SNAPSHOT_REPLY || words.len() < 7 || words[0] as u32 != STATUS_OK {
        return None;
    }
    let mut job = DevJob::empty();
    job.job_id = words[1] as u32;
    job.phase = Some(phase_from_word(words[2]));
    job.workspace_id = (words[3] & 0xFFFF_FFFF) as u32;
    let toolchain = (words[3] >> 32) as u32;
    job.toolchain = (toolchain != TOOLCHAIN_UNRESOLVED).then_some(toolchain);
    job.artifact_size = words[4] & 0xFFFF_FFFF;
    job.format = ((words[4] >> 32) & 0xFF) as u32;
    job.has_artifact = (words[4] >> 40) & 1 != 0;
    job.farm_status = Some((words[5] & 0xFF) as u32);
    let endpoint_len = ((words[5] >> 8) & 0xFF) as usize;
    let name_len = (words[6] as usize).min(MAX_ARTIFACT_NAME);
    let total = name_len
        .saturating_add(endpoint_len)
        .min(MAX_ARTIFACT_NAME + MAX_ENDPOINT_BYTES);
    let byte_words = total.div_ceil(8);
    if words.len() < 7 + byte_words {
        // Byte blob truncated in flight: keep the header fields, drop bytes.
        return Some(job);
    }
    let mut combined = [0u8; MAX_ARTIFACT_NAME + MAX_ENDPOINT_BYTES];
    unpack_into(&words[7..7 + byte_words], total, &mut combined);
    job.name_len = name_len;
    job.name[..name_len].copy_from_slice(&combined[..name_len]);
    let fitted = endpoint_len.min(MAX_ENDPOINT_BYTES);
    job.endpoint_len = fitted;
    job.endpoint[..fitted].copy_from_slice(&combined[name_len..name_len + fitted]);
    let base = 7 + byte_words;
    if ide_tail_field_count(words.get(base).copied().unwrap_or(0)) == Some(1)
        && words.len() >= base + 2
    {
        job.exec_mode = Some(words[base + 1]);
    }
    Some(job)
}

pub(crate) fn job_list_request(index: usize, reply_endpoint: rt::Handle) -> rt::RawMessage {
    let mut request = rt::RawMessage::empty(TAG_JOB_LIST_REQUEST);
    request.word_count = 1;
    request.words[0] = index as u64;
    request.handle_count = 1;
    request.handles[0] = reply_endpoint;
    request.handle_rights[0] = rt::rights::SEND;
    request
}

pub(crate) fn job_info_request(job_id: u32, reply_endpoint: rt::Handle) -> rt::RawMessage {
    let mut request = rt::RawMessage::empty(TAG_JOB_INFO_REQUEST);
    request.word_count = 1;
    request.words[0] = job_id as u64;
    request.handle_count = 1;
    request.handles[0] = reply_endpoint;
    request.handle_rights[0] = rt::rights::SEND;
    request
}

pub(crate) fn ide_snapshot_request(job_id: u32, reply_endpoint: rt::Handle) -> rt::RawMessage {
    let mut request = rt::RawMessage::empty(TAG_IDE_SNAPSHOT_REQUEST);
    request.word_count = 1;
    request.words[0] = job_id as u64;
    request.handle_count = 1;
    request.handles[0] = reply_endpoint;
    request.handle_rights[0] = rt::rights::SEND;
    request
}

pub(crate) fn job_state_name(state: u32) -> &'static str {
    match state {
        1 => "queued",
        2 => "running",
        3 => "succeeded",
        4 => "failed",
        5 => "unsupported",
        _ => "?",
    }
}

pub(crate) fn format_name(format: u32) -> &'static str {
    match format {
        1 => "flat",
        2 => "elf64",
        3 => "pe32+",
        4 => "macho64",
        _ => "?",
    }
}

pub(crate) fn target_name(target: u32) -> &'static str {
    match target {
        1 => "native-x64",
        2 => "linux-x64",
        3 => "windows-x64",
        4 => "macos-x64",
        _ => "?",
    }
}

pub(crate) fn route_kind_name(route: u32) -> &'static str {
    match route {
        ROUTE_KIND_DIRECT => "direct",
        ROUTE_KIND_RUNTIME_ENV => "routed-env",
        ROUTE_KIND_REMOTE_FARM => "remote-farm",
        _ => "?",
    }
}

pub(crate) fn export_state_name(export: u32) -> &'static str {
    match export {
        EXPORT_STATE_LOCAL => "local",
        EXPORT_STATE_PENDING => "farm-pending",
        _ => "?",
    }
}

pub(crate) fn farm_status_name(status: u32) -> &'static str {
    match status {
        FARM_STATUS_REGISTERED => "registered",
        FARM_STATUS_NOT_CONFIGURED => "none",
        FARM_STATUS_UNREACHABLE => "unreachable",
        _ => "?",
    }
}

/// Human summary of an ExecutionMode status word:
/// `mode(2 bits) | env_id(16 bits at 8..24) | reason(8 bits at 24)`.
pub(crate) fn write_exec_mode(word: u64, out: &mut rt::FixedLogBuffer<32>) {
    use core::fmt::Write as _;
    let env = (word >> 8) & 0xFFFF;
    let _ = match word & 0b11 {
        0 => write!(out, "direct"),
        1 => write!(out, "routed-env:{}", env),
        2 => write!(out, "routed-fallback:{}", env),
        _ => write!(out, "?"),
    };
}

/// Operator pointers shown when the panel cannot reach developer-service.
pub(crate) const CHANNEL_NOTE: &str =
    "this app holds no developer-service channel; builds run from the shell";
pub(crate) const OPERATOR_NOTE: &str = "shell: dev build <workspace-id> <target> | dev jobs | dev artifact <id> | dev save <id> <path>";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DevKey {
    None,
    Back,
}

pub(crate) fn handle_key(state: &mut DevState, key: u32) -> DevKey {
    use crate::state::{KEY_DOWN, KEY_ESC, KEY_UP};
    match key {
        KEY_UP => {
            state.move_selection(-1);
            DevKey::None
        }
        KEY_DOWN => {
            state.move_selection(1);
            DevKey::None
        }
        KEY_ESC => DevKey::Back,
        _ => DevKey::None,
    }
}

/// Left-list meta line for one job row: workspace, phase state, route.
pub(crate) fn row_meta(job: &DevJob) -> crate::repositories::heapless_line::Line {
    use core::fmt::Write as _;
    let mut line = crate::repositories::heapless_line::Line::new();
    let phase = job.phase;
    let _ = write!(
        &mut line,
        "ws{} {} {}",
        job.workspace_id,
        job_state_name(job.state),
        route_kind_name(phase.map(|phase| phase.route_kind).unwrap_or(u32::MAX)),
    );
    line
}

/// Right-pane detail lines for one job: artifact, size/format, workspace/
/// target, toolchain/exec, farm/export (+endpoint when exported). Missing
/// tail data renders as dashes rather than invented values.
pub(crate) fn detail_lines(job: &DevJob) -> [crate::repositories::heapless_line::Line; 5] {
    use core::fmt::Write as _;
    type Line = crate::repositories::heapless_line::Line;
    let mut lines = [
        Line::new(),
        Line::new(),
        Line::new(),
        Line::new(),
        Line::new(),
    ];
    let _ = write!(&mut lines[0], "artifact: {}", job.name_text());
    let _ = write!(
        &mut lines[1],
        "size: {} B  format: {}",
        job.artifact_size,
        format_name(job.format),
    );
    let _ = write!(
        &mut lines[2],
        "workspace: {}  target: {}",
        job.workspace_id,
        target_name(job.target),
    );
    let mut exec = rt::FixedLogBuffer::<32>::new();
    match job.exec_mode {
        Some(word) => write_exec_mode(word, &mut exec),
        None => {
            let _ = core::fmt::Write::write_str(&mut exec, "?");
        }
    }
    let exec_text = core::str::from_utf8(exec.as_bytes()).unwrap_or("?");
    match job.toolchain {
        Some(slot) if slot != TOOLCHAIN_UNRESOLVED => {
            let _ = write!(
                &mut lines[3],
                "toolchain slot: {}  exec: {}",
                slot, exec_text,
            );
        }
        _ => {
            let _ = write!(&mut lines[3], "toolchain slot: -  exec: {}", exec_text);
        }
    }
    let _ = write!(
        &mut lines[4],
        "farm: {}  export: {}",
        farm_status_name(job.farm_status.unwrap_or(FARM_STATUS_NOT_CONFIGURED)),
        export_state_name(
            job.phase
                .map(|phase| phase.export_state)
                .unwrap_or(u32::MAX)
        ),
    );
    if job.endpoint_len > 0 {
        let _ = write!(&mut lines[4], "  endpoint: {}", job.endpoint_text());
    }
    lines
}

/// Panel state for the developer/jobs surface. Mirrors the sources panel:
/// `available` stays false until real developer-service data lands, so the
/// default render is an honest unavailable notice, never a fake list.
pub(crate) struct DevState {
    pub(crate) open: bool,
    pub(crate) available: bool,
    pub(crate) jobs: [DevJob; MAX_JOBS],
    pub(crate) job_count: usize,
    pub(crate) selected: usize,
    pub(crate) scroll: usize,
}

impl DevState {
    pub(crate) const fn new() -> Self {
        Self {
            open: false,
            available: false,
            jobs: [DevJob::empty(); MAX_JOBS],
            job_count: 0,
            selected: 0,
            scroll: 0,
        }
    }

    /// Apply one decoded row (developer-service slots are sparse; only Ok
    /// rows join the list). Returns true when the row landed.
    pub(crate) fn apply_row(&mut self, job: DevJob) -> bool {
        if self.job_count >= MAX_JOBS {
            return false;
        }
        self.jobs[self.job_count] = job;
        self.job_count += 1;
        true
    }

    pub(crate) fn move_selection(&mut self, step: i32) {
        if self.job_count == 0 {
            return;
        }
        let next = self.selected as i32 + step;
        self.selected = next.clamp(0, self.job_count as i32 - 1) as usize;
    }

    pub(crate) fn ensure_visible(&mut self, visible: usize) {
        let visible = visible.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible {
            self.scroll = self.selected + 1 - visible;
        }
    }

    pub(crate) fn selected_job(&self) -> Option<&DevJob> {
        self.jobs
            .get(self.selected)
            .filter(|_| self.selected < self.job_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serviceos_userspace_runtime as rt;

    const NAME: &[u8] = b"serviceos.img";

    /// Pack bytes into words exactly like rt::pack_bytes (LE, zero pad).
    fn pack(source: &[u8], words: &mut [u64]) -> usize {
        let count = source.len().div_ceil(8);
        for (index, chunk) in source.chunks(8).enumerate() {
            let mut bytes = [0u8; 8];
            bytes[..chunk.len()].copy_from_slice(chunk);
            words[index] = u64::from_le_bytes(bytes);
        }
        count
    }

    fn tail_magic(fields: usize) -> u64 {
        IDE_TAIL_MAGIC | ((fields as u64) << 32)
    }

    fn phase_word(state: u32, route: u32, export: u32) -> u64 {
        state as u64 | ((route as u64) << 8) | ((export as u64) << 16)
    }

    #[test]
    fn ide_tail_magic_validates_self_describing_header() {
        assert_eq!(ide_tail_field_count(tail_magic(3)), Some(3));
        assert_eq!(ide_tail_field_count(tail_magic(5)), Some(5));
        assert_eq!(ide_tail_field_count(tail_magic(0)), Some(0));
        assert_eq!(ide_tail_field_count(0x4944_4530), None);
        assert_eq!(ide_tail_field_count(0), None);
    }

    #[test]
    fn phase_word_splits_state_route_export() {
        let phase = phase_from_word(phase_word(3, ROUTE_KIND_REMOTE_FARM, EXPORT_STATE_PENDING));
        assert_eq!(phase.state, 3);
        assert_eq!(phase.route_kind, ROUTE_KIND_REMOTE_FARM);
        assert_eq!(phase.export_state, EXPORT_STATE_PENDING);
        let plain = phase_from_word(2);
        assert_eq!(plain.state, 2);
        assert_eq!(plain.route_kind, ROUTE_KIND_DIRECT);
        assert_eq!(plain.export_state, EXPORT_STATE_LOCAL);
    }

    #[test]
    fn job_list_reply_roundtrip_with_tail() {
        let mut words = [0u64; 16];
        words[0] = STATUS_OK as u64;
        words[1] = 3;
        words[2] = 1;
        words[3] = 2; // target linux-x64
        words[4] = 2; // running
        words[5] = 1; // flat format
        words[6] = 4096;
        words[7] = tail_magic(3);
        words[8] = phase_word(2, ROUTE_KIND_REMOTE_FARM, EXPORT_STATE_PENDING);
        words[9] = 1; // toolchain slot 1
        words[10] = 1 | (13 << 8); // has artifact, name len 13
        let job = decode_job_list_reply(TAG_JOB_LIST_REPLY, &words).expect("decode");
        assert_eq!(job.job_id, 3);
        assert_eq!(job.workspace_id, 1);
        assert_eq!(job.target, 2);
        assert_eq!(job.state, 2);
        assert_eq!(job.format, 1);
        assert_eq!(job.artifact_size, 4096);
        let phase = job.phase.expect("tail phase");
        assert_eq!(phase.state, 2);
        assert_eq!(phase.route_kind, ROUTE_KIND_REMOTE_FARM);
        assert_eq!(phase.export_state, EXPORT_STATE_PENDING);
        assert_eq!(job.toolchain, Some(1));
        assert!(job.has_artifact);
        assert_eq!(job.name_len, 13);
        assert!(job.exec_mode.is_none());
    }

    #[test]
    fn job_list_reply_without_tail_still_decodes() {
        let mut words = [0u64; 16];
        words[0] = STATUS_OK as u64;
        words[1] = 0;
        words[2] = 0;
        words[3] = 1;
        words[4] = 1;
        words[5] = 2;
        words[6] = 8;
        let job = decode_job_list_reply(TAG_JOB_LIST_REPLY, &words).expect("decode");
        assert_eq!(job.job_id, 0);
        assert_eq!(job.phase, None);
        assert_eq!(job.toolchain, None);
        assert!(!job.has_artifact);
    }

    #[test]
    fn job_list_reply_reads_prefix_of_grown_tail() {
        let mut words = [0u64; 16];
        words[0] = STATUS_OK as u64;
        words[1] = 5;
        words[2] = 2;
        words[3] = 3; // target windows-x64
        words[4] = 3; // succeeded
        words[5] = 1;
        words[6] = 512;
        words[7] = tail_magic(5);
        words[8] = phase_word(3, ROUTE_KIND_DIRECT, EXPORT_STATE_LOCAL);
        words[9] = 0;
        words[10] = 1 | (8 << 8);
        // Additive fields: queue-to-finish duration plus rate/valid mask.
        words[11] = 240;
        words[12] = 100 | (0b1_1111u64 << 32);
        let job = decode_job_list_reply(TAG_JOB_LIST_REPLY, &words).expect("decode");
        let phase = job.phase.expect("prefix phase still decoded");
        assert_eq!(phase.state, 3);
        assert_eq!(phase.route_kind, ROUTE_KIND_DIRECT);
        assert_eq!(job.toolchain, Some(0));
        assert!(job.has_artifact);
        assert_eq!(job.name_len, 8);
        assert!(job.exec_mode.is_none());
    }

    #[test]
    fn job_list_reply_rejects_notfound_short_and_wrong_tag() {
        let mut words = [0u64; 16];
        words[0] = STATUS_NOT_FOUND as u64;
        assert_eq!(decode_job_list_reply(TAG_JOB_LIST_REPLY, &words), None);
        assert_eq!(
            decode_job_list_reply(TAG_JOB_LIST_REPLY, &[STATUS_OK as u64; 4]),
            None
        );
        words[0] = STATUS_OK as u64;
        assert_eq!(decode_job_list_reply(TAG_JOB_INFO_REPLY, &words), None);
    }

    #[test]
    fn job_info_reply_roundtrip_name_and_tail() {
        let mut words = [0u64; 24];
        words[0] = STATUS_OK as u64;
        words[1] = 5;
        words[2] = 2;
        words[3] = 4; // macos target
        words[4] = 3; // succeeded
        words[5] = 2; // elf64
        words[6] = 123456;
        words[7] = NAME.len() as u64;
        let name_words = pack(NAME, &mut words[8..]);
        let tail_base = 8 + name_words;
        words[tail_base] = tail_magic(5);
        words[tail_base + 1] = phase_word(3, ROUTE_KIND_DIRECT, EXPORT_STATE_LOCAL);
        words[tail_base + 2] = 0;
        words[tail_base + 3] = 1 | ((NAME.len() as u64) << 8);
        words[tail_base + 4] =
            EXPORT_STATE_LOCAL as u64 | (FARM_STATUS_NOT_CONFIGURED as u64) << 16;
        words[tail_base + 5] = 0; // direct spawn exec mode
        let job =
            decode_job_info_reply(TAG_JOB_INFO_REPLY, &words[..tail_base + 6]).expect("decode");
        assert_eq!(job.job_id, 5);
        assert_eq!(job.workspace_id, 2);
        assert_eq!(job.state, 3);
        assert_eq!(job.format, 2);
        assert_eq!(job.artifact_size, 123456);
        assert_eq!(job.name_text(), "serviceos.img");
        let phase = job.phase.expect("tail phase");
        assert_eq!(phase.state, 3);
        assert_eq!(phase.route_kind, ROUTE_KIND_DIRECT);
        assert_eq!(phase.export_state, EXPORT_STATE_LOCAL);
        assert_eq!(job.toolchain, Some(0));
        assert!(job.has_artifact);
        assert_eq!(job.farm_status, Some(FARM_STATUS_NOT_CONFIGURED));
        assert_eq!(job.endpoint_len, 0);
        assert_eq!(job.exec_mode, Some(0));
    }

    #[test]
    fn job_info_reply_survives_truncated_tail() {
        let mut words = [0u64; 24];
        words[0] = STATUS_OK as u64;
        words[7] = NAME.len() as u64;
        let name_words = pack(NAME, &mut words[8..]);
        let tail_base = 8 + name_words;
        words[tail_base] = tail_magic(5);
        // Only 2 of the 5 promised fields present.
        words[tail_base + 1] = phase_word(1, 0, 0);
        words[tail_base + 2] = 0;
        let job =
            decode_job_info_reply(TAG_JOB_INFO_REPLY, &words[..tail_base + 3]).expect("decode");
        assert_eq!(job.name_text(), "serviceos.img");
        assert_eq!(job.phase, None);
        assert_eq!(job.exec_mode, None);
    }

    #[test]
    fn ide_snapshot_roundtrip_name_endpoint_and_exec() {
        let endpoint = b"farm@10.0.0.9:7900";
        let mut words = [0u64; 24];
        words[0] = STATUS_OK as u64;
        words[1] = 7;
        words[2] = phase_word(2, ROUTE_KIND_REMOTE_FARM, EXPORT_STATE_PENDING);
        words[3] = 2 | ((3 as u64) << 32); // workspace 2, toolchain 3
        words[4] = 777 | ((1 as u64) << 32) | ((1 as u64) << 40);
        words[5] = FARM_STATUS_REGISTERED as u64 | ((endpoint.len() as u64) << 8);
        words[6] = NAME.len() as u64;
        let mut combined = [0u8; 128];
        combined[..NAME.len()].copy_from_slice(NAME);
        combined[NAME.len()..NAME.len() + endpoint.len()].copy_from_slice(endpoint);
        let total = NAME.len() + endpoint.len();
        let byte_words = pack(&combined[..total], &mut words[7..]);
        let tail_base = 7 + byte_words;
        words[tail_base] = tail_magic(1);
        words[tail_base + 1] = 1 | ((9 as u64) << 8); // routed env 9
        let job =
            decode_ide_snapshot(TAG_IDE_SNAPSHOT_REPLY, &words[..tail_base + 2]).expect("decode");
        assert_eq!(job.job_id, 7);
        let phase = job.phase.expect("phase");
        assert_eq!(phase.state, 2);
        assert_eq!(phase.route_kind, ROUTE_KIND_REMOTE_FARM);
        assert_eq!(phase.export_state, EXPORT_STATE_PENDING);
        assert_eq!(job.workspace_id, 2);
        assert_eq!(job.toolchain, Some(3));
        assert_eq!(job.artifact_size, 777);
        assert_eq!(job.format, 1);
        assert!(job.has_artifact);
        assert_eq!(job.farm_status, Some(FARM_STATUS_REGISTERED));
        assert_eq!(job.endpoint_text(), "farm@10.0.0.9:7900");
        assert_eq!(job.name_text(), "serviceos.img");
        assert_eq!(job.exec_mode, Some(1 | (9 << 8)));
    }

    #[test]
    fn ide_snapshot_rejects_notfound_and_short_header() {
        let mut words = [0u64; 24];
        words[0] = STATUS_NOT_FOUND as u64;
        assert_eq!(decode_ide_snapshot(TAG_IDE_SNAPSHOT_REPLY, &words), None);
        assert_eq!(
            decode_ide_snapshot(TAG_IDE_SNAPSHOT_REPLY, &[0, 1, 2]),
            None
        );
        assert_eq!(decode_ide_snapshot(TAG_JOB_LIST_REPLY, &words), None);
    }

    #[test]
    fn request_builders_match_wire_contract() {
        let list = job_list_request(2, 0x55);
        assert_eq!(list.tag, TAG_JOB_LIST_REQUEST);
        assert_eq!(list.word_count, 1);
        assert_eq!(list.words[0], 2);
        assert_eq!(list.handle_count, 1);
        assert_eq!(list.handles[0], 0x55);
        assert_eq!(list.handle_rights[0], rt::rights::SEND);

        let info = job_info_request(9, 0x66);
        assert_eq!(info.tag, TAG_JOB_INFO_REQUEST);
        assert_eq!(info.words[0], 9);

        let snapshot = ide_snapshot_request(4, 0x77);
        assert_eq!(snapshot.tag, TAG_IDE_SNAPSHOT_REQUEST);
        assert_eq!(snapshot.words[0], 4);
    }

    #[test]
    fn label_strings_track_service_enums() {
        assert_eq!(job_state_name(1), "queued");
        assert_eq!(job_state_name(2), "running");
        assert_eq!(job_state_name(3), "succeeded");
        assert_eq!(job_state_name(4), "failed");
        assert_eq!(job_state_name(5), "unsupported");
        assert_eq!(job_state_name(9), "?");
        assert_eq!(format_name(1), "flat");
        assert_eq!(format_name(2), "elf64");
        assert_eq!(format_name(3), "pe32+");
        assert_eq!(format_name(4), "macho64");
        assert_eq!(format_name(0), "?");
        assert_eq!(target_name(1), "native-x64");
        assert_eq!(target_name(2), "linux-x64");
        assert_eq!(target_name(3), "windows-x64");
        assert_eq!(target_name(4), "macos-x64");
        assert_eq!(route_kind_name(ROUTE_KIND_DIRECT), "direct");
        assert_eq!(route_kind_name(ROUTE_KIND_RUNTIME_ENV), "routed-env");
        assert_eq!(route_kind_name(ROUTE_KIND_REMOTE_FARM), "remote-farm");
        assert_eq!(route_kind_name(7), "?");
        assert_eq!(export_state_name(EXPORT_STATE_LOCAL), "local");
        assert_eq!(export_state_name(EXPORT_STATE_PENDING), "farm-pending");
        assert_eq!(farm_status_name(FARM_STATUS_REGISTERED), "registered");
        assert_eq!(farm_status_name(FARM_STATUS_NOT_CONFIGURED), "none");
        assert_eq!(farm_status_name(FARM_STATUS_UNREACHABLE), "unreachable");
        let mut mode = rt::FixedLogBuffer::<32>::new();
        write_exec_mode(0, &mut mode);
        assert_eq!(core::str::from_utf8(mode.as_bytes()).unwrap(), "direct");
        let mut mode = rt::FixedLogBuffer::<32>::new();
        write_exec_mode(1 | (3 << 8), &mut mode);
        assert_eq!(
            core::str::from_utf8(mode.as_bytes()).unwrap(),
            "routed-env:3"
        );
        let mut mode = rt::FixedLogBuffer::<32>::new();
        write_exec_mode(2 | (5 << 8) | (1 << 24), &mut mode);
        assert_eq!(
            core::str::from_utf8(mode.as_bytes()).unwrap(),
            "routed-fallback:5"
        );
    }

    #[test]
    fn panel_state_selection_clamps_and_scrolls() {
        let mut state = DevState::new();
        state.move_selection(-1);
        state.move_selection(1);
        assert_eq!(state.selected, 0);
        for job_id in 0..3u32 {
            let mut job = DevJob::empty();
            job.job_id = job_id;
            assert!(state.apply_row(job));
        }
        assert_eq!(state.job_count, 3);
        state.move_selection(1);
        state.move_selection(1);
        state.move_selection(1);
        assert_eq!(state.selected, 2);
        state.move_selection(-5);
        assert_eq!(state.selected, 0);
        state.move_selection(9);
        assert_eq!(state.selected, 2);
        state.ensure_visible(2);
        assert_eq!(state.scroll, 1);
        assert_eq!(state.selected_job().map(|job| job.job_id), Some(2));
    }

    #[test]
    fn panel_state_apply_row_respects_slot_capacity() {
        let mut state = DevState::new();
        for job_id in 0..(MAX_JOBS as u32 + 2) {
            let mut job = DevJob::empty();
            job.job_id = job_id;
            state.apply_row(job);
        }
        assert_eq!(state.job_count, MAX_JOBS);
    }

    #[test]
    fn panel_starts_unavailable_and_dormant() {
        let state = DevState::new();
        assert!(!state.open);
        assert!(!state.available);
        assert_eq!(state.job_count, 0);
    }

    #[test]
    fn panel_key_routing_moves_selection_and_reports_close() {
        use crate::state::{KEY_DOWN, KEY_ESC, KEY_UP};
        let mut state = DevState::new();
        let mut job = DevJob::empty();
        job.job_id = 1;
        state.apply_row(job);
        assert_eq!(handle_key(&mut state, KEY_DOWN), DevKey::None);
        assert_eq!(state.selected, 1.min(state.job_count - 1));
        assert_eq!(handle_key(&mut state, KEY_UP), DevKey::None);
        assert_eq!(state.selected, 0);
        assert_eq!(handle_key(&mut state, KEY_ESC), DevKey::Back);
        assert_eq!(handle_key(&mut state, 42), DevKey::None);
    }

    #[test]
    fn row_meta_names_phase_and_workspace() {
        let mut job = DevJob::empty();
        job.job_id = 4;
        job.workspace_id = 1;
        job.state = 2;
        job.phase = Some(JobPhase {
            state: 2,
            route_kind: ROUTE_KIND_REMOTE_FARM,
            export_state: EXPORT_STATE_PENDING,
        });
        let line = row_meta(&job);
        assert_eq!(line.as_str(), "ws1 running remote-farm");
        let bare = row_meta(&DevJob::empty());
        assert_eq!(bare.as_str(), "ws0 ? ?");
    }

    #[test]
    fn detail_lines_report_artifact_and_farm_honestly() {
        let mut job = DevJob::empty();
        job.job_id = 6;
        job.workspace_id = 2;
        job.target = 2;
        job.state = 3;
        job.format = 2;
        job.artifact_size = 51200;
        job.name_len = 13;
        job.name[..13].copy_from_slice(b"serviceos.img");
        job.toolchain = Some(1);
        job.farm_status = Some(FARM_STATUS_REGISTERED);
        job.endpoint_len = 18;
        job.endpoint[..18].copy_from_slice(b"farm@10.0.0.9:7900");
        job.exec_mode = Some(0);
        job.phase = Some(JobPhase {
            state: 3,
            route_kind: ROUTE_KIND_DIRECT,
            export_state: EXPORT_STATE_LOCAL,
        });
        let lines = detail_lines(&job);
        assert!(lines.len() >= 5);
        assert_eq!(lines[0].as_str(), "artifact: serviceos.img");
        assert_eq!(lines[1].as_str(), "size: 51200 B  format: elf64");
        assert_eq!(lines[2].as_str(), "workspace: 2  target: linux-x64");
        assert_eq!(lines[3].as_str(), "toolchain slot: 1  exec: direct");
        assert_eq!(
            lines[4].as_str(),
            "farm: registered  export: local  endpoint: farm@10.0.0.9:7900"
        );
        let mut unresolved = DevJob::empty();
        unresolved.toolchain = None;
        let lines = detail_lines(&unresolved);
        assert_eq!(lines[3].as_str(), "toolchain slot: -  exec: ?");
    }
}

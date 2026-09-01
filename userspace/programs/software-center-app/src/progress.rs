//! Live operation progress for GUI-triggered install/update: opens a
//! package-source log subscription before the mutation request is sent and
//! pumps package-service's per-phase progress records while the main loop
//! yields, ending when the operation reply lands. Mirrors the shell's
//! `pkg --verbose` stream (decode/filter/stop state machine) with the app's
//! bounded status line as the render surface. Degrades silently to the
//! plain blocking call (final reply only) when the log channel or a
//! subscription slot is unavailable, and degrades to reply-only watching
//! when the stream goes quiet past the idle bound. Remove is not streamed:
//! package-service emits no progress records for it (verified against
//! operations.rs handle_remove_request), so remove keeps the plain path.

use rt::{LogSeverity, LogTag, PackageTag, RawMessage, ServiceId};
use serviceos_userspace_runtime as rt;

use crate::state::{AppState, MAX_STATUS_BYTES, OperationState, service_label};

/// Operation words carried in progress-record arg0; mirrors the service-side
/// journal action codes (ops_model.rs). Remove (3) never emits progress.
pub(crate) const OP_INSTALL: u32 = 1;
pub(crate) const OP_UPDATE: u32 = 2;

/// Progress event word (LogEvent::PackageOperationProgress, ABI value 72).
fn progress_event_word() -> u64 {
    rt::LogEvent::PackageOperationProgress as u64
}

/// Five phase-entry records per operation; the pump repaints only when the
/// (phase, step, total) triple actually changes, so a whole operation costs
/// at most a handful of repaints.
pub(crate) const MAX_RECORDS_PER_PUMP: usize = 64;
pub(crate) const MAX_RECORDS_TOTAL: usize = 512;

/// No reply and no progress for this long: stop watching the stream and
/// degrade to final-reply-only. Generous on purpose (a remote materialize on
/// TCG can idle well past shorter bounds) while still bounding a lost-reply
/// path that would otherwise spin forever.
pub(crate) const PROGRESS_IDLE_TIMEOUT_TICKS: u64 = 120 * 100;

/// One iteration of the stop state machine: a received reply always wins,
/// an idle stretch past the bound degrades the stream, anything else keeps
/// polling. Pure so the stop matrix stays host-testable.
pub(crate) enum StreamStop {
    Reply,
    Timeout,
    Continue,
}

pub(crate) fn stream_stop(reply_received: bool, idle_ticks: u64) -> StreamStop {
    if reply_received {
        StreamStop::Reply
    } else if idle_ticks >= PROGRESS_IDLE_TIMEOUT_TICKS {
        StreamStop::Timeout
    } else {
        StreamStop::Continue
    }
}

/// Decodes one subscription StreamRecord into (op, phase, step, total) when
/// it is a package-operation-progress record; None for anything else
/// (other events on the package source pass through silently).
pub(crate) fn decode_progress_record(message: &RawMessage) -> Option<(u32, u32, u32, u32)> {
    if message.tag != LogTag::StreamRecord as u32 || message.word_count < 9 {
        return None;
    }
    if message.words[5] != progress_event_word() {
        return None;
    }
    let op = message.words[6] as u32;
    if !matches!(op, OP_INSTALL | OP_UPDATE) {
        return None;
    }
    let word = message.words[7];
    let phase = (word & 0xff) as u32;
    let step = ((word >> 8) & 0xffff) as u32;
    let total = ((word >> 24) & 0xffff) as u32;
    Some((op, phase, step, total))
}

/// True when a progress record names the operation being streamed, so a
/// concurrent unrelated mutation never leaks its lines into this render.
pub(crate) fn progress_matches(decoded: (u32, u32, u32, u32), op: u32) -> bool {
    decoded.0 == op
}

pub(crate) fn op_name(op: u32) -> &'static str {
    match op {
        OP_INSTALL => "install",
        OP_UPDATE => "update",
        _ => "none",
    }
}

/// Phase names mirror the service-side pack_progress phases.
pub(crate) fn phase_name(phase: u32) -> &'static str {
    match phase {
        0 => "resolve",
        1 => "materialize",
        2 => "verify",
        3 => "activate",
        4 => "persist",
        _ => "unknown",
    }
}

/// Whole-operation percent across five equally weighted phases; mirrors the
/// service-side progress_percent so the GUI and `pkg --verbose` agree.
pub(crate) fn progress_percent(phase: u32, step: u32, total: u32) -> u32 {
    const PROGRESS_PHASES: u32 = 5;
    if phase >= PROGRESS_PHASES || total == 0 {
        return 0;
    }
    let per_phase = 100 / PROGRESS_PHASES;
    phase * per_phase + step.min(total) * per_phase / total
}

/// Reply status decode; mirrors the runtime's package_status_from_word so a
/// streamed reply produces byte-identical final statuses to the blocking
/// wrappers that use it.
pub(crate) fn status_from_word(word: u64) -> rt::PackageStatus {
    match word as u32 {
        x if x == rt::PackageStatus::Ok as u32 => rt::PackageStatus::Ok,
        x if x == rt::PackageStatus::NotFound as u32 => rt::PackageStatus::NotFound,
        x if x == rt::PackageStatus::AlreadyInstalled as u32 => rt::PackageStatus::AlreadyInstalled,
        x if x == rt::PackageStatus::NotInstalled as u32 => rt::PackageStatus::NotInstalled,
        x if x == rt::PackageStatus::Busy as u32 => rt::PackageStatus::Busy,
        x if x == rt::PackageStatus::Denied as u32 => rt::PackageStatus::Denied,
        x if x == rt::PackageStatus::IntegrityFailed as u32 => rt::PackageStatus::IntegrityFailed,
        x if x == rt::PackageStatus::End as u32 => rt::PackageStatus::End,
        x if x == rt::PackageStatus::NoChange as u32 => rt::PackageStatus::NoChange,
        x if x == rt::PackageStatus::NoRollback as u32 => rt::PackageStatus::NoRollback,
        x if x == rt::PackageStatus::Unsupported as u32 => rt::PackageStatus::Unsupported,
        x if x == rt::PackageStatus::Offline as u32 => rt::PackageStatus::Offline,
        x if x == rt::PackageStatus::Interrupted as u32 => rt::PackageStatus::Interrupted,
        x if x == rt::PackageStatus::VerificationFailed as u32 => {
            rt::PackageStatus::VerificationFailed
        }
        _ => rt::PackageStatus::Busy,
    }
}

/// Reply status to error mapping; mirrors the runtime's package_status_error
/// (pub(crate) there), so error_label renders the same word it would have
/// rendered for the blocking call.
pub(crate) fn status_to_error(status: rt::PackageStatus) -> rt::Error {
    match status {
        rt::PackageStatus::NotFound => rt::Error::NotFound,
        rt::PackageStatus::AlreadyInstalled
        | rt::PackageStatus::Busy
        | rt::PackageStatus::NoChange
        | rt::PackageStatus::Offline
        | rt::PackageStatus::Interrupted => rt::Error::Busy,
        rt::PackageStatus::NotInstalled
        | rt::PackageStatus::NoRollback
        | rt::PackageStatus::End
        | rt::PackageStatus::Unsupported
        | rt::PackageStatus::InvalidParameter
        | rt::PackageStatus::AlreadyExists => rt::Error::InvalidArgument,
        rt::PackageStatus::Denied => rt::Error::PermissionDenied,
        rt::PackageStatus::IntegrityFailed | rt::PackageStatus::VerificationFailed => {
            rt::Error::InvalidCall
        }
        rt::PackageStatus::Ok => rt::Error::InvalidArgument,
        // Foreign-session unblock (concurrent NoKeyPair ABI addition):
        // minimal arm so the workspace keeps compiling; the owning session
        // should refine the mapping.
        rt::PackageStatus::NoKeyPair => rt::Error::InvalidArgument,
    }
}

/// Builds the mutation request exactly as the runtime's package_mutation
/// does (service_id, version_len=0) with a reply channel attached.
pub(crate) fn build_mutation_request(
    reply_second: rt::Handle,
    request_tag: PackageTag,
    service_id: ServiceId,
) -> RawMessage {
    let mut request = RawMessage::empty(request_tag as u32);
    request.word_count = 2;
    request.words[0] = service_id as u32 as u64;
    request.words[1] = 0;
    request.handle_count = 1;
    request.handles[0] = reply_second;
    request.handle_rights[0] = rt::rights::SEND;
    request
}

/// Reply tag matching the mutation request tag.
pub(crate) fn reply_tag_for(request_tag: PackageTag) -> u32 {
    match request_tag {
        PackageTag::UpdateRequest => PackageTag::UpdateReply as u32,
        _ => PackageTag::InstallReply as u32,
    }
}

/// Decodes the operation reply: status in words[0]; reply tag and minimum
/// shape verified by the caller against the expected PackageTag.
pub(crate) fn reply_status(reply: &RawMessage) -> Option<rt::PackageStatus> {
    if reply.word_count < 1 {
        return None;
    }
    Some(status_from_word(reply.words[0]))
}

/// Bounded progress status line: "<op> <label>: <phase> s/t steps (N%)"
/// within MAX_STATUS_BYTES.
pub(crate) fn format_progress_line(
    buffer: &mut [u8],
    op: u32,
    service_id: ServiceId,
    phase: u32,
    step: u32,
    total: u32,
) -> usize {
    let text = format_args!(
        "{} {}: {} {}/{} steps ({}%)",
        op_name(op),
        service_label(service_id),
        phase_name(phase),
        step,
        total,
        progress_percent(phase, step, total),
    );
    write_bounded(buffer, text)
}

/// Degrade note shown once when the stream goes quiet past the bound or the
/// subscription is lost: the reply is still awaited and will render.
pub(crate) fn format_degraded_line(buffer: &mut [u8], op: u32, service_id: ServiceId) -> usize {
    let text = format_args!(
        "{} {}: progress stream idle, awaiting reply",
        op_name(op),
        service_label(service_id),
    );
    write_bounded(buffer, text)
}

fn write_bounded(buffer: &mut [u8], args: core::fmt::Arguments<'_>) -> usize {
    let mut framed = rt::FixedLogBuffer::<MAX_STATUS_BYTES>::new();
    let _ = core::fmt::Write::write_fmt(&mut framed, args);
    let bytes = framed.as_bytes();
    let take = bytes.len().min(buffer.len());
    buffer[..take].copy_from_slice(&bytes[..take]);
    take
}

/// Opens a package-source log subscription on the app's log channel (the
/// positional startup grant). None = degrade (log channel absent or the
/// subscribe handshake failed).
pub(crate) fn open_subscription(log_handle: rt::Handle) -> Option<rt::Handle> {
    if log_handle == rt::INVALID_HANDLE {
        return None;
    }
    rt::log_subscribe(
        log_handle,
        LogSeverity::Trace,
        Some(ServiceId::Package),
        None,
    )
    .ok()
}

pub(crate) enum PumpEnd {
    /// Keep waiting; the operation stays active.
    Continue,
    /// Progress advanced to this triple for the running op: repaint.
    Progress(u32, u32, u32),
    /// Stream degraded (idle bound or subscription loss): repaint once for
    /// the honest note, then reply-only watching.
    Degraded,
    /// The operation reply landed: finish and render the final status.
    ReplyReceived,
}

/// Pure pump decision for one main-loop pass: what the subscription drain
/// produced and whether the stop machine says to continue. Host-testable.
pub(crate) fn pump_decision(
    decoded_for_op: Option<(u32, u32, u32)>,
    reply_received: bool,
    degraded_by_drain: bool,
    idle_ticks: u64,
) -> PumpEnd {
    if reply_received {
        return PumpEnd::ReplyReceived;
    }
    match stream_stop(reply_received, idle_ticks) {
        StreamStop::Timeout => PumpEnd::Degraded,
        StreamStop::Reply | StreamStop::Continue => {
            if let Some((phase, step, total)) = decoded_for_op {
                PumpEnd::Progress(phase, step, total)
            } else if degraded_by_drain {
                PumpEnd::Degraded
            } else {
                PumpEnd::Continue
            }
        }
    }
}

/// Drains the subscription queue, updating the operation's activity and
/// progress tracking. Returns the newest decoded triple for the running op
/// (None = nothing new for this op) and whether the stream degraded.
pub(crate) fn drain_subscription(
    operation: &mut OperationState,
) -> (Option<(u32, u32, u32)>, bool) {
    let mut newest: Option<(u32, u32, u32)> = None;
    let mut degraded = false;
    if operation.degraded || operation.subscription == rt::INVALID_HANDLE {
        return (None, false);
    }
    for _ in 0..MAX_RECORDS_PER_PUMP {
        if operation.records_seen >= MAX_RECORDS_TOTAL {
            degraded = true;
            break;
        }
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(operation.subscription, &mut message) {
            Ok(()) => {
                operation.records_seen += 1;
                if let Some((record_op, phase, step, total)) = decode_progress_record(&message) {
                    if progress_matches((record_op, phase, step, total), operation.op)
                        && operation.rendered != (phase, step, total)
                    {
                        // Only matching progress records count as activity,
                        // mirroring the shell stream: unrelated package-source
                        // traffic must not extend the idle bound.
                        operation.last_activity_tick = rt::monotonic_now().unwrap_or(0);
                        operation.rendered = (phase, step, total);
                        newest = Some((phase, step, total));
                    }
                }
            }
            Err(rt::Error::QueueEmpty) => break,
            Err(_) => {
                degraded = true;
                break;
            }
        }
    }
    if degraded {
        let _ = rt::handle_close(operation.subscription);
        operation.subscription = rt::INVALID_HANDLE;
        operation.degraded = true;
    }
    (newest, degraded)
}

/// Nonblocking reply poll for the operation's reply channel.
pub(crate) fn receive_reply(operation: &OperationState) -> rt::Result<Option<RawMessage>> {
    let mut reply = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(operation.reply_pair, &mut reply) {
        Ok(()) => Ok(Some(reply)),
        Err(rt::Error::QueueEmpty) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn idle_ticks(operation: &OperationState) -> u64 {
    let now = rt::monotonic_now().unwrap_or(operation.last_activity_tick);
    now.saturating_sub(operation.last_activity_tick)
}

/// Closes every handle the operation still owns.
pub(crate) fn close_operation(operation: &mut OperationState) {
    if operation.subscription != rt::INVALID_HANDLE {
        let _ = rt::handle_close(operation.subscription);
        operation.subscription = rt::INVALID_HANDLE;
    }
    let _ = rt::handle_close(operation.reply_pair);
    operation.reply_pair = rt::INVALID_HANDLE;
}

/// Main-loop pump for an active operation: drains progress, repaints on
/// change, and on the reply lands (or the stream degrades past the idle
/// bound) finishes via `finish`, which owns the final status rendering.
/// Returns true when the frame changed and a repaint is due.
pub(crate) fn pump_operation(
    state: &mut AppState,
    finish: impl FnOnce(&mut AppState, rt::Result<()>),
) -> bool {
    let Some(mut operation) = state.operation.take() else {
        return false;
    };
    let reply = match receive_reply(&operation) {
        Ok(reply) => reply,
        Err(_) => {
            close_operation(&mut operation);
            finish(state, Err(rt::Error::InvalidArgument));
            return true;
        }
    };
    let (progress, degraded_by_drain) = drain_subscription(&mut operation);
    let reply_received = reply.is_some();
    let decision = pump_decision(
        progress,
        reply_received,
        degraded_by_drain,
        idle_ticks(&operation),
    );
    let mut changed = false;
    match decision {
        PumpEnd::ReplyReceived => {
            let result = match reply {
                Some(message) if message.tag == operation.reply_tag => {
                    match reply_status(&message) {
                        Some(rt::PackageStatus::Ok) => Ok(()),
                        Some(status) => Err(status_to_error(status)),
                        None => Err(rt::Error::InvalidArgument),
                    }
                }
                _ => Err(rt::Error::InvalidArgument),
            };
            close_operation(&mut operation);
            finish(state, result);
            return true;
        }
        PumpEnd::Progress(phase, step, total) => {
            let mut line = [0u8; crate::state::MAX_STATUS_BYTES];
            let len = format_progress_line(
                &mut line,
                operation.op,
                operation.service_id,
                phase,
                step,
                total,
            );
            state.status[..len].copy_from_slice(&line[..len]);
            state.status_len = len;
            changed = true;
        }
        PumpEnd::Degraded => {
            if !operation.degraded {
                let _ = rt::handle_close(operation.subscription);
                operation.subscription = rt::INVALID_HANDLE;
            }
            operation.degraded = true;
            if operation.note_shown {
                state.operation = Some(operation);
                return changed;
            }
            let mut line = [0u8; crate::state::MAX_STATUS_BYTES];
            let len = format_degraded_line(&mut line, operation.op, operation.service_id);
            state.status[..len].copy_from_slice(&line[..len]);
            state.status_len = len;
            operation.note_shown = true;
            changed = true;
        }
        PumpEnd::Continue => {}
    }
    state.operation = Some(operation);
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream_record(op: u64, phase: u32, step: u32, total: u32) -> RawMessage {
        let mut message = RawMessage::empty(LogTag::StreamRecord as u32);
        message.word_count = 9;
        message.words[5] = progress_event_word();
        message.words[6] = op;
        message.words[7] =
            (phase as u64 & 0xff) | ((step as u64 & 0xffff) << 8) | ((total as u64 & 0xffff) << 24);
        message
    }

    #[test]
    fn progress_word_packs_and_decodes() {
        let (op, phase, step, total) =
            decode_progress_record(&stream_record(OP_INSTALL as u64, 1, 2, 5)).unwrap();
        assert_eq!((op, phase, step, total), (OP_INSTALL, 1, 2, 5));
    }

    #[test]
    fn unknown_events_ops_and_shapes_are_skipped() {
        // Wrong event word: not a progress record.
        let mut other = stream_record(OP_UPDATE as u64, 0, 1, 5);
        other.words[5] = 17; // PackageInstalled
        assert!(decode_progress_record(&other).is_none());
        // Remove and unknown ops never stream.
        assert!(decode_progress_record(&stream_record(3, 0, 1, 5)).is_none());
        assert!(decode_progress_record(&stream_record(9, 0, 1, 5)).is_none());
        // Wrong tag: not a stream record.
        let mut wrong_tag = stream_record(OP_UPDATE as u64, 0, 1, 5);
        wrong_tag.tag = 0;
        assert!(decode_progress_record(&wrong_tag).is_none());
        // Short record.
        let mut short = stream_record(OP_INSTALL as u64, 0, 1, 5);
        short.word_count = 4;
        assert!(decode_progress_record(&short).is_none());
    }

    #[test]
    fn progress_matches_scopes_records_to_the_running_op() {
        assert!(progress_matches((OP_INSTALL, 0, 1, 5), OP_INSTALL));
        assert!(!progress_matches((OP_UPDATE, 0, 1, 5), OP_INSTALL));
        assert!(!progress_matches((OP_INSTALL, 0, 1, 5), OP_UPDATE));
    }

    #[test]
    fn operation_labels_cover_streamed_ops_only() {
        assert_eq!(op_name(OP_INSTALL), "install");
        assert_eq!(op_name(OP_UPDATE), "update");
        assert_eq!(op_name(0), "none");
        assert_eq!(op_name(3), "none");
    }

    #[test]
    fn phase_entry_matrix_is_monotonic_like_the_service() {
        // The five phase-entry records an install emits, in order.
        let entries: [(u32, u32, u32); 5] = [(0, 0, 5), (1, 1, 5), (2, 2, 5), (3, 3, 5), (4, 4, 5)];
        let mut previous = 0u32;
        for (phase, step, total) in entries {
            let (decoded_op, dp, ds, dt) =
                decode_progress_record(&stream_record(OP_UPDATE as u64, phase, step, total))
                    .unwrap();
            assert_eq!((decoded_op, dp, ds, dt), (OP_UPDATE, phase, step, total));
            let percent = progress_percent(dp, ds, dt);
            assert!(percent >= previous, "percent must be monotonic");
            assert!(percent <= 100);
            previous = percent;
        }
        assert_eq!(previous, 96);
    }

    #[test]
    fn percent_degenerates_like_the_service_math() {
        assert_eq!(progress_percent(0, 5, 0), 0);
        assert_eq!(progress_percent(9, 1, 5), 0);
        assert_eq!(progress_percent(2, 3, 5), 52);
        // Step clamps to total: full phase weight.
        assert_eq!(progress_percent(2, 99, 5), 60);
        assert_eq!(progress_percent(4, 5, 5), 100);
    }

    #[test]
    fn stream_stop_matrix_covers_reply_idle_and_continue() {
        // Reply wins even when idle time has already grown large.
        assert!(matches!(
            stream_stop(true, PROGRESS_IDLE_TIMEOUT_TICKS),
            StreamStop::Reply
        ));
        // Degrade: no reply past the bound stops the stream.
        assert!(matches!(
            stream_stop(false, PROGRESS_IDLE_TIMEOUT_TICKS),
            StreamStop::Timeout
        ));
        assert!(matches!(
            stream_stop(false, PROGRESS_IDLE_TIMEOUT_TICKS + 1),
            StreamStop::Timeout
        ));
        // Still inside the bound: keep polling.
        assert!(matches!(
            stream_stop(false, PROGRESS_IDLE_TIMEOUT_TICKS - 1),
            StreamStop::Continue
        ));
        assert!(matches!(stream_stop(false, 0), StreamStop::Continue));
    }

    #[test]
    fn pump_decision_reply_always_wins() {
        // A landed reply wins over new progress, degrade, and idle state.
        assert!(matches!(
            pump_decision(Some((1, 2, 5)), true, true, PROGRESS_IDLE_TIMEOUT_TICKS),
            PumpEnd::ReplyReceived
        ));
        // Degrade fires even with fresh progress when the idle bound passed.
        assert!(matches!(
            pump_decision(Some((1, 2, 5)), false, false, PROGRESS_IDLE_TIMEOUT_TICKS),
            PumpEnd::Degraded
        ));
        // Fresh progress inside the bound: repaint.
        assert!(matches!(
            pump_decision(Some((1, 2, 5)), false, false, 0),
            PumpEnd::Progress(1, 2, 5)
        ));
        // Subscription loss inside the bound: degrade once.
        assert!(matches!(
            pump_decision(None, false, true, 0),
            PumpEnd::Degraded
        ));
        // Nothing new: keep waiting.
        assert!(matches!(
            pump_decision(None, false, false, 0),
            PumpEnd::Continue
        ));
    }

    #[test]
    fn reply_status_gates_shape_and_maps_errors_like_the_runtime() {
        // Empty reply is invalid.
        let reply = RawMessage::empty(0);
        assert!(reply_status(&reply).is_none());
        // A one-word reply decodes.
        let mut ok = RawMessage::empty(0);
        ok.word_count = 1;
        ok.words[0] = 0;
        assert_eq!(reply_status(&ok), Some(rt::PackageStatus::Ok));
        // Error mapping mirrors the runtime's package_status_error.
        assert_eq!(
            status_to_error(rt::PackageStatus::NotFound),
            rt::Error::NotFound
        );
        assert_eq!(status_to_error(rt::PackageStatus::Busy), rt::Error::Busy);
        assert_eq!(
            status_to_error(rt::PackageStatus::Denied),
            rt::Error::PermissionDenied
        );
        assert_eq!(
            status_to_error(rt::PackageStatus::IntegrityFailed),
            rt::Error::InvalidCall
        );
        assert_eq!(
            status_to_error(rt::PackageStatus::NotInstalled),
            rt::Error::InvalidArgument
        );
    }

    #[test]
    fn mutation_request_mirrors_the_runtime_shape() {
        let request = build_mutation_request(7, PackageTag::InstallRequest, ServiceId::Shell);
        assert_eq!(request.tag, PackageTag::InstallRequest as u32);
        assert_eq!(request.word_count, 2);
        assert_eq!(request.words[0], ServiceId::Shell as u32 as u64);
        assert_eq!(request.words[1], 0);
        assert_eq!(request.handle_count, 1);
        assert_eq!(request.handles[0], 7);
        assert_eq!(request.handle_rights[0], rt::rights::SEND);
    }

    #[test]
    fn progress_lines_stay_within_the_status_budget() {
        let mut line = [0u8; crate::state::MAX_STATUS_BYTES];
        let len = format_progress_line(&mut line, OP_UPDATE, ServiceId::Shell, 1, 3, 5);
        assert!(len > 0 && len <= crate::state::MAX_STATUS_BYTES);
        let text = core::str::from_utf8(&line[..len]).unwrap();
        assert!(text.starts_with("update "));
        assert!(text.contains("materialize 3/5 steps (32%)"));
        // Degrade note fits too.
        let len = format_degraded_line(&mut line, OP_INSTALL, ServiceId::Shell);
        assert!(len > 0 && len <= crate::state::MAX_STATUS_BYTES);
        let text = core::str::from_utf8(&line[..len]).unwrap();
        assert!(text.starts_with("install "));
        assert!(text.ends_with("awaiting reply"));
    }
}

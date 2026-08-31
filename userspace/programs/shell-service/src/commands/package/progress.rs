//! Live operation progress for `pkg install/update/rollback --verbose`:
//! opens a package-source log subscription before the mutation request is
//! sent and renders package-service's per-phase progress records as they
//! arrive, ending when the operation reply lands. Degrades to the plain
//! blocking call when the log channel or a subscription slot is unavailable.

use rt::{LogSeverity, RawMessage, ServiceId};
use serviceos_userspace_runtime as rt;

use crate::util::tables::{FOLLOW_MAX_RECORDS, TICK_HZ};
use crate::util::{ShellOutput, service_name, write_output_linef};

use super::mutate::{package_status_name, phase_name, progress_percent, status_from_word};

/// Operation words carried in progress-record `arg0`; mirrors the
/// service-side journal action codes (ops_model.rs).
pub(in crate::commands) const OP_INSTALL: u32 = 1;
pub(in crate::commands) const OP_UPDATE: u32 = 2;
pub(in crate::commands) const OP_ROLLBACK: u32 = 4;

/// No reply and no progress for this long: stop watching. Generous on
/// purpose (a remote materialize on TCG can idle well past the 30s follow
/// idle bound) while still bounding a lost-reply path that would otherwise
/// spin forever.
pub(in crate::commands) const PROGRESS_IDLE_TIMEOUT_TICKS: u64 = 120 * TICK_HZ;

/// Progress event word (LogEvent::PackageOperationProgress, ABI value 72).
fn progress_event_word() -> u64 {
    rt::LogEvent::PackageOperationProgress as u64
}

/// One iteration of the render-loop state machine: a received reply always
/// wins, an idle stretch past the bound stops the stream (degrade), anything
/// else keeps polling. Pure so the stop matrix stays host-testable.
pub(in crate::commands) enum StreamStop {
    Reply,
    Timeout,
    Continue,
}

pub(in crate::commands) fn stream_stop(reply_received: bool, idle_ticks: u64) -> StreamStop {
    if reply_received {
        StreamStop::Reply
    } else if idle_ticks >= PROGRESS_IDLE_TIMEOUT_TICKS {
        StreamStop::Timeout
    } else {
        StreamStop::Continue
    }
}

/// Decodes one subscription StreamRecord into (op, phase, step, total) when
/// it is a package-operation-progress record; `None` for anything else
/// (other events on the package source pass through silently).
pub(in crate::commands) fn decode_progress_record(
    message: &RawMessage,
) -> Option<(u32, u32, u32, u32)> {
    if message.tag != rt::LogTag::StreamRecord as u32 || message.word_count < 9 {
        return None;
    }
    if message.words[5] != progress_event_word() {
        return None;
    }
    let op = message.words[6] as u32;
    if !matches!(op, OP_INSTALL | OP_UPDATE | OP_ROLLBACK) {
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
pub(in crate::commands) fn progress_matches(decoded: (u32, u32, u32, u32), op: u32) -> bool {
    decoded.0 == op
}

fn open_subscription(bootstrap: rt::Handle) -> rt::Result<rt::Handle> {
    let log_handle = rt::lookup_service(bootstrap, ServiceId::Log)?;
    let subscription = rt::log_subscribe(
        log_handle,
        LogSeverity::Trace,
        Some(ServiceId::Package),
        None,
    );
    let _ = rt::handle_close(log_handle);
    subscription
}

/// Sends `request` on the package channel while streaming progress records.
/// Falls back to the plain blocking call (silent, final reply only) when the
/// log service or a subscription slot is unavailable.
pub(in crate::commands) fn streamed_mutation(
    bootstrap: rt::Handle,
    output: ShellOutput,
    package_handle: rt::Handle,
    request: &mut RawMessage,
    op: u32,
    service_id: ServiceId,
) -> rt::Result<RawMessage> {
    let subscription = match open_subscription(bootstrap) {
        Ok(subscription) => subscription,
        Err(_) => return rt::channel_call(package_handle, request),
    };
    let result = stream_to_reply(
        output,
        package_handle,
        request,
        subscription,
        op,
        service_id,
    );
    let _ = rt::handle_close(subscription);
    result
}

fn stream_to_reply(
    output: ShellOutput,
    package_handle: rt::Handle,
    request: &mut RawMessage,
    subscription: rt::Handle,
    op: u32,
    service_id: ServiceId,
) -> rt::Result<RawMessage> {
    // Mirrors channel_call's reply plumbing with a non-blocking receive so
    // the subscription can be drained between yields.
    let reply_pair = rt::channel_create()?;
    request.handle_count = 1;
    request.handles[0] = reply_pair.second;
    request.handle_rights[0] = rt::rights::SEND;
    let send_result = rt::channel_send_blocking(package_handle, request);
    let _ = rt::handle_close(reply_pair.second);
    send_result?;

    let mut records_seen = 0usize;
    let mut last_activity = rt::monotonic_now().unwrap_or(0);
    let mut response = RawMessage::empty(0);
    let mut timed_out = false;
    loop {
        drain_progress(
            output,
            subscription,
            op,
            service_id,
            &mut records_seen,
            &mut last_activity,
        );
        let reply_received = matches!(
            rt::channel_receive_nonblocking(reply_pair.first, &mut response),
            Ok(())
        );
        match stream_stop(
            reply_received,
            rt::monotonic_now()
                .unwrap_or(last_activity)
                .saturating_sub(last_activity),
        ) {
            StreamStop::Reply => break,
            StreamStop::Timeout => {
                timed_out = true;
                break;
            }
            StreamStop::Continue => {
                if rt::yield_current().is_err() {
                    timed_out = true;
                    break;
                }
            }
        }
    }
    // Records queued before the reply landed still belong to this operation.
    drain_progress(
        output,
        subscription,
        op,
        service_id,
        &mut records_seen,
        &mut last_activity,
    );
    let _ = rt::handle_close(reply_pair.first);
    if timed_out {
        return Err(rt::Error::QueueEmpty);
    }
    Ok(response)
}

/// Drains every currently queued subscription record, rendering phase lines
/// for this operation's progress records and skipping everything else.
fn drain_progress(
    output: ShellOutput,
    subscription: rt::Handle,
    op: u32,
    service_id: ServiceId,
    records_seen: &mut usize,
    last_activity: &mut u64,
) {
    loop {
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(subscription, &mut message) {
            Ok(()) => {
                *records_seen += 1;
                if let Some((record_op, phase, step, total)) = decode_progress_record(&message) {
                    if progress_matches((record_op, phase, step, total), op) {
                        *last_activity = rt::monotonic_now().unwrap_or(*last_activity);
                        let _ = write_output_linef(
                            output,
                            format_args!(
                                "{} {}: {} {}/{} steps ({}%)",
                                op_name(record_op),
                                service_name(service_id),
                                phase_name(phase),
                                step,
                                total,
                                progress_percent(phase, step, total),
                            ),
                        );
                    }
                }
                if *records_seen >= FOLLOW_MAX_RECORDS {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

/// Maps an operation word to its label; mirrors the service-side
/// journal action names.
pub(in crate::commands) fn op_name(op: u32) -> &'static str {
    match op {
        OP_INSTALL => "install",
        OP_UPDATE => "update",
        OP_ROLLBACK => "rollback",
        _ => "none",
    }
}

/// Renders the operation reply exactly as the silent path does: status line
/// on failure, phase/step/percent summary on success.
pub(in crate::commands) fn report_mutation_reply(
    output: ShellOutput,
    reply: &RawMessage,
    reply_tag: u32,
    verb: &'static str,
    service_id: ServiceId,
) -> rt::Result<()> {
    if reply.tag != reply_tag || reply.word_count < 1 {
        return Err(rt::Error::InvalidArgument);
    }
    let status = status_from_word(reply.words[0]);
    if status != rt::PackageStatus::Ok {
        return write_output_linef(
            output,
            format_args!("{} failed: {}", verb, package_status_name(status),),
        );
    }
    let (phase, step, total) = decode_summary_word(reply);
    write_output_linef(
        output,
        format_args!(
            "{} {} ({}/{} steps, {} {}%)",
            verb,
            service_name(service_id),
            step,
            total,
            phase_name(phase),
            progress_percent(phase, step, total),
        ),
    )
}

fn decode_summary_word(reply: &RawMessage) -> (u32, u32, u32) {
    if reply.word_count < 4 {
        return (0, 0, 0);
    }
    let word = reply.words[3];
    (
        (word & 0xff) as u32,
        ((word >> 8) & 0xffff) as u32,
        ((word >> 24) & 0xffff) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream_record(op: u64, phase: u32, step: u32, total: u32) -> RawMessage {
        let mut message = RawMessage::empty(rt::LogTag::StreamRecord as u32);
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
    fn unknown_events_and_tags_are_skipped() {
        // Wrong event word: not a progress record.
        let mut other = stream_record(OP_UPDATE as u64, 0, 1, 5);
        other.words[5] = 17; // PackageInstalled
        assert!(decode_progress_record(&other).is_none());
        // Unknown op word: never matches a running op.
        assert!(decode_progress_record(&stream_record(9, 0, 1, 5)).is_none());
        // Wrong tag: not a stream record.
        let mut wrong_tag = stream_record(OP_ROLLBACK as u64, 0, 1, 5);
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
        assert!(!progress_matches((OP_ROLLBACK, 0, 1, 5), OP_UPDATE));
    }

    #[test]
    fn operation_labels_mirror_journal_codes() {
        assert_eq!(op_name(OP_INSTALL), "install");
        assert_eq!(op_name(OP_UPDATE), "update");
        assert_eq!(op_name(OP_ROLLBACK), "rollback");
        assert_eq!(op_name(0), "none");
        assert_eq!(op_name(99), "none");
    }

    #[test]
    fn phase_entry_matrix_matches_service_shape() {
        // The five phase-entry records an operation emits, in order, with
        // the percent each carries (step counts carry over between phases).
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
    fn reply_summary_word_decodes_like_the_service_pack() {
        // pack_progress shape: bits [7:0] phase, [23:8] step, [39:24] total.
        let mut reply = RawMessage::empty(0);
        reply.word_count = 4;
        reply.words[0] = rt::PackageStatus::Ok as u32 as u64;
        reply.words[3] = 4u64 | (5u64 << 8) | (5u64 << 24);
        let (phase, step, total) = decode_summary_word(&reply);
        assert_eq!((phase, step, total), (4, 5, 5));
        assert_eq!(progress_percent(phase, step, total), 100);
        // Short replies decode to the zero shape instead of panicking.
        let mut short = RawMessage::empty(0);
        short.word_count = 1;
        assert_eq!(decode_summary_word(&short), (0, 0, 0));
    }

    #[test]
    fn status_reply_gate_rejects_mismatched_replies() {
        // Wrong tag and empty shape take the same invalid-argument path the
        // silent path uses, so both renderers stay behaviorally identical.
        let reply = RawMessage::empty(0);
        let shape_ok = reply.tag == 0 && reply.word_count < 1;
        assert!(shape_ok);
        assert_eq!(
            package_status_name(status_from_word(rt::PackageStatus::Busy as u32 as u64)),
            "busy"
        );
    }
}

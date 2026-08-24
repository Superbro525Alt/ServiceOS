use core::fmt::Write;

use rt::{ConsoleTag, LogDomain, LogSeverity, RawMessage, ServiceId, StatusTag};
use serviceos_userspace_runtime as rt;

use crate::util::{
    FixedValueText, MAX_DESKTOP_WINDOWS, ShellOutput, availability_name, desktop_app_name,
    manager_status_name, parse_desktop_app_name, parse_service_name, phase_name, service_name,
    startup_name, stash_pending_line, tables, write_log_record, write_output_linef,
};
use tables::{
    CrashShape, FOLLOW_IDLE_TIMEOUT_TICKS, FOLLOW_MAX_RECORDS, FollowStop, HealthRollup,
    MAX_CRASH_ROWS, ROLLUP_WORDS, detail_kind_name, domain_from_word, event_name_from_word,
    follow_stop, health_name, parse_domain_word, parse_health_rollup, ps_state_cell, service_cell,
    service_id_from_word, severity_cell, severity_from_word,
};

enum FollowFilter {
    Source(ServiceId),
    Domain(LogDomain),
}

pub(crate) fn cmd_logs_follow(
    bootstrap: rt::Handle,
    output: ShellOutput,
    filter_word: &str,
) -> rt::Result<()> {
    let Some(filter) = parse_follow_filter(filter_word) else {
        return write_output_linef(output, format_args!("usage: logs follow <domain|service>"));
    };

    let log_handle = rt::lookup_service(bootstrap, ServiceId::Log)?;
    let subscription = match filter {
        FollowFilter::Source(source) => {
            rt::log_subscribe(log_handle, LogSeverity::Trace, Some(source), None)
        }
        FollowFilter::Domain(domain) => {
            rt::log_subscribe(log_handle, LogSeverity::Trace, None, Some(domain))
        }
    };
    let _ = rt::handle_close(log_handle);
    let subscription = subscription?;

    // Only console-backed sessions give a Ctrl-C interrupt path: console-service
    // turns an 0x03 byte into an immediate zero-length completion of the
    // pending read-line request. Graphical terminal panes have no such path
    // (they ignore the console read-line tag entirely), so those follows end
    // via the idle timeout or record cap instead. The writer function pointer
    // is the only sink-type marker available inside this crate.
    #[allow(unpredictable_function_pointer_comparisons)]
    let interruptible = {
        const CONSOLE_WRITE: fn(rt::Handle, &str) -> rt::Result<()> = rt::console_session_write;
        output.write == CONSOLE_WRITE
    };

    let mut parked_input: Option<rt::Handle> = None;
    if interruptible {
        parked_input = arm_interrupt_read(output.handle).ok();
    }

    let mut records_seen = 0usize;
    let mut last_activity = rt::monotonic_now()?;
    let mut input_event: Option<FollowStop> = None;
    loop {
        let stop = poll_follow_pass(
            output,
            subscription,
            &mut records_seen,
            &mut last_activity,
            &mut input_event,
            parked_input,
        );
        if stop != FollowStop::Continue {
            finish_follow(output, subscription, parked_input, stop)?;
            return Ok(());
        }
        if rt::yield_current().is_err() {
            finish_follow(output, subscription, parked_input, FollowStop::IdleTimeout)?;
            return Ok(());
        }
    }
}

fn finish_follow(
    output: ShellOutput,
    subscription: rt::Handle,
    parked_input: Option<rt::Handle>,
    stop: FollowStop,
) -> rt::Result<()> {
    // Never abandon an armed read-line request: the console session keeps at
    // most one pending reply slot, so it must be consumed before the next
    // prompt is drawn.
    if let Some(parked) = parked_input {
        drain_parked_read(parked);
    }
    let _ = rt::handle_close(subscription);

    match stop {
        FollowStop::Interrupted => write_output_linef(output, format_args!("(follow ended)")),
        FollowStop::OperatorLine => write_output_linef(
            output,
            format_args!("(follow ended; running submitted line)"),
        ),
        FollowStop::IdleTimeout => write_output_linef(
            output,
            format_args!("(follow idle timeout after {FOLLOW_IDLE_TIMEOUT_TICKS} ticks)"),
        ),
        FollowStop::RecordCap => write_output_linef(
            output,
            format_args!("(follow stopped after {FOLLOW_MAX_RECORDS} records)"),
        ),
        FollowStop::Continue => Ok(()),
    }
}

fn parse_follow_filter(word: &str) -> Option<FollowFilter> {
    if let Some(service_id) = parse_service_name(word) {
        return Some(FollowFilter::Source(service_id));
    }
    parse_domain_word(word).map(FollowFilter::Domain)
}

/// Parks a non-blocking session-read-line request on the console session so a
/// later Ctrl-C (or submitted line) completes it while we stream logs.
fn arm_interrupt_read(session: rt::Handle) -> rt::Result<rt::Handle> {
    let reply = rt::channel_create()?;
    let mut request = RawMessage::empty(ConsoleTag::SessionReadLineRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rt::rights::SEND;
    let send_result = rt::channel_send(session, &request);
    let _ = rt::handle_close(reply.second);
    send_result?;
    Ok(reply.first)
}

/// Consumes the outstanding read-line reply when one is about to arrive so
/// the console session is left in a clean state for the next prompt. The wait
/// is bounded; if nothing arrives (terminal-pane endpoints ignore the
/// console read-line request, and an idle operator may not press a key), the
/// reply endpoint is closed and the main prompt loop retries read-line until
/// the stale slot clears.
fn drain_parked_read(parked: rt::Handle) {
    let mut response = RawMessage::empty(0);
    for _ in 0..DRAIN_PASSES {
        match rt::channel_receive_nonblocking(parked, &mut response) {
            Ok(()) | Err(rt::Error::InvalidCall) => break,
            Err(_) => {}
        }
        let _ = rt::yield_current();
    }
    let _ = rt::handle_close(parked);
}

const DRAIN_PASSES: usize = 16;

#[allow(clippy::too_many_arguments)]
fn poll_follow_pass(
    output: ShellOutput,
    subscription: rt::Handle,
    records_seen: &mut usize,
    last_activity: &mut u64,
    input_event: &mut Option<FollowStop>,
    parked_input: Option<rt::Handle>,
) -> FollowStop {
    loop {
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(subscription, &mut message) {
            Ok(()) => {
                if let Some(record) = decode_stream_record(&message) {
                    let _ = write_log_record(output, record);
                }
                *records_seen += 1;
                *last_activity = rt::monotonic_now().unwrap_or(*last_activity);
            }
            Err(_) => break,
        }
    }

    if input_event.is_none() {
        if let Some(parked) = parked_input {
            let mut response = RawMessage::empty(0);
            match rt::channel_receive_nonblocking(parked, &mut response) {
                Ok(()) => {
                    let len = (response.words[0] as usize).min(crate::MAX_LINE_BYTES);
                    if len == 0 {
                        *input_event = Some(FollowStop::Interrupted);
                    } else {
                        let (bytes, filled) = unpack_line_bytes(&response.words[1..], len);
                        stash_pending_line(&bytes[..filled]);
                        *input_event = Some(FollowStop::OperatorLine);
                    }
                }
                Err(_) => {}
            }
        }
    }

    let idle_ticks = rt::monotonic_now()
        .unwrap_or(*last_activity)
        .saturating_sub(*last_activity);
    follow_stop(*input_event, *records_seen, idle_ticks)
}

fn unpack_line_bytes(words: &[u64], len: usize) -> ([u8; crate::MAX_LINE_BYTES], usize) {
    let mut bytes = [0u8; crate::MAX_LINE_BYTES];
    let mut copied = 0usize;
    for word in words.iter().copied() {
        if copied >= len {
            break;
        }
        let word_bytes = word.to_le_bytes();
        let chunk = (len - copied).min(word_bytes.len());
        bytes[copied..copied + chunk].copy_from_slice(&word_bytes[..chunk]);
        copied += chunk;
    }
    (bytes, copied)
}

fn decode_stream_record(message: &RawMessage) -> Option<rt::LogRecord> {
    if message.tag != rt::LogTag::StreamRecord as u32 || message.word_count < 9 {
        return None;
    }
    Some(rt::LogRecord {
        sequence: message.words[0],
        tick: message.words[1],
        source: service_id_from_word(message.words[2]).unwrap_or(ServiceId::RootManager),
        severity: severity_from_word(message.words[3]),
        domain: domain_from_word(message.words[4]).unwrap_or(LogDomain::Service),
        event: event_from_word(message.words[5]),
        arg0: message.words[6],
        arg1: message.words[7],
        arg2: message.words[8],
    })
}

/// Word-to-event mapping for the shapes `write_log_record` renders specially;
/// everything else falls back to the generic record line.
fn event_from_word(value: u64) -> rt::LogEvent {
    use rt::LogEvent as E;
    match value {
        x if x == E::ServiceStarted as u64 => E::ServiceStarted,
        x if x == E::ServiceReady as u64 => E::ServiceReady,
        x if x == E::ServiceFailed as u64 => E::ServiceFailed,
        x if x == E::ServiceRestarting as u64 => E::ServiceRestarting,
        x if x == E::ConfigLoaded as u64 => E::ConfigLoaded,
        x if x == E::NetworkInterfaceReady as u64 => E::NetworkInterfaceReady,
        x if x == E::NetworkAddressConfigured as u64 => E::NetworkAddressConfigured,
        x if x == E::NetworkResolveCompleted as u64 => E::NetworkResolveCompleted,
        x if x == E::NetworkProbeCompleted as u64 => E::NetworkProbeCompleted,
        x if x == E::DisplayOutputReady as u64 => E::DisplayOutputReady,
        x if x == E::KernelTrap as u64 => E::KernelTrap,
        x if x == E::SurfaceCreated as u64 => E::SurfaceCreated,
        x if x == E::SessionReady as u64 => E::SessionReady,
        x if x == E::SessionFocusChanged as u64 => E::SessionFocusChanged,
        _ => E::ServiceStarted,
    }
}

pub(crate) fn cmd_logs_crashes(
    bootstrap: rt::Handle,
    output: ShellOutput,
    count: usize,
) -> rt::Result<()> {
    let requested = count.max(1).min(MAX_CRASH_ROWS);
    let log_handle = rt::lookup_service(bootstrap, ServiceId::Log)?;
    let (oldest, next) = rt::log_query_info(log_handle)?;
    let retained = next.saturating_sub(oldest);

    let mut matched = 0usize;
    for sequence in (oldest..next).rev() {
        let record = match rt::log_query_record(log_handle, sequence)? {
            Some(record) => record,
            None => continue,
        };
        let shape = CrashShape {
            severity_word: record.severity as u64,
            event_word: record.event as u64,
        };
        if !shape.is_crash() {
            continue;
        }
        print_crash_row(output, &record)?;
        matched += 1;
        if matched >= requested {
            break;
        }
        if matched % 4 == 0 {
            rt::yield_current()?;
        }
    }
    let _ = rt::handle_close(log_handle);

    if matched == 0 {
        write_output_linef(
            output,
            format_args!("no crash-shaped records in {retained} retained"),
        )
    } else {
        write_output_linef(
            output,
            format_args!("{} recent crash(es), {retained} records retained", matched),
        )
    }
}

fn print_crash_row(output: ShellOutput, record: &rt::LogRecord) -> rt::Result<()> {
    let source = service_cell(record.source as u32 as u64);
    write_output_linef(
        output,
        format_args!(
            "#{:<6} @{:<10} {:<21} {:<7} {} {} {} {}",
            record.sequence,
            record.tick,
            source,
            severity_cell(record.severity as u64),
            EventLabel(record.event as u64),
            record.arg0,
            record.arg1,
            record.arg2,
        ),
    )
}

struct EventLabel(u64);

impl core::fmt::Display for EventLabel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match event_name_from_word(self.0) {
            Some(name) => f.write_str(name),
            None => write!(f, "event-{}", self.0 as u32),
        }
    }
}

pub(crate) fn cmd_status_health(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let status_handle = rt::lookup_service(bootstrap, ServiceId::Status)?;
    let response = snapshot_status(status_handle)?;
    let _ = rt::handle_close(status_handle);
    if response.tag != StatusTag::SnapshotReply as u32 {
        return Err(rt::Error::InvalidArgument);
    }

    let now = rt::monotonic_now()?;
    let word_count = response.word_count as usize;
    if word_count < ROLLUP_WORDS {
        // Legacy snapshot without rollup words: degrade to the basic view.
        return write_output_linef(
            output,
            format_args!(
                "ticks={} heartbeats={} last-heartbeat={} tracked-services={}",
                now,
                response.words.first().copied().unwrap_or(0),
                response.words.get(1).copied().unwrap_or(0),
                response.words.get(2).copied().unwrap_or(0),
            ),
        );
    }

    let rollup =
        parse_health_rollup(&response.words[..word_count]).ok_or(rt::Error::InvalidArgument)?;
    write_health_table(output, &rollup, now)
}

/// Raw snapshot request against the status-service contract so the full
/// rollup reply stays visible to operator tooling (the shared runtime helper
/// only surfaces the first three words).
fn snapshot_status(status_handle: rt::Handle) -> rt::Result<RawMessage> {
    let reply = rt::channel_create()?;
    let mut request = RawMessage::empty(StatusTag::SnapshotRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rt::rights::SEND;
    rt::channel_send(status_handle, &request)?;
    let _ = rt::handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    rt::channel_receive_blocking(reply.first, &mut response)?;
    let _ = rt::handle_close(reply.first);
    Ok(response)
}

fn write_health_table(output: ShellOutput, rollup: &HealthRollup, now: u64) -> rt::Result<()> {
    write_output_linef(
        output,
        format_args!(
            "system health @tick {} heartbeats={} last-heartbeat={} tracked-services={}",
            now, rollup.heartbeats, rollup.last_tick, rollup.total,
        ),
    )?;
    write_output_linef(
        output,
        format_args!(
            "{:<10} {:>7} {:>7} {:>7} {:>10} {:>7} {:>7} {:>8}",
            "HEALTH", "healthy", "degrad", "failing", "recovering", "dormant", "unknown", "restart"
        ),
    )?;
    write_output_linef(
        output,
        format_args!(
            "{:<10} {:>7} {:>7} {:>7} {:>10} {:>7} {:>7} {:>8}",
            "services",
            rollup.healthy,
            rollup.degraded,
            rollup.failing,
            rollup.recovering,
            rollup.dormant,
            rollup.unknown,
            rollup.restarting_count,
        ),
    )?;
    write_output_linef(
        output,
        format_args!(
            "attention: problems={} degraded=[{}] restarting=[{}] worst-offender={}",
            rollup.problem_count(),
            IdList(rollup.degraded_ids, rollup.degraded_len),
            IdList(rollup.restarting_ids, rollup.restarting_len),
            rollup.worst_offender_label(),
        ),
    )
}

struct IdList([u32; 2], usize);

impl core::fmt::Display for IdList {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let (ids, len) = (self.0, self.1);
        let mut wrote = false;
        for id in ids.iter().take(len) {
            if *id == 0 {
                continue;
            }
            if wrote {
                f.write_str(",")?;
            }
            write!(f, "{}", service_cell(*id as u64))?;
            wrote = true;
        }
        if !wrote {
            f.write_str("none")?;
        }
        Ok(())
    }
}

pub(crate) fn cmd_status_svc(
    bootstrap: rt::Handle,
    output: ShellOutput,
    service_id: ServiceId,
) -> rt::Result<()> {
    let info = rt::manager_service_status(bootstrap, service_id)?;
    let template = rt::manager_service_template(bootstrap, service_id)?;
    let status_handle = rt::lookup_service(bootstrap, ServiceId::Status)?;
    let reported = rt::status_query_service(status_handle, service_id);
    let _ = rt::handle_close(status_handle);

    write_output_linef(
        output,
        format_args!(
            "service {} (id={})",
            service_name(service_id),
            service_id as u32
        ),
    )?;
    write_output_linef(
        output,
        format_args!(
            "manager: status={} phase={} startup={} availability={} attempts={} last-exit={:#x}",
            manager_status_name(info.status),
            phase_name(info.phase),
            startup_name(info.startup),
            availability_name(info.availability),
            info.attempts,
            info.last_exit,
        ),
    )?;
    write_output_linef(
        output,
        format_args!(
            "timing: blocked-on={} last-start={} last-ready={} next-restart={}",
            service_name(info.blocked_dependency),
            info.last_start_tick,
            info.last_ready_tick,
            info.next_restart_tick,
        ),
    )?;
    write_output_linef(
        output,
        format_args!(
            "policy: ready-timeout={} restart-limit={} restart-backoff={} grants={} lookups={}",
            template.ready_timeout_ticks,
            template.restart_limit,
            template.restart_backoff_ticks,
            template.grant_count,
            template.lookup_count,
        ),
    )?;
    match reported {
        Ok(Some(entry)) => write_output_linef(
            output,
            format_args!(
                "status-service: health={} detail={} {} {} updated={}",
                health_name(entry.health),
                detail_kind_name(entry.detail_kind),
                entry.detail0,
                entry.detail1,
                entry.updated_tick,
            ),
        ),
        Ok(None) => write_output_linef(output, format_args!("status-service: not reported yet")),
        Err(_) => write_output_linef(output, format_args!("status-service: query failed")),
    }
}

pub(crate) fn cmd_ps_app(
    bootstrap: rt::Handle,
    output: ShellOutput,
    app_word: Option<&str>,
) -> rt::Result<()> {
    // Deliberately built on the window-listing contract: live apps are the
    // ones with windows. The desktop list-apps reply shape is avoided because
    // its full page (3 + 4*4 words) exceeds the 16-word IPC limit server-side.
    let desktop_handle = rt::lookup_service(bootstrap, ServiceId::DesktopShell)?;
    let mut windows = [rt::DesktopWindowInfo {
        app_id: rt::DesktopAppId::Settings,
        surface_id: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        z_order: 0,
        focused: false,
        minimized: false,
        visible: false,
    }; MAX_DESKTOP_WINDOWS];
    let count = rt::desktop_list_windows(desktop_handle, &mut windows)?;
    let _ = rt::handle_close(desktop_handle);

    if let Some(app_id) = app_word.and_then(parse_desktop_app_name) {
        let mut listed = 0usize;
        for window in windows.iter().copied().take(count) {
            if window.app_id != app_id {
                continue;
            }
            listed += 1;
            write_output_linef(
                output,
                format_args!(
                    "app {} id={} state={} surface={} pos=({}, {}) size={}x{} z={} minimized={} visible={}",
                    desktop_app_name(window.app_id),
                    window.app_id as u32,
                    ps_state_cell(true, window.focused),
                    window.surface_id,
                    window.x,
                    window.y,
                    window.width,
                    window.height,
                    window.z_order,
                    window.minimized,
                    window.visible,
                ),
            )?;
        }
        if listed == 0 {
            return write_output_linef(
                output,
                format_args!("app {} not running", desktop_app_name(app_id)),
            );
        }
        return Ok(());
    }

    if count == 0 {
        return write_output_linef(output, format_args!("no desktop apps running"));
    }
    write_output_linef(
        output,
        format_args!(
            "{:<4} {:<12} {:<8} {:<8} {:<11} {}",
            "ID", "APP", "STATE", "SURFACE", "GEOMETRY", "FLAGS"
        ),
    )?;
    for window in windows.iter().copied().take(count) {
        let mut geometry = FixedValueText::empty();
        let _ = write!(geometry, "{}x{}", window.width, window.height);
        write_output_linef(
            output,
            format_args!(
                "{:<4} {:<12} {:<8} {:<8} {:<11} {}{}{}",
                window.app_id as u32,
                desktop_app_name(window.app_id),
                ps_state_cell(true, window.focused),
                window.surface_id,
                geometry,
                if window.focused { "f" } else { "" },
                if window.minimized { "m" } else { "" },
                if window.visible { "v" } else { "" },
            ),
        )?;
    }
    Ok(())
}

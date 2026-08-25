use core::fmt::Write as _;

use rt::{ConsoleTag, LogEvent, RawMessage};
use serviceos_userspace_runtime as rt;

use crate::format::{
    domain_from_word, domain_name, event_from_word, event_name, format_ipv4, format_mac,
    service_id_from_word, service_name, severity_from_word, severity_name, unpack_mac,
};
use crate::input::render_session_line;
use crate::session::{broadcast_grid_line, handle_session_open};
use crate::state::{BootProgress, MAX_SESSIONS, Session, active_session};

pub(crate) fn handle_public_message(
    sessions: &mut [Session; MAX_SESSIONS],
    boot_progress: &mut BootProgress,
    message: &RawMessage,
) -> rt::Result<()> {
    match message.tag {
        x if x == ConsoleTag::WriteRecord as u32 => {
            if message.word_count < 7 {
                return Ok(());
            }

            let source = service_id_from_word(message.words[0]);
            let severity = severity_from_word(message.words[1]);
            let domain = domain_from_word(message.words[2]);
            let event = event_from_word(message.words[3]);
            let target_service = service_id_from_word(message.words[4]);
            let boot_changed = (source == rt::ServiceId::RootManager
                || event == LogEvent::DesktopReady)
                && boot_progress.note_event(event, target_service);

            if boot_changed {
                write_boot_progress_line(sessions, *boot_progress)?;
            }

            if should_suppress_console_record(*boot_progress, severity, event) {
                return Ok(());
            }

            let _ = match event {
                LogEvent::ServiceStarted | LogEvent::ServiceReady | LogEvent::ServiceRestarting => {
                    write_structured_line(
                        sessions,
                        "console",
                        format_args!(
                            "seq={} level={} source={} domain={} event={} service={} detail={}",
                            message.words[6],
                            severity_name(severity),
                            service_name(source),
                            domain_name(domain),
                            event_name(event),
                            service_name(target_service),
                            message.words[5],
                        ),
                    )
                }
                LogEvent::ServiceFailed => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} service={} exit={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        service_name(target_service),
                        message.words[5],
                    ),
                ),
                LogEvent::LookupGranted => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} requester={} target={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        service_name(service_id_from_word(message.words[4])),
                        service_name(service_id_from_word(message.words[5])),
                    ),
                ),
                LogEvent::ConfigLoaded => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} minimum-severity={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                    ),
                ),
                LogEvent::StatusStarted => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} heartbeat-ticks={} console-period={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
                LogEvent::StatusHeartbeat | LogEvent::ConsoleWrite => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} count={} tick={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
                LogEvent::ConfigRead => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} key={} value={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
                LogEvent::NetworkInterfaceReady => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} iface={} mac={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        format_mac(unpack_mac(message.words[5])),
                    ),
                ),
                LogEvent::NetworkAddressConfigured => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} addr={} gateway={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        format_ipv4(message.words[4] as u32),
                        format_ipv4(message.words[5] as u32),
                    ),
                ),
                LogEvent::NetworkResolveCompleted => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} addr={} count={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        format_ipv4(message.words[4] as u32),
                        message.words[5],
                    ),
                ),
                LogEvent::NetworkProbeCompleted => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} addr={} elapsed-ms={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        format_ipv4(message.words[4] as u32),
                        message.words[5],
                    ),
                ),
                LogEvent::DisplayOutputReady => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} mode={}x{}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
                LogEvent::SurfaceCreated => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} surface={} session={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
                LogEvent::CompositorPresented => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} surfaces={} presents={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
                LogEvent::SessionReady => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} session={} surfaces={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
                LogEvent::SessionFocusChanged => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} session={} surface={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
                LogEvent::DesktopReady => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} session={} width={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
                LogEvent::DesktopAppLaunched
                | LogEvent::DesktopAppExited
                | LogEvent::DesktopFocusChanged
                | LogEvent::AppRendered
                | LogEvent::InputKeyDelivered => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} app={} detail={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
                _ => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} detail0={} detail1={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
            };
        }
        x if x == ConsoleTag::SessionOpenRequest as u32 => {
            if message.handle_count < 1 {
                return Ok(());
            }
            handle_session_open(sessions, message.handles[0])?;
        }
        _ => {}
    }

    Ok(())
}

fn should_suppress_console_record(
    boot_progress: BootProgress,
    severity: rt::LogSeverity,
    event: LogEvent,
) -> bool {
    if severity != rt::LogSeverity::Info {
        return false;
    }

    if !boot_progress.complete {
        return true;
    }

    !matches!(event, LogEvent::DesktopReady)
}

fn write_boot_progress_line(
    sessions: &mut [Session; MAX_SESSIONS],
    boot_progress: BootProgress,
) -> rt::Result<()> {
    let total = boot_progress.total_services();
    if total == 0 {
        return Ok(());
    }

    let ready = boot_progress.ready_services();
    let failed = boot_progress.failed_services();
    let starting = boot_progress.starting_services();
    let completed = ready.saturating_add(failed).min(total);
    let filled = (completed as usize * 20) / total as usize;
    let mut bar = [b'-'; 20];
    for slot in bar.iter_mut().take(filled) {
        *slot = b'=';
    }
    let bar = core::str::from_utf8(&bar).unwrap_or("--------------------");

    if active_session(sessions).is_some() {
        let _ = rt::debug_console_write(b"\r\n");
    }
    rt::write_logf(
        "boot",
        format_args!(
            "[{}] ready={}/{} starting={} failed={}",
            bar, ready, total, starting, failed
        ),
    )?;
    feed_grid_and_broadcast(
        sessions,
        format_args!(
            "boot [{}] ready={}/{} starting={} failed={}",
            bar, ready, total, starting, failed
        ),
    );
    if let Some(session) = active_session(sessions) {
        let _ = render_session_line(session);
    }
    Ok(())
}

/// Mirror one rendered console line into the retained VT grid and stream it
/// to subscribed console-session clients (graphical surfaces).
fn feed_grid_and_broadcast(sessions: &mut [Session; MAX_SESSIONS], args: core::fmt::Arguments<'_>) {
    let mut buffer = rt::FixedLogBuffer::<192>::new();
    let _ = buffer.write_fmt(args);
    crate::grid::record_line(buffer.as_bytes());
    broadcast_grid_line(sessions, buffer.as_bytes());
}

fn write_structured_line(
    sessions: &mut [Session; MAX_SESSIONS],
    domain: &str,
    args: core::fmt::Arguments<'_>,
) -> rt::Result<()> {
    if active_session(sessions).is_some() {
        let _ = rt::debug_console_write(b"\r\n");
    }
    rt::write_logf(domain, args)?;
    feed_grid_and_broadcast(sessions, args);
    if let Some(session) = active_session(sessions) {
        let _ = render_session_line(session);
    }
    Ok(())
}

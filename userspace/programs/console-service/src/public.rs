use serviceos_userspace_runtime as rt;
use rt::{ConsoleTag, LogEvent, RawMessage};

use crate::format::{
    domain_from_word, domain_name, event_from_word, event_name, format_ipv4, format_mac,
    service_id_from_word, service_name, severity_from_word, severity_name, unpack_mac,
};
use crate::input::render_session_line;
use crate::session::handle_session_open;
use crate::state::{active_session, Session, MAX_SESSIONS};

pub(crate) fn handle_public_message(
    sessions: &mut [Session; MAX_SESSIONS],
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
                            service_name(service_id_from_word(message.words[4])),
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
                        service_name(service_id_from_word(message.words[4])),
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

fn write_structured_line(
    sessions: &[Session; MAX_SESSIONS],
    domain: &str,
    args: core::fmt::Arguments<'_>,
) -> rt::Result<()> {
    if active_session(sessions).is_some() {
        let _ = rt::debug_console_write(b"\r\n");
    }
    rt::write_logf(domain, args)?;
    if let Some(session) = active_session(sessions) {
        let _ = render_session_line(session);
    }
    Ok(())
}

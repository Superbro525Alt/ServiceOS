use rt::{LogDomain, LogEvent, LogSeverity, ServiceId};
use serviceos_userspace_runtime as rt;

pub(crate) fn emit_log(
    log_handle: rt::Handle,
    severity: LogSeverity,
    event: LogEvent,
    arg0: u64,
    arg1: u64,
) -> rt::Result<()> {
    rt::send_log_record(
        log_handle,
        ServiceId::DesktopShell,
        severity,
        LogDomain::Desktop,
        event,
        arg0,
        arg1,
    )
}

pub(crate) fn emit_text_log(domain: &str, args: core::fmt::Arguments<'_>) -> rt::Result<()> {
    rt::write_logf(domain, args)
}

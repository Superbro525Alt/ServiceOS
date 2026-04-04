use serviceos_userspace_runtime as rt;

use crate::util::{
    domain_name, event_name, format_ipv4, format_mac, service_name, severity_name, unpack_mac,
    write_output_linef, ShellOutput,
};

pub(crate) fn write_log_record(output: ShellOutput, record: rt::LogRecord) -> rt::Result<()> {
    match record.event {
        rt::LogEvent::ConfigLoaded => write_output_linef(
            output,
            format_args!(
                "#{} {} {} {}/{} minimum-severity={}",
                record.sequence,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                record.arg0,
            ),
        ),
        rt::LogEvent::NetworkInterfaceReady => write_output_linef(
            output,
            format_args!(
                "#{} {} {} {}/{} iface={} mac={}",
                record.sequence,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                record.arg0,
                format_mac(unpack_mac(record.arg1)),
            ),
        ),
        rt::LogEvent::NetworkAddressConfigured => write_output_linef(
            output,
            format_args!(
                "#{} {} {} {}/{} addr={} gateway={}",
                record.sequence,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                format_ipv4(record.arg0 as u32),
                format_ipv4(record.arg1 as u32),
            ),
        ),
        rt::LogEvent::NetworkResolveCompleted => write_output_linef(
            output,
            format_args!(
                "#{} {} {} {}/{} addr={} count={}",
                record.sequence,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                format_ipv4(record.arg0 as u32),
                record.arg1,
            ),
        ),
        rt::LogEvent::NetworkProbeCompleted => write_output_linef(
            output,
            format_args!(
                "#{} {} {} {}/{} addr={} elapsed-ms={}",
                record.sequence,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                format_ipv4(record.arg0 as u32),
                record.arg1,
            ),
        ),
        rt::LogEvent::DisplayOutputReady => write_output_linef(
            output,
            format_args!(
                "#{}@{} {} {} {}/{} {}x{}",
                record.sequence,
                record.tick,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                record.arg0,
                record.arg1,
            ),
        ),
        rt::LogEvent::KernelTrap => write_output_linef(
            output,
            format_args!(
                "#{}@{} {} {} {}/{} code={:#x} ip={:#x} aux={:#x}",
                record.sequence,
                record.tick,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                record.arg0,
                record.arg1,
                record.arg2,
            ),
        ),
        rt::LogEvent::SurfaceCreated
        | rt::LogEvent::SessionReady
        | rt::LogEvent::SessionFocusChanged => write_output_linef(
            output,
            format_args!(
                "#{}@{} {} {} {}/{} {} {}",
                record.sequence,
                record.tick,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                record.arg0,
                record.arg1,
            ),
        ),
        _ => write_output_linef(
            output,
            format_args!(
                "#{}@{} {} {} {}/{} {} {} {}",
                record.sequence,
                record.tick,
                severity_name(record.severity),
                service_name(record.source),
                domain_name(record.domain),
                event_name(record.event),
                record.arg0,
                record.arg1,
                record.arg2,
            ),
        ),
    }
}

use serviceos_abi::{ServiceId, ServiceImageId, rights};

use crate::{BootStoreError, ServiceGrant};

pub fn parse_service_grant(value: &str) -> Result<ServiceGrant, BootStoreError> {
    let Some((service, rights)) = value.split_once(':') else {
        return Err(BootStoreError::InvalidManifest);
    };
    Ok(ServiceGrant {
        target: parse_service_id(service.trim())?,
        rights: parse_rights(rights.trim())?,
    })
}

pub fn parse_rights(value: &str) -> Result<u64, BootStoreError> {
    let mut bits = rights::NONE;
    for part in value.split('+').map(str::trim).filter(|v| !v.is_empty()) {
        bits |= match part {
            "read" => rights::READ,
            "write" => rights::WRITE,
            "map" => rights::MAP,
            "signal" => rights::SIGNAL,
            "wait" => rights::WAIT,
            "send" => rights::SEND,
            "receive" => rights::RECEIVE,
            "duplicate" => rights::DUPLICATE,
            "transfer" => rights::TRANSFER,
            "manage" => rights::MANAGE,
            _ => return Err(BootStoreError::InvalidManifest),
        };
    }
    Ok(bits)
}

pub fn parse_service_id(value: &str) -> Result<ServiceId, BootStoreError> {
    match value {
        "root-manager" => Ok(ServiceId::RootManager),
        "storage-service" => Ok(ServiceId::Storage),
        "console-service" => Ok(ServiceId::Console),
        "config-service" => Ok(ServiceId::Config),
        "log-service" => Ok(ServiceId::Log),
        "status-service" => Ok(ServiceId::Status),
        "shell-service" => Ok(ServiceId::Shell),
        "package-service" => Ok(ServiceId::Package),
        "announce-service" => Ok(ServiceId::Announce),
        "network-service" => Ok(ServiceId::Network),
        "graphics-service" => Ok(ServiceId::Graphics),
        "session-service" => Ok(ServiceId::Session),
        "desktop-shell-service" => Ok(ServiceId::DesktopShell),
        "terminal-service" => Ok(ServiceId::Terminal),
        "audio-service" => Ok(ServiceId::Audio),
        "runtime-service" => Ok(ServiceId::Runtime),
        "developer-service" => Ok(ServiceId::Developer),
        "clipboard-service" => Ok(ServiceId::Clipboard),
        "security-service" => Ok(ServiceId::Security),
        _ => Err(BootStoreError::InvalidManifest),
    }
}

pub fn parse_image_id(value: &str) -> Result<ServiceImageId, BootStoreError> {
    match value {
        "root-manager" => Ok(ServiceImageId::RootManager),
        "storage-service" => Ok(ServiceImageId::StorageService),
        "console-service" => Ok(ServiceImageId::ConsoleService),
        "config-service" => Ok(ServiceImageId::ConfigService),
        "log-service" => Ok(ServiceImageId::LogService),
        "status-service" => Ok(ServiceImageId::StatusService),
        "shell-service" => Ok(ServiceImageId::ShellService),
        "sysinfo-tool" => Ok(ServiceImageId::SysinfoTool),
        "package-service" => Ok(ServiceImageId::PackageService),
        "announce-service" => Ok(ServiceImageId::AnnounceService),
        "network-service" => Ok(ServiceImageId::NetworkService),
        "graphics-service" => Ok(ServiceImageId::GraphicsService),
        "session-service" => Ok(ServiceImageId::SessionService),
        "desktop-shell-service" => Ok(ServiceImageId::DesktopShellService),
        "terminal-service" => Ok(ServiceImageId::TerminalService),
        "terminal-app" => Ok(ServiceImageId::TerminalApp),
        "audio-service" => Ok(ServiceImageId::AudioService),
        "runtime-service" => Ok(ServiceImageId::RuntimeService),
        "posix-host-tool" => Ok(ServiceImageId::PosixHostTool),
        "developer-service" => Ok(ServiceImageId::DeveloperService),
        "cross-builder-tool" => Ok(ServiceImageId::CrossBuilderTool),
        "clipboard-service" => Ok(ServiceImageId::ClipboardService),
        "software-center-app" => Ok(ServiceImageId::SoftwareCenterApp),
        "security-service" => Ok(ServiceImageId::SecurityService),
        _ => Err(BootStoreError::InvalidManifest),
    }
}

pub fn parse_u32(value: &str) -> Result<u32, BootStoreError> {
    value
        .parse::<u32>()
        .map_err(|_| BootStoreError::InvalidManifest)
}

pub fn parse_integrity(value: &str) -> Result<u64, BootStoreError> {
    let Some(hex) = value.strip_prefix("fnv64:0x") else {
        return Err(BootStoreError::InvalidManifest);
    };
    u64::from_str_radix(hex, 16).map_err(|_| BootStoreError::InvalidManifest)
}

pub fn split_key_value(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once('=')?;
    Some((key.trim(), value.trim()))
}

pub fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, BootStoreError> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or(BootStoreError::Truncated)?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

pub fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, BootStoreError> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or(BootStoreError::Truncated)?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_manifest, parse_package_manifest};

    #[test]
    fn manifest_parser_accepts_service_graph_schema() {
        let manifest = parse_manifest(
            br#"
service=status-service
image=status-service
startup=eager
availability=required
ready_timeout=250
restart=on-failure:2:8
depends=log-service, config-service
grant=log-service:send
lookup=config-service:send
resource=services/status-service/resources/banner.txt
"#,
        )
        .expect("manifest should parse");

        assert_eq!(manifest.service_id, ServiceId::Status);
        assert_eq!(manifest.image_id, ServiceImageId::StatusService);
        assert_eq!(manifest.startup, crate::ServiceStartupMode::Eager);
        assert_eq!(manifest.availability, crate::ServiceAvailability::Required);
        assert_eq!(manifest.ready_timeout_ticks, 250);
        assert_eq!(
            manifest.restart,
            crate::RestartPolicy::OnFailure {
                max_restarts: 2,
                backoff_ticks: 8
            }
        );
        assert_eq!(manifest.dependency_count, 2);
        assert_eq!(manifest.grant_count, 1);
        assert_eq!(manifest.lookup_count, 1);
        assert_eq!(
            manifest.resources[0].as_str().expect("resource path"),
            "services/status-service/resources/banner.txt"
        );
    }

    #[test]
    fn package_manifest_parser_accepts_repository_schema() {
        let manifest = parse_package_manifest(
            br#"
package=announce-service
version=1.1.0
compat=serviceos.bootstore.v1
service=announce-service
service_manifest=packages/announce-service/1.1.0/service/manifest.svc
activation=manual
depends=log-service
content=packages/announce-service/1.1.0/service/manifest.svc
content=packages/announce-service/1.1.0/resources/message.txt
integrity=fnv64:0x1234
"#,
        )
        .expect("package manifest should parse");

        assert_eq!(manifest.service_id, ServiceId::Announce);
        assert_eq!(
            manifest
                .service_manifest
                .as_str()
                .expect("service manifest path"),
            "packages/announce-service/1.1.0/service/manifest.svc"
        );
        assert_eq!(manifest.content_count, 2);
        assert_eq!(manifest.integrity, 0x1234);
    }

    #[test]
    fn restart_policy_matrix_parses_all_shapes() {
        let fail_stop =
            parse_manifest(b"service=log-service\nimage=log-service\nrestart=fail-stop\n")
                .expect("fail-stop manifest should parse");
        assert_eq!(fail_stop.restart, crate::RestartPolicy::FailStop);

        let supervisor = parse_manifest(
            b"service=log-service\nimage=log-service\nrestart=supervisor:status-service:3:20\n",
        )
        .expect("supervisor manifest should parse");
        assert_eq!(
            supervisor.restart,
            crate::RestartPolicy::SupervisorRestart {
                supervisor: ServiceId::Status,
                max_restarts: 3,
                backoff_ticks: 20
            }
        );

        let supervisor_default_backoff = parse_manifest(
            b"service=log-service\nimage=log-service\nrestart=supervisor:console-service:1\n",
        )
        .expect("supervisor single-word manifest should parse");
        assert_eq!(
            supervisor_default_backoff.restart,
            crate::RestartPolicy::SupervisorRestart {
                supervisor: ServiceId::Console,
                max_restarts: 1,
                backoff_ticks: 0
            }
        );

        // Legacy shapes stay byte-compatible.
        let legacy =
            parse_manifest(b"service=log-service\nimage=log-service\nrestart=on-failure:0\n")
                .expect("legacy on-failure manifest should parse");
        assert_eq!(
            legacy.restart,
            crate::RestartPolicy::OnFailure {
                max_restarts: 0,
                backoff_ticks: 0
            }
        );

        for bad in [
            "restart=bogus:1",
            "restart=supervisor:not-a-service:1",
            "restart=on-failure:1:2:3",
            "restart=supervisor:",
        ] {
            let mut text = b"service=log-service\nimage=log-service\n".to_vec();
            text.extend_from_slice(bad.as_bytes());
            text.extend_from_slice(b"\n");
            assert!(parse_manifest(&text).is_err(), "{bad} should not parse");
        }
    }
}

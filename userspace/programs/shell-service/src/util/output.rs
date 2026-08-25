use core::fmt::Write;

use rt::{FixedLogBuffer, LogDomain, LogEvent, LogSeverity, ServiceId};
use serviceos_userspace_runtime as rt;

pub(crate) const MAX_LISTED_SERVICES: usize = 24;
pub(crate) const MAX_STORAGE_PATH: usize = 96;
pub(crate) const MAX_CAT_CHUNK: usize = 96;
pub(crate) const MAX_VERSION_BYTES: usize = 24;
pub(crate) const MAX_DESKTOP_APPS: usize = 8;
pub(crate) const MAX_DESKTOP_WINDOWS: usize = 8;
pub(crate) const MAX_SESSION_WRITE_BYTES: usize = (rt::IPC_MAX_WORDS - 1) * 8;

#[derive(Clone, Copy)]
pub struct ShellOutput {
    pub handle: rt::Handle,
    pub write: fn(rt::Handle, &str) -> rt::Result<()>,
}

impl ShellOutput {
    pub const fn new(handle: rt::Handle, write: fn(rt::Handle, &str) -> rt::Result<()>) -> Self {
        Self { handle, write }
    }
}

pub const HELP_TEXT: &str = "\
help: show this command list\r\n\
sessions: list operator sessions hosted by this shell\r\n\
history [count]: show this session's command history (oldest first)\r\n\
login <name> <secret>: bind an account identity to this operator session\r\n\
whoami: show the owning identity of this operator session\r\n\
logout: unbind this session's account identity\r\n\
console grid: render the kernel console VT grid snapshot\r\n\
console follow: stream live kernel console records until idle timeout\r\n\
services: list managed services\r\n\
service <name>: show one service state\r\n\
service-caps <name>: inspect delegated lookup capabilities for one service\r\n\
service-revoke-lookup <service> <target> <revoke|default>: change future delegated lookup policy\r\n\
restart <name>: request a service restart\r\n\
logs [count]: show recent structured logs\r\n\
logs stream [count]: subscribe to live structured logs\r\n\
logs follow <domain|service>: stream live records until Ctrl-C (console) or idle timeout\r\n\
logs crashes [count]: list recent crash-shaped log records\r\n\
config: show core configuration values\r\n\
config get <key>: read one persisted configuration key\r\n\
config set <key> <value>: update one persisted configuration key\r\n\
store ls [prefix]: list storage paths\r\n\
store mounts: list storage namespace mounts\r\n\
store mkdir <path>: create a writable directory under a mutable namespace\r\n\
store write <path> <text>: create or replace a writable text file\r\n\
store rm <path>: remove a writable file or empty directory\r\n\
cat <path>: print a text resource\r\n\
status: show status-service heartbeat and tracked service count\r\n\
status services: list structured service health/status entries\r\n\
status health: show system health rollup from the status snapshot\r\n\
status svc <name>: inspect one service across manager and status views\r\n\
ps app [name]: list desktop apps ps-style or inspect one app\r\n\
status watch [count]: stream status changes from status-service\r\n\
net ifaces: show network interfaces\r\n\
net route: show the default route\r\n\
net sockets: show active network sockets\r\n\
net resolve <name>: resolve a host or literal\r\n\
net ping <name|ip>: run an ICMP reachability probe\r\n\
net http <host> [path]: fetch a URL over TCP through network-service\r\n\
audio endpoints: show audio endpoints\r\n\
audio streams: show active audio streams\r\n\
audio tone <hz> [ms]: play a diagnostic tone through audio-service\r\n\
runtime envs: list compatibility/runtime environments\r\n\
runtime create posix: create a posix-like runtime environment\r\n\
runtime inspect <env-id>: show one runtime environment\r\n\
runtime mounts <env-id>: list mapped runtime mounts\r\n\
runtime vars <env-id>: list runtime environment variables\r\n\
runtime runs: list runtime-backed workloads\r\n\
runtime launch <env-id> <inspect|env|mounts|cat> [path]: launch a runtime-backed workload\r\n\
runtime destroy <env-id>: destroy an idle runtime environment\r\n\
security apps: list native app launch policies and sensitive capability groups\r\n\
security app <name> [allow|block|default]: review or override one native app policy\r\n\
security runtimes: list runtime environments and their effective authority state\r\n\
security runtime <env-id> [approve|deny|reset]: review or change one runtime approval state\r\n\
security repos: list repository trust and sync state\r\n\
security package <name>: inspect package trust/provenance state\r\n\
security workspace <id>: inspect developer workspace authority metadata\r\n\
security audit [count]: show recent native/runtime security audit records\r\n\
dev toolchains: list developer toolchains and target support\r\n\
dev toolchain <id>: show one toolchain and its SDK root\r\n\
dev workspaces: list packaged developer workspaces\r\n\
dev workspace <id>: show one workspace and its target mappings\r\n\
dev build <workspace-id> <native|linux|windows|macos>: run a build job\r\n\
dev jobs: list developer build jobs\r\n\
dev artifact <job-id>: inspect one built artifact\r\n\
dev save <job-id> <path>: save one built artifact into writable storage\r\n\
gfx outputs: show graphics outputs\r\n\
gfx surfaces: show compositor surfaces\r\n\
gfx sessions: show graphical sessions\r\n\
gfx focus <surface-id>: change focused session surface\r\n\
desktop status: show desktop shell status\r\n\
desktop apps: list desktop app state\r\n\
desktop windows: list desktop window state\r\n\
desktop workspace [status]: show current workspace state\r\n\
desktop workspace switch <1-4>: switch desktop workspace\r\n\
desktop workspace move <1-4>: move the focused window to another workspace\r\n\
desktop notifications [count]: show desktop notification history\r\n\
desktop launch <settings|files|monitor|terminal|software>: launch a desktop app\r\n\
desktop focus <settings|files|monitor|terminal|software>: focus a desktop app\r\n\
desktop next: focus the next visible window\r\n\
desktop close <settings|files|monitor|terminal|software>: close a desktop app window\r\n\
desktop minimize <settings|files|monitor|terminal|software>: minimize a desktop app window\r\n\
desktop restore <settings|files|monitor|terminal|software>: restore a minimized app window\r\n\
desktop maximize <settings|files|monitor|terminal|software>: maximize or restore a window\r\n\
desktop move <settings|files|monitor|terminal|software> <x> <y>: move a window\r\n\
desktop resize <settings|files|monitor|terminal|software> <width> <height>: resize a window\r\n\
desktop click <x> <y>: inject a pointer click into the desktop session\r\n\
desktop notify <text>: post a desktop shell notification\r\n\
desktop open <path>: open a storage path in the files app\r\n\
desktop launch terminal: open the graphical terminal app\r\n\
pkg list: list repository packages\r\n\
pkg catalog: browse the current package catalog\r\n\
pkg repos: list configured package repositories\r\n\
pkg repo add <name> <url> [trust] [channel] [ring]: review then register a third-party repository (--yes commits)\r\n\
pkg repo enable|disable <name>: change operator state for an onboarded source\r\n\
pkg repo remove <name>: revoke an onboarded source's approval\r\n\
pkg repo status: show onboarding ledger, side-load policy, and host arch\r\n\
pkg sideload policy [allow|warn|deny]: set the side-loading policy switch\r\n\
pkg repo sync [all|index]: fetch repository metadata through package-service\r\n\
pkg info <name>: inspect one package\r\n\
  pkg install <name> [version] [@source] [--yes] [--force-compat]: activate a package from a chosen source\r\n\
  pkg update <name> [version] [@source] [--yes] [--force-compat]: switch to a newer package version\r\n\
 pkg remove <name>: deactivate a package\r\n\
 pkg rollback <name>: restore the prior active version (prints the rollback summary)\r\n\
pkg history <name>: show current and rollback versions\r\n\
pkg provenance <name>: inspect package source and trust state\r\n\
pkg policy <name>: inspect package channel/ring/pin policy\r\n\
pkg pin <name> <version|none>: pin or unpin a package version\r\n\
pkg channel <name> <stable|beta|canary>: set package update channel\r\n\
pkg ring <name> <production|preview|testing>: set staged rollout ring\r\n\
 pkg verify: validate installed package state (also shows operation-journal status)\r\n\
 pkg repair: repair interrupted or broken package state\r\n\
 pkg recover: resume or discard an interrupted install/update/rollback\r\n\
pkg gc: garbage-collect old package artifacts\r\n\
run sysinfo: launch a transient tool\r\n\
run pkg <name>: launch an installed package through the manager launch path\r\n\
run image <path>: launch a flat image resource through the manager loader path\r\n";

pub fn emit_shell_log(
    bootstrap: rt::Handle,
    source_service: ServiceId,
    severity: LogSeverity,
    event: LogEvent,
    arg0: u64,
    arg1: u64,
) -> rt::Result<()> {
    let log_handle = rt::lookup_service(bootstrap, ServiceId::Log)?;
    let result = rt::send_log_record(
        log_handle,
        source_service,
        severity,
        LogDomain::Shell,
        event,
        arg0,
        arg1,
    );
    let _ = rt::handle_close(log_handle);
    result
}

pub fn write_output_linef(output: ShellOutput, args: core::fmt::Arguments<'_>) -> rt::Result<()> {
    let mut buffer = FixedLogBuffer::<256>::new();
    let _ = buffer.write_fmt(args);
    let _ = buffer.write_str("\r\n");
    let text = core::str::from_utf8(buffer.as_bytes()).map_err(|_| rt::Error::InvalidArgument)?;
    shell_output_write(output, text)
}

pub fn shell_output_write(output: ShellOutput, text: &str) -> rt::Result<()> {
    let bytes = text.as_bytes();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let end = (offset + MAX_SESSION_WRITE_BYTES).min(bytes.len());
        let chunk =
            core::str::from_utf8(&bytes[offset..end]).map_err(|_| rt::Error::InvalidArgument)?;
        (output.write)(output.handle, chunk)?;
        offset = end;
    }
    Ok(())
}

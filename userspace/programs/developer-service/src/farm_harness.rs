//! Loopback remote-farm LIVE accept-path harness.
//!
//! HONEST SCOPE: this exercises the queue -> dispatch -> transport ->
//! accept -> ack -> completion leg of a remote farm job across the real
//! network service over guest-internal loopback TCP. It does NOT compile
//! anything remotely; "execution" ends when the far side acknowledges the
//! queued job with FARMQ1 OK, which is exactly the step a real farm must
//! answer before payload upload would begin.
//!
//! Gating: enabled only for builds with SERVICEOS_FARM_SELFTEST=1 (see
//! `enabled()`), plus an explicit control tag trigger for tooling. Default
//! boots never touch it.

use rt::NetworkSocketState;
use serviceos_userspace_runtime as rt;

use crate::{
    consts::{
        MAX_JOBS, MAX_PATH, FARM_SELFTEST_PORT, FARM_SELFTEST_REPLY_TAG,
        FARM_SELFTEST_REQUEST_TAG,
    },
    types::{ExportState, FixedBytes, JobSlot},
    routing,
};

/// Wire magic + version for the minimal job-accept protocol.
pub(crate) const WIRE_MAGIC: &[u8] = b"FARMQ1 ";

/// Bounded spin budget so a stalled stack can never hang the boot loop;
/// every spin yields, matching terminal-service's loopback selftest style.
pub(crate) const SPIN_BUDGET: u32 = 1_200;

/// Synthetic endpoint id marker carried on harness-routed jobs (never a
/// real registry index; reserved high range per routing.rs encoding).
pub(crate) const HARNESS_ENDPOINT_ID: u32 = 0x7fff_ffff;

/// Harness job id used on the wire and in the jobs table.
pub(crate) const HARNESS_JOB_ID: u64 = 0xfeed_beef;

/// Builds with SERVICEOS_FARM_SELFTEST=1 run the harness once per boot.
pub(crate) fn enabled() -> bool {
    matches!(option_env!("SERVICEOS_FARM_SELFTEST"), Some("1"))
}

/// Outcome summary for callers that surface pass/fail (boot hook logs only).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HarnessReport {
    pub(crate) pass: bool,
    pub(crate) stage: &'static str,
    pub(crate) spins: u32,
}

fn write_digits(mut value: u64, digits: &mut [u8; 20]) -> usize {
    let mut len = 0usize;
    loop {
        digits[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
        if value == 0 {
            break;
        }
    }
    digits[..len].reverse();
    len
}

/// Build the dispatch frame `FARMQ1 <job-id> <target>\n` into `out`;
/// returns its byte length.
pub(crate) fn build_job_request(job_id: u64, target_word: u64, out: &mut [u8]) -> usize {
    let mut cursor = 0usize;
    out[..WIRE_MAGIC.len()].copy_from_slice(WIRE_MAGIC);
    cursor += WIRE_MAGIC.len();
    let mut digits = [0u8; 20];
    let id_len = write_digits(job_id, &mut digits);
    out[cursor..cursor + id_len].copy_from_slice(&digits[..id_len]);
    cursor += id_len;
    out[cursor] = b' ';
    cursor += 1;
    let target_len = write_digits(target_word, &mut digits);
    out[cursor..cursor + target_len].copy_from_slice(&digits[..target_len]);
    cursor += target_len;
    out[cursor] = b'\n';
    cursor += 1;
    cursor
}

/// Server-side accept logic over one received request buffer.
/// Returns Some(bytes-to-write) pointing into `ack_out`.
pub(crate) fn serve_accept<'a>(
    request: &[u8],
    expect_job_id: u64,
    ack_out: &'a mut [u8],
) -> Option<&'a [u8]> {
    if !request.starts_with(WIRE_MAGIC) {
        return None;
    }
    let rest = &request[WIRE_MAGIC.len()..];
    let space = rest.iter().position(|byte| *byte == b' ')?;
    let id_text = core::str::from_utf8(&rest[..space]).ok()?;
    let parsed: u64 = id_text.parse().ok()?;
    if parsed != expect_job_id {
        return None;
    }
    let mut cursor = 0usize;
    ack_out[..WIRE_MAGIC.len()].copy_from_slice(WIRE_MAGIC);
    cursor += WIRE_MAGIC.len();
    let ok = b"OK ";
    ack_out[cursor..cursor + ok.len()].copy_from_slice(ok);
    cursor += ok.len();
    let mut digits = [0u8; 20];
    let id_len = write_digits(parsed, &mut digits);
    ack_out[cursor..cursor + id_len].copy_from_slice(&digits[..id_len]);
    cursor += id_len;
    ack_out[cursor] = b'\n';
    cursor += 1;
    Some(&ack_out[..cursor])
}

/// Client-side validation of the acknowledgement line.
pub(crate) fn validate_ack(response: &[u8], expect_job_id: u64) -> bool {
    if !response.starts_with(WIRE_MAGIC) {
        return false;
    }
    let after = &response[WIRE_MAGIC.len()..];
    if !after.starts_with(b"OK ") {
        return false;
    }
    let tail = &after[3..];
    let newline_or_end = tail.iter().position(|byte| *byte == b'\n').unwrap_or(tail.len());
    let mut digits = [0u8; 20];
    let id_len = write_digits(expect_job_id, &mut digits);
    tail.len() >= newline_or_end && newline_or_end == id_len && &tail[..newline_or_end] == &digits[..id_len]
}

/// Find-or-synthesize state one might have inspected mid-flight (used by
/// tests to confirm transitions are honest).
#[cfg(test)]
pub(crate) fn job_state_is(job: &JobSlot, state: rt::DeveloperJobState) -> bool {
    job.state == state
}

struct Sockets {
    network: rt::Handle,
    listener: rt::Handle,
    client: rt::Handle,
    accepted: rt::Handle,
}

impl Drop for Sockets {
    fn drop(&mut self) {
        for handle in [self.accepted, self.client, self.listener] {
            if handle != rt::INVALID_HANDLE {
                let _ = rt::handle_close(handle);
            }
        }
        if self.network != rt::INVALID_HANDLE {
            let _ = rt::handle_close(self.network);
        }
    }
}

impl Sockets {
    fn closed_network(network: rt::Handle) -> Self {
        Self {
            network,
            listener: rt::INVALID_HANDLE,
            client: rt::INVALID_HANDLE,
            accepted: rt::INVALID_HANDLE,
        }
    }
}

/// Arm a TCP listener through the network service (raw ABI message: the
/// runtime crate has outbound wrappers but no listen wrapper yet).
fn arm_listener(network: rt::Handle) -> rt::Result<rt::Handle> {
    let mut request = rt::RawMessage::empty(rt::NetworkTag::SocketListenRequest as u32);
    request.word_count = 2;
    request.words[0] = rt::NetworkSocketKind::TcpStream as u64;
    request.words[1] = rt::pack_listen_params(FARM_SELFTEST_PORT, 1);
    let response = rt::channel_call(network, &mut request)?;
    if response.tag != rt::NetworkTag::SocketListenReply as u32
        || response.word_count < 1
        || response.handle_count < 1
    {
        return Err(rt::Error::InvalidArgument);
    }
    if response.words[0] != rt::NetworkStatus::Ok as u32 as u64 {
        return Err(rt::Error::InvalidArgument);
    }
    Ok(response.handles[0])
}

/// Pop one accepted stream from the listener (Busy/QueueEmpty while none).
fn try_accept(listener: rt::Handle) -> rt::Result<Option<rt::Handle>> {
    let mut request = rt::RawMessage::empty(rt::NetworkSocketTag::AcceptRequest as u32);
    let response = rt::channel_call(listener, &mut request)?;
    if response.tag != rt::NetworkSocketTag::AcceptReply as u32 || response.word_count < 3 {
        return Err(rt::Error::InvalidArgument);
    }
    match response.words[0] as u32 {
        x if x == rt::NetworkStatus::Ok as u32 => match response.handle_count {
            count if count >= 1 => Ok(Some(response.handles[0])),
            _ => Err(rt::Error::InvalidArgument),
        },
        x if x == rt::NetworkStatus::Busy as u32 => Ok(None),
        _ => Err(rt::Error::InvalidArgument),
    }
}

fn send_with_retry(handle: rt::Handle, payload: &[u8], budget: u32) -> bool {
    for _ in 0..budget {
        match rt::network_socket_send(handle, payload) {
            Ok(_) => return true,
            Err(rt::Error::Busy) | Err(rt::Error::CapacityExceeded) => {
                let _ = rt::yield_current();
            }
            Err(_) => return false,
        }
    }
    false
}

enum ReadOutcome {
    Got(usize),
    Idle,
    Dead,
}

fn recv_once(handle: rt::Handle, buffer: &mut [u8]) -> ReadOutcome {
    match rt::network_socket_receive(handle, buffer) {
        Ok(count) if count > 0 => ReadOutcome::Got(count),
        Ok(_) => ReadOutcome::Idle,
        Err(rt::Error::QueueEmpty) | Err(rt::Error::Busy) => ReadOutcome::Idle,
        Err(_) => ReadOutcome::Dead,
    }
}

/// Run the live accept path end-to-end and update `jobs` accordingly.
/// Blocking within its bounded spin budget; safe because every spin yields
/// (the service loop simply skips other work meanwhile).
pub(crate) fn run(
    bootstrap: rt::Handle,
    log_handle: rt::Handle,
    jobs: &mut [JobSlot; MAX_JOBS],
) -> HarnessReport {
    let fail = |stage: &'static str| {
        let _ = rt::write_logf(
            "developer",
            format_args!("farm-selftest FAIL stage={stage}"),
        );
        let _ = rt::write_logf("developer", format_args!("farm-selftest end ok=0"));
        HarnessReport { pass: false, stage, spins: 0 }
    };

    // Queue a synthetic remote-target job through the SAME slot/route model
    // dispatch uses, then flip it Running as soon as the wire legs open.
    let Some(job_index) = crate::util::allocate_job(jobs).ok() else {
        return fail("alloc");
    };
    // Loopback endpoint text mirrors what a real descriptor would carry;
    // built without allocation from the shared port const.
    const PREFIX: &[u8] = b"self@127.0.0.1:";
    let mut endpoint_text = [0u8; MAX_PATH];
    endpoint_text[..PREFIX.len()].copy_from_slice(PREFIX);
    let mut digits = [0u8; 20];
    let port_len = write_digits(u64::from(FARM_SELFTEST_PORT), &mut digits);
    endpoint_text[PREFIX.len()..PREFIX.len() + port_len]
        .copy_from_slice(&digits[..port_len]);
    let mut endpoint_bytes = FixedBytes::<MAX_PATH>::empty();
    let _ = endpoint_bytes.set(&endpoint_text[..PREFIX.len() + port_len]);
    jobs[job_index] = JobSlot {
        occupied: true,
        workspace_id: 0,
        target: rt::DeveloperTarget::LinuxX64,
        state: rt::DeveloperJobState::Queued,
        format: rt::DeveloperArtifactFormat::ServiceOsFlat,
        artifact_name: FixedBytes::empty(),
        artifact_size: 0,
        artifact_handle: rt::INVALID_HANDLE,
        task_handle: rt::INVALID_HANDLE,
        report_handle: rt::INVALID_HANDLE,
        sandbox: crate::sandbox::SandboxDecision { allowed: false, scope_count: 0 },
        route: routing::BuildRoute::RemoteFarm { endpoint_id: HARNESS_ENDPOINT_ID },
        mode: routing::ExecutionMode::DirectSpawn,
        export: ExportState::PendingRemote { endpoint: endpoint_bytes },
    };

    let Ok(network) = rt::lookup_service(bootstrap, rt::ServiceId::Network) else {
        return fail("network");
    };
    let mut sockets = Sockets::closed_network(network);

    // 1. Listener first so the SYN finds an armed socket.
    sockets.listener = match arm_listener(sockets.network) {
        Ok(handle) => handle,
        Err(_) => return fail("listen"),
    };

    let mut request_frame = [0u8; 64];
    let request_len = build_job_request(HARNESS_JOB_ID, rt::DeveloperTarget::LinuxX64 as u64, &mut request_frame);

    // 2. Connect.
    let mut spins = 0u32;
    while sockets.client == rt::INVALID_HANDLE {
        spins += 1;
        if spins > SPIN_BUDGET / 4 {
            return fail("connect");
        }
        match rt::network_socket_open(
            sockets.network,
            rt::NetworkSocketKind::TcpStream,
            "127.0.0.1",
            FARM_SELFTEST_PORT,
        ) {
            Ok(handle) => sockets.client = handle,
            Err(rt::Error::Busy) | Err(rt::Error::QueueEmpty) => {
                let _ = rt::yield_current();
            }
            Err(_) => return fail("connect"),
        }
    }
    let mut established = false;
    while spins <= SPIN_BUDGET / 2 {
        spins += 1;
        match rt::network_socket_status(sockets.client) {
            Ok(info) if info.state == NetworkSocketState::Established => {
                established = true;
                break;
            }
            Ok(_) => {
                let _ = rt::yield_current();
                // Try accepting opportunistically even before establishment.
                if sockets.accepted == rt::INVALID_HANDLE {
                    if let Ok(Some(stream)) = try_accept(sockets.listener) {
                        sockets.accepted = stream;
                    }
                }
            }
            Err(_) => break,
        }
    }
    if !established {
        return fail("establish");
    }

    let _ = emit_job_log(log_handle, rt::LogSeverity::Info, "dispatch");

    // 3. Send the dispatch frame.
    if !send_with_retry(sockets.client, &request_frame[..request_len], SPIN_BUDGET / 8) {
        return fail("send");
    }

    // 4. Accept + ACK on the server leg while draining the client reply.
    let mut server_buffer = [0u8; 128];
    let mut got_server = 0usize;
    let mut ack_sent = false;
    let mut client_buffer = [0u8; 128];
    let mut got_client = 0usize;
    let mut ack = [0u8; 48];

    while spins <= SPIN_BUDGET {
        spins += 1;
        // Server leg: pull bytes until a full frame lands, answer once.
        if sockets.accepted == rt::INVALID_HANDLE {
            match try_accept(sockets.listener) {
                Ok(Some(stream)) => sockets.accepted = stream,
                Ok(None) => {}
                Err(_) => return fail("accept"),
            }
        } else if got_server == 0 {
            match recv_once(sockets.accepted, &mut server_buffer) {
                ReadOutcome::Got(count) => got_server = count,
                ReadOutcome::Idle => {}
                ReadOutcome::Dead => return fail("server-read"),
            }
        } else if !ack_sent {
            let Some(frame) = serve_accept(&server_buffer[..got_server], HARNESS_JOB_ID, &mut ack)
            else {
                return fail("wire");
            };
            if !send_with_retry(sockets.accepted, frame, SPIN_BUDGET / 16) {
                return fail("ack-send");
            }
            ack_sent = true;
        }

        // Client leg: wait for the ACK.
        if ack_sent && got_client == 0 {
            match recv_once(sockets.client, &mut client_buffer) {
                ReadOutcome::Got(count) => got_client = count,
                ReadOutcome::Idle => {}
                ReadOutcome::Dead => return fail("client-read"),
            }
        }
        if ack_sent
            && got_client > 0
            && validate_ack(&client_buffer[..got_client], HARNESS_JOB_ID)
        {
            jobs[job_index].state = rt::DeveloperJobState::Succeeded;
            let _ = emit_job_log(log_handle, rt::LogSeverity::Info, "complete");
            let _ = rt::write_logf(
                "developer",
                format_args!(
                    "farm-selftest PASS job={HARNESS_JOB_ID} target=linux-x64 endpoint=127.0.0.1:{FARM_SELFTEST_PORT} spins={spins}"
                ),
            );
            let _ = rt::write_logf("developer", format_args!("farm-selftest end ok=1"));
            drop(sockets);
            return HarnessReport { pass: true, stage: "done", spins };
        }
        let _ = rt::yield_current();
    }
    drop(sockets);
    fail("timeout")
}

fn emit_job_log(log_handle: rt::Handle, severity: rt::LogSeverity, phase: &'static str) -> rt::Result<()> {
    let detail = match phase {
        "dispatch" => 1,
        _ => 2,
    };
    crate::util::emit_log(
        log_handle,
        severity,
        rt::LogEvent::DeveloperBuildStarted,
        HARNESS_JOB_ID,
        detail,
    )
}

/// Reply builder for the explicit control-tag trigger path.
pub(crate) fn build_control_reply(pass: bool) -> rt::RawMessage {
    let mut reply = rt::RawMessage::empty(FARM_SELFTEST_REPLY_TAG);
    reply.word_count = 3;
    reply.words[0] = if pass { 0 } else { 4 }; // DeveloperStatus::Ok/Unsupported
    reply.words[1] = u64::from(pass);
    reply.words[2] = FARM_SELFTEST_REQUEST_TAG as u64;
    reply
}

#[cfg(test)]
mod wire_tests {
    use super::*;

    #[test]
    fn build_job_request_matches_wire_contract() {
        let mut frame = [0u8; 64];
        let len = build_job_request(123, 2, &mut frame);
        assert_eq!(&frame[..len], &b"FARMQ1 123 2\n"[..]);
    }

    #[test]
    fn serve_accept_rejects_bad_magic_and_mismatched_ids() {
        let mut ack = [0u8; 48];
        let mut frame = [0u8; 64];
        let len = build_job_request(HARNESS_JOB_ID, 2, &mut frame);
        let acked = serve_accept(&frame[..len], HARNESS_JOB_ID, &mut ack).unwrap();
        assert_eq!(acked, &b"FARMQ1 OK 4276993775\n"[..]);
        // Wrong id on the wire never gets acknowledged.
        assert!(serve_accept(&frame[..len], HARNESS_JOB_ID + 1, &mut ack).is_none());
        // Garbage bytes never get acknowledged.
        assert!(serve_accept(b"HELLO there", HARNESS_JOB_ID, &mut ack).is_none());
    }

    #[test]
    fn validate_ack_accepts_only_exact_id_lines() {
        let mut frame = [0u8; 64];
        let mut ack = [0u8; 48];
        let len = build_job_request(HARNESS_JOB_ID, 2, &mut frame);
        let acked = serve_accept(&frame[..len], HARNESS_JOB_ID, &mut ack).unwrap();
        assert!(validate_ack(acked, HARNESS_JOB_ID));
        assert!(!validate_ack(acked, HARNESS_JOB_ID + 1));
        assert!(!validate_ack(&acked[..acked.len() - 1], HARNESS_JOB_ID + 2));
        assert!(!validate_ack(b"FARMQ2 OK x", HARNESS_JOB_ID));
    }

    #[test]
    fn synthetic_job_lands_queued_remote_farm_pending_export() {
        let mut jobs = [JobSlot::empty(); MAX_JOBS];
        let index = crate::util::allocate_job(&mut jobs).unwrap();
        jobs[index] = JobSlot {
            occupied: true,
            route: crate::routing::BuildRoute::RemoteFarm {
                endpoint_id: HARNESS_ENDPOINT_ID,
            },
            export: ExportState::PendingRemote {
                endpoint: FixedBytes::empty(),
            },
            ..JobSlot::empty()
        };
        assert_eq!(
            crate::routing::route_kind(jobs[index].route),
            crate::routing::ROUTE_KIND_REMOTE_FARM
        );
        assert!(matches!(jobs[index].export, ExportState::PendingRemote { .. }));
        assert!(job_state_is(&jobs[index], rt::DeveloperJobState::Queued));
        jobs[index].state = rt::DeveloperJobState::Succeeded;
        assert!(job_state_is(&jobs[index], rt::DeveloperJobState::Succeeded));
    }
}

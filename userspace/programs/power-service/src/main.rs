//! Power service: power policy (SLEEP_INHIBIT/ALLOW flags over a
//! refcounted inhibit registry), suspend groundwork (prepare-for-suspend
//! broadcast contract stub to registered listeners), and battery/thermal/
//! device health reporting v0.
//!
//! Activation (manual, not in the default boot graph): the image is built
//! into the boot store as `services/power-service/program.img` and spawned
//! on demand via the manager's stored-image launch path. The service is NOT
//! registered under a named `ServiceId`, mirroring account-service and
//! backup-service.
//!
//! Startup handle convention: none required. The service owns no persistent
//! state and needs no storage channel; it keeps an idle-only loop, sampling
//! the monotonic tick counter periodically for its health snapshot.
//!
//! Honest hardware note: userspace has no port-IO or physical-memory access
//! path today, so both battery probes (ACPI DSDT walk, PM-port status
//! sample) report their graceful absence states at runtime. The probe logic
//! itself is pure and host-tested; a kernel ACPI/fw-cfg snapshot contract
//! can light it up later without ABI changes to this service's tags.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

mod protocol;

use rt::{ControlTag, LifecycleEvent, RawMessage};
use serviceos_power_service::{BatteryReport, format_status_text, power_tag};

use crate::protocol::{PowerServiceState, RequestScratch, handle_request};

use serviceos_userspace_runtime as rt;

const HEALTH_SAMPLE_YIELDS: u64 = 4096;
const STATUS_TEXT_BYTES: usize = 256;
const EXIT_OK: u64 = 0;
const EXIT_STARTUP: u64 = 0xfc01;
const EXIT_LOOP: u64 = 0xfc02;

#[cfg(not(test))]
rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return EXIT_STARTUP;
    }
    if startup.tag != ControlTag::Startup as u32 {
        return EXIT_STARTUP;
    }

    // Probes run against whatever transports exist; none do in userspace
    // yet, so the report starts at the honest absence state.
    let mut state = PowerServiceState::new(BatteryReport::unavailable());
    log_probe_state(&state);
    state.sample_health(rt::monotonic_now().unwrap_or(0));

    // Public control channel; handed to clients by whoever spawns us.
    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return EXIT_STARTUP,
    };
    let _ = public.second;

    let mut yields_since_sample: u64 = 0;
    loop {
        if lifecycle_stop_requested(bootstrap) {
            let _ = rt::handle_close(public.first);
            return EXIT_OK;
        }

        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                serve(&mut state, &request);
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return EXIT_LOOP,
        }

        yields_since_sample += 1;
        if yields_since_sample >= HEALTH_SAMPLE_YIELDS {
            yields_since_sample = 0;
            state.sample_health(rt::monotonic_now().unwrap_or(0));
        }

        if rt::yield_current().is_err() {
            return EXIT_LOOP;
        }
    }
}

fn serve(state: &mut PowerServiceState, request: &RawMessage) {
    let reply_to = request.handles[0];
    let mut prepared = *request;

    // Listener registration receives a duplicated handle from the caller in
    // words[1]; when absent and a reply handle exists, duplicate it here so
    // plain requests still work and the service owns its listener copies.
    if request.tag == power_tag::LISTENER_ADD_REQUEST && request.words.get(1) == Some(&0) {
        if reply_to != 0 {
            if let Ok(dup) = rt::handle_duplicate(reply_to, 0) {
                prepared.words[1] = dup as u64;
                prepared.word_count = prepared.word_count.max(2);
            }
        }
    }

    let mut response = RawMessage::empty(0);
    let mut scratch = RequestScratch::new();
    let plan = handle_request(state, &prepared, &mut response, &mut scratch);

    if response.tag != 0 && reply_to != 0 {
        let _ = rt::channel_send(reply_to, &response);
    }
    if request.tag == power_tag::LISTENER_REMOVE_REQUEST && response.tag != 0 {
        if let Some(&handle) = response.words.get(1) {
            if handle != 0 {
                let _ = rt::handle_close(handle as rt::Handle);
            }
        }
    }

    if let Some(plan) = plan {
        broadcast_prepare(state, plan.sequence);
    }
}

/// Deliver one prepare-for-suspend event per registered listener; listeners
/// whose channel rejects the send are unregistered and their duplicated
/// handles closed. Contract stub only — nothing sleeps.
fn broadcast_prepare(state: &mut PowerServiceState, sequence: u64) {
    let slots = state.listeners.slots();
    for slot in slots.into_iter().flatten() {
        let mut event = RawMessage::empty(power_tag::SUSPEND_PREPARE_EVENT);
        event.word_count = 1;
        event.words[0] = sequence;
        if rt::channel_send(slot.handle as rt::Handle, &event).is_err() {
            if let Ok(handle) = state.listeners.remove(slot.cookie) {
                if handle != 0 {
                    let _ = rt::handle_close(handle as rt::Handle);
                }
            }
        }
    }
}

fn log_probe_state(state: &PowerServiceState) {
    let mut text = [0u8; STATUS_TEXT_BYTES];
    let health = serviceos_power_service::health_snapshot(None, 0);
    let written =
        format_status_text(&state.policy, &state.battery, &health, &mut text).unwrap_or(0);
    let _ = rt::debug_log(&text[..written]);
}

fn lifecycle_stop_requested(bootstrap: rt::Handle) -> bool {
    let mut lifecycle = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut lifecycle) {
        Ok(()) => {
            lifecycle.tag == ControlTag::Lifecycle as u32
                && lifecycle.word_count >= 1
                && lifecycle.words[0] == LifecycleEvent::Stopped as u32 as u64
        }
        Err(rt::Error::QueueEmpty) => false,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serviceos_power_service::{
        HealthSnapshot, ListenerTable, MAX_LISTENERS, OWNER_WORDS, PowerPolicy, Presence,
        ProbeEvidence, SleepState, acpi_battery_report,
    };

    fn request(tag: u32, words: &[u64]) -> RawMessage {
        let mut message = RawMessage::empty(tag);
        message.word_count = words.len() as u32;
        message.words[..words.len()].copy_from_slice(words);
        message
    }

    fn handle_request_simple(
        state: &mut PowerServiceState,
        tag: u32,
        words: &[u64],
    ) -> (RawMessage, Option<serviceos_power_service::BroadcastPlan>) {
        let mut response = RawMessage::empty(0);
        let mut scratch = RequestScratch::new();
        let plan = handle_request(state, &request(tag, words), &mut response, &mut scratch);
        (response, plan)
    }

    #[test]
    fn status_reflects_policy_battery_and_health() {
        let mut state = PowerServiceState::new(BatteryReport::unavailable());
        let cookie = state.policy.acquire([0; OWNER_WORDS]).expect("acquire");
        let _ = cookie;
        state.sample_health(1234);
        let (response, plan) = handle_request_simple(&mut state, power_tag::STATUS_REQUEST, &[]);
        assert!(plan.is_none());
        assert_eq!(response.tag, power_tag::STATUS_REPLY);
        assert_eq!(response.words[0], 0);
        assert_eq!(response.words[1], SleepState::Inhibited as u64);
        assert_eq!(response.words[2], 1);
        assert_eq!(response.words[3], ProbeEvidence::NotAvailable as u64);
        assert_eq!(response.words[4], Presence::Unknown as u64);
        assert_eq!(response.words[5], 1234);
    }

    #[test]
    fn inhibit_refcount_gates_sleep_state() {
        let mut state = PowerServiceState::new(BatteryReport::unavailable());
        let (first, _) =
            handle_request_simple(&mut state, power_tag::INHIBIT_ACQUIRE_REQUEST, &[4, 0]);
        let (second, _) =
            handle_request_simple(&mut state, power_tag::INHIBIT_ACQUIRE_REQUEST, &[5, 0]);
        assert_eq!(first.words[0], 0);
        assert_eq!(second.words[0], 0);
        assert_eq!(state.policy.sleep_state(), SleepState::Inhibited);

        // Refcount semantics: releasing one inhibitor keeps the gate shut.
        let (reply, _) = handle_request_simple(
            &mut state,
            power_tag::INHIBIT_RELEASE_REQUEST,
            &[first.words[1]],
        );
        assert_eq!(reply.words[0], 0);
        assert_eq!(state.policy.sleep_state(), SleepState::Inhibited);

        let (reply, _) = handle_request_simple(
            &mut state,
            power_tag::INHIBIT_RELEASE_REQUEST,
            &[second.words[1]],
        );
        assert_eq!(reply.words[0], 0);
        assert_eq!(state.policy.sleep_state(), SleepState::Allow);

        let (reply, _) = handle_request_simple(
            &mut state,
            power_tag::INHIBIT_RELEASE_REQUEST,
            &[second.words[1]],
        );
        assert_eq!(
            reply.words[0],
            serviceos_power_service::PowerError::UnknownCookie.to_code() as u64
        );
    }

    #[test]
    fn inhibit_capacity_and_owner_roundtrip() {
        let mut policy = PowerPolicy::new();
        let mut cookies = [0u64; serviceos_power_service::MAX_INHIBITS];
        for index in 0..cookies.len() {
            cookies[index] = policy.acquire([index as u64 + 1, 0]).expect("slot");
        }
        assert_eq!(
            policy.acquire([9, 9]),
            Err(serviceos_power_service::PowerError::CapacityExceeded)
        );
        for cookie in cookies {
            assert_eq!(policy.release(cookie), Ok(()));
        }
        assert_eq!(policy.sleep_state(), SleepState::Allow);
    }

    #[test]
    fn listener_table_add_remove_roundtrip_and_capacity() {
        let mut table = ListenerTable::new();
        let mut cookies = [0u64; MAX_LISTENERS];
        for (index, slot) in cookies.iter_mut().enumerate() {
            *slot = table.add(index as u64 + 10).expect("slot");
        }
        assert_eq!(
            table.add(99),
            Err(serviceos_power_service::PowerError::CapacityExceeded)
        );
        assert_eq!(table.remove(cookies[2]), Ok(12));
        // Cookies stay monotonic even after a slot frees up.
        assert_eq!(table.add(99), Ok(5));
        assert_eq!(
            table.remove(12345),
            Err(serviceos_power_service::PowerError::UnknownListener)
        );
    }

    #[test]
    fn suspend_prepare_broadcasts_plan_with_sequence_stub() {
        let mut state = PowerServiceState::new(BatteryReport::unavailable());
        let first_cookie = state.listeners.add(11).expect("listener");
        let _second = state.listeners.add(22).expect("listener");
        state.listeners.remove(first_cookie).expect("remove");

        let (response, plan) =
            handle_request_simple(&mut state, power_tag::SUSPEND_PREPARE_REQUEST, &[]);
        assert_eq!(response.tag, power_tag::SUSPEND_PREPARE_REPLY);
        assert_eq!(response.words[0], 0);
        let plan = plan.expect("broadcast plan");
        assert_eq!(plan.count, 1);
        assert_eq!(plan.targets[0].map(|slot| slot.handle), Some(22));
        assert_eq!(plan.sequence, 1);

        let (_, second_plan) =
            handle_request_simple(&mut state, power_tag::SUSPEND_PREPARE_REQUEST, &[]);
        assert_eq!(second_plan.expect("plan").sequence, 2);

        // Dry-run contract: no sleep happens, listeners stay registered.
        assert_eq!(
            state.listeners.slots()[1].map(|slot| slot.cookie),
            Some(_second)
        );
    }

    #[test]
    fn health_snapshot_sampling_tracks_delta_and_flags() {
        let mut state = PowerServiceState::new(BatteryReport::unavailable());
        let first = state.sample_health(500);
        assert_eq!(first.tick_delta, 500);
        assert_eq!(first.flags(), 0);
        let second = state.sample_health(1500);
        assert_eq!(second.prev_ticks, Some(500));
        assert_eq!(second.tick_delta, 1000);
        assert_eq!(second.flags(), 1);
        // Drift vs wall clock and memory pressure stay honestly unavailable.
        assert_eq!(second.drift_estimate_ppm, None);
        assert_eq!(second.memory_pressure_percent, None);

        let (response, _) =
            handle_request_simple(&mut state, power_tag::HEALTH_SNAPSHOT_REQUEST, &[2000]);
        assert_eq!(response.tag, power_tag::HEALTH_SNAPSHOT_REPLY);
        assert_eq!(response.words[0], 0);
        assert_eq!(response.words[1], 2000);
        assert_eq!(response.words[2], 500);
        assert_eq!(response.words[3], 1);
    }

    #[test]
    fn status_text_reports_honest_absence() {
        let mut state = PowerServiceState::new(BatteryReport::unavailable());
        state.policy.acquire([0; OWNER_WORDS]).expect("acquire");
        state.sample_health(42_000);
        let mut text = [0u8; STATUS_TEXT_BYTES];
        let written = format_status_text(
            &state.policy,
            &state.battery,
            &HealthSnapshot {
                now_ticks: state.last_health.now_ticks,
                prev_ticks: None,
                tick_delta: state.last_health.tick_delta,
                drift_estimate_ppm: None,
                memory_pressure_percent: None,
            },
            &mut text,
        )
        .expect("fits");
        let rendered = str::from_utf8(&text[..written]).expect("ascii");
        assert!(rendered.contains("power: state=inhibited inhibits=1"));
        assert!(rendered.contains("presence=0 detail=1"));
        assert!(rendered.contains("uptime-ticks=42000"));
        assert!(rendered.contains("drift=unavailable mem-pressure=unavailable"));

        let mut tiny = [0u8; 8];
        assert_eq!(
            format_status_text(&state.policy, &state.battery, &state.last_health, &mut tiny),
            None
        );
    }

    #[test]
    fn unknown_tags_are_ignored_without_reply() {
        let mut state = PowerServiceState::new(BatteryReport::unavailable());
        let (response, plan) = handle_request_simple(&mut state, 0x7fff, &[]);
        assert_eq!(response.tag, 0);
        assert!(plan.is_none());
        assert_eq!(acpi_battery_report(None), BatteryReport::unavailable());
    }
}

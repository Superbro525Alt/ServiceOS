//! Wire protocol for the power service's own control channel. Requests
//! carry a reply channel as handles[0]; replies are status-first
//! (`PowerError::to_code`, 0 = Ok) followed by op-specific words.

use serviceos_power_service::{
    BroadcastPlan, BatteryReport, HealthSnapshot, ListenerTable, OWNER_WORDS, PowerError,
    PowerPolicy, SleepState, health_snapshot, next_event_sequence, pack_words, power_tag,
    unpack_words,
};
use serviceos_userspace_runtime::RawMessage;

/// Combined mutable state the request handler drives. `battery` is fixed at
/// startup from whatever probes were possible; everything else mutates.
pub struct PowerServiceState {
    pub policy: PowerPolicy,
    pub listeners: ListenerTable,
    pub battery: BatteryReport,
    pub last_health: HealthSnapshot,
    pub event_sequence: u64,
}

impl PowerServiceState {
    pub fn new(battery: BatteryReport) -> Self {
        Self {
            policy: PowerPolicy::new(),
            listeners: ListenerTable::new(),
            battery,
            last_health: health_snapshot(None, 0),
            event_sequence: 0,
        }
    }

    /// Advance to a fresh health sample; returns the new snapshot.
    pub fn sample_health(&mut self, now_ticks: u64) -> HealthSnapshot {
        let prev = if self.last_health.now_ticks == 0 {
            None
        } else {
            Some(self.last_health.now_ticks)
        };
        let snapshot = health_snapshot(prev, now_ticks);
        self.last_health = snapshot;
        snapshot
    }
}

pub struct RequestScratch {
    pub owner: [u8; OWNER_WORDS * 8],
}

impl RequestScratch {
    pub fn new() -> Self {
        Self {
            owner: [0; OWNER_WORDS * 8],
        }
    }
}

impl Default for RequestScratch {
    fn default() -> Self {
        Self::new()
    }
}

fn fail(response: &mut RawMessage, error: PowerError) {
    response.word_count = 1;
    response.words[0] = error.to_code() as u64;
}

/// Handle one control request. Returns `Some(plan)` when a prepare-for-
/// suspend broadcast must be delivered by the caller (it owns channel send).
pub fn handle_request(
    state: &mut PowerServiceState,
    request: &RawMessage,
    response: &mut RawMessage,
    scratch: &mut RequestScratch,
) -> Option<BroadcastPlan> {
    match request.tag {
        x if x == power_tag::STATUS_REQUEST => {
            response.tag = power_tag::STATUS_REPLY;
            response.word_count = 6;
            response.words[0] = 0;
            response.words[1] = match state.policy.sleep_state() {
                SleepState::Allow => 0,
                SleepState::Inhibited => 1,
            };
            response.words[2] = state.policy.inhibit_count() as u64;
            response.words[3] = state.battery.evidence as u64;
            response.words[4] = state.battery.presence as u64;
            response.words[5] = state.last_health.now_ticks;
            None
        }
        x if x == power_tag::INHIBIT_ACQUIRE_REQUEST => {
            response.tag = power_tag::INHIBIT_ACQUIRE_REPLY;
            let owner_len = *request.words.first().unwrap_or(&0) as usize;
            let mut owner = [0u64; OWNER_WORDS];
            if owner_len > 0 {
                if unpack_words(
                    &request.words[1..],
                    owner_len.min(OWNER_WORDS * 8),
                    &mut scratch.owner,
                )
                .is_err()
                {
                    fail(response, PowerError::InvalidArgument);
                    return None;
                }
                pack_words(&mut owner, &scratch.owner);
            }
            match state.policy.acquire(owner) {
                Ok(cookie) => {
                    response.word_count = 2;
                    response.words[0] = 0;
                    response.words[1] = cookie;
                    None
                }
                Err(error) => {
                    fail(response, error);
                    None
                }
            }
        }
        x if x == power_tag::INHIBIT_RELEASE_REQUEST => {
            response.tag = power_tag::INHIBIT_RELEASE_REPLY;
            let Some(&cookie) = request.words.first() else {
                fail(response, PowerError::InvalidArgument);
                return None;
            };
            match state.policy.release(cookie) {
                Ok(()) => {
                    response.word_count = 1;
                    response.words[0] = 0;
                    None
                }
                Err(error) => {
                    fail(response, error);
                    None
                }
            }
        }
        x if x == power_tag::LISTENER_ADD_REQUEST => {
            response.tag = power_tag::LISTENER_ADD_REPLY;
            // The caller duplicates handles[0] and passes the duplicate in
            // words[0]; the service keeps only duplicated handles.
            let Some(&handle) = request.words.get(1) else {
                fail(response, PowerError::InvalidArgument);
                return None;
            };
            match state.listeners.add(handle) {
                Ok(cookie) => {
                    response.word_count = 2;
                    response.words[0] = 0;
                    response.words[1] = cookie;
                    None
                }
                Err(error) => {
                    fail(response, error);
                    None
                }
            }
        }
        x if x == power_tag::LISTENER_REMOVE_REQUEST => {
            response.tag = power_tag::LISTENER_REMOVE_REPLY;
            let Some(&cookie) = request.words.first() else {
                fail(response, PowerError::InvalidArgument);
                return None;
            };
            match state.listeners.remove(cookie) {
                Ok(handle) => {
                    response.word_count = 2;
                    response.words[0] = 0;
                    response.words[1] = handle;
                    None
                }
                Err(error) => {
                    fail(response, error);
                    None
                }
            }
        }
        x if x == power_tag::SUSPEND_PREPARE_REQUEST => {
            response.tag = power_tag::SUSPEND_PREPARE_REPLY;
            // Dry-run broadcast only: S3 entry is not implemented (not
            // reliably exercisable under QEMU TCG), so this never sleeps.
            state.event_sequence = next_event_sequence(state.event_sequence);
            let plan = state.listeners.plan_broadcast(state.event_sequence);
            response.word_count = 4;
            response.words[0] = 0;
            response.words[1] = plan.count as u64;
            response.words[2] = plan.sequence;
            response.words[3] = 0;
            return Some(plan);
        }
        x if x == power_tag::HEALTH_SNAPSHOT_REQUEST => {
            response.tag = power_tag::HEALTH_SNAPSHOT_REPLY;
            let now = *request.words.first().unwrap_or(&state.last_health.now_ticks);
            let snapshot = state.sample_health(now);
            response.word_count = 4;
            response.words[0] = 0;
            response.words[1] = snapshot.now_ticks;
            response.words[2] = snapshot.tick_delta;
            response.words[3] = snapshot.flags();
            None
        }
        _ => None,
    }
}

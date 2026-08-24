use rt::{RawMessage, RuntimeEnvState, RuntimeKind, RuntimeStatus, RuntimeTag};
use serviceos_userspace_runtime as rt;

use crate::consts::MAX_RUNTIMES;

/// BuildRequest word[2] runtime profile tag: 0 means "no runtime requested".
pub(crate) const RUNTIME_PROFILE_NONE: u32 = 0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BuildRoute {
    DirectSpawn,
    RuntimeEnv { env_id: u32 },
}

/// Wire encoding carried to the worker: nonzero = routed through the
/// runtime-service environment (env_id + 1), zero = direct spawn.
pub(crate) fn encode_route_word(route: BuildRoute) -> u64 {
    match route {
        BuildRoute::DirectSpawn => 0,
        BuildRoute::RuntimeEnv { env_id } => u64::from(env_id.wrapping_add(1)),
    }
}

/// Minimal snapshot of a runtime-service environment for the decision fn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeEnvSnapshot {
    pub(crate) env_id: u32,
    pub(crate) kind: u32,
    pub(crate) state: u32,
}

impl RuntimeEnvSnapshot {
    pub(crate) fn new(env_id: u32, kind: RuntimeKind, state: RuntimeEnvState) -> Self {
        Self {
            env_id,
            kind: kind as u32,
            state: state as u32,
        }
    }
}

/// Routing decision: a build/run job tagged with a runtime profile routes
/// through the matching active runtime environment; anything else falls back
/// to the existing direct worker spawn path.
pub(crate) fn route_for(profile: u32, envs: &[RuntimeEnvSnapshot]) -> BuildRoute {
    if profile == RUNTIME_PROFILE_NONE {
        return BuildRoute::DirectSpawn;
    }
    let mut best: Option<RuntimeEnvSnapshot> = None;
    for env in envs {
        if env.kind != profile || env.state != RuntimeEnvState::Ready as u32 {
            continue;
        }
        best = match best {
            Some(current) if current.env_id <= env.env_id => Some(current),
            _ => Some(*env),
        };
    }
    match best {
        Some(env) => BuildRoute::RuntimeEnv {
            env_id: env.env_id,
        },
        None => BuildRoute::DirectSpawn,
    }
}

/// Probe the live runtime-service environment table. Returns None when the
/// runtime service is unavailable or its contract answers unexpectedly; the
/// caller treats that as the direct-spawn fallback.
pub(crate) fn probe_runtime_envs(bootstrap: rt::Handle) -> Option<[RuntimeEnvSnapshot; MAX_RUNTIMES]> {
    let runtime_handle = rt::lookup_service(bootstrap, rt::ServiceId::Runtime).ok()?;
    let result = list_envs(runtime_handle);
    let _ = rt::handle_close(runtime_handle);
    result
}

fn list_envs(runtime_handle: rt::Handle) -> Option<[RuntimeEnvSnapshot; MAX_RUNTIMES]> {
    let mut envs = [RuntimeEnvSnapshot {
        env_id: 0,
        kind: 0,
        state: 0,
    }; MAX_RUNTIMES];
    let mut filled = 0usize;
    let mut start = 0usize;
    loop {
        let reply = rt::channel_create().ok()?;
        let mut request = RawMessage::empty(RuntimeTag::EnvListRequest as u32);
        request.word_count = 1;
        request.words[0] = start as u64;
        request.handle_count = 1;
        request.handles[0] = reply.second;
        request.handle_rights[0] = rt::rights::SEND;
        rt::channel_send(runtime_handle, &request).ok()?;
        let _ = rt::handle_close(reply.second);

        let mut response = RawMessage::empty(0);
        rt::channel_receive_blocking(reply.first, &mut response).ok()?;
        let _ = rt::handle_close(reply.first);
        if response.tag != RuntimeTag::EnvListReply as u32 || response.word_count < 3 {
            return None;
        }
        if response.words[0] != RuntimeStatus::Ok as u32 as u64 {
            return None;
        }
        let count = response.words[1] as usize;
        let next = response.words[2] as usize;
        if filled + count > MAX_RUNTIMES || response.word_count as usize != 3 + count * 6 {
            return None;
        }
        for page_index in 0..count {
            let base = 3 + page_index * 6;
            let kind = match response.words[base + 1] as u32 {
                x if x == RuntimeKind::Posix as u32 => RuntimeKind::Posix,
                x if x == RuntimeKind::Windows as u32 => RuntimeKind::Windows,
                _ => continue,
            };
            let state = match response.words[base + 2] as u32 {
                x if x == RuntimeEnvState::Ready as u32 => RuntimeEnvState::Ready,
                x if x == RuntimeEnvState::Busy as u32 => RuntimeEnvState::Busy,
                x if x == RuntimeEnvState::Destroyed as u32 => RuntimeEnvState::Destroyed,
                x if x == RuntimeEnvState::PendingApproval as u32 => {
                    RuntimeEnvState::PendingApproval
                }
                x if x == RuntimeEnvState::Denied as u32 => RuntimeEnvState::Denied,
                _ => continue,
            };
            envs[filled] = RuntimeEnvSnapshot::new(response.words[base] as u32, kind, state);
            filled += 1;
        }
        if count == 0 || next <= start {
            return Some(envs);
        }
        start = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(env_id: u32, kind: RuntimeKind) -> RuntimeEnvSnapshot {
        RuntimeEnvSnapshot::new(env_id, kind, RuntimeEnvState::Ready)
    }

    #[test]
    fn no_profile_routes_direct() {
        let envs = [ready(0, RuntimeKind::Posix)];
        assert_eq!(route_for(RUNTIME_PROFILE_NONE, &envs), BuildRoute::DirectSpawn);
        assert_eq!(route_for(RUNTIME_PROFILE_NONE, &[]), BuildRoute::DirectSpawn);
    }

    #[test]
    fn matching_active_runtime_routes_to_env() {
        let envs = [ready(2, RuntimeKind::Posix)];
        assert_eq!(
            route_for(RuntimeKind::Posix as u32, &envs),
            BuildRoute::RuntimeEnv { env_id: 2 }
        );
    }

    #[test]
    fn inactive_environments_do_not_match() {
        let envs = [
            RuntimeEnvSnapshot::new(1, RuntimeKind::Posix, RuntimeEnvState::Busy),
            RuntimeEnvSnapshot::new(3, RuntimeKind::Posix, RuntimeEnvState::Denied),
            RuntimeEnvSnapshot::new(4, RuntimeKind::Posix, RuntimeEnvState::Destroyed),
        ];
        assert_eq!(route_for(RuntimeKind::Posix as u32, &envs), BuildRoute::DirectSpawn);
    }

    #[test]
    fn kind_mismatch_falls_back_direct() {
        let envs = [ready(1, RuntimeKind::Posix)];
        assert_eq!(route_for(RuntimeKind::Windows as u32, &envs), BuildRoute::DirectSpawn);
    }

    #[test]
    fn picks_lowest_matching_env_id() {
        let envs = [
            ready(5, RuntimeKind::Posix),
            ready(2, RuntimeKind::Posix),
            ready(7, RuntimeKind::Posix),
        ];
        assert_eq!(
            route_for(RuntimeKind::Posix as u32, &envs),
            BuildRoute::RuntimeEnv { env_id: 2 }
        );
    }

    #[test]
    fn unavailable_probe_maps_to_direct_spawn() {
        assert_eq!(probe_runtime_envs(0), None);
        assert_eq!(route_for(RuntimeKind::Posix as u32, &[]), BuildRoute::DirectSpawn);
    }

    #[test]
    fn route_word_encoding_round_trips() {
        assert_eq!(encode_route_word(BuildRoute::DirectSpawn), 0);
        assert_eq!(
            encode_route_word(BuildRoute::RuntimeEnv { env_id: 3 }),
            4
        );
    }
}

use rt::{RawMessage, RuntimeEnvState, RuntimeKind, RuntimeStatus, RuntimeTag};
use serviceos_userspace_runtime as rt;

use crate::consts::MAX_RUNTIMES;

/// BuildRequest word[2] runtime profile tag: 0 means "no runtime requested".
pub(crate) const RUNTIME_PROFILE_NONE: u32 = 0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BuildRoute {
    DirectSpawn,
    RuntimeEnv { env_id: u32 },
    RemoteFarm { endpoint_id: u32 },
}

/// Route-kind codes used by the IDE reply tails: direct spawn, runtime
/// environment, remote farm endpoint.
pub(crate) const ROUTE_KIND_DIRECT: u64 = 0;
pub(crate) const ROUTE_KIND_RUNTIME_ENV: u64 = 1;
pub(crate) const ROUTE_KIND_REMOTE_FARM: u64 = 2;

pub(crate) fn route_kind(route: BuildRoute) -> u64 {
    match route {
        BuildRoute::DirectSpawn => ROUTE_KIND_DIRECT,
        BuildRoute::RuntimeEnv { .. } => ROUTE_KIND_RUNTIME_ENV,
        BuildRoute::RemoteFarm { .. } => ROUTE_KIND_REMOTE_FARM,
    }
}

/// Wire encoding carried to the worker: nonzero = routed through the
/// runtime-service environment (env_id + 1), zero = direct spawn. Remote
/// farm routes never spawn a local worker; their reserved range starts at
/// 0x4000_0000 (endpoint_id + 1) so the encoding can never collide with an
/// environment id.
pub(crate) fn encode_route_word(route: BuildRoute) -> u64 {
    match route {
        BuildRoute::DirectSpawn => 0,
        BuildRoute::RuntimeEnv { env_id } => u64::from(env_id.wrapping_add(1)),
        BuildRoute::RemoteFarm { endpoint_id } => {
            0x4000_0000 | u64::from(endpoint_id.wrapping_add(1))
        }
    }
}

/// Minimal snapshot of a runtime-service environment for the decision fn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeEnvSnapshot {
    pub(crate) env_id: u32,
    pub(crate) kind: u32,
    pub(crate) state: u32,
    /// Granted capability bitmask from the env-list contract
    /// (`runtime_capability` bits: FILE_READ=bit0, TERMINAL_IO=bit1, ...).
    pub(crate) capabilities: u32,
}

impl RuntimeEnvSnapshot {
    pub(crate) fn new(env_id: u32, kind: RuntimeKind, state: RuntimeEnvState) -> Self {
        Self {
            env_id,
            kind: kind as u32,
            state: state as u32,
            capabilities: 0,
        }
    }

    pub(crate) fn with_capabilities(mut self, capabilities: u32) -> Self {
        self.capabilities = capabilities;
        self
    }

    pub(crate) fn grants_file_read(&self) -> bool {
        self.capabilities & rt::runtime_capability::FILE_READ != 0
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
        Some(env) => BuildRoute::RuntimeEnv { env_id: env.env_id },
        None => BuildRoute::DirectSpawn,
    }
}

/// Probe the live runtime-service environment table. Returns None when the
/// runtime service is unavailable or its contract answers unexpectedly; the
/// caller treats that as the direct-spawn fallback.
pub(crate) fn probe_runtime_envs(
    bootstrap: rt::Handle,
) -> Option<[RuntimeEnvSnapshot; MAX_RUNTIMES]> {
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
        capabilities: 0,
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
            envs[filled] = RuntimeEnvSnapshot::new(response.words[base] as u32, kind, state)
                .with_capabilities(response.words[base + 3] as u32);
            filled += 1;
        }
        if count == 0 || next <= start {
            return Some(envs);
        }
        start = next;
    }
}

/// How a job actually executed, recorded on the job record and echoed in
/// logs so the two spawn paths are always distinguishable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExecutionMode {
    /// Legacy path: worker launched straight from the boot-store image.
    DirectSpawn,
    /// Worker launch went through the runtime env's exec contract
    /// (RunLaunchRequest with the guest-exec workload marker).
    RoutedEnv { env_id: u32 },
    /// A routed launch was attempted but refused; the job fell back to the
    /// direct spawn. Reason codes: 1 = env lacks FILE_READ grant, 2 = exec
    /// request answered with an error status, 3 = intersected permission
    /// set has no usable scopes.
    RoutedFallback { env_id: u32, reason: u8 },
}

pub(crate) const FALLBACK_NO_FILE_GRANT: u8 = 1;
pub(crate) const FALLBACK_EXEC_REFUSED: u8 = 2;
pub(crate) const FALLBACK_NO_USABLE_SCOPES: u8 = 3;

impl ExecutionMode {
    /// Compact status word for logs and reply tails: bit layout is
    /// `mode(2 bits at 0..2) | env_id(16 bits at 8..24) | reason(8 at 24)`.
    pub(crate) fn status_word(self) -> u64 {
        match self {
            ExecutionMode::DirectSpawn => 0,
            ExecutionMode::RoutedEnv { env_id } => 1 | (u64::from(env_id & 0xFFFF) << 8),
            ExecutionMode::RoutedFallback { env_id, reason } => {
                2 | (u64::from(env_id & 0xFFFF) << 8) | (u64::from(reason) << 24)
            }
        }
    }

    pub(crate) fn routed(self) -> bool {
        matches!(self, ExecutionMode::RoutedEnv { .. })
    }
}

/// Mode-selection matrix (host-tested): decide how a job whose route points
/// at `env` runs. `scopes_after_intersect` is the fs-scope count that
/// survives the permission-set / env-grant intersection.
pub(crate) fn select_execution_mode(
    route: BuildRoute,
    env: Option<RuntimeEnvSnapshot>,
    scopes_after_intersect: usize,
) -> ExecutionMode {
    let BuildRoute::RuntimeEnv { env_id } = route else {
        return ExecutionMode::DirectSpawn;
    };
    let Some(env) = env.filter(|env| env.env_id == env_id) else {
        // Snapshot missing for the chosen env (probe raced): fall back.
        return ExecutionMode::RoutedFallback {
            env_id,
            reason: FALLBACK_EXEC_REFUSED,
        };
    };
    if !env.grants_file_read() {
        return ExecutionMode::RoutedFallback {
            env_id,
            reason: FALLBACK_NO_FILE_GRANT,
        };
    }
    if scopes_after_intersect == 0 {
        return ExecutionMode::RoutedFallback {
            env_id,
            reason: FALLBACK_NO_USABLE_SCOPES,
        };
    }
    ExecutionMode::RoutedEnv { env_id }
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
        assert_eq!(
            route_for(RUNTIME_PROFILE_NONE, &envs),
            BuildRoute::DirectSpawn
        );
        assert_eq!(
            route_for(RUNTIME_PROFILE_NONE, &[]),
            BuildRoute::DirectSpawn
        );
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
        assert_eq!(
            route_for(RuntimeKind::Posix as u32, &envs),
            BuildRoute::DirectSpawn
        );
    }

    #[test]
    fn kind_mismatch_falls_back_direct() {
        let envs = [ready(1, RuntimeKind::Posix)];
        assert_eq!(
            route_for(RuntimeKind::Windows as u32, &envs),
            BuildRoute::DirectSpawn
        );
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
        assert_eq!(
            route_for(RuntimeKind::Posix as u32, &[]),
            BuildRoute::DirectSpawn
        );
    }

    #[test]
    fn route_word_encoding_round_trips() {
        assert_eq!(encode_route_word(BuildRoute::DirectSpawn), 0);
        assert_eq!(encode_route_word(BuildRoute::RuntimeEnv { env_id: 3 }), 4);
    }

    #[test]
    fn farm_routes_use_reserved_encoding_range() {
        let word = encode_route_word(BuildRoute::RemoteFarm { endpoint_id: 0 });
        assert_eq!(word, 0x4000_0001);
        assert!(word >= 0x4000_0000);
        assert_ne!(
            word,
            encode_route_word(BuildRoute::RuntimeEnv { env_id: u32::MAX })
        );
    }

    #[test]
    fn route_kind_maps_each_variant() {
        assert_eq!(route_kind(BuildRoute::DirectSpawn), ROUTE_KIND_DIRECT);
        assert_eq!(
            route_kind(BuildRoute::RuntimeEnv { env_id: 9 }),
            ROUTE_KIND_RUNTIME_ENV
        );
        assert_eq!(
            route_kind(BuildRoute::RemoteFarm { endpoint_id: 2 }),
            ROUTE_KIND_REMOTE_FARM
        );
    }

    fn caps(env_id: u32, kind: RuntimeKind, capabilities: u32) -> RuntimeEnvSnapshot {
        ready(env_id, kind).with_capabilities(capabilities)
    }

    #[test]
    fn mode_matrix_direct_routes_stay_direct() {
        assert_eq!(
            select_execution_mode(BuildRoute::DirectSpawn, None, 0),
            ExecutionMode::DirectSpawn
        );
        let farm = BuildRoute::RemoteFarm { endpoint_id: 1 };
        assert_eq!(
            select_execution_mode(farm, Some(caps(2, RuntimeKind::Posix, 0x1)), 3),
            ExecutionMode::DirectSpawn
        );
    }

    #[test]
    fn mode_matrix_env_without_file_grant_falls_back() {
        let route = BuildRoute::RuntimeEnv { env_id: 4 };
        let env = caps(4, RuntimeKind::Posix, rt::runtime_capability::NETWORK);
        assert_eq!(
            select_execution_mode(route, Some(env), 2),
            ExecutionMode::RoutedFallback {
                env_id: 4,
                reason: FALLBACK_NO_FILE_GRANT
            }
        );
    }

    #[test]
    fn mode_matrix_missing_snapshot_falls_back() {
        let route = BuildRoute::RuntimeEnv { env_id: 4 };
        assert_eq!(
            select_execution_mode(route, None, 2),
            ExecutionMode::RoutedFallback {
                env_id: 4,
                reason: FALLBACK_EXEC_REFUSED
            }
        );
        // Snapshot for a different env id does not count.
        let other = caps(9, RuntimeKind::Posix, rt::runtime_capability::FILE_READ);
        assert!(matches!(
            select_execution_mode(route, Some(other), 2),
            ExecutionMode::RoutedFallback { .. }
        ));
    }

    #[test]
    fn mode_matrix_empty_intersection_falls_back() {
        let route = BuildRoute::RuntimeEnv { env_id: 4 };
        let env = caps(4, RuntimeKind::Posix, rt::runtime_capability::FILE_READ);
        assert_eq!(
            select_execution_mode(route, Some(env), 0),
            ExecutionMode::RoutedFallback {
                env_id: 4,
                reason: FALLBACK_NO_USABLE_SCOPES
            }
        );
    }

    #[test]
    fn mode_matrix_ready_granted_scoped_env_runs_routed() {
        let route = BuildRoute::RuntimeEnv { env_id: 6 };
        let env = caps(
            6,
            RuntimeKind::Posix,
            rt::runtime_capability::FILE_READ | rt::runtime_capability::TERMINAL_IO,
        );
        assert_eq!(
            select_execution_mode(route, Some(env), 2),
            ExecutionMode::RoutedEnv { env_id: 6 }
        );
    }

    #[test]
    fn execution_mode_status_words_distinguish_paths() {
        assert_eq!(ExecutionMode::DirectSpawn.status_word(), 0);
        assert_eq!(
            ExecutionMode::RoutedEnv { env_id: 3 }.status_word(),
            1 | (3 << 8)
        );
        assert_eq!(
            ExecutionMode::RoutedFallback {
                env_id: 3,
                reason: FALLBACK_NO_FILE_GRANT
            }
            .status_word(),
            2 | (3 << 8) | (u64::from(FALLBACK_NO_FILE_GRANT) << 24)
        );
        assert!(!ExecutionMode::DirectSpawn.routed());
        assert!(ExecutionMode::RoutedEnv { env_id: 1 }.routed());
        assert!(
            !ExecutionMode::RoutedFallback {
                env_id: 1,
                reason: 1
            }
            .routed()
        );
    }
}

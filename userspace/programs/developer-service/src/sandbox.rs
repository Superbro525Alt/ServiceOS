use serviceos_userspace_runtime as rt;

use crate::{
    consts::MAX_PATH,
    types::{FixedBytes, ToolchainSlot, WorkspaceSlot},
};

pub(crate) const MAX_SCOPES: usize = 4;
pub(crate) const SANDBOX_TEXT_MAX: usize = 512;

/// Builder-report status codes shared with `cross-builder-tool`:
/// 0 = ok, 1 = unsupported target, 2 = generic failure, 3 = sandbox denial.
pub(crate) const BUILDER_STATUS_SANDBOX_DENIED: u64 = 3;

#[derive(Clone, Copy)]
pub(crate) struct PermissionSet {
    pub(crate) scopes: [FixedBytes<MAX_PATH>; MAX_SCOPES],
    pub(crate) scope_count: usize,
    pub(crate) network_denied: bool,
}

impl PermissionSet {
    pub(crate) fn empty() -> Self {
        Self {
            scopes: [FixedBytes::empty(); MAX_SCOPES],
            scope_count: 0,
            network_denied: true,
        }
    }

    pub(crate) fn push_scope(&mut self, scope: &[u8]) -> rt::Result<()> {
        if self.scope_count >= MAX_SCOPES {
            return Err(rt::Error::CapacityExceeded);
        }
        self.scopes[self.scope_count].set(scope)?;
        self.scope_count += 1;
        Ok(())
    }

    pub(crate) fn allows_path(&self, path: &[u8]) -> bool {
        self.scopes[..self.scope_count]
            .iter()
            .any(|scope| scope_contains(scope.as_bytes(), path))
    }
}

/// Explicit allow/deny decision recorded alongside a build job.
#[derive(Clone, Copy)]
pub(crate) struct SandboxDecision {
    pub(crate) allowed: bool,
    pub(crate) scope_count: usize,
}

pub(crate) fn trim_trailing_slash(scope: &[u8]) -> &[u8] {
    let mut end = scope.len();
    while end > 1 && scope[end - 1] == b'/' {
        end -= 1;
    }
    &scope[..end]
}

/// Prefix containment on path-component boundaries only:
/// "ws/src" covers "ws/src/main.rs" and "ws/src" itself, never "ws/srcfile".
pub(crate) fn scope_contains(scope: &[u8], path: &[u8]) -> bool {
    let scope = trim_trailing_slash(scope);
    if scope.is_empty() || path.is_empty() {
        return false;
    }
    if !path.starts_with(scope) {
        return false;
    }
    path.len() == scope.len() || path[scope.len()] == b'/'
}

fn parent_directory(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|byte| *byte == b'/') {
        Some(index) => &path[..index],
        None => &[],
    }
}

/// Derive the job's permission set from the workspace descriptor (project
/// directory as read/write root) plus the selected toolchain SDK root.
/// Network access is always denied for build workers.
pub(crate) fn derive_permission_set(
    workspace: &WorkspaceSlot,
    toolchain: &ToolchainSlot,
) -> PermissionSet {
    let mut set = PermissionSet::empty();
    let root = parent_directory(workspace.source_path.as_bytes());
    if !root.is_empty() {
        let _ = set.push_scope(root);
    }
    if !toolchain.sdk_root.as_bytes().is_empty() {
        let _ = set.push_scope(toolchain.sdk_root.as_bytes());
    }
    set.network_denied = true;
    set
}

pub(crate) fn validate_job_paths(
    set: &PermissionSet,
    request_in: &[u8],
    request_out: &[u8],
) -> bool {
    set.allows_path(request_in) && set.allows_path(request_out)
}

/// Output location for the job's artifact: project root joined with the
/// workspace's artifact name.
pub(crate) fn workspace_output_path(workspace: &WorkspaceSlot) -> FixedBytes<MAX_PATH> {
    let mut out = FixedBytes::<MAX_PATH>::empty();
    let root = parent_directory(workspace.source_path.as_bytes());
    let name = workspace.artifact.as_bytes();
    if !root.is_empty() && root.len() + 1 + name.len() <= MAX_PATH {
        let mut bytes = [0u8; MAX_PATH];
        bytes[..root.len()].copy_from_slice(root);
        bytes[root.len()] = b'/';
        bytes[root.len() + 1..root.len() + 1 + name.len()].copy_from_slice(name);
        let _ = out.set(&bytes[..root.len() + 1 + name.len()]);
    }
    out
}

/// Serialize the permission set handed to the worker:
/// `fs=<scope>;<scope>` / `net=denied` / `in=<source>` / `out=<artifact>`.
pub(crate) fn serialize_permission_text(
    set: &PermissionSet,
    request_in: &[u8],
    request_out: &[u8],
    out: &mut [u8],
) -> rt::Result<usize> {
    if out.len() < SANDBOX_TEXT_MAX {
        return Err(rt::Error::BufferTooSmall);
    }
    let mut cursor = 0usize;
    cursor += write_chunk(&mut out[cursor..], b"fs=")?;
    for index in 0..set.scope_count {
        if index > 0 {
            cursor += write_chunk(&mut out[cursor..], b";")?;
        }
        cursor += write_chunk(&mut out[cursor..], set.scopes[index].as_bytes())?;
    }
    cursor += write_chunk(&mut out[cursor..], b"\n")?;
    cursor += write_chunk(
        &mut out[cursor..],
        if set.network_denied {
            b"net=denied\n" as &[u8]
        } else {
            b"net=allowed\n" as &[u8]
        },
    )?;
    cursor += write_chunk(&mut out[cursor..], b"in=")?;
    cursor += write_chunk(&mut out[cursor..], request_in)?;
    cursor += write_chunk(&mut out[cursor..], b"\n")?;
    cursor += write_chunk(&mut out[cursor..], b"out=")?;
    cursor += write_chunk(&mut out[cursor..], request_out)?;
    cursor += write_chunk(&mut out[cursor..], b"\n")?;
    Ok(cursor)
}

fn write_chunk(out: &mut [u8], bytes: &[u8]) -> rt::Result<usize> {
    if bytes.len() > out.len() {
        return Err(rt::Error::BufferTooSmall);
    }
    out[..bytes.len()].copy_from_slice(bytes);
    Ok(bytes.len())
}

pub(crate) fn decision_for(
    set: &PermissionSet,
    request_in: &[u8],
    request_out: &[u8],
) -> SandboxDecision {
    SandboxDecision {
        allowed: set.network_denied && validate_job_paths(set, request_in, request_out),
        scope_count: set.scope_count,
    }
}

/// Intersect the job's permission set with a runtime environment's granted
/// capabilities. Minimum-privilege rules:
/// - fs scopes survive only when the env grants FILE_READ (otherwise nothing
///   inside the env could read the workspace/SDK paths at all);
/// - network stays denied for build workers even when the env has the
///   NETWORK bit — an env grant never widens the job's permissions;
/// - request paths are re-validated by the caller against the result.
pub(crate) fn intersect_with_env(set: &PermissionSet, env_capabilities: u32) -> PermissionSet {
    let mut out = PermissionSet::empty();
    if env_capabilities & rt::runtime_capability::FILE_READ == 0 {
        return out;
    }
    for index in 0..set.scope_count {
        let _ = out.push_scope(set.scopes[index].as_bytes());
    }
    out.network_denied = true;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &[u8] = b"packages/developer-service/1.0.0/projects/hello-cross/message.txt";
    const PROJECT_ROOT: &[u8] = b"packages/developer-service/1.0.0/projects/hello-cross";
    const SDK_ROOT: &[u8] = b"packages/developer-service/1.0.0/sdk/linux";

    fn artifact_out_path() -> FixedBytes<MAX_PATH> {
        let mut slot = FixedBytes::<MAX_PATH>::empty();
        let mut bytes = [0u8; MAX_PATH];
        let mut len = 0usize;
        bytes[..PROJECT_ROOT.len()].copy_from_slice(PROJECT_ROOT);
        len += PROJECT_ROOT.len();
        bytes[len] = b'/';
        len += 1;
        bytes[len..len + b"hello-cross".len()].copy_from_slice(b"hello-cross");
        len += b"hello-cross".len();
        slot.set(&bytes[..len]).unwrap();
        slot
    }

    fn derived() -> PermissionSet {
        let mut workspace = WorkspaceSlot::empty();
        workspace.source_path.set(SOURCE).unwrap();
        let mut toolchain = ToolchainSlot::empty();
        toolchain.sdk_root.set(SDK_ROOT).unwrap();
        derive_permission_set(&workspace, &toolchain)
    }

    #[test]
    fn exact_scope_match_is_allowed() {
        assert!(scope_contains(PROJECT_ROOT, PROJECT_ROOT));
    }

    #[test]
    fn child_path_is_allowed() {
        assert!(scope_contains(PROJECT_ROOT, SOURCE));
    }

    #[test]
    fn sibling_prefix_collision_is_denied() {
        assert!(!scope_contains(b"ws/src", b"ws/srcfile"));
        assert!(!scope_contains(b"ws/src", b"ws/srcextra/main.rs"));
    }

    #[test]
    fn trailing_slash_scopes_are_normalized() {
        assert!(scope_contains(b"ws/src/", b"ws/src/main.rs"));
        assert!(scope_contains(b"ws/src///", b"ws/src/main.rs"));
    }

    #[test]
    fn empty_scope_or_path_is_denied() {
        assert!(!scope_contains(b"", b"ws/file"));
        assert!(!scope_contains(b"ws", b""));
    }

    #[test]
    fn unrelated_path_is_denied() {
        assert!(!scope_contains(
            PROJECT_ROOT,
            b"packages/other/project/file.txt"
        ));
    }

    #[test]
    fn parent_escape_is_denied() {
        assert!(!scope_contains(
            b"packages/developer-service/1.0.0/projects/hello-cross/deep",
            SOURCE
        ));
    }

    #[test]
    fn derived_set_has_two_scopes_and_allows_job_paths() {
        let set = derived();
        assert_eq!(set.scope_count, 2);
        assert!(set.network_denied);
        assert!(set.allows_path(SOURCE));
        let artifact_out = artifact_out_path();
        assert!(set.allows_path(artifact_out.as_bytes()));
        assert!(!set.allows_path(b"packages/elsewhere/x.txt"));
    }

    #[test]
    fn derived_set_rejects_when_sdk_missing_still_allows_source() {
        let mut workspace = WorkspaceSlot::empty();
        workspace.source_path.set(SOURCE).unwrap();
        let toolchain = ToolchainSlot::empty();
        let set = derive_permission_set(&workspace, &toolchain);
        assert_eq!(set.scope_count, 1);
        assert!(validate_job_paths(
            &set,
            SOURCE,
            b"packages/developer-service/1.0.0/projects/hello-cross/out.bin"
        ));
        assert!(!validate_job_paths(&set, b"outside/x", SOURCE));
    }

    #[test]
    fn serialize_emits_expected_lines() {
        let set = derived();
        let mut text = [0u8; SANDBOX_TEXT_MAX];
        let len = serialize_permission_text(&set, SOURCE, b"pkg/hello-cross/out.bin", &mut text)
            .expect("serialize");
        let rendered = core::str::from_utf8(&text[..len]).unwrap();
        assert_eq!(
            rendered,
            "fs=packages/developer-service/1.0.0/projects/hello-cross;\
             packages/developer-service/1.0.0/sdk/linux\nnet=denied\n\
             in=packages/developer-service/1.0.0/projects/hello-cross/message.txt\n\
             out=pkg/hello-cross/out.bin\n"
        );
    }

    #[test]
    fn serialize_rejects_oversize_output_buffer() {
        let set = derived();
        let mut tiny = [0u8; 16];
        assert!(serialize_permission_text(&set, SOURCE, b"x", &mut tiny).is_err());
    }

    #[test]
    fn decision_records_allow_with_scope_count() {
        let set = derived();
        let artifact_out = artifact_out_path();
        let decision = decision_for(&set, SOURCE, artifact_out.as_bytes());
        assert!(decision.allowed);
        assert_eq!(decision.scope_count, 2);

        let denied = decision_for(&set, b"elsewhere/x", artifact_out.as_bytes());
        assert!(!denied.allowed);
        assert_eq!(denied.scope_count, 2);
    }

    #[test]
    fn intersection_keeps_scopes_when_env_grants_file_read() {
        let set = derived();
        let caps = rt::runtime_capability::FILE_READ;
        let merged = intersect_with_env(&set, caps);
        assert_eq!(merged.scope_count, set.scope_count);
        assert!(merged.network_denied);
        assert!(merged.allows_path(SOURCE));
    }

    #[test]
    fn intersection_without_file_read_drops_everything() {
        let set = derived();
        for env_caps in [0u32, rt::runtime_capability::NETWORK | rt::runtime_capability::AUDIO] {
            let merged = intersect_with_env(&set, env_caps);
            assert_eq!(merged.scope_count, 0);
            assert!(merged.network_denied);
            assert!(!merged.allows_path(SOURCE));
            assert!(!validate_job_paths(&merged, SOURCE, SOURCE));
        }
    }

    #[test]
    fn intersection_never_grants_network_to_builds() {
        let mut set = PermissionSet::empty();
        let _ = set.push_scope(b"ws/src");
        // Env grants network + terminal + file: the build still runs with
        // fs-only, net-denied permissions.
        let env_caps = rt::runtime_capability::FILE_READ
            | rt::runtime_capability::NETWORK
            | rt::runtime_capability::TERMINAL_IO;
        let merged = intersect_with_env(&set, env_caps);
        assert_eq!(merged.scope_count, 1);
        assert!(merged.network_denied);
    }
}

use rt::ServiceId;
use serviceos_userspace_runtime as rt;

use crate::util::{ShellOutput, write_output_linef};

use super::onboard::{self, RepoAddPlan};
use super::parse::{
    MAX_PACKAGE_TEXT, channel_name, parse_channel, parse_repo_trust, parse_ring, parse_usize,
    repo_sync_state_name, ring_name, trust_mode_name,
};

pub(super) fn cmd_pkg_repos(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let mut name = [0u8; MAX_PACKAGE_TEXT];
    let mut url = [0u8; MAX_PACKAGE_TEXT];
    let mut index = 0usize;

    while let Some(repo) = rt::package_repository_list(package_handle, index, &mut name, &mut url)?
    {
        let name_text =
            core::str::from_utf8(&name[..repo.name_len]).map_err(|_| rt::Error::InvalidArgument)?;
        let url_text =
            core::str::from_utf8(&url[..repo.url_len]).map_err(|_| rt::Error::InvalidArgument)?;
        let ledger_state = match onboard::onboard_lookup(name_text) {
            Some(true) => " onboarded",
            Some(false) => " onboarded-disabled",
            None => "",
        };
        write_output_linef(
            output,
            format_args!(
                "#{} {} pkgs={} trust={} sync={} channel={} ring={} enabled={} digest={:016x} source={}{}",
                repo.repo_index,
                name_text,
                repo.package_count,
                trust_mode_name(repo.trust_mode),
                repo_sync_state_name(repo.sync_state),
                channel_name(repo.channel),
                ring_name(repo.ring),
                if repo.enabled { "yes" } else { "no" },
                repo.last_digest,
                url_text,
                ledger_state,
            ),
        )?;
        index += 1;
    }

    let _ = rt::handle_close(package_handle);
    if index == 0 {
        write_output_linef(output, format_args!("no repositories"))
    } else {
        Ok(())
    }
}

pub(super) fn cmd_pkg_repo<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    mut parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("add") => {
            let Some(name) = parts.next() else {
                return write_output_linef(
                    output,
                    format_args!(
                        "usage: pkg repo add <name> <url> [unsigned|pinned:<hex>|signed-key] [stable|beta|canary] [production|preview|testing] [--yes]"
                    ),
                );
            };
            let Some(url) = parts.next() else {
                return write_output_linef(
                    output,
                    format_args!(
                        "usage: pkg repo add <name> <url> [unsigned|pinned:<hex>|signed-key] [stable|beta|canary] [production|preview|testing] [--yes]"
                    ),
                );
            };
            let mut positionals: [&str; 3] = ["unsigned", "stable", "user"];
            let mut filled = 0usize;
            let mut confirmed = false;
            for token in parts {
                if token == "--yes" {
                    confirmed = true;
                    continue;
                }
                if filled < 3 {
                    positionals[filled] = token;
                    filled += 1;
                }
            }
            cmd_pkg_repo_add(
                bootstrap,
                output,
                name,
                url,
                positionals[0],
                positionals[1],
                positionals[2],
                confirmed,
            )
        }
        Some("sync") => match parts.next() {
            Some("all") | None => cmd_pkg_repo_sync(bootstrap, output, None),
            Some(index) => match parse_usize(index) {
                Some(value) => cmd_pkg_repo_sync(bootstrap, output, Some(value)),
                None => {
                    write_output_linef(output, format_args!("usage: pkg repo sync [all|index]"))
                }
            },
        },
        Some("enable") => match parts.next() {
            Some(name) => onboard::cmd_pkg_repo_set_enabled(output, name, true),
            None => write_output_linef(output, format_args!("usage: pkg repo enable <name>")),
        },
        Some("disable") => match parts.next() {
            Some(name) => onboard::cmd_pkg_repo_set_enabled(output, name, false),
            None => write_output_linef(output, format_args!("usage: pkg repo disable <name>")),
        },
        Some("remove") => match parts.next() {
            Some(name) => onboard::cmd_pkg_repo_remove(output, name),
            None => write_output_linef(output, format_args!("usage: pkg repo remove <name>")),
        },
        Some("status") => onboard::cmd_pkg_repo_status(bootstrap, output),
        _ => write_output_linef(
            output,
            format_args!("usage: pkg repo <add|sync|enable|disable|remove|status> ..."),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_pkg_repo_add(
    bootstrap: rt::Handle,
    output: ShellOutput,
    name: &str,
    url: &str,
    trust_text: &str,
    channel_text: &str,
    ring_text: &str,
    confirmed: bool,
) -> rt::Result<()> {
    let Some((trust_mode, digest)) = parse_repo_trust(trust_text) else {
        return write_output_linef(
            output,
            format_args!("trust must be unsigned, pinned:<hex-digest>, or signed-key"),
        );
    };
    let Some(channel) = parse_channel(channel_text) else {
        return write_output_linef(
            output,
            format_args!("channel must be stable, beta, or nightly"),
        );
    };
    let Some(ring) = parse_ring(ring_text) else {
        return write_output_linef(output, format_args!("ring must be user, beta, or canary"));
    };

    let plan = RepoAddPlan {
        name,
        url,
        trust_mode,
        pinned_digest: digest,
    };
    if !confirmed {
        return onboard::write_repo_review(output, &plan);
    }

    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let result = rt::package_repository_add(
        package_handle,
        plan.name,
        plan.url,
        trust_mode,
        channel,
        ring,
        true,
        digest,
    );
    let _ = rt::handle_close(package_handle);
    result?;
    onboard::onboard_record(name)?;
    write_output_linef(
        output,
        format_args!(
            "added repository {} (trust {}; manage with pkg repo status)",
            name,
            trust_mode_name(trust_mode),
        ),
    )
}

fn cmd_pkg_repo_sync(
    bootstrap: rt::Handle,
    output: ShellOutput,
    repo_index: Option<usize>,
) -> rt::Result<()> {
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let result = rt::package_repository_sync(package_handle, repo_index);
    let _ = rt::handle_close(package_handle);
    let info = result?;
    write_output_linef(
        output,
        format_args!("synced={} failed={}", info.synced, info.failed),
    )
}

use rt::ServiceId;
use serviceos_userspace_runtime as rt;

use crate::util::{ShellOutput, write_output_linef};

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
        write_output_linef(
            output,
            format_args!(
                "#{} {} pkgs={} trust={} sync={} channel={} ring={} enabled={} digest={:016x} source={}",
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
                        "usage: pkg repo add <name> <url> [unsigned|pinned:<hex>] [stable|beta|canary] [production|preview|testing]"
                    ),
                );
            };
            let Some(url) = parts.next() else {
                return write_output_linef(
                    output,
                    format_args!(
                        "usage: pkg repo add <name> <url> [unsigned|pinned:<hex>] [stable|beta|canary] [production|preview|testing]"
                    ),
                );
            };
            let trust = parts.next().unwrap_or("unsigned");
            let channel = parts.next().unwrap_or("stable");
            let ring = parts.next().unwrap_or("user");
            cmd_pkg_repo_add(bootstrap, output, name, url, trust, channel, ring)
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
        _ => write_output_linef(output, format_args!("usage: pkg repo <add|sync> ...")),
    }
}

fn cmd_pkg_repo_add(
    bootstrap: rt::Handle,
    output: ShellOutput,
    name: &str,
    url: &str,
    trust_text: &str,
    channel_text: &str,
    ring_text: &str,
) -> rt::Result<()> {
    let Some((trust_mode, digest)) = parse_repo_trust(trust_text) else {
        return write_output_linef(
            output,
            format_args!("trust must be unsigned or pinned:<hex-digest>"),
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
    let package_handle = rt::lookup_service(bootstrap, ServiceId::Package)?;
    let result = rt::package_repository_add(
        package_handle,
        name,
        url,
        trust_mode,
        channel,
        ring,
        true,
        digest,
    );
    let _ = rt::handle_close(package_handle);
    result?;
    write_output_linef(output, format_args!("added repository {}", name))
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

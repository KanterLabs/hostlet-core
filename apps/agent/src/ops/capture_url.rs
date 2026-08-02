//! Capture-target URL validation for the screenshotter.
//!
//! The screenshotter container runs with `--network host`, so host-local
//! services (loopback, LAN, link-local) are directly reachable from it. This
//! validator rejects non-public addresses as defense in depth; the per-request
//! enforcement that allows the target's own origin while blocking redirect
//! hops and subresources lives in apps/screenshotter/capture.js.

use anyhow::{bail, Context};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::Path;

pub(super) fn validate_capture_url(value: &str) -> anyhow::Result<()> {
    let url = url::Url::parse(value).context("capture_url must be an absolute URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("capture_url must use http or https");
    }
    match url.host() {
        Some(url::Host::Ipv4(ip)) if blocked_ipv4(ip) => {
            bail!("capture_url must not target a private or local address")
        }
        Some(url::Host::Ipv6(ip)) if blocked_ipv6(ip) => {
            bail!("capture_url must not target a private or local address")
        }
        Some(url::Host::Domain(host)) => {
            let host = host.trim_end_matches('.').to_ascii_lowercase();
            if host == "localhost" || host.ends_with(".localhost") || !host.contains('.') {
                bail!("capture_url must use a public hostname");
            }
        }
        None => bail!("capture_url must include a host"),
        _ => {}
    }
    Ok(())
}

/// Validate the origin used by the internal local-router screenshot path and
/// return its canonical tenant hostname.  The caller must run the general
/// public-target validator first; this function repeats that check so it is
/// safe to use independently in focused tests and future call sites.
pub(super) fn validate_internal_capture_url(
    value: &str,
    base_domain: &str,
) -> anyhow::Result<String> {
    validate_capture_url(value)?;
    let base_domain = crate::validate_base_domain(base_domain)?;
    let url = url::Url::parse(value).context("capture_url must be an absolute URL")?;
    if url.scheme() != "https" {
        bail!("capture_url must use https for internal screenshot routing");
    }
    if !url.username().is_empty() || url.password().is_some() || raw_authority_has_at(value) {
        bail!("capture_url must not include userinfo");
    }
    if url.port().is_some_and(|port| port != 443) {
        bail!("capture_url must not use a non-default port");
    }
    let Some(url::Host::Domain(host)) = url.host() else {
        bail!("capture_url must use a hostname for internal screenshot routing");
    };
    if host.ends_with('.') || !host.is_ascii() {
        bail!("capture_url host must be a canonical ASCII hostname");
    }
    let host = host.to_ascii_lowercase();
    crate::validate_hostname_labels(&host, "capture_url host")?;
    let suffix = format!(".{base_domain}");
    let tenant = host
        .strip_suffix(&suffix)
        .filter(|value| !value.is_empty() && !value.contains('.'))
        .context("capture_url host must be exactly one label below HOSTLET_BASE_DOMAIN")?;
    crate::validate_hostname_labels(tenant, "capture_url host")?;
    Ok(host)
}

fn raw_authority_has_at(value: &str) -> bool {
    let Some((_, authority_and_path)) = value.split_once("://") else {
        return false;
    };
    let authority_end = authority_and_path
        .find(['/', '?', '#'])
        .unwrap_or(authority_and_path.len());
    authority_and_path[..authority_end].contains('@')
}

/// Verify that the local Caddy router has an installed Hostlet snippet for
/// the exact validated tenant hostname.  A comment match is intentionally
/// line-exact; accepting a substring or a different route's host would let a
/// capture use an unintended upstream.
pub(super) async fn verify_local_caddy_route(
    snippets_dir: &Path,
    host: &str,
) -> anyhow::Result<()> {
    let expected = format!("# hostlet-domain: {host}");
    let mut entries = tokio::fs::read_dir(snippets_dir).await.with_context(|| {
        format!(
            "cannot read local Caddy snippets directory {}",
            snippets_dir.display()
        )
    })?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("caddy") {
            continue;
        }
        let contents = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("cannot read local Caddy snippet {}", path.display()))?;
        if contents.lines().any(|line| line == expected) {
            return Ok(());
        }
    }
    bail!("no installed local Caddy snippet matches # hostlet-domain: {host}")
}

fn blocked_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_broadcast()
        || octets[0] == 0
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
}

fn blocked_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return blocked_ipv4(v4);
    }
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (ip.segments()[0] & 0xfe00) == 0xfc00
        || (ip.segments()[0] & 0xffc0) == 0xfe80
}

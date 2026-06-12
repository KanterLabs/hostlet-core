//! Capture-target URL validation for the screenshotter.
//!
//! The screenshotter container runs with `--network host`, so host-local
//! services (loopback, LAN, link-local) are directly reachable from it. This
//! validator rejects non-public addresses as defense in depth; the per-request
//! enforcement that allows the target's own origin while blocking redirect
//! hops and subresources lives in apps/screenshotter/capture.js.

use anyhow::{bail, Context};
use std::net::{Ipv4Addr, Ipv6Addr};

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

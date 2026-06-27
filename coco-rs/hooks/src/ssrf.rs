//! SSRF guard for HTTP hooks.
//!
//! Blocks private/link-local address ranges to prevent project-configured
//! HTTP hooks from reaching cloud metadata endpoints (169.254.169.254) or
//! internal infrastructure.
//!
//! Loopback (127.0.0.0/8, ::1) is intentionally ALLOWED — local dev policy
//! servers are a primary HTTP hook use case.

use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::Ipv6Addr;
use std::net::SocketAddr;

/// Returns true if the IP address is in a range that HTTP hooks should not reach.
///
/// Blocked IPv4:
///   0.0.0.0/8        "this" network
///   10.0.0.0/8       private
///   100.64.0.0/10    shared address space / CGNAT
///   169.254.0.0/16   link-local (cloud metadata)
///   172.16.0.0/12    private
///   192.168.0.0/16   private
///
/// Blocked IPv6:
///   ::               unspecified
///   fc00::/7         unique local
///   fe80::/10        link-local
///   ::ffff:<v4>      mapped IPv4 in a blocked range
///
/// Allowed (returns false):
///   127.0.0.0/8      loopback (local dev hooks)
///   ::1              loopback
///   everything else
pub fn is_blocked_address(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => is_blocked_v4(*v4),
        IpAddr::V6(v6) => is_blocked_v6(v6),
    }
}

fn is_blocked_v4(addr: Ipv4Addr) -> bool {
    let [a, b, _, _] = addr.octets();

    // Loopback explicitly allowed
    if a == 127 {
        return false;
    }

    // 0.0.0.0/8
    if a == 0 {
        return true;
    }
    // 10.0.0.0/8
    if a == 10 {
        return true;
    }
    // 169.254.0.0/16 — link-local, cloud metadata
    if a == 169 && b == 254 {
        return true;
    }
    // 172.16.0.0/12
    if a == 172 && (16..=31).contains(&b) {
        return true;
    }
    // 100.64.0.0/10 — shared address space (RFC 6598, CGNAT)
    if a == 100 && (64..=127).contains(&b) {
        return true;
    }
    // 192.168.0.0/16
    if a == 192 && b == 168 {
        return true;
    }

    false
}

fn is_blocked_v6(addr: &Ipv6Addr) -> bool {
    // ::1 loopback explicitly allowed
    if addr.is_loopback() {
        return false;
    }

    // :: unspecified
    if addr.is_unspecified() {
        return true;
    }

    let segments = addr.segments();

    // IPv4-mapped IPv6 (::ffff:X.X.X.X)
    if segments[0] == 0
        && segments[1] == 0
        && segments[2] == 0
        && segments[3] == 0
        && segments[4] == 0
        && segments[5] == 0xffff
    {
        let v4 = Ipv4Addr::new(
            (segments[6] >> 8) as u8,
            (segments[6] & 0xff) as u8,
            (segments[7] >> 8) as u8,
            (segments[7] & 0xff) as u8,
        );
        return is_blocked_v4(v4);
    }

    // fc00::/7 — unique local
    if segments[0] & 0xfe00 == 0xfc00 {
        return true;
    }

    // fe80::/10 — link-local
    if segments[0] & 0xffc0 == 0xfe80 {
        return true;
    }

    false
}

/// Extract the host from a URL string.
///
/// Returns `None` if the URL cannot be parsed.
fn extract_host(url: &str) -> Option<&str> {
    let after_scheme = url.split("://").nth(1)?;
    // Strip userinfo (user:pass@host)
    let host_section = after_scheme.split('/').next()?;
    let host_section = if let Some((_userinfo, rest)) = host_section.split_once('@') {
        rest
    } else {
        host_section
    };
    // Strip port
    // Handle IPv6 literal: [::1]:8080
    if host_section.starts_with('[') {
        let end = host_section.find(']')?;
        Some(&host_section[1..end])
    } else {
        Some(host_section.split(':').next().unwrap_or(host_section))
    }
}

/// Resolve a URL's host and check if any resolved address is in a blocked range.
///
/// Returns `Ok(true)` if the address is blocked (should not be reached).
/// Returns `Ok(false)` if all resolved addresses are allowed.
pub async fn check_url_ssrf(url: &str) -> crate::Result<bool> {
    let host = extract_host(url).ok_or_else(|| crate::HooksError::SsrfFailed {
        url: url.to_string(),
        message: "URL has no host".to_string(),
    })?;

    // If it's already an IP literal, check directly
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(is_blocked_address(&ip));
    }

    // DNS resolution — use port 80 as placeholder (only need IP addresses)
    let addrs = tokio::net::lookup_host(format!("{host}:80"))
        .await
        .map_err(|e| crate::HooksError::SsrfFailed {
            url: url.to_string(),
            message: format!("DNS resolution failed for {host}: {e}"),
        })?;

    for addr in addrs {
        if is_blocked_address(&addr.ip()) {
            return Ok(true);
        }
    }

    Ok(false)
}

/// A `reqwest` DNS resolver that drops SSRF-blocked addresses, so a connection
/// can only be made to an allowed IP — **the validated IP is the connected IP**.
///
/// This closes the DNS-rebinding TOCTOU that the standalone pre-flight
/// [`check_url_ssrf`] cannot: between the pre-flight resolution and reqwest's
/// own resolution at connect time, a hostname can rebind to a private / cloud-
/// metadata address. Installing this as reqwest's resolver makes the guarded
/// resolution the one that actually drives the socket.
///
/// NOTE: reqwest does NOT invoke a custom resolver when the URL host is an IP
/// literal — those connect directly, so the [`check_url_ssrf`] pre-flight is
/// still required to block e.g. `http://169.254.169.254/`.
#[derive(Debug, Clone, Default)]
pub struct SsrfGuardedResolver;

impl reqwest::dns::Resolve for SsrfGuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            // System resolution (the port is a placeholder — reqwest substitutes
            // the real one). Drop every address in a blocked range so a rebind to
            // a private / link-local IP cannot be connected to.
            let resolved = tokio::net::lookup_host((host.as_str(), 0)).await?;
            let allowed: Vec<SocketAddr> =
                resolved.filter(|a| !is_blocked_address(&a.ip())).collect();
            if allowed.is_empty() {
                return Err(format!(
                    "SSRF guard: all addresses resolved for `{host}` are private/link-local"
                )
                .into());
            }
            let addrs: reqwest::dns::Addrs = Box::new(allowed.into_iter());
            Ok(addrs)
        })
    }
}

/// Check if a URL is allowed by the URL allowlist.
/// Returns `true` if the URL is allowed (or no allowlist is set).
pub fn url_matches_allowlist(url: &str, allowed_urls: &[String]) -> bool {
    if allowed_urls.is_empty() {
        return true;
    }
    allowed_urls.iter().any(|pattern| {
        // Match with * as wildcard (any characters)
        let regex_str = regex::escape(pattern).replace(r"\*", ".*");
        regex::Regex::new(&format!("^{regex_str}$"))
            .map(|re| re.is_match(url))
            .unwrap_or(false)
    })
}

#[cfg(test)]
#[path = "ssrf.test.rs"]
mod tests;

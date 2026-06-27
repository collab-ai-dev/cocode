//! Domain-based network filtering for sandbox proxies.

use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::Ipv6Addr;

use crate::config::NetworkMode;

/// Domain filter with allow/deny lists, wildcard support, and network mode.
///
/// Deny list always takes precedence. An empty allow list permits all
/// domains not in the deny list. Wildcard entries like `*.example.com`
/// match any subdomain of `example.com`.
///
/// When `block_non_public_ips` is enabled, connections to private/reserved
/// IP addresses are blocked to prevent SSRF attacks.
///
/// In `Limited` mode, only GET/HEAD/OPTIONS HTTP methods are allowed.
/// CONNECT tunnels and SOCKS5 are blocked.
pub struct DomainFilter {
    allowed_domains: Vec<String>,
    denied_domains: Vec<String>,
    /// Unix socket paths allowed through the sandbox.
    allowed_unix_sockets: Vec<std::path::PathBuf>,
    /// Allow all Unix sockets without filtering.
    allow_all_unix_sockets: bool,
    /// Allow binding to localhost ports.
    allow_local_binding: bool,
    /// Block connections to non-public IP addresses (SSRF prevention).
    block_non_public_ips: bool,
    /// Network access mode (Full or Limited).
    network_mode: NetworkMode,
}

impl DomainFilter {
    /// Create a new domain filter.
    ///
    /// Domains are normalized to lowercase. Wildcard prefixes (`*.`)
    /// match any subdomain of the base domain.
    pub fn new(allowed: Vec<String>, denied: Vec<String>) -> Self {
        Self {
            allowed_domains: allowed.into_iter().map(|d| d.to_lowercase()).collect(),
            denied_domains: denied.into_iter().map(|d| d.to_lowercase()).collect(),
            allowed_unix_sockets: Vec::new(),
            allow_all_unix_sockets: false,
            allow_local_binding: false,
            block_non_public_ips: false,
            network_mode: NetworkMode::Full,
        }
    }

    /// Create a domain filter from the full `NetworkConfig`.
    pub fn from_network_config(config: &crate::config::NetworkConfig) -> Self {
        Self {
            allowed_domains: config
                .allowed_domains
                .iter()
                .map(|d| d.to_lowercase())
                .collect(),
            denied_domains: config
                .denied_domains
                .iter()
                .map(|d| d.to_lowercase())
                .collect(),
            allowed_unix_sockets: config.allow_unix_sockets.clone(),
            allow_all_unix_sockets: config.allow_all_unix_sockets,
            allow_local_binding: config.allow_local_binding,
            block_non_public_ips: config.block_non_public_ips,
            network_mode: config.mode,
        }
    }

    /// Enable SSRF prevention (block non-public IPs).
    pub fn with_ssrf_protection(mut self) -> Self {
        self.block_non_public_ips = true;
        self
    }

    /// Network access mode.
    pub fn network_mode(&self) -> NetworkMode {
        self.network_mode
    }

    /// Check if an HTTP method is allowed in the current mode.
    pub fn allows_method(&self, method: &str) -> bool {
        self.network_mode.allows_method(method)
    }

    /// Check if a Unix socket path is allowed.
    pub fn is_unix_socket_allowed(&self, path: &std::path::Path) -> bool {
        if self.allow_all_unix_sockets {
            return true;
        }
        self.allowed_unix_sockets
            .iter()
            .any(|allowed| path.starts_with(allowed))
    }

    /// Whether local port binding is allowed.
    pub fn allow_local_binding(&self) -> bool {
        self.allow_local_binding
    }

    /// Check if a domain is allowed through the proxy.
    ///
    /// The host string is normalized before matching (trailing dots stripped,
    /// bracketed IPv6 unwrapped, ports removed, lowercased).
    ///
    /// Evaluation order:
    /// 1. SSRF check -> deny non-public IPs (if enabled)
    /// 2. Deny list match -> deny (highest priority)
    /// 3. Allow list empty -> allow (open policy)
    /// 4. Allow list match -> allow
    /// 5. Otherwise -> deny
    pub fn is_allowed(&self, domain: &str) -> bool {
        let normalized = normalize_host(domain);

        // SSRF prevention: block non-public IP addresses.
        if self.block_non_public_ips
            && let Ok(ip) = normalized.parse::<IpAddr>()
            && is_non_public_ip(ip)
        {
            tracing::info!(ip = %normalized, "SSRF prevention: blocked non-public IP");
            return false;
        }

        if self.matches_list(&normalized, &self.denied_domains) {
            return false;
        }

        if self.allowed_domains.is_empty() {
            // No allow-list. With a deny-list configured this is blocklist mode
            // (allow everything not denied). With NO domain config at all, deny
            // by default — this filter is the sandbox's egress gate, and
            // enabling the sandbox must not silently grant unrestricted network
            // (the deny-by-default posture the adapter advertises).
            return !self.denied_domains.is_empty();
        }

        self.matches_list(&normalized, &self.allowed_domains)
    }

    /// Check if a domain matches any entry in the given list.
    ///
    /// Supports exact match and wildcard suffix matching (`*.example.com`).
    fn matches_list(&self, domain: &str, list: &[String]) -> bool {
        list.iter().any(|pattern| {
            if let Some(suffix) = pattern.strip_prefix("*.") {
                // Wildcard: *.example.com matches sub.example.com
                // but not example.com itself.
                domain.ends_with(&format!(".{suffix}"))
            } else {
                domain == pattern
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Host normalization
// ---------------------------------------------------------------------------

/// Normalize a host string for consistent policy matching.
///
/// - Strips trailing dots (`example.com.` -> `example.com`)
/// - Removes bracketed IPv6 notation (`[::1]` -> `::1`)
/// - Strips port suffixes (`example.com:443` -> `example.com`)
/// - Lowercases the result
pub fn normalize_host(host: &str) -> String {
    let mut s = host.trim().to_lowercase();

    // Strip bracketed IPv6: [::1]:443 → ::1
    if s.starts_with('[') {
        if let Some(bracket_end) = s.find(']') {
            s = s[1..bracket_end].to_string();
        }
    } else if let Some((host_part, port_str)) = s.rsplit_once(':') {
        // Strip port for non-bracketed hosts: example.com:443 → example.com
        // Only strip if the part after ':' looks like a port number,
        // not an IPv6 segment.
        if port_str.chars().all(|c| c.is_ascii_digit()) {
            s = host_part.to_string();
        }
    }

    // Strip trailing dots (DNS root notation).
    while s.ends_with('.') {
        s.pop();
    }

    s
}

// ---------------------------------------------------------------------------
// SSRF prevention: non-public IP detection
// ---------------------------------------------------------------------------

/// Returns `true` if the given IP address is non-public (private, reserved,
/// loopback, link-local, CGNAT, TEST-NET, etc.).
///
/// Used to prevent SSRF attacks where a tool might be tricked into connecting
/// to internal network addresses.
pub fn is_non_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_non_public_ipv4(ip),
        IpAddr::V6(ip) => is_non_public_ipv6(ip),
    }
}

fn is_non_public_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ipv4_in_cidr(ip, [0, 0, 0, 0], /*prefix*/ 8)       // "this network" (RFC 1122)
        || ipv4_in_cidr(ip, [100, 64, 0, 0], /*prefix*/ 10)    // CGNAT (RFC 6598)
        || ipv4_in_cidr(ip, [192, 0, 0, 0], /*prefix*/ 24)     // IETF Protocol Assignments (RFC 6890)
        || ipv4_in_cidr(ip, [192, 0, 2, 0], /*prefix*/ 24)     // TEST-NET-1 (RFC 5737)
        || ipv4_in_cidr(ip, [198, 18, 0, 0], /*prefix*/ 15)    // Benchmarking (RFC 2544)
        || ipv4_in_cidr(ip, [198, 51, 100, 0], /*prefix*/ 24)  // TEST-NET-2 (RFC 5737)
        || ipv4_in_cidr(ip, [203, 0, 113, 0], /*prefix*/ 24)   // TEST-NET-3 (RFC 5737)
        || ipv4_in_cidr(ip, [240, 0, 0, 0], /*prefix*/ 4) // Reserved (RFC 6890)
}

fn ipv4_in_cidr(ip: Ipv4Addr, base: [u8; 4], prefix: u8) -> bool {
    let ip = u32::from(ip);
    let base = u32::from(Ipv4Addr::from(base));
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    (ip & mask) == (base & mask)
}

fn is_non_public_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4() {
        return is_non_public_ipv4(v4) || ip.is_loopback();
    }
    // Non-globally-routable ranges:
    //  - ::1 loopback
    //  - fc00::/7 unique-local (RFC 4193)
    //  - fe80::/10 link-local
    //  - :: unspecified
    //  - multicast ranges
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
}

#[cfg(test)]
#[path = "filter.test.rs"]
mod tests;

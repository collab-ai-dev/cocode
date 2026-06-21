use super::*;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::Ipv6Addr;

// ---------------------------------------------------------------------------
// DomainFilter: basic domain filtering
// ---------------------------------------------------------------------------

#[test]
fn test_is_allowed_no_config_denies_by_default() {
    // No allow-list and no deny-list = no network config. The egress gate must
    // deny by default so enabling the sandbox doesn't silently grant full
    // network. (Configure allowed_domains, or set network.mode = Full, to allow.)
    let filter = DomainFilter::new(vec![], vec![]);
    assert!(!filter.is_allowed("example.com"));
    assert!(!filter.is_allowed("anything.org"));
}

#[test]
fn test_is_allowed_deny_only_is_blocklist_mode() {
    // A deny-list with no allow-list = blocklist mode: allow everything not
    // explicitly denied.
    let filter = DomainFilter::new(vec![], vec!["evil.com".to_string()]);
    assert!(filter.is_allowed("good.com"));
    assert!(!filter.is_allowed("evil.com"));
}

#[test]
fn test_is_allowed_deny_list_blocks() {
    let filter = DomainFilter::new(vec![], vec!["evil.com".to_string()]);
    assert!(!filter.is_allowed("evil.com"));
    assert!(filter.is_allowed("good.com"));
}

#[test]
fn test_is_allowed_deny_takes_precedence_over_allow() {
    let filter = DomainFilter::new(
        vec!["example.com".to_string()],
        vec!["example.com".to_string()],
    );
    assert!(!filter.is_allowed("example.com"));
}

#[test]
fn test_is_allowed_allow_list_restricts() {
    let filter = DomainFilter::new(
        vec!["allowed.com".to_string(), "also-ok.org".to_string()],
        vec![],
    );
    assert!(filter.is_allowed("allowed.com"));
    assert!(filter.is_allowed("also-ok.org"));
    assert!(!filter.is_allowed("blocked.net"));
}

#[test]
fn test_is_allowed_wildcard_deny() {
    let filter = DomainFilter::new(vec![], vec!["*.evil.com".to_string()]);
    assert!(!filter.is_allowed("sub.evil.com"));
    assert!(!filter.is_allowed("deep.sub.evil.com"));
    // Exact domain does not match wildcard pattern.
    assert!(filter.is_allowed("evil.com"));
}

#[test]
fn test_is_allowed_wildcard_allow() {
    let filter = DomainFilter::new(vec!["*.example.com".to_string()], vec![]);
    assert!(filter.is_allowed("api.example.com"));
    assert!(filter.is_allowed("deep.sub.example.com"));
    // Exact domain does not match wildcard pattern.
    assert!(!filter.is_allowed("example.com"));
}

#[test]
fn test_is_allowed_case_insensitive() {
    let filter = DomainFilter::new(
        vec!["Example.COM".to_string()],
        vec!["EVIL.org".to_string()],
    );
    assert!(filter.is_allowed("example.com"));
    assert!(filter.is_allowed("EXAMPLE.COM"));
    assert!(!filter.is_allowed("evil.org"));
    assert!(!filter.is_allowed("Evil.Org"));
}

#[test]
fn test_is_allowed_wildcard_case_insensitive() {
    let filter = DomainFilter::new(vec!["*.Example.COM".to_string()], vec![]);
    assert!(filter.is_allowed("sub.example.com"));
    assert!(filter.is_allowed("SUB.EXAMPLE.COM"));
}

#[test]
fn test_is_allowed_deny_wildcard_overrides_allow_exact() {
    let filter = DomainFilter::new(
        vec!["api.example.com".to_string()],
        vec!["*.example.com".to_string()],
    );
    assert!(!filter.is_allowed("api.example.com"));
}

#[test]
fn test_is_allowed_allow_exact_and_wildcard() {
    let filter = DomainFilter::new(
        vec!["example.com".to_string(), "*.example.com".to_string()],
        vec![],
    );
    assert!(filter.is_allowed("example.com"));
    assert!(filter.is_allowed("api.example.com"));
    assert!(!filter.is_allowed("other.com"));
}

// ---------------------------------------------------------------------------
// Host normalization
// ---------------------------------------------------------------------------

#[test]
fn test_normalize_host_trailing_dots() {
    assert_eq!(normalize_host("example.com."), "example.com");
    assert_eq!(normalize_host("example.com.."), "example.com");
}

#[test]
fn test_normalize_host_port_stripping() {
    assert_eq!(normalize_host("example.com:443"), "example.com");
    assert_eq!(normalize_host("example.com:80"), "example.com");
}

#[test]
fn test_normalize_host_bracketed_ipv6() {
    assert_eq!(normalize_host("[::1]"), "::1");
    assert_eq!(normalize_host("[::1]:443"), "::1");
    assert_eq!(normalize_host("[fe80::1%25eth0]"), "fe80::1%25eth0");
}

#[test]
fn test_normalize_host_lowercase() {
    assert_eq!(normalize_host("EXAMPLE.COM"), "example.com");
    assert_eq!(normalize_host("Example.Com:443"), "example.com");
}

#[test]
fn test_normalize_host_whitespace_trimming() {
    assert_eq!(normalize_host("  example.com  "), "example.com");
}

#[test]
fn test_normalize_host_combined() {
    // Trailing dot + port + uppercase
    assert_eq!(normalize_host("EXAMPLE.COM.:443"), "example.com");
}

#[test]
fn test_is_allowed_normalizes_host() {
    let filter = DomainFilter::new(vec!["example.com".to_string()], vec![]);
    // Trailing dot should match after normalization.
    assert!(filter.is_allowed("example.com."));
    // Port should be stripped.
    assert!(filter.is_allowed("example.com:443"));
    // Combined.
    assert!(filter.is_allowed("EXAMPLE.COM.:8080"));
}

// ---------------------------------------------------------------------------
// SSRF prevention: non-public IP detection
// ---------------------------------------------------------------------------

#[test]
fn test_is_non_public_ip_loopback() {
    assert!(is_non_public_ip(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    assert!(is_non_public_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
}

#[test]
fn test_is_non_public_ip_private_rfc1918() {
    assert!(is_non_public_ip("10.0.0.1".parse().unwrap()));
    assert!(is_non_public_ip("172.16.0.1".parse().unwrap()));
    assert!(is_non_public_ip("192.168.1.1".parse().unwrap()));
}

#[test]
fn test_is_non_public_ip_link_local() {
    assert!(is_non_public_ip("169.254.0.1".parse().unwrap()));
}

#[test]
fn test_is_non_public_ip_cgnat() {
    assert!(is_non_public_ip("100.64.0.1".parse().unwrap()));
    assert!(is_non_public_ip("100.127.255.254".parse().unwrap()));
}

#[test]
fn test_is_non_public_ip_test_nets() {
    assert!(is_non_public_ip("192.0.2.1".parse().unwrap())); // TEST-NET-1
    assert!(is_non_public_ip("198.51.100.1".parse().unwrap())); // TEST-NET-2
    assert!(is_non_public_ip("203.0.113.1".parse().unwrap())); // TEST-NET-3
}

#[test]
fn test_is_non_public_ip_benchmarking() {
    assert!(is_non_public_ip("198.18.0.1".parse().unwrap()));
    assert!(is_non_public_ip("198.19.255.255".parse().unwrap()));
}

#[test]
fn test_is_non_public_ip_reserved() {
    assert!(is_non_public_ip("240.0.0.1".parse().unwrap()));
}

#[test]
fn test_is_non_public_ip_public_addresses_pass() {
    assert!(!is_non_public_ip("8.8.8.8".parse().unwrap()));
    assert!(!is_non_public_ip("1.1.1.1".parse().unwrap()));
    assert!(!is_non_public_ip("151.101.1.1".parse().unwrap()));
}

#[test]
fn test_is_non_public_ipv6_unique_local() {
    assert!(is_non_public_ip("fd00::1".parse().unwrap()));
}

#[test]
fn test_is_non_public_ipv6_link_local() {
    assert!(is_non_public_ip("fe80::1".parse().unwrap()));
}

#[test]
fn test_ssrf_prevention_in_filter() {
    // Allow-list the public hosts so deny-by-default doesn't mask the SSRF gate.
    let filter = DomainFilter::new(
        vec!["8.8.8.8".to_string(), "example.com".to_string()],
        vec![],
    )
    .with_ssrf_protection();
    // Private IPs are blocked by SSRF even when not in the allow-list.
    assert!(!filter.is_allowed("127.0.0.1"));
    assert!(!filter.is_allowed("10.0.0.1"));
    assert!(!filter.is_allowed("192.168.1.1"));
    assert!(!filter.is_allowed("[::1]"));
    // Allow-listed public IP / domain pass.
    assert!(filter.is_allowed("8.8.8.8"));
    assert!(filter.is_allowed("example.com"));
}

#[test]
fn test_ssrf_prevention_disabled_by_default() {
    // Allow-list the private IPs; with SSRF protection OFF they must pass
    // (this isolates the SSRF gate from deny-by-default, which would otherwise
    // block them for lack of an allow-list entry).
    let filter = DomainFilter::new(
        vec!["127.0.0.1".to_string(), "10.0.0.1".to_string()],
        vec![],
    );
    assert!(filter.is_allowed("127.0.0.1"));
    assert!(filter.is_allowed("10.0.0.1"));
}

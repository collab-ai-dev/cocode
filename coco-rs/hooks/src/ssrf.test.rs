use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::Ipv6Addr;

use super::*;

// -----------------------------------------------------------------------
// is_blocked_address — IPv4
// -----------------------------------------------------------------------

#[test]
fn test_loopback_v4_allowed() {
    let addr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
    assert!(!is_blocked_address(&addr));
}

#[test]
fn test_loopback_v4_other_allowed() {
    let addr = IpAddr::V4(Ipv4Addr::new(127, 1, 2, 3));
    assert!(!is_blocked_address(&addr));
}

#[test]
fn test_this_network_blocked() {
    let addr = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0));
    assert!(is_blocked_address(&addr));
}

#[test]
fn test_private_10_blocked() {
    let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
    assert!(is_blocked_address(&addr));
}

#[test]
fn test_link_local_blocked() {
    let addr = IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254));
    assert!(is_blocked_address(&addr));
}

#[test]
fn test_private_172_blocked() {
    let addr = IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1));
    assert!(is_blocked_address(&addr));
    let addr = IpAddr::V4(Ipv4Addr::new(172, 31, 255, 255));
    assert!(is_blocked_address(&addr));
}

#[test]
fn test_cgnat_blocked() {
    let addr = IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1));
    assert!(is_blocked_address(&addr));
    let addr = IpAddr::V4(Ipv4Addr::new(100, 127, 255, 255));
    assert!(is_blocked_address(&addr));
}

#[test]
fn test_private_192_168_blocked() {
    let addr = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
    assert!(is_blocked_address(&addr));
}

#[test]
fn test_public_ip_allowed() {
    let addr = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));
    assert!(!is_blocked_address(&addr));
    let addr = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));
    assert!(!is_blocked_address(&addr));
}

#[test]
fn test_172_15_allowed() {
    // 172.15.x.x is NOT in the 172.16.0.0/12 range
    let addr = IpAddr::V4(Ipv4Addr::new(172, 15, 0, 1));
    assert!(!is_blocked_address(&addr));
}

#[test]
fn test_172_32_allowed() {
    let addr = IpAddr::V4(Ipv4Addr::new(172, 32, 0, 1));
    assert!(!is_blocked_address(&addr));
}

#[test]
fn test_100_63_allowed() {
    let addr = IpAddr::V4(Ipv4Addr::new(100, 63, 0, 1));
    assert!(!is_blocked_address(&addr));
}

#[test]
fn test_100_128_allowed() {
    let addr = IpAddr::V4(Ipv4Addr::new(100, 128, 0, 1));
    assert!(!is_blocked_address(&addr));
}

// -----------------------------------------------------------------------
// is_blocked_address — IPv6
// -----------------------------------------------------------------------

#[test]
fn test_loopback_v6_allowed() {
    let addr = IpAddr::V6(Ipv6Addr::LOCALHOST);
    assert!(!is_blocked_address(&addr));
}

#[test]
fn test_unspecified_v6_blocked() {
    let addr = IpAddr::V6(Ipv6Addr::UNSPECIFIED);
    assert!(is_blocked_address(&addr));
}

#[test]
fn test_unique_local_v6_blocked() {
    // fc00::/7
    let addr = IpAddr::V6(Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 1));
    assert!(is_blocked_address(&addr));
    let addr = IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1));
    assert!(is_blocked_address(&addr));
}

#[test]
fn test_link_local_v6_blocked() {
    // fe80::/10
    let addr = IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1));
    assert!(is_blocked_address(&addr));
}

#[test]
fn test_v4_mapped_v6_blocked() {
    // ::ffff:10.0.0.1 (maps to private 10.x)
    let addr = IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x0a00, 0x0001));
    assert!(is_blocked_address(&addr));
}

#[test]
fn test_v4_mapped_v6_loopback_allowed() {
    // ::ffff:127.0.0.1
    let addr = IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x7f00, 0x0001));
    assert!(!is_blocked_address(&addr));
}

#[test]
fn test_v4_mapped_v6_metadata_blocked() {
    // ::ffff:169.254.169.254
    let addr = IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0xa9fe, 0xa9fe));
    assert!(is_blocked_address(&addr));
}

#[test]
fn test_global_v6_allowed() {
    let addr = IpAddr::V6(Ipv6Addr::new(0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888));
    assert!(!is_blocked_address(&addr));
}

// -----------------------------------------------------------------------
// extract_host
// -----------------------------------------------------------------------

#[test]
fn test_extract_host_simple() {
    assert_eq!(
        extract_host("https://example.com/path"),
        Some("example.com")
    );
}

#[test]
fn test_extract_host_with_port() {
    assert_eq!(
        extract_host("http://example.com:8080/path"),
        Some("example.com")
    );
}

#[test]
fn test_extract_host_ip_literal() {
    assert_eq!(extract_host("http://192.168.1.1/path"), Some("192.168.1.1"));
}

#[test]
fn test_extract_host_ipv6_literal() {
    assert_eq!(extract_host("http://[::1]:8080/path"), Some("::1"));
}

#[test]
fn test_extract_host_with_userinfo() {
    assert_eq!(
        extract_host("http://user:pass@example.com/path"),
        Some("example.com")
    );
}

#[test]
fn test_extract_host_no_scheme() {
    assert_eq!(extract_host("not-a-url"), None);
}

// -----------------------------------------------------------------------
// url_matches_allowlist
// -----------------------------------------------------------------------

#[test]
fn test_allowlist_empty_allows_all() {
    assert!(url_matches_allowlist("https://anything.com", &[]));
}

#[test]
fn test_allowlist_exact_match() {
    let allowed = vec!["https://example.com/hook".to_string()];
    assert!(url_matches_allowlist("https://example.com/hook", &allowed));
    assert!(!url_matches_allowlist("https://other.com/hook", &allowed));
}

#[test]
fn test_allowlist_wildcard() {
    let allowed = vec!["https://*.example.com/*".to_string()];
    assert!(url_matches_allowlist(
        "https://api.example.com/hook",
        &allowed
    ));
    assert!(!url_matches_allowlist("https://evil.com/hook", &allowed));
}

#[test]
fn test_allowlist_multiple_patterns() {
    let allowed = vec![
        "https://api.example.com/*".to_string(),
        "https://hooks.internal.co/*".to_string(),
    ];
    assert!(url_matches_allowlist(
        "https://api.example.com/v1/hook",
        &allowed
    ));
    assert!(url_matches_allowlist(
        "https://hooks.internal.co/webhook",
        &allowed
    ));
    assert!(!url_matches_allowlist("https://evil.com/hook", &allowed));
}

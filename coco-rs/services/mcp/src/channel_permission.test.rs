use pretty_assertions::assert_eq;

use super::*;

// ── ChannelPermission serialization ──

#[test]
fn test_channel_permission_serialize_roundtrip() {
    let perm = ChannelPermission {
        server_name: "slack-server".to_string(),
        channel: "#general".to_string(),
        allowed: true,
    };

    let json = serde_json::to_value(&perm).expect("serialize");
    assert_eq!(json["server_name"], "slack-server");
    assert_eq!(json["channel"], "#general");
    assert_eq!(json["allowed"], true);

    let back: ChannelPermission = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back, perm);
}

#[test]
fn test_channel_permission_denied_roundtrip() {
    let perm = ChannelPermission {
        server_name: "github-mcp".to_string(),
        channel: "my-org/private-repo".to_string(),
        allowed: false,
    };

    let json = serde_json::to_value(&perm).expect("serialize");
    assert_eq!(json["allowed"], false);

    let back: ChannelPermission = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back, perm);
}

// ── DenyAllRelay ──

#[test]
fn test_deny_all_relay_check() {
    let relay = DenyAllRelay;
    assert!(!relay.check_permission("any-server", "any-channel"));
}

#[tokio::test]
async fn test_deny_all_relay_request() {
    let relay = DenyAllRelay;
    assert!(!relay.request_permission("any-server", "any-channel").await);
}

// ── StaticPermissionRelay ──

#[test]
fn test_static_relay_check_granted() {
    let relay = StaticPermissionRelay::new(vec![ChannelPermission {
        server_name: "slack".to_string(),
        channel: "#dev".to_string(),
        allowed: true,
    }]);

    assert!(relay.check_permission("slack", "#dev"));
}

#[test]
fn test_static_relay_check_denied_by_flag() {
    let relay = StaticPermissionRelay::new(vec![ChannelPermission {
        server_name: "slack".to_string(),
        channel: "#secret".to_string(),
        allowed: false,
    }]);

    assert!(!relay.check_permission("slack", "#secret"));
}

#[test]
fn test_static_relay_check_unknown_returns_false() {
    let relay = StaticPermissionRelay::new(vec![ChannelPermission {
        server_name: "slack".to_string(),
        channel: "#dev".to_string(),
        allowed: true,
    }]);

    assert!(!relay.check_permission("slack", "#other"));
    assert!(!relay.check_permission("other-server", "#dev"));
}

#[test]
fn test_static_relay_empty_permissions() {
    let relay = StaticPermissionRelay::new(vec![]);
    assert!(!relay.check_permission("any", "any"));
}

#[tokio::test]
async fn test_static_relay_request_always_false() {
    let relay = StaticPermissionRelay::new(vec![ChannelPermission {
        server_name: "slack".to_string(),
        channel: "#dev".to_string(),
        allowed: true,
    }]);

    // request_permission always returns false (no interactive prompting)
    assert!(!relay.request_permission("slack", "#dev").await);
}

#[test]
fn test_static_relay_multiple_permissions() {
    let relay = StaticPermissionRelay::new(vec![
        ChannelPermission {
            server_name: "slack".to_string(),
            channel: "#dev".to_string(),
            allowed: true,
        },
        ChannelPermission {
            server_name: "slack".to_string(),
            channel: "#ops".to_string(),
            allowed: true,
        },
        ChannelPermission {
            server_name: "github".to_string(),
            channel: "my-org/public".to_string(),
            allowed: true,
        },
        ChannelPermission {
            server_name: "github".to_string(),
            channel: "my-org/private".to_string(),
            allowed: false,
        },
    ]);

    assert!(relay.check_permission("slack", "#dev"));
    assert!(relay.check_permission("slack", "#ops"));
    assert!(relay.check_permission("github", "my-org/public"));
    assert!(!relay.check_permission("github", "my-org/private"));
    assert!(!relay.check_permission("unknown", "#dev"));
}

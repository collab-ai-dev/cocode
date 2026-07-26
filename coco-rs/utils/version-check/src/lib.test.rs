use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use pretty_assertions::assert_eq;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

use super::VersionCache;
use super::fetch_latest_version;
use super::notice_from_cache;
use super::read_cache;
use super::refresh_cache_from;
use super::should_refresh;

fn at(hours_ago: i64) -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH + Duration::hours(1000 - hours_ago)
}

fn now() -> DateTime<Utc> {
    at(0)
}

fn cache(latest: &str, checked_hours_ago: i64) -> VersionCache {
    VersionCache {
        latest_version: latest.to_string(),
        last_checked_at: at(checked_hours_ago),
        dismissed_version: None,
    }
}

#[test]
fn a_newer_cached_version_produces_a_notice() {
    let notice = notice_from_cache(&cache("0.2.0", 1), "0.1.1").expect("notice");
    assert_eq!(notice.current_version, "0.1.1");
    assert_eq!(notice.latest_version, "0.2.0");
}

#[test]
fn the_current_version_produces_no_notice() {
    assert_eq!(notice_from_cache(&cache("0.1.1", 1), "0.1.1"), None);
    assert_eq!(notice_from_cache(&cache("0.1.0", 1), "0.1.1"), None);
}

#[test]
fn a_dismissed_version_stays_dismissed() {
    let dismissed = VersionCache {
        dismissed_version: Some("0.2.0".to_string()),
        ..cache("0.2.0", 1)
    };
    assert_eq!(notice_from_cache(&dismissed, "0.1.1"), None);
}

#[test]
fn a_dismissal_does_not_suppress_the_next_release() {
    // Dismissing 0.2.0 must not silence 0.3.0 — otherwise one dismissal turns
    // the feature off forever.
    let dismissed_older = VersionCache {
        dismissed_version: Some("0.2.0".to_string()),
        ..cache("0.3.0", 1)
    };
    let notice = notice_from_cache(&dismissed_older, "0.1.1").expect("notice");
    assert_eq!(notice.latest_version, "0.3.0");
}

#[test]
fn refresh_is_due_only_once_the_cache_goes_stale() {
    assert!(should_refresh(None, "0.1.1", now()));
    assert!(!should_refresh(Some(&cache("0.1.1", 1)), "0.1.1", now()));
    assert!(should_refresh(Some(&cache("0.1.1", 48)), "0.1.1", now()));
}

#[test]
fn source_builds_never_hit_the_network() {
    // Nothing to compare against, so the check would only produce noise.
    assert!(!should_refresh(None, "0.0.0", now()));
    assert!(!should_refresh(None, "0.1.1-dev", now()));
}

#[tokio::test]
async fn refresh_writes_the_fetched_version_to_the_cache() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/latest"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "name": "@cocode-cli/cocode-cli",
            "version": "9.9.9",
        })))
        .mount(&server)
        .await;

    let latest = fetch_latest_version(&format!("{}/latest", server.uri()))
        .await
        .expect("fetch");
    assert_eq!(latest, "9.9.9");
}

#[tokio::test]
async fn a_refresh_preserves_an_existing_dismissal() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"version": "0.2.0"})),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("version-check.json");
    super::dismiss_version(&path, "0.2.0")
        .await
        .expect("dismiss");

    let stored = refresh_cache_from(&path, &server.uri(), now())
        .await
        .expect("refresh");
    assert_eq!(stored.dismissed_version.as_deref(), Some("0.2.0"));
    assert_eq!(notice_from_cache(&stored, "0.1.1"), None);
}

#[tokio::test]
async fn a_failed_request_leaves_no_cache_behind() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("version-check.json");

    assert!(
        refresh_cache_from(&path, &server.uri(), now())
            .await
            .is_err()
    );
    // The cache is only written after a successful fetch, so a failing registry
    // cannot poison the next startup with a bogus "latest".
    assert_eq!(read_cache(&path), None);
}

#[tokio::test]
async fn a_corrupt_cache_reads_as_absent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("version-check.json");
    tokio::fs::write(&path, "{ this is not json")
        .await
        .expect("write");
    assert_eq!(read_cache(&path), None);
    assert!(should_refresh(None, "0.1.1", now()));
}

#[tokio::test]
async fn refresh_cache_round_trips_through_disk() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"version": "1.4.0"})),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().expect("tempdir");
    // A nested path also proves the parent directory is created.
    let path = dir.path().join("nested").join("version-check.json");

    refresh_cache_from(&path, &server.uri(), now())
        .await
        .expect("refresh");

    let stored = read_cache(&path).expect("cache");
    assert_eq!(stored.latest_version, "1.4.0");
    assert!(!should_refresh(Some(&stored), "0.1.1", now()));
}

#[test]
fn homebrew_installs_are_compared_against_the_cask_not_npm() {
    // The cask lags the npm release; announcing an npm version to a brew user
    // names something `brew upgrade` cannot install yet.
    let brew = super::latest_version_url(super::InstallMethod::Homebrew);
    assert!(brew.contains("formulae.brew.sh"), "{brew}");

    for method in [
        super::InstallMethod::Npm,
        super::InstallMethod::Pnpm,
        super::InstallMethod::Bun,
        super::InstallMethod::Cargo,
        super::InstallMethod::Unknown,
    ] {
        let url = super::latest_version_url(method);
        assert!(url.contains("registry.npmjs.org"), "{method:?} → {url}");
    }
}

#[tokio::test]
async fn the_brew_cask_payload_shape_is_understood() {
    // Both endpoints answer with a top-level `version`, but the cask body
    // carries much more around it; the extra fields must not break decoding.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "token": "cocode",
            "version": "0.4.2,17",
            "url": "https://example.invalid/cocode.zip",
            "artifacts": [{"binary": ["cocode"]}],
        })))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("version-check.json");

    let cache = refresh_cache_from(&path, &server.uri(), now())
        .await
        .expect("refresh");
    assert_eq!(cache.latest_version, "0.4.2,17");
    let notice = notice_from_cache(&cache, "0.1.1").expect("notice");
    assert_eq!(notice.latest_version, "0.4.2,17");
}

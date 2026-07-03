use super::*;

use pretty_assertions::assert_eq;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path as match_path;

/// Serve `body` at `GET /model.bin` (wiremock sets `Content-Length`).
async fn serve(body: &[u8]) -> (MockServer, String) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(match_path("/model.bin"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
        .mount(&server)
        .await;
    let url = format!("{}/model.bin", server.uri());
    (server, url)
}

fn request(url: String, dest: PathBuf) -> DownloadRequest {
    DownloadRequest {
        url,
        dest,
        expected_sha256: None,
        expected_size: None,
        user_agent: "coco-utils-download-test/1".to_string(),
        restrict_to_public: false,
    }
}

#[test]
fn temp_path_appends_part_suffix() {
    assert_eq!(
        temp_path(Path::new("/a/b/ggml-base.en.bin")),
        PathBuf::from("/a/b/ggml-base.en.bin.part")
    );
}

#[tokio::test]
async fn downloads_and_installs_atomically_with_progress() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("model.bin");
    let body = b"hello whisper weights, a few bytes".to_vec();
    let (server, url) = serve(&body).await;
    let sha = hex::encode(Sha256::digest(&body));

    let req = DownloadRequest {
        expected_sha256: Some(sha),
        expected_size: Some(body.len() as u64),
        ..request(url, dest.clone())
    };
    let (tx, mut rx) = mpsc::channel(64);
    download_file(req, Some(tx), CancellationToken::new())
        .await
        .expect("download should succeed");

    assert_eq!(std::fs::read(&dest).unwrap(), body);
    assert!(
        !temp_path(&dest).exists(),
        "the .part temp must be renamed away"
    );

    let mut started = false;
    let mut done_total = None;
    while let Ok(ev) = rx.try_recv() {
        match ev {
            DownloadProgress::Started { .. } => started = true,
            DownloadProgress::Done { total } => done_total = Some(total),
            _ => {}
        }
    }
    assert!(started, "expected a Started event");
    assert_eq!(done_total, Some(body.len() as u64));
    drop(server);
}

#[tokio::test]
async fn checksum_mismatch_leaves_no_file_behind() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("model.bin");
    let (server, url) = serve(b"the real body").await;

    let req = DownloadRequest {
        expected_sha256: Some("00".repeat(32)), // valid length, wrong digest
        ..request(url, dest.clone())
    };
    let err = download_file(req, None, CancellationToken::new())
        .await
        .expect_err("checksum should fail");

    assert!(matches!(err, DownloadError::ChecksumMismatch { .. }));
    assert!(!dest.exists(), "dest must not exist on checksum failure");
    assert!(
        !temp_path(&dest).exists(),
        "the .part temp must be cleaned up"
    );
    drop(server);
}

#[tokio::test]
async fn pre_cancelled_download_returns_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("model.bin");
    let (server, url) = serve(b"anything").await;
    let cancel = CancellationToken::new();
    cancel.cancel();

    let err = download_file(request(url, dest.clone()), None, cancel)
        .await
        .expect_err("cancelled should error");

    assert!(matches!(err, DownloadError::Cancelled));
    assert!(!dest.exists());
    drop(server);
}

#[tokio::test]
async fn non_success_status_is_reported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("model.bin");

    let err = download_file(
        request(format!("{}/missing", server.uri()), dest.clone()),
        None,
        CancellationToken::new(),
    )
    .await
    .expect_err("404 should error");

    assert!(matches!(err, DownloadError::Status { status: 404, .. }));
    assert!(!dest.exists());
}

#[tokio::test]
async fn restrict_blocks_link_local_metadata_ip() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("model.bin");
    // Cloud metadata endpoint — must be rejected before any connection.
    let req = DownloadRequest {
        restrict_to_public: true,
        ..request(
            "http://169.254.169.254/latest/meta-data".to_string(),
            dest.clone(),
        )
    };
    let err = download_file(req, None, CancellationToken::new())
        .await
        .expect_err("link-local host must be blocked");
    assert!(matches!(err, DownloadError::BlockedHost { .. }));
    assert!(!dest.exists());
}

#[tokio::test]
async fn restrict_blocks_private_and_loopback_and_bad_scheme() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("model.bin");
    for url in [
        "http://10.0.0.5/x",
        "http://127.0.0.1/x",
        "http://[::1]/x",
        "file:///etc/passwd",
    ] {
        let req = DownloadRequest {
            restrict_to_public: true,
            ..request(url.to_string(), dest.clone())
        };
        let err = download_file(req, None, CancellationToken::new())
            .await
            .expect_err("must block");
        assert!(
            matches!(err, DownloadError::BlockedHost { .. }),
            "expected BlockedHost for {url}, got {err:?}"
        );
    }
}

#[test]
fn blocked_host_allows_public_and_hostnames_but_not_private() {
    // Public IP + hostname pass; private/loopback/link-local IP literals blocked.
    assert!(blocked_host(&reqwest::Url::parse("https://huggingface.co/x").unwrap()).is_none());
    assert!(blocked_host(&reqwest::Url::parse("https://8.8.8.8/x").unwrap()).is_none());
    assert!(blocked_host(&reqwest::Url::parse("http://169.254.169.254/x").unwrap()).is_some());
    assert!(blocked_host(&reqwest::Url::parse("http://192.168.1.1/x").unwrap()).is_some());
}

#[tokio::test]
async fn content_length_size_mismatch_fails_before_transfer() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("model.bin");
    let (server, url) = serve(b"12345").await; // 5 bytes

    let req = DownloadRequest {
        expected_size: Some(999),
        ..request(url, dest.clone())
    };
    let err = download_file(req, None, CancellationToken::new())
        .await
        .expect_err("size mismatch should fail");

    assert!(matches!(
        err,
        DownloadError::SizeMismatch {
            expected: 999,
            actual: 5
        }
    ));
    assert!(!dest.exists());
    drop(server);
}

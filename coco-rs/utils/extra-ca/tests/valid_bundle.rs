use std::io::Read;
use std::io::Write;
use std::net::SocketAddr;
use std::net::TcpListener;
use std::sync::Arc;

use rustls::ServerConfig;
use rustls::ServerConnection;
use rustls::StreamOwned;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::PrivateKeyDer;
use rustls::pki_types::pem::PemObject;

const HOST: &str = "foobar.com";
const CHAIN: &[u8] = include_bytes!("fixtures/chain.pem");
const KEY: &[u8] = include_bytes!("fixtures/end.key");
const RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";

type TestResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn spawn_https_server(
    connection_count: usize,
) -> TestResult<(SocketAddr, std::thread::JoinHandle<TestResult<()>>)> {
    let certificates = CertificateDer::pem_slice_iter(CHAIN).collect::<Result<Vec<_>, _>>()?;
    let private_key = PrivateKeyDer::from_pem_slice(KEY)?;
    let provider = rustls::crypto::ring::default_provider();
    let config = ServerConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certificates, private_key)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let task = std::thread::spawn(move || -> TestResult<()> {
        for stream in listener.incoming().take(connection_count) {
            let connection = ServerConnection::new(Arc::new(config.clone()))?;
            let mut stream = StreamOwned::new(connection, stream?);
            let mut request = [0; 1024];
            if stream.read(&mut request).is_ok_and(|read| read > 0) {
                stream.write_all(RESPONSE)?;
                stream.flush()?;
            }
        }
        Ok(())
    });
    Ok((address, task))
}

#[test]
fn configured_bundle_establishes_async_and_blocking_tls() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ca.pem");
    std::fs::write(&path, include_bytes!("fixtures/root.pem")).expect("write fixture");

    // SAFETY: this integration-test binary has one test, and the variable is
    // set before either the server thread or Tokio runtime is created.
    unsafe {
        std::env::set_var(coco_utils_extra_ca::ENV_COCO_EXTRA_CA_BUNDLE, &path);
    }

    let (address, server) = spawn_https_server(3).expect("spawn HTTPS server");
    let url = format!("https://{HOST}:{}/", address.port());

    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    let unconfigured = reqwest::Client::builder()
        .no_proxy()
        .resolve(HOST, address)
        .build()
        .expect("unconfigured client");
    let error = runtime
        .block_on(unconfigured.get(&url).send())
        .expect_err("private CA must be rejected before configuration");
    assert!(error.is_connect());

    assert_eq!(coco_utils_extra_ca::extra_root_ders().len(), 1);
    let async_client = coco_utils_extra_ca::client_builder()
        .no_proxy()
        .resolve(HOST, address)
        .build()
        .expect("async client with configured root");
    let response = runtime
        .block_on(async_client.get(&url).send())
        .expect("async TLS request");
    assert_eq!(runtime.block_on(response.text()).unwrap(), "ok");
    drop(runtime);

    let blocking_client = coco_utils_extra_ca::with_extra_root_certificates_blocking(
        reqwest::blocking::Client::builder()
            .no_proxy()
            .resolve(HOST, address),
    )
    .build()
    .expect("blocking client with configured root");
    assert_eq!(
        blocking_client.get(url).send().unwrap().text().unwrap(),
        "ok"
    );

    server
        .join()
        .expect("HTTPS server thread")
        .expect("HTTPS server");
}

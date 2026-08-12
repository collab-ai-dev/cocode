use super::*;
use coco_mcp_types::ContentBlock;
use pretty_assertions::assert_eq;
use rmcp::model::CallToolResult as RmcpCallToolResult;
use serde_json::json;

use serial_test::serial;
use std::ffi::OsString;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::sync::Arc;

use rustls::ServerConfig;
use rustls::ServerConnection;
use rustls::StreamOwned;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::PrivateKeyDer;
use rustls::pki_types::pem::PemObject;

const TEST_ROOT: &[u8] = include_bytes!("../../../utils/extra-ca/tests/fixtures/root.pem");
const TEST_CHAIN: &[u8] = include_bytes!("../../../utils/extra-ca/tests/fixtures/chain.pem");
const TEST_KEY: &[u8] = include_bytes!("../../../utils/extra-ca/tests/fixtures/end.key");

struct EnvVarGuard {
    key: String,
    original: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &str, value: &str) -> Self {
        let original = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value);
        }
        Self {
            key: key.to_string(),
            original,
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(value) = &self.original {
            unsafe {
                std::env::set_var(&self.key, value);
            }
        } else {
            unsafe {
                std::env::remove_var(&self.key);
            }
        }
    }
}

#[tokio::test]
async fn reqwest_013_adapter_establishes_tls_with_extra_root() {
    let certificates = CertificateDer::pem_slice_iter(TEST_CHAIN)
        .collect::<Result<Vec<_>, _>>()
        .expect("certificate chain");
    let private_key = PrivateKeyDer::from_pem_slice(TEST_KEY).expect("private key");
    let provider = rustls::crypto::ring::default_provider();
    let config = ServerConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_no_client_auth()
        .with_single_cert(certificates, private_key)
        .expect("server certificate");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    let address = listener.local_addr().expect("server address");
    let server = std::thread::spawn(move || {
        let connection = ServerConnection::new(Arc::new(config)).expect("TLS server");
        let stream = listener.accept().expect("TCP connection").0;
        let mut stream = StreamOwned::new(connection, stream);
        let mut request = [0; 1024];
        let read = stream.read(&mut request).expect("read request");
        assert!(read > 0, "client closed before sending an HTTP request");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .expect("write response");
    });

    let roots = CertificateDer::pem_slice_iter(TEST_ROOT)
        .map(|certificate| certificate.unwrap().as_ref().to_vec())
        .collect::<Vec<_>>();
    let client = with_extra_root_ders(ClientBuilder::new(), &roots)
        .no_proxy()
        .resolve("foobar.com", address)
        .build()
        .expect("reqwest 0.13 client");
    let body = client
        .get(format!("https://foobar.com:{}/", address.port()))
        .send()
        .await
        .expect("TLS request")
        .text()
        .await
        .expect("response body");

    assert_eq!(body, "ok");
    server.join().expect("HTTPS server");
}

#[tokio::test]
async fn create_env_honors_overrides() {
    let value = "custom".to_string();
    let env = create_env_for_mcp_server(Some(HashMap::from([("TZ".into(), value.clone())])), &[]);
    assert_eq!(env.get("TZ"), Some(&value));
}

#[test]
#[serial(extra_rmcp_env)]
fn create_env_includes_additional_whitelisted_variables() {
    let custom_var = "EXTRA_RMCP_ENV";
    let value = "from-env";
    let _guard = EnvVarGuard::set(custom_var, value);
    let env = create_env_for_mcp_server(None, &[custom_var.to_string()]);
    assert_eq!(env.get(custom_var), Some(&value.to_string()));
}

#[test]
fn convert_call_tool_result_defaults_missing_content() -> Result<()> {
    let structured_content = json!({ "key": "value" });
    // rmcp 1.7's CallToolResult is #[non_exhaustive], so build via a
    // constructor then clear `content` (structured_error mirrors the value
    // into content; this test specifically exercises the empty-content path).
    let mut rmcp_result = RmcpCallToolResult::structured_error(structured_content.clone());
    rmcp_result.content = vec![];

    let result = convert_call_tool_result(rmcp_result)?;

    assert!(result.content.is_empty());
    assert_eq!(result.structured_content, Some(structured_content));
    assert_eq!(result.is_error, Some(true));

    Ok(())
}

#[test]
fn convert_call_tool_result_preserves_existing_content() -> Result<()> {
    let rmcp_result = RmcpCallToolResult::success(vec![rmcp::model::Content::text("hello")]);

    let result = convert_call_tool_result(rmcp_result)?;

    assert_eq!(result.content.len(), 1);
    match &result.content[0] {
        ContentBlock::TextContent(text_content) => {
            assert_eq!(text_content.text, "hello");
            assert_eq!(text_content.r#type, "text");
        }
        other => panic!("expected text content got {other:?}"),
    }
    assert_eq!(result.structured_content, None);
    assert_eq!(result.is_error, Some(false));

    Ok(())
}

//! API posting and getting utilities.

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use reqwest::Response;
use serde_json::Value;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::APICallError;
use vercel_ai_provider::WireTapHandle;

use crate::response_handler::ResponseHandler;

/// Type alias for boxed byte streams.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

/// Convert an APICallError to AISdkError.
fn api_error_to_sdk_error(error: APICallError) -> AISdkError {
    let message = error.message.clone();
    AISdkError::new(message).with_cause(Box::new(error))
}

/// Check if the cancellation token is triggered.
fn check_cancelled(signal: &Option<CancellationToken>) -> Result<(), AISdkError> {
    if let Some(token) = signal
        && token.is_cancelled()
    {
        return Err(AISdkError::new("Request was cancelled"));
    }
    Ok(())
}

/// Get or create a default HTTP client.
fn get_client(client: &Option<Arc<reqwest::Client>>) -> reqwest::Client {
    client.as_ref().map(|c| (**c).clone()).unwrap_or_default()
}

/// POST JSON data to an API endpoint.
pub async fn post_json_to_api<T>(
    url: &str,
    headers: Option<HashMap<String, String>>,
    body: &Value,
    success_handler: impl ResponseHandler<T>,
    error_handler: impl ResponseHandler<AISdkError>,
    abort_signal: Option<CancellationToken>,
) -> Result<T, AISdkError> {
    post_json_to_api_with_client(
        url,
        headers,
        body,
        success_handler,
        error_handler,
        abort_signal,
        None,
    )
    .await
}

/// POST JSON data to an API endpoint with a custom client.
///
/// # Arguments
///
/// * `url` - The API endpoint URL.
/// * `headers` - Optional headers to include in the request.
/// * `body` - The JSON body to send.
/// * `success_handler` - Handler for successful responses.
/// * `error_handler` - Handler for error responses.
/// * `abort_signal` - Optional cancellation token.
/// * `client` - Optional custom HTTP client for connection pooling.
pub async fn post_json_to_api_with_client<T>(
    url: &str,
    headers: Option<HashMap<String, String>>,
    body: &Value,
    success_handler: impl ResponseHandler<T>,
    error_handler: impl ResponseHandler<AISdkError>,
    abort_signal: Option<CancellationToken>,
    client: Option<Arc<reqwest::Client>>,
) -> Result<T, AISdkError> {
    check_cancelled(&abort_signal)?;

    let client = get_client(&client);
    let mut request = client.post(url).json(body);

    if let Some(h) = headers {
        for (key, value) in h {
            request = request.header(&key, &value);
        }
    }

    let response = request.send().await.map_err(|e| {
        // Connection/DNS/timeout failures are transient — TS treats
        // APIConnectionError as always-retryable so the backoff loop fires.
        api_error_to_sdk_error(APICallError::retryable(format!("Request failed: {e}"), url))
    })?;

    check_cancelled(&abort_signal)?;
    handle_response(response, url, body.clone(), success_handler, error_handler).await
}

/// GET data from an API endpoint.
pub async fn get_from_api<T>(
    url: &str,
    headers: Option<HashMap<String, String>>,
    success_handler: impl ResponseHandler<T>,
    error_handler: impl ResponseHandler<AISdkError>,
    abort_signal: Option<CancellationToken>,
) -> Result<T, AISdkError> {
    get_from_api_with_client(
        url,
        headers,
        success_handler,
        error_handler,
        abort_signal,
        None,
    )
    .await
}

/// GET data from an API endpoint with a custom client.
///
/// # Arguments
///
/// * `url` - The API endpoint URL.
/// * `headers` - Optional headers to include in the request.
/// * `success_handler` - Handler for successful responses.
/// * `error_handler` - Handler for error responses.
/// * `abort_signal` - Optional cancellation token.
/// * `client` - Optional custom HTTP client for connection pooling.
pub async fn get_from_api_with_client<T>(
    url: &str,
    headers: Option<HashMap<String, String>>,
    success_handler: impl ResponseHandler<T>,
    error_handler: impl ResponseHandler<AISdkError>,
    abort_signal: Option<CancellationToken>,
    client: Option<Arc<reqwest::Client>>,
) -> Result<T, AISdkError> {
    check_cancelled(&abort_signal)?;

    let client = get_client(&client);
    let mut request = client.get(url);

    if let Some(h) = headers {
        for (key, value) in h {
            request = request.header(&key, &value);
        }
    }

    let response = request.send().await.map_err(|e| {
        // Connection/DNS/timeout failures are transient — TS treats
        // APIConnectionError as always-retryable so the backoff loop fires.
        api_error_to_sdk_error(APICallError::retryable(format!("Request failed: {e}"), url))
    })?;

    handle_response(response, url, Value::Null, success_handler, error_handler).await
}

async fn handle_response<T>(
    response: Response,
    url: &str,
    request_body: Value,
    success_handler: impl ResponseHandler<T>,
    error_handler: impl ResponseHandler<AISdkError>,
) -> Result<T, AISdkError> {
    let status = response.status();
    let url = url.to_string();

    if status.is_success() {
        success_handler.handle(response, &url, &request_body).await
    } else {
        let error = error_handler
            .handle(response, &url, &request_body)
            .await
            .unwrap_or_else(|e| {
                api_error_to_sdk_error(
                    APICallError::new(
                        format!("HTTP {status} error, and error handler failed: {e}"),
                        &url,
                    )
                    .with_status(status.as_u16()),
                )
            });
        Err(error)
    }
}

/// POST and return a stream.
pub async fn post_stream_to_api(
    url: &str,
    headers: Option<HashMap<String, String>>,
    body: &Value,
    abort_signal: Option<CancellationToken>,
) -> Result<ByteStream, AISdkError> {
    post_stream_to_api_with_client(url, headers, body, abort_signal, None).await
}

/// POST and return a stream with a custom client.
///
/// # Arguments
///
/// * `url` - The API endpoint URL.
/// * `headers` - Optional headers to include in the request.
/// * `body` - The JSON body to send.
/// * `abort_signal` - Optional cancellation token.
/// * `client` - Optional custom HTTP client for connection pooling.
pub async fn post_stream_to_api_with_client(
    url: &str,
    headers: Option<HashMap<String, String>>,
    body: &Value,
    abort_signal: Option<CancellationToken>,
    client: Option<Arc<reqwest::Client>>,
) -> Result<ByteStream, AISdkError> {
    check_cancelled(&abort_signal)?;

    let client = get_client(&client);
    let mut request = client.post(url).json(body);

    if let Some(h) = headers {
        for (key, value) in h {
            request = request.header(&key, &value);
        }
    }

    let response = request.send().await.map_err(|e| {
        // Connection/DNS/timeout failures are transient — TS treats
        // APIConnectionError as always-retryable so the backoff loop fires.
        api_error_to_sdk_error(APICallError::retryable(format!("Request failed: {e}"), url))
    })?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(api_error_to_sdk_error(
            APICallError::new(format!("HTTP {status}: {body}"), url)
                .with_status(status.as_u16())
                .with_response_body(body),
        ));
    }

    Ok(Box::pin(response.bytes_stream()))
}

/// API response wrapper that includes HTTP response headers.
pub struct ApiResponse<T> {
    /// The parsed response value.
    pub value: T,
    /// HTTP response headers.
    pub headers: HashMap<String, String>,
}

/// Extract response headers into a HashMap.
fn extract_response_headers(response: &Response) -> HashMap<String, String> {
    response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|v| (name.to_string(), v.to_string()))
        })
        .collect()
}

/// POST JSON data to an API endpoint with a custom client, returning response headers.
pub async fn post_json_to_api_with_client_and_headers<T>(
    url: &str,
    headers: Option<HashMap<String, String>>,
    body: &Value,
    success_handler: impl ResponseHandler<T>,
    error_handler: impl ResponseHandler<AISdkError>,
    abort_signal: Option<CancellationToken>,
    client: Option<Arc<reqwest::Client>>,
) -> Result<ApiResponse<T>, AISdkError> {
    check_cancelled(&abort_signal)?;

    let client = get_client(&client);
    let mut request = client.post(url).json(body);

    if let Some(h) = headers {
        for (key, value) in h {
            request = request.header(&key, &value);
        }
    }

    let response = request.send().await.map_err(|e| {
        // Connection/DNS/timeout failures are transient — TS treats
        // APIConnectionError as always-retryable so the backoff loop fires.
        api_error_to_sdk_error(APICallError::retryable(format!("Request failed: {e}"), url))
    })?;

    check_cancelled(&abort_signal)?;

    let response_headers = extract_response_headers(&response);
    let value =
        handle_response(response, url, body.clone(), success_handler, error_handler).await?;
    Ok(ApiResponse {
        value,
        headers: response_headers,
    })
}

/// POST and return a stream with a custom client, also returning response headers.
pub async fn post_stream_to_api_with_client_and_headers(
    url: &str,
    headers: Option<HashMap<String, String>>,
    body: &Value,
    abort_signal: Option<CancellationToken>,
    client: Option<Arc<reqwest::Client>>,
) -> Result<(ByteStream, HashMap<String, String>), AISdkError> {
    check_cancelled(&abort_signal)?;

    let client = get_client(&client);
    let mut request = client.post(url).json(body);

    if let Some(h) = headers {
        for (key, value) in h {
            request = request.header(&key, &value);
        }
    }

    let response = request.send().await.map_err(|e| {
        // Connection/DNS/timeout failures are transient — TS treats
        // APIConnectionError as always-retryable so the backoff loop fires.
        api_error_to_sdk_error(APICallError::retryable(format!("Request failed: {e}"), url))
    })?;

    let status = response.status();
    let response_headers = extract_response_headers(&response);

    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(api_error_to_sdk_error(
            APICallError::new(format!("HTTP {status}: {body}"), url)
                .with_status(status.as_u16())
                .with_response_body(body),
        ));
    }

    Ok((Box::pin(response.bytes_stream()), response_headers))
}

// --- Wire-tap variants -------------------------------------------------
//
// These mirror the helpers above but feed a `WireTap` debug sink the raw
// request bytes and the raw response. They delegate to the untapped
// helpers so error/parse semantics are identical when no tap is
// installed. On success the streaming variants tee every chunk; on a
// non-2xx HTTP failure the error body is recovered from the returned
// `AISdkError` (whose cause is the `APICallError` carrying
// `response_body` + `status_code`) and fed to the sink — so HTTP 4xx/5xx
// bodies are captured on both the streaming and JSON paths.

/// Wrap a byte stream so each chunk is mirrored to `tap` as it flows.
/// Returns the stream unchanged when `tap` is `None` (zero overhead).
pub fn tap_byte_stream(stream: ByteStream, tap: Option<WireTapHandle>) -> ByteStream {
    use futures::StreamExt;
    match tap {
        Some(t) => Box::pin(stream.inspect(move |item| {
            if let Ok(bytes) = item {
                t.on_response_chunk(bytes.as_ref());
            }
        })),
        None => stream,
    }
}

/// Feed the outgoing request to `tap` before send.
fn tap_request(
    tap: &Option<WireTapHandle>,
    url: &str,
    headers: &Option<HashMap<String, String>>,
    body: &Value,
) {
    if let Some(t) = tap {
        let header_map = headers.clone().unwrap_or_default();
        let body_bytes = serde_json::to_vec(body).unwrap_or_default();
        t.on_request(url, &header_map, &body_bytes);
    }
}

/// Feed a failed HTTP response's body to `tap`. The body + status are
/// recovered from the `APICallError` cause that the transport helpers
/// attach on a non-2xx response; if that's absent (a transport-level
/// failure), the error message itself is captured so the dump is never
/// silently empty.
fn tap_error(tap: &Option<WireTapHandle>, err: &AISdkError) {
    let Some(t) = tap else { return };
    if let Some(api) = err
        .cause
        .as_deref()
        .and_then(|c| c.downcast_ref::<APICallError>())
    {
        let status = api.status_code.unwrap_or(0);
        let body = api.response_body.as_deref().unwrap_or(&api.message);
        t.on_response_body(status, &HashMap::new(), body.as_bytes());
    } else {
        t.on_response_body(0, &HashMap::new(), err.message.as_bytes());
    }
}

/// [`post_json_to_api_with_client`] with a wire-tap sink.
#[allow(clippy::too_many_arguments)]
pub async fn post_json_to_api_with_client_tapped<T>(
    url: &str,
    headers: Option<HashMap<String, String>>,
    body: &Value,
    success_handler: impl ResponseHandler<T>,
    error_handler: impl ResponseHandler<AISdkError>,
    abort_signal: Option<CancellationToken>,
    client: Option<Arc<reqwest::Client>>,
    tap: Option<WireTapHandle>,
) -> Result<T, AISdkError> {
    tap_request(&tap, url, &headers, body);
    post_json_to_api_with_client(
        url,
        headers,
        body,
        success_handler,
        error_handler,
        abort_signal,
        client,
    )
    .await
    .inspect_err(|e| tap_error(&tap, e))
}

/// [`post_json_to_api_with_client_and_headers`] with a wire-tap sink.
#[allow(clippy::too_many_arguments)]
pub async fn post_json_to_api_with_client_and_headers_tapped<T>(
    url: &str,
    headers: Option<HashMap<String, String>>,
    body: &Value,
    success_handler: impl ResponseHandler<T>,
    error_handler: impl ResponseHandler<AISdkError>,
    abort_signal: Option<CancellationToken>,
    client: Option<Arc<reqwest::Client>>,
    tap: Option<WireTapHandle>,
) -> Result<ApiResponse<T>, AISdkError> {
    tap_request(&tap, url, &headers, body);
    post_json_to_api_with_client_and_headers(
        url,
        headers,
        body,
        success_handler,
        error_handler,
        abort_signal,
        client,
    )
    .await
    .inspect_err(|e| tap_error(&tap, e))
}

/// [`post_stream_to_api_with_client`] with a wire-tap sink: feeds the
/// request, then tees every response chunk to `tap`.
pub async fn post_stream_to_api_with_client_tapped(
    url: &str,
    headers: Option<HashMap<String, String>>,
    body: &Value,
    abort_signal: Option<CancellationToken>,
    client: Option<Arc<reqwest::Client>>,
    tap: Option<WireTapHandle>,
) -> Result<ByteStream, AISdkError> {
    tap_request(&tap, url, &headers, body);
    let stream = post_stream_to_api_with_client(url, headers, body, abort_signal, client)
        .await
        .inspect_err(|e| tap_error(&tap, e))?;
    Ok(tap_byte_stream(stream, tap))
}

/// [`post_stream_to_api_with_client_and_headers`] with a wire-tap sink.
pub async fn post_stream_to_api_with_client_and_headers_tapped(
    url: &str,
    headers: Option<HashMap<String, String>>,
    body: &Value,
    abort_signal: Option<CancellationToken>,
    client: Option<Arc<reqwest::Client>>,
    tap: Option<WireTapHandle>,
) -> Result<(ByteStream, HashMap<String, String>), AISdkError> {
    tap_request(&tap, url, &headers, body);
    let (stream, response_headers) =
        post_stream_to_api_with_client_and_headers(url, headers, body, abort_signal, client)
            .await
            .inspect_err(|e| tap_error(&tap, e))?;
    Ok((tap_byte_stream(stream, tap), response_headers))
}

/// Error handler trait for API errors.
#[async_trait]
pub trait ErrorHandler: Send + Sync {
    async fn handle(&self, response: Response, url: &str, request_body: &Value) -> AISdkError;
}

/// Default error handler.
pub struct DefaultErrorHandler;

#[async_trait]
impl ErrorHandler for DefaultErrorHandler {
    async fn handle(&self, response: Response, url: &str, _request_body: &Value) -> AISdkError {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        let is_retryable = status.as_u16() == 429 || status.as_u16() >= 500;

        api_error_to_sdk_error(
            APICallError::new(format!("HTTP {status}: {body}"), url)
                .with_status(status.as_u16())
                .with_response_body(body)
                .with_retryable(is_retryable),
        )
    }
}

/// API error type.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Request cancelled")]
    Cancelled,
}

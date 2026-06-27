//! Fetch function types.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

/// A fetch function type.
pub type Fetch = Box<
    dyn Fn(FetchOptions) -> Pin<Box<dyn Future<Output = Result<FetchResponse, FetchError>> + Send>>
        + Send
        + Sync,
>;

/// Options for a fetch request.
#[derive(Debug, Clone)]
pub struct FetchOptions {
    /// The URL to fetch.
    pub url: String,
    /// The HTTP method.
    pub method: HttpMethod,
    /// Request headers.
    pub headers: Option<HashMap<String, String>>,
    /// Request body.
    pub body: Option<Vec<u8>>,
}

impl FetchOptions {
    /// Create a new GET request.
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            method: HttpMethod::Get,
            headers: None,
            body: None,
        }
    }

    /// Create a new POST request.
    pub fn post(url: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            url: url.into(),
            method: HttpMethod::Post,
            headers: None,
            body: Some(body),
        }
    }

    /// Add headers.
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = Some(headers);
        self
    }
}

/// HTTP methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
}

/// Response from a fetch request.
#[derive(Debug)]
pub struct FetchResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response headers.
    pub headers: HashMap<String, String>,
    /// Response body.
    pub body: Vec<u8>,
}

impl FetchResponse {
    /// Check if the response is successful (status 2xx).
    pub fn is_success(&self) -> bool {
        self.status >= 200 && self.status < 300
    }

    /// Get the body as a string.
    pub fn text(&self) -> Result<String, std::string::FromUtf8Error> {
        String::from_utf8(self.body.clone())
    }

    /// Get the body as JSON.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_slice(&self.body)
    }
}

/// Error from a fetch request.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("Network error: {0}")]
    Network(String),
    #[error("HTTP error: {0}")]
    Http(u16),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Create a default fetch function using reqwest.
pub fn default_fetch() -> Fetch {
    Box::new(|options: FetchOptions| {
        Box::pin(async move {
            let client = reqwest::Client::new();
            let mut request = match options.method {
                HttpMethod::Get => client.get(&options.url),
                HttpMethod::Post => client.post(&options.url),
                HttpMethod::Put => client.put(&options.url),
                HttpMethod::Delete => client.delete(&options.url),
                HttpMethod::Patch => client.patch(&options.url),
            };

            if let Some(headers) = options.headers {
                for (key, value) in headers {
                    request = request.header(&key, &value);
                }
            }

            if let Some(body) = options.body {
                request = request.body(body);
            }

            let response = request
                .send()
                .await
                .map_err(|e| FetchError::Network(e.to_string()))?;

            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                .collect();

            let body = response
                .bytes()
                .await
                .map_err(|e| FetchError::Network(e.to_string()))?;

            Ok(FetchResponse {
                status,
                headers,
                body: body.to_vec(),
            })
        })
    })
}

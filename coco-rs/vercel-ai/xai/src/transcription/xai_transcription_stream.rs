//! Real-time streaming transcription over WebSocket (xAI `/stt`).
//!
//! Mirrors the `doStream` path of `XaiTranscriptionModel` in `@ai-sdk/xai`:
//! open a WebSocket to `wss://…/stt?<params>`, and on the server's
//! `transcript.created` greeting start pumping audio frames while forwarding
//! `transcript.partial` / `transcript.final` / `transcript.done` events as
//! [`TranscriptionModelV4StreamPart`]s. A driver task owns the socket and
//! pushes parts through an `mpsc` channel; the returned stream drains it.

use std::collections::BTreeMap;
use std::collections::HashMap;

use futures::SinkExt;
use futures::StreamExt;
use futures::stream::SplitSink;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderName;
use tokio_tungstenite::tungstenite::http::HeaderValue;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::APICallError;
use vercel_ai_provider::AudioChunkStream;
use vercel_ai_provider::TranscriptionInputAudioFormat;
use vercel_ai_provider::TranscriptionModelV4StreamPart;
use vercel_ai_provider::Warning;

use super::xai_transcription_options::XaiTranscriptionProviderOptions;

/// Convert an `http(s)://` base URL to its WebSocket scheme (`ws(s)://`).
/// Mirrors `toWebSocketUrl`.
fn to_ws_scheme(base_url: &str) -> String {
    if let Some(rest) = base_url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base_url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base_url.to_string()
    }
}

/// Map an input-audio media type to the xAI `encoding` value.
/// Mirrors `encodingFromInputAudioFormat`.
fn encoding_from_media_type(media_type: &str) -> &'static str {
    match media_type {
        "audio/pcmu" => "mulaw",
        "audio/pcma" => "alaw",
        _ => "pcm",
    }
}

/// Whether an input-audio media type is one xAI recognizes for raw PCM framing.
/// Mirrors `isKnownInputAudioFormat`.
pub(crate) fn is_known_input_audio_format(media_type: &str) -> bool {
    matches!(media_type, "audio/pcm" | "audio/pcmu" | "audio/pcma")
}

/// Build the `wss://…/stt?<params>` URL. Pure — mirrors
/// `buildXaiStreamingTranscriptionUrl`. `sample_rate` falls back to the input
/// format's rate, and `encoding` to the format's media type.
pub(crate) fn build_streaming_url(
    base_url: &str,
    input_audio_format: &TranscriptionInputAudioFormat,
    opts: &XaiTranscriptionProviderOptions,
) -> Result<String, AISdkError> {
    let ws_base = to_ws_scheme(base_url);
    let mut url = reqwest::Url::parse(&format!("{ws_base}/stt"))
        .map_err(|e| AISdkError::new(format!("xAI streaming transcription: bad base URL: {e}")))?;

    {
        let mut q = url.query_pairs_mut();
        if let Some(sample_rate) = opts.sample_rate.or(input_audio_format.rate) {
            q.append_pair("sample_rate", &sample_rate.to_string());
        }
        let encoding = opts
            .audio_format
            .map(|f| f.as_str().to_string())
            .unwrap_or_else(|| {
                encoding_from_media_type(&input_audio_format.media_type).to_string()
            });
        q.append_pair("encoding", &encoding);

        if let Some(ref language) = opts.language {
            q.append_pair("language", language);
        }
        if let Some(diarize) = opts.diarize {
            q.append_pair("diarize", &diarize.to_string());
        }
        if let Some(filler_words) = opts.filler_words {
            q.append_pair("filler_words", &filler_words.to_string());
        }
        if let Some(multichannel) = opts.multichannel {
            q.append_pair("multichannel", &multichannel.to_string());
        }
        if let Some(channels) = opts.channels {
            q.append_pair("channels", &channels.to_string());
        }
        if let Some(streaming) = &opts.streaming {
            if let Some(interim) = streaming.interim_results {
                q.append_pair("interim_results", &interim.to_string());
            }
            if let Some(endpointing) = streaming.endpointing {
                q.append_pair("endpointing", &endpointing.to_string());
            }
            if let Some(smart_turn) = streaming.smart_turn {
                q.append_pair("smart_turn", &smart_turn.to_string());
            }
            if let Some(timeout) = streaming.smart_turn_timeout {
                q.append_pair("smart_turn_timeout", &timeout.to_string());
            }
        }
        if let Some(keyterm) = &opts.keyterm {
            for term in keyterm.terms() {
                q.append_pair("keyterm", &term);
            }
        }
    }

    Ok(url.to_string())
}

/// Parameters for [`open_streaming_transcription`], threaded from `do_stream`.
pub(crate) struct StreamingParams {
    pub url: String,
    pub headers: HashMap<String, String>,
    pub warnings: Vec<Warning>,
    pub audio: AudioChunkStream,
    pub include_raw: bool,
    pub abort: Option<tokio_util::sync::CancellationToken>,
    /// Language echoed back in the terminal `finish` part.
    pub language: Option<String>,
    /// How many `transcript.done` events to await before finishing (channel
    /// count for multichannel, else 1).
    pub expected_done_count: usize,
}

type WsWriter = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;

/// Connect the WebSocket and spawn the driver task. Returns the part stream.
pub(crate) async fn open_streaming_transcription(
    params: StreamingParams,
) -> Result<
    std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<TranscriptionModelV4StreamPart, AISdkError>> + Send>,
    >,
    AISdkError,
> {
    let mut request = params.url.as_str().into_client_request().map_err(|e| {
        AISdkError::new(format!(
            "xAI streaming transcription: invalid WebSocket URL: {e}"
        ))
    })?;
    {
        let h = request.headers_mut();
        for (key, value) in &params.headers {
            if let (Ok(name), Ok(val)) = (
                HeaderName::from_bytes(key.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                h.insert(name, val);
            }
        }
    }

    let (ws, _resp) = connect_async(request).await.map_err(|e| {
        AISdkError::new(format!("xAI streaming transcription connect failed: {e}")).with_cause(
            Box::new(APICallError::new(e.to_string(), &params.url).with_retryable(true)),
        )
    })?;

    let (tx, rx) = mpsc::channel::<Result<TranscriptionModelV4StreamPart, AISdkError>>(64);
    tokio::spawn(drive(ws, tx, params));

    let stream = futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    Ok(Box::pin(stream))
}

/// Own the socket: forward server events as parts, and — once the server sends
/// `transcript.created` — pump audio frames concurrently.
async fn drive(
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    tx: mpsc::Sender<Result<TranscriptionModelV4StreamPart, AISdkError>>,
    params: StreamingParams,
) {
    let StreamingParams {
        warnings,
        audio,
        include_raw,
        abort,
        language,
        expected_done_count,
        ..
    } = params;

    let (write, mut read) = ws.split();
    let mut write = Some(write);
    let mut audio = Some(audio);
    let mut pump: Option<tokio::task::JoinHandle<()>> = None;
    let mut done_texts: BTreeMap<i64, String> = BTreeMap::new();
    let mut done_duration: Option<f64> = None;

    loop {
        let msg = tokio::select! {
            biased;
            _ = wait_abort(&abort) => {
                let _ = tx
                    .send(Err(AISdkError::new("xAI streaming transcription aborted")))
                    .await;
                break;
            }
            m = read.next() => m,
        };

        let Some(msg) = msg else { break }; // socket closed
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                let _ = tx
                    .send(Err(AISdkError::new(format!(
                        "xAI streaming transcription error: {e}"
                    ))))
                    .await;
                break;
            }
        };

        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
        };

        // safeParseJSON: silently ignore non-JSON payloads.
        let Ok(raw) = serde_json::from_str::<Value>(&text) else {
            continue;
        };

        if include_raw {
            let _ = tx
                .send(Ok(TranscriptionModelV4StreamPart::Raw {
                    raw_value: raw.clone(),
                }))
                .await;
        }

        match raw.get("type").and_then(Value::as_str) {
            Some("transcript.created") => {
                let _ = tx
                    .send(Ok(TranscriptionModelV4StreamPart::StreamStart {
                        warnings: warnings.clone(),
                    }))
                    .await;
                if let (Some(w), Some(a)) = (write.take(), audio.take()) {
                    pump = Some(tokio::spawn(pump_audio(w, a)));
                }
            }
            Some("transcript.partial") => {
                let _ = tx.send(Ok(build_partial(&raw))).await;
            }
            Some("transcript.done") => {
                let channel_index = raw
                    .get("channel_index")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let text = raw
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                done_texts.insert(channel_index, text);
                if let Some(duration) = raw.get("duration").and_then(Value::as_f64) {
                    done_duration = Some(duration);
                }
                if done_texts.len() >= expected_done_count {
                    // BTreeMap iterates channels in ascending order.
                    let full = done_texts.values().cloned().collect::<Vec<_>>().join("\n");
                    let _ = tx
                        .send(Ok(TranscriptionModelV4StreamPart::Finish {
                            text: full,
                            segments: Vec::new(),
                            language: language.clone(),
                            duration_in_seconds: done_duration,
                            provider_metadata: None,
                        }))
                        .await;
                    break;
                }
            }
            Some("error") => {
                // xAI STT errors are terminal: surface the message as a stream
                // error (matches the TS `controller.error`).
                let message = raw
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("xAI STT error")
                    .to_string();
                let _ = tx.send(Err(AISdkError::new(message))).await;
                break;
            }
            _ => {}
        }
    }

    if let Some(pump) = pump {
        pump.abort();
    }
}

/// Await an abort token, or park forever when there is none.
async fn wait_abort(abort: &Option<tokio_util::sync::CancellationToken>) {
    match abort {
        Some(token) => token.cancelled().await,
        None => std::future::pending::<()>().await,
    }
}

/// Build a `transcript-partial` / `transcript-final` part from a raw event.
/// Mirrors the `transcript.partial` branch (`is_final` selects `final`).
fn build_partial(raw: &Value) -> TranscriptionModelV4StreamPart {
    let channel_index = raw.get("channel_index").and_then(Value::as_i64);
    let id = channel_index.map(|c| format!("channel-{c}"));
    let text = raw
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let start = raw.get("start").and_then(Value::as_f64);
    let duration = raw.get("duration").and_then(Value::as_f64);

    if raw.get("is_final").and_then(Value::as_bool) == Some(true) {
        let end_second = match (start, duration) {
            (Some(s), Some(d)) => Some(s + d),
            _ => None,
        };
        TranscriptionModelV4StreamPart::TranscriptFinal {
            id,
            text,
            start_second: start,
            end_second,
            channel_index,
            provider_metadata: None,
        }
    } else {
        TranscriptionModelV4StreamPart::TranscriptPartial {
            id,
            text,
            start_second: start,
            duration_in_seconds: duration,
            channel_index,
            provider_metadata: None,
        }
    }
}

/// Pump audio frames as binary WebSocket messages, then signal `audio.done`.
async fn pump_audio(mut write: WsWriter, mut audio: AudioChunkStream) {
    while let Some(chunk) = audio.next().await {
        if write.send(Message::Binary(chunk.into())).await.is_err() {
            return;
        }
    }
    let _ = write
        .send(Message::Text(r#"{"type":"audio.done"}"#.into()))
        .await;
}

#[cfg(test)]
#[path = "xai_transcription_stream.test.rs"]
mod tests;

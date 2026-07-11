//! Wire-level tests for xAI **streaming** transcription (`do_stream`): a local
//! `ws://` server speaks the xAI `/stt` protocol (greeting → read audio frames
//! + `audio.done` → partial/final/done events), and the real provider decodes
//! it into `TranscriptionModelV4StreamPart`s.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;

use futures::SinkExt;
use futures::StreamExt;
use serde_json::json;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;
use vercel_ai_provider::ProviderOptions;
use vercel_ai_provider::TranscriptionInputAudioFormat;
use vercel_ai_provider::TranscriptionModelV4;
use vercel_ai_provider::TranscriptionModelV4StreamOptions;
use vercel_ai_provider::TranscriptionModelV4StreamPart;
use vercel_ai_xai::XaiProviderSettings;
use vercel_ai_xai::create_xai;

/// Spawn a local server that runs the given per-connection handler once, and
/// return the `http://` base URL (so the provider builds a `ws://…/stt` URL).
async fn serve<F, Fut>(handler: F) -> String
where
    F: FnOnce(tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await
            && let Ok(ws) = accept_async(stream).await
        {
            handler(ws).await;
        }
    });
    format!("http://{addr}/v1")
}

fn audio_stream(chunks: Vec<Vec<u8>>) -> vercel_ai_provider::AudioChunkStream {
    Box::pin(futures::stream::iter(chunks))
}

fn stream_options(base_media_type: &str) -> TranscriptionModelV4StreamOptions {
    TranscriptionModelV4StreamOptions {
        audio: audio_stream(vec![vec![1, 2, 3], vec![4, 5, 6]]),
        input_audio_format: TranscriptionInputAudioFormat::new(base_media_type, Some(16000)),
        provider_options: None,
        abort_signal: None,
        headers: None,
        include_raw_chunks: false,
    }
}

async fn drain(
    mut result: vercel_ai_provider::TranscriptionModelV4StreamResult,
) -> Vec<Result<TranscriptionModelV4StreamPart, vercel_ai_provider::AISdkError>> {
    let mut parts = Vec::new();
    while let Some(part) = result.stream.next().await {
        parts.push(part);
    }
    parts
}

#[tokio::test]
async fn do_stream_decodes_partial_final_and_finish() {
    let base = serve(|mut ws| async move {
        // Greeting starts the client's audio pump.
        ws.send(Message::Text(json!({"type":"transcript.created"}).to_string().into()))
            .await
            .unwrap();
        // Consume audio frames until `audio.done`.
        while let Some(Ok(msg)) = ws.next().await {
            match msg {
                Message::Binary(_) => continue,
                Message::Text(t) if t.contains("audio.done") => break,
                Message::Close(_) => return,
                _ => continue,
            }
        }
        // Interim, then final, then the terminal done.
        ws.send(Message::Text(
            json!({"type":"transcript.partial","text":"hel","start":0.0,"duration":0.2,"is_final":false})
                .to_string().into(),
        )).await.unwrap();
        ws.send(Message::Text(
            json!({"type":"transcript.partial","text":"hello","start":0.0,"duration":0.5,"is_final":true,"channel_index":0})
                .to_string().into(),
        )).await.unwrap();
        ws.send(Message::Text(
            json!({"type":"transcript.done","text":"hello world","channel_index":0,"duration":1.0})
                .to_string().into(),
        )).await.unwrap();
        let _ = ws.close(None).await;
    })
    .await;

    let provider = create_xai(XaiProviderSettings {
        base_url: Some(base),
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let model = provider.transcription("grok-stt");
    let result = model
        .do_stream(stream_options("audio/pcm"))
        .await
        .expect("do_stream");
    let parts = drain(result).await;

    // Ordered shape: stream-start → partial → final → finish.
    let tags: Vec<String> = parts
        .iter()
        .map(|p| match p {
            Ok(part) => serde_json::to_value(part)
                .ok()
                .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(String::from))
                .unwrap_or_else(|| "<unknown>".into()),
            Err(_) => "<error>".into(),
        })
        .collect();
    assert_eq!(
        tags,
        vec![
            "stream-start",
            "transcript-partial",
            "transcript-final",
            "finish"
        ]
    );

    // Value asserts on partial / final / finish.
    let mut saw_partial = false;
    let mut saw_final = false;
    let mut finish = None;
    for part in parts.into_iter().flatten() {
        match part {
            TranscriptionModelV4StreamPart::TranscriptPartial { text, .. } => {
                assert_eq!(text, "hel");
                saw_partial = true;
            }
            TranscriptionModelV4StreamPart::TranscriptFinal {
                text, end_second, ..
            } => {
                assert_eq!(text, "hello");
                assert_eq!(end_second, Some(0.5));
                saw_final = true;
            }
            TranscriptionModelV4StreamPart::Finish {
                text,
                duration_in_seconds,
                ..
            } => finish = Some((text, duration_in_seconds)),
            _ => {}
        }
    }
    assert!(saw_partial && saw_final);
    let (text, duration) = finish.expect("finish part");
    assert_eq!(text, "hello world"); // from transcript.done, not the partials
    assert_eq!(duration, Some(1.0));
}

#[tokio::test]
async fn do_stream_error_event_terminates_with_error() {
    let base = serve(|mut ws| async move {
        ws.send(Message::Text(
            json!({"type":"transcript.created"}).to_string().into(),
        ))
        .await
        .unwrap();
        while let Some(Ok(msg)) = ws.next().await {
            match msg {
                Message::Text(t) if t.contains("audio.done") => break,
                Message::Close(_) => return,
                _ => continue,
            }
        }
        ws.send(Message::Text(
            json!({"type":"error","message":"transcription failed"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        let _ = ws.close(None).await;
    })
    .await;

    let provider = create_xai(XaiProviderSettings {
        base_url: Some(base),
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let model = provider.transcription("grok-stt");
    let result = model
        .do_stream(stream_options("audio/pcm"))
        .await
        .expect("do_stream");
    let parts = drain(result).await;

    let err = parts
        .iter()
        .find_map(|p| p.as_ref().err())
        .expect("a terminal error was yielded");
    assert!(
        err.message.contains("transcription failed"),
        "got: {}",
        err.message
    );
}

#[tokio::test]
async fn do_stream_multichannel_without_channels_errors() {
    // Validation happens before any connection — no server needed.
    let provider = create_xai(XaiProviderSettings {
        base_url: Some("https://api.x.ai/v1".to_string()),
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let model = provider.transcription("grok-stt");

    let mut ns = HashMap::new();
    ns.insert("multichannel".to_string(), json!(true));
    let mut map = HashMap::new();
    map.insert("xai".to_string(), ns);

    let mut opts = stream_options("audio/pcm");
    opts.provider_options = Some(ProviderOptions(map));

    let err = match model.do_stream(opts).await {
        Ok(_) => panic!("multichannel without channels must reject"),
        Err(e) => e,
    };
    assert!(
        err.message.contains("channels is required"),
        "got: {}",
        err.message
    );
}

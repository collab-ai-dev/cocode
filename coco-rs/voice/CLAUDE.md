# coco-voice

Voice input (speech-to-text dictation). Standalone crate (peer to `retrieval` /
`bridge`); Tier-2 errors (`thiserror`, no `coco-error`).

## Key Types

| Type | Purpose |
|------|---------|
| `VoiceEngine` (trait) | Backend-agnostic STT seam. `capabilities()` + async `transcribe(wav, params, cancel)`. Callers branch on `VoiceCapabilities`, never backend identity (mirrors retrieval's `Reranker`). |
| `VoiceCapabilities` | `{ requires_network, on_device, streaming }` — drives the privacy-posture footer. |
| `RemoteOpenAiEngine` | MVP backend. Thin caller of the `coco_inference::transcribe_audio` seam over an injected `Arc<dyn TranscriptionModelV4>`. |
| `LocalWhisperEngine` | Phase-2 on-device backend behind the `local-voice` feature (whisper.cpp via `whisper-rs`). |
| `create_voice_engine(&VoiceConfig, remote_handle)` | Factory (retrieval's `create_reranker` recipe). `Local` without the feature → typed `VoiceError::FeatureNotEnabled`. |
| `VoiceSession` | The state machine app/tui drives: `Idle → Recording → Transcribing → Idle`. `start()` is sync (spawns capture); `stop()` kicks off async transcription and emits on the sink. |
| `VoiceEvent` | Isolated event stream (`RecordingStarted` / `Transcribing` / `Final` / `Error`) — **not** bridged into `CoreEvent`; only the final text touches user input. |
| `VoiceError` | Tier-2 error enum (`NoAudioDevice` / `NoSpeechDetected` / `FeatureNotEnabled` / `TranscriptionFailed` / `Connection` / `Cancelled` / `Capture`). |

## Design invariants

- **On/off is `coco_types::Feature::Voice`, not a config field.** Backend /
  language / model live in `coco_config::VoiceConfig`.
- **No direct `vercel-ai` dep.** Remote STT routes through the
  `services/inference` seam (`transcribe_audio`, `TranscriptionModelV4`,
  `build_openai_transcription_model`) — the seam guard forbids `vercel-ai*`
  outside `common/llm-types` + `services/inference`.
- **Capture is always local** (`coco-utils-audio`), regardless of backend; only
  inference is remote. Audio is normalized to 16 kHz mono WAV.
- **`from_v4` handle injection is mandatory** — the `TranscriptionModel::String`
  path resolves via `get_default_provider()`, which coco never sets. app/cli
  bootstrap builds the handle and injects it.
- Recording finalize + local inference run off the async runtime
  (`spawn_blocking`); the session spawns via a captured `runtime::Handle`.

## Features

- `capture` → enables `coco-utils-audio/cpal` (real microphone). Turned on by
  app/cli's `voice` feature (shipped binary). Off ⇒ capture stub, "no mic".
- `local-voice` → `whisper-rs` + `hound` (on-device Whisper). Off by default.

## Wiring

Bootstrap: `app/cli/src/voice_bootstrap.rs` (gated on `Feature::Voice`).
TUI: `App::with_voice` + the `voice_rx` select arm + `App::toggle_voice`
(intercepts `TuiCommand::VoiceToggle` — the session lives on `App`, not
`AppState`). Keybinding: `voice:pushToTalk` (default `f3`). Commands:
`/voice`, `/voice-lang`.

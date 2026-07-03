# coco-voice

Voice input (speech-to-text dictation). Standalone crate (peer to `retrieval` /
`bridge`); Tier-2 errors (`thiserror`, no `coco-error`).

## Key Types

| Type | Purpose |
|------|---------|
| `VoiceEngine` (trait) | Backend-agnostic STT seam. `capabilities()` + async `transcribe(wav, params, cancel)`. Callers branch on `VoiceCapabilities`, never backend identity (mirrors retrieval's `Reranker`). |
| `VoiceCapabilities` | `{ requires_network, on_device, streaming }` — drives the privacy-posture footer. |
| `RemoteOpenAiEngine` | MVP backend. Thin caller of the `coco_inference::transcribe_audio` seam over an injected `Arc<dyn TranscriptionModelV4>`. |
| `LocalWhisperEngine` | On-device backend behind the `local-voice` feature (whisper.cpp via `whisper-rs`). `new(config)` is cheap+infallible; the model loads **lazily** in `ensure_ctx()` on the first `transcribe` (a `OnceCell`), so `backend=local` never blocks startup. `ensure_ctx` is the single load choke point (and the download-on-first-use hook site). |
| `create_voice_engine(&VoiceConfig, remote_handle)` | Factory (retrieval's `create_reranker` recipe). `Remote` needs the injected handle; `Local` dispatches on `config.local.engine` (`LocalSttEngine`, closed match). `Local` without the `local-voice` feature → typed `VoiceError::FeatureNotEnabled`. |
| `VoiceSession` | The state machine app/tui drives: `Idle → Recording → Transcribing → Idle`. `start()` is sync (spawns capture); `stop()` kicks off async transcription and emits on the sink. |
| `VoiceEvent` | Isolated event stream (`RecordingStarted` / `Transcribing` / `Final` / `Error`) — **not** bridged into `CoreEvent`; only the final text touches user input. |
| `VoiceError` | Tier-2 error enum (`NoAudioDevice` / `NoSpeechDetected` / `FeatureNotEnabled` / `TranscriptionFailed` / `Connection` / `Cancelled` / `Capture`). |

## Design invariants

- **On/off is `coco_types::Feature::Voice`, not a config field.** Backend /
  language / model live in `coco_config::VoiceConfig`.
- **Remote is a `(provider, model)` pair, not bespoke creds.**
  `voice.remote.provider` keys into the providers registry (base_url + auth
  reused from there), so every OpenAI-wire STT host is a providers.json entry.
  Local is a `LocalSttEngine` discriminant + a flat per-engine knob struct
  (`voice.local.whisper.*`) so adding an engine is additive.
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
- `local-voice` → `whisper-rs` + `hound` (on-device Whisper). Off by default;
  needs a C++ toolchain at build time (`cmake` + `libclang`/`clang`). Reachable
  from app/cli via its `voice-local` feature.
- **macOS Metal** is enabled automatically for `local-voice` builds via a
  *target-specific dependency* (`[target.'cfg(target_os="macos")'.dependencies]
  whisper-rs = { features = ["metal"] }`) — NOT a cargo feature, so `--all-
  features` on Linux never tries to build the mac-only Metal toolchain.
  `WhisperContextParameters::default()` uses whatever GPU backend is compiled
  in, so no code change. CoreML is deferred (needs a second encoder artifact).

## Model download

`models.rs` (ungated) is the whisper ggml catalog: `KNOWN_MODELS`
(name → file, pinned **SHA-256**, size — the authoritative HuggingFace git-LFS
values) plus URL/path/request resolution. `download.rs` (ungated) orchestrates a
fetch via `coco-utils-download` (streamed, atomic `*.part`+rename, checksum-
verified, cancellable) and bridges progress onto `VoiceEvent::Download`.

- **Auto-download on first use**: `local::ensure_ctx` fetches missing weights
  when `models::may_auto_download` holds — a *known, checksum-pinned* model,
  `auto_download` on, and no custom `model_url`. That is the trust boundary: a
  project settings file can't point silent auto-download at an unverified URL.
- **Explicit**: `coco_voice::download_whisper_model` (the `/voice-config
  download` path). Works without `local-voice` (pre-staging weights).
- URL priority: `model_url` (full override, unverified) > `download_base` +
  file (mirror, still checksum-verified for known models) > built-in HF base.

## Wiring

Bootstrap: `app/cli/src/voice_bootstrap.rs` (gated on `Feature::Voice`).
TUI: `App::with_voice` + the `voice_rx` select arm + `App::toggle_voice`
(intercepts `TuiCommand::VoiceToggle` — the session lives on `App`, not
`AppState`). Keybinding: `voice:pushToTalk` (default `f3`). Commands:
`/voice` (on/off), `/voice-config` (backend, language, remote provider/model,
local whisper model + weights download).

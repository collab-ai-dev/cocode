# coco-voice — Crate Plan

Directory: `voice/` (v2)
TS source: `voice/` (1 file, 54 LOC), `services/voice.ts` (525 LOC), `services/voiceKeyterms.ts` (106 LOC), `services/voiceStreamSTT.ts` (544 LOC), `hooks/useVoice.ts` (1,144 LOC), `hooks/useVoiceIntegration.tsx` (676 LOC), `hooks/useVoiceEnabled.ts` (25 LOC)
Total: ~3.1K LOC across 7 files

## Dependencies

```
coco-voice depends on:
  - coco-config   (voice enabled setting, OAuth token access)
  - coco-error
  - tokio         (async runtime, channels, timers)
  - tokio-tungstenite (WebSocket STT client)

coco-voice does NOT depend on:
  - coco-tui      (TUI consumes voice, not the reverse)
  - coco-inference (voice uses its own WebSocket, not LLM API)
  - coco-otel     (telemetry injected via callback)
```

## Data Definitions

```rust
/// Feature gate: OAuth + GrowthBook kill-switch.
pub struct VoiceAvailability {
    pub enabled: bool,
    pub has_auth: bool,
    pub gate_enabled: bool,
}

/// Audio recording configuration.
pub struct RecordingConfig {
    pub sample_rate: i32,       // 16000 Hz
    pub channels: i32,          // 1 (mono)
    pub silence_threshold: f32, // 0.03 (3%)
    pub silence_duration_ms: i64, // 2000ms
}

/// Platform-specific audio recorder with fallback chain.
pub enum AudioBackend {
    Native,         // cpal/NAPI equivalent
    Arecord,        // ALSA utils (Linux)
    Sox,            // SoX rec command (Linux/macOS)
}

/// Voice STT WebSocket connection.
pub struct VoiceStreamConnection {
    ws: WebSocketSender,
    keepalive_handle: JoinHandle<()>,
}

/// Transcript event from STT server.
pub enum TranscriptEvent {
    Interim { text: String },
    Final { text: String },
    Error { message: String, fatal: bool },
    Closed,
}

/// Smart finalization source (how the final transcript was obtained).
pub enum FinalizeSource {
    PostCloseStreamEndpoint,
    NoDataTimeout,
    SafetyTimeout,
    WsClose,
    WsAlreadyClosed,
}

/// Hold-to-talk activation state.
pub enum HoldToTalkState {
    Idle,
    Warmup { press_count: i32 },
    Active { prefix: String, suffix: String },
}
```

## Core Logic

### Feature Gating (from `voiceModeEnabled.ts`, 54 LOC)

```rust
/// Check voice availability: OAuth + kill-switch.
pub fn is_voice_mode_enabled(config: &Config) -> VoiceAvailability;
```

### Audio Recording (from `voice.ts`, 525 LOC)

```rust
/// Multi-platform audio recording with fallback chain.
pub struct AudioRecorder;

impl AudioRecorder {
    /// Detect available recording backend: Native > arecord > SoX.
    pub async fn detect_backend() -> Option<AudioBackend>;

    /// Check recording dependencies and suggest install commands.
    pub async fn check_dependencies() -> RecordingDependencies;

    /// Start recording, emitting PCM chunks via channel.
    pub async fn start_recording(
        config: &RecordingConfig,
        chunk_tx: mpsc::Sender<Vec<u8>>,
        cancel: CancellationToken,
    ) -> Result<()>;

    /// Request microphone permission (macOS TCC dialog).
    pub async fn request_microphone_permission() -> bool;
}
```

### Voice Stream STT (from `voiceStreamSTT.ts`, 544 LOC)

```rust
/// WebSocket client for Anthropic voice_stream speech-to-text.
pub struct VoiceStream;

impl VoiceStream {
    /// Connect to wss://api.anthropic.com/api/ws/ with OAuth.
    pub async fn connect(
        oauth_token: &str,
        keyterms: &[String],
        transcript_tx: mpsc::Sender<TranscriptEvent>,
    ) -> Result<VoiceStreamConnection>;
}

impl VoiceStreamConnection {
    /// Send audio chunk (PCM 16kHz, 16-bit, mono).
    pub fn send(&self, audio_chunk: &[u8]) -> Result<()>;

    /// Signal end of audio, wait for final transcript.
    /// Three resolve triggers: TranscriptEndpoint (~300ms),
    /// no-data timeout (1.5s), safety timeout (5s).
    pub async fn finalize(&self) -> Result<(String, FinalizeSource)>;

    /// Close connection.
    pub fn close(&self);

    pub fn is_connected(&self) -> bool;
}
```

Protocol: binary PCM frames + JSON control messages (KeepAlive every 8s, CloseStream).
Server responses: TranscriptText (interim/final), TranscriptEndpoint, TranscriptError.

### Keyterms (from `voiceKeyterms.ts`, 106 LOC)

```rust
/// Build domain-specific vocabulary hints for STT accuracy.
/// Sources: hardcoded terms, project name, git branch, recent files.
/// Max 50 terms, filtered: 2 < len <= 20.
pub async fn get_voice_keyterms(recent_files: Option<&[String]>) -> Vec<String>;

/// Split camelCase/PascalCase/kebab-case/snake_case identifiers.
pub fn split_identifier(name: &str) -> Vec<String>;
```

### Voice Recording Engine (business logic from `useVoice.ts`, 1,144 LOC)

```rust
/// Core recording state machine: idle → recording → processing → idle.
pub enum VoiceState {
    Idle,
    Recording,
    Processing,
}

/// Voice recording session manager.
/// Orchestrates: audio capture → WebSocket STT → transcript assembly → retry.
pub struct VoiceSession {
    state: VoiceState,
    recorder: AudioRecorder,
    stream: Option<VoiceStreamConnection>,
}

impl VoiceSession {
    /// Start recording: init audio backend, connect WebSocket, begin streaming.
    pub async fn start(
        &mut self,
        config: &VoiceConfig,
        transcript_tx: mpsc::Sender<TranscriptEvent>,
    ) -> Result<()>;

    /// Stop recording: finalize STT, wait for final transcript.
    pub async fn stop(&mut self) -> Result<String>;

    /// Handle auto-repeat key release detection.
    /// Uses FIRST_PRESS_FALLBACK_MS (2000ms) timeout for single-press detection.
    pub fn handle_key_release_detection(&mut self, timestamp_ms: i64) -> bool;
}

/// Compute RMS amplitude from PCM buffer for waveform visualization.
pub fn compute_level(pcm_data: &[i16]) -> f32;

/// Map language names to BCP-47 codes for STT.
pub fn normalize_language_for_stt(language: &str) -> String;
```

Key behaviors:
- Focus-mode continuous sessions with silence timeout
- Retry logic for network failures (reconnect WebSocket)
- Analytics emission for recording events
- Auto-repeat key release detection (distinguishes hold from tap)

### Hold-to-Talk (business logic from `useVoiceIntegration.tsx`, 676 LOC)

```rust
/// Hold-to-talk state machine.
/// Two activation modes:
/// - Modifier+letter (Ctrl+X): activates on first press
/// - Bare chars (space): requires 5 rapid presses (>120ms gap = normal typing)
pub struct HoldToTalkManager {
    state: HoldToTalkState,
    hold_threshold: i32,     // 5
    warmup_threshold: i32,   // 2
}

impl HoldToTalkManager {
    pub fn handle_key_event(&mut self, key: &str, timestamp_ms: i64) -> HoldToTalkAction;
    pub fn strip_trailing(&mut self, input: &str, max_strip: usize) -> (String, usize);
    pub fn reset_anchor(&mut self);
}

pub enum HoldToTalkAction {
    PassThrough,
    Swallow,
    Activate { prefix: String, suffix: String },
    Deactivate,
}
```

## Module Layout

```
voice/
  mod.rs              — pub mod, re-exports
  availability.rs     — feature gate (OAuth + kill-switch)
  recorder.rs         — AudioRecorder, backend detection, fallback chain
  voice_stream.rs     — WebSocket STT client, finalization logic
  session.rs          — VoiceSession state machine (from useVoice.ts: record→process→idle, retry, focus-mode)
  keyterms.rs         — vocabulary hint assembly
  hold_to_talk.rs     — hold-to-talk state machine (extracted from useVoiceIntegration.tsx)
```

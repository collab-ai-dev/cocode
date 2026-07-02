# coco-utils-audio

Microphone capture for voice input, normalized to the single canonical STT
format (16 kHz mono 16-bit PCM WAV). Tier-2 (`thiserror`). The only crate that
opens a mic — keeps device code out of core/services (mirrors `utils/pty`
wrapping `portable-pty`).

## Key Types

| Type | Purpose |
|------|---------|
| `AudioCapture` (trait) | `is_available()` (enumerates without opening — no premature macOS TCC prompt) + `start() -> Box<dyn RecordingHandle>`. |
| `RecordingHandle` (trait) | `stop()` **blocks** until the stream drains and encodes to WAV bytes — call off the async runtime. |
| `default_capture()` | Factory: `CpalCapture` when the `cpal` feature is compiled, else an unsupported stub (`is_available() == false`). |
| `AudioCaptureError` | `NotCompiled` / `NoInputDevice` / `UnsupportedFormat` / `NoAudioCaptured` / `Backend` / `Encode`. |
| `encode_wav_16k_mono`, `resample_to_16k`, `TARGET_SAMPLE_RATE` | Pure resample + WAV encode (always compiled; `hound` is pure Rust). |

## Design notes

- **cpal's `Stream` is `!Send`** on several platforms, so all cpal objects live
  on one dedicated OS thread (`coco-audio-capture`); the async side talks to it
  purely over `std::sync::mpsc`. `start()` reports device/permission failure
  synchronously (before returning) via a ready channel.
- Multi-channel input is downmixed to mono, any device sample rate is
  linear-resampled to 16 kHz, and f32/i16/u16 sample formats are handled.
- **`cpal` is optional** (feature `cpal`) so the default workspace build pulls
  no platform audio system-deps (ALSA/CoreAudio/WASAPI). Enabled transitively
  by `coco-voice/capture` ← app/cli's `voice` feature.

//! Pure resample + WAV encoding. No system deps — always compiled.
//!
//! Both remote (OpenAI-wire) and local (whisper.cpp) STT accept 16 kHz mono
//! PCM, so capture is normalized to that single canonical format here.

use crate::error::AudioCaptureError;

/// Canonical STT sample rate (Whisper-native; accepted by OpenAI transcribe).
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Linear-resample mono `f32` samples from `src_rate` to [`TARGET_SAMPLE_RATE`].
///
/// Linear interpolation is deliberately simple: speech STT is robust to it and
/// it avoids pulling a DSP dependency. Returns the input unchanged when the
/// source is already at the target rate.
pub fn resample_to_16k(samples: &[f32], src_rate: u32) -> Vec<f32> {
    if src_rate == TARGET_SAMPLE_RATE || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = TARGET_SAMPLE_RATE as f64 / src_rate as f64;
    let out_len = ((samples.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        // Position in the source signal for this output sample.
        let src_pos = i as f64 / ratio;
        let idx = src_pos.floor() as usize;
        let frac = src_pos - idx as f64;
        let a = samples.get(idx).copied().unwrap_or(0.0);
        let b = samples.get(idx + 1).copied().unwrap_or(a);
        out.push((a as f64 + (b as f64 - a as f64) * frac) as f32);
    }
    out
}

/// Resample to 16 kHz mono and encode as 16-bit PCM WAV bytes (in memory).
pub fn encode_wav_16k_mono(samples: &[f32], src_rate: u32) -> Result<Vec<u8>, AudioCaptureError> {
    let resampled = resample_to_16k(samples, src_rate);
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = std::io::Cursor::new(Vec::<u8>::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec)
            .map_err(|e| AudioCaptureError::Encode(e.to_string()))?;
        for &s in &resampled {
            let clamped = s.clamp(-1.0, 1.0);
            let value = (clamped * i16::MAX as f32) as i16;
            writer
                .write_sample(value)
                .map_err(|e| AudioCaptureError::Encode(e.to_string()))?;
        }
        writer
            .finalize()
            .map_err(|e| AudioCaptureError::Encode(e.to_string()))?;
    }
    Ok(cursor.into_inner())
}

#[cfg(test)]
#[path = "wav.test.rs"]
mod tests;

use super::*;
use pretty_assertions::assert_eq;

#[test]
fn resample_passthrough_at_target_rate() {
    let samples = vec![0.1, -0.2, 0.3];
    let out = resample_to_16k(&samples, TARGET_SAMPLE_RATE);
    assert_eq!(out, samples);
}

#[test]
fn resample_downsamples_length_proportionally() {
    // 48 kHz -> 16 kHz is a 3:1 downsample, so ~1/3 the samples.
    let samples = vec![0.0f32; 4800];
    let out = resample_to_16k(&samples, 48_000);
    assert_eq!(out.len(), 1600);
}

#[test]
fn resample_empty_is_empty() {
    assert!(resample_to_16k(&[], 44_100).is_empty());
}

#[test]
fn encode_produces_riff_wav_header() {
    let samples = vec![0.0f32, 0.5, -0.5, 1.0, -1.0];
    let bytes = encode_wav_16k_mono(&samples, TARGET_SAMPLE_RATE).expect("encode");
    // RIFF....WAVE magic.
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    // 44-byte canonical PCM header + 2 bytes per mono sample.
    assert_eq!(bytes.len(), 44 + samples.len() * 2);
}

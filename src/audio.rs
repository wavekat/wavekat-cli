//! Canonicalise raw WAV bytes to 16 kHz / mono / 16-bit PCM.
//!
//! `wk exports adapt smart-turn` runs every clip through this so the
//! exported parquet contains uniformly decodable, downstream-ready
//! audio. Two side benefits:
//!
//! 1. **Validation.** Bad clips (corrupt bytes, non-WAV files with a
//!    `.wav` extension, truncated payloads) fail at adapt time with a
//!    clear per-row error instead of crashing a downstream training
//!    loop 30 minutes in.
//! 2. **Normalisation.** Multi-channel or off-rate inputs are reshaped
//!    so notebooks don't have to defensively handle channel counts or
//!    sample rates.
//!
//! Resampling is plain linear interpolation. Snapshots produced by the
//! wavekat platform are already 16 kHz / mono in practice — this path
//! mostly exists to handle the long tail of imported clips, where
//! aliasing introduced by a fast resampler isn't worth a heavyweight
//! dependency.

use std::io::Cursor;

use anyhow::{anyhow, Context, Result};
use hound::{SampleFormat, WavReader, WavSpec, WavWriter};

pub const TARGET_SR: u32 = 16_000;

/// Decode arbitrary WAV bytes, downmix to mono, resample to 16 kHz, and
/// re-encode as 16-bit PCM. Returns the canonical bytes.
pub fn canonicalize_wav(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut reader = WavReader::new(Cursor::new(bytes))
        .context("decoding source WAV (file may be corrupt or not actually WAV)")?;
    let spec = reader.spec();
    let n_channels = spec.channels as usize;
    if n_channels == 0 {
        return Err(anyhow!("WAV header reports 0 channels"));
    }

    let interleaved = decode_to_f32(&mut reader, spec)?;

    let mono = if n_channels == 1 {
        interleaved
    } else {
        downmix(&interleaved, n_channels)
    };

    let resampled = if spec.sample_rate == TARGET_SR {
        mono
    } else {
        resample_linear(&mono, spec.sample_rate, TARGET_SR)
    };

    encode_pcm16(&resampled)
}

fn decode_to_f32<R: std::io::Read>(reader: &mut WavReader<R>, spec: WavSpec) -> Result<Vec<f32>> {
    match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Int, 8) => reader
            .samples::<i8>()
            .map(|s| s.map(|v| v as f32 / 128.0))
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("hound: reading 8-bit samples"),
        (SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / 32_768.0))
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("hound: reading 16-bit samples"),
        (SampleFormat::Int, 24) => reader
            .samples::<i32>()
            .map(|s| s.map(|v| v as f32 / 8_388_608.0))
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("hound: reading 24-bit samples"),
        (SampleFormat::Int, 32) => reader
            .samples::<i32>()
            .map(|s| s.map(|v| v as f32 / 2_147_483_648.0))
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("hound: reading 32-bit samples"),
        (SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("hound: reading 32-bit float samples"),
        (fmt, bits) => Err(anyhow!(
            "unsupported WAV sample format {:?} / {} bit",
            fmt,
            bits
        )),
    }
}

fn downmix(interleaved: &[f32], n_channels: usize) -> Vec<f32> {
    interleaved
        .chunks_exact(n_channels)
        .map(|frame| frame.iter().sum::<f32>() / n_channels as f32)
        .collect()
}

fn resample_linear(input: &[f32], in_rate: u32, out_rate: u32) -> Vec<f32> {
    if in_rate == out_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = in_rate as f64 / out_rate as f64;
    let out_len = ((input.len() as f64) / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let idx = pos.floor() as usize;
        let frac = (pos - idx as f64) as f32;
        let s0 = input[idx];
        let s1 = if idx + 1 < input.len() {
            input[idx + 1]
        } else {
            s0
        };
        out.push(s0 * (1.0 - frac) + s1 * frac);
    }
    out
}

fn encode_pcm16(samples: &[f32]) -> Result<Vec<u8>> {
    let spec = WavSpec {
        channels: 1,
        sample_rate: TARGET_SR,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut buf: Vec<u8> = Vec::new();
    {
        let cursor = Cursor::new(&mut buf);
        let mut writer = WavWriter::new(cursor, spec).context("hound: building output WAV writer")?;
        for &s in samples {
            let clamped = s.clamp(-1.0, 1.0);
            let v = (clamped * i16::MAX as f32) as i16;
            writer.write_sample(v).context("hound: writing sample")?;
        }
        writer.finalize().context("hound: finalising WAV")?;
    }
    Ok(buf)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Build an in-memory PCM-16 WAV from `samples` (interleaved if
    /// stereo+). Used by tests in this module and by the smart-turn
    /// adapter tests that need real WAV bytes.
    pub fn make_test_wav(samples: &[f32], sr: u32, channels: u16) -> Vec<u8> {
        let spec = WavSpec {
            channels,
            sample_rate: sr,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut buf = Vec::new();
        {
            let cursor = Cursor::new(&mut buf);
            let mut w = WavWriter::new(cursor, spec).unwrap();
            for &s in samples {
                let v = (s.clamp(-1.0, 1.0) * 32_767.0) as i16;
                w.write_sample(v).unwrap();
            }
            w.finalize().unwrap();
        }
        buf
    }

    fn read_wav_mono16(bytes: &[u8]) -> (Vec<f32>, u32) {
        let mut r = WavReader::new(Cursor::new(bytes)).unwrap();
        let spec = r.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.sample_format, SampleFormat::Int);
        let samples: Vec<f32> = r
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32_768.0)
            .collect();
        (samples, spec.sample_rate)
    }

    #[test]
    fn passes_16k_mono_through() {
        let input = make_test_wav(&vec![0.5; 100], 16_000, 1);
        let out = canonicalize_wav(&input).unwrap();
        let (samples, sr) = read_wav_mono16(&out);
        assert_eq!(sr, 16_000);
        assert_eq!(samples.len(), 100);
    }

    #[test]
    fn downmixes_stereo_to_mono() {
        // L=+1.0, R=-1.0 → mono ≈ 0
        let mut interleaved = Vec::new();
        for _ in 0..100 {
            interleaved.push(1.0);
            interleaved.push(-1.0);
        }
        let input = make_test_wav(&interleaved, 16_000, 2);
        let out = canonicalize_wav(&input).unwrap();
        let (samples, sr) = read_wav_mono16(&out);
        assert_eq!(sr, 16_000);
        assert_eq!(samples.len(), 100);
        for s in samples {
            assert!(s.abs() < 0.01, "expected ~0, got {s}");
        }
    }

    #[test]
    fn downmixes_eight_channel_to_mono() {
        // 8-channel: 4 channels at +1, 4 at -1 → mono ≈ 0
        let mut interleaved = Vec::new();
        for _ in 0..50 {
            for c in 0..8 {
                interleaved.push(if c < 4 { 1.0 } else { -1.0 });
            }
        }
        let input = make_test_wav(&interleaved, 16_000, 8);
        let out = canonicalize_wav(&input).unwrap();
        let (samples, sr) = read_wav_mono16(&out);
        assert_eq!(sr, 16_000);
        assert_eq!(samples.len(), 50);
        for s in samples {
            assert!(s.abs() < 0.01, "expected ~0, got {s}");
        }
    }

    #[test]
    fn resamples_48k_mono_to_16k() {
        let input = make_test_wav(&vec![0.5; 480], 48_000, 1);
        let out = canonicalize_wav(&input).unwrap();
        let (samples, sr) = read_wav_mono16(&out);
        assert_eq!(sr, 16_000);
        // 480 in / 3:1 ratio = 160 out
        assert_eq!(samples.len(), 160);
    }

    #[test]
    fn rejects_non_wav_bytes() {
        let err = canonicalize_wav(b"not a wav file at all").unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.to_lowercase().contains("wav"),
            "expected WAV-related error, got: {msg}"
        );
    }

    #[test]
    fn rejects_empty_bytes() {
        assert!(canonicalize_wav(&[]).is_err());
    }
}

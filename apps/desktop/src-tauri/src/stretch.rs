//! Key and tempo changes: high-quality time-stretch and pitch-shift (Signalsmith
//! Stretch), applied to each stem when a song loads, like warping in Ableton.

use rig_engine::StemAudio;

/// `length` is output length / input length (0.5 = twice as fast).
/// `semitones` shifts the pitch without changing the length.
pub fn apply(a: &StemAudio, sample_rate: u32, length: f64, semitones: f32) -> StemAudio {
    if (length - 1.0).abs() < 1e-6 && semitones == 0.0 {
        return a.clone();
    }
    let ch = a.channels.max(1);
    let frames = a.frames() as usize;
    let out_frames = ((frames as f64) * length).round().max(1.0) as usize;
    let mut out = vec![0f32; out_frames * ch];
    let mut s = signalsmith_stretch::Stretch::preset_default(ch as u32, sample_rate);
    if semitones != 0.0 {
        s.set_transpose_factor_semitones(semitones, None);
    }
    s.exact(&a.data[..frames * ch], &mut out);
    StemAudio { channels: ch, data: out.into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rising_zero_crossings(x: &[f32]) -> usize {
        x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
    }

    #[test]
    fn tempo_change_keeps_pitch_and_key_change_keeps_length() {
        let sr = 48_000;
        let tone: Vec<f32> = (0..sr as usize * 4).map(|i| (i as f32 / sr as f32 * 440.0 * std::f32::consts::TAU).sin() * 0.5).collect();
        let a = StemAudio::mono(tone);
        // 20% faster: shorter, same pitch
        let faster = apply(&a, sr, 1.0 / 1.2, 0.0);
        assert_eq!(faster.frames(), (4.0 * sr as f64 / 1.2).round() as u64);
        let mid = &faster.data[sr as usize..2 * sr as usize];
        assert!((rising_zero_crossings(mid) as i64 - 440).abs() <= 3);
        // up 2 semitones: same length, higher pitch (~494 Hz)
        let up = apply(&a, sr, 1.0, 2.0);
        assert_eq!(up.frames(), a.frames());
        let mid = &up.data[sr as usize..2 * sr as usize];
        assert!((rising_zero_crossings(mid) as i64 - 494).abs() <= 4, "{}", rising_zero_crossings(mid));
    }
}

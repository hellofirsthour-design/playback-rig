//! Sustained tonic pads, one audio file per key (like MultiTracks pad packs).
//! A pad plays independently of the song transport: it fades in and out, follows
//! the song's key, and keeps playing between songs.

use rig_engine::{Route, StemAudio};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Key names as charts write them.
pub const NAMES: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];

/// Pitch class of a key name ("A" = 9, "Bb" = 10, "F#m" = 6). Minor keys use their root.
pub fn pitch_class(key: &str) -> Option<u8> {
    let k = key.trim();
    let mut c = k.chars();
    let base = match c.next()?.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest: String = c.collect();
    let acc = if rest.starts_with('#') || rest.starts_with('♯') || rest.to_ascii_lowercase().starts_with("sharp") {
        1
    } else if rest.starts_with('b') || rest.starts_with('♭') || rest.to_ascii_lowercase().starts_with("flat") {
        -1
    } else {
        0
    };
    Some(((base + acc + 12) % 12) as u8)
}

/// Finds the key in a pad file name: "A Pad.wav", "Pad - Bb.m4a", "C#-Db.mp3".
pub fn key_in_name(file: &str) -> Option<u8> {
    let stem = Path::new(file).file_stem()?.to_str()?;
    let tokens: Vec<String> = stem
        .split(|c: char| !(c.is_alphanumeric() || c == '#' || c == '♯' || c == '♭'))
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect();
    tokens.iter().rev().find_map(|t| {
        let mut c = t.chars();
        let first = c.next()?;
        if !matches!(first, 'A'..='G') {
            return None;
        }
        let rest: String = c.collect();
        let ok = ["", "#", "b", "♯", "♭", "m", "#m", "bm", "maj", "min", "sharp", "flat", "major", "minor"].contains(&rest.as_str());
        ok.then(|| pitch_class(t)).flatten()
    })
}

/// Pad files in a folder, by pitch class.
pub fn scan(folder: &Path) -> [Option<PathBuf>; 12] {
    let mut out: [Option<PathBuf>; 12] = Default::default();
    let Ok(rd) = std::fs::read_dir(folder) else { return out };
    let mut files: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file() && crate::decode::is_audio(p)).collect();
    files.sort();
    for p in files {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with("._") {
            continue;
        }
        if let Some(pc) = key_in_name(name) {
            out[pc as usize].get_or_insert(p);
        }
    }
    out
}

/// A pad ready to loop forever: stereo, with its loop point crossfaded so it's seamless.
pub struct PadVoice {
    pub data: Arc<[f32]>,
    pub frames: usize,
    pub pos: usize,
    pub gain: f32,
}

/// Up to 90 s of the file, looped with a 4 s crossfade.
pub fn make_voice(a: &StemAudio, sample_rate: u32) -> Result<PadVoice, String> {
    let sr = sample_rate as usize;
    let total = a.frames() as usize;
    let x = (4 * sr).min(total / 4);
    if total < 4 * sr || x == 0 {
        return Err("That pad file is too short to loop (it needs at least 4 seconds).".into());
    }
    let len = (total - x).min(90 * sr);
    let mut data = vec![0f32; len * 2];
    for i in 0..len {
        let (l, r) = a.frame(i as u64);
        data[2 * i] = l;
        data[2 * i + 1] = r;
    }
    // Blend the audio just past the loop end into the loop start. Linear, because a
    // sustained pad sounds much the same at both ends (equal power would swell).
    for i in 0..x {
        let fi = i as f32 / x as f32;
        let fo = 1.0 - fi;
        let (l, r) = a.frame((len + i) as u64);
        data[2 * i] = data[2 * i] * fi + l * fo;
        data[2 * i + 1] = data[2 * i + 1] * fi + r * fo;
    }
    normalize(&mut data);
    Ok(PadVoice { data: data.into(), frames: len, pos: 0, gain: 0.0 })
}

/// Pad packs come mastered loud. Every pad is brought to the same moderate loudness
/// (RMS about -20 dBFS), so 100% sits under the band instead of on top of it, and
/// switching sounds or keys doesn't jump in level. Very quiet files are raised at most 4x.
pub const PAD_RMS: f32 = 0.1;
fn normalize(data: &mut [f32]) {
    if data.is_empty() {
        return;
    }
    let rms = (data.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / data.len() as f64).sqrt() as f32;
    if rms < 1e-6 {
        return;
    }
    let k = (PAD_RMS / rms).min(4.0);
    data.iter_mut().for_each(|v| *v *= k);
}

/// The pad level slider (0-150%) as a gain, on an audio taper: the slider moves through
/// loudness the way the ear hears it, so 8% is properly quiet and 50% is about -12 dB.
pub fn level_gain(level: f32) -> f32 {
    let l = level.clamp(0.0, 1.5);
    l * l
}

/// Plays pads inside the audio callback. No allocation: voices arrive ready-made and
/// finished ones are handed back to be freed elsewhere.
pub struct PadPlayer {
    pub cur: Option<Box<PadVoice>>,
    pub old: Option<Box<PadVoice>>,
    pub on: bool,
    pub level: f32,
    pub route: Route,
    step: f32,
}

impl PadPlayer {
    pub fn new(sample_rate: u32) -> Self {
        PadPlayer { cur: None, old: None, on: false, level: 0.6, route: Route::Pair(0), step: 1.0 / (4.0 * sample_rate as f32) }
    }

    /// Crossfades to a new pad; returns the voice that was fading out, if one has to go.
    pub fn swap(&mut self, v: Box<PadVoice>) -> Option<Box<PadVoice>> {
        let dropped = self.old.take();
        self.old = self.cur.replace(v);
        dropped
    }

    /// Adds the pad into `out` (interleaved, `ch` channels). Returns a finished voice to free.
    pub fn process(&mut self, out: &mut [f32], ch: usize) -> Option<Box<PadVoice>> {
        if self.cur.is_none() && self.old.is_none() {
            return None;
        }
        let frames = out.len() / ch;
        let (pl, pr) = match self.route {
            Route::Pair(c) if (c as usize) + 1 < ch => (c as usize, c as usize + 1),
            Route::Mono(c) if (c as usize) < ch => (c as usize, c as usize),
            _ => (0, 1.min(ch - 1)),
        };
        let mono = pl == pr;
        let target = if self.on { 1.0 } else { 0.0 };
        for f in 0..frames {
            let base = f * ch;
            for (v, tgt) in [(self.cur.as_deref_mut(), target), (self.old.as_deref_mut(), 0.0)] {
                let Some(v) = v else { continue };
                if v.gain < tgt {
                    v.gain = (v.gain + self.step).min(tgt);
                } else if v.gain > tgt {
                    v.gain = (v.gain - self.step).max(tgt);
                }
                if v.gain <= 0.0 {
                    continue;
                }
                let g = v.gain * v.gain * self.level; // gentler fade curve
                let (l, r) = (v.data[2 * v.pos] * g, v.data[2 * v.pos + 1] * g);
                if mono {
                    out[base + pl] += (l + r) * 0.5;
                } else {
                    out[base + pl] += l;
                    out[base + pr] += r;
                }
                v.pos += 1;
                if v.pos >= v.frames {
                    v.pos = 0;
                }
            }
        }
        if self.old.as_ref().map_or(false, |v| v.gain <= 0.0) {
            return self.old.take();
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_keys_from_pad_file_names() {
        assert_eq!(key_in_name("A Pad.wav"), Some(9));
        assert_eq!(key_in_name("Pad - Bb.m4a"), Some(10));
        assert_eq!(key_in_name("MultiTracks Pads - C#.mp3"), Some(1));
        assert_eq!(key_in_name("Fm Pad.wav"), Some(5));
        assert_eq!(key_in_name("Pad.wav"), None);
        assert_eq!(pitch_class("F#m"), Some(6));
        assert_eq!(pitch_class("Bb"), Some(10));
    }

    #[test]
    fn pad_fades_in_loops_seamlessly_and_fades_out() {
        let sr = 1000;
        let a = StemAudio::mono(vec![0.5; 20_000]); // 20 s of DC at 0.5
        let mut p = PadPlayer::new(sr);
        p.level = 1.0;
        p.swap(Box::new(make_voice(&a, sr).unwrap()));
        p.on = true;
        let mut out = vec![0f32; 2 * 30_000];
        p.process(&mut out, 2);
        assert!(out[2 * 100] > 0.0 && out[2 * 100] < 0.01, "fading in");
        // after the 4 s fade, through the loop point (16 s), the level stays put
        // (DC at 0.5 is evened out to the standard pad loudness, PAD_RMS.)
        assert!(out[2 * 5_000..].chunks(2).all(|f| (f[0] - PAD_RMS).abs() < 1e-4), "steady and seamless");
        p.on = false;
        let mut out = vec![0f32; 2 * 5_000];
        p.process(&mut out, 2);
        assert_eq!(out[2 * 4_500], 0.0, "faded out");
    }

    #[test]
    fn pads_come_out_at_the_same_loudness() {
        let sr = 1000;
        let mk = |amp: f32| StemAudio::mono((0..20_000).map(|i| amp * ((i as f32) * 0.05).sin()).collect());
        let rms = |v: &PadVoice| (v.data.iter().map(|x| x * x).sum::<f32>() / v.data.len() as f32).sqrt();
        let loud = make_voice(&mk(0.9), sr).unwrap();
        let soft = make_voice(&mk(0.2), sr).unwrap();
        assert!((rms(&loud) - PAD_RMS).abs() < 0.01, "loud pad brought down: {}", rms(&loud));
        assert!((rms(&soft) - PAD_RMS).abs() < 0.01, "soft pad brought up: {}", rms(&soft));
        // the level slider is a taper: 8% is about -22 dB below 50%, 100% is unity
        assert!((level_gain(1.0) - 1.0).abs() < 1e-6);
        assert!((level_gain(0.5) - 0.25).abs() < 1e-6);
        assert!(level_gain(0.08) < 0.007);
        assert_eq!(level_gain(-1.0), 0.0);
    }
}

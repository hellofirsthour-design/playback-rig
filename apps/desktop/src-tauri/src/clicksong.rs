//! Click songs: for a song with no tracks, generate a Click stem and a spoken Guide
//! stem from a tempo and a list of sections. The result is an ordinary song folder,
//! so everything else (arranging, pedals, key and tempo) works the same.

use crate::songfile::{Marker, SongFile, StemFile, SETUP_VERSION};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize)]
pub struct Spec {
    pub name: String,
    pub bpm: f64,
    pub beats_per_bar: u32,
    #[serde(default)]
    pub key: String,
    /// (section name, length in bars)
    pub sections: Vec<(String, u32)>,
    #[serde(default = "yes")]
    pub guide: bool,
}

fn yes() -> bool {
    true
}

const SR: u32 = 48_000;

fn click(sr: u32, accent: bool) -> Vec<f32> {
    let f = if accent { 1760.0 } else { 1175.0 };
    let a = if accent { 0.9 } else { 0.6 };
    (0..(sr as usize / 25)).map(|i| { let t = i as f32 / sr as f32; (std::f32::consts::TAU * f * t).sin() * a * (-t * 80.0).exp() }).collect()
}

fn mix_at(dst: &mut [f32], at: usize, src: &[f32], gain: f32) {
    for (i, v) in src.iter().enumerate() {
        if let Some(d) = dst.get_mut(at + i) {
            *d += v * gain;
        }
    }
}

/// Speaks a phrase with the Mac's built-in voice. None if that isn't available.
fn speak(text: &str, work: &Path) -> Option<Vec<f32>> {
    let out = work.join(format!("say-{}.aiff", text.replace(|c: char| !c.is_alphanumeric(), "_")));
    let ok = std::process::Command::new("say")
        .args(["-r", "190", "-o"])
        .arg(&out)
        .args(["--data-format=LEI16@48000", text])
        .status()
        .ok()?
        .success();
    if !ok {
        return None;
    }
    let a = crate::decode::decode(&out, SR).ok()?;
    let _ = std::fs::remove_file(&out);
    Some(crate::guide::to_mono(&a))
}

fn write_wav(path: &Path, x: &[f32]) -> Result<(), String> {
    let spec = hound::WavSpec { channels: 1, sample_rate: SR, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for v in x {
        w.write_sample(*v).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

fn safe(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || " -_'&".contains(c) { c } else { '_' }).collect();
    let s = s.trim().to_string();
    if s.is_empty() { "Click song".into() } else { s }
}

/// Builds the song folder under `root` and returns its path. `voice` turns text into
/// audio (None = no guide); it's a parameter so tests can run without a Mac voice.
pub fn create(root: &Path, spec: &Spec, voice: &dyn Fn(&str) -> Option<Vec<f32>>) -> Result<(PathBuf, bool), String> {
    if !(30.0..=300.0).contains(&spec.bpm) {
        return Err("Tempo must be between 30 and 300 BPM.".into());
    }
    let sections: Vec<(String, u32)> = spec.sections.iter().filter(|(n, b)| !n.trim().is_empty() && *b > 0).map(|(n, b)| (n.trim().to_string(), *b)).collect();
    if sections.is_empty() {
        return Err("Add at least one section with a length in bars.".into());
    }
    let bpb = spec.beats_per_bar.clamp(1, 12);
    let beat = 60.0 / spec.bpm;
    let beat_f = |bar: u32, b: u32| (((bar - 1) * bpb + b) as f64 * beat * SR as f64).round() as usize;
    let total_bars = 1 + sections.iter().map(|s| s.1).sum::<u32>(); // bar 1 is the count
    let len = beat_f(total_bars + 1, 0) + SR as usize;

    // Click: every beat, accent on the downbeat.
    let (hi, lo) = (click(SR, true), click(SR, false));
    let mut clk = vec![0f32; len];
    for bar in 1..=total_bars {
        for b in 0..bpb {
            mix_at(&mut clk, beat_f(bar, b), if b == 0 { &hi } else { &lo }, 1.0);
        }
    }

    // Guide: count the first bar in, then call each section in the bar before it
    // (the name on beat 1, then the last two beats counted).
    let mut guide = vec![0f32; len];
    let mut has_guide = false;
    if spec.guide {
        let words = ["one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve"];
        let mut said = std::collections::HashMap::<String, Option<Vec<f32>>>::new();
        let mut say = |t: &str| said.entry(t.to_string()).or_insert_with(|| voice(t)).clone();
        if say("one").is_some() {
            has_guide = true;
            for b in 0..bpb {
                if let Some(w) = say(words[b as usize]) {
                    mix_at(&mut guide, beat_f(1, b), &w, 0.9);
                }
            }
            let mut bar = 2;
            for (i, (name, bars)) in sections.iter().enumerate() {
                if i > 0 {
                    if let Some(w) = say(name) {
                        mix_at(&mut guide, beat_f(bar - 1, 0), &w, 0.9);
                    }
                    for b in bpb.saturating_sub(2).max(1)..bpb {
                        if let Some(w) = say(words[b as usize]) {
                            mix_at(&mut guide, beat_f(bar - 1, b), &w, 0.9);
                        }
                    }
                }
                bar += bars;
            }
        }
    }

    let mut folder = root.join(safe(&spec.name));
    let mut n = 2;
    while folder.exists() {
        folder = root.join(format!("{} {n}", safe(&spec.name)));
        n += 1;
    }
    std::fs::create_dir_all(&folder).map_err(|e| format!("Couldn't create {}: {e}", folder.display()))?;
    write_wav(&folder.join("Click.wav"), &clk)?;
    let mut stems = vec![StemFile { file: "Click.wav".into(), name: "Click".into(), route: rig_engine::Route::Pair(2), gain: 1.0, muted: false }];
    if has_guide {
        write_wav(&folder.join("Guide.wav"), &guide)?;
        stems.push(StemFile { file: "Guide.wav".into(), name: "Guide".into(), route: rig_engine::Route::Pair(2), gain: 1.0, muted: false });
    }
    let mut markers = vec![Marker { name: "Count".into(), bar: 1 }];
    let mut bar = 2;
    for (name, bars) in &sections {
        markers.push(Marker { name: name.clone(), bar });
        bar += bars;
    }
    crate::guide::unique_names(&mut markers);
    let sf = SongFile {
        name: spec.name.trim().to_string(),
        bpm: spec.bpm,
        beats_per_bar: bpb,
        key: spec.key.trim().to_string(),
        click: false,
        markers,
        stems,
        markers_from: Some("manual".into()),
        setup: SETUP_VERSION,
        ..Default::default()
    };
    sf.save(&folder)?;
    Ok((folder, has_guide))
}

/// The real voice: macOS `say`, in a scratch folder.
pub fn mac_voice(work: PathBuf) -> impl Fn(&str) -> Option<Vec<f32>> {
    move |t: &str| speak(t, &work)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_click_and_guide_song_on_the_grid() {
        let root = std::env::temp_dir().join(format!("rig-click-{}", std::process::id()));
        let spec = Spec {
            name: "Way Maker (click)".into(),
            bpm: 120.0,
            beats_per_bar: 4,
            key: "E".into(),
            sections: vec![("Verse".into(), 4), ("Chorus".into(), 8), ("Chorus".into(), 8)],
            guide: true,
        };
        let fake_voice = |_: &str| Some(vec![0.25f32; 4800]); // 0.1 s "word"
        let (folder, guide) = create(&root, &spec, &fake_voice).unwrap();
        assert!(guide);
        let sf = SongFile::scan(&folder, 10).unwrap();
        let m: Vec<(&str, u32)> = sf.markers.iter().map(|m| (m.name.as_str(), m.bar)).collect();
        assert_eq!(m, [("Count", 1), ("Verse", 2), ("Chorus 1", 6), ("Chorus 2", 14)]);
        assert_eq!(sf.bpm, 120.0);
        let click = crate::decode::decode(&folder.join("Click.wav"), SR).unwrap();
        // beat 1 of bar 6 (Chorus 1) = 20 beats * 0.5 s = 10 s; the click starts there
        let at = (10.0 * SR as f64) as u64;
        assert!(click.frame(at + 10).0.abs() > 0.1 && click.frame(at - 200).0.abs() < 1e-3);
        // the guide calls "Chorus" on beat 1 of the bar before (bar 5 = 8 s)
        let g = crate::decode::decode(&folder.join("Guide.wav"), SR).unwrap();
        assert!(g.frame((8.0 * SR as f64) as u64 + 100).0 > 0.2);
        std::fs::remove_dir_all(root).ok();
    }
}

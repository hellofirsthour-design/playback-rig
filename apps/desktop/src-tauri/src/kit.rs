//! The Drummer's kit on disk: `kit.json` plus one stereo file per recorded hit (made by
//! scripts/make-drum-kit.py from DRSKit). Loaded off the audio thread at the output rate.

use rig_engine::drummer::{Hit, Kit, Piece, PIECES};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
struct KitFile {
    pieces: std::collections::BTreeMap<String, PieceFile>,
}

#[derive(Deserialize)]
struct PieceFile {
    hits: Vec<HitFile>,
}

#[derive(Deserialize)]
struct HitFile {
    file: String,
    rms: f32,
}

pub fn load(dir: &Path, sample_rate: u32) -> Result<Kit, String> {
    let text = std::fs::read_to_string(dir.join("kit.json")).map_err(|e| format!("The drum kit isn't there ({e})."))?;
    let kf: KitFile = serde_json::from_str(&text).map_err(|e| format!("The drum kit is damaged ({e})."))?;
    let pieces = std::thread::scope(|s| {
        let hs: Vec<_> = PIECES
            .iter()
            .map(|name| {
                let pf = kf.pieces.get(*name);
                s.spawn(move || -> Result<Piece, String> {
                    let mut hits = vec![];
                    for h in pf.map(|p| p.hits.as_slice()).unwrap_or(&[]) {
                        let a = crate::decode::decode(&dir.join(&h.file), sample_rate)?;
                        let data: Vec<f32> = if a.channels == 2 { a.data.to_vec() } else { a.data.iter().flat_map(|x| [*x, *x]).collect() };
                        hits.push(Hit { data: data.into(), rms: h.rms });
                    }
                    Ok(Piece { hits })
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("A drum failed to load.".into()))).collect::<Result<Vec<_>, _>>()
    })?;
    Ok(Kit { pieces })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig_engine::drummer::{Cue, DrumCmd, Drummer};

    /// Renders demos with the real kit: RIG_KIT=<kit folder> RIG_OUT=<folder>.
    #[test]
    #[ignore]
    fn render_demos() {
        let kit_dir = std::env::var("RIG_KIT").unwrap();
        let out = std::env::var("RIG_OUT").unwrap();
        let sr = 48_000;
        // (name, bpm, beats, compound, script of (bar, command))
        let journey = |bpm: f64, beats: u32, compound: bool| -> Vec<(u32, DrumCmd)> {
            vec![
                (0, DrumCmd::Tempo(bpm)),
                (0, DrumCmd::Meter(beats, compound)),
                (0, DrumCmd::Intensity(0.15)),
                (0, DrumCmd::Start),
                (4, DrumCmd::Intensity(0.35)),
                (8, DrumCmd::Intensity(0.55)),
                (11, DrumCmd::Cue(Cue::Fill)),
                (12, DrumCmd::Intensity(0.75)),
                (15, DrumCmd::Cue(Cue::Build)),
                (25, DrumCmd::Cue(Cue::Fill)),
                (26, DrumCmd::Cue(Cue::Drop)),
                (34, DrumCmd::Cue(Cue::Stop)),
            ]
        };
        let bed = std::env::var("RIG_BED").ok();
        for (name, bpm, beats, compound) in [("A - flow 4-4 at 72", 72.0, 4, false), ("B - flow 6-8, dotted quarter 50", 150.0, 6, true), ("C - over Worthy at 67.5", 67.5, 4, false)] {
            if name.starts_with('C') && bed.is_none() {
                continue;
            }
            let kit = load(Path::new(&kit_dir), sr).unwrap();
            let mut d = Drummer::new(sr);
            d.set_kit(Box::new(kit));
            let script = journey(bpm, beats, compound);
            let bar_s = beats as f64 * 60.0 / bpm;
            let total = (37.0 * bar_s * sr as f64) as usize;
            let mut buf = vec![0f32; total * 2];
            let block = 256;
            let mut f = 0;
            let mut next = 0;
            while f + block <= total {
                let t = f as f64 / sr as f64;
                // Cues go in a beat before their bar, like a person pressing a button.
                while next < script.len() && t >= (script[next].0 as f64 * bar_s - if script[next].0 > 0 { bar_s / beats as f64 } else { 0.0 }).max(0.0) {
                    d.handle(script[next].1);
                    next += 1;
                }
                d.process(&mut buf[f * 2..(f + block) * 2], 2);
                f += block;
            }
            // C: the drummer over a bed of Worthy's keys, pads and strings (stereo wav).
            if name.starts_with('C') {
                let b = crate::decode::decode(Path::new(bed.as_ref().unwrap()), sr).unwrap();
                for (i, s) in buf.iter_mut().enumerate() {
                    *s = *s * 0.7 + b.data.get(i).copied().unwrap_or(0.0);
                }
            }
            let peak = buf.iter().fold(0f32, |m, x| m.max(x.abs()));
            let g = 0.89 / peak.max(1e-6);
            let spec = hound::WavSpec { channels: 2, sample_rate: sr, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
            let mut w = hound::WavWriter::create(format!("{out}/{name}.wav"), spec).unwrap();
            for s in buf {
                w.write_sample((s * g * 32767.0) as i16).unwrap();
            }
            w.finalize().unwrap();
            eprintln!("{name}: peak {peak:.2}");
        }
    }
}

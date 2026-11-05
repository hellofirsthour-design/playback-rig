//! Renders an arrangement through the engine to a multichannel WAV, with no audio
//! hardware. Uses a generated demo song so it runs anywhere.
//!
//!   rig-render --arrangement "Intro,Chorus,Verse 1,Chorus,Chorus,Outro" \
//!              --mute "Verse 1=Drums" --vamp-at 4 --channels 10 --out demo.wav
//!
//! Outputs 1-2 carry the stems (front of house), 3-4 carry the click (in-ears).

use rig_engine::{ArrItem, Arrangement, Command, Engine, Quantize, Route, Section, Song, StemAudio, StemInfo};
use std::process::ExitCode;

const SR: u32 = 48_000;
const BPM: f64 = 84.0;
const PROG: &str = "D A Bm G D A Bm G D A Bm G G A Bm A G D A Bm G D A Bm Bm G D A Bm G D A G A D D";
const SECTIONS: [(&str, u64); 6] = [("Intro", 0), ("Verse 1", 4), ("Pre-Chorus", 12), ("Chorus", 16), ("Bridge", 24), ("Outro", 32)];

fn demo() -> (Song, Vec<StemAudio>) {
    let prog: Vec<&str> = PROG.split(' ').collect();
    let bars = prog.len() as u64;
    let fpb = SR as f64 * 60.0 / BPM;
    let bar = fpb * 4.0;
    let len = (bars as f64 * bar).ceil() as usize;
    let sr = SR as f32;
    let (mut drums, mut bass, mut keys) = (vec![0f32; len], vec![0f32; len], vec![0f32; len]);
    let hz = |m: f32| 440.0 * 2f32.powf((m - 69.0) / 12.0);
    let chord = |c: &str| match c {
        "D" => ([62., 66., 69.], 38.),
        "A" => ([61., 64., 69.], 33.),
        "Bm" => ([62., 66., 71.], 35.),
        _ => ([62., 67., 71.], 31.),
    };
    let mut seed = 1u32;
    let mut noise = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32 * 2.0 - 1.0
    };
    for (b, c) in prog.iter().enumerate() {
        let (pad, root) = chord(c);
        let s0 = (b as f64 * bar) as usize;
        let n = (bar as usize).min(len - s0);
        let full = (4..32).contains(&b);
        for m in pad {
            let w = std::f32::consts::TAU * hz(m) / sr;
            for i in 0..n {
                let env = (i as f32 / (0.12 * sr)).min(1.0) * ((n - i) as f32 / (0.15 * sr)).min(1.0);
                keys[s0 + i] += (w * i as f32).sin() * env * 0.12;
            }
        }
        if !full {
            continue;
        }
        let w = std::f32::consts::TAU * hz(root) / sr;
        for beat in [0.0, 2.0, 3.5] {
            let b0 = s0 + (beat * fpb) as usize;
            let l = ((if beat == 3.5 { 0.45 } else { 1.6 }) * fpb) as usize;
            for i in 0..l.min(len.saturating_sub(b0)) {
                let env = (i as f32 / 200.0).min(1.0) * (-(i as f32) / sr * 2.2).exp() * ((l - i) as f32 / 400.0).min(1.0);
                let x = w * i as f32;
                bass[b0 + i] += (x.sin() + 0.3 * (2.0 * x).sin()) * env * 0.35;
            }
        }
        let bridge = (24..32).contains(&b);
        for beat in 0..4 {
            let t0 = s0 + (beat as f64 * fpb) as usize;
            if bridge || beat % 2 == 0 {
                let mut ph = 0f32;
                for i in 0..((0.35 * sr) as usize).min(len - t0) {
                    let t = i as f32 / sr;
                    ph += std::f32::consts::TAU * (45.0 + 75.0 * (-t * 30.0).exp()) / sr;
                    drums[t0 + i] += ph.sin() * (-t * 9.0).exp() * 0.8;
                }
            }
            if !bridge && beat % 2 == 1 {
                for i in 0..((0.2 * sr) as usize).min(len - t0) {
                    let t = i as f32 / sr;
                    drums[t0 + i] += noise() * (-t * 22.0).exp() * 0.35;
                }
            }
        }
    }
    let starts: Vec<u64> = SECTIONS.iter().map(|s| (s.1 as f64 * bar).round() as u64).collect();
    let sections = SECTIONS
        .iter()
        .enumerate()
        .map(|(i, (name, _))| Section {
            name: name.to_string(),
            start: starts[i],
            end: starts.get(i + 1).copied().unwrap_or(len as u64),
        })
        .collect();
    let stem = |name: &str| StemInfo { name: name.into(), route: Route::Pair(0), gain: 0.7, muted: false };
    let song = Song {
        name: "Open Hands (demo)".into(),
        sample_rate: SR,
        bpm: BPM,
        beats_per_bar: 4,
        offset: 0,
        length: len as u64,
        sections,
        stems: vec![stem("Drums"), stem("Bass"), stem("Keys")],
        guide_stem: None,
        calls: vec![],
    };
    (song, vec![StemAudio::mono(drums), StemAudio::mono(bass), StemAudio::mono(keys)])
}

struct Args {
    arrangement: Option<String>,
    mutes: Vec<String>,
    vamp_at: Option<usize>,
    channels: usize,
    out: String,
}

fn args() -> Result<Args, String> {
    let mut a = Args { arrangement: None, mutes: vec![], vamp_at: None, channels: 4, out: "render.wav".into() };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().ok_or(format!("{k} needs a value"));
        match k.as_str() {
            "--arrangement" => a.arrangement = Some(v()?),
            "--mute" => a.mutes.push(v()?),
            "--vamp-at" => a.vamp_at = Some(v()?.parse().map_err(|_| "--vamp-at takes a block number")?),
            "--channels" => a.channels = v()?.parse().map_err(|_| "--channels takes a number")?,
            "--out" => a.out = v()?,
            "-h" | "--help" => return Err(include_str!("main.rs").lines().take(7).collect::<Vec<_>>().join("\n")),
            _ => return Err(format!("unknown option {k}")),
        }
    }
    Ok(a)
}

fn run() -> Result<(), String> {
    let a = args()?;
    let (song, audio) = demo();
    let mut arr = match &a.arrangement {
        Some(s) => Arrangement::from_names(&song, &s.split(',').map(str::trim).collect::<Vec<_>>())?,
        None => Arrangement::linear(&song),
    };
    // --mute "Verse 1=Drums" mutes Drums in every Verse 1 block.
    for m in &a.mutes {
        let (sec, stem) = m.split_once('=').ok_or("--mute takes Section=Stem")?;
        let si = song.section_index(sec.trim()).ok_or(format!("no section called \"{sec}\""))?;
        let st = song.stems.iter().position(|s| s.name.eq_ignore_ascii_case(stem.trim())).ok_or(format!("no stem called \"{stem}\""))?;
        for it in arr.items.iter_mut().filter(|it: &&mut ArrItem| it.section == si) {
            it.muted_stems.push(st);
        }
    }
    let names: Vec<&str> = arr.items.iter().map(|it| song.sections[it.section].name.as_str()).collect();
    println!("Arrangement: {}", names.join(" → "));

    let ch = a.channels;
    let mut e = Engine::new(song, arr, audio, ch)?;
    e.handle(Command::SetQuantize(Quantize::SectionEnd));
    if ch < 4 {
        e.handle(Command::SetClickRoute(Route::Mono(1)));
        for i in 0..e.mix().len() {
            e.handle(Command::SetStemRoute(i, Route::Mono(0)));
        }
    }
    e.handle(Command::Play);

    let spec = hound::WavSpec { channels: ch as u16, sample_rate: SR, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut w = hound::WavWriter::create(&a.out, spec).map_err(|e| e.to_string())?;
    let mut buf = vec![0f32; 512 * ch];
    let mut frames = 0u64;
    let mut vamped = 0u32;
    let mut last_item = usize::MAX;
    let mut last_fpos = 0u64;
    while e.is_playing() {
        e.process(&mut buf);
        let s = e.snapshot();
        if s.item != last_item && !s.counting_in {
            println!("  {:>6.1}s  block {}  bar {}", frames as f64 / SR as f64, s.item + 1, s.bar);
            last_item = s.item;
        }
        // Demonstrate a live vamp: loop the chosen block twice, then let it go.
        if Some(s.item + 1) == a.vamp_at && !s.vamp && vamped == 0 {
            e.handle(Command::ToggleVamp);
            vamped = 1;
        } else if s.vamp && s.frame_in_section < last_fpos {
            println!("  {:>6.1}s  vamp: block {} again", frames as f64 / SR as f64, s.item + 1);
            vamped += 1;
            if vamped > 2 {
                e.handle(Command::ToggleVamp);
            }
        }
        last_fpos = s.frame_in_section;
        for v in &buf {
            w.write_sample(*v).map_err(|e| e.to_string())?;
        }
        frames += (buf.len() / ch) as u64;
        if frames > SR as u64 * 60 * 20 {
            return Err("render ran past 20 minutes; stopping".into());
        }
    }
    w.finalize().map_err(|e| e.to_string())?;
    println!("Wrote {} ({:.1}s, {} channels)", a.out, frames as f64 / SR as f64, ch);
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

//! Smooth transitions between songs (medleys, segues, cuts to another song), prepared
//! off the audio thread while the current song plays:
//!
//! - the next song comes in on the downbeat at the last song's tempo, holds it for two
//!   bars, then eases into its own tempo, arriving by the time its guide first speaks
//!   (a pre-rendered `Prelude` the engine plays first);
//! - the last song's sustained parts (keys, pads, guitars) bloom into a reverb wash over
//!   the seam, and the song settles down in level over its last two bars when it's much
//!   louder than where it's landing;
//! - optionally, the Mac's own voice announces where it's going ("Worthy. Instrumental.")
//!   in the guide's output, for whoever runs tracks without calling cues.

use rig_engine::{stem_role, Prelude, Role, StemAudio};

/// How the incoming song's tempo moves: held at the last song's tempo, then eased.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glide {
    pub hold_beats: u32,
    pub glide_beats: u32,
}

/// Two bars held, then a glide that lands on the song's own tempo by its first guide call
/// (at least two beats, at most two bars), all inside the entry block.
pub fn plan_glide(beat_src: f64, beats_per_bar: u32, beats_to_first_call: Option<f64>, block_beats: f64) -> Glide {
    let bar = beats_per_bar.max(1);
    let mut hold = 2 * bar;
    let mut glide = match beats_to_first_call {
        Some(b) if b.floor() as u32 >= hold + 2 => (b.floor() as u32 - hold).min(2 * bar),
        _ => bar,
    }
    .max(2);
    // Leave a beat of the block for the hand-back.
    let room = (block_beats - 1.0).floor().max(0.0) as u32;
    while hold + glide > room && hold > 0 {
        hold -= 1;
    }
    while hold + glide > room && glide > 1 {
        glide -= 1;
    }
    let _ = beat_src;
    Glide { hold_beats: hold, glide_beats: glide }
}

/// (source frames, output frames) for each stretch of the entry, then one more beat at
/// the song's own tempo for the hand-back crossfade.
pub fn segments(sr: u32, beat_src: f64, from_bpm: f64, to_bpm: f64, g: Glide) -> Vec<(u64, u64)> {
    let mut segs = Vec::new();
    let out_beat = |bpm: f64| sr as f64 * 60.0 / bpm * (beat_src / (sr as f64 * 60.0 / to_bpm));
    if g.hold_beats > 0 {
        let s = beat_src * g.hold_beats as f64;
        segs.push((s.round() as u64, (out_beat(from_bpm) * g.hold_beats as f64).round() as u64));
    }
    for k in 0..g.glide_beats {
        let f = (k as f64 + 0.5) / g.glide_beats as f64;
        let f = f * f * (3.0 - 2.0 * f);
        let bpm = from_bpm + (to_bpm - from_bpm) * f;
        segs.push((beat_src.round() as u64, out_beat(bpm).round() as u64));
    }
    segs.push((beat_src.round() as u64, beat_src.round() as u64));
    segs
}

/// One stem, stretched segment by segment from `start` (Signalsmith, pitch kept).
pub fn glide_stem(a: &StemAudio, sr: u32, start: u64, segs: &[(u64, u64)]) -> StemAudio {
    let ch = a.channels.max(1);
    let frames = a.frames() as usize;
    let get = |from: usize, len: usize| -> Vec<f32> {
        let mut v = vec![0f32; len * ch];
        if from < frames {
            let n = len.min(frames - from);
            v[..n * ch].copy_from_slice(&a.data[from * ch..(from + n) * ch]);
        }
        v
    };
    let mut st = signalsmith_stretch::Stretch::preset_default(ch as u32, sr);
    let lat = st.input_latency();
    let total_out: u64 = segs.iter().map(|s| s.1).sum();
    let mut out = vec![0f32; total_out as usize * ch];
    let start = start as usize;
    let rate0 = segs.first().map_or(1.0, |s| s.0 as f64 / s.1.max(1) as f64);
    st.seek(get(start, lat), rate0);
    let (mut pos, mut o) = (start + lat, 0usize);
    for (i, &(s, n)) in segs.iter().enumerate() {
        let s_in = if i == 0 { (s as usize).saturating_sub(lat) } else { s as usize };
        let input = get(pos, s_in);
        st.process(&input, &mut out[o * ch..(o + n as usize) * ch]);
        pos += s_in;
        o += n as usize;
    }
    StemAudio { channels: ch, data: out.into() }
}

/// The incoming song's entry: every stem stretched along the glide, and its key frames.
pub fn prelude(stems: &[StemAudio], sr: u32, item: usize, block_start: u64, beat_src: f64, from_bpm: f64, to_bpm: f64, g: Glide) -> Prelude {
    let segs = segments(sr, beat_src, from_bpm, to_bpm, g);
    let mut map = vec![(0u64, 0u64)];
    let (mut o, mut s) = (0u64, 0u64);
    for &(si, oi) in &segs[..segs.len() - 1] {
        o += oi;
        s += si;
        map.push((o, s));
    }
    let stems = stems.iter().map(|a| glide_stem(a, sr, block_start, &segs)).collect();
    Prelude { item, stems, map }
}

/// RMS level (dB) of a mix of stems over [from, from+len): the parts that reach the
/// room (not click or guide, not muted), at their levels.
pub fn mix_db(stems: &[(&StemAudio, &str, f32)], from: u64, len: u64) -> f32 {
    let mut sum = 0f64;
    let mut n = 0u64;
    for f in (from..from + len).step_by(4) {
        let (mut l, mut r) = (0f32, 0f32);
        for (a, name, g) in stems {
            if *g == 0.0 || matches!(stem_role(name), Role::Click | Role::Guide) {
                continue;
            }
            let (x, y) = a.frame(f);
            l += x * g;
            r += y * g;
        }
        sum += (l as f64).powi(2) + (r as f64).powi(2);
        n += 2;
    }
    (10.0 * (sum / n.max(1) as f64 + 1e-12).log10()) as f32
}

/// How far the last song comes down over its last two bars: by most of the difference
/// when it's much louder than where it's landing (up to 8 dB), as a gain.
pub fn settle_gain(out_db: f32, in_db: f32) -> f32 {
    let drop = ((out_db - in_db - 2.0).clamp(0.0, 10.0)) * 0.8;
    10f32.powf(-drop / 20.0)
}

/// The reverb wash: the last beat of the outgoing sustained parts, faded in, through a
/// plate-like reverb (Freeverb), with the low end removed. Stereo; starts one beat before
/// the change.
pub fn wash(stems: &[(&StemAudio, &str, f32)], sr: u32, exit_src: u64, beat: u64, gain: f32) -> StemAudio {
    let tail = (sr as f64 * 3.0) as usize;
    let n = beat as usize + tail;
    let mut send = vec![(0f32, 0f32); n];
    let from = exit_src.saturating_sub(beat);
    for (a, name, g) in stems {
        if *g == 0.0 || stem_role(name) != Role::Sustain {
            continue;
        }
        for k in 0..beat as usize {
            let (x, y) = a.frame(from + k as u64);
            let ramp = k as f32 / beat.max(1) as f32;
            send[k].0 += x * g * ramp;
            send[k].1 += y * g * ramp;
        }
    }
    let mut verb = Freeverb::new(sr);
    let mut hp = [Biquad::highpass(250.0, sr as f32), Biquad::highpass(250.0, sr as f32)];
    let mut out = vec![0f32; n * 2];
    for (k, (l, r)) in send.into_iter().enumerate() {
        let (wl, wr) = verb.process(l, r);
        out[2 * k] = hp[0].run(wl) * 0.55 * gain;
        out[2 * k + 1] = hp[1].run(wr) * 0.55 * gain;
    }
    // fade the very end so it never clicks off
    let f = (sr as usize / 2).min(n);
    for k in 0..f {
        let w = k as f32 / f as f32;
        out[2 * (n - 1 - k)] *= w;
        out[2 * (n - 1 - k) + 1] *= w;
    }
    StemAudio { channels: 2, data: out.into() }
}

/// "Instrumental 1" -> "Instrumental", "Bridge 1 (2)" -> "Bridge", "Pre-Chorus 2" -> "Pre-Chorus".
pub fn section_label(name: &str) -> String {
    let base = name.split('(').next().unwrap_or(name).trim();
    let words: Vec<&str> = base.split_whitespace().collect();
    let keep = words.iter().rposition(|w| !w.chars().all(|c| c.is_ascii_digit())).map_or(0, |i| i + 1);
    words[..keep].join(" ")
}

/// What the voice says: "Worthy. Instrumental." (song name, then where it comes in).
pub fn announcement(song: &str, section: &str) -> String {
    let s = section_label(section);
    let song = song.trim();
    match (song.is_empty(), s.is_empty()) {
        (false, false) => format!("{song}. {s}."),
        (false, true) => format!("{song}."),
        (true, false) => format!("{s}."),
        _ => String::new(),
    }
}

/// The Mac's built-in voice saying `text`, at `sr`. None where there's no voice (not a
/// Mac) or it fails; the transition just goes ahead without it.
pub fn speak(text: &str, sr: u32, cache: &std::path::Path) -> Option<StemAudio> {
    if text.is_empty() {
        return None;
    }
    let _ = std::fs::create_dir_all(cache);
    let key: u64 = text.bytes().fold(1469598103934665603u64, |h, b| (h ^ b as u64).wrapping_mul(1099511628211));
    let path = cache.join(format!("say-{key:016x}-{sr}.wav"));
    if !path.exists() {
        let ok = std::process::Command::new("say")
            .args(["-o"])
            .arg(&path)
            .args(["--file-format=WAVE", &format!("--data-format=LEI16@{sr}"), "-r", "185", text])
            .status()
            .ok()?
            .success();
        if !ok {
            return None;
        }
    }
    crate::decode::decode(&path, sr).ok()
}

/// Freeverb (Jezar's public-domain reverb): 8 combs and 4 all-passes per side.
struct Freeverb {
    combs: [Vec<Comb>; 2],
    alls: [Vec<AllPass>; 2],
}
struct Comb {
    buf: Vec<f32>,
    i: usize,
    store: f32,
}
struct AllPass {
    buf: Vec<f32>,
    i: usize,
}
impl Freeverb {
    fn new(sr: u32) -> Self {
        let k = sr as f32 / 44100.0;
        let size = |n: usize, spread: usize| ((n + spread) as f32 * k).max(1.0) as usize;
        let comb = |n, s| Comb { buf: vec![0.0; size(n, s)], i: 0, store: 0.0 };
        let all = |n, s| AllPass { buf: vec![0.0; size(n, s)], i: 0 };
        let cn = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
        let an = [556, 441, 341, 225];
        Freeverb {
            combs: [cn.iter().map(|&n| comb(n, 0)).collect(), cn.iter().map(|&n| comb(n, 23)).collect()],
            alls: [an.iter().map(|&n| all(n, 0)).collect(), an.iter().map(|&n| all(n, 23)).collect()],
        }
    }
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let input = (l + r) * 0.015;
        let mut out = [0f32; 2];
        for side in 0..2 {
            let mut acc = 0.0;
            for c in self.combs[side].iter_mut() {
                let y = c.buf[c.i];
                c.store = y * 0.8 + c.store * 0.2; // room 0.84 feedback, damping 0.2
                c.buf[c.i] = input + c.store * 0.84;
                c.i = (c.i + 1) % c.buf.len();
                acc += y;
            }
            for a in self.alls[side].iter_mut() {
                let b = a.buf[a.i];
                a.buf[a.i] = acc + b * 0.5;
                a.i = (a.i + 1) % a.buf.len();
                acc = b - acc;
            }
            out[side] = acc;
        }
        (out[0], out[1])
    }
}

struct Biquad {
    k: [f32; 5],
    s: [f32; 4],
}
impl Biquad {
    fn highpass(hz: f32, sr: f32) -> Self {
        let w = std::f32::consts::TAU * hz / sr;
        let (sn, cs) = w.sin_cos();
        let a = sn / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + a;
        Biquad { k: [(1.0 + cs) / 2.0 / a0, -(1.0 + cs) / a0, (1.0 + cs) / 2.0 / a0, -2.0 * cs / a0, (1.0 - a) / a0], s: [0.0; 4] }
    }
    fn run(&mut self, x: f32) -> f32 {
        let [b0, b1, b2, a1, a2] = self.k;
        let y = b0 * x + b1 * self.s[0] + b2 * self.s[1] - a1 * self.s[2] - a2 * self.s[3];
        self.s = [x, self.s[0], y, self.s[2]];
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Short tone bursts on every beat of a 67.5 BPM song.
    fn pulses(sr: u32, bpm: f64, beats: usize) -> StemAudio {
        let beat = sr as f64 * 60.0 / bpm;
        let n = (beat * beats as f64) as usize;
        let mut v = vec![0f32; n];
        for b in 0..beats {
            let s = (b as f64 * beat) as usize;
            for k in 0..(sr as usize / 100) {
                if s + k < n {
                    v[s + k] = (k as f32 * 0.3).sin() * 0.8;
                }
            }
        }
        StemAudio::mono(v)
    }

    fn onsets(a: &StemAudio, sr: u32) -> Vec<f64> {
        let (mut on, mut i) = (vec![], 0usize);
        let d = &a.data;
        while i < d.len() {
            if d[i].abs() > 0.3 {
                on.push(i as f64 / sr as f64);
                i += sr as usize / 5;
            } else {
                i += 1;
            }
        }
        on
    }

    #[test]
    fn the_entry_holds_the_old_tempo_then_eases_into_its_own() {
        let sr = 48_000;
        let (from, to) = (75.0, 67.5);
        let beat = sr as f64 * 60.0 / to;
        let song = pulses(sr, to, 24);
        let g = plan_glide(beat, 4, Some(12.0), 24.0);
        assert_eq!(g, Glide { hold_beats: 8, glide_beats: 4 }, "2 bars held, glide done by the guide at beat 12");
        let p = prelude(&[song], sr, 0, 0, beat, from, to, g);
        // Key frames: 8 beats of source in 8 beats at 75; ends at 12 beats of source.
        assert_eq!(p.map[1].1, (beat * 8.0).round() as u64);
        assert!((p.map[1].0 as f64 - 8.0 * sr as f64 * 60.0 / from).abs() < 2.0);
        assert!((p.map.last().unwrap().1 as f64 - 12.0 * beat).abs() < 4.0, "hands back at beat 12");
        let iv: Vec<f64> = onsets(&p.stems[0], sr).windows(2).map(|w| 60.0 / (w[1] - w[0])).collect();
        for (k, bpm) in iv.iter().enumerate().take(7) {
            assert!((bpm - from).abs() < 1.0, "beat {k} held at 75: {bpm}");
        }
        assert!(iv[8] < from - 0.3 && iv[8] > to, "easing: {:?}", &iv[7..12]);
        assert!((iv[11] - to).abs() < 1.0, "arrived at 67.5 by the guide: {:?}", &iv[7..12]);
    }

    #[test]
    fn a_short_block_or_an_early_guide_shortens_the_glide() {
        // Guide speaks at beat 6: hold two bars anyway, one-bar glide.
        assert_eq!(plan_glide(1.0, 4, Some(6.0), 32.0), Glide { hold_beats: 8, glide_beats: 4 });
        // No guide: one-bar glide.
        assert_eq!(plan_glide(1.0, 4, None, 32.0), Glide { hold_beats: 8, glide_beats: 4 });
        // A late guide: up to two bars of glide.
        assert_eq!(plan_glide(1.0, 4, Some(30.0), 40.0), Glide { hold_beats: 8, glide_beats: 8 });
        // A one-bar block: everything fits inside it.
        let g = plan_glide(1.0, 4, None, 4.0);
        assert!(g.hold_beats + g.glide_beats <= 3, "{g:?}");
    }

    #[test]
    fn labels_and_announcements() {
        assert_eq!(section_label("Instrumental 1"), "Instrumental");
        assert_eq!(section_label("Bridge 1 (2)"), "Bridge");
        assert_eq!(section_label("Pre-Chorus 2"), "Pre-Chorus");
        assert_eq!(section_label("Verse"), "Verse");
        assert_eq!(announcement("Worthy", "Instrumental 1"), "Worthy. Instrumental.");
        assert_eq!(announcement("Worthy", ""), "Worthy.");
    }

    #[test]
    fn settle_comes_down_by_most_of_the_gap() {
        assert_eq!(settle_gain(-20.0, -19.0), 1.0, "about the same level: no settle");
        assert!((20.0 * settle_gain(-14.5, -27.0).log10() + 8.0).abs() < 0.01, "12.5 dB louder: down 8 dB");
        assert!((20.0 * settle_gain(-14.0, -19.0).log10() + 2.4).abs() < 0.01);
    }

    #[test]
    fn the_wash_is_only_the_sustained_parts_and_rings_after_the_seam() {
        let sr = 48_000;
        let tone = StemAudio::mono((0..sr as usize).map(|i| (i as f32 * 0.05).sin()).collect());
        let beat = sr as u64 / 2;
        let w = wash(&[(&tone, "Drums", 1.0), (&tone, "BGVS", 1.0)], sr, sr as u64, beat, 1.0);
        assert!(w.data.iter().all(|v| *v == 0.0), "no drums or vocals in the wash");
        let w = wash(&[(&tone, "Keys 1", 1.0)], sr, sr as u64, beat, 1.0);
        let e = |a: usize, b: usize| w.data[a * 2..b * 2].iter().map(|v| v * v).sum::<f32>();
        assert!(e(beat as usize, beat as usize + sr as usize / 2) > 1e-3, "rings after the seam");
        assert!(e(w.frames() as usize - 100, w.frames() as usize) < 1e-6, "dies away at the end");
    }
}

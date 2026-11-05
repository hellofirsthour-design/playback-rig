//! Reads a MultiTracks-style song folder to set it up automatically:
//! - tempo and first downbeat from the Click stem (onset grid fit, accent = downbeat),
//! - section markers from the Guide stem: find each spoken cue, recognise it with an
//!   offline Whisper model, and place the marker on the downbeat after the cue.

use crate::songfile::Marker;
use rig_engine::StemAudio;
use std::path::Path;

pub fn to_mono(a: &StemAudio) -> Vec<f32> {
    (0..a.frames()).map(|i| { let (l, r) = a.frame(i); (l + r) * 0.5 }).collect()
}

/* ---------------- tempo from the click ---------------- */

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tempo {
    pub bpm: f64,
    /// Time of the first downbeat, in seconds.
    pub offset: f64,
}

struct Onset {
    t: f64,
    amp: f32,
    zcr: f32,
}

fn onsets(x: &[f32], sr: u32) -> Vec<Onset> {
    let hop = (sr as usize / 200).max(1); // 5 ms
    let env: Vec<f32> = x.chunks(hop).map(|c| c.iter().fold(0.0f32, |m, v| m.max(v.abs()))).collect();
    let peak = env.iter().cloned().fold(0.0f32, f32::max);
    if peak < 1e-4 {
        return vec![];
    }
    let thr = peak * 0.3;
    let refractory = (0.1 * 200.0) as usize; // 100 ms in hops
    let mut out = vec![];
    let mut last: Option<usize> = None;
    // Before the file starts counts as silence: a click right at sample 0 (as in
    // MultiTracks stems) is a click, not something to skip.
    for i in 0..env.len() {
        let prev = if i == 0 { 0.0 } else { env[i - 1] };
        if env[i] >= thr && prev < thr * 0.6 && last.map_or(true, |l| i - l > refractory) {
            last = Some(i);
            // refine to the first sample over the threshold
            let s0 = i * hop;
            let s1 = ((i + 1) * hop).min(x.len());
            let first = (s0..s1).find(|&s| x[s].abs() >= thr).unwrap_or(s0);
            let w = &x[first..(first + sr as usize / 50).min(x.len())]; // 20 ms
            let amp = w.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            let zc = w.windows(2).filter(|p| (p[0] >= 0.0) != (p[1] >= 0.0)).count() as f32 / w.len().max(1) as f32;
            out.push(Onset { t: first as f64 / sr as f64, amp, zcr: zc });
        }
    }
    out
}

pub fn tempo_from_click(x: &[f32], sr: u32, bpb: u32) -> Option<Tempo> {
    let on = onsets(x, sr);
    if on.len() < 8 {
        return None;
    }
    let mut iv: Vec<f64> = on.windows(2).map(|w| w[1].t - w[0].t).filter(|d| (0.2..=2.0).contains(d)).collect();
    if iv.len() < 6 {
        return None;
    }
    iv.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut period = iv[iv.len() / 2];
    // Beat index of each onset, walking forward so gaps in the click don't drift.
    let mut k = vec![0i64; on.len()];
    for i in 1..on.len() {
        k[i] = k[i - 1] + ((on[i].t - on[i - 1].t) / period).round().max(1.0) as i64;
    }
    // Least-squares fit t = t0 + k * period.
    let n = on.len() as f64;
    let (sk, st) = (k.iter().map(|&v| v as f64).sum::<f64>(), on.iter().map(|o| o.t).sum::<f64>());
    let skk: f64 = k.iter().map(|&v| (v * v) as f64).sum();
    let skt: f64 = k.iter().zip(&on).map(|(&v, o)| v as f64 * o.t).sum();
    let den = n * skk - sk * sk;
    if den.abs() < 1e-9 {
        return None;
    }
    period = (n * skt - sk * st) / den;
    let t0 = (st - period * sk) / n;
    let mut bpm = 60.0 / period;
    if (bpm - bpm.round()).abs() < 0.08 {
        bpm = bpm.round();
    } else {
        bpm = (bpm * 10.0).round() / 10.0;
    }
    // Downbeat: the beat position in the bar whose clicks are louder or higher.
    let bpb = bpb.max(1) as i64;
    let mut amp = vec![(0.0f64, 0u32); bpb as usize];
    let mut zcr = vec![0.0f64; bpb as usize];
    for (o, &kk) in on.iter().zip(&k) {
        let p = kk.rem_euclid(bpb) as usize;
        amp[p].0 += o.amp as f64;
        amp[p].1 += 1;
        zcr[p] += o.zcr as f64;
    }
    let mean = |v: f64, c: u32| if c == 0 { 0.0 } else { v / c as f64 };
    let am: Vec<f64> = (0..bpb as usize).map(|p| mean(amp[p].0, amp[p].1)).collect();
    let zm: Vec<f64> = (0..bpb as usize).map(|p| mean(zcr[p], amp[p].1)).collect();
    let pick = |v: &[f64]| -> Option<usize> {
        let (i, &best) = v.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap())?;
        let rest: Vec<f64> = v.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, &x)| x).collect();
        let other = rest.iter().cloned().fold(0.0, f64::max);
        (best > other * 1.08).then_some(i)
    };
    let phase = pick(&am).or_else(|| pick(&zm)).unwrap_or(0) as f64;
    let beat = 60.0 / bpm;
    let mut offset = t0 + phase * period;
    let bar = beat * bpb as f64;
    while offset - bar >= -0.001 {
        offset -= bar;
    }
    Some(Tempo { bpm, offset: offset.max(0.0) })
}

/* ---------------- spoken cues on the guide ---------------- */

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
}

/// Finds bursts of speech on a guide track (which is silent between cues).
pub fn find_cues(x: &[f32], sr: u32) -> Vec<Cue> {
    let hop = (sr as usize / 100).max(1); // 10 ms
    let rms: Vec<f32> = x.chunks(hop).map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt()).collect();
    let mut sorted = rms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p99 = sorted.get(sorted.len() * 99 / 100).copied().unwrap_or(0.0);
    let thr = (p99 * 0.12).max(0.004);
    let mut cues: Vec<Cue> = vec![];
    let mut start: Option<usize> = None;
    let mut quiet = 0usize;
    let gap = 30; // 300 ms of silence ends a cue
    for (i, &v) in rms.iter().enumerate() {
        if v >= thr {
            if start.is_none() {
                start = Some(i);
            }
            quiet = 0;
        } else if let Some(s) = start {
            quiet += 1;
            if quiet >= gap {
                cues.push(Cue { start: s as f64 / 100.0, end: (i - quiet + 1) as f64 / 100.0 });
                start = None;
                quiet = 0;
            }
        }
    }
    if let Some(s) = start {
        cues.push(Cue { start: s as f64 / 100.0, end: (rms.len() - quiet) as f64 / 100.0 });
    }
    cues.retain(|c| c.end - c.start >= 0.12);
    cues
}

/// Turns what the guide said into a section name, or None for counts and
/// performance cues ("1 2 3 4", "all in", "hits").
pub fn label_for(text: &str) -> Option<String> {
    let norm: String = text.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' }).collect();
    let words: Vec<String> = norm
        .split_whitespace()
        .map(|w| match w {
            "one" | "first" => "1",
            "two" | "to" | "too" | "second" => "2",
            "three" | "third" => "3",
            "four" | "for" | "fourth" => "4",
            "five" => "5",
            "six" => "6",
            other => other,
        }
        .to_string())
        .collect();
    let joined = format!(" {} ", words.join(" "));
    const KINDS: [(&[&str], &str); 22] = [
        (&[" pre chorus ", " prechorus ", " pre course ", " pre corus "], "Pre-Chorus"),
        (&[" post chorus ", " postchorus "], "Post-Chorus"),
        (&[" chorus ", " course ", " corus ", " choruses "], "Chorus"),
        (&[" verse ", " verses ", " vers "], "Verse"),
        (&[" bridge ", " bridges "], "Bridge"),
        (&[" intro ", " introduction "], "Intro"),
        (&[" outro ", " out row ", " outra "], "Outro"),
        (&[" instrumental "], "Instrumental"),
        (&[" interlude "], "Interlude"),
        (&[" turnaround ", " turn around "], "Turnaround"),
        (&[" refrain "], "Refrain"),
        (&[" breakdown ", " break down "], "Breakdown"),
        (&[" build ", " builds "], "Build"),
        (&[" tag ", " tags "], "Tag"),
        (&[" vamp ", " vamps "], "Vamp"),
        (&[" ending ", " end "], "Ending"),
        (&[" solo "], "Solo"),
        (&[" rap "], "Rap"),
        (&[" hook "], "Hook"),
        (&[" exhortation "], "Exhortation"),
        (&[" a cappella ", " acapella ", " a capella "], "A Cappella"),
        (&[" channel "], "Channel"),
    ];
    let (pos, name) = KINDS
        .iter()
        .find_map(|(keys, name)| keys.iter().find_map(|k| joined.find(k).map(|p| (p + k.len() - 1, *name))))?;
    let num = joined[pos..].split_whitespace().next().filter(|w| w.len() <= 2 && w.chars().all(|c| c.is_ascii_digit()));
    Some(match num {
        Some(n) => format!("{name} {n}"),
        None => name.to_string(),
    })
}

/// Cues for the band and MD ("Build", "Drums in", "All in", "Hits"), not parts of the
/// song's form, so never section markers.
pub fn is_band_cue(name: &str) -> bool {
    let n: String = name.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphabetic() { c } else { ' ' }).collect();
    let n = n.split_whitespace().collect::<Vec<_>>().join(" ");
    let cues = [
        "build", "builds", "build up", "buildup", "drums in", "drums out", "all in", "band in", "kick in", "hits", "hit", "hold",
        "stop", "stops", "swell", "crash", "big", "quiet", "down", "break", "acapella", "a cappella", "last time", "one more time",
    ];
    cues.contains(&n.as_str()) || n.starts_with("build ") || n.ends_with(" in") && !n.contains("intro")
}

/// One burst of speech on the guide and what it said.
#[derive(Clone, Debug, PartialEq)]
pub struct Heard {
    pub start: f64,
    pub end: f64,
    pub text: String,
    /// Section name, if the words name a section.
    pub label: Option<String>,
}

/// A count ("1, 2, 3, 4", "and") rather than a cue. Unreadable short bursts count too.
pub fn is_count(h: &Heard) -> bool {
    let norm: String = h.text.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' }).collect();
    let words: Vec<&str> = norm.split_whitespace().collect();
    if words.is_empty() {
        return h.end - h.start < 0.7;
    }
    words.iter().all(|w| matches!(*w, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "one" | "two" | "three" | "four" | "five" | "six" | "seven" | "eight" | "to" | "too" | "for" | "free" | "and"))
}

/// Places a marker for each section cue. MultiTracks guides usually say the section
/// name and then count the next bar in ("Verse... 1, 2, 3, 4"): the section starts on
/// the downbeat after the count. With no count after it, the section starts on the
/// downbeat after the cue.
pub fn place_markers(heard: &[Heard], tempo: Tempo, bpb: u32) -> Vec<Marker> {
    let beat = 60.0 / tempo.bpm;
    let bar = beat * bpb.max(1) as f64;
    let mut out: Vec<Marker> = vec![];
    for (i, h) in heard.iter().enumerate() {
        let Some(name) = &h.label else { continue };
        if is_band_cue(name) {
            continue;
        }
        let mut end = h.end;
        let mut counted = false;
        for f in &heard[i + 1..] {
            if f.label.is_some() || f.start > h.end + 1.6 * bar || !is_count(f) {
                break;
            }
            end = f.end;
            counted = true;
        }
        // A breakdown is a section only when it's counted in like one.
        if name.starts_with("Breakdown") && !counted {
            continue;
        }
        let b = ((end - 0.35 * beat - tempo.offset) / bar).ceil().max(0.0) as u32 + 1;
        if out.last().map_or(false, |m| m.bar >= b) {
            continue; // two cues for the same bar: keep the first
        }
        out.push(Marker { name: name.clone(), bar: b });
    }
    unique_names(&mut out);
    out
}

/// "Chorus", "Chorus" -> "Chorus 1", "Chorus 2" (section names must be unique).
pub fn unique_names(out: &mut [Marker]) {
    let mut total = std::collections::HashMap::<String, usize>::new();
    for m in out.iter() {
        *total.entry(m.name.clone()).or_default() += 1;
    }
    let mut seen = std::collections::HashMap::<String, usize>::new();
    for m in out.iter_mut() {
        if total[&m.name] > 1 {
            let n = seen.entry(m.name.clone()).or_default();
            *n += 1;
            let has_num = m.name.rsplit(' ').next().map_or(false, |w| w.chars().all(|c| c.is_ascii_digit()));
            m.name = if has_num { format!("{} ({})", m.name, n) } else { format!("{} {}", m.name, n) };
        }
    }
}

/* ---------------- speech recognition ---------------- */

pub struct Transcriber {
    ctx: whisper_rs::WhisperContext,
}

const PROMPT: &str = "Intro. Verse 1. Verse 2. Pre-Chorus. Chorus. Post-Chorus. Bridge. Instrumental. Interlude. Turnaround. Tag. Vamp. Outro. Ending. Breakdown. Build. Refrain.";

impl Transcriber {
    pub fn new(model: &Path) -> Result<Self, String> {
        whisper_rs::install_logging_hooks();
        let ctx = whisper_rs::WhisperContext::new_with_params(&model.to_string_lossy(), whisper_rs::WhisperContextParameters::default())
            .map_err(|e| format!("Couldn't load the speech model: {e}"))?;
        Ok(Transcriber { ctx })
    }

    /// `clip` is 16 kHz mono.
    pub fn transcribe(&self, clip: &[f32]) -> Result<String, String> {
        let mut st = self.ctx.create_state().map_err(|e| e.to_string())?;
        let mut p = whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
        p.set_language(Some("en"));
        p.set_n_threads(std::thread::available_parallelism().map_or(4, |n| n.get().min(8)) as i32);
        p.set_no_context(true);
        p.set_single_segment(true);
        p.set_no_timestamps(true);
        p.set_print_progress(false);
        p.set_print_realtime(false);
        p.set_print_special(false);
        p.set_print_timestamps(false);
        p.set_suppress_blank(true);
        p.set_initial_prompt(PROMPT);
        // Cues are a word or two: cap the output so the model can't repeat itself.
        p.set_max_tokens(8);
        p.set_temperature(0.0);
        let mut padded = vec![0.0f32; 8000];
        padded.extend_from_slice(clip);
        padded.extend(std::iter::repeat(0.0).take(8000));
        st.full(p, &padded).map_err(|e| e.to_string())?;
        let mut text = String::new();
        for seg in st.as_iter() {
            text.push_str(&seg.to_str_lossy().map_err(|e| e.to_string())?);
            text.push(' ');
        }
        Ok(text.trim().to_string())
    }
}

/// Result of reading a song's click and guide.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Analysis {
    pub bpm: Option<f64>,
    pub offset_ms: Option<f64>,
    pub markers: Vec<Marker>,
    /// What the guide said and when (seconds), for the "what was heard" list.
    pub heard: Vec<(f64, String)>,
}

/// The click's tempo, reconciled with a tempo known from the folder name: an
/// eighth-note click reads as double time, so it is counted in eighths instead.
pub fn tempo_with_known(x: &[f32], sr: u32, bpb: u32, known: Option<f64>) -> Option<Tempo> {
    let t = tempo_from_click(x, sr, bpb)?;
    let Some(k) = known.filter(|k| *k > 0.0) else { return Some(t) };
    let r = t.bpm / k;
    if (r - 2.0).abs() < 0.06 {
        let t2 = tempo_from_click(x, sr, bpb * 2).unwrap_or(t);
        return Some(Tempo { bpm: k, offset: t2.offset });
    }
    if (r - 1.0).abs() < 0.03 || (r - 0.5).abs() < 0.02 {
        return Some(Tempo { bpm: k, offset: t.offset });
    }
    Some(t)
}

/// Guides count bars in ("one, two, three, four"): every "one" is on a downbeat.
/// Shifts the downbeat by whole beats so the spoken "one"s line up with it.
pub fn align_to_counts(tempo: Tempo, heard: &[Heard], bpb: u32) -> Tempo {
    let ones: Vec<f64> = heard
        .iter()
        .filter(|h| {
            let w: String = h.text.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            w == "1" || w == "one"
        })
        .map(|h| h.start)
        .collect();
    if ones.is_empty() {
        return tempo;
    }
    let beat = 60.0 / tempo.bpm;
    let bar = beat * bpb.max(1) as f64;
    let err = |o: f64| -> f64 { ones.iter().map(|t| { let r = (t - o).rem_euclid(bar); r.min(bar - r) }).sum() };
    let mut best = (err(tempo.offset), tempo.offset);
    for k in 1..bpb.max(1) {
        let o = tempo.offset + k as f64 * beat;
        let e = err(o);
        if e < best.0 - 0.02 {
            best = (e, o);
        }
    }
    Tempo { bpm: tempo.bpm, offset: best.1.rem_euclid(bar) }
}

/// When the click starts: the first click in the file is bar 1, beat 1 (MultiTracks
/// stems, and click songs, start on the one). None if there's no click to hear.
pub fn first_click(x: &[f32], sr: u32) -> Option<f64> {
    onsets(x, sr).first().map(|o| if o.t < 0.03 { 0.0 } else { o.t })
}

/// Puts bar 1 on the first click. Sections keep the audio position they had, rounded
/// to the new bars, so a small correction moves them with the grid and a whole-bar
/// difference renumbers them. `x` is the click as it plays (after any tempo change).
/// Returns true if the grid changed.
pub fn snap_grid_to_click(f: &mut crate::songfile::SongFile, x: &[f32], sr: u32) -> bool {
    let Some(first) = first_click(x, sr) else { return false };
    let new = first / f.stretch(); // back to the recording's own time
    let cur = f.offset_ms / 1000.0;
    if (new - cur).abs() < 0.0005 {
        return false;
    }
    let bar = 60.0 / f.bpm * f.beats_per_bar.max(1) as f64;
    for m in f.markers.iter_mut() {
        let t = cur + (m.bar.max(1) - 1) as f64 * bar;
        m.bar = ((t - new) / bar).round().max(0.0) as u32 + 1;
    }
    let mut seen = 0;
    f.markers.retain(|m| {
        let keep = m.bar > seen;
        seen = seen.max(m.bar);
        keep
    });
    f.offset_ms = new * 1000.0;
    true
}

/// Where the guide says sections start, without needing to understand the words: each
/// "call" (a cue, plus any count after it) is followed by its section on the next
/// downbeat. Bursts of speech less than ~3/4 of a bar apart belong to the same call.
/// Returns 1-based bar numbers on the song's grid (bpm, bar 1 at `offset` seconds).
pub fn call_starts(guide: &[f32], sr: u32, bpm: f64, bpb: u32, offset: f64) -> Vec<u32> {
    let beat = 60.0 / bpm;
    let bar = beat * bpb.max(1) as f64;
    let cues = find_cues(guide, sr);
    let mut ends: Vec<f64> = vec![];
    let mut last_end = f64::NEG_INFINITY;
    for c in &cues {
        if c.start - last_end > 0.75 * bar {
            ends.push(c.end);
        } else if let Some(e) = ends.last_mut() {
            *e = c.end;
        }
        last_end = c.end;
    }
    let mut out: Vec<u32> = ends.iter().map(|e| ((e - 0.35 * beat - offset) / bar).ceil().max(0.0) as u32 + 1).collect();
    out.dedup();
    out
}

/// Moves each section marker onto the guide's section start beside it: a marker that
/// sits on the cue's own bar (one early) or one bar late moves to where the guide
/// says the section starts. Markers already on a start, or with no call nearby, stay.
pub fn align_markers_to_calls(markers: &mut Vec<Marker>, starts: &[u32]) -> bool {
    let mut changed = false;
    for m in markers.iter_mut() {
        if starts.contains(&m.bar) {
            continue;
        }
        if starts.contains(&(m.bar + 1)) {
            m.bar += 1;
            changed = true;
        } else if m.bar > 1 && starts.contains(&(m.bar - 1)) {
            m.bar -= 1;
            changed = true;
        }
    }
    markers.sort_by_key(|m| m.bar);
    markers.dedup_by_key(|m| m.bar);
    changed
}

/// Drops numbers the app added that aren't needed any more ("Intro 2" when there's
/// only one Intro). Returns (old name, new name) for each rename.
pub fn tidy_names(markers: &mut [Marker]) -> Vec<(String, String)> {
    fn base(n: &str) -> &str {
        let t = n.trim_end_matches(')');
        let cut = t.rfind(|c: char| c == ' ' || c == '(').map(|i| (&t[..i], &t[i + 1..]));
        match cut {
            Some((b, num)) if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) => b.trim_end(),
            _ => n,
        }
    }
    let mut renames = vec![];
    let bases: Vec<String> = markers.iter().map(|m| base(&m.name).to_string()).collect();
    for (i, m) in markers.iter_mut().enumerate() {
        let b = &bases[i];
        if b != &m.name && bases.iter().filter(|x| *x == b).count() == 1 && !b.is_empty() {
            // "Verse 2" with no other verse keeps its number only if it was the song's
            // own name; the app's numbering is always "Name N" from unique_names.
            if !matches!(b.to_ascii_lowercase().as_str(), "verse" | "chorus" | "bridge" | "tag" | "pre-chorus" | "post-chorus" | "refrain" | "vamp" | "interlude" | "instrumental" | "turnaround" | "breakdown" | "build" | "outro" | "ending" | "intro" | "solo" | "hook" | "rap") {
                continue;
            }
            if b.eq_ignore_ascii_case("verse") || b.eq_ignore_ascii_case("chorus") || b.eq_ignore_ascii_case("bridge") {
                continue; // "Verse 2" / "Chorus 3" are meaningful even alone
            }
            renames.push((m.name.clone(), b.clone()));
            m.name = b.clone();
        }
    }
    renames
}

#[allow(clippy::too_many_arguments)]
pub fn analyze(
    click: Option<&StemAudio>,
    guide: Option<&StemAudio>,
    sr: u32,
    bpb: u32,
    fallback_bpm: f64,
    known_bpm: Option<f64>,
    model: Option<&Path>,
    mut progress: impl FnMut(&str, f32),
) -> Result<Analysis, String> {
    progress("Finding the tempo", 0.0);
    let tempo = click.and_then(|c| tempo_with_known(&to_mono(c), sr, bpb, known_bpm));
    let mut use_tempo = tempo.unwrap_or(Tempo { bpm: known_bpm.unwrap_or(fallback_bpm), offset: 0.0 });
    let mut heard: Vec<Heard> = vec![];
    if let (Some(g), Some(model)) = (guide, model) {
        progress("Listening to the guide", 0.05);
        let mono = to_mono(g);
        let cues = find_cues(&mono, sr);
        let t = Transcriber::new(model)?;
        for (i, c) in cues.iter().enumerate() {
            progress("Listening to the guide", 0.05 + 0.95 * i as f32 / cues.len().max(1) as f32);
            let s0 = ((c.start - 0.1).max(0.0) * sr as f64) as usize;
            let s1 = (((c.end + 0.15) * sr as f64) as usize).min(mono.len());
            let mut text = String::new();
            if s1 > s0 && c.end - c.start <= 6.0 {
                // One unreadable cue shouldn't stop the whole song.
                if let Ok(clip) = crate::decode::resample(&[mono[s0..s1].to_vec()], sr, 16_000).map(|mut v| v.pop().unwrap_or_default()) {
                    text = t.transcribe(&clip).unwrap_or_default();
                }
            }
            heard.push(Heard { start: c.start, end: c.end, label: label_for(&text), text });
        }
    }
    // Bar 1 is the first click when there is one; otherwise the counts decide.
    match click.and_then(|c| first_click(&to_mono(c), sr)) {
        Some(first) if tempo.is_some() => use_tempo.offset = first,
        _ => use_tempo = align_to_counts(use_tempo, &heard, bpb),
    }
    progress("Done", 1.0);
    let found = tempo.is_some() || known_bpm.is_some();
    Ok(Analysis {
        bpm: found.then_some(use_tempo.bpm),
        offset_ms: found.then_some(use_tempo.offset * 1000.0),
        markers: place_markers(&heard, use_tempo, bpb),
        heard: heard.into_iter().map(|h| (h.start, h.text)).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click_track(sr: u32, bpm: f64, start: f64, beats: usize, accent: bool) -> Vec<f32> {
        let beat = 60.0 / bpm;
        let len = ((start + beats as f64 * beat + 1.0) * sr as f64) as usize;
        let mut x = vec![0.0f32; len];
        for k in 0..beats {
            let s0 = ((start + k as f64 * beat) * sr as f64) as usize;
            let (f, a) = if accent && k % 4 == 0 { (1760.0, 0.9) } else { (1175.0, 0.6) };
            for i in 0..(sr as usize / 25) {
                let t = i as f32 / sr as f32;
                x[s0 + i] += (std::f32::consts::TAU * f * t).sin() * a * (-t * 80.0).exp();
            }
        }
        x
    }

    #[test]
    fn finds_tempo_and_downbeat_from_click() {
        // Count-in starts on beat 3 (0.5 s), so the first downbeat is 2 beats later.
        let sr = 48_000;
        let beat = 60.0 / 72.0;
        let mut x = click_track(sr, 72.0, 0.5 + 2.0 * beat, 64, true);
        // Two pickup clicks before the first downbeat.
        let pick = click_track(sr, 72.0, 0.5, 2, false);
        for (i, v) in pick.iter().enumerate().take(x.len()) {
            x[i] += v;
        }
        let t = tempo_from_click(&x, sr, 4).unwrap();
        assert_eq!(t.bpm, 72.0);
        assert!((t.offset - (0.5 + 2.0 * beat)).abs() < 0.003, "offset {}", t.offset);
    }

    #[test]
    fn eighth_note_click_is_read_at_the_folder_tempo() {
        // 77.5 BPM song, click on every eighth (155/min), accents every 4 eighths
        // (twice a bar). Bar 1 starts at 0.5 s.
        let sr = 48_000;
        let x = click_track(sr, 155.0, 0.5, 64, true);
        let t = tempo_with_known(&x, sr, 4, Some(77.5)).unwrap();
        assert_eq!(t.bpm, 77.5);
        let bar = 60.0 / 77.5 * 4.0;
        // accents can't tell the two half-bars apart; the guide's "one"s can
        let heard = vec![h(0.5 + bar, 0.8 + bar, "One."), h(0.5 + 2.0 * bar, 0.8 + 2.0 * bar, "1")];
        let fixed = align_to_counts(t, &heard, 4);
        let d = (fixed.offset - 0.5).rem_euclid(bar);
        assert!(d.min(bar - d) < 0.01, "downbeat at {} (bar {bar})", fixed.offset);
    }

    #[test]
    fn bar_one_is_the_first_click_and_sections_move_with_the_grid() {
        let sr = 48_000;
        let bar = 60.0 / 77.5 * 4.0;
        let mk = |off: f64| crate::songfile::SongFile {
            name: "S".into(),
            bpm: 77.5,
            beats_per_bar: 4,
            offset_ms: off * 1000.0,
            markers: vec![crate::songfile::Marker { name: "Intro".into(), bar: 3 }, crate::songfile::Marker { name: "Verse".into(), bar: 7 }],
            ..Default::default()
        };
        // Eighth-note click from the very start of the file (a MultiTracks stem).
        let x = click_track(sr, 155.0, 0.01, 200, true);
        // The grid read 0.28 s late (not a whole number of clicks): bar 1 moves to 0.
        let mut f = mk(0.28);
        assert!(snap_grid_to_click(&mut f, &x, sr));
        assert_eq!(f.offset_ms, 0.0);
        assert_eq!((f.markers[0].bar, f.markers[1].bar), (3, 7), "sections move with the grid");
        assert!(!snap_grid_to_click(&mut f, &x, sr), "already right");
        // A grid a whole bar late renumbers the sections so they stay on the same audio.
        let mut g = mk(bar);
        assert!(snap_grid_to_click(&mut g, &x, sr));
        assert_eq!((g.markers[0].bar, g.markers[1].bar), (4, 8));
        // The click starts at the very first sample of the file (real MultiTracks stems
        // do): that first click is bar 1, not the one after it.
        let z = click_track(sr, 155.0, 0.0, 200, true);
        let mut z1 = mk(60.0 / 155.0); // the grid one click late, as 0.5.1 had it
        assert!(snap_grid_to_click(&mut z1, &z, sr));
        assert_eq!(z1.offset_ms, 0.0, "bar 1 at the very start");
        assert_eq!((z1.markers[0].bar, z1.markers[1].bar), (3, 7));
        assert_eq!(first_click(&z, sr), Some(0.0));
        // A click that starts later in the file: bar 1 is there.
        let y = click_track(sr, 155.0, 1.2, 100, true);
        let mut h2 = mk(0.0);
        assert!(snap_grid_to_click(&mut h2, &y, sr));
        assert!((h2.offset_ms - 1200.0).abs() < 2.0, "{}", h2.offset_ms);
    }

    #[test]
    fn band_cues_are_not_sections_and_breakdown_needs_a_count() {
        let tempo = Tempo { bpm: 60.0, offset: 0.0 }; // bar 4 s
        let heard = vec![
            h(1.0, 1.5, "Verse 1"),
            h(9.0, 9.5, "Build"),         // band cue
            h(11.0, 11.4, "Drums in"),    // band cue (not even labelled)
            h(17.0, 17.5, "Breakdown"),   // called mid-section, no count: not a section
            h(25.0, 25.6, "Breakdown"), h(28.0, 28.2, "1"), h(29.0, 29.2, "2"), h(30.0, 30.2, "3"), h(31.0, 31.2, "4"),
        ];
        let got: Vec<String> = place_markers(&heard, tempo, 4).into_iter().map(|m| m.name).collect();
        assert_eq!(got, ["Verse 1", "Breakdown"]);
        assert!(is_band_cue("Build Up") && is_band_cue("All In") && is_band_cue("Drums In") && !is_band_cue("Intro") && !is_band_cue("Breakdown"));
    }

    #[test]
    fn markers_move_a_bar_after_the_guide_call() {
        // 77.5 BPM, bar 1 at 0. Calls: count-in over bars 1-2, cues in bars 6, 14, 16
        // (with a count word) and 24. Ableton markers: Intro 3, Verse 7 (right), Tag 14
        // and Chorus 16 (on the cue: one early), Turnaround 25 (right).
        let sr = 16_000;
        let bpm = 77.5;
        let beat = 60.0 / bpm;
        let bar = beat * 4.0;
        let at = |b: u32, beat_in: f64| (b - 1) as f64 * bar + beat_in * beat;
        let mut x = vec![0f32; (sr as f64 * at(30, 0.0)) as usize];
        let mut say = |t: f64, len: f64| {
            for i in (t * sr as f64) as usize..((t + len) * sr as f64) as usize {
                x[i] = (i as f32 * 0.3).sin() * 0.4;
            }
        };
        for (b, bt) in [(1, 0.0), (1, 2.0), (2, 0.0), (2, 1.0), (2, 2.0), (2, 3.0)] {
            say(at(b, bt), 0.3); // "1 . 2 . | 1 2 3 4"
        }
        say(at(6, 0.0), 0.6); // "Verse one"
        say(at(14, 0.0), 0.4); // "Tag"
        say(at(16, 0.0), 0.5); // "Chorus"
        say(at(16, 1.0), 0.3); // "two"
        say(at(24, 0.0), 0.6); // "Turnaround"
        let starts = call_starts(&x, sr, bpm, 4, 0.0);
        assert_eq!(starts, vec![3, 7, 15, 17, 25]);
        let mk = |n: &str, b: u32| Marker { name: n.into(), bar: b };
        let mut m = vec![mk("Intro 2", 3), mk("Verse 1", 7), mk("Tag 1", 14), mk("Chorus 1", 16), mk("Turnaround", 25)];
        assert!(align_markers_to_calls(&mut m, &starts));
        let got: Vec<u32> = m.iter().map(|m| m.bar).collect();
        assert_eq!(got, [3, 7, 15, 17, 25]);
        let renames = tidy_names(&mut m);
        let names: Vec<&str> = m.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["Intro", "Verse 1", "Tag", "Chorus 1", "Turnaround"]);
        assert_eq!(renames, vec![("Intro 2".to_string(), "Intro".to_string()), ("Tag 1".to_string(), "Tag".to_string())]);
    }

    #[test]
    fn keeps_non_integer_tempos() {
        let t = tempo_from_click(&click_track(44_100, 68.5, 0.2, 40, true), 44_100, 4).unwrap();
        assert_eq!(t.bpm, 68.5);
    }

    #[test]
    fn cues_are_bursts_between_silence() {
        let sr = 16_000;
        let mut x = vec![0.0f32; sr as usize * 6];
        for (a, b) in [(1.0, 1.6), (3.0, 3.3), (3.45, 3.9)] {
            for i in (a * sr as f64) as usize..(b * sr as f64) as usize {
                x[i] = (i as f32 * 0.2).sin() * 0.3;
            }
        }
        let c = find_cues(&x, sr);
        assert_eq!(c.len(), 2, "{c:?}"); // the 150 ms gap joins "Verse" + "two"
        assert!((c[0].start - 1.0).abs() < 0.02 && (c[1].end - 3.9).abs() < 0.02);
    }

    #[test]
    fn labels_from_guide_words() {
        assert_eq!(label_for("Verse one."), Some("Verse 1".into()));
        assert_eq!(label_for("Verse 2"), Some("Verse 2".into()));
        assert_eq!(label_for("Pre-chorus."), Some("Pre-Chorus".into()));
        assert_eq!(label_for(" Chorus!"), Some("Chorus".into()));
        assert_eq!(label_for("Bridge."), Some("Bridge".into()));
        assert_eq!(label_for("Turn around"), Some("Turnaround".into()));
        assert_eq!(label_for("One, two, three, four."), None);
        assert_eq!(label_for("All in"), None);
        assert_eq!(label_for("Hits"), None);
    }

    fn h(start: f64, end: f64, text: &str) -> Heard {
        Heard { start, end, text: text.into(), label: label_for(text) }
    }

    #[test]
    fn section_starts_after_the_count_that_follows_the_cue() {
        let tempo = Tempo { bpm: 60.0, offset: 1.0 }; // beat 1 s, bar 4 s, bar 1 at 1 s
        let heard = vec![
            h(1.0, 1.3, "1"), h(2.0, 2.3, "2"), h(3.0, 3.3, "3"), h(4.0, 4.3, "4"), // count-in: no marker
            h(13.0, 13.8, "Verse one."),                                        // bar 4, beat 1
            h(17.0, 17.3, "1"), h(18.0, 18.3, "2"), h(19.0, 19.3, "Free."), h(20.0, 20.3, "4."), // bar 5 count
            h(30.0, 30.6, "Chorus"),                                             // bar 8, beat 2, no count
            h(46.2, 46.6, "Chorus"), h(49.0, 49.3, ""), h(50.0, 50.3, "two"), h(51.0, 51.3, "3"), h(52.0, 52.2, "4"),
        ];
        let got: Vec<(String, u32)> = place_markers(&heard, tempo, 4).into_iter().map(|m| (m.name, m.bar)).collect();
        assert_eq!(got, vec![
            ("Verse 1".into(), 6),   // after the count in bar 5
            ("Chorus 1".into(), 9),  // no count: next downbeat
            ("Chorus 2".into(), 14), // count in bar 13 (one word unreadable) -> bar 14
        ]);
    }

    /// Full run against a real guide file and model. Set RIG_GUIDE, RIG_MODEL, RIG_BPM.
    #[test]
    #[ignore]
    fn reads_a_real_guide() {
        let (Ok(g), Ok(m)) = (std::env::var("RIG_GUIDE"), std::env::var("RIG_MODEL")) else { return };
        let bpm: f64 = std::env::var("RIG_BPM").ok().and_then(|v| v.parse().ok()).unwrap_or(120.0);
        let click = std::env::var("RIG_CLICK").ok().map(|p| crate::decode::decode(Path::new(&p), 48_000).unwrap());
        let guide = crate::decode::decode(Path::new(&g), 48_000).unwrap();
        let known: Option<f64> = std::env::var("RIG_KNOWN").ok().and_then(|v| v.parse().ok());
        let a = analyze(click.as_ref(), Some(&guide), 48_000, 4, bpm, known, Some(Path::new(&m)), |_, _| {}).unwrap();
        println!("{a:#?}");
    }
}

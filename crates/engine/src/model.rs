//! Song, section, stem and arrangement data. Everything here is plain data:
//! it is built and edited outside the audio thread, then handed to the engine.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Where a stem (or the click) goes on the audio interface.
/// Channel numbers are 0-based: `Pair(0)` is outputs 1-2, `Mono(2)` is output 3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Route {
    Pair(u16),
    Mono(u16),
}

impl Default for Route {
    fn default() -> Self {
        Route::Pair(0)
    }
}

/// A named region of the original song audio, in source frames.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub name: String,
    pub start: u64,
    pub end: u64,
}

impl Section {
    pub fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Mixer settings for one stem.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StemInfo {
    pub name: String,
    #[serde(default)]
    pub route: Route,
    #[serde(default = "one")]
    pub gain: f32,
    #[serde(default)]
    pub muted: bool,
}

fn one() -> f32 {
    1.0
}

/// Beat position at time `t` on a beat grid (`beats[k]` = time of beat k, 0 = bar 1
/// beat 1). With fewer than two beats it's a steady grid (`offset`, `per_beat`).
/// Beyond the grid's ends the nearest beat length carries on. Works in any unit
/// (frames or seconds) and doesn't allocate.
pub fn beat_pos(beats: &[f64], offset: f64, per_beat: f64, t: f64) -> f64 {
    let b = beats;
    let n = b.len();
    if n < 2 {
        return (t - offset) / per_beat;
    }
    if t < b[0] {
        return (t - b[0]) / (b[1] - b[0]);
    }
    if t >= b[n - 1] {
        return (n - 1) as f64 + (t - b[n - 1]) / (b[n - 1] - b[n - 2]);
    }
    let i = b.partition_point(|&x| x <= t) - 1; // b[i] <= t < b[i+1]
    i as f64 + (t - b[i]) / (b[i + 1] - b[i])
}

/// Time of a beat position on a beat grid (the inverse of `beat_pos`).
pub fn beat_time(beats: &[f64], offset: f64, per_beat: f64, beat: f64) -> f64 {
    let b = beats;
    let n = b.len();
    if n < 2 {
        return offset + beat * per_beat;
    }
    if beat < 0.0 {
        return b[0] + beat * (b[1] - b[0]);
    }
    if beat >= (n - 1) as f64 {
        return b[n - 1] + (beat - (n - 1) as f64) * (b[n - 1] - b[n - 2]);
    }
    let i = beat.floor() as usize;
    b[i] + (beat - i as f64) * (b[i + 1] - b[i])
}

/// A song as delivered (e.g. a MultiTracks download): stems, tempo, sections.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Song {
    pub name: String,
    pub sample_rate: u32,
    pub bpm: f64,
    pub beats_per_bar: u32,
    /// Frame of the first downbeat (bar 1, beat 1).
    #[serde(default)]
    pub offset: u64,
    /// Length in frames of the longest stem.
    pub length: u64,
    pub sections: Vec<Section>,
    pub stems: Vec<StemInfo>,
    /// The spoken guide stem, if any. With `calls`, it says the right section before
    /// each block, however the song is arranged.
    #[serde(default)]
    pub guide_stem: Option<usize>,
    /// For each section (same order as `sections`): the guide's call for it (the cue and
    /// any count, in source frames, ending where the section starts), if the guide has one.
    #[serde(default)]
    pub calls: Vec<Option<(u64, u64)>>,
    /// Songs whose tempo changes: the source frame of every beat, from bar 1 beat 1
    /// (index 0). Empty = one steady tempo (`bpm`, from `offset`).
    #[serde(default)]
    pub beat_frames: Vec<f64>,
}

impl Song {
    pub fn frames_per_beat(&self) -> f64 {
        self.sample_rate as f64 * 60.0 / self.bpm
    }
    pub fn frames_per_bar(&self) -> f64 {
        self.frames_per_beat() * self.beats_per_bar as f64
    }
    /// Beat position (0 = bar 1 beat 1, fractional) at a source frame. Follows tempo
    /// changes when the song has a beat grid. Doesn't allocate.
    pub fn beat_pos(&self, f: f64) -> f64 {
        beat_pos(&self.beat_frames, self.offset as f64, self.frames_per_beat(), f)
    }
    /// Source frame of a beat position (the inverse of `beat_pos`).
    pub fn beat_frame(&self, beat: f64) -> f64 {
        beat_time(&self.beat_frames, self.offset as f64, self.frames_per_beat(), beat)
    }
    /// Tempo at a source frame.
    pub fn bpm_at(&self, f: f64) -> f64 {
        if self.beat_frames.len() < 2 {
            return self.bpm;
        }
        let b = self.beat_pos(f).floor();
        let len = self.beat_frame(b + 1.0) - self.beat_frame(b);
        if len > 0.0 {
            self.sample_rate as f64 * 60.0 / len
        } else {
            self.bpm
        }
    }
    pub fn section_index(&self, name: &str) -> Option<usize> {
        self.sections.iter().position(|s| s.name.eq_ignore_ascii_case(name))
    }
    /// Checks the song is internally consistent. Call before handing it to the engine.
    pub fn validate(&self) -> Result<(), String> {
        if self.sample_rate == 0 {
            return Err("sample rate is 0".into());
        }
        if !(self.bpm > 0.0) {
            return Err("tempo must be above 0 BPM".into());
        }
        if self.beats_per_bar == 0 {
            return Err("beats per bar must be at least 1".into());
        }
        if self.beat_frames.windows(2).any(|w| !(w[1] > w[0])) {
            return Err("the beat grid must move forward".into());
        }
        for s in &self.sections {
            if s.start >= s.end {
                return Err(format!("section \"{}\" ends before it starts", s.name));
            }
            if s.end > self.length {
                return Err(format!("section \"{}\" runs past the end of the song", s.name));
            }
        }
        Ok(())
    }
}

/// A stem trimmed inside one block: it only plays from `start` to `end` (frames from
/// the block's start), fading in and out over `fade_in` / `fade_out` frames.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trim {
    pub stem: usize,
    pub start: u64,
    pub end: u64,
    #[serde(default)]
    pub fade_in: u64,
    #[serde(default)]
    pub fade_out: u64,
}

impl Trim {
    /// Gain (0-1) for frame `f` of the block.
    #[inline]
    pub fn gain(&self, f: u64) -> f32 {
        if f < self.start || f >= self.end {
            return 0.0;
        }
        let mut g = 1.0f32;
        if self.fade_in > 0 && f - self.start < self.fade_in {
            g *= (f - self.start) as f32 / self.fade_in as f32;
        }
        if self.fade_out > 0 && self.end - f <= self.fade_out {
            g *= (self.end - f) as f32 / self.fade_out as f32;
        }
        g
    }
}

/// One block in the arrangement: play this section, with these stems muted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArrItem {
    pub section: usize,
    /// Stem indices silenced for this block only (e.g. drums out in verse 1).
    #[serde(default)]
    pub muted_stems: Vec<usize>,
    /// Stems that play only part of this block.
    #[serde(default)]
    pub trims: Vec<Trim>,
    /// (start, length) in frames from the block's start: everything fades out over
    /// `length` and the block ends there (e.g. a long ending cut short).
    #[serde(default)]
    pub fade_out: Option<(u64, u64)>,
}

impl ArrItem {
    pub fn new(section: usize) -> Self {
        ArrItem { section, muted_stems: vec![], trims: vec![], fade_out: None }
    }
}

/// The order sections play in. Repeats are just repeated items.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Arrangement {
    pub items: Vec<ArrItem>,
}

impl Arrangement {
    /// The song as recorded: every section once, in order.
    pub fn linear(song: &Song) -> Self {
        Arrangement {
            items: (0..song.sections.len())
                .map(ArrItem::new)
                .collect(),
        }
    }

    /// Builds an arrangement from section names, e.g. `["Verse 1", "Chorus", "Chorus"]`.
    pub fn from_names<S: AsRef<str>>(song: &Song, names: &[S]) -> Result<Self, String> {
        let items = names
            .iter()
            .map(|n| {
                song.section_index(n.as_ref())
                    .map(ArrItem::new)
                    .ok_or_else(|| format!("no section called \"{}\"", n.as_ref()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Arrangement { items })
    }

    pub fn validate(&self, song: &Song) -> Result<(), String> {
        for (i, it) in self.items.iter().enumerate() {
            if it.section >= song.sections.len() {
                return Err(format!("block {} points at a section that doesn't exist", i + 1));
            }
            if let Some(s) = it.muted_stems.iter().find(|&&s| s >= song.stems.len()) {
                return Err(format!("block {} mutes stem {} which doesn't exist", i + 1, s + 1));
            }
            if let Some(t) = it.trims.iter().find(|t| t.stem >= song.stems.len()) {
                return Err(format!("block {} trims stem {} which doesn't exist", i + 1, t.stem + 1));
            }
            if let Some((_, l)) = it.fade_out {
                if l == 0 {
                    return Err(format!("block {} has a fade-out of no length", i + 1));
                }
            }
        }
        Ok(())
    }
}

/// Decoded audio for one stem: interleaved f32, 1 or 2 channels.
/// Shared and read-only, so the audio thread never allocates or locks to read it.
#[derive(Clone, Debug)]
pub struct StemAudio {
    pub channels: usize,
    pub data: Arc<[f32]>,
}

impl StemAudio {
    pub fn mono(data: Vec<f32>) -> Self {
        StemAudio { channels: 1, data: data.into() }
    }
    pub fn stereo_interleaved(data: Vec<f32>) -> Self {
        StemAudio { channels: 2, data: data.into() }
    }
    pub fn frames(&self) -> u64 {
        (self.data.len() / self.channels.max(1)) as u64
    }
    /// Returns (left, right); silence past the end.
    #[inline]
    pub fn frame(&self, i: u64) -> (f32, f32) {
        let i = i as usize;
        match self.channels {
            1 => {
                let v = self.data.get(i).copied().unwrap_or(0.0);
                (v, v)
            }
            _ => {
                let j = i * self.channels;
                match (self.data.get(j), self.data.get(j + 1)) {
                    (Some(&l), Some(&r)) => (l, r),
                    _ => (0.0, 0.0),
                }
            }
        }
    }
}

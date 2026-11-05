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
}

impl Song {
    pub fn frames_per_beat(&self) -> f64 {
        self.sample_rate as f64 * 60.0 / self.bpm
    }
    pub fn frames_per_bar(&self) -> f64 {
        self.frames_per_beat() * self.beats_per_bar as f64
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

/// One block in the arrangement: play this section, with these stems muted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArrItem {
    pub section: usize,
    /// Stem indices silenced for this block only (e.g. drums out in verse 1).
    #[serde(default)]
    pub muted_stems: Vec<usize>,
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
                .map(|section| ArrItem { section, muted_stems: vec![] })
                .collect(),
        }
    }

    /// Builds an arrangement from section names, e.g. `["Verse 1", "Chorus", "Chorus"]`.
    pub fn from_names<S: AsRef<str>>(song: &Song, names: &[S]) -> Result<Self, String> {
        let items = names
            .iter()
            .map(|n| {
                song.section_index(n.as_ref())
                    .map(|section| ArrItem { section, muted_stems: vec![] })
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

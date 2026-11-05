//! A song is a folder of stems plus `rig-song.json` with tempo, section markers,
//! mixer settings and the arrangement. Sections and the arrangement are stored by
//! bar number and name, so they survive a change of sample rate.

use crate::decode::is_audio;
use rig_engine::{ArrItem, Arrangement, Route, Section, Song, StemInfo};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "rig-song.json";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Marker {
    pub name: String,
    /// 1-based bar number where the section starts.
    pub bar: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StemFile {
    pub file: String,
    pub name: String,
    pub route: Route,
    pub gain: f32,
    pub muted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Block {
    pub section: String,
    #[serde(default)]
    pub muted: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SongFile {
    pub name: String,
    pub bpm: f64,
    pub beats_per_bar: u32,
    #[serde(default)]
    pub offset_ms: f64,
    #[serde(default)]
    pub key: String,
    /// Built-in click. Off by default when the song has its own Click stem.
    pub click: bool,
    pub markers: Vec<Marker>,
    pub stems: Vec<StemFile>,
    /// Empty means "as recorded".
    #[serde(default)]
    pub arrangement: Vec<Block>,
    /// Where the markers came from: None (not set up yet), "guide" or "manual".
    #[serde(default)]
    pub markers_from: Option<String>,
}

pub fn is_click(name: &str) -> bool {
    name.to_ascii_lowercase().contains("click")
}

pub fn is_guide(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("guide") || n.contains("cue")
}

fn is_cue(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("click") || n.contains("guide") || n.contains("cue")
}

pub fn default_route(name: &str, channels: usize) -> Route {
    if is_cue(name) && channels >= 4 {
        Route::Pair(2)
    } else {
        Route::Pair(0)
    }
}

/// Finds "72 BPM", "72bpm" or "bpm 72" in a name.
pub fn bpm_in(name: &str) -> Option<f64> {
    let n = name.to_ascii_lowercase();
    let i = n.find("bpm")?;
    let before: String = n[..i].trim_end().chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let before: String = before.chars().rev().collect();
    if let Ok(v) = before.parse::<f64>() {
        return Some(v).filter(|v| (30.0..=300.0).contains(v));
    }
    let after: String = n[i + 3..].trim_start().chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    after.parse::<f64>().ok().filter(|v| (30.0..=300.0).contains(v))
}

fn stem_name(file: &str) -> String {
    Path::new(file).file_stem().and_then(|s| s.to_str()).unwrap_or(file).trim().to_string()
}

impl SongFile {
    pub fn path(folder: &Path) -> PathBuf {
        folder.join(FILE_NAME)
    }

    /// Reads `rig-song.json` if present, then reconciles it with the audio files actually
    /// in the folder (new files are added, missing ones dropped).
    pub fn scan(folder: &Path, channels: usize) -> Result<SongFile, String> {
        let mut files: Vec<String> = std::fs::read_dir(folder)
            .map_err(|e| format!("Can't open {}: {e}", folder.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_audio(p))
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
            .collect();
        files.sort_by_key(|f| (!is_cue(f), f.to_ascii_lowercase()));
        if files.is_empty() {
            return Err(format!("No audio files in {}. Pick the folder that holds the stems.", folder.display()));
        }
        let folder_name = folder.file_name().and_then(|n| n.to_str()).unwrap_or("Untitled").to_string();
        let mut sf = match std::fs::read_to_string(Self::path(folder)) {
            Ok(text) => serde_json::from_str::<SongFile>(&text).map_err(|e| format!("{FILE_NAME} is damaged: {e}"))?,
            Err(_) => SongFile {
                name: folder_name.clone(),
                bpm: std::iter::once(folder_name.as_str()).chain(files.iter().map(String::as_str)).find_map(bpm_in).unwrap_or(120.0),
                beats_per_bar: 4,
                offset_ms: 0.0,
                key: String::new(),
                click: !files.iter().any(|f| f.to_ascii_lowercase().contains("click")),
                markers: vec![Marker { name: "Top".into(), bar: 1 }],
                stems: vec![],
                arrangement: vec![],
                markers_from: None,
            },
        };
        sf.stems.retain(|s| files.contains(&s.file));
        for f in &files {
            if !sf.stems.iter().any(|s| &s.file == f) {
                let name = stem_name(f);
                sf.stems.push(StemFile { route: default_route(&name, channels), file: f.clone(), name, gain: 1.0, muted: false });
            }
        }
        Ok(sf)
    }

    pub fn save(&self, folder: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(Self::path(folder), text).map_err(|e| format!("Couldn't save {FILE_NAME}: {e}"))
    }

    /// Turns bar-based markers and named blocks into engine data.
    pub fn build(&self, sample_rate: u32, length: u64) -> Result<(Song, Arrangement), String> {
        if !(self.bpm > 0.0) {
            return Err("Tempo must be above 0 BPM".into());
        }
        let bpb = self.beats_per_bar.max(1);
        let offset = (self.offset_ms.max(0.0) / 1000.0 * sample_rate as f64).round() as u64;
        let bar = sample_rate as f64 * 60.0 / self.bpm * bpb as f64;
        let mut marks: Vec<(String, u64)> = self
            .markers
            .iter()
            .map(|m| (m.name.clone(), offset + ((m.bar.max(1) - 1) as f64 * bar).round() as u64))
            .filter(|(_, s)| *s < length)
            .collect();
        marks.sort_by_key(|m| m.1);
        marks.dedup_by_key(|m| m.1);
        if marks.first().map_or(true, |m| m.1 > 0) {
            marks.insert(0, ("Top".into(), 0));
        }
        let sections: Vec<Section> = marks
            .iter()
            .enumerate()
            .map(|(i, (name, start))| Section { name: name.clone(), start: *start, end: marks.get(i + 1).map_or(length, |m| m.1) })
            .collect();
        let song = Song {
            name: self.name.clone(),
            sample_rate,
            bpm: self.bpm,
            beats_per_bar: bpb,
            offset,
            length,
            sections,
            stems: self
                .stems
                .iter()
                .map(|s| StemInfo { name: s.name.clone(), route: s.route, gain: s.gain, muted: s.muted })
                .collect(),
        };
        let items: Vec<ArrItem> = self
            .arrangement
            .iter()
            .filter_map(|b| {
                let section = song.section_index(&b.section)?;
                let muted_stems = b.muted.iter().filter_map(|m| self.stems.iter().position(|s| &s.name == m)).collect();
                Some(ArrItem { section, muted_stems })
            })
            .collect();
        let arr = if items.is_empty() { Arrangement::linear(&song) } else { Arrangement { items } };
        song.validate()?;
        arr.validate(&song)?;
        Ok((song, arr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_bpm_in_names() {
        assert_eq!(bpm_in("Way Maker 68 BPM"), Some(68.0));
        assert_eq!(bpm_in("click_72bpm.wav"), Some(72.0));
        assert_eq!(bpm_in("BPM 140 - Drums"), Some(140.0));
        assert_eq!(bpm_in("Drums.wav"), None);
    }

    #[test]
    fn markers_and_blocks_become_engine_data() {
        let sf = SongFile {
            name: "S".into(),
            bpm: 60.0,
            beats_per_bar: 4,
            offset_ms: 500.0,
            key: "D".into(),
            click: true,
            markers: vec![Marker { name: "Verse".into(), bar: 1 }, Marker { name: "Chorus".into(), bar: 3 }],
            stems: vec![StemFile { file: "Drums.wav".into(), name: "Drums".into(), route: Route::Pair(0), gain: 1.0, muted: false }],
            arrangement: vec![
                Block { section: "Chorus".into(), muted: vec![] },
                Block { section: "Verse".into(), muted: vec!["Drums".into()] },
                Block { section: "Missing".into(), muted: vec![] },
            ],
            markers_from: None,
        };
        let (song, arr) = sf.build(1000, 20_000).unwrap();
        let names: Vec<&str> = song.sections.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Top", "Verse", "Chorus"], "pickup before the first downbeat becomes Top");
        assert_eq!(song.sections[1].start, 500);
        assert_eq!(song.sections[2].start, 8500);
        assert_eq!(song.sections[2].end, 20_000);
        assert_eq!(arr.items.len(), 2, "unknown section names are skipped");
        assert_eq!(arr.items[1].muted_stems, vec![0]);
    }

    #[test]
    fn scan_builds_defaults_and_routes_cues_to_in_ears() {
        let dir = std::env::temp_dir().join(format!("rig-scan-{} 72 BPM", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["Drums.wav", "Click.wav", "Guide.wav", "notes.txt"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let sf = SongFile::scan(&dir, 10).unwrap();
        assert_eq!(sf.bpm, 72.0);
        assert!(!sf.click, "song has its own click stem");
        let names: Vec<&str> = sf.stems.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Click", "Guide", "Drums"]);
        assert_eq!(sf.stems[0].route, Route::Pair(2));
        assert_eq!(sf.stems[2].route, Route::Pair(0));
        std::fs::remove_dir_all(dir).ok();
    }
}

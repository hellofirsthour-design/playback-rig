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

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
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
    /// Key change in semitones (+2 = up a whole step). Click and guide keep their pitch.
    #[serde(default)]
    pub transpose: i32,
    /// Playback tempo, when different from the recorded tempo (`bpm`).
    #[serde(default)]
    pub target_bpm: Option<f64>,
    /// Version of the automatic setup that last ran (see SETUP_VERSION).
    #[serde(default)]
    pub setup: u32,
    /// Version of the click-grid rule last applied (see GRID_VERSION). Older values
    /// get the grid lined up again on load; markers keep their bars.
    #[serde(default)]
    pub grid_version: u32,
}

/// Raise to make songs set up automatically by an older version set up again.
pub const SETUP_VERSION: u32 = 3;

/// Raise to line up the grid of automatically set up songs with the click again.
pub const GRID_VERSION: u32 = 4;

/// Where a stem sits in the track list: click and guide first, then Drums,
/// Percussion, Bass, Guitars, Keys, Other instruments, Vocals, FX.
pub fn stem_category(name: &str) -> u8 {
    let n = name.to_ascii_lowercase();
    let words: Vec<&str> = n.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |keys: &[&str]| keys.iter().any(|k| n.contains(k));
    let word = |keys: &[&str]| words.iter().any(|w| keys.contains(w));
    if has(&["click", "guide", "cue"]) {
        0
    } else if word(&["fx", "sfx", "riser", "risers", "swell", "swells", "impact", "impacts", "noise"]) || has(&["effect"]) {
        8
    } else if has(&["vox", "vocal", "voc ", "bgv", "alto", "tenor", "soprano", "choir", "harmon", "lead vox", "sing"]) || word(&["voc", "bv", "bvs", "vocals"]) {
        7
    } else if has(&["perc", "shaker", "tamb", "conga", "bongo", "clap", "loop", "cajon"]) {
        2
    } else if has(&["drum", "kick", "snare", "hat", "tom", "overhead", "cymbal", "kit"]) {
        1
    } else if has(&["bass", "sub"]) {
        3
    } else if has(&["guitar", "gtr", "electric", "acoustic"]) || word(&["eg", "ag", "eg1", "eg2", "eg3", "ag1", "ag2"]) {
        4
    } else if has(&["key", "piano", "rhodes", "wurli", "organ", "synth", "pad", "b3", "ep"]) {
        5
    } else {
        6
    }
}

/// Track-list order: category, then name with numbers in number order (EG 2 before EG 10).
pub fn stem_sort_key(name: &str) -> (u8, Vec<(String, u64)>) {
    let mut parts = vec![];
    let mut text = String::new();
    let mut num = String::new();
    for c in name.to_ascii_lowercase().chars() {
        if c.is_ascii_digit() {
            num.push(c);
        } else {
            if !num.is_empty() {
                parts.push((std::mem::take(&mut text), num.parse().unwrap_or(0)));
                num.clear();
            }
            text.push(c);
        }
    }
    parts.push((text, num.parse().unwrap_or(0)));
    (stem_category(name), parts)
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

pub fn stem_name(file: &str) -> String {
    Path::new(file).file_stem().and_then(|s| s.to_str()).unwrap_or(file).trim().to_string()
}

fn audio_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file() && is_audio(p))
                .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
                .filter(|n| !n.starts_with("._"))
                .collect()
        })
        .unwrap_or_default()
}

/// Stem files relative to the song folder. Looks in the folder itself, then in its
/// subfolders (a "MultiTracks" folder first), as MultiTracks downloads are laid out.
pub fn find_stems(folder: &Path) -> Result<Vec<String>, String> {
    let mut files = audio_in(folder);
    if files.is_empty() {
        let mut subs: Vec<PathBuf> = std::fs::read_dir(folder)
            .map_err(|e| format!("Can't open {}: {e}", folder.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .filter(|p| !p.file_name().and_then(|n| n.to_str()).map_or(true, |n| n.starts_with('.') || n.eq_ignore_ascii_case("Ableton Project Info")))
            .collect();
        subs.sort_by_key(|p| (!p.file_name().and_then(|n| n.to_str()).map_or(false, |n| n.to_ascii_lowercase().contains("multitrack")), p.clone()));
        for d in subs {
            let found = audio_in(&d);
            if !found.is_empty() {
                let rel = d.file_name().unwrap().to_string_lossy().to_string();
                files = found.into_iter().map(|f| format!("{rel}/{f}")).collect();
                break;
            }
        }
    }
    if files.is_empty() {
        return Err(format!("No audio files in {} or its subfolders. Pick the song's folder (the one with the stems or a MultiTracks folder inside).", folder.display()));
    }
    files.sort_by_key(|f| (!is_cue(&stem_name(f)), stem_name(f).to_ascii_lowercase()));
    Ok(files)
}

/// The song's name as shown in folders: the folder's own name, or its parent's
/// when the chosen folder is the "MultiTracks" stems folder itself.
pub fn song_title(folder: &Path) -> String {
    let own = folder.file_name().and_then(|n| n.to_str()).unwrap_or("Untitled");
    if own.eq_ignore_ascii_case("multitracks") || own.eq_ignore_ascii_case("stems") {
        if let Some(p) = folder.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()) {
            return p.to_string();
        }
    }
    own.to_string()
}

/// "See a Victory-At Midnight-A-77.50bpm" -> ("See a Victory", Some("A"), Some(77.5)).
pub fn parse_folder_name(title: &str) -> (String, Option<String>, Option<f64>) {
    let parts: Vec<&str> = title.split('-').map(str::trim).filter(|p| !p.is_empty()).collect();
    let bpm = bpm_in(title);
    let is_key = |p: &str| {
        let p = p.trim();
        let mut c = p.chars();
        matches!(c.next(), Some('A'..='G')) && c.as_str().chars().all(|ch| matches!(ch, '#' | 'b' | 'm')) && p.len() <= 4
    };
    let key = parts.iter().rev().find(|p| is_key(p)).map(|p| p.to_string());
    let name = parts.first().map(|p| p.to_string()).filter(|p| bpm_in(p).is_none() && !is_key(p)).unwrap_or_else(|| title.to_string());
    (name, key, bpm)
}

impl SongFile {
    pub fn path(folder: &Path) -> PathBuf {
        folder.join(FILE_NAME)
    }

    /// Reads `rig-song.json` if present, then reconciles it with the audio files actually
    /// in the folder (new files are added, missing ones dropped).
    pub fn scan(folder: &Path, channels: usize) -> Result<SongFile, String> {
        let files = find_stems(folder)?;
        let title = song_title(folder);
        let (name, key, folder_bpm) = parse_folder_name(&title);
        let existing = std::fs::read_to_string(Self::path(folder)).ok();
        let mut sf = match existing {
            Some(text) => serde_json::from_str::<SongFile>(&text).map_err(|e| format!("{FILE_NAME} is damaged: {e}"))?,
            None => SongFile {
                name,
                bpm: folder_bpm.or_else(|| files.iter().find_map(|f| bpm_in(f))).unwrap_or(120.0),
                beats_per_bar: 4,
                offset_ms: 0.0,
                key: key.unwrap_or_default(),
                click: !files.iter().any(|f| is_click(&stem_name(f))),
                markers: vec![Marker { name: "Top".into(), bar: 1 }],
                stems: vec![],
                arrangement: vec![],
                markers_from: None,
                transpose: 0,
                target_bpm: None,
                setup: SETUP_VERSION,
                grid_version: 0,
            },
        };
        // Songs set up automatically by an older version are set up again: the tempo
        // could come out at double time and markers could miss the bar grid.
        if sf.setup < SETUP_VERSION {
            if matches!(sf.markers_from.as_deref(), None | Some("ableton") | Some("guide")) {
                sf.markers_from = None;
                sf.grid_version = 0;
                if let Some(b) = folder_bpm {
                    sf.bpm = b;
                }
            }
            sf.setup = SETUP_VERSION;
        }
        // A MultiTracks download includes an Ableton set with the exact section markers.
        if sf.markers_from.is_none() {
            if let Some(info) = crate::als::find(folder).and_then(|p| crate::als::read(&p).ok()) {
                sf.apply_ableton(&info, folder_bpm);
            }
        }
        sf.stems.retain(|s| files.contains(&s.file));
        for f in &files {
            if !sf.stems.iter().any(|s| &s.file == f) {
                let name = stem_name(f);
                sf.stems.push(StemFile { route: default_route(&name, channels), file: f.clone(), name, gain: 1.0, muted: false });
            }
        }
        sf.stems.sort_by_key(|st| stem_sort_key(&st.name));
        Ok(sf)
    }

    /// Takes the meter and section locators from an Ableton set. Locators are turned
    /// into seconds through the clip's own timing (start and warp markers), then
    /// snapped to the song's bar grid. `known_bpm` (from the folder name) wins over
    /// the set's tempo, which MultiTracks often runs at double time (eighth-note click).
    pub fn apply_ableton(&mut self, info: &crate::als::AlsInfo, known_bpm: Option<f64>) -> bool {
        let live_bpm = info.bpm.or(known_bpm).unwrap_or(self.bpm);
        let bpm = known_bpm.unwrap_or(live_bpm);
        self.bpm = bpm;
        if let Some(n) = info.beats_per_bar {
            self.beats_per_bar = n;
        }
        if info.locators.is_empty() {
            return false;
        }
        let bpb = self.beats_per_bar.max(1) as f64;
        let beat = 60.0 / bpm;
        let bar = beat * bpb;
        let secs: Vec<(f64, &String)> = info.locators.iter().map(|(b, n)| (info.beat_to_sec(*b), n)).collect();
        // The arrangement starts on a downbeat; choose the beat phase that puts the
        // locators closest to bar lines (matters when the set runs at double time).
        let start = info.beat_to_sec(0.0);
        let off_err = |o: f64| -> f64 {
            secs.iter().map(|(t, _)| { let r = (t - o).rem_euclid(bar); r.min(bar - r) }).sum()
        };
        let mut offset = start;
        let mut best = off_err(start);
        for k in 1..bpb as i64 {
            let o = start + k as f64 * beat;
            let e = off_err(o);
            if e < best - 1e-6 {
                best = e;
                offset = o;
            }
        }
        let offset = offset.rem_euclid(bar);
        let mut markers: Vec<Marker> = vec![];
        for (i, (t, name)) in secs.iter().enumerate() {
            // Band/MD cues ("Build", "Drums in") aren't parts of the song's form.
            if crate::guide::is_band_cue(name) {
                continue;
            }
            let b = ((t - offset) / bar).round().max(0.0) as u32 + 1;
            if markers.last().map_or(false, |m| m.bar >= b) {
                continue;
            }
            let name = if name.is_empty() { format!("Section {}", i + 1) } else { (*name).clone() };
            markers.push(Marker { name, bar: b });
        }
        crate::guide::unique_names(&mut markers);
        self.markers = markers;
        self.offset_ms = offset * 1000.0;
        self.arrangement = vec![];
        self.markers_from = Some("ableton".into());
        true
    }

    /// Tempo the song plays at.
    pub fn play_bpm(&self) -> f64 {
        self.target_bpm.filter(|b| *b > 0.0).unwrap_or(self.bpm)
    }

    /// Output length / recorded length (1.0 = no tempo change).
    pub fn stretch(&self) -> f64 {
        self.bpm / self.play_bpm()
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
        let bpm = self.play_bpm();
        let bpb = self.beats_per_bar.max(1);
        let offset = (self.offset_ms.max(0.0) * self.stretch() / 1000.0 * sample_rate as f64).round() as u64;
        let bar = sample_rate as f64 * 60.0 / bpm * bpb as f64;
        let mut marks: Vec<(String, u64)> = self
            .markers
            .iter()
            .map(|m| (m.name.clone(), offset + ((m.bar.max(1) - 1) as f64 * bar).round() as u64))
            .filter(|(_, s)| *s < length)
            .collect();
        marks.sort_by_key(|m| m.1);
        marks.dedup_by_key(|m| m.1);
        // Audio before the first section: the count-in (a bar or more) or a pickup.
        let lead = if marks.first().map_or(true, |m| m.1 > 0) {
            let first = marks.first().map_or(length, |m| m.1);
            let name = if first as f64 >= bar * 0.9 { "Count-in" } else { "Top" };
            marks.insert(0, (name.into(), 0));
            Some(name)
        } else {
            None
        };
        let sections: Vec<Section> = marks
            .iter()
            .enumerate()
            .map(|(i, (name, start))| Section { name: name.clone(), start: *start, end: marks.get(i + 1).map_or(length, |m| m.1) })
            .collect();
        let song = Song {
            name: self.name.clone(),
            sample_rate,
            bpm,
            beats_per_bar: bpb,
            offset,
            length,
            sections,
            stems: self
                .stems
                .iter()
                .map(|s| StemInfo { name: s.name.clone(), route: s.route, gain: s.gain, muted: s.muted })
                .collect(),
            guide_stem: None,
            calls: vec![],
        };
        let items: Vec<ArrItem> = self
            .arrangement
            .iter()
            .filter_map(|b| {
                // Saved arrangements may call the lead-in "Top" or "Count-in".
                let lead_alias = lead.is_some() && (b.section == "Top" || b.section == "Count-in");
                let section = song.section_index(&b.section).or(lead_alias.then_some(0))?;
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
    fn reads_multitracks_folder_names() {
        assert_eq!(parse_folder_name("See a Victory-At Midnight-A-77.50bpm"), ("See a Victory".into(), Some("A".into()), Some(77.5)));
        assert_eq!(parse_folder_name("Way Maker-Bb-68bpm"), ("Way Maker".into(), Some("Bb".into()), Some(68.0)));
        assert_eq!(parse_folder_name("Goodness of God"), ("Goodness of God".into(), None, None));
    }

    #[test]
    fn finds_stems_in_a_multitracks_subfolder_and_reads_the_ableton_set() {
        let dir = std::env::temp_dir().join(format!("rig-mt-{}", std::process::id())).join("See a Victory-At Midnight-A-77.50bpm");
        std::fs::create_dir_all(dir.join("MultiTracks")).unwrap();
        std::fs::create_dir_all(dir.join("Ableton Project Info")).unwrap();
        for f in ["Drums.wav", "Click.wav", "Guide.wav", "._Drums.wav"] {
            std::fs::write(dir.join("MultiTracks").join(f), b"x").unwrap();
        }
        std::fs::write(dir.join("Album.jpg"), b"x").unwrap();
        std::fs::write(dir.join("MultiTrack.als"), r#"<Ableton><LiveSet><Locators><Locators>
            <Locator Id="0"><Time Value="0" /><Name Value="Intro" /></Locator>
            <Locator Id="1"><Time Value="16" /><Name Value="Verse" /></Locator>
            <Locator Id="2"><Time Value="48" /><Name Value="Chorus" /></Locator>
            <Locator Id="3"><Time Value="80" /><Name Value="Chorus" /></Locator>
            </Locators></Locators><Tempo><LomId Value="0" /><Manual Value="77.5" /></Tempo><Numerator Value="4" /></LiveSet></Ableton>"#).unwrap();
        let sf = SongFile::scan(&dir, 10).unwrap();
        assert_eq!(sf.name, "See a Victory");
        assert_eq!(sf.key, "A");
        assert_eq!(sf.bpm, 77.5);
        let files: Vec<&str> = sf.stems.iter().map(|s| s.file.as_str()).collect();
        assert_eq!(files, ["MultiTracks/Click.wav", "MultiTracks/Guide.wav", "MultiTracks/Drums.wav"]);
        assert_eq!(sf.markers_from.as_deref(), Some("ableton"));
        let m: Vec<(&str, u32)> = sf.markers.iter().map(|m| (m.name.as_str(), m.bar)).collect();
        assert_eq!(m, [("Intro", 1), ("Verse", 5), ("Chorus 1", 13), ("Chorus 2", 21)]);
        // picking the MultiTracks folder itself works too
        let sf2 = SongFile::scan(&dir.join("MultiTracks"), 10).unwrap();
        assert_eq!(sf2.name, "See a Victory");
        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
    }

    #[test]
    fn double_time_ableton_set_lands_on_the_real_bar_grid() {
        // Live runs at 155 (eighth-note click) for a 77.5 BPM song. The clip starts at
        // arrangement beat 0, 0.1 s into the audio.
        let info = crate::als::AlsInfo {
            bpm: Some(155.0),
            beats_per_bar: Some(4),
            locators: vec![(16.0, "Intro".into()), (48.0, "Verse".into()), (80.0, "Build".into()), (112.0, "Chorus".into())],
            clip: Some(crate::als::ClipTiming { time: 0.0, start: 0.0, warped: true, warp: vec![(0.0, 0.1), (155.0, 60.1)] }),
        };
        let mut sf = SongFile { name: "S".into(), bpm: 120.0, beats_per_bar: 4, ..Default::default() };
        sf.apply_ableton(&info, Some(77.5));
        assert_eq!(sf.bpm, 77.5);
        assert!((sf.offset_ms - 100.0).abs() < 0.5, "offset {}", sf.offset_ms);
        let m: Vec<(&str, u32)> = sf.markers.iter().map(|m| (m.name.as_str(), m.bar)).collect();
        assert_eq!(m, [("Intro", 3), ("Verse", 7), ("Chorus", 15)]);
        // every section is a whole number of bars on the 77.5 grid
        let (song, _) = sf.build(48_000, 48_000 * 60).unwrap();
        let bar = song.frames_per_bar();
        for s in &song.sections[1..song.sections.len() - 1] {
            let bars = s.len() as f64 / bar;
            assert!((bars - bars.round()).abs() * bar < 1.5, "{} is {bars} bars (more than a sample off)", s.name);
        }
    }

    #[test]
    fn tempo_change_scales_section_positions() {
        let mut sf = SongFile { name: "S".into(), bpm: 60.0, beats_per_bar: 4, offset_ms: 1000.0, markers: vec![Marker { name: "A".into(), bar: 1 }, Marker { name: "B".into(), bar: 3 }], ..Default::default() };
        sf.target_bpm = Some(120.0); // twice as fast: half the length
        let (song, _) = sf.build(1000, 9000).unwrap();
        assert_eq!(song.bpm, 120.0);
        assert_eq!(song.sections[1].start, 500, "downbeat moves from 1.0 s to 0.5 s");
        assert_eq!(song.sections[2].start, 500 + 4000);
    }

    #[test]
    fn tracks_are_ordered_like_a_band() {
        let mut v = vec!["BGV", "EG 2", "Keys 1", "Alto", "Bass", "Drums (Live)", "FX", "Click Track", "Perc Loop", "EG 10", "Strings", "AG", "Guide", "Choir", "Pad", "Synth Bass", "Rhodes"];
        v.sort_by_key(|n| stem_sort_key(n));
        assert_eq!(v, ["Click Track", "Guide", "Drums (Live)", "Perc Loop", "Bass", "Synth Bass", "AG", "EG 2", "EG 10", "Keys 1", "Pad", "Rhodes", "Strings", "Alto", "BGV", "Choir", "FX"]);
    }

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
                transpose: 0,
                target_bpm: None,
                setup: SETUP_VERSION,
                grid_version: 0,
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

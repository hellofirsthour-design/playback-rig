//! Reads an Ableton Live set (.als, gzipped XML) such as the MultiTrack.als that
//! comes with MultiTracks downloads: tempo, time signature and the arrangement
//! locators, which are the song's section markers.

use flate2::read::GzDecoder;
use regex::Regex;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct AlsInfo {
    /// The set's master tempo (Live's beat grid; may be double/half the song's feel).
    pub bpm: Option<f64>,
    pub beats_per_bar: Option<u32>,
    /// (position in beats from the start of the arrangement, name)
    pub locators: Vec<(f64, String)>,
    /// How arrangement beats map onto the audio files (from the first audio clip).
    pub clip: Option<ClipTiming>,
}

/// Timing of an arrangement audio clip: where it starts and how it is warped.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipTiming {
    /// Arrangement position of the clip, in beats.
    pub time: f64,
    /// Clip-internal beat that plays at `time` (start marker).
    pub start: f64,
    pub warped: bool,
    /// Warp markers: (clip beat, seconds in the audio file).
    pub warp: Vec<(f64, f64)>,
}

impl AlsInfo {
    /// Seconds into the audio files at an arrangement position (beats).
    pub fn beat_to_sec(&self, beat: f64) -> f64 {
        let tempo = self.bpm.unwrap_or(120.0);
        let Some(c) = &self.clip else { return beat * 60.0 / tempo };
        let cb = beat - c.time + c.start;
        if !c.warped || c.warp.is_empty() {
            return (beat - c.time) * 60.0 / tempo + c.start * 60.0 / tempo;
        }
        if c.warp.len() == 1 {
            let (b0, s0) = c.warp[0];
            return s0 + (cb - b0) * 60.0 / tempo;
        }
        // piecewise linear between warp markers, extended past the ends
        let seg = c.warp.windows(2).position(|w| cb < w[1].0).unwrap_or(c.warp.len() - 2);
        let (b0, s0) = c.warp[seg];
        let (b1, s1) = c.warp[seg + 1];
        if (b1 - b0).abs() < 1e-9 {
            return s0;
        }
        s0 + (cb - b0) * (s1 - s0) / (b1 - b0)
    }
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn section<'a>(xml: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let a = xml.find(open)?;
    let rest = &xml[a..];
    Some(&rest[..rest.find(close).map_or(rest.len(), |b| b + close.len())])
}

fn num(text: &str, re: &str) -> Option<f64> {
    Regex::new(re).unwrap().captures(text).and_then(|c| c[1].parse::<f64>().ok())
}

pub fn parse(xml: &str) -> AlsInfo {
    // Tempo and meter live on the master track. (Every track's mixer also has a
    // Tempo entry, so searching the whole file picks up the wrong one.)
    let master = section(xml, "<MasterTrack>", "</MasterTrack>").unwrap_or(xml);
    let bpm = num(master, r#"(?s)<Tempo>.*?<Manual Value="([0-9.eE+]+)""#).filter(|b| (20.0..=400.0).contains(b));
    // Live encodes the meter as numerator-1 + 99*log2(denominator): 4/4 = 201, 6/8 = 302.
    let beats_per_bar = num(master, r#"(?s)<TimeSignature>.*?<Manual Value="(\d+)""#)
        .map(|v| (v as u32 % 99) + 1)
        .or_else(|| num(xml, r#"<Numerator Value="(\d+)""#).map(|n| n as u32))
        .filter(|n| (1..=16).contains(n));
    let mut locators = vec![];
    // Ableton nests <Locators><Locators>…</Locators></Locators>; the entries all sit
    // before the first closing tag.
    if let Some(start) = xml.find("<Locators>") {
        let rest = &xml[start..];
        let body = &rest[..rest.find("</Locators>").unwrap_or(rest.len())];
        let loc = Regex::new(r#"(?s)<Locator Id="\d+">(.*?)</Locator>"#).unwrap();
        let time = Regex::new(r#"<Time Value="([-0-9.eE+]+)""#).unwrap();
        let name = Regex::new(r#"<Name Value="([^"]*)""#).unwrap();
        for c in loc.captures_iter(body) {
            let b = &c[1];
            if let Some(t) = time.captures(b).and_then(|m| m[1].parse::<f64>().ok()) {
                let n = name.captures(b).map(|m| unescape(&m[1])).unwrap_or_default();
                locators.push((t, n.trim().to_string()));
            }
        }
    }
    locators.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let clip = Regex::new(r#"<AudioClip Id="\d+" Time="([-0-9.eE+]+)""#).unwrap().captures(xml).and_then(|c| {
        let time: f64 = c[1].parse().ok()?;
        let body = section(&xml[c.get(0)?.start()..], "<AudioClip", "</AudioClip>")?;
        let loop_start = num(body, r#"<LoopStart Value="([-0-9.eE+]+)""#).unwrap_or(0.0);
        let start_rel = num(body, r#"<StartRelative Value="([-0-9.eE+]+)""#).unwrap_or(0.0);
        let warped = Regex::new(r#"<IsWarped Value="(true|false)""#).unwrap().captures(body).map_or(true, |m| &m[1] == "true");
        let wm = Regex::new(r#"<WarpMarker Id="\d+" SecTime="([-0-9.eE+]+)" BeatTime="([-0-9.eE+]+)""#).unwrap();
        let mut warp: Vec<(f64, f64)> = wm.captures_iter(body).filter_map(|m| Some((m[2].parse().ok()?, m[1].parse().ok()?))).collect();
        warp.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        Some(ClipTiming { time, start: loop_start + start_rel, warped, warp })
    });
    AlsInfo { bpm, beats_per_bar, locators, clip }
}

pub fn read(path: &Path) -> Result<AlsInfo, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    let mut xml = String::new();
    if bytes.starts_with(&[0x1f, 0x8b]) {
        GzDecoder::new(&bytes[..]).read_to_string(&mut xml).map_err(|e| format!("The Ableton set is damaged: {e}"))?;
    } else {
        xml = String::from_utf8_lossy(&bytes).into_owned();
    }
    Ok(parse(&xml))
}

/// Finds the Ableton set for a song folder (in it, or next to a "MultiTracks" folder).
pub fn find(folder: &Path) -> Option<PathBuf> {
    let look = |d: &Path| -> Option<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(d).ok()?.filter_map(|e| e.ok()).map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()).map_or(false, |e| e.eq_ignore_ascii_case("als")))
            .filter(|p| !p.file_name().and_then(|n| n.to_str()).map_or(true, |n| n.starts_with("._")))
            .collect();
        v.sort();
        v.into_iter().next()
    };
    look(folder).or_else(|| folder.parent().and_then(look))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton MajorVersion="5" MinorVersion="11.0_433">
<LiveSet>
  <Locators>
    <Locators>
      <Locator Id="0"><LomId Value="0" /><Time Value="16" /><Name Value="Verse 1" /><Annotation Value="" /><IsSongStart Value="false" /></Locator>
      <Locator Id="1"><LomId Value="0" /><Time Value="0" /><Name Value="Intro" /><Annotation Value="" /></Locator>
      <Locator Id="2"><LomId Value="0" /><Time Value="80" /><Name Value="Chorus &amp; Tag" /></Locator>
    </Locators>
  </Locators>
  <MasterTrack>
    <DeviceChain><Mixer>
      <Tempo>
        <LomId Value="0" />
        <Manual Value="77.5" />
      </Tempo>
      <TimeSignature><LomId Value="0" /><Manual Value="201" /></TimeSignature>
    </Mixer></DeviceChain>
  </MasterTrack>
  <RemoteableTimeSignature><Numerator Value="4" /><Denominator Value="4" /></RemoteableTimeSignature>
</LiveSet>
</Ableton>"#;

    #[test]
    fn reads_tempo_meter_and_locators() {
        let a = parse(SAMPLE);
        assert_eq!(a.bpm, Some(77.5));
        assert_eq!(a.beats_per_bar, Some(4));
        assert!(a.clip.is_none());
        assert_eq!(a.locators, vec![(0.0, "Intro".into()), (16.0, "Verse 1".into()), (80.0, "Chorus & Tag".into())]);
    }

    #[test]
    fn master_tempo_wins_and_warp_markers_map_beats_to_seconds() {
        // A track mixer's Tempo (120) comes before the master's (155) in the file.
        let xml = r#"<LiveSet><Tracks><AudioTrack><DeviceChain><Mixer><Tempo><LomId Value="0" /><Manual Value="120" /></Tempo></Mixer>
          <AudioClip Id="3" Time="4"><LoopStart Value="0" /><StartRelative Value="0" /><IsWarped Value="true" />
          <WarpMarkers><WarpMarker Id="0" SecTime="0.25" BeatTime="0" /><WarpMarker Id="1" SecTime="31.25" BeatTime="80" /></WarpMarkers></AudioClip>
          </DeviceChain></AudioTrack></Tracks>
          <MasterTrack><DeviceChain><Mixer><Tempo><LomId Value="0" /><Manual Value="155" /></Tempo>
          <TimeSignature><LomId Value="0" /><Manual Value="302" /></TimeSignature></Mixer></DeviceChain></MasterTrack></LiveSet>"#;
        let a = parse(xml);
        assert_eq!(a.bpm, Some(155.0));
        assert_eq!(a.beats_per_bar, Some(6), "302 = 6/8");
        // clip starts at arrangement beat 4; its beat 0 is 0.25 s into the file; 80 beats per 31 s
        assert!((a.beat_to_sec(4.0) - 0.25).abs() < 1e-9);
        assert!((a.beat_to_sec(84.0) - 31.25).abs() < 1e-9);
        assert!((a.beat_to_sec(0.0) - (0.25 - 4.0 * 31.0 / 80.0)).abs() < 1e-9, "before the clip: extend the first segment");
    }

    #[test]
    fn reads_a_gzipped_set() {
        use flate2::write::GzEncoder;
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("rig-als-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("MultiTracks")).unwrap();
        let mut gz = GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(SAMPLE.as_bytes()).unwrap();
        std::fs::write(dir.join("MultiTrack.als"), gz.finish().unwrap()).unwrap();
        // found both from the song folder and from its MultiTracks subfolder
        assert!(find(&dir).is_some());
        let p = find(&dir.join("MultiTracks")).unwrap();
        assert_eq!(read(&p).unwrap().locators.len(), 3);
        std::fs::remove_dir_all(dir).ok();
    }
}

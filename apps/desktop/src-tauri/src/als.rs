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
    /// The time signature at the start of the song: (beats, note value), e.g. (6, 8).
    pub meter: Option<(u32, u32)>,
    /// (position in beats from the start of the arrangement, name)
    pub locators: Vec<(f64, String)>,
    /// How arrangement beats map onto the audio files (from the first audio clip).
    pub clip: Option<ClipTiming>,
    /// The master tempo's automation: (arrangement beat, BPM), in order. Live ramps
    /// linearly between points; two points at the same beat are a jump. Empty when
    /// the tempo isn't automated.
    pub tempo_points: Vec<(f64, f64)>,
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
    /// Tempo automation with more than one tempo in it.
    pub fn tempo_varies(&self) -> bool {
        self.tempo_points.windows(2).any(|w| (w[1].1 - w[0].1).abs() > 0.01)
    }

    /// Seconds of arrangement time at an arrangement beat, following tempo automation.
    pub fn arr_sec(&self, beat: f64) -> f64 {
        let base = self.bpm.unwrap_or(120.0);
        let pts = &self.tempo_points;
        if pts.is_empty() {
            return beat * 60.0 / base;
        }
        // tempo at beat 0 and every point after it
        let at = |b: f64| -> f64 {
            match pts.iter().rposition(|p| p.0 <= b) {
                None => pts[0].1,
                Some(i) if i + 1 < pts.len() && pts[i + 1].0 > pts[i].0 => {
                    let (b0, v0) = pts[i];
                    let (b1, v1) = pts[i + 1];
                    v0 + (v1 - v0) * (b - b0) / (b1 - b0)
                }
                Some(i) => pts[i].1,
            }
        };
        let seg = |b0: f64, b1: f64| -> f64 {
            // linear tempo from b0 to b1: integral of 60/bpm
            let (v0, v1) = (at(b0), at(b1 - 1e-9));
            if (v1 - v0).abs() < 1e-9 {
                60.0 * (b1 - b0) / v0
            } else {
                60.0 * (b1 - b0) / (v1 - v0) * (v1 / v0).ln()
            }
        };
        let (lo, hi, sign) = if beat >= 0.0 { (0.0, beat, 1.0) } else { (beat, 0.0, -1.0) };
        let mut cuts: Vec<f64> = vec![lo];
        cuts.extend(pts.iter().map(|p| p.0).filter(|&b| b > lo && b < hi));
        cuts.push(hi);
        sign * cuts.windows(2).map(|w| if w[1] > w[0] { seg(w[0], w[1]) } else { 0.0 }).sum::<f64>()
    }

    /// Seconds into the audio at an arrangement beat, following tempo automation.
    /// A warped clip maps beats to audio through its warp markers; an unwarped one
    /// plays in real time from where it starts.
    pub fn audio_sec(&self, beat: f64) -> f64 {
        if self.tempo_points.is_empty() {
            return self.beat_to_sec(beat);
        }
        match &self.clip {
            Some(c) if c.warped && c.warp.len() >= 2 => self.beat_to_sec(beat),
            Some(c) => self.arr_sec(beat) - self.arr_sec(c.time) + c.start * 60.0 / self.bpm.unwrap_or(120.0),
            None => self.arr_sec(beat),
        }
    }

    /// The song's beat grid from the tempo automation (seconds into the audio of each
    /// beat, beat 0 = bar 1 at `offset`), when the tempo changes. `song_bpm` is the
    /// song's own tempo, to tell a set running at double time; `len` is the audio's length.
    pub fn beat_grid(&self, song_bpm: f64, offset: f64, len: f64) -> Option<Vec<f64>> {
        if !self.tempo_varies() {
            return None;
        }
        let live = self.bpm.or_else(|| self.tempo_points.first().map(|p| p.1)).unwrap_or(song_bpm);
        let r = (live / song_bpm).round().clamp(1.0, 4.0);
        // The arrangement beat that plays at `offset` (bar 1): search, as audio_sec is monotonic.
        let (mut a, mut b) = (-64.0, 100_000.0);
        for _ in 0..80 {
            let m = (a + b) / 2.0;
            if self.audio_sec(m) < offset {
                a = m;
            } else {
                b = m;
            }
        }
        let b0 = (a + b) / 2.0;
        let mut out = vec![];
        let mut k = 0.0;
        loop {
            let t = self.audio_sec(b0 + r * k);
            if t > len + 1.0 || out.len() > 20_000 {
                break;
            }
            out.push(t);
            k += 1.0;
        }
        (out.len() >= 2 && out.windows(2).all(|w| w[1] > w[0])).then_some(out)
    }

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
    // The song's meter is the time signature automation at the start of the song if the
    // set has any (signature markers), else the master's own setting.
    let decode = |v: u32| -> Option<(u32, u32)> {
        let (n, d) = (v % 99 + 1, 1u32 << (v / 99).min(5));
        ((1..=16).contains(&n) && [1, 2, 4, 8, 16, 32].contains(&d)).then_some((n, d))
    };
    let ts = section(master, "<TimeSignature>", "</TimeSignature>").unwrap_or("");
    let automated = Regex::new(r#"<AutomationTarget Id="(\d+)""#).unwrap().captures(ts).and_then(|c| {
        let id = c[1].to_string();
        let env = Regex::new(r#"(?s)<AutomationEnvelope Id="\d+">(.*?)</AutomationEnvelope>"#).unwrap();
        let ev = Regex::new(r#"<EnumEvent Id="\d+" Time="([-0-9.eE+]+)" Value="(\d+)""#).unwrap();
        let body = env
            .captures_iter(xml)
            .map(|c| c.get(1).map_or("", |m| m.as_str()))
            .find(|b| b.contains(&format!(r#"<PointeeId Value="{id}" />"#)) || b.contains(&format!(r#"<PointeeId Value="{id}"/>"#)))?;
        let first = ev
            .captures_iter(body)
            .filter_map(|m| Some((m[1].parse::<f64>().ok()?, m[2].parse::<u32>().ok()?)))
            .filter(|(t, _)| *t <= 0.0)
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        first.and_then(|(_, v)| decode(v))
    });
    let manual = num(ts, r#"<Manual Value="(\d+)""#).and_then(|v| decode(v as u32));
    let remote = num(xml, r#"<Numerator Value="(\d+)""#).zip(num(xml, r#"<Denominator Value="(\d+)""#)).map(|(n, d)| (n as u32, d as u32));
    let meter = automated.or(manual).or(remote).filter(|(n, d)| (1..=16).contains(n) && [2, 4, 8, 16].contains(d));
    let beats_per_bar = meter.map(|m| m.0);
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
    // Tempo automation: the master tempo's automation target, and the envelope pointing at it.
    let mut tempo_points: Vec<(f64, f64)> = vec![];
    if let Some(id) = Regex::new(r#"(?s)<Tempo>.*?<AutomationTarget Id="(\d+)""#).unwrap().captures(master).map(|c| c[1].to_string()) {
        let env = Regex::new(r#"(?s)<AutomationEnvelope Id="\d+">(.*?)</AutomationEnvelope>"#).unwrap();
        let ev = Regex::new(r#"<FloatEvent Id="\d+" Time="([-0-9.eE+]+)" Value="([-0-9.eE+]+)""#).unwrap();
        let target = format!(r#"<PointeeId Value="{id}" />"#);
        let target2 = format!(r#"<PointeeId Value="{id}"/>"#);
        for c in env.captures_iter(xml) {
            let body = &c[1];
            if body.contains(&target) || body.contains(&target2) {
                for m in ev.captures_iter(body) {
                    if let (Ok(t), Ok(v)) = (m[1].parse::<f64>(), m[2].parse::<f64>()) {
                        if (20.0..=400.0).contains(&v) {
                            // Live's "before the song" event sits at a huge negative time.
                            tempo_points.push((t.max(0.0), v));
                        }
                    }
                }
                break;
            }
        }
    }
    tempo_points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    AlsInfo { bpm, beats_per_bar, meter, locators, clip, tempo_points }
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

    #[test]
    fn reads_six_eight_from_the_master_or_its_automation() {
        // Set as the master's own setting
        let a = parse(r#"<Ableton><LiveSet><MasterTrack><DeviceChain><Mixer><Tempo><Manual Value="90" /></Tempo>
            <TimeSignature><LomId Value="0" /><Manual Value="302" /></TimeSignature></Mixer></DeviceChain></MasterTrack></LiveSet></Ableton>"#);
        assert_eq!(a.meter, Some((6, 8)));
        // Master left at 4/4, the song's 6/8 is a time signature marker (automation) at the start
        let b = parse(r#"<Ableton><LiveSet><MasterTrack><DeviceChain><Mixer><Tempo><Manual Value="90" /></Tempo>
            <TimeSignature><LomId Value="0" /><Manual Value="201" /><AutomationTarget Id="9"><LockEnvelope Value="0" /></AutomationTarget></TimeSignature></Mixer></DeviceChain>
            <AutomationEnvelopes><Envelopes><AutomationEnvelope Id="0"><EnvelopeTarget><PointeeId Value="9" /></EnvelopeTarget><Automation><Events>
              <EnumEvent Id="1" Time="-63072000" Value="302" /><EnumEvent Id="2" Time="96" Value="201" />
            </Events></Automation></AutomationEnvelope></Envelopes></AutomationEnvelopes></MasterTrack></LiveSet></Ableton>"#);
        assert_eq!(b.meter, Some((6, 8)), "the signature at the start of the song wins");
        assert_eq!(b.beats_per_bar, Some(6));
        // 3/4 (2 + 99*2 = 200) still reads as before
        let c = parse(r#"<MasterTrack><TimeSignature><Manual Value="200" /></TimeSignature></MasterTrack>"#);
        assert_eq!(c.meter, Some((3, 4)));
    }

    #[test]
    fn reads_tempo_automation_and_integrates_it() {
        let xml = r#"<Ableton><LiveSet><MasterTrack><DeviceChain><Mixer>
            <Tempo><LomId Value="0" /><Manual Value="120" /><AutomationTarget Id="8"><LockEnvelope Value="0" /></AutomationTarget></Tempo>
            </Mixer></DeviceChain>
            <AutomationEnvelopes><Envelopes>
              <AutomationEnvelope Id="0"><EnvelopeTarget><PointeeId Value="3" /></EnvelopeTarget><Automation><Events>
                <FloatEvent Id="1" Time="0" Value="0.5" /></Events></Automation></AutomationEnvelope>
              <AutomationEnvelope Id="1"><EnvelopeTarget><PointeeId Value="8" /></EnvelopeTarget><Automation><Events>
                <FloatEvent Id="2" Time="-63072000" Value="120" />
                <FloatEvent Id="3" Time="32" Value="120" />
                <FloatEvent Id="4" Time="32" Value="150" />
                <FloatEvent Id="5" Time="64" Value="150" />
                <FloatEvent Id="6" Time="72" Value="75" />
              </Events></Automation></AutomationEnvelope>
            </Envelopes></AutomationEnvelopes></MasterTrack></LiveSet></Ableton>"#;
        let info = parse(xml);
        assert_eq!(info.tempo_points.len(), 5);
        assert!(info.tempo_varies());
        // 32 beats at 120 = 16 s, then 32 at 150 = 12.8 s
        assert!((info.arr_sec(32.0) - 16.0).abs() < 1e-9);
        assert!((info.arr_sec(64.0) - 28.8).abs() < 1e-9);
        // a ramp from 150 down to 75 over 8 beats: 60*8/(75-150)*ln(75/150) s
        let ramp = 60.0 * 8.0 / (75.0 - 150.0) * (75.0f64 / 150.0).ln();
        assert!((info.arr_sec(72.0) - (28.8 + ramp)).abs() < 1e-9);
        // no clip: the grid is the arrangement's own beats from 0
        let g = info.beat_grid(120.0, 0.0, 40.0).unwrap();
        assert!((g[32] - 16.0).abs() < 1e-6 && (g[64] - 28.8).abs() < 1e-6);
        // a steady set has no grid
        assert!(parse(SAMPLE).beat_grid(77.5, 0.0, 100.0).is_none());
    }

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

//! Reads an Ableton Live set (.als, gzipped XML) such as the MultiTrack.als that
//! comes with MultiTracks downloads: tempo, time signature and the arrangement
//! locators, which are the song's section markers.

use flate2::read::GzDecoder;
use regex::Regex;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct AlsInfo {
    pub bpm: Option<f64>,
    pub beats_per_bar: Option<u32>,
    /// (position in beats from the start of the arrangement, name)
    pub locators: Vec<(f64, String)>,
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

pub fn parse(xml: &str) -> AlsInfo {
    let num = |re: &str| Regex::new(re).unwrap().captures(xml).and_then(|c| c[1].parse::<f64>().ok());
    let bpm = num(r#"(?s)<Tempo>.*?<Manual Value="([0-9.]+)""#).filter(|b| (20.0..=400.0).contains(b));
    let beats_per_bar = num(r#"<Numerator Value="(\d+)""#).map(|n| n as u32).filter(|n| (1..=16).contains(n));
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
    AlsInfo { bpm, beats_per_bar, locators }
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
        assert_eq!(a.locators, vec![(0.0, "Intro".into()), (16.0, "Verse 1".into()), (80.0, "Chorus & Tag".into())]);
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

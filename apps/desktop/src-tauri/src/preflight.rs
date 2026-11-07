//! "Check the set": everything that goes wrong on a Sunday morning because of setup, not
//! because of a missing feature. Run it before the service; it changes nothing.

use crate::songfile::{self, SongFile};
use rig_engine::Route;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Level {
    Ok,
    Warn,
    Bad,
}

#[derive(Clone, Debug, Serialize)]
pub struct Check {
    pub level: Level,
    pub text: String,
}

fn ok(t: impl Into<String>) -> Check {
    Check { level: Level::Ok, text: t.into() }
}
fn warn(t: impl Into<String>) -> Check {
    Check { level: Level::Warn, text: t.into() }
}
fn bad(t: impl Into<String>) -> Check {
    Check { level: Level::Bad, text: t.into() }
}

/// What the rig looks like right now.
pub struct Rig<'a> {
    /// The interface chosen in Setup (None = the system default).
    pub want_output: Option<&'a str>,
    /// The output that's open: name and number of outputs.
    pub open_output: Option<(&'a str, usize)>,
    /// Where the click bus, the guide bus, front of house (the drums bus) and the built-in click go.
    pub click: Route,
    pub guide: Route,
    pub foh: Route,
    pub builtin_click: Route,
    pub want_midi: Option<&'a str>,
    pub midi_connected: bool,
    pub on_battery: Option<bool>,
    pub master_gain: f32,
    pub pads_folder: Option<&'a Path>,
    /// Pad files found, by pitch class.
    pub pad_keys: [bool; 12],
    pub band: &'a [String],
}

fn channels(r: Route) -> Vec<u16> {
    match r {
        Route::Pair(n) => vec![n, n + 1],
        Route::Mono(n) => vec![n],
    }
}
fn overlaps(a: Route, b: Route) -> bool {
    let b = channels(b);
    channels(a).iter().any(|c| b.contains(c))
}
fn outs(r: Route) -> String {
    match r {
        Route::Pair(n) => format!("Out {}-{}", n + 1, n + 2),
        Route::Mono(n) => format!("Out {}", n + 1),
    }
}

/// Every check, worst first within each part (rig, then songs in set order).
pub fn run(rig: &Rig, songs: &[(PathBuf, Result<SongFile, String>)]) -> Vec<Check> {
    let mut v = vec![];
    // The interface.
    match (rig.want_output, rig.open_output) {
        (_, None) => v.push(bad("No audio output is open. Plug in the interface (or pick one in Setup → Audio output).")),
        (Some(w), Some((o, _))) if w != o => v.push(bad(format!("Your interface ({w}) isn't connected: playing on {o}. Plug it in; the app switches back by itself when stopped."))),
        (_, Some((o, ch))) if ch < 4 => v.push(warn(format!("{o} has only {ch} outputs, so click and guide play in front of house. Pick your interface in Setup → Audio output."))),
        (_, Some((o, ch))) => v.push(ok(format!("Audio: {o}, {ch} outputs."))),
    }
    // In-ears vs front of house.
    if rig.open_output.is_some_and(|(_, ch)| ch >= 4) {
        let mut ears = true;
        for (what, r) in [("The click track", rig.click), ("The guide", rig.guide), ("The built-in click", rig.builtin_click)] {
            if overlaps(r, rig.foh) {
                v.push(bad(format!("{what} goes to {}, the same as front of house. Setup → Outputs.", outs(r))));
                ears = false;
            }
        }
        if ears {
            v.push(ok(format!("Click and guide go to {}, front of house to {}.", outs(rig.click), outs(rig.foh))));
        }
    }
    if rig.master_gain < 0.1 {
        v.push(warn("The master fader is all the way down."));
    }
    // The controller.
    match (rig.want_midi, rig.midi_connected) {
        (None, _) => v.push(warn("No MIDI controller chosen (Setup → MIDI controller).")),
        (Some(m), false) => v.push(bad(format!("{m} isn't connected. Check its USB cable; it reconnects by itself."))),
        (Some(m), true) => v.push(ok(format!("Controller: {m}."))),
    }
    if rig.on_battery == Some(true) {
        v.push(warn("The Mac is on battery. Plug in the charger."));
    }
    if rig.band.is_empty() {
        v.push(ok("Live tonight: nothing marked, every track plays."));
    } else {
        v.push(ok(format!("Live tonight: {} (their tracks are quiet).", rig.band.join(", "))));
    }
    if songs.is_empty() {
        v.push(warn("The set is empty."));
    }
    // Each song.
    let mut fine = 0;
    for (i, (folder, f)) in songs.iter().enumerate() {
        let n = i + 1;
        let label = |f: Option<&SongFile>| format!("{n}. {}", f.map(|f| f.name.clone()).unwrap_or_else(|| folder.file_name().and_then(|x| x.to_str()).unwrap_or("Song").to_string()));
        let f = match f {
            Err(e) => {
                v.push(bad(format!("{}: {e}", label(None))));
                continue;
            }
            Ok(f) => f,
        };
        let name = label(Some(f));
        let before = v.len();
        let missing: Vec<&str> = f.stems.iter().filter(|s| !folder.join(&s.file).exists()).map(|s| s.name.as_str()).collect();
        if !missing.is_empty() {
            v.push(bad(format!("{name}: missing {}.", missing.join(", "))));
        }
        if f.stems.is_empty() {
            v.push(bad(format!("{name}: no audio files.")));
        }
        let has_click = f.stems.iter().any(|s| songfile::is_click(&s.name) && !s.muted);
        if !has_click && !f.click {
            v.push(warn(format!("{name}: no click (no click track, built-in click off).")));
        }
        if f.bus_mix.get("cues").is_some_and(|b| b.muted || b.gain < 0.05) {
            v.push(warn(format!("{name}: the Click fader is down or muted in this song's mix.")));
        }
        if !f.stems.iter().any(|s| songfile::is_guide(&s.name)) {
            v.push(warn(format!("{name}: no guide track, so nobody hears the section calls.")));
        }
        let wants_pad = f.transition.pad || f.transition.pad_start != songfile::PadStart::Off;
        if wants_pad {
            match (rig.pads_folder, crate::song_key_pc(f)) {
                (None, _) => v.push(warn(format!("{name}: uses the pad, but no pads folder is set."))),
                (Some(_), Some(pc)) if !rig.pad_keys[pc as usize] => v.push(warn(format!("{name}: uses the pad, but there's no pad in {}.", crate::pads::NAMES[pc as usize]))),
                (Some(_), None) => v.push(warn(format!("{name}: uses the pad, but has no key set."))),
                _ => {}
            }
        }
        if v.len() == before {
            fine += 1;
        }
    }
    if fine > 0 {
        v.push(ok(format!("{fine} of {} songs ready: files there, click and guide in place.", songs.len())));
    }
    v
}

/// Whether the Mac is running on battery (None where it can't tell).
pub fn on_battery() -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("pmset").args(["-g", "batt"]).output().ok()?;
        let t = String::from_utf8_lossy(&out.stdout);
        Some(t.contains("Battery Power"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rig<'a>(band: &'a [String]) -> Rig<'a> {
        Rig {
            want_output: Some("UMC1820"),
            open_output: Some(("UMC1820", 10)),
            click: Route::Pair(2),
            guide: Route::Pair(2),
            foh: Route::Pair(0),
            builtin_click: Route::Pair(2),
            want_midi: Some("MIDI Mix"),
            midi_connected: true,
            on_battery: Some(false),
            master_gain: 1.0,
            pads_folder: None,
            pad_keys: [false; 12],
            band,
        }
    }

    #[test]
    fn a_good_rig_is_all_ok_and_the_click_in_front_of_house_is_caught() {
        let r = run(&rig(&[]), &[]);
        assert!(r.iter().all(|c| c.level != Level::Bad), "{r:?}");
        let mut bad_rig = rig(&[]);
        bad_rig.click = Route::Pair(0);
        bad_rig.open_output = Some(("MacBook Air Speakers", 2));
        let r = run(&bad_rig, &[]);
        assert!(r.iter().any(|c| c.level == Level::Bad && c.text.contains("isn't connected")), "{r:?}");
        let mut foh = rig(&[]);
        foh.guide = Route::Mono(1);
        let r = run(&foh, &[]);
        assert!(r.iter().any(|c| c.level == Level::Bad && c.text.starts_with("The guide goes to Out 2")), "{r:?}");
    }

    #[test]
    fn songs_with_missing_files_or_no_click_are_flagged() {
        let dir = std::env::temp_dir().join(format!("rig-pre-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Click Track.wav"), b"x").unwrap();
        let st = |f: &str| songfile::StemFile { file: f.into(), name: f.trim_end_matches(".wav").into(), route: Route::Pair(0), gain: 1.0, muted: false };
        let mut f: SongFile = serde_json::from_str(&serde_json::to_string(&SongFile::scan(&dir, 2).unwrap()).unwrap()).unwrap();
        f.name = "Worthy".into();
        f.stems = vec![st("Click Track.wav"), st("Guide.wav")];
        let mut g = f.clone();
        g.name = "Gratitude".into();
        g.stems = vec![st("Click Track.wav")];
        g.stems[0].muted = true;
        g.click = false;
        let r = run(&rig(&[]), &[(dir.clone(), Ok(f)), (dir.clone(), Ok(g))]);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(r.iter().any(|c| c.level == Level::Bad && c.text == "1. Worthy: missing Guide."), "{r:?}");
        assert!(r.iter().any(|c| c.level == Level::Warn && c.text.starts_with("2. Gratitude: no click")), "{r:?}");
    }
}

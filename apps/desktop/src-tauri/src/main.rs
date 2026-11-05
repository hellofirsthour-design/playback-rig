// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod als;
mod audio;
mod clicksong;
mod decode;
mod guide;
mod metronome;
mod midi_io;
mod model;
mod pads;
mod songfile;
mod stretch;

use audio::{OpenOutput, Output, OutputInfo, ToAudio};
use rig_engine::midi::{Action, Binding, Event, MidiMap};
use rig_engine::{Arrangement, Command, Engine, Quantize, Route, Snapshot, Song, StemAudio};
use serde::{Deserialize, Serialize};
use songfile::{Block, Ending, Marker, SongFile, TrimSpec, Transition};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_updater::UpdaterExt;

/* ---------------- state ---------------- */

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    output: Option<String>,
    midi: Option<String>,
    setlist: Vec<PathBuf>,
    current: Option<usize>,
    quantize: Quantize,
    count_in: bool,
    click_gain: f32,
    click_route: Option<Route>,
    /// Settings format; older settings are migrated on load.
    version: u32,
    /// Folder of pad files, one per key.
    pads_folder: Option<PathBuf>,
    pad_level: f32,
    pad_route: Option<Route>,
    /// The free click's last tempo and meter.
    free_click_bpm: f64,
    free_click_bpb: u32,
    /// 0.5.0's per-group buses (read once to carry their settings over; never written).
    #[serde(skip_serializing)]
    buses: Vec<Bus>,
    /// The buses, in track-list order. Shared by every song.
    bus_list: Vec<Bus>,
    /// Tracks moved to a bus by hand, by track name (lowercase) -> bus id.
    /// Applies to that track name in every song.
    bus_of_track: std::collections::BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct Bus {
    id: String,
    name: String,
    gain: f32,
    muted: bool,
    /// Where the bus's tracks go; None = each track uses its own output.
    route: Option<Route>,
    /// Made with "New bus" (can be deleted).
    custom: bool,
}

impl Default for Bus {
    fn default() -> Self {
        Bus { id: String::new(), name: String::new(), gain: 1.0, muted: false, route: None, custom: false }
    }
}

/// The built-in buses and the track groups (songfile::stem_category) that go to each.
const DEFAULT_BUSES: [(&str, &str, &[u8]); 8] = [
    ("cues", "Click & guide", &[0]),
    ("drums", "Drums & percussion", &[1, 2]),
    ("bass", "Bass", &[3]),
    ("guitars", "Guitars", &[4]),
    ("keys", "Keys", &[5]),
    ("other", "Other instruments", &[6]),
    ("vocals", "Vocals", &[7]),
    ("fx", "FX", &[8]),
];

impl Settings {
    /// Makes sure every built-in bus exists, carrying over 0.5.0's per-group settings.
    fn ensure_buses(&mut self) {
        for (i, (id, name, cats)) in DEFAULT_BUSES.iter().enumerate() {
            // Settings from 0.5.0 had one entry per group: copy them over.
            if let (Some(old), Some(b)) = (self.buses.get(cats[0] as usize).cloned(), self.bus_list.iter_mut().find(|b| b.id == *id)) {
                b.gain = old.gain;
                b.muted = old.muted;
                b.route = old.route;
            }
            if !self.bus_list.iter().any(|b| b.id == *id) {
                let old = self.buses.get(cats[0] as usize).cloned().unwrap_or_default();
                let b = Bus { id: id.to_string(), name: name.to_string(), gain: old.gain, muted: old.muted, route: old.route, custom: false };
                let at = self.bus_list.iter().take_while(|x| !x.custom && DEFAULT_BUSES.iter().position(|d| d.0 == x.id).map_or(false, |p| p < i)).count();
                self.bus_list.insert(at.min(self.bus_list.len()), b);
            }
        }
        self.buses.clear();
    }
    /// The bus a track belongs to: its hand-picked bus, else its group's built-in bus.
    fn bus_id_for(&self, track: &str) -> String {
        if let Some(id) = self.bus_of_track.get(&track.to_ascii_lowercase()) {
            if self.bus_list.iter().any(|b| &b.id == id) {
                return id.clone();
            }
        }
        let c = songfile::stem_category(track);
        DEFAULT_BUSES.iter().find(|d| d.2.contains(&c)).map_or("other", |d| d.0).to_string()
    }
    fn bus_for(&self, track: &str) -> Bus {
        let id = self.bus_id_for(track);
        self.bus_list.iter().find(|b| b.id == id).cloned().unwrap_or_default()
    }
    fn bus_mut(&mut self, id: &str) -> Option<&mut Bus> {
        self.bus_list.iter_mut().find(|b| b.id == id)
    }
}

/// What the engine plays for a stem once its bus is applied: (gain, muted, route).
fn effective(settings: &Settings, st: &songfile::StemFile) -> (f32, bool, Route) {
    let b = settings.bus_for(&st.name);
    (st.gain * b.gain, st.muted || b.muted, b.route.unwrap_or(st.route))
}

/// Sends every stem's bus-applied level, mute and output to the engine.
fn push_mix(core: &Core, settings: &Settings, file: &SongFile) {
    for (i, st) in file.stems.iter().enumerate() {
        let (g, m, r) = effective(settings, st);
        core.send(Command::SetStemGain(i, g));
        core.send(Command::SetStemMute(i, m));
        core.send(Command::SetStemRoute(i, r));
    }
}

impl Default for Settings {
    fn default() -> Self {
        let mut s = Settings {
            output: None,
            midi: None,
            setlist: vec![],
            current: None,
            quantize: Quantize::Bar,
            count_in: false,
            click_gain: 0.8,
            click_route: None,
            version: 1,
            pads_folder: None,
            pad_level: 0.6,
            pad_route: None,
            free_click_bpm: 120.0,
            free_click_bpb: 4,
            buses: vec![],
            bus_list: vec![],
            bus_of_track: Default::default(),
        };
        s.ensure_buses();
        s
    }
}

struct Loaded {
    folder: PathBuf,
    file: SongFile,
    song: Song,
    arr: Arrangement,
    stems: Vec<StemAudio>,
    sample_rate: u32,
    length: u64,
    /// Waveform overview per stem: one byte (0-255) per 10 ms, base64.
    peaks: Vec<String>,
}

const PEAKS_PER_SEC: u32 = 100;

fn peaks(a: &StemAudio, sr: u32) -> String {
    use base64::Engine as _;
    let hop = (sr / PEAKS_PER_SEC).max(1) as u64;
    let n = a.frames().div_ceil(hop);
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        let mut m = 0.0f32;
        for f in i * hop..((i + 1) * hop).min(a.frames()) {
            let (l, r) = a.frame(f);
            m = m.max(l.abs()).max(r.abs());
        }
        out.push((m.min(1.0) * 255.0).round() as u8);
    }
    base64::engine::general_purpose::STANDARD.encode(out)
}

/// Shared with the MIDI and hold-timer threads.
struct Core {
    output: Mutex<Option<Output>>,
    status: Arc<Mutex<Option<Snapshot>>>,
    /// Counts handoffs to the next song done by the audio thread.
    segues: Arc<AtomicU64>,
    /// The free click is running (the audio thread stops it when a song starts).
    metro_on: Arc<AtomicBool>,
    map: Mutex<MidiMap>,
    config_dir: PathBuf,
    data_dir: PathBuf,
    start: Instant,
}

impl Core {
    fn send(&self, c: Command) -> bool {
        self.output.lock().unwrap().as_mut().map_or(false, |o| o.send(ToAudio::Cmd(c)))
    }
    fn snapshot(&self) -> Option<Snapshot> {
        *self.status.lock().unwrap()
    }
    fn playing(&self) -> bool {
        self.snapshot().map_or(false, |s| s.playing)
    }
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
    fn save_map(&self) {
        let map = self.map.lock().unwrap().clone();
        write_json(&self.config_dir.join("midi.json"), &map);
    }
}

struct AppState {
    core: Arc<Core>,
    song: Mutex<Option<Loaded>>,
    settings: Mutex<Settings>,
    midi: Mutex<Option<midi_io::MidiPort>>,
    pad_on: Mutex<bool>,
    /// Pitch class of the pad loaded on the audio thread.
    pad_key: Mutex<Option<u8>>,
    /// The next song in the set, loaded ahead so switching (or a segue) is instant.
    next: Mutex<NextSong>,
    /// Handoffs already applied to `song` (see Core::segues).
    seen_segues: AtomicU64,
    /// The pad was brought in automatically for this song's ending.
    pad_auto: AtomicBool,
    /// A pad key picked on the Live page (None = the song's key). Back to the song's key
    /// when the song changes.
    pad_choice: Mutex<Option<u8>>,
}

#[derive(Default)]
struct NextSong {
    loaded: Option<Loaded>,
    /// Being loaded in the background.
    loading: Option<PathBuf>,
    /// Couldn't be loaded; not retried until the set changes.
    failed: Option<PathBuf>,
    /// What was last queued on the audio thread (current song, next song, transition).
    queued: Option<String>,
}

/// The song's key as played (recorded key + key change), as a pitch class.
fn song_key_pc(f: &SongFile) -> Option<u8> {
    pads::pitch_class(&f.key).map(|pc| ((pc as i32 + f.transpose).rem_euclid(12)) as u8)
}

/// Brings the pad in line with the switch and the song's key: fades in/out, or
/// crossfades to the pad for a new key (decoded on a worker thread).
fn pad_update(state: &AppState) -> Result<(), String> {
    let on = *state.pad_on.lock().unwrap();
    let s = state.settings.lock().unwrap().clone();
    if let Some(o) = state.core.output.lock().unwrap().as_mut() {
        o.send(ToAudio::PadLevel(s.pad_level));
        o.send(ToAudio::PadRoute(s.pad_route.unwrap_or(Route::Pair(0))));
        o.send(ToAudio::PadOn(on));
    }
    if !on {
        return Ok(());
    }
    let folder = s.pads_folder.clone().ok_or("Choose your pads folder in Setup → Pads first.")?;
    let chosen = *state.pad_choice.lock().unwrap();
    let song_pc = state.song.lock().unwrap().as_ref().and_then(|l| song_key_pc(&l.file));
    let pc = chosen.or(song_pc).ok_or("This song has no key. Pick a pad key, or set the song's key in Song → Song details.")?;
    if *state.pad_key.lock().unwrap() == Some(pc) {
        return Ok(());
    }
    let file = pads::scan(&folder)[pc as usize].clone().ok_or(format!("There's no {} pad in your pads folder.", pads::NAMES[pc as usize]))?;
    let sr = state.core.output.lock().unwrap().as_ref().map(|o| o.info.sample_rate).ok_or("No audio output is open.")?;
    *state.pad_key.lock().unwrap() = Some(pc);
    let core = state.core.clone();
    std::thread::spawn(move || {
        if let Ok(v) = decode::decode(&file, sr).and_then(|a| pads::make_voice(&a, sr)) {
            if let Some(o) = core.output.lock().unwrap().as_mut() {
                o.send(ToAudio::Pad(Box::new(v)));
            }
        }
    });
    Ok(())
}

impl AppState {
    fn save_settings(&self) {
        let s = self.settings.lock().unwrap().clone();
        write_json(&self.core.config_dir.join("settings.json"), &s);
    }
}

fn write_json<T: Serialize>(path: &Path, v: &T) {
    if let Ok(text) = serde_json::to_string_pretty(v) {
        let _ = std::fs::write(path, text);
    }
}

fn read_json<T: for<'a> Deserialize<'a> + Default>(path: &Path) -> T {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/* ---------------- MIDI → engine ---------------- */

/// Fader position (0-1) to gain, with an audio taper. Top of the fader is about +2 dB.
fn fader(v: f32) -> f32 {
    (v * v * 1.25).min(2.0)
}

fn dispatch(core: &Core, app: &AppHandle, ev: Event) {
    match ev {
        Event::Learned(_) => {
            core.save_map();
            let _ = app.emit("midi-learned", ());
        }
        Event::Level(Action::StemLevel(i), v) => {
            let g = fader(v);
            core.send(Command::SetStemGain(i, g));
            let _ = app.emit("stem-level", (i, g));
        }
        Event::Level(Action::ClickLevel, v) => {
            let g = fader(v);
            core.send(Command::SetClickGain(g));
            let _ = app.emit("click-level", g);
        }
        Event::Level(..) => {}
        Event::Fire(a) => match a {
            Action::PlayStop => {
                core.send(if core.playing() { Command::Stop } else { Command::Play });
            }
            Action::Panic => {
                core.send(Command::Stop);
            }
            Action::Next => {
                core.send(Command::Next);
            }
            Action::SkipAhead => {
                core.send(Command::Next);
                core.send(Command::Next);
            }
            Action::Repeat => {
                core.send(Command::RepeatCurrent);
            }
            Action::ToggleVamp => {
                core.send(Command::ToggleVamp);
            }
            Action::NextSong => {
                let _ = app.emit("pedal", "next-song");
            }
            Action::PadOnly => {
                let _ = app.emit("pedal", "pad-only");
            }
            Action::ToggleView => {
                let _ = app.emit("pedal", "toggle-view");
            }
            Action::FadeOut => {
                fade_out(&app.state::<AppState>(), 2.0);
            }
            Action::FreeClick => {
                let st = app.state::<AppState>();
                let (bpm, bpb) = {
                    let s = st.settings.lock().unwrap();
                    (s.free_click_bpm, s.free_click_bpb)
                };
                let on = !st.core.metro_on.load(Ordering::Acquire);
                if free_click(st.clone(), on, bpm, bpb).is_ok() {
                    let _ = app.emit("pedal", "refresh");
                }
            }
            Action::StemLevel(_) | Action::ClickLevel => {}
        },
    }
}

fn start_midi(core: &Arc<Core>, app: &AppHandle, name: &str) -> Result<midi_io::MidiPort, String> {
    let core = core.clone();
    let app = app.clone();
    midi_io::open(name, move |bytes| {
        let ev = core.map.lock().unwrap().handle(bytes, core.now_ms());
        if let Some(ev) = ev {
            dispatch(&core, &app, ev);
        }
    })
}

fn open_output(core: &Core, app: &AppHandle, name: Option<String>) -> Result<OpenOutput, String> {
    *core.output.lock().unwrap() = None; // close the old one first
    *core.status.lock().unwrap() = None;
    let h = app.clone();
    let out = audio::open(name, core.status.clone(), core.segues.clone(), core.metro_on.clone(), move |e| {
        let _ = h.emit("audio-error", e);
    })?;
    let info = out.info.clone();
    *core.output.lock().unwrap() = Some(out);
    Ok(info)
}

/* ---------------- songs ---------------- */

fn load(folder: &Path, sample_rate: u32, channels: usize) -> Result<Loaded, String> {
    let file = SongFile::scan(folder, channels)?;
    // Key and tempo changes are rendered here, one thread per stem. Click and guide
    // are only time-stretched, never pitch-shifted.
    let length_factor = file.stretch();
    let results: Vec<Result<StemAudio, String>> = std::thread::scope(|s| {
        let hs: Vec<_> = file
            .stems
            .iter()
            .map(|st| {
                let p = folder.join(&st.file);
                let semis = if songfile::is_click(&st.name) || songfile::is_guide(&st.name) { 0.0 } else { file.transpose as f32 };
                s.spawn(move || decode::decode(&p, sample_rate).map(|a| stretch::apply(&a, sample_rate, length_factor, semis)))
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("A stem failed to load.".into()))).collect()
    });
    let stems = results.into_iter().collect::<Result<Vec<_>, _>>()?;
    let length = stems.iter().map(|s| s.frames()).max().unwrap_or(0);
    // Songs set up automatically get their grid lined up with the click you hear.
    let mut file = file;
    if file.grid_version < songfile::GRID_VERSION && matches!(file.markers_from.as_deref(), Some("ableton") | Some("guide")) {
        let ableton = (file.markers_from.as_deref() == Some("ableton")).then(|| als::find(folder).and_then(|p| als::read(&p).ok())).flatten();
        let had_map = !file.beat_times.is_empty();
        if let Some(i) = file.stems.iter().position(|s| songfile::is_click(&s.name)) {
            let click = guide::to_mono(&stems[i]);
            guide::snap_grid_to_click(&mut file, &click, sample_rate);
            // Songs whose tempo changes: the grid follows every beat of the click.
            if file.beat_times.is_empty() {
                if let Some(g) = guide::beat_grid_from_click(&click, sample_rate, file.play_bpm()) {
                    let k = file.stretch();
                    file.set_beat_grid(g.iter().map(|t| t / k).collect());
                }
            }
        } else if file.beat_times.is_empty() {
            // No click: the Ableton set's tempo automation, if it has any.
            if let Some(info) = &ableton {
                let len = length as f64 / sample_rate as f64 / file.stretch();
                if let Some(g) = info.beat_grid(file.bpm, file.offset_ms / 1000.0, len) {
                    file.set_beat_grid(g);
                }
            }
        }
        // A new tempo map: put the Ableton markers back where they are in the audio.
        if !had_map && !file.beat_times.is_empty() {
            if let Some(info) = &ableton {
                file.place_locators(info);
            }
        }
        // Sections start a bar after the guide's call: move each marker onto the
        // section start the guide gives beside it, and drop numbering that isn't needed.
        if let Some(g) = file.stems.iter().position(|s| songfile::is_guide(&s.name)) {
            let starts = guide::call_starts_on(&guide::to_mono(&stems[g]), sample_rate, &guide::Grid::of(&file));
            guide::align_markers_to_calls(&mut file.markers, &starts);
        }
        for (old, new) in guide::tidy_names(&mut file.markers) {
            for b in file.arrangement.iter_mut().filter(|b| b.section == old) {
                b.section = new.clone();
            }
        }
        file.grid_version = songfile::GRID_VERSION;
        let _ = file.save(folder);
    }
    let (mut song, arr) = file.build(sample_rate, length)?;
    cut_guide(&mut song, &file, &stems, sample_rate);
    let peaks = stems.iter().map(|s| peaks(s, sample_rate)).collect();
    Ok(Loaded { folder: folder.to_path_buf(), file, song, arr, stems, sample_rate, length, peaks })
}

/// Cuts the guide by section, so it calls whatever section actually comes next.
fn cut_guide(song: &mut Song, file: &SongFile, stems: &[StemAudio], sample_rate: u32) {
    if let Some(g) = file.stems.iter().position(|s| songfile::is_guide(&s.name)) {
        let c = guide::calls_on(&guide::to_mono(&stems[g]), sample_rate, &guide::Grid::of(file));
        song.calls = guide::call_regions(song, &c, sample_rate);
        song.guide_stem = Some(g);
    }
}

/// The commands that set an engine's mixer and options from the settings.
fn mix_commands(settings: &Settings, l: &Loaded, ch: usize) -> Vec<Command> {
    let mut c = vec![
        Command::SetQuantize(settings.quantize),
        Command::SetClick(l.file.click),
        Command::SetClickGain(settings.click_gain),
        Command::SetClickRoute(settings.click_route.unwrap_or(songfile::default_route("click", ch))),
    ];
    for (i, st) in l.file.stems.iter().enumerate() {
        let (g, m, r) = effective(settings, st);
        c.extend([Command::SetStemGain(i, g), Command::SetStemMute(i, m), Command::SetStemRoute(i, r)]);
    }
    c
}

fn make_engine(settings: &Settings, l: &Loaded, ch: usize) -> Result<Engine, String> {
    let mut e = Engine::new(l.song.clone(), l.arr.clone(), l.stems.clone(), ch)?;
    e.handle(Command::SetCountIn(settings.count_in));
    for c in mix_commands(settings, l, ch) {
        e.handle(c);
    }
    Ok(e)
}

/// Keeps the next song loaded and queued on the audio thread, following the set and
/// the current song's transition. Called on every status poll; cheap when nothing changed.
fn sync_next(app: &AppHandle, state: &AppState) {
    let want = {
        let s = state.settings.lock().unwrap();
        s.current.and_then(|c| s.setlist.get(c + 1)).cloned()
    };
    let cur = {
        let g = state.song.lock().unwrap();
        g.as_ref().map(|l| (l.folder.clone(), l.file.transition, end_bar(l), l.sample_rate, l.file.stems.iter().enumerate().filter(|(_, s)| songfile::is_click(&s.name) || songfile::is_guide(&s.name)).map(|(i, _)| i).collect::<Vec<_>>()))
    };
    let Some((cur_folder, tr, bar, sr, cues)) = cur else { return };
    let want = want.filter(|w| *w != cur_folder);
    // Load it in the background if it isn't loaded (or is loaded for another output rate).
    let start_load = {
        let mut n = state.next.lock().unwrap();
        let have = n.loaded.as_ref().filter(|l| l.sample_rate == sr).map(|l| l.folder.clone());
        if have != want {
            n.loaded = None;
            n.queued = None; // forces an Unqueue below
        }
        if want.is_some() && n.loaded.is_none() && n.loading.is_none() && n.failed != want {
            n.loading = want.clone();
            want.clone()
        } else {
            None
        }
    };
    if let Some(folder) = start_load {
        let ch = state.core.output.lock().unwrap().as_ref().map_or(2, |o| o.info.channels);
        let app = app.clone();
        std::thread::spawn(move || {
            let r = load(&folder, sr, ch);
            let st = app.state::<AppState>();
            let mut n = st.next.lock().unwrap();
            if n.loading.as_ref() == Some(&folder) {
                n.loading = None;
                match r {
                    Ok(l) => n.loaded = Some(l),
                    Err(_) => n.failed = Some(folder),
                }
            }
        });
    }
    // Queue it for the handoff, or take it off the queue.
    let settings = state.settings.lock().unwrap().clone();
    let mut n = state.next.lock().unwrap();
    let sig = match (&n.loaded, tr.mode) {
        (Some(l), m) if m != songfile::TransitionMode::Stop => format!("{}|{}|{:?}|{}|{}", cur_folder.display(), l.folder.display(), m, tr.bars, bar),
        _ => "none".to_string(),
    };
    if n.queued.as_deref() == Some(sig.as_str()) {
        return;
    }
    let mut out = state.core.output.lock().unwrap();
    let Some(o) = out.as_mut() else { return };
    let sent = match (&n.loaded, sig != "none") {
        (Some(l), true) => match make_engine(&settings, l, o.info.channels) {
            Ok(mut e) => {
                e.handle(Command::SetCountIn(false)); // it follows straight on
                let (overlap, fade) = match tr.mode {
                    songfile::TransitionMode::Crossfade => {
                        let f = (tr.bars.max(0.0) * bar).round() as u64;
                        (f, f)
                    }
                    _ => (0, 0),
                };
                let h = audio::Handoff { overlap, fade, cues: [cues.first().copied(), cues.get(1).copied()] };
                o.send(ToAudio::Queue(Box::new(e), h))
            }
            Err(_) => false,
        },
        _ => o.send(ToAudio::Unqueue),
    };
    if sent {
        n.queued = Some(sig);
    }
}

/// Length in frames of the song's last bar (where a crossfade happens), following
/// any tempo change.
fn end_bar(l: &Loaded) -> f64 {
    let end = l.arr.items.last().map_or(l.length, |it| l.song.sections[it.section].end) as f64;
    let e = l.song.beat_pos(end);
    let bpb = l.song.beats_per_bar.max(1) as f64;
    (l.song.beat_frame(e) - l.song.beat_frame(e - bpb)).max(1.0)
}

/// After the audio thread handed over to the next song: make it the current song.
fn take_segue(state: &AppState) -> bool {
    let done = state.core.segues.load(Ordering::Acquire);
    if done == state.seen_segues.swap(done, Ordering::AcqRel) {
        return false;
    }
    let next = {
        let mut n = state.next.lock().unwrap();
        n.queued = None;
        n.loaded.take()
    };
    let Some(l) = next else { return false };
    let settings = state.settings.lock().unwrap().clone();
    let ch = state.core.output.lock().unwrap().as_ref().map_or(2, |o| o.info.channels);
    // The queued engine was set up when it was queued; bring its mixer up to date.
    for c in mix_commands(&settings, &l, ch) {
        state.core.send(c);
    }
    let idx = settings.setlist.iter().position(|p| *p == l.folder);
    *state.song.lock().unwrap() = Some(l);
    if idx.is_some() {
        state.settings.lock().unwrap().current = idx;
        state.save_settings();
    }
    state.pad_auto.store(false, Ordering::Relaxed);
    *state.pad_choice.lock().unwrap() = None;
    let _ = pad_update(state); // the pad follows the new song's key
    true
}

/// "Pad underneath": bring the pad in when the song's last block starts.
fn auto_pad(state: &AppState, snap: &Option<Snapshot>) -> bool {
    let Some(sn) = snap else { return false };
    let (want, last) = {
        let g = state.song.lock().unwrap();
        match g.as_ref() {
            Some(l) => (l.file.transition.pad, sn.item + 1 >= l.arr.items.len()),
            None => (false, false),
        }
    };
    if !(want && sn.playing && last) {
        if !last {
            state.pad_auto.store(false, Ordering::Relaxed);
        }
        return false;
    }
    if state.pad_auto.swap(true, Ordering::Relaxed) || *state.pad_on.lock().unwrap() {
        return false;
    }
    if state.settings.lock().unwrap().pads_folder.is_none() {
        return false;
    }
    *state.pad_on.lock().unwrap() = true;
    if pad_update(state).is_err() {
        *state.pad_on.lock().unwrap() = false;
        let _ = pad_update(state);
        return false;
    }
    true
}

/// Builds a fresh engine for the loaded song and hands it to the audio thread.
fn install(core: &Core, settings: &Settings, l: &Loaded) -> Result<(), String> {
    let mut out = core.output.lock().unwrap();
    let o = out.as_mut().ok_or("No audio output is open. Pick one at the top of the window.")?;
    if o.info.sample_rate != l.sample_rate {
        return Err("The audio output changed sample rate. Reload the song.".into());
    }
    let e = make_engine(settings, l, o.info.channels)?;
    if !o.send(ToAudio::Swap(Box::new(e))) {
        return Err("The audio thread is busy. Try again.".into());
    }
    o.collect_garbage();
    Ok(())
}

/* ---------------- views for the UI ---------------- */

#[derive(Serialize)]
struct SectionView {
    name: String,
    bar: i64,
    bars: f64,
    start_sec: f64,
    secs: f64,
}

#[derive(Serialize)]
struct BlockView {
    section: String,
    section_index: usize,
    muted: Vec<String>,
    bars: f64,
    /// Tracks playing only part of this block (beats from the block's start).
    trims: Vec<TrimSpec>,
    /// The block fades out early: (start, length) in bars.
    fade_out: Option<(f64, f64)>,
}

#[derive(Serialize)]
struct StemView {
    name: String,
    route: Route,
    gain: f32,
    muted: bool,
}

#[derive(Serialize)]
struct SongView {
    folder: String,
    name: String,
    bpm: f64,
    beats_per_bar: u32,
    offset_ms: f64,
    key: String,
    click: bool,
    seconds: f64,
    sample_rate: u32,
    markers: Vec<Marker>,
    sections: Vec<SectionView>,
    stems: Vec<StemView>,
    blocks: Vec<BlockView>,
    has_click: bool,
    has_guide: bool,
    /// Markers haven't been set up yet (by the guide or by hand).
    needs_setup: bool,
    /// "ableton", "guide", "manual" or None.
    markers_from: Option<String>,
    /// Key change in semitones; `bpm` is the recorded tempo, `play_bpm` what plays.
    transpose: i32,
    play_bpm: f64,
    transition: Transition,
    ending: Option<Ending>,
    /// Songs whose tempo changes: the time (seconds, as it plays) of every bar line.
    /// Empty for a steady tempo.
    downbeats: Vec<f64>,
    /// (bar, BPM as it plays) where the tempo changes. Empty for a steady tempo.
    tempo_changes: Vec<(u32, f64)>,
}

impl SongView {
    fn from(l: &Loaded) -> Self {
        let bar = l.song.frames_per_bar();
        let bpb = l.song.beats_per_bar.max(1) as f64;
        let sr = l.sample_rate as f64;
        // Songs whose tempo changes: every downbeat, so the ruler follows the tempo.
        let downbeats: Vec<f64> = if l.song.beat_frames.is_empty() {
            vec![]
        } else {
            let last = l.song.beat_pos(l.length as f64);
            (0..)
                .map(|k| k as f64 * bpb)
                .take_while(|&b| b <= last)
                .map(|b| (l.song.beat_frame(b) / sr * 1000.0).round() / 1000.0)
                .collect()
        };
        SongView {
            folder: l.folder.display().to_string(),
            name: l.file.name.clone(),
            bpm: l.file.bpm,
            beats_per_bar: l.file.beats_per_bar,
            offset_ms: l.file.offset_ms,
            key: l.file.key.clone(),
            click: l.file.click,
            seconds: l.length as f64 / l.sample_rate as f64,
            sample_rate: l.sample_rate,
            markers: l.file.markers.clone(),
            sections: l
                .song
                .sections
                .iter()
                .map(|s| SectionView {
                    name: s.name.clone(),
                    bar: (l.song.beat_pos(s.start as f64) / bpb).round() as i64 + 1,
                    bars: (l.song.beat_pos(s.end as f64) - l.song.beat_pos(s.start as f64)) / bpb,
                    start_sec: s.start as f64 / l.sample_rate as f64,
                    secs: s.len() as f64 / l.sample_rate as f64,
                })
                .collect(),
            stems: l.file.stems.iter().map(|s| StemView { name: s.name.clone(), route: s.route, gain: s.gain, muted: s.muted }).collect(),
            blocks: l
                .arr
                .items
                .iter()
                .map(|it| BlockView {
                    section: l.song.sections[it.section].name.clone(),
                    section_index: it.section,
                    muted: it.muted_stems.iter().map(|&i| l.file.stems[i].name.clone()).collect(),
                    bars: l.song.sections[it.section].len() as f64 / bar,
                    trims: it
                        .trims
                        .iter()
                        .map(|t| {
                            let b = |f: u64| (f as f64 / l.song.frames_per_beat() * 1000.0).round() / 1000.0;
                            TrimSpec { track: l.file.stems[t.stem].name.clone(), from: b(t.start), to: b(t.end), fade_in: b(t.fade_in), fade_out: b(t.fade_out) }
                        })
                        .collect(),
                    fade_out: it.fade_out.map(|(s, n)| (s as f64 / bar, n as f64 / bar)),
                })
                .collect(),
            has_click: l.file.stems.iter().any(|s| songfile::is_click(&s.name)),
            has_guide: l.file.stems.iter().any(|s| songfile::is_guide(&s.name)),
            needs_setup: l.file.markers_from.is_none(),
            markers_from: l.file.markers_from.clone(),
            transpose: l.file.transpose,
            play_bpm: l.file.play_bpm(),
            transition: l.file.transition,
            ending: l.file.ending,
            downbeats,
            tempo_changes: l.file.tempo_changes().into_iter().map(|(b, t)| (b, (t / l.file.stretch() * 10.0).round() / 10.0)).collect(),
        }
    }
}

#[derive(Serialize)]
struct SetItem {
    name: String,
    path: String,
    found: bool,
    transition: songfile::TransitionMode,
}

#[derive(Serialize)]
struct AppView {
    outputs: Vec<OutputInfo>,
    output: Option<OpenOutput>,
    midi_inputs: Vec<String>,
    midi: Option<String>,
    setlist: Vec<SetItem>,
    current: Option<usize>,
    song: Option<SongView>,
    quantize: Quantize,
    count_in: bool,
    click_gain: f32,
    click_route: Route,
    bindings: Vec<Binding>,
    learning: bool,
    pads: PadsView,
    /// Buses for the groups in the current song, in track-list order.
    buses: Vec<BusView>,
    /// The next song in the set is loaded and ready.
    next_ready: bool,
    free_click: FreeClickView,
}

#[derive(Serialize)]
struct FreeClickView {
    on: bool,
    bpm: f64,
    beats_per_bar: u32,
}

#[derive(Serialize)]
struct BusView {
    id: String,
    custom: bool,
    name: String,
    gain: f32,
    muted: bool,
    route: Option<Route>,
    /// Indices of this song's stems in the group.
    stems: Vec<usize>,
}

#[derive(Serialize)]
struct PadsView {
    folder: Option<String>,
    /// Keys with a pad file, e.g. ["C", "D", "A"].
    keys: Vec<String>,
    on: bool,
    level: f32,
    route: Route,
    /// The key the pad plays for the current song.
    song_key: Option<String>,
    /// A key picked by hand (None = the song's key).
    chosen: Option<String>,
}

fn view(state: &AppState) -> AppView {
    let s = state.settings.lock().unwrap().clone();
    let output = state.core.output.lock().unwrap().as_ref().map(|o| o.info.clone());
    let ch = output.as_ref().map_or(2, |o| o.channels);
    let (bindings, learning) = {
        let map = state.core.map.lock().unwrap();
        (map.bindings.clone(), map.is_learning())
    };
    // Take each lock on its own line: a guard inside the struct literal below would
    // live until the end of the whole expression, and locking the same mutex twice
    // there deadlocks (that froze 0.4.0 on launch).
    let (song, song_key, stem_names) = {
        let g = state.song.lock().unwrap();
        let names: Vec<String> = g.as_ref().map(|l| l.file.stems.iter().map(|st| st.name.clone()).collect()).unwrap_or_default();
        (g.as_ref().map(SongView::from), g.as_ref().and_then(|l| song_key_pc(&l.file)).map(|pc| pads::NAMES[pc as usize].to_string()), names)
    };
    // Buses in their own order, each with this song's tracks in track-list order.
    // Built-in buses with no tracks in this song are hidden; your own always show.
    let mut buses: Vec<BusView> = s
        .bus_list
        .iter()
        .map(|b| BusView { id: b.id.clone(), custom: b.custom, name: b.name.clone(), gain: b.gain, muted: b.muted, route: b.route, stems: vec![] })
        .collect();
    for (i, n) in stem_names.iter().enumerate() {
        let id = s.bus_id_for(n);
        if let Some(b) = buses.iter_mut().find(|b| b.id == id) {
            b.stems.push(i);
        }
    }
    buses.retain(|b| b.custom || !b.stems.is_empty());
    let midi = state.midi.lock().unwrap().as_ref().map(|m| m.name.clone());
    let next_ready = state.next.lock().unwrap().loaded.is_some();
    let pad_choice = *state.pad_choice.lock().unwrap();
    let free_click = FreeClickView { on: state.core.metro_on.load(Ordering::Acquire), bpm: s.free_click_bpm, beats_per_bar: s.free_click_bpb };
    let pad_on = *state.pad_on.lock().unwrap();
    AppView {
        outputs: audio::list_outputs(),
        output,
        midi_inputs: midi_io::inputs(),
        midi,
        setlist: s
            .setlist
            .iter()
            .map(|p| {
                let found = p.is_dir();
                let file = std::fs::read_to_string(SongFile::path(p)).ok().and_then(|t| serde_json::from_str::<SongFile>(&t).ok());
                let transition = file.as_ref().map_or(songfile::TransitionMode::Stop, |f| f.transition.mode);
                let name = file.map(|f| f.name).unwrap_or_else(|| p.file_name().and_then(|n| n.to_str()).unwrap_or("Song").to_string());
                SetItem { name, path: p.display().to_string(), found, transition }
            })
            .collect(),
        current: s.current,
        song,
        next_ready,
        quantize: s.quantize,
        count_in: s.count_in,
        click_gain: s.click_gain,
        click_route: s.click_route.unwrap_or(songfile::default_route("click", ch)),
        bindings,
        learning,
        buses,
        pads: PadsView {
            folder: s.pads_folder.as_ref().map(|p| p.display().to_string()),
            keys: s.pads_folder.as_ref().map(|f| pads::scan(f).iter().enumerate().filter(|(_, p)| p.is_some()).map(|(i, _)| pads::NAMES[i].to_string()).collect()).unwrap_or_default(),
            on: pad_on,
            level: s.pad_level,
            route: s.pad_route.unwrap_or(Route::Pair(0)),
            song_key,
            chosen: pad_choice.map(|pc| pads::NAMES[pc as usize].to_string()),
        },
        free_click,
    }
}

/* ---------------- commands ---------------- */

#[tauri::command]
fn app_state(state: State<'_, AppState>) -> AppView {
    view(&state)
}

#[derive(Serialize)]
struct Status {
    snapshot: Option<Snapshot>,
    learning: bool,
    /// The next song took over (segue or crossfade): reload the view.
    song_changed: bool,
    /// The pad was switched on or off here (pad underneath): reload the view.
    pad_changed: bool,
    /// The free click is running.
    free_click: bool,
}

#[tauri::command]
fn status(app: AppHandle, state: State<'_, AppState>) -> Status {
    if let Some(o) = state.core.output.lock().unwrap().as_mut() {
        o.collect_garbage();
    }
    let song_changed = take_segue(&state);
    let snapshot = state.core.snapshot();
    let pad_changed = auto_pad(&state, &snapshot);
    sync_next(&app, &state);
    let learning = state.core.map.lock().unwrap().is_learning();
    Status { snapshot, learning, song_changed, pad_changed, free_click: state.core.metro_on.load(Ordering::Acquire) }
}

async fn reload_current(state: &AppState) -> Result<(), String> {
    let folder = state.song.lock().unwrap().as_ref().map(|l| l.folder.clone());
    let Some(folder) = folder else { return Ok(()) };
    let (sr, ch) = {
        let out = state.core.output.lock().unwrap();
        let o = out.as_ref().ok_or("No audio output is open.")?;
        (o.info.sample_rate, o.info.channels)
    };
    let loaded = tauri::async_runtime::spawn_blocking(move || load(&folder, sr, ch)).await.map_err(|e| e.to_string())??;
    let settings = state.settings.lock().unwrap().clone();
    install(&state.core, &settings, &loaded)?;
    *state.song.lock().unwrap() = Some(loaded);
    Ok(())
}

#[tauri::command]
async fn choose_output(app: AppHandle, state: State<'_, AppState>, name: Option<String>) -> Result<AppView, String> {
    if state.core.playing() {
        return Err("Stop playback before changing the audio output.".into());
    }
    let info = open_output(&state.core, &app, name)?;
    state.settings.lock().unwrap().output = Some(info.name.clone());
    state.save_settings();
    *state.pad_key.lock().unwrap() = None; // the new output starts without a pad
    reload_current(&state).await?;
    let _ = pad_update(&state);
    Ok(view(&state))
}

#[tauri::command]
fn choose_midi(app: AppHandle, state: State<'_, AppState>, name: Option<String>) -> Result<AppView, String> {
    *state.midi.lock().unwrap() = None;
    if let Some(n) = &name {
        let port = start_midi(&state.core, &app, n)?;
        *state.midi.lock().unwrap() = Some(port);
    }
    state.settings.lock().unwrap().midi = name;
    state.save_settings();
    Ok(view(&state))
}

#[tauri::command]
async fn add_songs(app: AppHandle, state: State<'_, AppState>) -> Result<AppView, String> {
    let picked = app.dialog().file().set_title("Choose song folders (one folder of stems per song)").blocking_pick_folders();
    if let Some(folders) = picked {
        let mut s = state.settings.lock().unwrap();
        for f in folders {
            if let Ok(p) = f.into_path() {
                if !s.setlist.contains(&p) {
                    s.setlist.push(p);
                }
            }
        }
    }
    state.save_settings();
    Ok(view(&state))
}

#[tauri::command]
fn remove_song(state: State<'_, AppState>, index: usize) -> Result<AppView, String> {
    {
        let mut s = state.settings.lock().unwrap();
        if index >= s.setlist.len() {
            return Err("That song isn't in the set.".into());
        }
        if s.current == Some(index) {
            if state.core.playing() {
                return Err("Stop playback before removing the current song.".into());
            }
            s.current = None;
            *state.song.lock().unwrap() = None;
        } else if let Some(c) = s.current.filter(|&c| c > index) {
            s.current = Some(c - 1);
        }
        s.setlist.remove(index);
    }
    state.save_settings();
    Ok(view(&state))
}

#[tauri::command]
fn move_song(state: State<'_, AppState>, index: usize, delta: i32) -> Result<AppView, String> {
    {
        let mut s = state.settings.lock().unwrap();
        let j = index as i64 + delta as i64;
        if index >= s.setlist.len() || j < 0 || j as usize >= s.setlist.len() {
            drop(s);
            return Ok(view(&state));
        }
        let j = j as usize;
        s.setlist.swap(index, j);
        s.current = s.current.map(|c| if c == index { j } else if c == j { index } else { c });
    }
    state.save_settings();
    Ok(view(&state))
}

#[tauri::command]
async fn select_song(state: State<'_, AppState>, index: usize) -> Result<AppView, String> {
    if state.core.playing() {
        return Err("Stop playback to change songs.".into());
    }
    let folder = state.settings.lock().unwrap().setlist.get(index).cloned().ok_or("That song isn't in the set.")?;
    let (sr, ch) = {
        let out = state.core.output.lock().unwrap();
        let o = out.as_ref().ok_or("No audio output is open. Pick one at the top of the window.")?;
        (o.info.sample_rate, o.info.channels)
    };
    // The next song is usually loaded already.
    let ready = {
        let mut n = state.next.lock().unwrap();
        let hit = n.loaded.as_ref().is_some_and(|l| l.folder == folder && l.sample_rate == sr);
        if hit {
            n.queued = None;
            n.loaded.take()
        } else {
            None
        }
    };
    let loaded = match ready {
        Some(l) => l,
        None => tauri::async_runtime::spawn_blocking(move || load(&folder, sr, ch)).await.map_err(|e| e.to_string())??,
    };
    let settings = state.settings.lock().unwrap().clone();
    install(&state.core, &settings, &loaded)?;
    if !SongFile::path(&loaded.folder).exists() {
        let _ = loaded.file.save(&loaded.folder);
    }
    *state.song.lock().unwrap() = Some(loaded);
    state.settings.lock().unwrap().current = Some(index);
    state.save_settings();
    *state.pad_choice.lock().unwrap() = None;
    let _ = pad_update(&state); // pad follows the new song's key
    Ok(view(&state))
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum UiCmd {
    PlayStop,
    Stop,
    Next,
    SkipAhead,
    Repeat,
    Vamp,
    CancelJump,
    Goto { block: usize },
    Seek { block: usize, frame: u64 },
    Quantize { q: Quantize },
    CountIn { on: bool },
    Click { on: bool },
    ClickGain { gain: f32 },
    ClickRoute { route: Route },
    StemGain { stem: usize, gain: f32 },
    StemMute { stem: usize, muted: bool },
    /// Mute without saving (used for solo).
    StemMuteLive { stem: usize, muted: bool },
    BusGain { bus: String, gain: f32 },
    BusMute { bus: String, muted: bool },
    /// None: the group's stems use their own outputs.
    BusRoute { bus: String, route: Option<Route> },
    StemRoute { stem: usize, route: Route },
    /// Fade the song out over this many bars and stop.
    FadeOut { bars: f64 },
}

/// Fades the playing song out over `bars` of its own bars.
fn fade_out(state: &AppState, bars: f64) {
    let frames = state.song.lock().unwrap().as_ref().map(|l| (bars.max(0.25) * l.song.frames_per_bar()).round() as u64);
    if let Some(f) = frames {
        state.core.send(Command::FadeOut(f));
    }
}

#[tauri::command]
fn cmd(state: State<'_, AppState>, c: UiCmd) -> Result<(), String> {
    let core = &state.core;
    let mut save_song = false;
    let mut save_settings = false;
    {
        let mut song = state.song.lock().unwrap();
        let mut s = state.settings.lock().unwrap();
        match c {
            UiCmd::PlayStop => {
                core.send(if core.playing() { Command::Stop } else { Command::Play });
            }
            UiCmd::Stop => {
                core.send(Command::Stop);
            }
            UiCmd::Next => {
                core.send(Command::Next);
            }
            UiCmd::SkipAhead => {
                core.send(Command::Next);
                core.send(Command::Next);
            }
            UiCmd::Repeat => {
                core.send(Command::RepeatCurrent);
            }
            UiCmd::Vamp => {
                core.send(Command::ToggleVamp);
            }
            UiCmd::CancelJump => {
                core.send(Command::CancelJump);
            }
            UiCmd::FadeOut { bars } => {
                if let Some(l) = song.as_ref() {
                    core.send(Command::FadeOut((bars.max(0.25) * l.song.frames_per_bar()).round() as u64));
                }
            }
            UiCmd::Goto { block } => {
                core.send(Command::Goto(block));
            }
            UiCmd::Seek { block, frame } => {
                core.send(Command::Seek(block, frame));
            }
            UiCmd::Quantize { q } => {
                s.quantize = q;
                save_settings = true;
                core.send(Command::SetQuantize(q));
            }
            UiCmd::CountIn { on } => {
                s.count_in = on;
                save_settings = true;
                core.send(Command::SetCountIn(on));
            }
            UiCmd::ClickGain { gain } => {
                s.click_gain = gain;
                save_settings = true;
                core.send(Command::SetClickGain(gain));
            }
            UiCmd::ClickRoute { route } => {
                s.click_route = Some(route);
                save_settings = true;
                core.send(Command::SetClickRoute(route));
            }
            UiCmd::Click { on } => {
                if let Some(l) = song.as_mut() {
                    l.file.click = on;
                    save_song = true;
                }
                core.send(Command::SetClick(on));
            }
            UiCmd::StemGain { stem, gain } => {
                if let Some(st) = song.as_mut().and_then(|l| l.file.stems.get_mut(stem)) {
                    st.gain = gain;
                    save_song = true;
                }
                if let Some(st) = song.as_ref().and_then(|l| l.file.stems.get(stem)) {
                    core.send(Command::SetStemGain(stem, effective(&s, st).0));
                }
            }
            UiCmd::StemMute { stem, muted } => {
                if let Some(st) = song.as_mut().and_then(|l| l.file.stems.get_mut(stem)) {
                    st.muted = muted;
                    save_song = true;
                }
                if let Some(st) = song.as_ref().and_then(|l| l.file.stems.get(stem)) {
                    core.send(Command::SetStemMute(stem, effective(&s, st).1));
                }
            }
            UiCmd::StemMuteLive { stem, muted } => {
                let bus_muted = song.as_ref().and_then(|l| l.file.stems.get(stem)).map_or(false, |st| effective(&s, st).1);
                core.send(Command::SetStemMute(stem, muted || bus_muted));
            }
            UiCmd::BusGain { bus, gain } => {
                if let Some(b) = s.bus_mut(&bus) {
                    b.gain = gain.clamp(0.0, 2.0);
                }
                save_settings = true;
                if let Some(l) = song.as_ref() {
                    push_mix(core, &s, &l.file);
                }
            }
            UiCmd::BusMute { bus, muted } => {
                if let Some(b) = s.bus_mut(&bus) {
                    b.muted = muted;
                }
                save_settings = true;
                if let Some(l) = song.as_ref() {
                    push_mix(core, &s, &l.file);
                }
            }
            UiCmd::BusRoute { bus, route } => {
                if let Some(b) = s.bus_mut(&bus) {
                    b.route = route;
                }
                save_settings = true;
                if let Some(l) = song.as_ref() {
                    push_mix(core, &s, &l.file);
                }
            }
            UiCmd::StemRoute { stem, route } => {
                if let Some(st) = song.as_mut().and_then(|l| l.file.stems.get_mut(stem)) {
                    st.route = route;
                    save_song = true;
                }
                if let Some(st) = song.as_ref().and_then(|l| l.file.stems.get(stem)) {
                    core.send(Command::SetStemRoute(stem, effective(&s, st).2));
                }
            }
        }
        if save_song {
            if let Some(l) = song.as_mut() {
                // Keep the in-memory song in step so a rebuild keeps the mix.
                for (i, st) in l.file.stems.iter().enumerate() {
                    if let Some(m) = l.song.stems.get_mut(i) {
                        m.gain = st.gain;
                        m.muted = st.muted;
                        m.route = st.route;
                    }
                }
                l.file.save(&l.folder)?;
            }
        }
    }
    if save_settings {
        state.save_settings();
    }
    Ok(())
}

#[derive(Deserialize)]
struct SongMeta {
    name: String,
    bpm: f64,
    beats_per_bar: u32,
    offset_ms: f64,
    key: String,
    markers: Vec<Marker>,
}

fn edit_song(state: &AppState, f: impl FnOnce(&mut SongFile)) -> Result<AppView, String> {
    if state.core.playing() {
        return Err("Stop playback before editing the song.".into());
    }
    {
        let mut song = state.song.lock().unwrap();
        let l = song.as_mut().ok_or("Load a song first.")?;
        let mut file = l.file.clone();
        f(&mut file);
        let (mut s, a) = file.build(l.sample_rate, l.length)?;
        cut_guide(&mut s, &file, &l.stems, l.sample_rate);
        l.file = file;
        l.song = s;
        l.arr = a;
        let settings = state.settings.lock().unwrap().clone();
        install(&state.core, &settings, l)?;
        l.file.save(&l.folder)?;
    }
    Ok(view(state))
}

#[tauri::command]
fn update_song(state: State<'_, AppState>, meta: SongMeta) -> Result<AppView, String> {
    edit_song(&state, |f| {
        f.markers_from = Some("manual".into());
        if !meta.name.trim().is_empty() {
            f.name = meta.name.trim().to_string();
        }
        let bpm = meta.bpm.clamp(30.0, 300.0);
        let offset = meta.offset_ms.max(0.0);
        if (bpm - f.bpm).abs() > 1e-9 || meta.beats_per_bar != f.beats_per_bar {
            f.beat_times.clear(); // a new tempo by hand: one steady tempo
        } else if !f.beat_times.is_empty() {
            // moving the downbeat moves the whole tempo map
            let d = (offset - f.offset_ms) / 1000.0;
            f.beat_times.iter_mut().for_each(|t| *t += d);
        }
        f.bpm = bpm;
        f.beats_per_bar = meta.beats_per_bar.clamp(1, 12);
        f.offset_ms = offset;
        f.key = meta.key.trim().to_string();
        let mut m = meta.markers;
        m.retain(|x| !x.name.trim().is_empty());
        m.sort_by_key(|x| x.bar);
        f.markers = m;
    })
}

/// The song's transition into the next one, and its ending. The transition can change
/// while playing; the ending changes the arrangement, so it needs playback stopped.
#[tauri::command]
fn set_transition(state: State<'_, AppState>, transition: Transition, ending: Option<Ending>) -> Result<AppView, String> {
    let same_ending = state.song.lock().unwrap().as_ref().ok_or("Load a song first.")?.file.ending == ending;
    let mut t = transition;
    t.bars = t.bars.clamp(0.25, 64.0);
    if same_ending {
        {
            let mut g = state.song.lock().unwrap();
            let l = g.as_mut().ok_or("Load a song first.")?;
            l.file.transition = t;
            l.file.save(&l.folder)?;
        }
        return Ok(view(&state));
    }
    if state.core.playing() {
        return Err("Stop playback before changing the ending.".into());
    }
    edit_song(&state, |f| {
        f.transition = t;
        f.ending = ending.map(|e| Ending { at: e.at.max(0.0), bars: e.bars.clamp(0.25, 64.0) });
    })
}

/// Saves a new arrangement. While playing it is swapped in live: `keep` is the
/// new index of the block that's playing, so playback carries on from there.
#[tauri::command]
fn set_arrangement(state: State<'_, AppState>, blocks: Vec<Block>, keep: Option<usize>) -> Result<AppView, String> {
    if !state.core.playing() {
        return edit_song(&state, |f| f.arrangement = blocks);
    }
    {
        let mut song = state.song.lock().unwrap();
        let l = song.as_mut().ok_or("Load a song first.")?;
        let mut file = l.file.clone();
        file.arrangement = blocks;
        let (_, arr) = file.build(l.sample_rate, l.length)?;
        let keep = keep.or_else(|| state.core.snapshot().map(|s| s.item)).unwrap_or(0).min(arr.items.len() - 1);
        let sent = state.core.output.lock().unwrap().as_mut().map_or(false, |o| o.send(ToAudio::Arrange(Box::new(arr.clone()), keep)));
        if !sent {
            return Err("The audio thread is busy. Try again.".into());
        }
        l.arr = arr;
        l.file = file;
        l.file.save(&l.folder)?;
    }
    Ok(view(&state))
}

/* ---------------- drag and drop ---------------- */

#[derive(Serialize, Clone, Default)]
struct DropResult {
    songs_added: usize,
    tracks_added: Vec<String>,
    message: Option<String>,
}

/// Copies dropped audio files in next to the song's stems (the folder its first stem
/// is in), renaming instead of overwriting. Returns the new tracks' names.
fn copy_tracks(song_folder: &Path, first_stem: Option<&str>, files: &[PathBuf]) -> Result<Vec<String>, String> {
    let rel_dir = first_stem.and_then(|f| Path::new(f).parent()).map(PathBuf::from).unwrap_or_default();
    let dir = song_folder.join(&rel_dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut names = vec![];
    for src in files {
        let stem = src.file_stem().and_then(|n| n.to_str()).unwrap_or("Track").to_string();
        let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("wav").to_string();
        if src.parent() == Some(dir.as_path()) {
            names.push(stem); // already there
            continue;
        }
        let mut name = stem.clone();
        let mut n = 2;
        while dir.join(format!("{name}.{ext}")).exists() {
            name = format!("{stem} {n}");
            n += 1;
        }
        std::fs::copy(src, dir.join(format!("{name}.{ext}"))).map_err(|e| format!("Couldn't copy {}: {e}", src.display()))?;
        names.push(name);
    }
    Ok(names)
}

/// Folders dropped on the window become songs in the set; audio files dropped while a
/// song is loaded become new tracks of that song (starting at bar 1).
async fn handle_drop(app: &AppHandle, paths: Vec<PathBuf>) -> DropResult {
    let state = app.state::<AppState>();
    let mut r = DropResult::default();
    let (dirs, files): (Vec<PathBuf>, Vec<PathBuf>) = paths.into_iter().partition(|p| p.is_dir());
    let audio: Vec<PathBuf> = files.iter().filter(|p| decode::is_audio(p)).cloned().collect();
    {
        let mut s = state.settings.lock().unwrap();
        for d in dirs {
            if songfile::find_stems(&d).is_ok() && !s.setlist.contains(&d) {
                s.setlist.push(d);
                r.songs_added += 1;
            }
        }
    }
    if r.songs_added > 0 {
        state.save_settings();
    }
    if audio.len() < files.len() {
        r.message = Some("Some dropped files aren't audio, so they were skipped.".into());
    }
    if audio.is_empty() {
        return r;
    }
    if state.core.playing() {
        r.message = Some("Stop playback before adding tracks.".into());
        return r;
    }
    let target = state.song.lock().unwrap().as_ref().map(|l| (l.folder.clone(), l.file.stems.first().map(|s| s.file.clone())));
    let Some((folder, first)) = target else {
        r.message = Some("Load a song first, then drop audio files on it to add them as tracks.".into());
        return r;
    };
    let names = match copy_tracks(&folder, first.as_deref(), &audio) {
        Ok(n) => n,
        Err(e) => {
            r.message = Some(e);
            return r;
        }
    };
    if let Err(e) = reload_current(&state).await {
        r.message = Some(e);
        return r;
    }
    r.tracks_added = names;
    r
}

/// Bus management: make, rename and delete buses, and move a track to a bus.
#[derive(Deserialize)]
#[serde(tag = "type")]
enum BusEdit {
    Create { name: String },
    Rename { bus: String, name: String },
    Delete { bus: String },
    /// Move every track with this name (in any song) to a bus; None = its default bus.
    Assign { track: String, bus: Option<String> },
}

#[tauri::command]
fn bus_edit(state: State<'_, AppState>, edit: BusEdit) -> Result<AppView, String> {
    {
        let mut s = state.settings.lock().unwrap();
        s.ensure_buses();
        match edit {
            BusEdit::Create { name } => {
                let name = name.trim();
                let name = if name.is_empty() { "New bus" } else { name };
                let mut n = 1;
                let id = loop {
                    let id = format!("bus-{n}");
                    if !s.bus_list.iter().any(|b| b.id == id) {
                        break id;
                    }
                    n += 1;
                };
                s.bus_list.push(Bus { id, name: name.to_string(), custom: true, ..Default::default() });
            }
            BusEdit::Rename { bus, name } => {
                let name = name.trim().to_string();
                if name.is_empty() {
                    return Err("A bus needs a name.".into());
                }
                s.bus_mut(&bus).ok_or("That bus doesn't exist.")?.name = name;
            }
            BusEdit::Delete { bus } => {
                let b = s.bus_list.iter().find(|b| b.id == bus).ok_or("That bus doesn't exist.")?;
                if !b.custom {
                    return Err("Built-in buses can't be deleted. You can rename them, or move their tracks elsewhere.".into());
                }
                s.bus_list.retain(|b| b.id != bus);
                s.bus_of_track.retain(|_, v| *v != bus); // its tracks go back to their default bus
            }
            BusEdit::Assign { track, bus } => {
                let key = track.trim().to_ascii_lowercase();
                match bus {
                    Some(id) if s.bus_list.iter().any(|b| b.id == id) => {
                        s.bus_of_track.insert(key, id);
                    }
                    Some(_) => return Err("That bus doesn't exist.".into()),
                    None => {
                        s.bus_of_track.remove(&key);
                    }
                }
            }
        }
    }
    // Settings and the song are locked one after the other, never together.
    let s = state.settings.lock().unwrap().clone();
    if let Some(l) = state.song.lock().unwrap().as_ref() {
        push_mix(&state.core, &s, &l.file);
    }
    state.save_settings();
    Ok(view(&state))
}

#[tauri::command]
async fn choose_pads_folder(app: AppHandle, state: State<'_, AppState>) -> Result<AppView, String> {
    let picked = app.dialog().file().set_title("Choose your pads folder (one audio file per key)").blocking_pick_folder();
    if let Some(p) = picked.and_then(|f| f.into_path().ok()) {
        if pads::scan(&p).iter().all(|f| f.is_none()) {
            return Err("No pad files found there. Each file's name needs its key in it, like \"A Pad.wav\" or \"Pad - Bb.m4a\".".into());
        }
        state.settings.lock().unwrap().pads_folder = Some(p);
        state.save_settings();
        *state.pad_key.lock().unwrap() = None;
        let _ = pad_update(&state);
    }
    Ok(view(&state))
}

#[tauri::command]
fn pad_set(state: State<'_, AppState>, on: bool) -> Result<AppView, String> {
    *state.pad_on.lock().unwrap() = on;
    if let Err(e) = pad_update(&state) {
        *state.pad_on.lock().unwrap() = false;
        let _ = pad_update(&state);
        return Err(e);
    }
    Ok(view(&state))
}

/// Plays the pad in a key picked by hand (None = back to the song's key).
#[tauri::command]
fn pad_key(state: State<'_, AppState>, key: Option<String>) -> Result<AppView, String> {
    let pc = match key.as_deref() {
        Some(k) => Some(pads::pitch_class(k).ok_or(format!("\"{k}\" isn't a key."))?),
        None => None,
    };
    *state.pad_choice.lock().unwrap() = pc;
    *state.pad_on.lock().unwrap() = true;
    if let Err(e) = pad_update(&state) {
        *state.pad_on.lock().unwrap() = false;
        let _ = pad_update(&state);
        return Err(e);
    }
    Ok(view(&state))
}

/// The free click: a click with no song (between songs, announcements, rehearsal).
/// It goes to the click output at the click level, and stops when a song starts.
#[tauri::command]
fn free_click(state: State<'_, AppState>, on: bool, bpm: f64, beats_per_bar: u32) -> Result<AppView, String> {
    let bpm = bpm.clamp(30.0, 300.0);
    let bpb = beats_per_bar.clamp(1, 12);
    if on && state.core.playing() {
        return Err("A song is playing; its own click is on. Stop the song to run the free click.".into());
    }
    let (gain, route) = {
        let mut s = state.settings.lock().unwrap();
        s.free_click_bpm = bpm;
        s.free_click_bpb = bpb;
        (s.click_gain, s.click_route)
    };
    state.save_settings();
    {
        let mut out = state.core.output.lock().unwrap();
        let o = out.as_mut().ok_or("No audio output is open.")?;
        let route = route.unwrap_or(songfile::default_route("click", o.info.channels));
        if !o.send(ToAudio::Metro { on, bpm, beats_per_bar: bpb, gain, route }) {
            return Err("The audio thread is busy. Try again.".into());
        }
    }
    state.core.metro_on.store(on, Ordering::Release);
    Ok(view(&state))
}

#[tauri::command]
fn pad_mix(state: State<'_, AppState>, level: Option<f32>, route: Option<Route>) -> Result<(), String> {
    {
        let mut s = state.settings.lock().unwrap();
        if let Some(l) = level {
            s.pad_level = l.clamp(0.0, 1.5);
        }
        if let Some(r) = route {
            s.pad_route = Some(r);
        }
    }
    state.save_settings();
    let s = state.settings.lock().unwrap().clone();
    if let Some(o) = state.core.output.lock().unwrap().as_mut() {
        o.send(ToAudio::PadLevel(s.pad_level));
        o.send(ToAudio::PadRoute(s.pad_route.unwrap_or(Route::Pair(0))));
    }
    Ok(())
}

/// Makes a click (and spoken guide) song, adds it to the end of the set and returns
/// its position in the set.
#[tauri::command]
async fn create_click_song(app: AppHandle, state: State<'_, AppState>, spec: clicksong::Spec) -> Result<usize, String> {
    let root = app
        .path()
        .audio_dir()
        .or_else(|_| app.path().document_dir())
        .map_err(|e| e.to_string())?
        .join("Playback Rig")
        .join("Click Songs");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let work = state.core.data_dir.join("voice");
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let (folder, _guide) = tauri::async_runtime::spawn_blocking(move || {
        let voice = clicksong::mac_voice(work);
        clicksong::create(&root, &spec, &voice)
    })
    .await
    .map_err(|e| e.to_string())??;
    let mut s = state.settings.lock().unwrap();
    s.setlist.push(folder);
    let i = s.setlist.len() - 1;
    drop(s);
    state.save_settings();
    Ok(i)
}

/// Changes the key (semitones) and/or tempo, then re-renders the stems.
#[tauri::command]
async fn set_key_tempo(app: AppHandle, state: State<'_, AppState>, transpose: i32, bpm: Option<f64>) -> Result<AppView, String> {
    if state.core.playing() {
        return Err("Stop playback before changing the key or tempo.".into());
    }
    let (folder, sr, ch) = {
        let mut song = state.song.lock().unwrap();
        let l = song.as_mut().ok_or("Load a song first.")?;
        let mut f = l.file.clone();
        f.transpose = transpose.clamp(-12, 12);
        f.target_bpm = bpm.filter(|b| (b - f.bpm).abs() > 0.004).map(|b| b.clamp(f.bpm * 0.5, f.bpm * 2.0));
        f.save(&l.folder)?;
        let out = state.core.output.lock().unwrap();
        let o = out.as_ref().ok_or("No audio output is open.")?;
        (l.folder.clone(), o.info.sample_rate, o.info.channels)
    };
    let _ = app.emit("analysis", AnalysisProgress { stage: "Changing key and tempo".into(), progress: 0.4 });
    let loaded = tauri::async_runtime::spawn_blocking(move || load(&folder, sr, ch)).await.map_err(|e| e.to_string())??;
    let settings = state.settings.lock().unwrap().clone();
    install(&state.core, &settings, &loaded)?;
    *state.song.lock().unwrap() = Some(loaded);
    let _ = pad_update(&state);
    Ok(view(&state))
}

#[derive(Serialize)]
struct PeaksView {
    per_sec: u32,
    stems: Vec<String>,
}

#[tauri::command]
fn song_peaks(state: State<'_, AppState>) -> Option<PeaksView> {
    state.song.lock().unwrap().as_ref().map(|l| PeaksView { per_sec: PEAKS_PER_SEC, stems: l.peaks.clone() })
}

#[derive(Serialize, Clone)]
struct AnalysisProgress {
    stage: String,
    progress: f32,
}

#[derive(Serialize)]
struct AnalysisResult {
    view: AppView,
    heard: Vec<(f64, String)>,
    found_tempo: bool,
    sections: usize,
}

/// Reads the Click (tempo, downbeat) and Guide (section cues) stems and sets up the
/// song's markers. Downloads the speech model the first time.
#[tauri::command]
async fn analyze_song(app: AppHandle, state: State<'_, AppState>) -> Result<AnalysisResult, String> {
    if state.core.playing() {
        return Err("Stop playback before reading the guide.".into());
    }
    // Read the original recordings from disk, not the key/tempo-changed copies.
    let (click_path, guide_path, sr, bpb, bpm, known_bpm) = {
        let song = state.song.lock().unwrap();
        let l = song.as_ref().ok_or("Load a song first.")?;
        let known_bpm = songfile::parse_folder_name(&songfile::song_title(&l.folder)).2;
        let find = |f: fn(&str) -> bool| l.file.stems.iter().find(|s| f(&s.name)).map(|s| l.folder.join(&s.file));
        (find(songfile::is_click), find(songfile::is_guide), l.sample_rate, l.file.beats_per_bar, l.file.bpm, known_bpm)
    };
    if click_path.is_none() && guide_path.is_none() {
        return Err("This song has no Click or Guide stem to read. Add sections by hand in the Song panel.".into());
    }
    let (click, guide) = tauri::async_runtime::spawn_blocking(move || {
        let read = |p: Option<PathBuf>| p.map(|p| decode::decode(&p, sr)).transpose();
        Ok::<_, String>((read(click_path)?, read(guide_path)?))
    })
    .await
    .map_err(|e| e.to_string())??;
    let data_dir = state.core.data_dir.clone();
    let h = app.clone();
    let analysis = tauri::async_runtime::spawn_blocking(move || {
        let emit = |stage: &str, p: f32| {
            let _ = h.emit("analysis", AnalysisProgress { stage: stage.into(), progress: p });
        };
        let model = match &guide {
            Some(_) => Some(model::ensure(&data_dir, |p| emit("Downloading the speech model (one time, about 150 MB)", p))?),
            None => None,
        };
        guide::analyze(click.as_ref(), guide.as_ref(), sr, bpb, bpm, known_bpm, model.as_deref(), |s, p| emit(s, p))
    })
    .await
    .map_err(|e| e.to_string())??;
    let found_tempo = analysis.bpm.is_some();
    let sections = analysis.markers.len();
    let view = edit_song(&state, |f| {
        if analysis.bpm.is_some() || analysis.offset_ms.is_some() {
            f.beat_times.clear(); // reading the guide sets one steady tempo
        }
        if let Some(b) = analysis.bpm {
            f.bpm = b;
        }
        if let Some(o) = analysis.offset_ms {
            f.offset_ms = o;
        }
        if !analysis.markers.is_empty() {
            f.markers = analysis.markers.clone();
            f.arrangement = vec![];
        }
        f.markers_from = Some(if analysis.markers.is_empty() { "manual" } else { "guide" }.into());
        if analysis.offset_ms.is_some() {
            f.grid_version = songfile::GRID_VERSION; // the guide reading already put bar 1 on the click
        }
    })?;
    Ok(AnalysisResult { view, heard: analysis.heard, found_tempo, sections })
}

#[tauri::command]
fn midi_learn(state: State<'_, AppState>, tap: Action, hold: Option<Action>) {
    state.core.map.lock().unwrap().learn(tap, hold);
}

#[tauri::command]
fn midi_cancel(state: State<'_, AppState>) {
    state.core.map.lock().unwrap().cancel_learn();
}

#[tauri::command]
fn midi_unbind(state: State<'_, AppState>, index: usize) -> AppView {
    {
        let mut m = state.core.map.lock().unwrap();
        if index < m.bindings.len() {
            m.bindings.remove(index);
        }
    }
    state.core.save_map();
    view(&state)
}

/* ---------------- updates ---------------- */

#[derive(Serialize)]
struct UpdateInfo {
    version: String,
    current: String,
    notes: Option<String>,
}

/// Asks GitHub Releases whether a newer version exists. None = up to date.
#[tauri::command]
async fn check_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    let u = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| format!("Couldn't check for updates. Check the internet connection. ({e})"))?;
    Ok(u.map(|u| UpdateInfo { version: u.version.clone(), current: u.current_version.clone(), notes: u.body.clone() }))
}

/// Downloads the new version, checks its signature, installs it and restarts.
/// Refused while playing.
#[tauri::command]
async fn install_update(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if state.core.playing() {
        return Err("Stop playback before updating.".into());
    }
    let u = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| format!("Couldn't check for updates. ({e})"))?
        .ok_or("You're already on the latest version.")?;
    let h = app.clone();
    let mut got = 0u64;
    u.download_and_install(
        move |chunk, total| {
            got += chunk as u64;
            if let Some(t) = total.filter(|t| *t > 0) {
                let _ = h.emit("update-progress", got as f32 / t as f32);
            }
        },
        || {},
    )
    .await
    .map_err(|e| format!("The update didn't install: {e}"))?;
    *state.core.output.lock().unwrap() = None; // close the audio device cleanly
    app.restart();
}

#[tauri::command]
fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                let app = window.app_handle().clone();
                let paths = paths.clone();
                tauri::async_runtime::spawn(async move {
                    let r = handle_drop(&app, paths).await;
                    let _ = app.emit("dropped", r);
                });
            }
        })
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            std::fs::create_dir_all(&dir)?;
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let mut settings: Settings = read_json(&dir.join("settings.json"));
            settings.ensure_buses();
            if settings.version < 1 {
                settings.count_in = false; // count-in is now off unless asked for
                settings.version = 1;
                write_json(&dir.join("settings.json"), &settings);
            }
            let map: MidiMap = read_json(&dir.join("midi.json"));
            let core = Arc::new(Core {
                output: Mutex::new(None),
                status: Arc::new(Mutex::new(None)),
                segues: Arc::new(AtomicU64::new(0)),
                metro_on: Arc::new(AtomicBool::new(false)),
                map: Mutex::new(map),
                config_dir: dir,
                data_dir,
                start: Instant::now(),
            });
            let handle = app.handle().clone();
            if open_output(&core, &handle, settings.output.clone()).is_err() {
                if let Err(e) = open_output(&core, &handle, None) {
                    eprintln!("No audio output: {e}");
                }
            }
            let midi = settings.midi.as_deref().and_then(|n| start_midi(&core, &handle, n).ok());
            {
                let core = core.clone();
                let h = handle.clone();
                std::thread::Builder::new().name("midi-hold".into()).spawn(move || loop {
                    std::thread::sleep(Duration::from_millis(10));
                    let ev = core.map.lock().unwrap().tick(core.now_ms());
                    if let Some(ev) = ev {
                        dispatch(&core, &h, ev);
                    }
                })?;
            }
            app.manage(AppState {
                core,
                song: Mutex::new(None),
                settings: Mutex::new(settings),
                midi: Mutex::new(midi),
                pad_on: Mutex::new(false),
                pad_key: Mutex::new(None),
                next: Mutex::new(NextSong::default()),
                seen_segues: AtomicU64::new(0),
                pad_auto: AtomicBool::new(false),
                pad_choice: Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_state,
            status,
            choose_output,
            choose_midi,
            add_songs,
            remove_song,
            move_song,
            select_song,
            cmd,
            update_song,
            set_arrangement,
            set_transition,
            pad_key,
            free_click,
            song_peaks,
            analyze_song,
            check_update,
            set_key_tempo,
            choose_pads_folder,
            pad_set,
            bus_edit,
            pad_mix,
            create_click_song,
            install_update,
            app_version,
            midi_learn,
            midi_cancel,
            midi_unbind
        ])
        .run(tauri::generate_context!())
        .expect("error while running Playback Rig");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        let dir = std::env::temp_dir().join(format!("rig-state-{}", std::process::id()));
        AppState {
            core: Arc::new(Core {
                output: Mutex::new(None),
                status: Arc::new(Mutex::new(None)),
                segues: Arc::new(AtomicU64::new(0)),
                metro_on: Arc::new(AtomicBool::new(false)),
                map: Mutex::new(MidiMap::default()),
                config_dir: dir.clone(),
                data_dir: dir,
                start: Instant::now(),
            }),
            song: Mutex::new(None),
            settings: Mutex::new(Settings::default()),
            midi: Mutex::new(None),
            pad_on: Mutex::new(false),
            pad_key: Mutex::new(None),
            next: Mutex::new(NextSong::default()),
            seen_segues: AtomicU64::new(0),
            pad_auto: AtomicBool::new(false),
            pad_choice: Mutex::new(None),
        }
    }

    /// Runs `f` on another thread and fails if it doesn't finish (a deadlock).
    fn finishes<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(Duration::from_secs(10)).expect("timed out: deadlock")
    }

    fn write_wav(path: &Path, x: &[f32], sr: u32) {
        let spec = hound::WavSpec { channels: 1, sample_rate: sr, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for v in x {
            w.write_sample((v.clamp(-1.0, 1.0) * 32767.0) as i16).unwrap();
        }
        w.finalize().unwrap();
    }

    /// A MultiTracks-style download: stems in MultiTracks/, an Ableton set running at
    /// double time (155 for a 77.5 song) with a "Build" cue locator, an eighth-note
    /// click that starts on the very first sample (bar 1), folder name with key and tempo.
    fn multitracks_folder(root: &Path) -> PathBuf {
        let sr = 44_100u32;
        let folder = root.join("See a Victory-At Midnight-A-77.50bpm");
        let mt = folder.join("MultiTracks");
        std::fs::create_dir_all(&mt).unwrap();
        let eighth = 60.0 / 155.0;
        let len = (sr as f64 * 60.0) as usize;
        let mut click = vec![0f32; len];
        let mut k = 0;
        loop {
            let t = k as f64 * eighth;
            let s0 = (t * sr as f64) as usize;
            if s0 + 2000 >= len { break; }
            let (f, a) = if k % 4 == 0 { (1760.0, 0.9) } else { (1175.0, 0.55) };
            for i in 0..1500 {
                let tt = i as f32 / sr as f32;
                click[s0 + i] += (std::f32::consts::TAU * f * tt).sin() * a * (-tt * 80.0).exp();
            }
            k += 1;
        }
        write_wav(&mt.join("Click Track.wav"), &click, sr);
        let tone = |hz: f32, amp: f32| -> Vec<f32> { (0..len).map(|i| (i as f32 / sr as f32 * hz * std::f32::consts::TAU).sin() * amp).collect() };
        // Guide: "1 . 2 . | 1 2 3 4" count-in over bars 1-2, then a call in the bar
        // before each section (Verse in bar 6, Chorus in bars 10 and 14).
        let beat = 60.0 / 77.5;
        let at = |b: u32, bt: f64| ((b - 1) as f64 * 4.0 + bt) * beat;
        let mut guide = vec![0f32; len];
        let mut say = |t: f64, l: f64| {
            for i in (t * sr as f64) as usize..((t + l) * sr as f64) as usize {
                guide[i] = (i as f32 * 0.21).sin() * 0.4;
            }
        };
        for (b, bt) in [(1, 0.0), (1, 2.0), (2, 0.0), (2, 1.0), (2, 2.0), (2, 3.0)] {
            say(at(b, bt), 0.3);
        }
        for b in [6, 10, 14] {
            say(at(b, 0.0), 0.6);
            say(at(b, 2.0), 0.3);
            say(at(b, 3.0), 0.3);
        }
        write_wav(&mt.join("Guide.wav"), &guide, sr);
        write_wav(&mt.join("BGV.wav"), &tone(330.0, 0.2), sr);
        write_wav(&mt.join("Bass.wav"), &tone(55.0, 0.3), sr);
        write_wav(&mt.join("Drums (Live).wav"), &tone(80.0, 0.3), sr);
        write_wav(&mt.join("EG 2.wav"), &tone(220.0, 0.2), sr);
        write_wav(&mt.join("EG 1.wav"), &tone(220.0, 0.2), sr);
        // Live beats at 155: the set says beat 0 is 0.28 s into the audio (a reading
        // that left the grid late, as the user saw), 155 beats per 60 s.
        let als = format!(r#"<Ableton><LiveSet><Tracks><AudioTrack><DeviceChain><Mixer><Tempo><LomId Value="0" /><Manual Value="120" /></Tempo></Mixer>
            <AudioClip Id="1" Time="0"><LoopStart Value="0" /><StartRelative Value="0" /><IsWarped Value="true" />
            <WarpMarkers><WarpMarker Id="0" SecTime="{s0}" BeatTime="0" /><WarpMarker Id="1" SecTime="{s1}" BeatTime="155" /></WarpMarkers></AudioClip>
            </DeviceChain></AudioTrack></Tracks>
            <Locators><Locators>
              <Locator Id="0"><Time Value="16" /><Name Value="Intro" /></Locator>
              <Locator Id="1"><Time Value="48" /><Name Value="Verse 1" /></Locator>
              <Locator Id="2"><Time Value="64" /><Name Value="Build" /></Locator>
              <Locator Id="3"><Time Value="80" /><Name Value="Chorus" /></Locator>
              <Locator Id="4"><Time Value="104" /><Name Value="Chorus" /></Locator>
            </Locators></Locators>
            <MasterTrack><DeviceChain><Mixer><Tempo><LomId Value="0" /><Manual Value="155" /></Tempo>
            <TimeSignature><LomId Value="0" /><Manual Value="201" /></TimeSignature></Mixer></DeviceChain></MasterTrack></LiveSet></Ableton>"#,
            s0 = 0.28, s1 = 0.28 + 60.0);
        std::fs::write(folder.join("MultiTrack.als"), als).unwrap();
        folder
    }

    #[test]
    fn a_multitracks_song_loads_onto_the_click_grid_with_buses_and_key_change() {
        let root = std::env::temp_dir().join(format!("rig-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let folder = multitracks_folder(&root);
        let sr = 48_000;
        let l = load(&folder, sr, 10).unwrap();
        let f = &l.file;
        // folder name and Ableton set
        assert_eq!((f.name.as_str(), f.key.as_str(), f.bpm, f.beats_per_bar), ("See a Victory", "A", 77.5, 4));
        assert_eq!(f.markers_from.as_deref(), Some("ableton"));
        // band cue dropped, repeated name numbered, sections on real bars; the second
        // Chorus sat on its cue (bar 14) in the Ableton set and moves a bar later
        let m: Vec<(&str, u32)> = f.markers.iter().map(|m| (m.name.as_str(), m.bar)).collect();
        assert_eq!(m, [("Intro", 3), ("Verse 1", 7), ("Chorus 1", 11), ("Chorus 2", 15)]);
        assert_eq!(l.song.sections[0].name, "Count-in", "the two bars before Intro");
        // The guide is cut by section: each called section knows its call, which ends
        // where it starts and sits within the bar or two before it.
        assert_eq!(l.song.guide_stem, Some(1));
        assert_eq!(l.song.calls.len(), l.song.sections.len());
        let bar = l.song.frames_per_bar();
        for (i, name) in [(2, "Verse 1"), (3, "Chorus 1"), (4, "Chorus 2")] {
            assert_eq!(l.song.sections[i].name, name);
            let (cs, ce) = l.song.calls[i].unwrap_or_else(|| panic!("{name} has no call"));
            assert_eq!(ce, l.song.sections[i].start);
            assert!(ce - cs > (bar * 0.5) as u64 && ce - cs <= (bar * 2.1) as u64, "{name}: {cs}..{ce}");
        }
        assert_eq!(l.song.calls[0], None, "nothing calls the count-in");
        // bar 1 is the first click, at the very start of the file
        assert_eq!(f.grid_version, songfile::GRID_VERSION);
        assert_eq!(f.offset_ms, 0.0, "bar 1 is the first click");
        assert!(f.beat_times.is_empty(), "a steady song keeps its simple grid");
        // every section between the first and last is whole bars
        let bar = l.song.frames_per_bar();
        for s in &l.song.sections[1..l.song.sections.len() - 1] {
            let b = s.len() as f64 / bar;
            assert!((b - b.round()).abs() * bar < 2.0, "{} = {b} bars", s.name);
        }
        // band order: click & guide, drums, bass, guitars (EG 1 before EG 2), vocals
        let names: Vec<&str> = f.stems.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Click Track", "Guide", "Drums (Live)", "Bass", "EG 1", "EG 2", "BGV"]);
        // the setup was saved, and loading again doesn't redo or move it
        let again = load(&folder, sr, 10).unwrap();
        assert_eq!(again.file.offset_ms, f.offset_ms);
        assert_eq!(again.file.markers, f.markers);

        // buses: the Guitars bus to Out 5-6 at half level, muted
        let mut st = Settings::default();
        {
            let g = st.bus_mut("guitars").unwrap();
            g.route = Some(Route::Pair(4));
            g.gain = 0.5;
            g.muted = true;
        }
        let eg = &f.stems[4];
        assert_eq!(effective(&st, eg), (0.5, true, Route::Pair(4)));
        assert_eq!(effective(&st, &f.stems[3]), (1.0, false, f.stems[3].route), "bass bus untouched");

        // key change: up a tone, click and guide unchanged, same length, grid unchanged
        let mut f2 = f.clone();
        f2.transpose = 2;
        f2.save(&folder).unwrap();
        let k = load(&folder, sr, 10).unwrap();
        assert_eq!(k.file.transpose, 2);
        assert_eq!(k.length, l.length);
        assert_eq!(k.song.sections[1].start, l.song.sections[1].start);
        assert_eq!(song_key_pc(&k.file), Some(11), "A + 2 = B");
        // tempo change: 10% faster, sections move earlier in proportion
        let mut f3 = k.file.clone();
        f3.target_bpm = Some(85.25);
        f3.save(&folder).unwrap();
        let t = load(&folder, sr, 10).unwrap();
        let ratio = t.song.sections[2].start as f64 / l.song.sections[2].start as f64;
        assert!((ratio - 77.5 / 85.25).abs() < 0.002, "ratio {ratio}");
        assert_eq!(t.file.grid_version, songfile::GRID_VERSION, "a tempo change doesn't redo the grid");

        // the app state works with this song loaded (no deadlock, pads report the key)
        let state = Arc::new(state());
        *state.song.lock().unwrap() = Some(t);
        let s2 = state.clone();
        let v = finishes(move || view(&s2));
        assert_eq!(v.pads.song_key.as_deref(), Some("B"));
        let cats: Vec<&str> = v.buses.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(cats, ["Click & guide", "Drums & percussion", "Bass", "Guitars", "Vocals"]);
        assert_eq!(v.buses[3].stems, vec![4, 5]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn settings_from_0_5_0_keep_the_set_and_carry_bus_settings_over() {
        let old = r#"{"output":"UMC1820","setlist":["/songs/See a Victory"],"current":0,"quantize":"Bar","count_in":false,
            "click_gain":0.8,"version":1,"pad_level":0.6,
            "buses":[{"gain":1.0,"muted":false,"route":{"Pair":2}},{"gain":0.7,"muted":false,"route":{"Pair":4}}]}"#;
        let mut s: Settings = serde_json::from_str(old).expect("0.5.0 settings must still load");
        s.ensure_buses();
        assert_eq!(s.setlist, vec![PathBuf::from("/songs/See a Victory")]);
        assert_eq!(s.output.as_deref(), Some("UMC1820"));
        let d = s.bus_list.iter().find(|b| b.id == "drums").unwrap();
        assert_eq!((d.gain, d.route), (0.7, Some(Route::Pair(4))), "old Drums bus settings carried over");
        assert_eq!(s.bus_list.len(), DEFAULT_BUSES.len());
        // the old field is never written back
        assert!(!serde_json::to_string(&s).unwrap().contains("\"buses\""));
    }

    #[test]
    fn tracks_go_to_their_group_bus_unless_moved_and_come_back_when_a_bus_is_deleted() {
        let mut s = Settings::default();
        assert_eq!(s.bus_id_for("Perc Loop"), "drums", "percussion shares the drums bus");
        assert_eq!(s.bus_id_for("Tambourine"), "drums");
        assert_eq!(s.bus_id_for("Synth Bass"), "bass");
        s.bus_list.push(Bus { id: "bus-1".into(), name: "Pads".into(), custom: true, ..Default::default() });
        s.bus_of_track.insert("pad".into(), "bus-1".into());
        assert_eq!(s.bus_id_for("Pad"), "bus-1", "moved by name, any song");
        assert_eq!(s.bus_id_for("Keys 1"), "keys");
        s.bus_list.retain(|b| b.id != "bus-1");
        assert_eq!(s.bus_id_for("Pad"), "keys", "deleted bus: back to its default");
    }

    #[test]
    fn dropped_tracks_join_the_song_without_overwriting() {
        let root = std::env::temp_dir().join(format!("rig-drop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let folder = multitracks_folder(&root);
        let ext = root.join("exports");
        std::fs::create_dir_all(&ext).unwrap();
        write_wav(&ext.join("New Synth.wav"), &vec![0.1; 44_100], 44_100);
        write_wav(&ext.join("Bass.wav"), &vec![0.1; 44_100], 44_100); // same name as an existing stem
        let names = copy_tracks(&folder, Some("MultiTracks/Click Track.wav"), &[ext.join("New Synth.wav"), ext.join("Bass.wav")]).unwrap();
        assert_eq!(names, ["New Synth", "Bass 2"]);
        let l = load(&folder, 48_000, 10).unwrap();
        let tracks: Vec<&str> = l.file.stems.iter().map(|s| s.name.as_str()).collect();
        assert!(tracks.contains(&"New Synth") && tracks.contains(&"Bass 2") && tracks.contains(&"Bass"), "{tracks:?}");
        assert_eq!(l.file.stems.iter().find(|s| s.name == "Bass 2").unwrap().file, "MultiTracks/Bass 2.wav");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_click_song_loads_like_any_other_song() {
        let root = std::env::temp_dir().join(format!("rig-click-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let spec = clicksong::Spec { name: "Goodness".into(), bpm: 63.0, beats_per_bar: 4, key: "Ab".into(), sections: vec![("Verse".into(), 8), ("Chorus".into(), 8)], guide: true };
        let voice = |_: &str| Some(vec![0.2f32; 4000]);
        let (folder, _) = clicksong::create(&root, &spec, &voice).unwrap();
        let l = load(&folder, 48_000, 2).unwrap();
        assert_eq!(l.file.markers_from.as_deref(), Some("manual"));
        assert_eq!(l.file.grid_version, 0, "hand-made grids are never moved");
        assert_eq!(l.song.sections.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Count", "Verse", "Chorus"]);
        assert_eq!(song_key_pc(&l.file), Some(8));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_song_with_a_tempo_change_gets_a_grid_that_follows_it() {
        let root = std::env::temp_dir().join(format!("rig-tempo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let sr = 44_100u32;
        let folder = root.join("Tempo Song-A-70bpm");
        std::fs::create_dir_all(&folder).unwrap();
        // 8 bars at 70, then 12 bars at 84.
        let bt: Vec<f64> = (0..=80).scan(0.0, |t, k| { let v = *t; *t += 60.0 / if k < 32 { 70.0 } else { 84.0 }; Some(v) }).collect();
        let len = ((bt[80] + 2.0) * sr as f64) as usize;
        let mut click = vec![0f32; len];
        for (k, &t0) in bt.iter().enumerate() {
            let s0 = (t0 * sr as f64) as usize;
            let (f, a) = if k % 4 == 0 { (1760.0, 0.9) } else { (1175.0, 0.55) };
            for i in 0..1500 {
                let tt = i as f32 / sr as f32;
                click[s0 + i] += (std::f32::consts::TAU * f * tt).sin() * a * (-tt * 80.0).exp();
            }
        }
        write_wav(&folder.join("Click Track.wav"), &click, sr);
        let mut guide = vec![0f32; len];
        let bar = |b: usize| bt[(b - 1) * 4];
        let beat_t = |b: usize, k: usize| bt[(b - 1) * 4 + k];
        let mut say = |t: f64, l: f64| for i in (t * sr as f64) as usize..((t + l) * sr as f64) as usize { guide[i] = (i as f32 * 0.21).sin() * 0.4; };
        for b in [4, 8, 12] {
            say(bar(b), 0.5);
            say(beat_t(b, 2), 0.25);
            say(beat_t(b, 3), 0.25);
        }
        write_wav(&folder.join("Guide.wav"), &guide, sr);
        write_wav(&folder.join("Keys.wav"), &(0..len).map(|i| (i as f32 * 0.03).sin() * 0.2).collect::<Vec<_>>(), sr);
        let als = format!(r#"<Ableton><LiveSet><Tracks><AudioTrack><DeviceChain>
            <AudioClip Id="1" Time="0"><LoopStart Value="0" /><StartRelative Value="0" /><IsWarped Value="true" />
            <WarpMarkers><WarpMarker Id="0" SecTime="0" BeatTime="0" /><WarpMarker Id="1" SecTime="{a}" BeatTime="32" /><WarpMarker Id="2" SecTime="{b}" BeatTime="80" /></WarpMarkers></AudioClip>
            </DeviceChain></AudioTrack></Tracks>
            <Locators><Locators>
              <Locator Id="0"><Time Value="16" /><Name Value="Verse" /></Locator>
              <Locator Id="1"><Time Value="32" /><Name Value="Chorus" /></Locator>
              <Locator Id="2"><Time Value="48" /><Name Value="Bridge" /></Locator>
            </Locators></Locators>
            <MasterTrack><DeviceChain><Mixer><Tempo><LomId Value="0" /><Manual Value="70" /><AutomationTarget Id="8"><LockEnvelope Value="0" /></AutomationTarget></Tempo>
            <TimeSignature><LomId Value="0" /><Manual Value="201" /></TimeSignature></Mixer></DeviceChain>
            <AutomationEnvelopes><Envelopes><AutomationEnvelope Id="0"><EnvelopeTarget><PointeeId Value="8" /></EnvelopeTarget><Automation><Events>
              <FloatEvent Id="1" Time="-63072000" Value="70" /><FloatEvent Id="2" Time="32" Value="70" /><FloatEvent Id="3" Time="32" Value="84" />
            </Events></Automation></AutomationEnvelope></Envelopes></AutomationEnvelopes></MasterTrack></LiveSet></Ableton>"#, a = bt[32], b = bt[80]);
        std::fs::write(folder.join("MultiTrack.als"), als).unwrap();

        let l = load(&folder, 48_000, 2).unwrap();
        let f = &l.file;
        assert!(!f.beat_times.is_empty(), "the tempo change was found");
        let m: Vec<(&str, u32)> = f.markers.iter().map(|m| (m.name.as_str(), m.bar)).collect();
        assert_eq!(m, [("Verse", 5), ("Chorus", 9), ("Bridge", 13)]);
        assert_eq!(f.tempo_changes(), vec![(1, 70.0), (9, 84.0)]);
        // Sections start on the real downbeats, in the second tempo too.
        for (i, b) in [(1, 5), (2, 9), (3, 13)] {
            let want = bar(b) * 48_000.0;
            let got = l.song.sections[i].start as f64;
            assert!((got - want).abs() < 48.0 * 3.0, "{} at {got}, want {want}", l.song.sections[i].name);
        }
        assert_eq!(l.song.sections[0].name, "Count-in");
        // The view shows the bar lines where they are, and the tempo changes.
        let v = SongView::from(&l);
        assert!((v.downbeats[12] - bar(13)).abs() < 0.003, "bar 13 line at {}", v.downbeats[12]);
        assert_eq!(v.tempo_changes, vec![(1, 70.0), (9, 84.0)]);
        // The engine's click and bar numbers follow it: bar 13 at its real start.
        let s = &l.song;
        let pos = s.beat_pos(bar(13) * 48_000.0 + 10.0);
        assert_eq!((pos / 4.0).floor() as i64 + 1, 13);
        assert!((s.bpm_at(bar(13) * 48_000.0) - 84.0).abs() < 0.5);
        // The guide calls are found on the new grid: every section has its call.
        assert!(l.song.calls[1..].iter().all(|c| c.is_some()), "{:?}", l.song.calls);
        // Saved and loaded again: same grid, nothing moves.
        let again = load(&folder, 48_000, 2).unwrap();
        assert_eq!(again.file.markers, f.markers);
        assert_eq!(again.file.beat_times.len(), f.beat_times.len());
        assert!(again.file.beat_times.iter().zip(&f.beat_times).all(|(a, b)| (a - b).abs() < 1e-9), "saved and read back (to the last decimal)");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_segue_makes_the_next_song_current() {
        let root = std::env::temp_dir().join(format!("rig-segue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let spec = |n: &str| clicksong::Spec { name: n.into(), bpm: 70.0, beats_per_bar: 4, key: "G".into(), sections: vec![("Verse".into(), 4)], guide: false };
        let voice = |_: &str| Some(vec![0.2f32; 4000]);
        let (a, _) = clicksong::create(&root, &spec("First"), &voice).unwrap();
        let (b, _) = clicksong::create(&root, &spec("Second"), &voice).unwrap();
        let st = Arc::new(state());
        {
            let mut s = st.settings.lock().unwrap();
            s.setlist = vec![a.clone(), b.clone()];
            s.current = Some(0);
        }
        *st.song.lock().unwrap() = Some(load(&a, 48_000, 2).unwrap());
        st.next.lock().unwrap().loaded = Some(load(&b, 48_000, 2).unwrap());
        assert!(!take_segue(&st), "nothing happened yet");
        st.core.segues.fetch_add(1, Ordering::Release); // the audio thread handed over
        let s2 = st.clone();
        assert!(finishes(move || take_segue(&s2)));
        assert_eq!(st.song.lock().unwrap().as_ref().unwrap().folder, b);
        assert_eq!(st.settings.lock().unwrap().current, Some(1));
        assert!(st.next.lock().unwrap().loaded.is_none());
        assert!(!take_segue(&st), "applied once");
        let s3 = st.clone();
        let v = finishes(move || view(&s3));
        assert_eq!(v.song.unwrap().name, "Second");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn app_state_never_deadlocks() {
        let st = Arc::new(state());
        // with and without a song, pad on and off
        for on in [false, true] {
            *st.pad_on.lock().unwrap() = on;
            let s2 = st.clone();
            let v = finishes(move || view(&s2));
            assert!(v.song.is_none());
            let s3 = st.clone();
            finishes(move || pad_update(&s3).ok());
        }
        // with a song loaded
        let folder = std::env::temp_dir().join(format!("rig-state-song-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let mut f = SongFile { name: "S".into(), bpm: 120.0, beats_per_bar: 4, key: "A".into(), markers: vec![songfile::Marker { name: "Top".into(), bar: 1 }], ..Default::default() };
        f.stems = vec![];
        let (song, arr) = f.build(48_000, 48_000 * 10).unwrap();
        *st.song.lock().unwrap() = Some(Loaded { folder, file: f, song, arr, stems: vec![], sample_rate: 48_000, length: 48_000 * 10, peaks: vec![] });
        let s4 = st.clone();
        let v = finishes(move || view(&s4));
        assert_eq!(v.pads.song_key.as_deref(), Some("A"));
        assert!(v.song.is_some());
    }
}

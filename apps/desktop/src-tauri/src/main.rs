// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod decode;
mod guide;
mod midi_io;
mod model;
mod songfile;

use audio::{OpenOutput, Output, OutputInfo, ToAudio};
use rig_engine::midi::{Action, Binding, Event, MidiMap};
use rig_engine::{Arrangement, Command, Engine, Quantize, Route, Snapshot, Song, StemAudio};
use serde::{Deserialize, Serialize};
use songfile::{Block, Marker, SongFile};
use std::path::{Path, PathBuf};
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
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            output: None,
            midi: None,
            setlist: vec![],
            current: None,
            quantize: Quantize::Bar,
            count_in: true,
            click_gain: 0.8,
            click_route: None,
        }
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
    let out = audio::open(name, core.status.clone(), move |e| {
        let _ = h.emit("audio-error", e);
    })?;
    let info = out.info.clone();
    *core.output.lock().unwrap() = Some(out);
    Ok(info)
}

/* ---------------- songs ---------------- */

fn load(folder: &Path, sample_rate: u32, channels: usize) -> Result<Loaded, String> {
    let file = SongFile::scan(folder, channels)?;
    let results: Vec<Result<StemAudio, String>> = std::thread::scope(|s| {
        let hs: Vec<_> = file
            .stems
            .iter()
            .map(|st| {
                let p = folder.join(&st.file);
                s.spawn(move || decode::decode(&p, sample_rate))
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("A stem failed to load.".into()))).collect()
    });
    let stems = results.into_iter().collect::<Result<Vec<_>, _>>()?;
    let length = stems.iter().map(|s| s.frames()).max().unwrap_or(0);
    let (song, arr) = file.build(sample_rate, length)?;
    let peaks = stems.iter().map(|s| peaks(s, sample_rate)).collect();
    Ok(Loaded { folder: folder.to_path_buf(), file, song, arr, stems, sample_rate, length, peaks })
}

/// Builds a fresh engine for the loaded song and hands it to the audio thread.
fn install(core: &Core, settings: &Settings, l: &Loaded) -> Result<(), String> {
    let mut out = core.output.lock().unwrap();
    let o = out.as_mut().ok_or("No audio output is open. Pick one at the top of the window.")?;
    if o.info.sample_rate != l.sample_rate {
        return Err("The audio output changed sample rate. Reload the song.".into());
    }
    let ch = o.info.channels;
    let mut e = Engine::new(l.song.clone(), l.arr.clone(), l.stems.clone(), ch)?;
    e.handle(Command::SetQuantize(settings.quantize));
    e.handle(Command::SetCountIn(settings.count_in));
    e.handle(Command::SetClick(l.file.click));
    e.handle(Command::SetClickGain(settings.click_gain));
    e.handle(Command::SetClickRoute(settings.click_route.unwrap_or(songfile::default_route("click", ch))));
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
}

impl SongView {
    fn from(l: &Loaded) -> Self {
        let bar = l.song.frames_per_bar();
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
                    bar: ((s.start as f64 - l.song.offset as f64) / bar).round() as i64 + 1,
                    bars: s.len() as f64 / bar,
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
                })
                .collect(),
            has_click: l.file.stems.iter().any(|s| songfile::is_click(&s.name)),
            has_guide: l.file.stems.iter().any(|s| songfile::is_guide(&s.name)),
            needs_setup: l.file.markers_from.is_none(),
        }
    }
}

#[derive(Serialize)]
struct SetItem {
    name: String,
    path: String,
    found: bool,
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
}

fn view(state: &AppState) -> AppView {
    let s = state.settings.lock().unwrap().clone();
    let output = state.core.output.lock().unwrap().as_ref().map(|o| o.info.clone());
    let ch = output.as_ref().map_or(2, |o| o.channels);
    let (bindings, learning) = {
        let map = state.core.map.lock().unwrap();
        (map.bindings.clone(), map.is_learning())
    };
    AppView {
        outputs: audio::list_outputs(),
        output,
        midi_inputs: midi_io::inputs(),
        midi: state.midi.lock().unwrap().as_ref().map(|m| m.name.clone()),
        setlist: s
            .setlist
            .iter()
            .map(|p| {
                let found = p.is_dir();
                let name = std::fs::read_to_string(SongFile::path(p))
                    .ok()
                    .and_then(|t| serde_json::from_str::<SongFile>(&t).ok())
                    .map(|f| f.name)
                    .unwrap_or_else(|| p.file_name().and_then(|n| n.to_str()).unwrap_or("Song").to_string());
                SetItem { name, path: p.display().to_string(), found }
            })
            .collect(),
        current: s.current,
        song: state.song.lock().unwrap().as_ref().map(SongView::from),
        quantize: s.quantize,
        count_in: s.count_in,
        click_gain: s.click_gain,
        click_route: s.click_route.unwrap_or(songfile::default_route("click", ch)),
        bindings,
        learning,
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
}

#[tauri::command]
fn status(state: State<'_, AppState>) -> Status {
    if let Some(o) = state.core.output.lock().unwrap().as_mut() {
        o.collect_garbage();
    }
    Status { snapshot: state.core.snapshot(), learning: state.core.map.lock().unwrap().is_learning() }
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
    reload_current(&state).await?;
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
    let loaded = tauri::async_runtime::spawn_blocking(move || load(&folder, sr, ch)).await.map_err(|e| e.to_string())??;
    let settings = state.settings.lock().unwrap().clone();
    install(&state.core, &settings, &loaded)?;
    if !SongFile::path(&loaded.folder).exists() {
        let _ = loaded.file.save(&loaded.folder);
    }
    *state.song.lock().unwrap() = Some(loaded);
    state.settings.lock().unwrap().current = Some(index);
    state.save_settings();
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
    Quantize { q: Quantize },
    CountIn { on: bool },
    Click { on: bool },
    ClickGain { gain: f32 },
    ClickRoute { route: Route },
    StemGain { stem: usize, gain: f32 },
    StemMute { stem: usize, muted: bool },
    /// Mute without saving (used for solo).
    StemMuteLive { stem: usize, muted: bool },
    StemRoute { stem: usize, route: Route },
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
            UiCmd::Goto { block } => {
                core.send(Command::Goto(block));
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
                core.send(Command::SetStemGain(stem, gain));
            }
            UiCmd::StemMute { stem, muted } => {
                if let Some(st) = song.as_mut().and_then(|l| l.file.stems.get_mut(stem)) {
                    st.muted = muted;
                    save_song = true;
                }
                core.send(Command::SetStemMute(stem, muted));
            }
            UiCmd::StemMuteLive { stem, muted } => {
                core.send(Command::SetStemMute(stem, muted));
            }
            UiCmd::StemRoute { stem, route } => {
                if let Some(st) = song.as_mut().and_then(|l| l.file.stems.get_mut(stem)) {
                    st.route = route;
                    save_song = true;
                }
                core.send(Command::SetStemRoute(stem, route));
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
        let (s, a) = file.build(l.sample_rate, l.length)?;
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
        f.bpm = meta.bpm.clamp(30.0, 300.0);
        f.beats_per_bar = meta.beats_per_bar.clamp(1, 12);
        f.offset_ms = meta.offset_ms.max(0.0);
        f.key = meta.key.trim().to_string();
        let mut m = meta.markers;
        m.retain(|x| !x.name.trim().is_empty());
        m.sort_by_key(|x| x.bar);
        f.markers = m;
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
    let (click, guide, sr, bpb, bpm) = {
        let song = state.song.lock().unwrap();
        let l = song.as_ref().ok_or("Load a song first.")?;
        let find = |f: fn(&str) -> bool| l.file.stems.iter().position(|s| f(&s.name)).map(|i| l.stems[i].clone());
        (find(songfile::is_click), find(songfile::is_guide), l.sample_rate, l.file.beats_per_bar, l.file.bpm)
    };
    if click.is_none() && guide.is_none() {
        return Err("This song has no Click or Guide stem to read. Add sections by hand in the Song panel.".into());
    }
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
        guide::analyze(click.as_ref(), guide.as_ref(), sr, bpb, bpm, model.as_deref(), |s, p| emit(s, p))
    })
    .await
    .map_err(|e| e.to_string())??;
    let found_tempo = analysis.bpm.is_some();
    let sections = analysis.markers.len();
    let view = edit_song(&state, |f| {
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
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            std::fs::create_dir_all(&dir)?;
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let settings: Settings = read_json(&dir.join("settings.json"));
            let map: MidiMap = read_json(&dir.join("midi.json"));
            let core = Arc::new(Core {
                output: Mutex::new(None),
                status: Arc::new(Mutex::new(None)),
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
            app.manage(AppState { core, song: Mutex::new(None), settings: Mutex::new(settings), midi: Mutex::new(midi) });
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
            song_peaks,
            analyze_song,
            check_update,
            install_update,
            app_version,
            midi_learn,
            midi_cancel,
            midi_unbind
        ])
        .run(tauri::generate_context!())
        .expect("error while running Playback Rig");
}

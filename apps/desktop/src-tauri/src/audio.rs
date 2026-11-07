//! The audio interface. The cpal stream lives on its own thread (it isn't Send on
//! macOS). The engine lives inside the audio callback; everything reaches it
//! through lock-free queues, and replaced engines are sent back out to be freed
//! off the audio thread.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rig_engine::{Arrangement, Command, Engine, Snapshot};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};

pub enum ToAudio {
    Cmd(Command),
    Swap(Box<Engine>),
    /// A new arrangement while playing, and the new index of the block playing now.
    Arrange(Box<Arrangement>, usize),
    /// Crossfade to this pad.
    Pad(Box<crate::pads::PadVoice>),
    PadOn(bool),
    PadLevel(f32),
    PadRoute(rig_engine::Route),
    /// The next song, ready to start by itself (see `Handoff`).
    Queue(Box<Engine>, Handoff),
    /// Forget the queued next song.
    Unqueue,
    /// The free click: on/off, BPM, beats per bar, level, output.
    Metro { on: bool, bpm: f64, beats_per_bar: u32, gain: f32, route: rig_engine::Route },
}

/// Audio played alongside a song change: the reverb wash over the seam, or the spoken
/// cue for where it's going. It starts `lead` frames before the change.
pub struct Overlay {
    pub audio: rig_engine::StemAudio,
    pub route: rig_engine::Route,
    pub lead: u64,
    pub gain: f32,
}

/// How the queued next song takes over.
#[derive(Default)]
pub struct Handoff {
    /// Start the next song this many frames before the current one ends (0 = right after).
    pub overlap: u64,
    /// Fade the current song out over this many frames once the next one starts.
    pub fade: u64,
    /// The current song's click and guide stems, muted at the handoff so the in-ears
    /// only hear the new song's click.
    pub cues: [Option<usize>; 2],
    /// Take over at the current song's next bar line instead of at its end (cutting to
    /// another song mid-song); `fade` is then the blend.
    pub at_bar: bool,
    /// A smooth cut (with `at_bar`): the current song stops on the bar line and its
    /// sustained parts ring on for this many frames, instead of a fade.
    pub ring: u64,
    /// Played around the change (see `Overlay`).
    pub overlays: [Option<Box<Overlay>>; 2],
}

/// Things the audio thread hands back to be freed elsewhere. Only ever dropped.
/// The playing song, the next one waiting to take over, and the last one still fading
/// out. Lives on the audio thread; nothing here allocates after `new`.
pub struct Decks {
    pub engine: Option<Box<Engine>>,
    pub queued: Option<(Box<Engine>, Handoff)>,
    pub outgoing: Option<Box<Engine>>,
    scratch: Vec<f32>,
    /// Overlays playing now, and the frame each is at (negative = starts later in the buffer).
    overlays: [Option<(Box<Overlay>, i64)>; 2],
}

impl Decks {
    pub fn new(channels: usize) -> Self {
        Decks { engine: None, queued: None, outgoing: None, scratch: vec![0f32; 16384 * channels.max(1)], overlays: [None, None] }
    }

    /// Starts the queued song if the current one ends within this buffer (or within
    /// the crossfade). Returns true on a handoff; replaced engines go to `free`.
    pub fn handoff(&mut self, frames: u64, mut free: impl FnMut(Garbage)) -> bool {
        // Frames until the change, for the overlays that lead in before it.
        let to_change = match (self.engine.as_ref(), self.queued.as_ref()) {
            (Some(e), Some((_, h))) if h.at_bar => e.frames_to_bar().map(|b| e.frames_left().map_or(b, |l| l.min(b))),
            (Some(e), Some((_, h))) => e.frames_left().map(|l| l.saturating_sub(h.overlap)),
            _ => None,
        };
        if let (Some(tc), Some((_, h))) = (to_change, self.queued.as_mut()) {
            for slot in h.overlays.iter_mut() {
                if slot.as_ref().is_some_and(|o| tc <= o.lead + frames) {
                    let o = slot.take().unwrap();
                    let at = o.lead as i64 - tc as i64;
                    let free_slot = self.overlays.iter().position(|x| x.is_none()).unwrap_or(0);
                    if let Some((old, _)) = self.overlays[free_slot].replace((o, at)) {
                        free(Garbage::Overlay(old));
                    }
                }
            }
        }
        // The next song starts `overlap` frames before the current one ends (or on the
        // next bar line for a cut): in this buffer, after `delay` frames, so the handoff
        // lands on the exact frame.
        let delay = match (self.engine.as_ref(), self.queued.as_ref()) {
            (Some(e), Some((_, h))) if h.at_bar => match (e.frames_to_bar(), e.frames_left()) {
                // the next bar line, or the song's end if that comes first
                (Some(b), left) => match left.map_or(b, |l| l.min(b)) {
                    d if d <= frames => d,
                    _ => return false,
                },
                _ => return false,
            },
            (Some(e), Some((_, h))) => match e.frames_left() {
                Some(left) if left <= h.overlap + frames => left.saturating_sub(h.overlap),
                _ => return false,
            },
            _ => return false,
        };
        let Some((mut next, mut h)) = self.queued.take() else { return false };
        for o in h.overlays.iter_mut().filter_map(|o| o.take()) {
            free(Garbage::Overlay(o)); // too late to start (it led in from before the change)
        }
        next.handle(Command::PlayAfter(delay));
        if let Some(mut old) = self.engine.replace(next) {
            old.handle(Command::SetClick(false));
            for i in h.cues.into_iter().flatten() {
                old.handle(Command::SetStemMute(i, true));
            }
            if h.at_bar && h.ring > 0 {
                // A smooth cut: on the bar line, rhythm and vocals stop, the rest rings on.
                old.handle(Command::RingOutAt(delay, h.ring));
            } else if h.at_bar {
                // A cut: full to the bar line, then it fades under the new song.
                old.handle(Command::FadeOutAt(delay, h.fade.max(1)));
            } else if h.fade > 0 {
                // Starts at this buffer, ends exactly where the song would have.
                old.handle(Command::FadeOut(h.fade + delay));
            }
            if let Some(prev) = self.outgoing.replace(old) {
                free(Garbage::Engine(prev));
            }
        }
        true
    }

    /// Renders the current song, plus the outgoing one while it rings out, plus overlays.
    pub fn process(&mut self, data: &mut [f32], mut free: impl FnMut(Garbage)) {
        match self.engine.as_mut() {
            Some(e) => e.process(data),
            None => data.fill(0.0),
        }
        if let Some(o) = self.outgoing.as_mut() {
            if o.is_sounding() && data.len() <= self.scratch.len() {
                let buf = &mut self.scratch[..data.len()];
                o.process(buf);
                for (d, v) in data.iter_mut().zip(buf.iter()) {
                    *d += *v;
                }
            } else if let Some(done) = self.outgoing.take() {
                free(Garbage::Engine(done));
            }
        }
        let ch = self.engine.as_ref().or(self.outgoing.as_ref()).map_or(2, |e| e.channels());
        let frames = data.len() / ch.max(1);
        for slot in self.overlays.iter_mut() {
            let Some((o, at)) = slot.as_mut() else { continue };
            let len = o.audio.frames() as i64;
            for f in 0..frames {
                let p = *at + f as i64;
                if p >= 0 && p < len {
                    let (l, r) = o.audio.frame(p as u64);
                    Engine::write(data, f * ch, ch, o.route, l * o.gain, r * o.gain);
                }
            }
            *at += frames as i64;
            if *at >= len {
                if let Some((done, _)) = slot.take() {
                    free(Garbage::Overlay(done));
                }
            }
        }
    }
}

#[allow(dead_code)]
pub enum Garbage {
    Engine(Box<Engine>),
    Arrangement(Box<Arrangement>),
    Pad(Box<crate::pads::PadVoice>),
    Overlay(Box<Overlay>),
}

#[derive(Clone, Debug, Serialize)]
pub struct OutputInfo {
    pub name: String,
    pub channels: usize,
    pub is_default: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct OpenOutput {
    pub name: String,
    pub channels: usize,
    pub sample_rate: u32,
}

pub struct Output {
    pub info: OpenOutput,
    pub tx: rtrb::Producer<ToAudio>,
    garbage: rtrb::Consumer<Garbage>,
    stop: mpsc::Sender<()>,
}

impl Output {
    pub fn send(&mut self, m: ToAudio) -> bool {
        self.tx.push(m).is_ok()
    }
    /// Frees engines the audio thread has finished with.
    pub fn collect_garbage(&mut self) {
        while self.garbage.pop().is_ok() {}
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

fn f32_channels(dev: &cpal::Device) -> usize {
    dev.supported_output_configs()
        .map(|cs| cs.filter(|c| c.sample_format() == cpal::SampleFormat::F32).map(|c| c.channels() as usize).max().unwrap_or(0))
        .unwrap_or(0)
}

pub fn list_outputs() -> Vec<OutputInfo> {
    let host = cpal::default_host();
    let default = host.default_output_device().and_then(|d| d.name().ok());
    let Ok(devs) = host.output_devices() else { return vec![] };
    devs.filter_map(|d| {
        let name = d.name().ok()?;
        let channels = f32_channels(&d);
        (channels > 0).then(|| OutputInfo { is_default: Some(&name) == default.as_ref(), name, channels })
    })
    .collect()
}

/// Opens an output (by name, or the system default) using all of its channels.
pub fn open(name: Option<String>, status: Arc<Mutex<Option<Snapshot>>>, segues: Arc<AtomicU64>, metro_on: Arc<std::sync::atomic::AtomicBool>, levels: Arc<Mutex<[f32; 64]>>, master: Arc<std::sync::atomic::AtomicU32>, on_error: impl Fn(String, bool) + Send + 'static) -> Result<Output, String> {
    let (tx, mut rx) = rtrb::RingBuffer::<ToAudio>::new(1024);
    let (mut gtx, garbage) = rtrb::RingBuffer::<Garbage>::new(32);
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<OpenOutput, String>>();

    std::thread::Builder::new()
        .name("audio-output".into())
        .spawn(move || {
            let host = cpal::default_host();
            let dev = match &name {
                Some(n) => host.output_devices().ok().and_then(|mut ds| ds.find(|d| d.name().ok().as_deref() == Some(n.as_str()))),
                None => host.default_output_device(),
            };
            let Some(dev) = dev else {
                let _ = ready_tx.send(Err(match name {
                    Some(n) => format!("\"{n}\" isn't connected. Plug it in, or pick another output."),
                    None => "No audio output found.".into(),
                }));
                return;
            };
            let dev_name = dev.name().unwrap_or_else(|_| "Audio output".into());
            let configs: Vec<_> = match dev.supported_output_configs() {
                Ok(c) => c.filter(|c| c.sample_format() == cpal::SampleFormat::F32).collect(),
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("{dev_name}: {e}")));
                    return;
                }
            };
            let max_ch = configs.iter().map(|c| c.channels()).max().unwrap_or(0);
            let pick = configs
                .into_iter()
                .filter(|c| c.channels() == max_ch)
                .find_map(|c| [48_000, 44_100].into_iter().find_map(|sr| c.clone().try_with_sample_rate(cpal::SampleRate(sr))).or_else(|| Some(c.with_max_sample_rate())));
            let Some(cfg) = pick else {
                let _ = ready_tx.send(Err(format!("{dev_name} has no 32-bit float output mode.")));
                return;
            };
            let config: cpal::StreamConfig = cfg.config();
            let channels = config.channels as usize;
            let sample_rate = config.sample_rate.0;

            let mut decks = Decks::new(channels);
            let mut metro = crate::metronome::Metronome::new(sample_rate);
            let mut pads = crate::pads::PadPlayer::new(sample_rate);
            let mut out_level = Master::new(sample_rate);
            let stream = dev.build_output_stream(
                &config,
                move |data: &mut [f32], _| {
                    while let Ok(m) = rx.pop() {
                        match m {
                            ToAudio::Cmd(c) => {
                                if let Some(e) = decks.engine.as_mut() {
                                    e.handle(c)
                                }
                            }
                            ToAudio::Swap(e) => {
                                if let Some(old) = decks.engine.replace(e) {
                                    let _ = gtx.push(Garbage::Engine(old));
                                }
                            }
                            ToAudio::Arrange(mut arr, keep) => {
                                if let Some(e) = decks.engine.as_mut() {
                                    e.swap_arrangement(&mut arr, keep);
                                }
                                let _ = gtx.push(Garbage::Arrangement(arr));
                            }
                            ToAudio::Pad(v) => {
                                if let Some(old) = pads.swap(v) {
                                    let _ = gtx.push(Garbage::Pad(old));
                                }
                            }
                            ToAudio::PadOn(on) => pads.on = on,
                            ToAudio::PadLevel(l) => pads.level = l.clamp(0.0, 2.0),
                            ToAudio::PadRoute(r) => pads.route = r,
                            ToAudio::Queue(e, h) => {
                                if let Some((old, mut oh)) = decks.queued.replace((e, h)) {
                                    let _ = gtx.push(Garbage::Engine(old));
                                    for o in oh.overlays.iter_mut().filter_map(|o| o.take()) {
                                        let _ = gtx.push(Garbage::Overlay(o));
                                    }
                                }
                            }
                            ToAudio::Metro { on, bpm, beats_per_bar, gain, route } => {
                                metro.gain = gain;
                                metro.route = route;
                                metro.set(on, bpm, beats_per_bar);
                                metro_on.store(metro.on, Ordering::Release);
                            }
                            ToAudio::Unqueue => {
                                if let Some((old, mut oh)) = decks.queued.take() {
                                    let _ = gtx.push(Garbage::Engine(old));
                                    for o in oh.overlays.iter_mut().filter_map(|o| o.take()) {
                                        let _ = gtx.push(Garbage::Overlay(o));
                                    }
                                }
                            }
                        }
                    }
                    // Hand over to the next song when the current one is about to end.
                    let frames = (data.len() / channels.max(1)) as u64;
                    if decks.handoff(frames, |g| {
                        let _ = gtx.push(g);
                    }) {
                        segues.fetch_add(1, Ordering::Release);
                    }
                    decks.process(data, |g| {
                        let _ = gtx.push(g);
                    });
                    if let Some(e) = decks.engine.as_ref() {
                        if let Ok(mut st) = status.try_lock() {
                            *st = Some(e.snapshot());
                        }
                        // meters: the loudest each stem got since the screen last looked
                        if let Ok(mut lv) = levels.try_lock() {
                            for (l, p) in lv[..MASTER_SLOT].iter_mut().zip(e.peaks()) {
                                if *p > *l {
                                    *l = *p;
                                }
                            }
                        }
                    }
                    // The free click stops when a song starts.
                    if metro.on && decks.engine.as_ref().is_some_and(|e| e.is_playing()) {
                        metro.on = false;
                        metro_on.store(false, Ordering::Release);
                    }
                    metro.process(data, channels);
                    if let Some(done) = pads.process(data, channels) {
                        let _ = gtx.push(Garbage::Pad(done));
                    }
                    // The master fader: last, on everything that goes out.
                    let pk = out_level.process(data, channels, f32::from_bits(master.load(Ordering::Relaxed)));
                    if let Ok(mut lv) = levels.try_lock() {
                        if pk > lv[MASTER_SLOT] {
                            lv[MASTER_SLOT] = pk;
                        }
                    }
                },
                move |err| {
                    let gone = matches!(err, cpal::StreamError::DeviceNotAvailable);
                    on_error(err.to_string(), gone)
                },
                None,
            );
            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("{dev_name}: couldn't start ({e})")));
                    return;
                }
            };
            if let Err(e) = stream.play() {
                let _ = ready_tx.send(Err(format!("{dev_name}: couldn't start ({e})")));
                return;
            }
            let _ = ready_tx.send(Ok(OpenOutput { name: dev_name, channels, sample_rate }));
            let _ = stop_rx.recv(); // until the Output is dropped
            drop(stream);
        })
        .map_err(|e| e.to_string())?;

    let info = ready_rx.recv().map_err(|_| "The audio thread stopped unexpectedly.".to_string())??;
    Ok(Output { info, tx, garbage, stop: stop_tx })
}

/// Where the master meter's peak is kept in the shared levels (stems use the slots before it).
pub const MASTER_SLOT: usize = 63;

/// The master fader: one level on everything the app sends out (songs, built-in click,
/// pads). Moves are ramped over 30 ms so a fast fader move never clicks.
pub struct Master {
    cur: f32,
    step: f32,
}

impl Master {
    pub fn new(sample_rate: u32) -> Self {
        Master { cur: 1.0, step: 1.0 / (0.03 * sample_rate as f32).max(1.0) }
    }

    /// Scales `data` (interleaved) toward `target`; returns the loudest sample afterwards.
    pub fn process(&mut self, data: &mut [f32], channels: usize, target: f32) -> f32 {
        let target = if target.is_finite() { target.clamp(0.0, 2.0) } else { 1.0 };
        let mut peak = 0f32;
        for frame in data.chunks_mut(channels.max(1)) {
            if self.cur < target {
                self.cur = (self.cur + self.step).min(target);
            } else if self.cur > target {
                self.cur = (self.cur - self.step).max(target);
            }
            for s in frame.iter_mut() {
                *s *= self.cur;
                peak = peak.max(s.abs());
            }
        }
        peak
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig_engine::{Route, Section, Song, StemAudio, StemInfo};

    const CH: usize = 2;

    /// A one-stem song of `bars` 1-second bars (SR 1000, 240 BPM) whose every sample is `v`.
    /// Stem 0 is the music; stem 1 is a "Click Track" stem (value 1000*v) on output 2.
    fn song(v: f32, bars: u64) -> Engine {
        let len = bars * 1000;
        let s = Song {
            name: format!("Song {v}"),
            sample_rate: 1000,
            bpm: 240.0,
            beats_per_bar: 4,
            offset: 0,
            length: len,
            sections: vec![Section { name: "All".into(), start: 0, end: len }],
            stems: vec![
                StemInfo { name: "Music".into(), route: Route::Mono(0), gain: 1.0, muted: false },
                StemInfo { name: "Click Track".into(), route: Route::Mono(1), gain: 1.0, muted: false },
            ],
            guide_stem: None,
            calls: vec![],
            beat_frames: vec![],
        };
        let arr = Arrangement::linear(&s);
        let audio = vec![StemAudio::mono(vec![v; len as usize]), StemAudio::mono(vec![v * 1000.0; len as usize])];
        let mut e = Engine::new(s, arr, audio, CH).unwrap();
        e.handle(Command::SetCountIn(false));
        e.handle(Command::SetClick(false));
        e
    }

    /// Runs the decks for `frames` in 256-frame buffers like the audio callback; returns
    /// (output 1, output 2) and how many handoffs happened.
    fn run(d: &mut Decks, frames: usize) -> (Vec<f32>, Vec<f32>, u32) {
        let (mut a, mut b, mut n) = (vec![], vec![], 0);
        let mut buf = vec![0f32; 256 * CH];
        let mut done = 0;
        while done < frames {
            let k = (frames - done).min(256);
            if d.handoff(k as u64, drop) {
                n += 1;
            }
            d.process(&mut buf[..k * CH], drop);
            for f in 0..k {
                a.push(buf[f * CH]);
                b.push(buf[f * CH + 1]);
            }
            done += k;
        }
        (a, b, n)
    }

    #[test]
    fn a_segue_starts_the_next_song_on_the_exact_frame_after_the_last() {
        let mut d = Decks::new(CH);
        d.engine = Some(Box::new(song(1.0, 4)));
        d.queued = Some((Box::new(song(2.0, 4)), Handoff { overlap: 0, fade: 0, cues: [Some(1), None], at_bar: false, ring: 0, overlays: [None, None] }));
        d.engine.as_mut().unwrap().handle(Command::Play);
        let (a, _, n) = run(&mut d, 6000);
        assert_eq!(n, 1);
        assert_eq!(a[3990], 1.0, "the first song to its last frame");
        assert_eq!(a[3999], 1.0);
        assert!(a[4000] > 0.0 && a[4000] < 2.0, "the next one starts exactly after (with its 5 ms fade-in)");
        assert_eq!(a[4100], 2.0);
        assert_eq!(d.engine.as_ref().unwrap().song().name, "Song 2");
    }

    #[test]
    fn an_overlay_starts_its_lead_before_the_change_and_plays_through_it() {
        let mut d = Decks::new(CH);
        d.engine = Some(Box::new(song(1.0, 4)));
        // The wash: 1000 frames of 100.0, starting 500 frames before the segue at 4000.
        let wash = Overlay { audio: StemAudio::mono(vec![100.0; 1000]), route: Route::Mono(0), lead: 500, gain: 1.0 };
        d.queued = Some((Box::new(song(2.0, 4)), Handoff { cues: [Some(1), None], overlays: [Some(Box::new(wash)), None], ..Default::default() }));
        d.engine.as_mut().unwrap().handle(Command::Play);
        let (a, _, n) = run(&mut d, 6000);
        assert_eq!(n, 1);
        assert_eq!(a[3499], 1.0, "not yet");
        assert_eq!(a[3500], 101.0, "starts exactly 500 frames before the change");
        assert_eq!(a[4400], 102.0, "plays on under the new song");
        assert_eq!(a[4600], 2.0, "and ends");
        assert!(d.overlays.iter().all(|o| o.is_none()), "handed back once done");
    }

    #[test]
    fn a_smooth_cut_rings_out_the_last_song_on_the_bar() {
        let mut d = Decks::new(CH);
        d.engine = Some(Box::new(song(1.0, 8)));
        d.engine.as_mut().unwrap().handle(Command::Play);
        let _ = run(&mut d, 1500);
        d.queued = Some((Box::new(song(2.0, 8)), Handoff { cues: [Some(1), None], at_bar: true, ring: 400, ..Default::default() }));
        let (a, b, n) = run(&mut d, 1500); // frames 1500..2999; the bar line is at 2000
        assert_eq!(n, 1);
        assert_eq!(a[400], 1.0, "the first song up to the bar line");
        assert!((b[600] - 2000.0).abs() < 1.0, "only the new song's click: {}", b[600]);
        assert!(d.outgoing.is_none() || d.outgoing.as_ref().is_some_and(|o| !o.snapshot().playing), "the last song has stopped (ringing at most)");
        assert!((a[1200] - 2.0).abs() < 0.05, "then just the new song: {}", a[1200]);
    }

    #[test]
    fn a_cut_to_another_song_takes_over_on_the_next_bar_and_blends() {
        let mut d = Decks::new(CH);
        d.engine = Some(Box::new(song(1.0, 8)));
        d.engine.as_mut().unwrap().handle(Command::Play);
        let _ = run(&mut d, 1500); // halfway through bar 2
        // Cut to song 2: it takes over at the next bar line (frame 2000) and the first
        // song fades out under it over half a bar.
        d.queued = Some((Box::new(song(2.0, 8)), Handoff { overlap: 0, fade: 500, cues: [Some(1), None], at_bar: true, ring: 0, overlays: [None, None] }));
        let (a, b, n) = run(&mut d, 1500); // frames 1500..2999
        assert_eq!(n, 1);
        assert_eq!(a[400], 1.0, "the first song up to the bar line");
        assert!((a[500 + 250] - 2.5).abs() < 0.1, "blending: new at full, old at half: {}", a[750]);
        assert!((b[750] - 2000.0).abs() < 1.0, "only the new song's click: {}", b[750]);
        assert_eq!(a[1200], 2.0, "then just the new song");
    }

        #[test]
    fn a_crossfade_overlaps_and_fades_the_last_song_and_mutes_its_click() {
        let mut d = Decks::new(CH);
        d.engine = Some(Box::new(song(1.0, 8)));
        // Two-bar crossfade: the next song starts 2000 frames before the end.
        d.queued = Some((Box::new(song(2.0, 8)), Handoff { overlap: 2000, fade: 2000, cues: [Some(1), None], at_bar: false, ring: 0, overlays: [None, None] }));
        d.engine.as_mut().unwrap().handle(Command::Play);
        let (a, b, n) = run(&mut d, 9000);
        assert_eq!(n, 1);
        assert!((a[5800] - 1.0).abs() < 1e-6, "before the crossfade: {}", a[5800]);
        assert!(a[5999] > 0.94, "the fade starts where the crossfade does: {}", a[5999]);
        assert!(b[5800] == 1000.0, "old click before");
        // Halfway: old song at about half, new song at full.
        assert!((a[7000] - 2.5).abs() < 0.1, "mid-crossfade: {}", a[7000]);
        assert!((b[7000] - 2000.0).abs() < 1.0, "only the new song's click in the ears: {}", b[7000]);
        assert_eq!(a[8500], 2.0, "after the crossfade, just the new song");
        assert!(d.outgoing.is_none(), "the old song is freed once it's silent");
    }

    #[test]
    fn stopping_never_starts_the_next_song() {
        let mut d = Decks::new(CH);
        d.engine = Some(Box::new(song(1.0, 4)));
        d.queued = Some((Box::new(song(2.0, 4)), Handoff::default()));
        d.engine.as_mut().unwrap().handle(Command::Play);
        run(&mut d, 1000);
        d.engine.as_mut().unwrap().handle(Command::Stop);
        let (a, _, n) = run(&mut d, 5000);
        assert_eq!(n, 0);
        assert!(a.iter().skip(100).all(|&v| v == 0.0));
        assert!(d.queued.is_some(), "still waiting for the song to really end");
    }

    #[test]
    fn vamping_holds_the_next_song() {
        let mut d = Decks::new(CH);
        d.engine = Some(Box::new(song(1.0, 2)));
        d.queued = Some((Box::new(song(2.0, 2)), Handoff::default()));
        d.engine.as_mut().unwrap().handle(Command::Play);
        d.engine.as_mut().unwrap().handle(Command::ToggleVamp);
        let (a, _, n) = run(&mut d, 5000);
        assert_eq!(n, 0);
        assert_eq!(a[4500], 1.0, "still looping the first song");
    }

    #[test]
    fn the_master_fader_ramps_to_its_level_without_a_jump() {
        // SR 1000: the ramp takes 30 frames for a full 0..1 move.
        let mut m = Master::new(1000);
        let mut data = vec![1.0f32; 100 * CH];
        let pk = m.process(&mut data, CH, 0.5);
        // both channels of a frame get the same gain
        assert_eq!(data[0], data[1]);
        // no step: the first frame barely moves, and each frame moves at most one ramp step
        assert!(data[0] > 0.96, "first frame {}", data[0]);
        for f in 1..100 {
            assert!((data[f * CH] - data[(f - 1) * CH]).abs() <= 1.0 / 30.0 + 1e-6);
        }
        // settled at half after 15 frames, and stays there
        for f in 16..100 {
            assert!((data[f * CH] - 0.5).abs() < 1e-6, "frame {f}: {}", data[f * CH]);
        }
        assert!((pk - data[0]).abs() < 1e-6, "peak is the loudest sample out");
        // all the way down: silence; and back to unity
        let mut data = vec![1.0f32; 100 * CH];
        m.process(&mut data, CH, 0.0);
        assert_eq!(data[99 * CH], 0.0);
        let mut data = vec![0.8f32; 100 * CH];
        m.process(&mut data, CH, 1.0);
        assert!((data[99 * CH] - 0.8).abs() < 1e-6);
        // a broken value never mutes the band
        let mut data = vec![0.8f32; 100 * CH];
        m.process(&mut data, CH, f32::NAN);
        assert!((data[99 * CH] - 0.8).abs() < 1e-6);
    }
}

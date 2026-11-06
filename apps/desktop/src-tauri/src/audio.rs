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

/// How the queued next song takes over.
#[derive(Clone, Copy, Debug, Default)]
pub struct Handoff {
    /// Start the next song this many frames before the current one ends (0 = right after).
    pub overlap: u64,
    /// Fade the current song out over this many frames once the next one starts.
    pub fade: u64,
    /// The current song's click and guide stems, muted at the handoff so the in-ears
    /// only hear the new song's click.
    pub cues: [Option<usize>; 2],
}

/// Things the audio thread hands back to be freed elsewhere. Only ever dropped.
/// The playing song, the next one waiting to take over, and the last one still fading
/// out. Lives on the audio thread; nothing here allocates after `new`.
pub struct Decks {
    pub engine: Option<Box<Engine>>,
    pub queued: Option<(Box<Engine>, Handoff)>,
    pub outgoing: Option<Box<Engine>>,
    scratch: Vec<f32>,
}

impl Decks {
    pub fn new(channels: usize) -> Self {
        Decks { engine: None, queued: None, outgoing: None, scratch: vec![0f32; 16384 * channels.max(1)] }
    }

    /// Starts the queued song if the current one ends within this buffer (or within
    /// the crossfade). Returns true on a handoff; replaced engines go to `free`.
    pub fn handoff(&mut self, frames: u64, mut free: impl FnMut(Box<Engine>)) -> bool {
        // The next song starts `overlap` frames before the current one ends: in this
        // buffer, after `delay` frames, so the handoff lands on the exact frame.
        let delay = match (self.engine.as_ref(), self.queued.as_ref()) {
            (Some(e), Some((_, h))) => match e.frames_left() {
                Some(left) if left <= h.overlap + frames => left.saturating_sub(h.overlap),
                _ => return false,
            },
            _ => return false,
        };
        let Some((mut next, h)) = self.queued.take() else { return false };
        next.handle(Command::PlayAfter(delay));
        if let Some(mut old) = self.engine.replace(next) {
            old.handle(Command::SetClick(false));
            for i in h.cues.into_iter().flatten() {
                old.handle(Command::SetStemMute(i, true));
            }
            if h.fade > 0 {
                // Starts at this buffer, ends exactly where the song would have.
                old.handle(Command::FadeOut(h.fade + delay));
            }
            if let Some(prev) = self.outgoing.replace(old) {
                free(prev);
            }
        }
        true
    }

    /// Renders the current song, plus the outgoing one while it fades.
    pub fn process(&mut self, data: &mut [f32], mut free: impl FnMut(Box<Engine>)) {
        match self.engine.as_mut() {
            Some(e) => e.process(data),
            None => data.fill(0.0),
        }
        if let Some(o) = self.outgoing.as_mut() {
            if o.is_playing() && data.len() <= self.scratch.len() {
                let buf = &mut self.scratch[..data.len()];
                o.process(buf);
                for (d, v) in data.iter_mut().zip(buf.iter()) {
                    *d += *v;
                }
            } else if let Some(done) = self.outgoing.take() {
                free(done);
            }
        }
    }
}

#[allow(dead_code)]
pub enum Garbage {
    Engine(Box<Engine>),
    Arrangement(Box<Arrangement>),
    Pad(Box<crate::pads::PadVoice>),
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
pub fn open(name: Option<String>, status: Arc<Mutex<Option<Snapshot>>>, segues: Arc<AtomicU64>, metro_on: Arc<std::sync::atomic::AtomicBool>, levels: Arc<Mutex<[f32; 64]>>, on_error: impl Fn(String, bool) + Send + 'static) -> Result<Output, String> {
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
                                if let Some((old, _)) = decks.queued.replace((e, h)) {
                                    let _ = gtx.push(Garbage::Engine(old));
                                }
                            }
                            ToAudio::Metro { on, bpm, beats_per_bar, gain, route } => {
                                metro.gain = gain;
                                metro.route = route;
                                metro.set(on, bpm, beats_per_bar);
                                metro_on.store(metro.on, Ordering::Release);
                            }
                            ToAudio::Unqueue => {
                                if let Some((old, _)) = decks.queued.take() {
                                    let _ = gtx.push(Garbage::Engine(old));
                                }
                            }
                        }
                    }
                    // Hand over to the next song when the current one is about to end.
                    let frames = (data.len() / channels.max(1)) as u64;
                    if decks.handoff(frames, |e| {
                        let _ = gtx.push(Garbage::Engine(e));
                    }) {
                        segues.fetch_add(1, Ordering::Release);
                    }
                    decks.process(data, |e| {
                        let _ = gtx.push(Garbage::Engine(e));
                    });
                    if let Some(e) = decks.engine.as_ref() {
                        if let Ok(mut st) = status.try_lock() {
                            *st = Some(e.snapshot());
                        }
                        // meters: the loudest each stem got since the screen last looked
                        if let Ok(mut lv) = levels.try_lock() {
                            for (l, p) in lv.iter_mut().zip(e.peaks()) {
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
        d.queued = Some((Box::new(song(2.0, 4)), Handoff { overlap: 0, fade: 0, cues: [Some(1), None] }));
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
    fn a_crossfade_overlaps_and_fades_the_last_song_and_mutes_its_click() {
        let mut d = Decks::new(CH);
        d.engine = Some(Box::new(song(1.0, 8)));
        // Two-bar crossfade: the next song starts 2000 frames before the end.
        d.queued = Some((Box::new(song(2.0, 8)), Handoff { overlap: 2000, fade: 2000, cues: [Some(1), None] }));
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
}

//! The playback engine. `process` runs on the audio thread: it never allocates,
//! locks or blocks. Commands come in through `handle`, which the host calls from
//! the audio thread after draining a lock-free queue.

use crate::model::{Arrangement, Route, Section, Song, StemAudio, StemInfo};
use serde::{Deserialize, Serialize};

/// When a queued jump happens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quantize {
    /// As soon as a short fade allows (about 5 ms).
    Now,
    /// On the next downbeat.
    Bar,
    /// When the current block finishes.
    SectionEnd,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Play,
    /// Fades out and stops. When already stopped, returns to the top.
    Stop,
    /// While stopped: move the playhead to the start of a block.
    Locate(usize),
    /// Move the playhead to a frame inside a block (scrubbing). While playing it
    /// jumps there immediately, with the usual short fades.
    Seek(usize, u64),
    /// Queue the next block. Pressing again while a jump is queued moves the target one further.
    Next,
    /// Play the current block again after it ends.
    RepeatCurrent,
    /// Queue a specific block.
    Goto(usize),
    CancelJump,
    /// Loop the current block until turned off.
    ToggleVamp,
    SetQuantize(Quantize),
    SetStemGain(usize, f32),
    SetStemMute(usize, bool),
    SetStemRoute(usize, Route),
    SetClick(bool),
    SetClickGain(f32),
    SetClickRoute(Route),
    SetCountIn(bool),
    /// Fades the whole song out over this many frames, then stops (song transitions).
    FadeOut(u64),
    /// Starts playing after this many frames of silence, with no count-in (a segue
    /// that has to land on an exact frame).
    PlayAfter(u64),
    /// "Fade out now" on stage: fades the tracks and the guide out over this many frames
    /// but keeps playing, so the click (click stems and the built-in click) carries on
    /// until you stop.
    FadeTracks(u64),
    /// Length (frames) of the ring-out at a jump: where the arrangement skips ahead, the
    /// music from where it left keeps sounding briefly and fades away under the new block.
    /// 0 = a straight cut.
    SetJoinTail(u64),
}

/// What the screen needs to draw, read after each `process` call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Snapshot {
    pub playing: bool,
    pub counting_in: bool,
    pub finished: bool,
    pub item: usize,
    pub section: usize,
    pub frame_in_section: u64,
    pub section_frames: u64,
    pub bar: i64,
    pub beat: u32,
    pub pending: Option<usize>,
    pub frames_to_jump: Option<u64>,
    pub vamp: bool,
    /// Bars until this block ends (follows tempo changes).
    pub bars_left: f64,
    /// Tempo at the playhead.
    pub bpm: f64,
    /// The tracks are fading or faded out ("Fade out now"); the click carries on.
    pub tracks_out: bool,
}

struct Pending {
    target: usize,
    at: u64,
    /// Frame inside the target block to start from (0 = its start).
    from: u64,
    /// When the jump was asked for (absolute frame).
    queued: u64,
}

#[derive(Default)]
struct ClickVoice {
    phase: f32,
    step: f32,
    amp: f32,
    decay: f32,
    left: u32,
}

pub struct Engine {
    song: Song,
    arr: Arrangement,
    stems: Vec<StemAudio>,
    mix: Vec<StemInfo>,
    channels: usize,
    sr: f32,
    fade: u64,
    smooth_k: f32,
    gains: Vec<f32>,
    /// Loudest sample each stem sent out during the last `process` call (for meters).
    peaks: Vec<f32>,
    quant: Quantize,
    click_on: bool,
    click_gain: f32,
    click_route: Route,
    count_in: bool,
    playing: bool,
    /// (frames left, fade length) of a fade-out that ends in a stop.
    stopping: Option<(u64, u64)>,
    count_left: u64,
    count_total: u64,
    /// Silent frames before playback starts (PlayAfter).
    wait: u64,
    item: usize,
    fpos: u64,
    abs: u64,
    pending: Option<Pending>,
    vamp: bool,
    last_jump: Option<u64>,
    last_beat: Option<i64>,
    click: ClickVoice,
    finished: bool,
    /// Stems that are a click track (kept by FadeTracks, left out of ring-outs).
    is_click: Vec<bool>,
    /// (frames left, length) of a tracks-only fade; stays at (0, n) once faded.
    track_fade: Option<(u64, u64)>,
    /// Ring-out at jumps: configured length, and the one sounding now.
    tail_frames: u64,
    tail_src: u64,
    tail_left: u64,
    tail_len: u64,
    /// Each stem's level when the ring-out started.
    tail_gains: Vec<f32>,
}

impl Engine {
    /// `channels` is the number of outputs on the interface (e.g. 10).
    pub fn new(song: Song, arr: Arrangement, stems: Vec<StemAudio>, channels: usize) -> Result<Self, String> {
        song.validate()?;
        arr.validate(&song)?;
        if arr.items.is_empty() {
            return Err("the arrangement has no blocks".into());
        }
        if stems.len() != song.stems.len() {
            return Err(format!("song lists {} stems but {} were loaded", song.stems.len(), stems.len()));
        }
        if channels < 2 {
            return Err("the engine needs at least 2 output channels".into());
        }
        let sr = song.sample_rate as f32;
        let mix = song.stems.clone();
        let mut e = Engine {
            fade: (song.sample_rate as u64 / 200).max(1),
            smooth_k: 1.0 - (-1.0 / (0.005 * sr)).exp(),
            gains: vec![0.0; mix.len()],
            peaks: vec![0.0; mix.len()],
            is_click: mix.iter().map(|m| m.name.to_lowercase().contains("click")).collect(),
            track_fade: None,
            tail_frames: 0,
            tail_src: 0,
            tail_left: 0,
            tail_len: 0,
            tail_gains: vec![0.0; mix.len()],
            song,
            arr,
            stems,
            mix,
            channels,
            sr,
            quant: Quantize::Bar,
            click_on: true,
            click_gain: 0.8,
            click_route: if channels >= 4 { Route::Pair(2) } else { Route::Pair(0) },
            count_in: true,
            playing: false,
            stopping: None,
            count_left: 0,
            count_total: 0,
            wait: 0,
            item: 0,
            fpos: 0,
            abs: 0,
            pending: None,
            vamp: false,
            last_jump: None,
            last_beat: None,
            click: ClickVoice::default(),
            finished: false,
        };
        e.snap_gains();
        Ok(e)
    }

    pub fn song(&self) -> &Song {
        &self.song
    }
    pub fn arrangement(&self) -> &Arrangement {
        &self.arr
    }
    pub fn mix(&self) -> &[StemInfo] {
        &self.mix
    }
    pub fn is_playing(&self) -> bool {
        self.playing
    }
    /// Each stem's loudest output sample during the last `process` call, after its level,
    /// mutes and fades (0 when silent). For level meters; doesn't allocate.
    pub fn peaks(&self) -> &[f32] {
        &self.peaks
    }

    /// Replaces the arrangement. Only allowed while stopped; returns false otherwise.
    pub fn set_arrangement(&mut self, arr: Arrangement) -> Result<(), String> {
        if self.playing {
            return Err("stop playback before changing the arrangement".into());
        }
        arr.validate(&self.song)?;
        if arr.items.is_empty() {
            return Err("the arrangement has no blocks".into());
        }
        self.arr = arr;
        self.item = self.item.min(self.arr.items.len() - 1);
        self.fpos = 0;
        Ok(())
    }

    /// Swaps in a new arrangement while playing, without allocating or freeing:
    /// `new` receives the old arrangement, so the caller frees it off the audio thread.
    /// `keep` is the index, in the new arrangement, of the block that is playing now.
    /// The caller must validate `new` against the song first (`Arrangement::validate`).
    /// If the block at `keep` is a different section, playback moves to its start.
    pub fn swap_arrangement(&mut self, new: &mut Arrangement, keep: usize) {
        if new.items.is_empty() {
            return;
        }
        let old_section = self.arr.items.get(self.item).map(|it| it.section);
        std::mem::swap(&mut self.arr, new);
        self.item = keep.min(self.arr.items.len() - 1);
        self.pending = None;
        if Some(self.arr.items[self.item].section) != old_section {
            self.fpos = 0;
            if self.playing {
                self.last_jump = Some(self.abs);
                self.last_beat = None;
            }
        }
        self.finished = false;
    }

    pub fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Play => self.play(),
            Command::Stop => self.stop(),
            Command::Locate(i) => {
                if !self.playing && i < self.arr.items.len() {
                    self.item = i;
                    self.fpos = 0;
                    self.finished = false;
                }
            }
            Command::Seek(i, frame) => {
                if i < self.arr.items.len() {
                    let frame = frame.min(self.block_len(i).saturating_sub(1));
                    if self.playing && self.count_left == 0 {
                        self.pending = Some(Pending { target: i, at: self.abs + self.fade, from: frame, queued: self.abs });
                    } else if !self.playing {
                        self.item = i;
                        self.fpos = frame;
                        self.finished = false;
                    }
                }
            }
            Command::Next => {
                let base = self.pending.as_ref().map(|p| p.target).unwrap_or(self.item);
                self.queue(base + 1, self.quant);
            }
            Command::RepeatCurrent => self.queue(self.item, Quantize::SectionEnd),
            Command::Goto(i) => self.queue(i, self.quant),
            Command::CancelJump => self.pending = None,
            Command::ToggleVamp => self.vamp = !self.vamp,
            Command::SetQuantize(q) => self.quant = q,
            Command::SetStemGain(i, g) => {
                if let Some(m) = self.mix.get_mut(i) {
                    m.gain = g.clamp(0.0, 2.0);
                }
            }
            Command::SetStemMute(i, v) => {
                if let Some(m) = self.mix.get_mut(i) {
                    m.muted = v;
                }
            }
            Command::SetStemRoute(i, r) => {
                if let Some(m) = self.mix.get_mut(i) {
                    m.route = r;
                }
            }
            Command::SetClick(v) => self.click_on = v,
            Command::SetClickGain(g) => self.click_gain = g.clamp(0.0, 2.0),
            Command::SetClickRoute(r) => self.click_route = r,
            Command::SetCountIn(v) => self.count_in = v,
            Command::PlayAfter(n) => {
                if !self.playing {
                    let count_in = self.count_in;
                    self.count_in = false;
                    self.play();
                    self.count_in = count_in;
                    self.wait = n;
                }
            }
            Command::FadeTracks(n) => {
                if self.playing && self.track_fade.is_none() {
                    let n = n.max(1);
                    self.track_fade = Some((n, n));
                }
            }
            Command::SetJoinTail(n) => self.tail_frames = n,
            Command::FadeOut(n) => {
                if self.playing {
                    self.pending = None;
                    if self.count_left > 0 {
                        self.stop();
                    } else {
                        let n = n.max(1);
                        let left = self.stopping.map_or(n, |(l, _)| l.min(n));
                        self.stopping = Some((left, n.max(left)));
                    }
                }
            }
        }
    }

    fn play(&mut self) {
        if self.playing {
            self.stopping = None;
            return;
        }
        if self.finished {
            self.item = 0;
            self.fpos = 0;
            self.finished = false;
        }
        self.playing = true;
        self.stopping = None;
        self.pending = None;
        self.track_fade = None;
        self.tail_left = 0;
        self.last_beat = None;
        self.snap_gains();
        if self.count_in {
            // One bar at the tempo where playback starts.
            let s = self.section(self.item).start as f64 + self.fpos as f64;
            let fpb = self.song.sample_rate as f64 * 60.0 / self.song.bpm_at(s);
            self.count_total = (fpb * self.song.beats_per_bar as f64).round() as u64;
            self.count_left = self.count_total;
        } else {
            self.count_left = 0;
            self.last_jump = Some(self.abs);
        }
    }

    fn stop(&mut self) {
        if !self.playing {
            self.track_fade = None;
            self.item = 0;
            self.fpos = 0;
            self.pending = None;
            self.finished = false;
            return;
        }
        self.pending = None;
        if self.count_left > 0 || self.wait > 0 {
            self.count_left = 0;
            self.wait = 0;
            self.playing = false;
            self.click.left = 0;
        } else {
            self.stopping = Some((self.fade, self.fade));
        }
    }

    fn queue(&mut self, target: usize, q: Quantize) {
        if target >= self.arr.items.len() {
            return;
        }
        if !self.playing {
            self.item = target;
            self.fpos = 0;
            return;
        }
        let at = self.boundary(q);
        self.pending = Some(Pending { target, at, from: 0, queued: self.abs });
    }

    fn section(&self, item: usize) -> &Section {
        &self.song.sections[self.arr.items[item].section]
    }

    /// How long a block plays: its section, or less if it fades out early.
    fn block_len(&self, item: usize) -> u64 {
        let len = self.section(item).len();
        match self.arr.items[item].fade_out {
            Some((s, l)) => len.min(s + l).max(1),
            None => len,
        }
    }

    /// Frames until the song ends by itself, if nothing else is pressed. None while
    /// vamping or stopping (a stop is never a cue for the next song).
    pub fn frames_left(&self) -> Option<u64> {
        if !self.playing || self.vamp || self.stopping.is_some() {
            return None;
        }
        let (from_cur, next) = match &self.pending {
            Some(p) => (p.at.saturating_sub(self.abs) + self.block_len(p.target).saturating_sub(p.from), p.target + 1),
            None => (self.block_len(self.item).saturating_sub(self.fpos), self.item + 1),
        };
        let rest: u64 = (next..self.arr.items.len()).map(|i| self.block_len(i)).sum();
        Some(self.wait + self.count_left + from_cur + rest)
    }

    /// Absolute frame of the next allowed jump point.
    fn boundary(&self, q: Quantize) -> u64 {
        let sec = self.section(self.item);
        let rem = self.block_len(self.item).saturating_sub(self.fpos);
        let d = match q {
            Quantize::Now => self.fade,
            Quantize::SectionEnd => rem,
            Quantize::Bar => {
                let s = (sec.start + self.fpos) as f64;
                let bpb = self.song.beats_per_bar as f64;
                let mut nb = (self.song.beat_pos(s) / bpb).ceil() * bpb;
                let mut b = self.song.beat_frame(nb);
                if b - s < self.fade as f64 {
                    nb += bpb;
                    b = self.song.beat_frame(nb);
                }
                ((b - s).round() as u64).max(self.fade)
            }
        };
        self.abs + d.min(rem)
    }

    fn natural_next(&self) -> Option<usize> {
        if self.vamp {
            Some(self.item)
        } else if self.item + 1 < self.arr.items.len() {
            Some(self.item + 1)
        } else {
            None
        }
    }

    fn contiguous(&self, a: usize, b: usize) -> bool {
        self.block_len(a) == self.section(a).len() && self.section(a).end == self.section(b).start
    }

    /// Frames until the next point where the audio cuts to somewhere else.
    fn frames_to_cut(&self) -> Option<u64> {
        let rem = self.block_len(self.item).saturating_sub(self.fpos);
        if let Some(p) = &self.pending {
            let d = p.at.saturating_sub(self.abs);
            if d == rem && self.contiguous(self.item, p.target) {
                return None;
            }
            return Some(d);
        }
        match self.natural_next() {
            Some(n) if !self.contiguous(self.item, n) => Some(rem),
            _ => None,
        }
    }

    fn enter(&mut self, item: usize, cut: bool) {
        if cut && self.tail_frames > 0 && self.count_left == 0 {
            // Ring-out: carry on from where the old block left, fading, under the new one.
            self.tail_src = self.section(self.item).start + self.fpos;
            self.tail_len = self.tail_frames;
            self.tail_left = self.tail_frames;
            self.tail_gains.copy_from_slice(&self.gains);
        }
        self.item = item;
        self.fpos = 0;
        if cut {
            self.last_jump = Some(self.abs);
            self.last_beat = None;
        }
    }

    /// Where the guide stem reads from at source frame `src` (None = silence). In the
    /// last stretch of a block it plays the call for the block that's actually coming
    /// next (planned, queued, repeated or vamped); the call the recording had there for
    /// a different section is silenced.
    fn guide_src(&self, src: u64) -> Option<u64> {
        let sec = self.section(self.item);
        let (next, d, queued) = match &self.pending {
            Some(p) => (Some(p.target), p.at.saturating_sub(self.abs), Some(p.queued)),
            None => (self.natural_next(), self.block_len(self.item).saturating_sub(self.fpos), None),
        };
        if let Some(n) = next {
            let ns = self.arr.items[n].section;
            if let Some(Some((cs, ce))) = self.song.calls.get(ns) {
                let l = ce.saturating_sub(*cs);
                if d > 0 && d <= l {
                    // A jump asked for with less time left than its call takes: say the
                    // call from the start, from the moment it was asked for, so the band
                    // hears the name ("Bridge…") rather than only the end of the count.
                    if let Some(q) = queued {
                        let window = d + self.abs.saturating_sub(q);
                        if window < l {
                            let gs = cs + self.abs.saturating_sub(q);
                            return (gs < *ce).then_some(gs);
                        }
                    }
                    return Some(ce - d);
                }
            }
        }
        let end = sec.start + self.block_len(self.item);
        let silenced = self.song.calls.iter().flatten().any(|(cs, ce)| *ce == end && src >= *cs && src < *ce);
        if silenced {
            None
        } else {
            Some(src)
        }
    }

    fn target_gain(&self, i: usize) -> f32 {
        let m = &self.mix[i];
        let it = &self.arr.items[self.item];
        if m.muted || it.muted_stems.contains(&i) {
            return 0.0;
        }
        let mut g = m.gain;
        for t in &it.trims {
            if t.stem == i {
                g *= t.gain(self.fpos);
            }
        }
        g
    }

    fn snap_gains(&mut self) {
        for i in 0..self.gains.len() {
            self.gains[i] = self.target_gain(i);
        }
    }

    fn trigger_click(&mut self, accent: bool) {
        let f = if accent { 1760.0 } else { 1175.0 };
        self.click = ClickVoice {
            phase: 0.0,
            step: std::f32::consts::TAU * f / self.sr,
            amp: 0.9,
            decay: (-90.0 / self.sr).exp(),
            left: (0.05 * self.sr) as u32,
        };
    }

    fn click_sample(&mut self) -> f32 {
        if self.click.left == 0 {
            return 0.0;
        }
        let v = self.click.phase.sin() * self.click.amp;
        self.click.phase += self.click.step;
        self.click.amp *= self.click.decay;
        self.click.left -= 1;
        v
    }

    #[inline]
    fn write(out: &mut [f32], base: usize, ch: usize, route: Route, l: f32, r: f32) {
        match route {
            Route::Pair(c) => {
                let c = c as usize;
                if c + 1 < ch {
                    out[base + c] += l;
                    out[base + c + 1] += r;
                } else {
                    out[base] += l;
                    out[base + 1] += r;
                }
            }
            Route::Mono(c) => {
                let c = c as usize;
                let v = (l + r) * 0.5;
                out[base + if c < ch { c } else { 0 }] += v;
            }
        }
    }

    /// Fills `out` (interleaved, `channels` wide) with the next block of audio.
    pub fn process(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        self.peaks.iter_mut().for_each(|p| *p = 0.0);
        let ch = self.channels;
        let frames = out.len() / ch;
        let fade = self.fade as f32;
        let bpb = self.song.beats_per_bar as i64;
        for f in 0..frames {
            if !self.playing {
                break;
            }
            let base = f * ch;
            if self.wait > 0 {
                self.wait -= 1;
                self.abs += 1;
                if self.wait == 0 {
                    self.last_jump = Some(self.abs);
                }
                continue;
            }

            // Count-in: one bar of click before the audio starts.
            if self.count_left > 0 {
                let fpb = self.count_total as f64 / bpb.max(1) as f64;
                let done = (self.count_total - self.count_left) as f64;
                let beat = (done / fpb).floor() as i64;
                if done - beat as f64 * fpb < 1.0 {
                    self.trigger_click(beat % bpb == 0);
                }
                let c = self.click_sample() * self.click_gain;
                Self::write(out, base, ch, self.click_route, c, c);
                self.count_left -= 1;
                self.abs += 1;
                if self.count_left == 0 {
                    self.last_jump = Some(self.abs);
                    self.last_beat = None;
                }
                continue;
            }

            // Jumps and block changes.
            let due = self.pending.as_ref().filter(|p| self.abs >= p.at).map(|p| (p.target, p.from));
            if let Some((t, from)) = due {
                let cut = from > 0 || !(self.fpos == self.block_len(self.item) && self.contiguous(self.item, t));
                self.pending = None;
                self.enter(t, cut);
                self.fpos = from.min(self.block_len(t).saturating_sub(1));
            }
            if self.fpos >= self.block_len(self.item) {
                match self.natural_next() {
                    Some(n) => {
                        let cut = !self.contiguous(self.item, n);
                        self.enter(n, cut);
                    }
                    None => {
                        self.playing = false;
                        self.finished = true;
                        break;
                    }
                }
            }

            // Envelopes: stop fade, fade out before a cut, fade in after one.
            let mut master = 1.0f32;
            if let Some((left, total)) = self.stopping {
                if left == 0 {
                    self.playing = false;
                    self.stopping = None;
                    self.track_fade = None;
                    break;
                }
                master = left as f32 / total as f32;
                self.stopping = Some((left - 1, total));
            }
            // A block that fades out (e.g. an ending cut short) takes the click with it.
            if let Some((s, l)) = self.arr.items[self.item].fade_out {
                if self.fpos >= s {
                    master *= 1.0 - ((self.fpos - s) as f32 / l as f32).min(1.0);
                }
            }
            let mut env = master;
            // (With a ring-out the old audio carries on through the cut, so no fade before it.)
            if let Some(d) = self.frames_to_cut().filter(|_| self.tail_frames == 0) {
                if (d as f32) < fade {
                    env *= d as f32 / fade;
                }
            }
            if let Some(lj) = self.last_jump {
                let since = (self.abs - lj) as f32;
                if since < fade {
                    env *= (since + 1.0) / (fade + 1.0);
                }
            }

            // "Fade out now": tracks and guide fade, click stems don't.
            let mut tracks = 1.0f32;
            if let Some((left, total)) = self.track_fade {
                tracks = left as f32 / total as f32;
                if left > 0 {
                    self.track_fade = Some((left - 1, total));
                }
            }

            // Stems.
            let src = self.section(self.item).start + self.fpos;
            for i in 0..self.stems.len() {
                let t = self.target_gain(i);
                let g = &mut self.gains[i];
                *g += (t - *g) * self.smooth_k;
                if t == 0.0 && *g < 1e-5 {
                    *g = 0.0;
                    continue;
                }
                let gg = *g * env * if self.is_click[i] { 1.0 } else { tracks };
                if gg == 0.0 {
                    continue;
                }
                let (l, r) = if Some(i) == self.song.guide_stem && !self.song.calls.is_empty() {
                    match self.guide_src(src) {
                        Some(gs) => self.stems[i].frame(gs),
                        None => (0.0, 0.0),
                    }
                } else {
                    self.stems[i].frame(src)
                };
                let (ol, or) = (l * gg, r * gg);
                let pk = ol.abs().max(or.abs());
                if pk > self.peaks[i] {
                    self.peaks[i] = pk;
                }
                Self::write(out, base, ch, self.mix[i].route, ol, or);
            }

            // Ring-out from a jump: the music (not the click or guide) carries on from where
            // it left, fading away under the new block.
            if self.tail_left > 0 {
                let k = self.tail_left as f32 / self.tail_len.max(1) as f32;
                let w = k * k * (3.0 - 2.0 * k) * master * tracks;
                for i in 0..self.stems.len() {
                    if self.is_click[i] || Some(i) == self.song.guide_stem {
                        continue;
                    }
                    let g = self.tail_gains[i] * w;
                    if g == 0.0 {
                        continue;
                    }
                    let (l, r) = self.stems[i].frame(self.tail_src);
                    Self::write(out, base, ch, self.mix[i].route, l * g, r * g);
                }
                self.tail_src += 1;
                self.tail_left -= 1;
            }

            // Click, locked to the song's own bar grid.
            let pos = self.song.beat_pos(src as f64);
            let beat = pos.floor() as i64;
            if self.click_on && pos >= 0.0 && self.last_beat != Some(beat) && src as f64 - self.song.beat_frame(beat as f64) < 1.0 {
                self.trigger_click(beat.rem_euclid(bpb) == 0);
            }
            self.last_beat = Some(beat);
            let c = self.click_sample() * self.click_gain * master;
            if c != 0.0 {
                Self::write(out, base, ch, self.click_route, c, c);
            }

            self.fpos += 1;
            self.abs += 1;
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let it = self.item.min(self.arr.items.len() - 1);
        let sec = self.section(it);
        let src = (sec.start + self.fpos) as f64;
        let pos = self.song.beat_pos(src);
        let beat = pos.floor() as i64;
        let bpb = self.song.beats_per_bar as i64;
        let end = self.song.beat_pos((sec.start + self.block_len(it)) as f64);
        Snapshot {
            playing: self.playing,
            counting_in: self.count_left > 0,
            finished: self.finished,
            item: it,
            section: self.arr.items[it].section,
            frame_in_section: self.fpos,
            section_frames: self.block_len(it),
            bar: beat.div_euclid(bpb) + 1,
            beat: beat.rem_euclid(bpb) as u32 + 1,
            pending: self.pending.as_ref().map(|p| p.target),
            frames_to_jump: self.pending.as_ref().map(|p| p.at.saturating_sub(self.abs)),
            vamp: self.vamp,
            bars_left: ((end - pos) / bpb.max(1) as f64).max(0.0),
            bpm: self.song.bpm_at(src),
            tracks_out: self.track_fade.is_some(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    const SR: u32 = 1000; // 60 BPM -> 1000 frames per beat, 4000 per bar
    const CH: usize = 4;

    /// Three 2-bar sections A, B, C. One mono stem whose sample value is its frame index,
    /// so the output tells us exactly which source frame was played.
    fn rig(arr: Option<&[&str]>) -> Engine {
        let song = Song {
            name: "Test".into(),
            sample_rate: SR,
            bpm: 60.0,
            beats_per_bar: 4,
            offset: 0,
            length: 24000,
            sections: vec![
                Section { name: "A".into(), start: 0, end: 8000 },
                Section { name: "B".into(), start: 8000, end: 16000 },
                Section { name: "C".into(), start: 16000, end: 24000 },
            ],
            stems: vec![StemInfo { name: "Ramp".into(), route: Route::Mono(0), gain: 1.0, muted: false }],
            guide_stem: None,
            calls: vec![],
            beat_frames: vec![],
        };
        let arrangement = match arr {
            Some(names) => Arrangement::from_names(&song, names).unwrap(),
            None => Arrangement::linear(&song),
        };
        let audio = StemAudio::mono((0..24000).map(|i| i as f32).collect());
        let mut e = Engine::new(song, arrangement, vec![audio], CH).unwrap();
        e.handle(Command::SetCountIn(false));
        e.handle(Command::SetClick(false));
        e
    }

    /// Renders `frames` frames and returns every channel, de-interleaved.
    fn run(e: &mut Engine, frames: usize) -> Vec<Vec<f32>> {
        let mut chans = vec![Vec::with_capacity(frames); CH];
        let mut buf = vec![0.0; 256 * CH];
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(256);
            e.process(&mut buf[..n * CH]);
            for f in 0..n {
                for c in 0..CH {
                    chans[c].push(buf[f * CH + c]);
                }
            }
            done += n;
        }
        chans
    }

    #[test]
    fn plays_linear_song_and_finishes() {
        let mut e = rig(None);
        e.handle(Command::Play);
        let out = run(&mut e, 24100);
        assert_eq!(out[0][7999], 7999.0);
        assert_eq!(out[0][8000], 8000.0, "contiguous sections must not fade");
        assert_eq!(out[0][23999], 23999.0);
        assert_eq!(out[0][24050], 0.0);
        let s = e.snapshot();
        assert!(!s.playing && s.finished);
    }

    #[test]
    fn reordered_arrangement_plays_in_new_order() {
        let mut e = rig(Some(&["C", "A", "A"]));
        e.handle(Command::Play);
        let out = run(&mut e, 24000);
        assert_eq!(out[0][100], 16100.0);
        assert_eq!(out[0][8100], 100.0);
        assert_eq!(out[0][16100], 100.0, "repeated block starts over");
        // Fade around a cut: just before it is attenuated, never louder than the source.
        assert!(out[0][7998].abs() < 23998.0);
    }

    #[test]
    fn jump_on_next_bar_lands_on_the_downbeat() {
        let mut e = rig(None);
        e.handle(Command::Play);
        run(&mut e, 1500);
        e.handle(Command::Goto(2));
        assert_eq!(e.snapshot().pending, Some(2));
        let out = run(&mut e, 4000);
        // Absolute frames: 1500 + i. The next bar is at 4000.
        assert_eq!(out[0][3990 - 1500], 3990.0, "untouched until the fade");
        assert_eq!(out[0][4010 - 1500], 16010.0, "now playing C from its start");
        assert_eq!(e.snapshot().pending, None);
    }

    #[test]
    fn next_twice_skips_a_block() {
        let mut e = rig(None);
        e.handle(Command::SetQuantize(Quantize::SectionEnd));
        e.handle(Command::Play);
        run(&mut e, 100);
        e.handle(Command::Next);
        e.handle(Command::Next);
        assert_eq!(e.snapshot().pending, Some(2));
        let out = run(&mut e, 8000);
        assert_eq!(out[0][8000 - 100 + 20], 16020.0);
    }

    #[test]
    fn vamp_loops_the_current_block() {
        let mut e = rig(None);
        e.handle(Command::Play);
        e.handle(Command::ToggleVamp);
        let out = run(&mut e, 16100);
        assert_eq!(out[0][8020], 20.0);
        assert_eq!(out[0][16020], 20.0);
        e.handle(Command::ToggleVamp);
        let out = run(&mut e, 8000);
        // Vamp off: after this pass of A, B follows.
        assert!(out[0].iter().any(|&v| v >= 8000.0));
    }

    #[test]
    fn repeat_plays_the_block_again() {
        let mut e = rig(None);
        e.handle(Command::Play);
        run(&mut e, 2000);
        e.handle(Command::RepeatCurrent);
        let out = run(&mut e, 6100);
        assert_eq!(out[0][6000 + 50], 50.0);
    }

    #[test]
    fn per_block_stem_mute_fades_the_stem_out() {
        let mut e = rig(None);
        let mut arr = e.arrangement().clone();
        arr.items[1].muted_stems = vec![0];
        e.set_arrangement(arr).unwrap();
        e.handle(Command::Play);
        let out = run(&mut e, 16000);
        assert_eq!(out[0][7000], 7000.0);
        assert!(out[0][8200].abs() < 1e-3, "muted in block B");
        assert_eq!(out[0].len(), 16000);
    }

    #[test]
    fn routes_to_the_chosen_outputs() {
        let mut e = rig(None);
        e.handle(Command::SetStemRoute(0, Route::Pair(2)));
        e.handle(Command::Play);
        let out = run(&mut e, 500);
        assert_eq!(out[0][300], 0.0);
        assert_eq!(out[2][300], 300.0);
        assert_eq!(out[3][300], 300.0);
    }

    #[test]
    fn click_hits_every_beat_on_its_own_output() {
        let mut e = rig(None);
        e.handle(Command::SetStemMute(0, true));
        e.handle(Command::SetClick(true));
        e.handle(Command::SetClickRoute(Route::Mono(3)));
        e.handle(Command::Play);
        let out = run(&mut e, 2000);
        let peak = |a: usize, b: usize| out[3][a..b].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak(0, 50) > 0.1, "click on beat 1");
        assert_eq!(peak(600, 1000), 0.0, "silent between beats");
        assert!(peak(1000, 1050) > 0.1, "click on beat 2");
        assert_eq!(peak(0, 2000) > 0.0 && out[0][500] == 0.0, true);
    }

    #[test]
    fn count_in_is_one_bar_before_the_audio() {
        let mut e = rig(None);
        e.handle(Command::SetCountIn(true));
        e.handle(Command::Play);
        assert!(e.snapshot().counting_in);
        let out = run(&mut e, 4100);
        assert!(out[0][..4000].iter().all(|&v| v == 0.0));
        assert_eq!(out[0][4050], 50.0);
        assert!(out[2][..4000].iter().any(|&v| v != 0.0), "count-in click on the in-ears");
    }

    #[test]
    fn stop_fades_then_stop_again_returns_to_top() {
        let mut e = rig(None);
        e.handle(Command::Play);
        run(&mut e, 9000);
        e.handle(Command::Stop);
        let out = run(&mut e, 100);
        assert!(out[0][50] == 0.0);
        assert!(!e.is_playing());
        assert_eq!(e.snapshot().item, 1, "stop keeps the position");
        e.handle(Command::Stop);
        assert_eq!(e.snapshot().item, 0);
        assert_eq!(e.snapshot().frame_in_section, 0);
    }

    #[test]
    fn seek_moves_the_playhead_inside_a_block() {
        let mut e = rig(None);
        e.handle(Command::Seek(1, 2500)); // stopped: B at 2500
        assert_eq!((e.snapshot().item, e.snapshot().frame_in_section), (1, 2500));
        e.handle(Command::Play);
        let out = run(&mut e, 100);
        assert_eq!(out[0][50], 8000.0 + 2500.0 + 50.0, "plays from the seek point");
        e.handle(Command::Seek(2, 4000)); // playing: C at 4000, after a 5 ms fade
        let out = run(&mut e, 100);
        assert_eq!(out[0][50], 16000.0 + 4000.0 + 45.0);
    }

    /// The ramp stem as a guide: B's call is the last 2000 frames of A, C's the last 2000
    /// of B (the guide says the next section in the bar before it).
    fn guide_rig(arr: &[&str]) -> Engine {
        let mut e = rig(Some(arr));
        e.handle(Command::Stop);
        let mut song = e.song().clone();
        song.guide_stem = Some(0);
        song.calls = vec![None, Some((6000, 8000)), Some((14000, 16000))];
        let arrangement = Arrangement::from_names(&song, arr).unwrap();
        let audio = StemAudio::mono((0..24000).map(|i| i as f32).collect());
        let mut e = Engine::new(song, arrangement, vec![audio], CH).unwrap();
        e.handle(Command::SetCountIn(false));
        e.handle(Command::SetClick(false));
        e
    }

    #[test]
    fn guide_calls_the_section_that_actually_comes_next() {
        // A then C: the end of A must call C (source 14000..16000), not B.
        let mut e = guide_rig(&["A", "C"]);
        e.handle(Command::Play);
        let out = run(&mut e, 8100);
        assert_eq!(out[0][5000], 5000.0, "before the call: A as recorded");
        assert_eq!(out[0][7000], 15000.0, "C's call in the last stretch of A");
        // A then A (repeat): the end of A calls A, which has no call -> B's call silenced
        let mut e = guide_rig(&["A", "A"]);
        e.handle(Command::Play);
        let out = run(&mut e, 8100);
        assert_eq!(out[0][7000], 0.0, "B's call isn't heard when B isn't next");
        // A then B as recorded: unchanged
        let mut e = guide_rig(&["A", "B"]);
        e.handle(Command::Play);
        let out = run(&mut e, 8100);
        assert_eq!(out[0][7000], 7000.0);
        // C then C (duplicated): the end of the first C calls C again
        let mut e = guide_rig(&["B", "C", "C"]);
        e.handle(Command::Play);
        let out = run(&mut e, 24100);
        assert_eq!(out[0][8000 + 7000], 15000.0, "second C announced at the end of the first");
    }

    #[test]
    fn a_late_jump_says_the_name_of_where_it_goes() {
        // Playing A (calls: B 6000..8000, C 14000..16000). At frame 7000 the MD asks for C on
        // the next bar (8000): 1000 frames left, C's call takes 2000. The guide says C's
        // call from its start (the name) straight away, not the last half of it.
        let mut e = guide_rig(&["A", "B", "C"]);
        e.handle(Command::SetQuantize(Quantize::Bar));
        e.handle(Command::Play);
        let before = run(&mut e, 7000);
        assert_eq!(before[0][6500], 6500.0, "B's call while B is next");
        e.handle(Command::Goto(2));
        let out = run(&mut e, 1500);
        assert_eq!(out[0][100], 14100.0, "C's call from the start, 100 frames after asking");
        assert_eq!(out[0][900], 14900.0);
        // A jump asked for in good time still lines the call up to end on the downbeat.
        let mut e = guide_rig(&["A", "B", "C"]);
        e.handle(Command::SetQuantize(Quantize::SectionEnd));
        e.handle(Command::Play);
        run(&mut e, 3000);
        e.handle(Command::Goto(2));
        let out = run(&mut e, 5000);
        assert_eq!(out[0][4000], 15000.0, "frame 7000: C's call ends on the jump at 8000");
    }

    #[test]
    fn vamping_calls_the_same_section_again() {
        let mut e = guide_rig(&["B", "C"]);
        e.handle(Command::Play);
        e.handle(Command::ToggleVamp); // loop B
        let out = run(&mut e, 8000);
        // B has a call (6000..8000); the end of B now calls B again, not C
        assert_eq!(out[0][7000], 7000.0);
    }

    /// An engine for arrangement A, B, C with `edit` applied to its blocks.
    fn rig_with(edit: impl FnOnce(&mut Arrangement)) -> Engine {
        let e = rig(None);
        let song = e.song().clone();
        let mut arr = Arrangement::linear(&song);
        edit(&mut arr);
        let audio = StemAudio::mono((0..24000).map(|i| i as f32).collect());
        let mut e = Engine::new(song, arr, vec![audio], CH).unwrap();
        e.handle(Command::SetCountIn(false));
        e.handle(Command::SetClick(false));
        e
    }

    #[test]
    fn a_trimmed_clip_plays_only_inside_its_trim() {
        // Block B: the stem plays from 2000 to 6000 of the block, fading in over 1000.
        let mut e = rig_with(|a| a.items[1].trims.push(Trim { stem: 0, start: 2000, end: 6000, fade_in: 1000, fade_out: 0 }));
        e.handle(Command::Play);
        let out = run(&mut e, 24000);
        assert_eq!(out[0][7000], 7000.0, "A is untouched");
        assert_eq!(out[0][8000 + 1500], 0.0, "B before the trim is silent");
        let mid = out[0][8000 + 2500] / (8000.0 + 2500.0);
        assert!((mid - 0.5).abs() < 0.02, "halfway through the fade-in: {mid}");
        assert!((out[0][8000 + 4000] - 12000.0).abs() < 1.0, "fully in after the fade");
        assert_eq!(out[0][8000 + 6500], 0.0, "silent after the trim ends");
        assert!((out[0][16500] - 16500.0).abs() < 0.01, "C is untouched: {}", out[0][16500]);
    }

    #[test]
    fn a_block_fade_out_ends_the_song_early() {
        // The last block (C) fades out from 2000 over 4000 frames, then the song ends.
        let mut e = rig_with(|a| a.items[2].fade_out = Some((2000, 4000)));
        e.handle(Command::Play);
        assert_eq!(e.frames_left(), Some(16000 + 6000));
        let out = run(&mut e, 24000);
        assert_eq!(out[0][16000 + 1000], 17000.0, "before the fade");
        let half = out[0][16000 + 4000] / 20000.0;
        assert!((half - 0.5).abs() < 0.01, "halfway down: {half}");
        assert_eq!(out[0][16000 + 6000], 0.0);
        assert!(!e.is_playing() && e.snapshot().finished, "the song ends when the fade does");
    }

    #[test]
    fn a_faded_block_in_the_middle_moves_on_to_the_next() {
        let mut e = rig_with(|a| a.items[0].fade_out = Some((4000, 2000)));
        e.handle(Command::Play);
        let out = run(&mut e, 7000);
        assert!(out[0][5999] < 10.0, "A is faded out by 6000");
        // B starts at frame 6000 of the output, with the usual short fade-in.
        assert!((out[0][6000 + 100] - 8100.0).abs() < 1.0, "B follows straight after: {}", out[0][6100]);
    }

    #[test]
    fn fade_out_command_fades_the_song_and_stops() {
        let mut e = rig(None);
        e.handle(Command::Play);
        run(&mut e, 1000);
        e.handle(Command::FadeOut(2000));
        assert_eq!(e.frames_left(), None, "a fade-out is a stop, not the song ending");
        let out = run(&mut e, 3000);
        let half = out[0][1000] / 2000.0;
        assert!((half - 0.5).abs() < 0.01, "{half}");
        assert!(!e.is_playing());
    }

    #[test]
    fn play_after_starts_on_the_exact_frame() {
        let mut e = rig(None);
        e.handle(Command::SetCountIn(true)); // ignored: a segue never counts in
        e.handle(Command::PlayAfter(300));
        assert_eq!(e.frames_left(), Some(300 + 24000));
        let out = run(&mut e, 1000);
        assert_eq!(out[0][299], 0.0);
        assert!(out[0][300 + 100] == 100.0, "starts at frame 300 from the top: {}", out[0][400]);
        assert!(e.is_playing());
    }

    #[test]
    fn click_bars_and_jumps_follow_a_tempo_change() {
        // 60 BPM for two bars (1000 frames a beat), then 120 BPM (500 a beat).
        let mut e = rig(None);
        let mut song = e.song().clone();
        song.beat_frames = (0..=8).map(|k| k as f64 * 1000.0).chain((1..=32).map(|k| 8000.0 + k as f64 * 500.0)).collect();
        let arr = Arrangement::linear(&song);
        let audio = StemAudio::mono((0..24000).map(|i| i as f32).collect());
        e = Engine::new(song, arr, vec![audio], CH).unwrap();
        e.handle(Command::SetCountIn(false));
        e.handle(Command::SetClick(true));
        e.handle(Command::SetClickRoute(Route::Mono(3)));
        e.handle(Command::Play);
        let out = run(&mut e, 12000);
        // A click is a sine starting at phase 0: its first non-zero sample is one after it starts.
        let on: Vec<usize> = (1..12000).filter(|&i| out[3][i - 1] == 0.0 && out[3][i] != 0.0).map(|i| i - 1).collect();
        assert_eq!(on, [0, 1000, 2000, 3000, 4000, 5000, 6000, 7000, 8000, 8500, 9000, 9500, 10000, 10500, 11000, 11500], "the click speeds up at bar 3");
        let s = e.snapshot();
        assert_eq!((s.bar, s.beat), (5, 1), "frame 12000: two bars at 60, then two at 120");
        assert!((s.bpm - 120.0).abs() < 1e-9);
        // "Next bar" lands on the next downbeat at the new tempo: bar 6, at frame 14000.
        e.handle(Command::SetQuantize(Quantize::Bar));
        run(&mut e, 300);
        e.handle(Command::Next);
        assert_eq!(e.snapshot().frames_to_jump, Some(14000 - 12300));
        // B starts at 8000 and ends at 16000: (16000 - 12300) / 2000 frames a bar.
        assert!((e.snapshot().bars_left - 1.85).abs() < 1e-9, "{}", e.snapshot().bars_left);
    }

    #[test]
    fn peaks_follow_what_each_stem_sends_out() {
        let mut e = rig(None);
        e.handle(Command::Play);
        run(&mut e, 1000);
        assert!((e.peaks()[0] - 999.0).abs() < 1.0, "the ramp's loudest sample this block: {}", e.peaks()[0]);
        e.handle(Command::SetStemGain(0, 0.5));
        run(&mut e, 1000);
        assert!((e.peaks()[0] - 999.5).abs() < 1.0, "half level: {}", e.peaks()[0]);
        e.handle(Command::SetStemMute(0, true));
        run(&mut e, 2000);
        assert_eq!(e.peaks()[0], 0.0, "muted: nothing out");
    }

    #[test]
    fn frames_left_follows_jumps_and_vamps() {
        let mut e = rig(None);
        e.handle(Command::Play);
        run(&mut e, 1000);
        assert_eq!(e.frames_left(), Some(23000));
        e.handle(Command::SetQuantize(Quantize::SectionEnd));
        e.handle(Command::Goto(0)); // A again after A: one more block
        assert_eq!(e.frames_left(), Some(7000 + 24000));
        e.handle(Command::ToggleVamp);
        assert_eq!(e.frames_left(), None);
    }

    #[test]
    fn arrangement_cannot_be_rebuilt_while_playing() {
        let mut e = rig(None);
        e.handle(Command::Play);
        let arr = e.arrangement().clone();
        assert!(e.set_arrangement(arr).is_err());
    }

    #[test]
    fn live_swap_keeps_the_current_block_playing_and_uses_the_new_order() {
        let mut e = rig(None); // A B C
        e.handle(Command::Play);
        run(&mut e, 3000); // in A
        let song = e.song().clone();
        let mut new = Arrangement::from_names(&song, &["C", "A", "C"]).unwrap();
        e.swap_arrangement(&mut new, 1); // A is now block 2
        assert_eq!(new.items.len(), 3, "old arrangement handed back to be freed");
        assert_eq!(e.snapshot().item, 1);
        let out = run(&mut e, 5100);
        assert_eq!(out[0][1000], 4000.0, "A keeps playing where it was");
        assert_eq!(out[0][5050], 16050.0, "then the new next block (C)");
    }

    /// A two-stem rig: stem 0 "Ramp" (value = frame index) on output 0, stem 1
    /// "Click Track" (always 1.0) on output 1.
    fn rig_with_click() -> Engine {
        let song = Song {
            name: "Test".into(),
            sample_rate: SR,
            bpm: 60.0,
            beats_per_bar: 4,
            offset: 0,
            length: 24000,
            sections: vec![
                Section { name: "A".into(), start: 0, end: 8000 },
                Section { name: "B".into(), start: 8000, end: 16000 },
                Section { name: "C".into(), start: 16000, end: 24000 },
            ],
            stems: vec![
                StemInfo { name: "Ramp".into(), route: Route::Mono(0), gain: 1.0, muted: false },
                StemInfo { name: "Click Track".into(), route: Route::Mono(1), gain: 1.0, muted: false },
            ],
            guide_stem: None,
            calls: vec![],
            beat_frames: vec![],
        };
        let arr = Arrangement::linear(&song);
        let ramp = StemAudio::mono((0..24000).map(|i| i as f32).collect());
        let click = StemAudio::mono(vec![1.0; 24000]);
        let mut e = Engine::new(song, arr, vec![ramp, click], CH).unwrap();
        e.handle(Command::SetCountIn(false));
        e.handle(Command::SetClick(false));
        e
    }

    #[test]
    fn fade_out_now_fades_the_tracks_and_keeps_the_click() {
        let mut e = rig_with_click();
        e.handle(Command::Play);
        let mut buf = vec![0.0f32; 1000 * CH];
        e.process(&mut buf); // frames 0..999 (past the start fade-in)
        e.handle(Command::FadeTracks(1000));
        let mut buf = vec![0.0f32; 1000 * CH];
        e.process(&mut buf); // frames 1000..1999: the fade
        // Halfway through the fade the ramp is at half level; the click is untouched.
        let half = buf[500 * CH];
        assert!((half - 1500.0 * 0.5).abs() < 2.0, "ramp at half level, got {half}");
        assert_eq!(buf[500 * CH + 1], 1.0);
        assert!(e.snapshot().tracks_out);
        let mut buf = vec![0.0f32; 1000 * CH];
        e.process(&mut buf); // frames 2000..2999: faded
        assert!(buf.iter().step_by(CH).all(|v| *v == 0.0), "the tracks are silent");
        assert!(buf.iter().skip(1).step_by(CH).all(|v| *v == 1.0), "the click carries on");
        assert!(e.snapshot().playing, "still playing until you stop");
        // Stop, then play again: everything is back.
        e.handle(Command::Stop);
        let mut buf = vec![0.0f32; 100 * CH];
        e.process(&mut buf);
        assert!(!e.snapshot().playing);
        assert!(!e.snapshot().tracks_out);
        e.handle(Command::Play);
        let mut buf = vec![0.0f32; 500 * CH];
        e.process(&mut buf);
        assert!(buf[400 * CH] > 0.0, "tracks back after play");
    }

    #[test]
    fn a_jump_rings_out_instead_of_chopping() {
        // A then C: B is skipped, so A's end jumps to C's start.
        let tail = 300u64;
        let mut e = rig(Some(&["A", "C"]));
        e.handle(Command::SetJoinTail(tail));
        e.handle(Command::Play);
        let mut buf = vec![0.0f32; 8000 * CH];
        e.process(&mut buf); // all of A
        // A plays to its very last frame at full level (no fade before the jump).
        assert_eq!(buf[7999 * CH], 7999.0);
        let mut buf = vec![0.0f32; 1000 * CH];
        e.process(&mut buf);
        // After the 5-frame fade-in, the output is C's audio plus B's opening (where A
        // would have gone on to) fading away along a smoothstep.
        for k in [10u64, 100, 150, 250] {
            let kk = (tail - k) as f32 / tail as f32;
            let w = kk * kk * (3.0 - 2.0 * kk);
            let want = (16000 + k) as f32 + (8000 + k) as f32 * w;
            let got = buf[k as usize * CH];
            assert!((got - want).abs() < 1.0, "frame {k}: want {want}, got {got}");
        }
        // Once the ring-out is over, only C.
        for k in [tail, tail + 1, 900] {
            assert_eq!(buf[k as usize * CH], (16000 + k) as f32);
        }
        // With no ring-out it's the old straight cut.
        let mut e = rig(Some(&["A", "C"]));
        e.handle(Command::Play);
        let mut buf = vec![0.0f32; 8100 * CH];
        e.process(&mut buf);
        assert_eq!(buf[8050 * CH], 16050.0);
    }
}

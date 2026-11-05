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
}

struct Pending {
    target: usize,
    at: u64,
    /// Frame inside the target block to start from (0 = its start).
    from: u64,
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
    quant: Quantize,
    click_on: bool,
    click_gain: f32,
    click_route: Route,
    count_in: bool,
    playing: bool,
    stopping: Option<u64>,
    count_left: u64,
    count_total: u64,
    item: usize,
    fpos: u64,
    abs: u64,
    pending: Option<Pending>,
    vamp: bool,
    last_jump: Option<u64>,
    last_beat: Option<i64>,
    click: ClickVoice,
    finished: bool,
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
                    let frame = frame.min(self.section(i).len().saturating_sub(1));
                    if self.playing && self.count_left == 0 {
                        self.pending = Some(Pending { target: i, at: self.abs + self.fade, from: frame });
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
        self.last_beat = None;
        self.snap_gains();
        if self.count_in {
            self.count_total = self.song.frames_per_bar().round() as u64;
            self.count_left = self.count_total;
        } else {
            self.count_left = 0;
            self.last_jump = Some(self.abs);
        }
    }

    fn stop(&mut self) {
        if !self.playing {
            self.item = 0;
            self.fpos = 0;
            self.pending = None;
            self.finished = false;
            return;
        }
        self.pending = None;
        if self.count_left > 0 {
            self.count_left = 0;
            self.playing = false;
            self.click.left = 0;
        } else {
            self.stopping = Some(self.fade);
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
        self.pending = Some(Pending { target, at, from: 0 });
    }

    fn section(&self, item: usize) -> &Section {
        &self.song.sections[self.arr.items[item].section]
    }

    /// Absolute frame of the next allowed jump point.
    fn boundary(&self, q: Quantize) -> u64 {
        let sec = self.section(self.item);
        let rem = sec.len().saturating_sub(self.fpos);
        let d = match q {
            Quantize::Now => self.fade,
            Quantize::SectionEnd => rem,
            Quantize::Bar => {
                let s = (sec.start + self.fpos) as f64;
                let off = self.song.offset as f64;
                let bar = self.song.frames_per_bar();
                let mut b = off + ((s - off) / bar).ceil() * bar;
                if b - s < self.fade as f64 {
                    b += bar;
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
        self.section(a).end == self.section(b).start
    }

    /// Frames until the next point where the audio cuts to somewhere else.
    fn frames_to_cut(&self) -> Option<u64> {
        let rem = self.section(self.item).len().saturating_sub(self.fpos);
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
        self.item = item;
        self.fpos = 0;
        if cut {
            self.last_jump = Some(self.abs);
            self.last_beat = None;
        }
    }

    fn target_gain(&self, i: usize) -> f32 {
        let m = &self.mix[i];
        if m.muted || self.arr.items[self.item].muted_stems.contains(&i) {
            0.0
        } else {
            m.gain
        }
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
        let ch = self.channels;
        let frames = out.len() / ch;
        let fade = self.fade as f32;
        let fpb = self.song.frames_per_beat();
        let bpb = self.song.beats_per_bar as i64;
        for f in 0..frames {
            if !self.playing {
                break;
            }
            let base = f * ch;

            // Count-in: one bar of click before the audio starts.
            if self.count_left > 0 {
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
                let cut = from > 0 || !(self.fpos == self.section(self.item).len() && self.contiguous(self.item, t));
                self.pending = None;
                self.enter(t, cut);
                self.fpos = from.min(self.section(t).len().saturating_sub(1));
            }
            if self.fpos >= self.section(self.item).len() {
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
            if let Some(left) = self.stopping {
                if left == 0 {
                    self.playing = false;
                    self.stopping = None;
                    break;
                }
                master = left as f32 / fade;
                self.stopping = Some(left - 1);
            }
            let mut env = master;
            if let Some(d) = self.frames_to_cut() {
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
                let gg = *g * env;
                let (l, r) = self.stems[i].frame(src);
                Self::write(out, base, ch, self.mix[i].route, l * gg, r * gg);
            }

            // Click, locked to the song's own bar grid.
            let rel = src as f64 - self.song.offset as f64;
            let beat = (rel / fpb).floor() as i64;
            if self.click_on && rel >= 0.0 && self.last_beat != Some(beat) && rel - beat as f64 * fpb < 1.0 {
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
        let beat = ((src - self.song.offset as f64) / self.song.frames_per_beat()).floor() as i64;
        let bpb = self.song.beats_per_bar as i64;
        Snapshot {
            playing: self.playing,
            counting_in: self.count_left > 0,
            finished: self.finished,
            item: it,
            section: self.arr.items[it].section,
            frame_in_section: self.fpos,
            section_frames: sec.len(),
            bar: beat.div_euclid(bpb) + 1,
            beat: beat.rem_euclid(bpb) as u32 + 1,
            pending: self.pending.as_ref().map(|p| p.target),
            frames_to_jump: self.pending.as_ref().map(|p| p.at.saturating_sub(self.abs)),
            vamp: self.vamp,
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
}

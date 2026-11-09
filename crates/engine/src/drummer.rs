//! The Drummer: a live drummer for flow moments, played on a sampled acoustic kit.
//!
//! It plays like a person, not a loop: every hit picks the recorded hit closest to how hard
//! it's played (never the same one twice in a row) and matches its level, timing breathes a
//! few milliseconds around the grid (the kick tightest, the snare a touch behind), the
//! hi-hat chokes when it closes, and the parts change with the intensity, from nothing to
//! cinematic swells to a full groove. Fills, builds, drops and hits land on the bar.
//!
//! Real-time: `process` never allocates, locks or blocks. Voices and the kit are made
//! before they reach the audio thread.

use crate::model::Route;
use std::sync::Arc;

/// The kit's pieces, in the order of `Kit::pieces`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum P {
    Kick,
    KickTight,
    Snare,
    Cross,
    TomHigh,
    TomMid,
    TomFloor,
    HatClosed,
    HatPedal,
    HatHalf,
    HatOpen,
    Ride,
    RideCrash,
    RideBell,
    CrashL,
    CrashR,
}

pub const PIECES: [&str; 16] = [
    "kick", "kick_tight", "snare", "cross_stick", "tom_high", "tom_mid", "tom_floor", "hat_closed", "hat_pedal", "hat_half", "hat_open", "ride", "ride_crash", "ride_bell", "crash_left", "crash_right",
];

impl P {
    fn is_hat(self) -> bool {
        matches!(self, P::HatClosed | P::HatPedal | P::HatHalf | P::HatOpen)
    }
    /// How far the hit wanders from the grid (ms) and where it sits (ms, + = behind).
    fn feel(self) -> (f32, f32) {
        match self {
            P::Kick | P::KickTight => (1.5, 0.0),
            P::Snare | P::Cross => (2.5, 3.0),
            p if p.is_hat() => (3.5, -1.0),
            P::Ride | P::RideBell => (3.0, -1.0),
            _ => (3.0, 0.0),
        }
    }
}

/// One recorded hit: stereo, interleaved, at the output rate; and its attack loudness.
pub struct Hit {
    pub data: Arc<[f32]>,
    pub rms: f32,
}

pub struct Piece {
    /// Softest first.
    pub hits: Vec<Hit>,
}

pub struct Kit {
    pub pieces: Vec<Piece>,
}

impl Kit {
    fn piece(&self, p: P) -> Option<&Piece> {
        self.pieces.get(p as usize).filter(|x| !x.hits.is_empty())
    }
}

/// How the drummer plays: intensity is 0 (nothing) to 1 (everything).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cue {
    /// A fill into the next bar.
    Fill,
    /// Two bars of snare building into a big downbeat.
    Build,
    /// Crash on the next downbeat, then down to the swells.
    Drop,
    /// Crash and kick on the next downbeat, carrying on.
    Hit,
    /// Crash and kick on the next downbeat, then nothing.
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DrumCmd {
    Start,
    /// Stop at once (a short fade, no crash).
    Halt,
    Intensity(f32),
    Tempo(f64),
    /// Glide to this tempo over this many bars.
    GlideTo(f64, u32),
    /// Beats per bar and whether it's compound (6/8, 12/8: beats in threes).
    Meter(u32, bool),
    HalfTime(bool),
    Cue(Cue),
    Gain(f32),
    Route(Route),
}

const VOICES: usize = 48;
/// A step is a sixteenth (or, in 6/8, a sixteenth of the eighth-note pulse).
const LOOKAHEAD_MS: f64 = 12.0;

#[derive(Clone, Copy)]
struct Voice {
    on: bool,
    piece: usize,
    hit: usize,
    /// Frames until it starts.
    wait: u32,
    pos: usize,
    gain: f32,
    /// Fading out (choked): gain step per frame.
    fade: f32,
}

const SILENT: Voice = Voice { on: false, piece: 0, hit: 0, wait: 0, pos: 0, gain: 0.0, fade: 0.0 };

/// A planned hit inside a step.
#[derive(Clone, Copy)]
struct Note {
    p: P,
    vel: f32,
}

/// Up to this many notes in one step.
const PER_STEP: usize = 8;

pub struct Drummer {
    kit: Option<Box<Kit>>,
    sr: f64,
    voices: [Voice; VOICES],
    last_hit: [usize; 16],
    rng: u64,
    running: bool,
    halting: f32,
    bpm: f64,
    glide: Option<(f64, f64, u64, u64)>, // from, to, start step, steps
    beats: u32,
    compound: bool,
    half: bool,
    intensity: f32,
    /// The level being played this bar (0..=5) and the intensity it started at.
    level: u8,
    gain: f32,
    route: Route,
    /// Step counter since start, and the frame (relative to now) of the next step.
    step: u64,
    next_step_in: f64,
    /// What's coming: a cue waiting for its bar, and how far into a running one.
    pending: Option<Cue>,
    active: Option<(Cue, u64)>, // cue, step it started at
    /// A big downbeat to play at the next bar line.
    downbeat: Option<f32>,
    /// After a stop crash: let it ring, play nothing.
    stopped: bool,
    gain_smooth: f32,
}

impl Default for Drummer {
    fn default() -> Self {
        Drummer::new(48_000)
    }
}

impl Drummer {
    pub fn new(sample_rate: u32) -> Self {
        Drummer {
            kit: None,
            sr: sample_rate as f64,
            voices: [SILENT; VOICES],
            last_hit: [usize::MAX; 16],
            rng: 0x9E3779B97F4A7C15,
            running: false,
            halting: 0.0,
            bpm: 72.0,
            glide: None,
            beats: 4,
            compound: false,
            half: false,
            intensity: 0.5,
            level: 3,
            gain: 1.0,
            route: Route::Pair(0),
            step: 0,
            next_step_in: 0.0,
            pending: None,
            active: None,
            downbeat: None,
            stopped: false,
            gain_smooth: 1.0,
        }
    }

    /// Hands the kit over (made off the audio thread). Returns the old one to drop elsewhere.
    pub fn set_kit(&mut self, kit: Box<Kit>) -> Option<Box<Kit>> {
        for v in self.voices.iter_mut() {
            v.on = false;
        }
        self.kit.replace(kit)
    }

    pub fn running(&self) -> bool {
        self.running
    }
    pub fn level(&self) -> u8 {
        self.level
    }
    pub fn bpm(&self) -> f64 {
        self.bpm
    }
    pub fn sounding(&self) -> bool {
        self.voices.iter().any(|v| v.on)
    }

    pub fn handle(&mut self, c: DrumCmd) {
        match c {
            DrumCmd::Start => {
                if !self.running {
                    self.running = true;
                    self.stopped = false;
                    self.halting = 0.0;
                    self.step = 0;
                    self.next_step_in = 0.0;
                    self.level = level_of(self.intensity);
                    self.active = None;
                    self.pending = None;
                    self.downbeat = None;
                }
            }
            DrumCmd::Halt => {
                self.running = false;
                self.halting = 1.0;
            }
            DrumCmd::Intensity(x) => self.intensity = x.clamp(0.0, 1.0),
            DrumCmd::Tempo(b) => {
                self.bpm = b.clamp(30.0, 240.0);
                self.glide = None;
            }
            DrumCmd::GlideTo(b, bars) => {
                let steps = (bars.max(1) * self.steps_per_bar()) as u64;
                self.glide = Some((self.bpm, b.clamp(30.0, 240.0), self.step, steps));
            }
            DrumCmd::Meter(beats, compound) => {
                self.beats = beats.clamp(2, 12);
                self.compound = compound;
            }
            DrumCmd::HalfTime(on) => self.half = on,
            DrumCmd::Cue(c) => {
                if self.running {
                    self.pending = Some(c);
                }
            }
            DrumCmd::Gain(g) => self.gain = g.clamp(0.0, 4.0),
            DrumCmd::Route(r) => self.route = r,
        }
    }

    /// Steps in a bar: sixteenths in 4/4 (16), sixteenths of the eighth pulse in 6/8 (12).
    fn steps_per_bar(&self) -> u32 {
        if self.compound {
            self.beats * 2
        } else {
            self.beats * 4
        }
    }
    /// Steps per beat as counted (a quarter in 4/4, an eighth in 6/8).
    fn steps_per_beat(&self) -> u32 {
        if self.compound {
            2
        } else {
            4
        }
    }
    fn step_frames(&self) -> f64 {
        self.sr * 60.0 / self.bpm / self.steps_per_beat() as f64
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Roughly normal, mean 0, sd 1.
    fn gauss(&mut self) -> f32 {
        (self.rand() + self.rand() + self.rand() + self.rand() - 2.0) * 1.73
    }

    /// Plays a hit `delay` frames from now (velocity 0..1).
    fn trigger(&mut self, p: P, vel: f32, delay: u32) {
        let Some(kit) = self.kit.as_deref() else { return };
        let Some(piece) = kit.piece(p) else { return };
        let vel = vel.clamp(0.02, 1.0);
        let r = {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            (self.rng >> 40) as f32 / (1u64 << 24) as f32
        };
        let n = piece.hits.len();
        let loudest = piece.hits[n - 1].rms;
        // Velocity to loudness: about 36 dB from the softest touch to the hardest.
        let target = loudest * 10f32.powf(36.0 * (vel - 1.0) / 20.0);
        let mut best = 0;
        for (i, h) in piece.hits.iter().enumerate() {
            if (h.rms - target).abs() < (piece.hits[best].rms - target).abs() {
                best = i;
            }
        }
        // A neighbour now and then, and never the same hit twice in a row.
        let mut pick = best;
        if n > 2 {
            if r < 0.33 && best > 0 {
                pick = best - 1;
            } else if r > 0.66 && best + 1 < n {
                pick = best + 1;
            }
            if pick == self.last_hit[p as usize] {
                pick = if pick + 1 < n { pick + 1 } else { pick - 1 };
            }
        }
        self.last_hit[p as usize] = pick;
        let h = &piece.hits[pick];
        let gain = (target / h.rms.max(1e-6)).clamp(0.25, 4.0);
        // The hi-hat closing chokes an open hat.
        if p.is_hat() && p != P::HatOpen {
            let fade = 1.0 / (0.012 * self.sr as f32);
            for v in self.voices.iter_mut() {
                if v.on && (v.piece == P::HatOpen as usize || v.piece == P::HatHalf as usize) && v.wait <= delay {
                    v.fade = fade;
                }
            }
        }
        // A free voice, else the quietest-sounding one.
        let slot = self.voices.iter().position(|v| !v.on).unwrap_or_else(|| {
            let mut q = 0;
            for (i, v) in self.voices.iter().enumerate() {
                if v.gain < self.voices[q].gain {
                    q = i;
                }
            }
            q
        });
        self.voices[slot] = Voice { on: true, piece: p as usize, hit: pick, wait: delay, pos: 0, gain, fade: 0.0 };
    }

    /// The notes for one step of the groove at a level.
    fn groove(&mut self, level: u8, s: u32, bar: u64, notes: &mut [Option<Note>; PER_STEP]) {
        let spb = self.steps_per_beat();
        let bar_steps = self.steps_per_bar();
        let beat = s / spb;
        let sub = s % spb;
        let on_beat = sub == 0;
        let eighth = if self.compound { sub == 0 } else { sub % 2 == 0 };
        let backbeat_beat = if self.compound {
            // 6/8: the backbeat is the second big pulse (beat 4 of 6).
            beat == self.beats / 2
        } else if self.half {
            beat == 2
        } else {
            beat % 2 == 1
        };
        let last_beat = beat == self.beats - 1;
        let mut push = |p: P, vel: f32| {
            if let Some(slot) = notes.iter_mut().find(|n| n.is_none()) {
                *slot = Some(Note { p, vel });
            }
        };
        let i = self.intensity;
        // Within a level, the intensity nudges how hard it's played.
        let lift = 0.85 + 0.3 * (i * 5.0 - (level.max(1) - 1) as f32).clamp(0.0, 1.0);
        match level {
            0 => {}
            1 => {
                // Swells: a soft kick and floor tom to open every 4 bars, then a floor-tom
                // roll and a ride-cymbal roll swelling into the next phrase.
                let phrase = bar % 4;
                if phrase == 0 && s == 0 {
                    push(P::Kick, 0.5 * lift);
                    push(P::TomFloor, 0.42 * lift);
                    push(P::RideCrash, 0.3 * lift);
                }
                if phrase == 2 && on_beat && beat == 0 {
                    push(P::TomFloor, 0.25 * lift);
                }
                if phrase == 3 {
                    let k = s as f32 / bar_steps as f32;
                    let dense = if k < 0.5 { sub % 2 == 0 } else { true };
                    if dense {
                        push(P::TomFloor, (0.12 + 0.4 * k * k) * lift);
                        push(P::Ride, (0.06 + 0.32 * k * k) * lift);
                    }
                }
            }
            2 => {
                // Heartbeat: kick on the big beats, floor tom pulsing, hat foot on the backbeat.
                if on_beat && (beat == 0 || (!self.compound && beat == 2 && !self.half)) {
                    push(P::Kick, 0.6 * lift);
                }
                if on_beat {
                    push(P::TomFloor, if beat == 0 { 0.36 } else { 0.26 } * lift);
                }
                if on_beat && backbeat_beat {
                    push(P::HatPedal, 0.6 * lift);
                }
            }
            3 => {
                // Light groove: cross-stick backbeat, closed hat eighths, a simple kick.
                let kick = if self.compound {
                    s == 0 || (s == 8 && bar % 2 == 1)
                } else if self.half {
                    s == 0 || s == 10
                } else {
                    s == 0 || s == 8 || (s == 10 && bar % 2 == 1)
                };
                if kick {
                    push(P::Kick, if s == 0 { 0.72 } else { 0.6 } * lift);
                }
                if on_beat && backbeat_beat {
                    push(P::Cross, 0.75 * lift);
                }
                if eighth {
                    push(P::HatClosed, if on_beat { 0.5 } else { 0.32 } * lift);
                }
            }
            4 => {
                // Groove: snare backbeat with ghost notes, hats eighths opening into beat 1.
                let kick = if self.compound {
                    s == 0 || s == 10 || (s == 4 && bar % 2 == 1)
                } else if self.half {
                    s == 0 || s == 3 || s == 10
                } else {
                    s == 0 || s == 6 || s == 8 || (s == 11 && bar % 2 == 0)
                };
                if kick {
                    push(P::Kick, if s == 0 { 0.85 } else { 0.72 } * lift);
                }
                if on_beat && backbeat_beat {
                    push(P::Snare, 0.82 * lift);
                } else if !self.compound && (s == 7 || s == 13 || (s == 15 && bar % 2 == 1)) {
                    push(P::Snare, 0.17 * lift); // ghosts
                }
                let open = last_beat && sub == spb / 2 && bar % 2 == 1;
                if open {
                    push(P::HatHalf, 0.55 * lift);
                } else if eighth {
                    push(P::HatClosed, if on_beat { 0.62 } else { 0.42 } * lift);
                }
            }
            _ => {
                // Full: four on the floor, big backbeat, crash-ride washing on every beat,
                // floor tom doubling the eighths.
                if on_beat && !self.compound || (self.compound && (s == 0 || s == 6)) {
                    push(P::Kick, if s == 0 { 0.95 } else { 0.85 } * lift);
                }
                if on_beat && backbeat_beat {
                    push(P::Snare, 0.95 * lift);
                }
                if on_beat {
                    push(P::RideCrash, if beat == 0 { 0.6 } else { 0.5 } * lift);
                }
                if eighth {
                    push(P::TomFloor, if on_beat { 0.5 } else { 0.36 } * lift);
                }
            }
        }
    }

    /// A fill over the last `beats` beats of the bar (s = step within the bar).
    fn fill(&mut self, level: u8, s: u32, notes: &mut [Option<Note>; PER_STEP]) -> bool {
        let spb = self.steps_per_beat();
        let bar = self.steps_per_bar();
        let len = if level >= 4 { 2 * spb } else { spb };
        if s < bar - len {
            return false;
        }
        let k = (s - (bar - len)) as f32 / len as f32;
        let mut push = |p: P, vel: f32| {
            if let Some(slot) = notes.iter_mut().find(|n| n.is_none()) {
                *slot = Some(Note { p, vel });
            }
        };
        if level <= 2 {
            push(P::TomFloor, 0.2 + 0.45 * k);
            if s % 2 == 0 {
                push(P::TomMid, 0.15 + 0.35 * k);
            }
        } else {
            let p = if k < 0.25 { P::Snare } else if k < 0.5 { P::TomHigh } else if k < 0.75 { P::TomMid } else { P::TomFloor };
            push(p, 0.55 + 0.35 * k);
            if s % spb == 0 {
                push(P::Kick, 0.7);
            }
        }
        true
    }

    /// A build over two bars: snare eighths then sixteenths, crescendo, kick on the beat.
    fn build(&mut self, into: u64, s: u32, notes: &mut [Option<Note>; PER_STEP]) {
        let bar = self.steps_per_bar() as u64;
        let pos = into * bar + s as u64; // 0 .. 2 bars
        let k = pos as f32 / (2 * bar) as f32;
        // It starts from where the groove was, not from nothing.
        let from = 0.2 + 0.08 * self.level as f32;
        let spb = self.steps_per_beat();
        let mut push = |p: P, vel: f32| {
            if let Some(slot) = notes.iter_mut().find(|n| n.is_none()) {
                *slot = Some(Note { p, vel });
            }
        };
        let dense = if into == 0 { s % 2 == 0 } else { true };
        if dense {
            push(P::Snare, from + (0.98 - from) * k * k);
        }
        if s % spb == 0 {
            push(P::Kick, from + 0.2 + (0.75 - from) * k);
        }
        if into == 1 && s >= self.steps_per_bar() - spb {
            push(P::TomFloor, 0.6 + 0.35 * k);
        }
    }

    /// Everything that plays on this step.
    fn plan(&mut self, notes: &mut [Option<Note>; PER_STEP]) {
        let bar_steps = self.steps_per_bar() as u64;
        let s = (self.step % bar_steps) as u32;
        let bar = self.step / bar_steps;
        if s == 0 {
            // Bar line: what was asked for comes in now.
            if let Some((Cue::Build, start)) = self.active {
                if (self.step - start) / bar_steps >= 2 {
                    self.active = None;
                    self.downbeat = Some(1.0);
                    self.intensity = self.intensity.max(0.95);
                    self.level = 5;
                }
            } else if self.active.is_some() {
                self.active = None;
            }
            if let Some(v) = self.downbeat.take() {
                notes[0] = Some(Note { p: P::CrashL, vel: v * 0.9 });
                notes[1] = Some(Note { p: P::Kick, vel: v });
                notes[2] = Some(Note { p: P::CrashR, vel: v * 0.7 });
            }
            match self.pending.take() {
                Some(Cue::Hit) => {
                    notes[0] = Some(Note { p: P::CrashL, vel: 0.85 });
                    notes[1] = Some(Note { p: P::Kick, vel: 0.9 });
                }
                Some(Cue::Drop) => {
                    notes[0] = Some(Note { p: P::CrashL, vel: 0.8 });
                    notes[1] = Some(Note { p: P::Kick, vel: 0.85 });
                    notes[2] = Some(Note { p: P::TomFloor, vel: 0.6 });
                    self.intensity = 0.25;
                    self.level = 1;
                    self.step = 0; // the swells' 4-bar phrase starts over from this hit
                    return;
                }
                Some(Cue::Stop) => {
                    notes[0] = Some(Note { p: P::CrashL, vel: 0.85 });
                    notes[1] = Some(Note { p: P::Kick, vel: 0.9 });
                    notes[2] = Some(Note { p: P::CrashR, vel: 0.65 });
                    self.stopped = true;
                    self.running = false;
                    return;
                }
                Some(Cue::Build) => {
                    self.active = Some((Cue::Build, self.step));
                }
                Some(Cue::Fill) => self.pending = Some(Cue::Fill), // fills are placed at the end of a bar
                None => {}
            }
            let target = level_of(self.intensity);
            if target > self.level + 1 && notes[0].is_none() {
                notes[0] = Some(Note { p: P::CrashL, vel: 0.75 });
            }
            self.level = target;
        }
        if let Some((Cue::Build, start)) = self.active {
            let into = (self.step - start) / bar_steps;
            self.build(into, s, notes);
            return;
        }
        // A fill asked for in time takes the end of this bar, else the next one's.
        if self.pending == Some(Cue::Fill) {
            let len = if self.level >= 4 { 2 * self.steps_per_beat() } else { self.steps_per_beat() };
            if s == self.steps_per_bar() - len {
                self.active = Some((Cue::Fill, 0));
                self.pending = None;
            }
        }
        if self.active == Some((Cue::Fill, 0)) {
            let lv = self.level;
            if self.fill(lv, s, notes) {
                if s == self.steps_per_bar() - 1 {
                    self.downbeat = Some(0.8);
                }
                return;
            }
        }
        let lv = self.level;
        self.groove(lv, s, bar, notes);
    }

    /// Adds the drummer into `out` (interleaved, `ch` channels).
    pub fn process(&mut self, out: &mut [f32], ch: usize) {
        let frames = out.len() / ch.max(1);
        // Steps that start within this block (plus a little lookahead for hits ahead of the grid).
        if self.running {
            let look = LOOKAHEAD_MS * self.sr / 1000.0;
            while self.next_step_in < frames as f64 + look && self.running {
                if let Some((from, to, start, steps)) = self.glide {
                    let k = ((self.step - start.min(self.step)) as f64 / steps as f64).min(1.0);
                    let e = k * k * (3.0 - 2.0 * k);
                    self.bpm = from + (to - from) * e;
                    if k >= 1.0 {
                        self.glide = None;
                    }
                }
                let mut notes = [None; PER_STEP];
                self.plan(&mut notes);
                let at = self.next_step_in;
                for n in notes.into_iter().flatten() {
                    let (wander, sit) = n.p.feel();
                    let ms = sit + wander * self.gauss();
                    let vel = n.vel * (1.0 + 0.05 * self.gauss());
                    let t = (at + ms as f64 * self.sr / 1000.0).max(0.0);
                    self.trigger(n.p, vel, t as u32);
                }
                self.step += 1;
                self.next_step_in += self.step_frames();
            }
            self.next_step_in -= frames as f64;
        }
        let gain_target = if self.halting > 0.0 { 0.0 } else { self.gain };
        let Some(kit) = self.kit.as_deref() else { return };
        let step_g = 1.0 / (0.005 * self.sr as f32);
        let (l_ch, r_ch) = match self.route {
            Route::Pair(n) => (n as usize, n as usize + 1),
            Route::Mono(n) => (n as usize, n as usize),
        };
        let mono = l_ch == r_ch;
        if r_ch >= ch {
            return;
        }
        for v in self.voices.iter_mut().filter(|v| v.on) {
            let data = &kit.pieces[v.piece].hits[v.hit].data;
            let len = data.len() / 2;
            let mut gs = self.gain_smooth;
            for f in 0..frames {
                if v.wait > 0 {
                    v.wait -= 1;
                    continue;
                }
                if v.pos >= len {
                    v.on = false;
                    break;
                }
                gs += (gain_target - gs).clamp(-step_g, step_g);
                let mut amp = v.gain * gs;
                if v.fade > 0.0 {
                    v.gain -= v.fade * v.gain.max(0.05);
                    if v.gain <= 0.0005 {
                        v.on = false;
                        break;
                    }
                    amp = v.gain * gs;
                }
                let l = data[v.pos * 2] * amp;
                let r = data[v.pos * 2 + 1] * amp;
                if mono {
                    out[f * ch + l_ch] += 0.5 * (l + r);
                } else {
                    out[f * ch + l_ch] += l;
                    out[f * ch + r_ch] += r;
                }
                v.pos += 1;
            }
        }
        // The smoothed gain moves once per block whether or not anything sounded.
        self.gain_smooth += (gain_target - self.gain_smooth).clamp(-step_g * frames as f32, step_g * frames as f32);
        if self.halting > 0.0 && self.gain_smooth <= 0.0005 {
            self.halting = 0.0;
            for v in self.voices.iter_mut() {
                v.on = false;
            }
            self.gain_smooth = self.gain;
        }
    }
}

/// The level (0 nothing, 1 swells, 2 heartbeat, 3 light, 4 groove, 5 full) for an intensity.
pub fn level_of(i: f32) -> u8 {
    if i < 0.04 {
        0
    } else {
        ((i * 5.0).ceil() as u8).clamp(1, 5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test kit: each piece is a short click at a pitch, three hits at rising loudness.
    fn kit(sr: u32) -> Box<Kit> {
        let pieces = (0..16)
            .map(|p| {
                let hits = (1..=3)
                    .map(|k| {
                        let amp = 0.2 * k as f32;
                        let n = (sr / 20) as usize;
                        let mut d = vec![0f32; n * 2];
                        for i in 0..n {
                            let s = (i as f32 * (100.0 + 40.0 * p as f32) * std::f32::consts::TAU / sr as f32).sin() * amp * (1.0 - i as f32 / n as f32);
                            d[i * 2] = s;
                            d[i * 2 + 1] = s;
                        }
                        Hit { data: d.into(), rms: amp * 0.6 }
                    })
                    .collect();
                Piece { hits }
            })
            .collect();
        Box::new(Kit { pieces })
    }

    fn render(d: &mut Drummer, seconds: f64, sr: u32) -> Vec<f32> {
        let mut all = vec![];
        let block = 256;
        let n = (seconds * sr as f64) as usize / block;
        for _ in 0..n {
            let mut b = vec![0f32; block * 2];
            d.process(&mut b, 2);
            all.extend_from_slice(&b);
        }
        all
    }

    fn onsets(x: &[f32], sr: u32) -> Vec<f64> {
        let mut v = vec![];
        let mut quiet = usize::MAX / 2;
        for f in 0..x.len() / 2 {
            let a = x[f * 2].abs();
            if a > 0.02 && quiet > (sr / 50) as usize {
                v.push(f as f64 / sr as f64);
            }
            quiet = if a > 0.02 { 0 } else { quiet + 1 };
        }
        v
    }

    #[test]
    fn it_keeps_time_at_the_tempo() {
        let sr = 48_000;
        let mut d = Drummer::new(sr);
        d.set_kit(kit(sr));
        d.handle(DrumCmd::Tempo(120.0));
        d.handle(DrumCmd::Intensity(0.5)); // level 3: kick on 1 and 3
        d.handle(DrumCmd::Start);
        let x = render(&mut d, 4.0, sr);
        let on = onsets(&x, sr);
        // Something every half-beat at least, and the first hit within the lookahead + feel.
        assert!(on.len() >= 12, "{on:?}");
        assert!(on[0] < 0.03, "{on:?}");
        // Beat 1 of bar 2 (2.0 s at 120 BPM) has a hit within 15 ms.
        assert!(on.iter().any(|t| (t - 2.0).abs() < 0.015 + LOOKAHEAD_MS / 1000.0), "{on:?}");
    }

    #[test]
    fn more_intensity_plays_more_and_louder() {
        let sr = 48_000;
        let energy = |i: f32| {
            let mut d = Drummer::new(sr);
            d.set_kit(kit(sr));
            d.handle(DrumCmd::Tempo(100.0));
            d.handle(DrumCmd::Intensity(i));
            d.handle(DrumCmd::Start);
            let x = render(&mut d, 4.8, sr);
            x.iter().map(|s| s * s).sum::<f32>()
        };
        let (a, b, c, z) = (energy(0.3), energy(0.7), energy(1.0), energy(0.0));
        assert_eq!(z, 0.0, "intensity 0 is silence");
        assert!(a < b && b < c, "{a} {b} {c}");
    }

    #[test]
    fn stop_crashes_on_the_next_bar_then_plays_nothing() {
        let sr = 48_000;
        let mut d = Drummer::new(sr);
        d.set_kit(kit(sr));
        d.handle(DrumCmd::Tempo(120.0));
        d.handle(DrumCmd::Intensity(0.8));
        d.handle(DrumCmd::Start);
        let _ = render(&mut d, 1.0, sr); // half a bar
        d.handle(DrumCmd::Cue(Cue::Stop));
        let x = render(&mut d, 4.0, sr);
        assert!(!d.running());
        let on = onsets(&x, sr);
        // The last hit is the stop on the bar line (1.0 s from here), nothing after.
        let last = *on.last().unwrap();
        assert!((last - 1.0).abs() < 0.03, "{on:?}");
    }

    #[test]
    fn halt_fades_out_without_a_click() {
        let sr = 48_000;
        let mut d = Drummer::new(sr);
        d.set_kit(kit(sr));
        d.handle(DrumCmd::Intensity(1.0));
        d.handle(DrumCmd::Start);
        let _ = render(&mut d, 1.0, sr);
        d.handle(DrumCmd::Halt);
        let x = render(&mut d, 0.5, sr);
        let jump = x.windows(4).map(|w| (w[2] - w[0]).abs()).fold(0f32, f32::max);
        assert!(jump < 0.2, "{jump}");
        assert!(x[x.len() / 2..].iter().all(|s| *s == 0.0), "silent after the fade");
    }

    #[test]
    fn levels_follow_the_intensity() {
        assert_eq!(level_of(0.0), 0);
        assert_eq!(level_of(0.1), 1);
        assert_eq!(level_of(0.5), 3);
        assert_eq!(level_of(1.0), 5);
    }
}

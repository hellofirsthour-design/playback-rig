//! A click with no song: start it any time (between songs, during announcements, a
//! rehearsal with no tracks). It runs on the audio thread next to the song, goes to the
//! click output, and stops by itself when a song starts so the in-ears never hear two.

use rig_engine::Route;

pub struct Metronome {
    pub on: bool,
    pub bpm: f64,
    pub beats_per_bar: u32,
    pub gain: f32,
    pub route: Route,
    /// Clicks per beat (1 quarters, 2 eighths, 3 triplets, 4 sixteenths).
    pub div: u32,
    sr: f64,
    /// Frames since the current beat started.
    pos: f64,
    beat: u32,
    phase: f32,
    step: f32,
    amp: f32,
    decay: f32,
    left: u32,
}

impl Metronome {
    pub fn new(sample_rate: u32) -> Self {
        Metronome { on: false, bpm: 120.0, beats_per_bar: 4, gain: 0.8, route: Route::Pair(2), div: 1, sr: sample_rate as f64, pos: 0.0, beat: 0, phase: 0.0, step: 0.0, amp: 0.0, decay: 0.0, left: 0 }
    }

    /// Starts (from beat 1) or stops.
    pub fn set(&mut self, on: bool, bpm: f64, beats_per_bar: u32) {
        let was = self.on;
        self.bpm = bpm.clamp(20.0, 400.0);
        self.beats_per_bar = beats_per_bar.clamp(1, 16);
        self.on = on;
        if on && !was {
            self.pos = 0.0;
            self.beat = 0;
            self.left = 0;
            self.trigger(2);
        }
    }

    /// 2 = downbeat, 1 = beat, 0 = a subdivision (softer, lower).
    fn trigger(&mut self, level: u8) {
        let (f, amp) = match level {
            2 => (1760.0, 0.9),
            1 => (1175.0, 0.9),
            _ => (880.0, 0.45),
        };
        self.phase = 0.0;
        self.step = std::f32::consts::TAU * f / self.sr as f32;
        self.amp = amp;
        self.decay = (-90.0 / self.sr as f32).exp();
        self.left = (0.05 * self.sr) as u32;
    }

    /// Adds the click into `out` (interleaved, `ch` wide). Doesn't allocate.
    pub fn process(&mut self, out: &mut [f32], ch: usize) {
        if !self.on && self.left == 0 {
            return;
        }
        let div = self.div.clamp(1, 4);
        let per_sub = self.sr * 60.0 / self.bpm / div as f64;
        let frames = out.len() / ch.max(1);
        for f in 0..frames {
            if self.on && self.pos >= per_sub {
                // `beat` counts subdivisions through the bar.
                self.pos -= per_sub;
                self.beat = (self.beat + 1) % (self.beats_per_bar.max(1) * div);
                self.trigger(if self.beat == 0 { 2 } else if self.beat % div == 0 { 1 } else { 0 });
            }
            if self.left > 0 {
                let v = self.phase.sin() * self.amp * self.gain;
                self.phase += self.step;
                self.amp *= self.decay;
                self.left -= 1;
                let base = f * ch;
                match self.route {
                    Route::Pair(c) if (c as usize) + 1 < ch => {
                        out[base + c as usize] += v;
                        out[base + c as usize + 1] += v;
                    }
                    Route::Mono(c) if (c as usize) < ch => out[base + c as usize] += v,
                    _ => {
                        out[base] += v;
                        out[base + 1.min(ch - 1)] += v;
                    }
                }
            }
            if self.on {
                self.pos += 1.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_on_every_beat_with_the_one_accented() {
        let mut m = Metronome::new(1000);
        m.route = Route::Mono(0);
        m.set(true, 120.0, 4); // 500 frames a beat
        let mut out = vec![0f32; 2 * 2100];
        for chunk in out.chunks_mut(2 * 256) {
            m.process(chunk, 2);
        }
        let ch0: Vec<f32> = out.iter().step_by(2).copied().collect();
        let starts: Vec<usize> = (1..ch0.len()).filter(|&i| ch0[i - 1] == 0.0 && ch0[i] != 0.0).map(|i| i - 1).collect();
        assert_eq!(starts, [0, 500, 1000, 1500, 2000]);
        // the one is higher-pitched: more zero crossings in its first 40 frames
        let zc = |s: usize| ch0[s..s + 40].windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count();
        assert!(zc(2000) > zc(1500));
        m.set(false, 120.0, 4);
        let mut out = vec![0f32; 2 * 1000];
        m.process(&mut out, 2);
        assert!(out[2 * 200..].iter().all(|&v| v == 0.0), "stops after the last click rings out");
    }
}

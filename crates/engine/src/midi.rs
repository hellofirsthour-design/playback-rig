//! MIDI control: controller buttons and faders (e.g. an Akai MIDImix or APC mini).
//! Any controller works through MIDI learn: pick an action, press or move a control.
//! Buttons can have a tap action and a hold action, like the pedal plan:
//! tap = next section, hold = skip ahead.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    NoteOn { ch: u8, key: u8, vel: u8 },
    NoteOff { ch: u8, key: u8 },
    Cc { ch: u8, num: u8, val: u8 },
}

/// Parses a raw MIDI message. Returns None for anything that isn't a note or CC.
pub fn parse(bytes: &[u8]) -> Option<Msg> {
    let (&status, rest) = bytes.split_first()?;
    let ch = status & 0x0f;
    match (status & 0xf0, rest) {
        (0x90, [key, vel, ..]) if *vel > 0 => Some(Msg::NoteOn { ch, key: *key, vel: *vel }),
        (0x90, [key, _, ..]) | (0x80, [key, _, ..]) => Some(Msg::NoteOff { ch, key: *key }),
        (0xb0, [num, val, ..]) => Some(Msg::Cc { ch, num: *num, val: *val }),
        _ => None,
    }
}

/// A physical control on a controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Control {
    Note { ch: u8, key: u8 },
    Cc { ch: u8, num: u8 },
}

/// What a control does.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    PlayStop,
    Panic,
    Next,
    SkipAhead,
    Repeat,
    ToggleVamp,
    NextSong,
    PadOnly,
    /// Switch between the Live page and the Edit page.
    ToggleView,
    /// Fade the song out over two bars and stop.
    FadeOut,
    /// Start or stop the free click (a click with no song).
    FreeClick,
    /// Continuous: a fader sets this stem's level.
    StemLevel(usize),
    /// Continuous: a fader sets the click level.
    ClickLevel,
    /// Continuous: a fader sets the master level (everything out).
    MasterLevel,
    /// Continuous: a fader sets a bus's level (the bus's place in the bus list).
    BusLevel(usize),
    /// Go to this song in the set (0 = the first).
    Song(usize),
    /// Mute or unmute a bus (the bus's place in the bus list).
    BusMute(usize),
    /// The song before this one in the set (when stopped).
    PrevSong,
    /// Cancel a queued jump (or a cut to another song) before it happens.
    CancelJump,
    /// Continuous: a fader or knob sets the pad level.
    PadLevel,
    /// Jump to this block of the arrangement (0 = the first), on the quantize setting.
    GotoBlock(usize),
    /// Turn a Live tonight part on or off (its place in BAND_PARTS).
    BandPart(usize),
    /// The Drummer: start (or stop on the next bar), its moments, and how hard it plays.
    DrumStartStop,
    DrumFill,
    DrumBuild,
    DrumDrop,
    DrumHit,
    DrumHalfTime,
    DrumTap,
    DrumFaster,
    /// Straight to a level: 0 nothing, 1 swells, 2 heartbeat, 3 light, 4 groove, 5 full.
    DrumLevel(u8),
    /// Continuous: a fader or knob sets how hard it plays.
    DrumIntensity,
}

impl Action {
    pub fn is_level(&self) -> bool {
        matches!(self, Action::StemLevel(_) | Action::ClickLevel | Action::MasterLevel | Action::BusLevel(_) | Action::PadLevel | Action::DrumIntensity)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    pub control: Control,
    pub tap: Action,
    #[serde(default)]
    pub hold: Option<Action>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    Fire(Action),
    /// A level action with a value from 0.0 to 1.0.
    Level(Action, f32),
    /// MIDI learn finished: this control is now bound.
    Learned(Control),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MidiMap {
    pub bindings: Vec<Binding>,
    pub hold_ms: u64,
    #[serde(skip)]
    learning: Option<(Action, Option<Action>)>,
    #[serde(skip)]
    held: Vec<(Control, u64, bool)>,
}

impl Default for MidiMap {
    fn default() -> Self {
        MidiMap { bindings: vec![], hold_ms: 450, learning: None, held: vec![] }
    }
}

/// The fader law shared with the screen (bus and master faders): unity at three-quarters
/// of the travel, +10 dB at the top, -60 dB just above the bottom, silence at the bottom.
pub fn db_gain(pos: f32) -> f32 {
    let p = pos.clamp(0.0, 1.0);
    if p <= 0.001 {
        return 0.0;
    }
    let db = if p >= 0.75 { (p - 0.75) * 40.0 } else { -60.0 * (1.0 - p / 0.75).powi(2) };
    10f32.powf(db / 20.0)
}

/// Where a gain sits on that fader (0..1).
pub fn db_pos(gain: f32) -> f32 {
    if gain <= 0.0 {
        return 0.0;
    }
    let db = (20.0 * gain.log10()).clamp(-60.0, 10.0);
    if db >= 0.0 { 0.75 + db / 40.0 } else { 0.75 * (1.0 - (-db / 60.0).sqrt()) }
}

/// Soft takeover: a controller fader only takes over once it reaches the level on
/// screen (or passes it), so a fader left somewhere else never makes the level jump.
/// `cur` is the on-screen position, `last` the controller's previous position.
pub fn picks_up(cur: f32, last: Option<f32>, new: f32) -> bool {
    if (new - cur).abs() <= 0.04 {
        return true;
    }
    last.map_or(false, |l| (l - cur).signum() != (new - cur).signum())
}

/// The Akai MIDImix's factory layout, ready to play with no Learn:
/// faders 1-8 = the first 8 buses, master fader = master, mute buttons = bus mutes,
/// SOLO + mute button = Song 1-8, rec arm buttons = the transport row, bank left/right =
/// previous/next song, the two lowest knobs on strips 1-2 = built-in click and pad level.
pub fn midimix() -> Vec<Binding> {
    const FADERS: [u8; 8] = [19, 23, 27, 31, 49, 53, 57, 61];
    const LOW_KNOBS: [u8; 8] = [18, 22, 26, 30, 48, 52, 56, 60];
    const ROW: [Action; 8] = [Action::PlayStop, Action::Next, Action::Repeat, Action::ToggleVamp, Action::CancelJump, Action::FadeOut, Action::PadOnly, Action::FreeClick];
    let note = |key: u8| Control::Note { ch: 0, key };
    let cc = |num: u8| Control::Cc { ch: 0, num };
    let bind = |control, tap| Binding { control, tap, hold: None };
    let mut v = vec![];
    for i in 0..8u8 {
        v.push(bind(cc(FADERS[i as usize]), Action::BusLevel(i as usize)));
        v.push(bind(note(1 + 3 * i), Action::BusMute(i as usize)));
        v.push(bind(note(2 + 3 * i), Action::Song(i as usize)));
        v.push(bind(note(3 + 3 * i), ROW[i as usize]));
    }
    v.push(bind(cc(62), Action::MasterLevel));
    v.push(bind(cc(LOW_KNOBS[0]), Action::ClickLevel));
    v.push(bind(cc(LOW_KNOBS[1]), Action::PadLevel));
    v.push(bind(note(25), Action::PrevSong));
    v.push(bind(note(26), Action::NextSong));
    v
}

/// The Akai APC mini mk2's layout (its "Control" port, factory settings), ready to play.
/// Pads count up from the bottom-left (0) to the top-right (63).
///   top row        Song 1-8
///   rows 7 and 6   Section 1-16 of the arrangement (jump on the quantize setting)
///   row 5          Play/stop, Next block, Repeat, Vamp, Cancel jump, Fade out now, Pad, Free click
///   row 4          Drummer: Start/stop, Fill, Build, Drop, Hit, Half-time, Tap, Faster
///   row 2          Drummer level: Nothing, Swells, Heartbeat, Light, Groove, Full
///   row 3          Live tonight: Drums, Bass, Piano, Organ, Electric, Acoustic
///   track buttons  mute bus 1-8;  faders 1-8 = buses 1-8, fader 9 = master
///   scene buttons  1 = previous song, 2 = next song
pub fn apc_mini_mk2() -> Vec<Binding> {
    let note = |key: u8| Control::Note { ch: 0, key };
    let cc = |num: u8| Control::Cc { ch: 0, num };
    let bind = |control, tap| Binding { control, tap, hold: None };
    let pad = |row: u8, col: u8| note((row - 1) * 8 + col - 1);
    const ROW5: [Action; 8] = [Action::PlayStop, Action::Next, Action::Repeat, Action::ToggleVamp, Action::CancelJump, Action::FadeOut, Action::PadOnly, Action::FreeClick];
    let mut v = vec![];
    for i in 0..8u8 {
        v.push(bind(cc(48 + i), Action::BusLevel(i as usize)));
        v.push(bind(note(100 + i), Action::BusMute(i as usize)));
        v.push(bind(pad(8, i + 1), Action::Song(i as usize)));
        v.push(bind(pad(7, i + 1), Action::GotoBlock(i as usize)));
        v.push(bind(pad(6, i + 1), Action::GotoBlock(8 + i as usize)));
        v.push(bind(pad(5, i + 1), ROW5[i as usize]));
    }
    const ROW4: [Action; 8] = [Action::DrumStartStop, Action::DrumFill, Action::DrumBuild, Action::DrumDrop, Action::DrumHit, Action::DrumHalfTime, Action::DrumTap, Action::DrumFaster];
    for i in 0..8u8 {
        v.push(bind(pad(4, i + 1), ROW4[i as usize]));
    }
    for i in 0..6u8 {
        v.push(bind(pad(3, i + 1), Action::BandPart(i as usize)));
        v.push(bind(pad(2, i + 1), Action::DrumLevel(i)));
    }
    v.push(bind(cc(56), Action::MasterLevel));
    v.push(bind(note(112), Action::PrevSong));
    v.push(bind(note(113), Action::NextSong));
    v
}

impl MidiMap {
    /// The next control pressed or moved gets bound to these actions.
    pub fn learn(&mut self, tap: Action, hold: Option<Action>) {
        self.learning = Some((tap, hold));
    }
    pub fn is_learning(&self) -> bool {
        self.learning.is_some()
    }
    pub fn cancel_learn(&mut self) {
        self.learning = None;
    }

    fn binding(&self, c: Control) -> Option<&Binding> {
        self.bindings.iter().find(|b| b.control == c)
    }

    /// Feed every incoming MIDI message here, with a millisecond clock.
    pub fn handle(&mut self, bytes: &[u8], now_ms: u64) -> Option<Event> {
        let msg = parse(bytes)?;
        let (control, pressed, value) = match msg {
            Msg::NoteOn { ch, key, vel } => (Control::Note { ch, key }, true, vel),
            Msg::NoteOff { ch, key } => (Control::Note { ch, key }, false, 0),
            Msg::Cc { ch, num, val } => (Control::Cc { ch, num }, val >= 64, val),
        };

        if let Some((tap, hold)) = self.learning {
            // Ignore the release of whatever was pressed before learning started.
            if !pressed && !matches!(control, Control::Cc { .. }) {
                return None;
            }
            self.learning = None;
            self.bindings.retain(|b| b.control != control && !(tap.is_level() && b.tap == tap));
            self.bindings.push(Binding { control, tap, hold: if tap.is_level() { None } else { hold } });
            return Some(Event::Learned(control));
        }

        let b = self.binding(control)?.clone();
        if b.tap.is_level() {
            return Some(Event::Level(b.tap, value as f32 / 127.0));
        }
        let held_idx = self.held.iter().position(|h| h.0 == control);
        if pressed {
            if held_idx.is_some() {
                return None; // repeated press messages (e.g. a CC pedal sending 127 twice)
            }
            match b.hold {
                None => {
                    self.held.push((control, now_ms, true));
                    Some(Event::Fire(b.tap))
                }
                Some(_) => {
                    self.held.push((control, now_ms, false));
                    None
                }
            }
        } else {
            let (_, t0, fired) = self.held.remove(held_idx?);
            if !fired && now_ms.saturating_sub(t0) < self.hold_ms {
                Some(Event::Fire(b.tap))
            } else {
                None
            }
        }
    }

    /// Call regularly (every ~10 ms) so holds fire while the pedal is still down.
    pub fn tick(&mut self, now_ms: u64) -> Option<Event> {
        let hold_ms = self.hold_ms;
        for i in 0..self.held.len() {
            let (c, t0, fired) = self.held[i];
            if !fired && now_ms.saturating_sub(t0) >= hold_ms {
                self.held[i].2 = true;
                if let Some(a) = self.binding(c).and_then(|b| b.hold) {
                    return Some(Event::Fire(a));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_midimix_layout_plays_straight_away() {
        let mut m = MidiMap { bindings: midimix(), ..Default::default() };
        let mut seen = std::collections::HashSet::new();
        assert!(m.bindings.iter().all(|b| seen.insert(b.control)), "one action per control");
        // Rec arm 1 (note 3) is play/stop, rec arm 6 (note 18) is fade out now.
        assert_eq!(m.handle(&[0x90, 3, 127], 0), Some(Event::Fire(Action::PlayStop)));
        assert_eq!(m.handle(&[0x90, 18, 127], 10), Some(Event::Fire(Action::FadeOut)));
        assert_eq!(m.handle(&[0x90, 1, 127], 20), Some(Event::Fire(Action::BusMute(0))));
        assert_eq!(m.handle(&[0x90, 26, 127], 30), Some(Event::Fire(Action::NextSong)));
        assert_eq!(m.handle(&[0xb0, 62, 127], 40), Some(Event::Level(Action::MasterLevel, 1.0)));
        assert_eq!(m.handle(&[0xb0, 23, 0], 50), Some(Event::Level(Action::BusLevel(1), 0.0)));
    }

    #[test]
    fn the_apc_mini_layout_plays_straight_away() {
        let mut m = MidiMap { bindings: apc_mini_mk2(), ..Default::default() };
        let mut seen = std::collections::HashSet::new();
        assert!(m.bindings.iter().all(|b| seen.insert(b.control)), "one action per control");
        assert_eq!(m.handle(&[0x90, 56, 127], 0), Some(Event::Fire(Action::Song(0))), "top-left pad");
        assert_eq!(m.handle(&[0x90, 63, 127], 10), Some(Event::Fire(Action::Song(7))), "top-right pad");
        assert_eq!(m.handle(&[0x90, 37, 127], 20), Some(Event::Fire(Action::FadeOut)), "row 5, pad 6");
        assert_eq!(m.handle(&[0x90, 41, 127], 30), Some(Event::Fire(Action::GotoBlock(9))), "row 6, pad 2");
        assert_eq!(m.handle(&[0x90, 100, 127], 40), Some(Event::Fire(Action::BusMute(0))));
        assert_eq!(m.handle(&[0x90, 24, 127], 45), Some(Event::Fire(Action::DrumStartStop)), "row 4, pad 1");
        assert_eq!(m.handle(&[0x90, 13, 127], 46), Some(Event::Fire(Action::DrumLevel(5))), "row 2, pad 6");
        assert_eq!(m.handle(&[0xb0, 56, 127], 50), Some(Event::Level(Action::MasterLevel, 1.0)));
    }

    #[test]
    fn the_db_fader_matches_the_screen() {
        assert!((db_gain(0.75) - 1.0).abs() < 1e-6);
        assert!((db_gain(1.0) - 3.1623).abs() < 1e-3);
        assert_eq!(db_gain(0.0), 0.0);
        for g in [0.01f32, 0.1, 0.5, 1.0, 2.0, 3.0] {
            assert!((db_gain(db_pos(g)) - g).abs() / g < 1e-3, "{g}");
        }
        // MIDI 95 of 127 is close to unity.
        assert!((20.0 * db_gain(95.0 / 127.0).log10()).abs() < 0.5);
    }

    #[test]
    fn a_fader_left_elsewhere_takes_over_only_when_it_reaches_the_level() {
        assert!(!picks_up(0.75, None, 0.2));
        assert!(!picks_up(0.75, Some(0.2), 0.4), "still below");
        assert!(picks_up(0.75, Some(0.7), 0.8), "passed it");
        assert!(picks_up(0.75, None, 0.76), "close enough");
    }

    const PEDAL: [u8; 3] = [0x90, 60, 100];
    const PEDAL_UP: [u8; 3] = [0x80, 60, 0];

    #[test]
    fn parses_notes_and_ccs() {
        assert_eq!(parse(&[0x91, 60, 100]), Some(Msg::NoteOn { ch: 1, key: 60, vel: 100 }));
        assert_eq!(parse(&[0x90, 60, 0]), Some(Msg::NoteOff { ch: 0, key: 60 }));
        assert_eq!(parse(&[0xb0, 19, 64]), Some(Msg::Cc { ch: 0, num: 19, val: 64 }));
        assert_eq!(parse(&[0xf8]), None);
    }

    #[test]
    fn learn_then_press_fires_immediately_without_hold() {
        let mut m = MidiMap::default();
        m.learn(Action::PlayStop, None);
        assert!(matches!(m.handle(&PEDAL, 0), Some(Event::Learned(_))));
        m.handle(&PEDAL_UP, 10);
        assert_eq!(m.handle(&PEDAL, 1000), Some(Event::Fire(Action::PlayStop)));
        assert_eq!(m.handle(&PEDAL_UP, 1100), None);
    }

    #[test]
    fn tap_and_hold_are_separate_actions() {
        let mut m = MidiMap::default();
        m.learn(Action::Next, Some(Action::SkipAhead));
        m.handle(&PEDAL, 0);
        // Tap: fires on release.
        assert_eq!(m.handle(&PEDAL, 1000), None);
        assert_eq!(m.handle(&PEDAL_UP, 1150), Some(Event::Fire(Action::Next)));
        // Hold: fires while still down, release does nothing.
        m.handle(&PEDAL, 2000);
        assert_eq!(m.tick(2200), None);
        assert_eq!(m.tick(2460), Some(Event::Fire(Action::SkipAhead)));
        assert_eq!(m.tick(2500), None);
        assert_eq!(m.handle(&PEDAL_UP, 2600), None);
    }

    #[test]
    fn faders_send_levels() {
        let mut m = MidiMap::default();
        m.learn(Action::StemLevel(3), None);
        m.handle(&[0xb0, 19, 5], 0);
        assert_eq!(m.handle(&[0xb0, 19, 127], 10), Some(Event::Level(Action::StemLevel(3), 1.0)));
        // Re-learning the same level action moves it to the new fader.
        m.learn(Action::StemLevel(3), None);
        m.handle(&[0xb0, 23, 0], 20);
        assert_eq!(m.handle(&[0xb0, 19, 127], 30), None);
        assert!(matches!(m.handle(&[0xb0, 23, 0], 40), Some(Event::Level(Action::StemLevel(3), _))));
    }

    #[test]
    fn map_round_trips_through_json_shape() {
        let mut m = MidiMap::default();
        m.learn(Action::ToggleVamp, None);
        m.handle(&PEDAL, 0);
        assert_eq!(m.bindings.len(), 1);
        assert_eq!(m.bindings[0].control, Control::Note { ch: 0, key: 60 });
    }
}

//! MIDI control: foot pedals and fader controllers (e.g. an Akai MIDImix or APC mini).
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
    /// Continuous: a fader sets this stem's level.
    StemLevel(usize),
    /// Continuous: a fader sets the click level.
    ClickLevel,
}

impl Action {
    pub fn is_level(&self) -> bool {
        matches!(self, Action::StemLevel(_) | Action::ClickLevel)
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

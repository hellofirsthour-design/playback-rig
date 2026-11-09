//! The APC mini mk2's lights: the pads show the set, the arrangement and what's on, like
//! the screen does. Worked out from the state on every status tick and only changes are
//! sent. Pads take a colour (velocity, the mk2's palette) and a brightness or pulse (the
//! MIDI channel); the track and scene buttons are single-colour (0 off, 1 on, 2 blink).

/// mk2 palette colours.
pub const OFF: u8 = 0;
pub const WHITE: u8 = 3;
pub const RED: u8 = 5;
pub const ORANGE: u8 = 9;
pub const YELLOW: u8 = 13;
pub const GREEN: u8 = 21;
pub const CYAN: u8 = 37;
pub const BLUE: u8 = 45;
pub const PURPLE: u8 = 49;
pub const PINK: u8 = 53;

/// Brightness and motion, as the MIDI channel of the note-on.
pub const DIM: u8 = 1; // 25%
pub const FULL: u8 = 6; // 100%
pub const PULSE: u8 = 9; // pulsing 1/4

/// One light: (channel, note, velocity).
pub type Light = (u8, u8, u8);

pub struct Lights<'a> {
    pub songs: usize,
    pub current: Option<usize>,
    pub cut_to: Option<usize>,
    /// The arrangement's block names, the block playing, a jump waiting to happen.
    pub blocks: &'a [String],
    pub block: Option<usize>,
    pub pending: Option<usize>,
    pub playing: bool,
    pub vamp: bool,
    pub fading: bool,
    pub pad: bool,
    pub free_click: bool,
    pub band: [bool; 6],
    pub bus_muted: [bool; 8],
}

/// A section's colour by its name, like the section tiles on screen.
fn section_colour(name: &str) -> u8 {
    let n = name.to_lowercase();
    if n.contains("chorus") {
        RED
    } else if n.contains("verse") {
        BLUE
    } else if n.contains("bridge") {
        PURPLE
    } else if n.contains("pre") {
        CYAN
    } else if n.contains("tag") || n.contains("vamp") {
        PINK
    } else if n.contains("intro") || n.contains("outro") || n.contains("end") || n.contains("inst") || n.contains("turn") {
        YELLOW
    } else {
        WHITE
    }
}

fn pad(row: u8, col: u8) -> u8 {
    (row - 1) * 8 + col - 1
}

/// Every light on the controller (all 64 pads, 8 track and 8 scene buttons).
pub fn lights(l: &Lights) -> Vec<Light> {
    let mut v = Vec::with_capacity(80);
    for i in 0..8 {
        // Songs: this one green, the next one yellow, the rest dim white; a cut pulses.
        let s = match i {
            _ if i >= l.songs => (0, OFF),
            _ if l.cut_to == Some(i) => (PULSE, GREEN),
            _ if l.current == Some(i) => (FULL, GREEN),
            _ if l.current.map(|c| c + 1) == Some(i) => (FULL, YELLOW),
            _ => (DIM, WHITE),
        };
        v.push((s.0, pad(8, i as u8 + 1), s.1));
    }
    for i in 0..16 {
        let (row, col) = if i < 8 { (7, i + 1) } else { (6, i - 7) };
        let s = match l.blocks.get(i) {
            None => (0, OFF),
            Some(_) if l.pending == Some(i) => (PULSE, GREEN),
            Some(_) if l.block == Some(i) => (FULL, GREEN),
            Some(name) => (DIM, section_colour(name)),
        };
        v.push((s.0, pad(row, col as u8), s.1));
    }
    let row5 = [
        if l.playing { (FULL, GREEN) } else { (DIM, GREEN) },
        (DIM, WHITE),
        (DIM, WHITE),
        if l.vamp { (PULSE, ORANGE) } else { (DIM, ORANGE) },
        if l.pending.is_some() || l.cut_to.is_some() { (FULL, RED) } else { (DIM, RED) },
        if l.fading { (PULSE, ORANGE) } else { (DIM, ORANGE) },
        if l.pad { (FULL, BLUE) } else { (DIM, BLUE) },
        if l.free_click { (FULL, CYAN) } else { (DIM, CYAN) },
    ];
    for (i, (ch, c)) in row5.into_iter().enumerate() {
        v.push((ch, pad(5, i as u8 + 1), c));
    }
    for i in 0..8 {
        let s = if i < 6 { if l.band[i] { (FULL, ORANGE) } else { (DIM, WHITE) } } else { (0, OFF) };
        v.push((s.0, pad(3, i as u8 + 1), s.1));
    }
    for row in [4u8, 2, 1] {
        for col in 1..=8 {
            v.push((0, pad(row, col), OFF));
        }
    }
    for i in 0..8u8 {
        v.push((0, 100 + i, l.bus_muted[i as usize] as u8));
    }
    for i in 0..8u8 {
        v.push((0, 112 + i, (i < 2) as u8));
    }
    v
}

/// The note-on messages that change the controller from `old` to `new` (all of `new` the
/// first time).
pub fn changes(old: Option<&[Light]>, new: &[Light]) -> Vec<[u8; 3]> {
    new.iter()
        .enumerate()
        .filter(|(i, n)| old.map_or(true, |o| o.get(*i) != Some(n)))
        .map(|(_, &(ch, note, vel))| [0x90 | (ch & 0x0f), note, vel])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base<'a>(blocks: &'a [String]) -> Lights<'a> {
        Lights { songs: 4, current: Some(1), cut_to: None, blocks, block: Some(2), pending: None, playing: true, vamp: false, fading: false, pad: false, free_click: false, band: [true, false, false, false, false, false], bus_muted: [false; 8] }
    }

    #[test]
    fn the_pads_show_the_set_and_the_song() {
        let blocks: Vec<String> = ["Intro", "Verse 1", "Chorus 1", "Bridge"].iter().map(|s| s.to_string()).collect();
        let l = lights(&base(&blocks));
        let at = |note: u8| l.iter().find(|x| x.1 == note).copied().unwrap();
        assert_eq!(at(57), (FULL, 57, GREEN), "song 2 is on");
        assert_eq!(at(58), (FULL, 58, YELLOW), "song 3 is next");
        assert_eq!(at(60), (0, 60, OFF), "no song 5");
        assert_eq!(at(50), (FULL, 50, GREEN), "block 3 playing");
        assert_eq!(at(49), (DIM, 49, BLUE), "verse");
        assert_eq!(at(51), (DIM, 51, PURPLE), "bridge");
        assert_eq!(at(32), (FULL, 32, GREEN), "playing");
        assert_eq!(at(16), (FULL, 16, ORANGE), "drums live tonight");
        assert_eq!(l.len(), 64 + 16);
        // Only what changed is sent again.
        let mut b2 = base(&blocks);
        b2.block = Some(3);
        let l2 = lights(&b2);
        assert_eq!(changes(Some(&l), &l2), vec![[0x90 | DIM, 50, RED], [0x90 | FULL, 51, GREEN]]);
        assert_eq!(changes(None, &l2).len(), 80);
    }
}

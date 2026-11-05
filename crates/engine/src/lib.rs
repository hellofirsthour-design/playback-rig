//! Playback Rig engine.
//!
//! - [`model`]: songs, sections, stems and arrangements (plain data).
//! - [`engine`]: the real-time player. Quantized jumps, vamp, repeat, per-block
//!   stem mutes, multi-output routing, click and count-in.
//! - [`midi`]: pedal and fader-controller mapping with MIDI learn and tap/hold.

pub mod engine;
pub mod midi;
pub mod model;

pub use engine::{Command, Engine, Quantize, Snapshot};
pub use model::{beat_pos, beat_time, ArrItem, Arrangement, Route, Section, Song, StemAudio, StemInfo, Trim};

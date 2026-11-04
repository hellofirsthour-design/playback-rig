# Playback Rig

Backing-track playback for worship, built for a music director who plays while running tracks. It runs the setlist the way MultiTracks Playback does and lets you arrange songs the way Ableton's arrangement view does. Control is hands-free from foot pedals and a fader controller.

Product decisions are in [`docs/product.md`](docs/product.md).

## What's here

| Path | What it is |
|---|---|
| `crates/engine` | The real-time playback engine in Rust. It plays arrangements (reordered and repeated sections, stems muted per block), makes quantized jumps (now, next bar, end of section), handles next, repeat and vamp, routes to any output of a multi-channel interface, and generates the click and count-in. It also maps pedals and faders with MIDI learn and tap/hold. Its audio thread never allocates or blocks. |
| `crates/rig-render` | Offline renderer. It plays an arrangement through the engine and writes a multi-channel WAV, so the engine can be tested without hardware. |
| `prototype/index.html` | Browser prototype of the live screen: setlist, section pads, overview, mixer. Open it in Chrome. |

## Run it

The test suite:

```bash
cargo test
```

Render a rearranged demo song to a 10-channel WAV. This one drops the drums in verse 1, vamps the bridge, and puts stems on outputs 1-2 and the click on 3-4:

```bash
cargo run --release -p rig-render -- --arrangement "Intro,Chorus,Verse 1,Chorus,Chorus,Bridge,Outro" --mute "Verse 1=Drums" --vamp-at 6 --channels 10 --out demo.wav
```

## How the engine works

- **Arrangement.** An arrangement is a list of blocks. Each block points at a section of the original song and can mute stems for that block. Repeats are repeated blocks.
- **Jumps.** Every jump is scheduled for a known frame: the next downbeat, the end of the block, or about 5 ms out. That's how jumps land exactly on the bar.
- **Fades.** When the audio cuts to a different place, it gets a 5 ms fade out and a 5 ms fade in. Sections that follow each other in the original audio play straight through with no fade.
- **Click.** The click is locked to the song's own bar grid, so it stays right after any jump.
- **Threading.** The host calls `Engine::handle(Command)` and `Engine::process(&mut [f32])` on the audio thread and reads `Engine::snapshot()` to update the screen.

## Next

The Mac app shell (Tauri): choosing the audio interface, streaming stems from disk, pedal and controller input, then the Build mode and Live mode screens. See the build order in `docs/product.md`.

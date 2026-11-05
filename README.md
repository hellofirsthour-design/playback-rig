# Playback Rig

Backing-track playback for worship, built for a music director who plays while running tracks. It runs the setlist the way MultiTracks Playback does and lets you arrange songs the way Ableton's arrangement view does. Control is hands-free from foot pedals and a fader controller.

Product decisions are in [`docs/product.md`](docs/product.md).

## What's here

| Path | What it is |
|---|---|
| `crates/engine` | The real-time playback engine in Rust. It plays arrangements (reordered and repeated sections, stems muted per block), makes quantized jumps (now, next bar, end of section), handles next, repeat and vamp, routes to any output of a multi-channel interface, and generates the click and count-in. It also maps pedals and faders with MIDI learn and tap/hold. Its audio thread never allocates or blocks. |
| `crates/rig-render` | Offline renderer. It plays an arrangement through the engine and writes a multi-channel WAV, so the engine can be tested without hardware. |
| `apps/desktop` | **The Mac app** (Tauri). The engine runs on the audio thread of your interface. The screen has a MultiTracks-style section rail (drag to rearrange, even while playing) over an Ableton-style arrangement view (a waveform lane per stem). Songs set themselves up from the Click and Guide stems. Pedals and controllers come in over MIDI. |
| `prototype/index.html` | The earlier browser prototype. Kept for reference. |

## Get the Mac app

Download the **.dmg** from the latest release: https://github.com/hellofirsthour-design/playback-rig/releases/latest. Open it and drag **Playback Rig** into Applications. One build runs on both Apple Silicon and Intel Macs.

The app isn't signed with an Apple developer certificate yet, so the first launch needs one extra step. Right-click the app, choose **Open**, then click **Open** again. If macOS says the app is damaged, run this once in Terminal:

```bash
xattr -cr "/Applications/Playback Rig.app"
```

**Updates are automatic from version 0.2 on.** The app checks for a new version at launch. When one exists, **Install & restart** appears at the bottom of the window. It only installs while playback is stopped. Settings, the set list, pedal mappings and song setups are kept. You can also check by hand in **Setup → Updates**.

### Publishing a new version

Raise `version` in `apps/desktop/src-tauri/tauri.conf.json` and push. CI then builds, signs and publishes a GitHub Release with the update file the app reads. A push that doesn't raise the version publishes nothing. Signing uses the `TAURI_SIGNING_PRIVATE_KEY` repository secret, and its public half is in `tauri.conf.json`. If that key is lost, installed apps can't accept updates signed with a new key, and everyone has to reinstall once.

## First-time setup on the church Mac

1. **Setup → Audio output:** pick the Behringer. Out 1-2 is front of house; Out 3-4 is the in-ears.
2. **Setup → Pedals and controller:** pick the Akai (or your pedal) as the MIDI input. Then click **Learn** next to each pedal row and press the pedal or button you want for it.
3. **Add songs:** choose one folder per song, each holding that song's stems (for example, unzipped MultiTracks downloads).
4. **Pick a song.** The first time, the app reads the Click and Guide stems to find the tempo and sections. The first song also downloads the speech model (about 150 MB, one time). Then drag sections in the rail to build the arrangement. Everything is saved in `rig-song.json` inside the song's folder. To fix a marker by hand, open **Song**.

## Develop

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

Desktop app tests and a local build. On Linux this needs the WebKitGTK and ALSA development packages:

```bash
cd apps/desktop/src-tauri && cargo test && npx @tauri-apps/cli@2 dev
```

## Next

Song transitions (key-following pads, segues), guide cues cut by section, and streaming from disk for very long sets. See the build order in `docs/product.md`.

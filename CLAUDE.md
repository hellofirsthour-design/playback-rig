# Playback Rig: notes for Claude

- Read `docs/product.md` before changing behaviour. It records decisions made with the user, and it changes when they change.
- The user is a worship music director who plays an instrument while running tracks. Live features must work from a pedal or controller, never only from the mouse.
- `crates/engine` is real-time code. Nothing in `Engine::process` may allocate, lock, block or do I/O. Any new behaviour in the engine needs a test in `engine.rs` that renders audio and checks sample values (the ramp-stem pattern in the tests).
- Run `cargo test` before every commit. CI runs the tests and a `rig-render` pass.
- The target is a Mac with a Behringer interface (10 outputs). This Linux container has no ALSA or WebKit, so Tauri, CoreAudio and MIDI device code can't be built here. Verify it in CI on `macos-latest` instead.
- Releases: raise `version` in `apps/desktop/src-tauri/tauri.conf.json` (and `Cargo.toml`) to ship an update to the user's Mac. The app updates itself from GitHub Releases. Don't raise the version for work that isn't ready to reach Sunday morning.

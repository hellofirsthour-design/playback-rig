# Product decisions

What we're building, decided with Brian (worship leader and music director). Update this file when a decision changes.

## The job

Backing-track playback for Sunday worship. It does what MultiTracks Playback does live (setlist, click, guide cues, section jumps) and gives you the arrangement flexibility of Ableton's arrangement view during the week. It is our own design, not a copy of either.

## Who runs it

The music director, **while playing an instrument**. On Sunday it's hands-free: foot pedal first, laptop second. Every live action must be possible from a pedal or a fader controller, and the screen has to be readable from about 6 feet away.

## Two modes

| | Build mode (weekday) | Live mode (Sunday) |
|---|---|---|
| Purpose | Make the arrangement | Play it, with live calls |
| Screen | Timeline, mixer, editing | Current section, next section, bars left, queued call |
| Editing | Everything | None. You can't change the arrangement mid-song |

## The main screen

It combines Ableton's arrangement view with MultiTracks Playback, in our own design.

- **Top bar:** the transport, plus a display showing the current section, bar and beat, and the queued call ("Going to Chorus 2 in 1.5 bars").
- **Section rail** (MultiTracks-style): the arrangement as big colored blocks. Drag a block to reorder it, drag it off the rail to remove it, or use ⧉ to repeat it. Drag a section in from the palette below the rail. This works **while playing**. Cmd+Z undoes.
- **Arrangement view** (Ableton-style): one lane per stem with its waveform, cut into section clips in arrangement order, with a bar ruler, the playhead and the queued jump. Double-click a clip, or select it and press 0, to switch that stem off for that section only. Each track header has level, mute and solo.
- **Automatic setup**, the first time a song loads:
  1. **The Ableton set.** If the folder has a `.als` (MultiTracks downloads include `MultiTrack.als`), its locators become the section markers, and it supplies the tempo and meter. This is exact and instant.
  2. **The folder name** gives the title, key and tempo ("See a Victory-At Midnight-A-77.50bpm").
  3. **"Read guide"** (also the fallback) reads the **Click** for tempo and downbeat, and recognizes the **Guide**'s spoken cues with an offline speech model. MultiTracks guides say the section name and then count the next bar in ("Verse… 1, 2, 3, 4"), so the marker goes on the downbeat after the count. With no count, it goes on the downbeat after the cue. Repeated names are numbered.
- **Stems can be in a subfolder:** the app looks in the chosen folder, then in its subfolders (`MultiTracks/` first). Picking the `MultiTracks` folder itself also works.
- **Key and tempo** are in the Song panel, as in MultiTracks Playback. Each stem is time-stretched and pitch-shifted when the song loads (Signalsmith Stretch, comparable to Ableton's Complex Pro). Click and guide are only time-stretched. Section positions follow the new tempo.

- **The grid is the song's real downbeats.** The ruler draws them as they are, so every bar is the same length. The folder name's tempo is the authority: MultiTracks clicks and Ableton sets often run at double time with eighth-note clicks, and that's detected and corrected. The guide's spoken "one"s pick the true downbeat. If a song is still off, Song → Grid moves the downbeat by a beat or 10 ms and can halve or double the tempo.
- **Bar 1 is the first click.** MultiTracks stems (and click songs) start on the one, so the first click in the Click track is bar 1, beat 1. After the Ableton set or the guide decide which bar each section starts on, the grid is put on the click. Sections keep their audio position, rounded to bars. Each song is checked once (GRID_VERSION), and hand-made grids are never moved.
- **Band and MD cues are not sections.** Build / Build up, Drums in, All in, Hits, Hold and the like are never markers, whether they come from the guide or the Ableton set. A Breakdown is a section only when the guide counts into it like one, or the Ableton set has it as a marker.
- **Track order:** Click & guide, Drums, Percussion, Bass, Guitars, Keys, Other instruments, Vocals, FX. Within a group, numbers go in number order (EG 2 before EG 10).
- **Buses:** built in are Click & guide, Drums & percussion (drums, loops, tambourine, shakers…), Bass, Guitars, Keys, Other instruments, Vocals and FX. Each has a header in the track list with level, mute and output; a bus output overrides its tracks' own outputs. **+ New bus** makes your own. Double-click a bus name to rename it. Drag a track's name onto a bus header to move it: that's remembered by track name, so it applies in every song. Deleting your own bus sends its tracks back to their usual bus. Buses are shared by every song.
- **Drag and drop:** dropping a song folder on the window adds it to the set. Dropping audio files while a song is loaded copies them in next to its stems as new tracks, starting at bar 1 (export new parts from the song's start). A name clash gets a number, so nothing is overwritten.
- **Count-in is off by default.** The Count-in button turns it on.
- **Pads:** Setup → Pads takes a folder with one sustained pad per key (MultiTracks pad packs). The Pad button (or pedal 4 hold) fades in the pad for the current song's key, including any key change. It crossfades when the next song is in a different key and keeps playing between songs.
- **Click songs:** Set → New click song builds a song with no tracks from a tempo, key and a list of sections. It generates a Click stem and a spoken Guide (the Mac's voice counts in and calls each section in the bar before it), saved under Music → Playback Rig → Click Songs.

## Live behaviour

The plan is set, but you make live calls during the song:

- **Next:** queue the next block. Press it again to skip one further.
- **Repeat:** play this block again when it ends.
- **Vamp:** loop this block until you turn vamp off.
- **Jump timing:** set to Now, Next bar or End of section. The default is Next bar.

### Pedal plan (4 switches)

| Pedal | Tap | Hold |
|---|---|---|
| 1 | Play / stop | Panic stop with a fade |
| 2 | Next block | Skip ahead one more block |
| 3 | Repeat block | Vamp on / off |
| 4 | Next song, or start the segue | Pad only |

Every control is assigned by MIDI learn, so any pedal or controller works.

## Arrangement editing (Build mode)

- **Reorder** section blocks, and repeat or remove them.
- **Change stems per block:** for example, drums out in verse 1 or keys only in the bridge.
- **Tempo and key.** Pitch-shifting full stems gets audibly worse past about ±2 semitones, so for bigger key changes we recommend downloading the stems in the right key from MultiTracks.
- **Clip editing** on a timeline: trim, fade and move clips.

## Songs and transitions

- **Stems come from MultiTracks and Loop Community downloads.**
- **The guide stem is cut by section,** so each spoken cue ("Chorus", "Bridge") moves with its section when you reorder.
- **Each song has its own transition setting:** segue into the next song, end and wait, or pad underneath. Pads follow the song's key.

## Rig

- **Computer:** Mac.
- **Interface:** Behringer with 10 outputs, probably a UMC1820 (to be confirmed). Outputs 1-2 go to front of house (stems), 3-4 to the in-ears (click and guide). 5-10 are free for split stems later.
- **Controller:** an Akai fader controller with buttons, probably a MIDImix or APC mini (to be confirmed).
- **Integration:** ProPresenter lyrics advance with sections. Lights and Planning Center are not in scope for now.

## Build order

1. **Engine:** arrangement playback, quantized jumps, vamp and repeat, per-block mutes, routing, click, count-in, MIDI mapping. *Done, with tests.*
2. **Mac app (Tauri):** choose the audio interface and its outputs, take pedal and controller input (MIDI learn, tap/hold), run the engine on the audio thread, load songs from stem folders, resample to the interface's rate. *Done.* Stems are still loaded fully into memory; streaming from disk is still to do.
3. **Arrangement view, section rail, live rearranging and automatic setup from the click and guide.** *Done.*
4. Song transitions: pads, segues, guide cues cut by section.
5. Tempo and key changes, clip editing, ProPresenter.

## Positioning (decided 2026-11-05)

**Why this exists:** MultiTracks Playback is easy, but it's tied to an iPad and the cloud. When the Wi-Fi drops, songs and key changes won't load, and the app has to be restarted over and over on a Sunday. Ableton runs offline and sounds better, but it costs a lot and isn't built for this. MultiTracks' own downloaded stems also sound better than the Playback app.

**What sets us apart:**
1. **Offline first, always.** Nothing on stage depends on the network: loading a song, changing the key or tempo, switching songs. The only things that touch the internet are updates and the one-time speech-model download, and the app works without both.
2. **Own your files, no lock-in.** MultiTracks downloads, other stores' stems and your own recordings are all plain files on disk. No subscription is needed to play them.
3. **Smart transitions.** Key-aware pad bridges and tempo-matched segues between songs in a set.
4. **Planning Center sync.** Pull the Sunday set, keys and arrangements from Planning Center. (This one uses the network, but before the service, never during it.)

**How it's used:** the music director plays keys and runs tracks from a computer, controlled from a **MIDI controller**. A phone or iPad remote would be nice to have, but it's not a priority. Team sharing and editing on other devices are not needed yet.

**Wanted later:** lyrics and lighting cues (MIDI to ProPresenter), MIDI clock, SMPTE/LTC timecode, a backup computer for redundancy.

## Next up

1. Confirm the grid is right on real MultiTracks songs (needs See a Victory's MultiTrack.als).
2. **Song transitions:** crossfade or segue into the next song, with pads carrying across.
3. **Dynamic guide cues:** the guide calls the right section after a rearrangement.
4. Song and key changes stay instant: preload the next song in the set, so switching never waits.
5. Planning Center sync.

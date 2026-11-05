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
- **Automatic setup:** the first time a song loads, the app reads its **Click** stem (tempo and first downbeat) and its **Guide** stem. It recognizes the spoken cues with an offline speech model (about 150 MB, downloaded once) and places a marker on the downbeat after each cue. Repeated names are numbered (Chorus 1, Chorus 2). "Read guide" in the Song panel runs it again.

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

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

## Live page and Edit page

**Live / Edit** in the top bar (or L, or a learned MIDI button) switches pages; the app remembers the last one. The top bar (transport, display, Song, Setup) is the same on both.

- **Live page** (Playback-style, readable from the keys): Now and Next side by side (kept compact so the controls fit): the current section, bars left, beat lights, a progress bar, and what's next ("Next", "In 2 bars → Chorus 2" when a jump is queued, "Vamping"). Below, the arrangement on one line that scrolls along with the song (tap a block to go there), and under it the **fades**: the song's ending, the way into the next song, pad under the ending and **Fade out now**. On the left, the set; the next song is highlighted, with → for a segue and ⤳ for a crossfade.
  Below that, three tabs, so nobody has to dig into Setup mid-service (and someone else can run it from here):
  - **Mixer:** vertical faders (drag the cap, scroll, or arrow keys; double-click = 100%) with level meters and a mute per bus, plus the built-in click and pad levels. With more buses than fit, swipe sideways. Track-by-track mixing stays on the Edit page.
  - **Pads:** on/off, level, and any of the 12 keys (the song's key is marked). Picking a key overrides the song's key until the song changes.
  - **Click (free click):** a click with no song, for announcements, between songs or a rehearsal with no tracks. Start/stop, BPM −/+, tap tempo, meter, "song tempo". It goes to the click output at the click level and stops by itself when a song starts. Also a learnable MIDI button.
  - **Key −/+** sits on the tab bar (stopped only, since the stems are re-rendered).
- **Edit page:** the section rail and the arrangement view below. Grid lines run down every lane like Ableton: a line on each bar (every few bars when zoomed far out), fainter beat lines when zoomed in, and a stronger line where each section starts. They sit on the song's real downbeats, so they follow tempo changes.

## Look

- **Five looks** (Setup → Look), like Ableton's themes: Graphite (default), Midnight, Studio (Ableton-style grey), Sanctuary (warm) and Daylight (light, for bright rooms). Every colour comes from the theme, the arrangement view's included. Both pages share the same top bar, type and colours, so Live and Edit read as one program.
- **Play is green, stop is red.**
- **Level meters** on the Live mixer's faders (each bus shows its loudest track) and under every track on the Edit page, so you can see what's playing (or muted) at a glance.

## The main screen

It combines Ableton's arrangement view with MultiTracks Playback, in our own design.

- **Top bar:** the transport, plus a display showing the current section, bar and beat, and the queued call ("Going to Chorus 2 in 1.5 bars").
- **Section rail** (MultiTracks-style): the arrangement as big colored blocks. While playing it scrolls with the song, keeping the playing block (and the one before it) in view; scroll it yourself and it leaves you alone for a few seconds. Drag a block to reorder it, drag it off the rail to remove it, or use ⧉ to repeat it. Drag a section in from the palette below the rail. This works **while playing**. Cmd+Z undoes.
- **Arrangement view** (Ableton-style): one lane per stem with its waveform, cut into section clips in arrangement order, with a bar ruler, the playhead and the queued jump. Double-click a clip, or select it and press 0, to switch that stem off for that section only. Each track header has level, mute and solo.
- **Automatic setup**, the first time a song loads:
  1. **The Ableton set.** If the folder has a `.als` (MultiTracks downloads include `MultiTrack.als`), its locators become the section markers, and it supplies the tempo and meter. This is exact and instant.
  2. **The folder name** gives the title, key and tempo ("See a Victory-At Midnight-A-77.50bpm").
  3. **"Read guide"** (also the fallback) reads the **Click** for tempo and downbeat, and recognizes the **Guide**'s spoken cues with an offline speech model. MultiTracks guides say the section name and then count the next bar in ("Verse… 1, 2, 3, 4"), so the marker goes on the downbeat after the count. With no count, it goes on the downbeat after the cue. Repeated names are numbered.
- **Stems can be in a subfolder:** the app looks in the chosen folder, then in its subfolders (`MultiTracks/` first). Picking the `MultiTracks` folder itself also works.
- **Key and tempo** are in the Song panel, as in MultiTracks Playback. Each stem is time-stretched and pitch-shifted when the song loads (Signalsmith Stretch, comparable to Ableton's Complex Pro). Click and guide are only time-stretched. Section positions follow the new tempo.

- **The grid is the song's real downbeats.** The ruler draws them as they are, so every bar is the same length. The folder name's tempo is the authority: MultiTracks clicks and Ableton sets often run at double time with eighth-note clicks, and that's detected and corrected. The guide's spoken "one"s pick the true downbeat. If a song is still off, Song → Grid moves the downbeat by a beat or 10 ms and can halve or double the tempo.
- **Bar 1 is the first click.** MultiTracks stems (and click songs) start on the one, so the first click in the Click track is bar 1, beat 1. After the Ableton set or the guide decide which bar each section starts on, the grid is put on the click. Sections keep their audio position, rounded to bars. Each song is checked once (GRID_VERSION), and hand-made grids are never moved.
- **Every section starts a bar after its call.** On the Guide stem, a call is a cue plus any count after it, and the section starts on the next downbeat. Every marker goes through the same rule: a marker sitting on its cue moves one bar later, one bar late moves one bar earlier, one already right stays. A marker with no call next to it (the guide is silent there) stays where the Ableton set put it. The audio before the first section is the **Count-in** (or a pickup, if shorter than a bar). Numbers the app added are dropped when they aren't needed ("Intro", not "Intro 2").
- **Band and MD cues are not sections.** Build / Build up, Drums in, All in, Hits, Hold and the like are never markers, whether they come from the guide or the Ableton set. A Breakdown is a section only when the guide counts into it like one, or the Ableton set has it as a marker.
- **Track order:** Click & guide, Drums, Percussion, Bass, Guitars, Keys, Other instruments, Vocals, FX. Within a group, numbers go in number order (EG 2 before EG 10).
- **Buses:** built in are Click & guide, Drums & percussion (drums, loops, tambourine, shakers…), Bass, Guitars, Keys, Other instruments, Vocals and FX. Each has a header in the track list with level and mute, and shows its output; outputs are set in Setup → Outputs. **+ New bus** makes your own. Double-click a bus name to rename it. Drag a track's name onto a bus header to move it: that's remembered by track name, so it applies in every song. Deleting your own bus sends its tracks back to their usual bus. Buses are shared by every song.
- **Drag and drop:** dropping a song folder on the window adds it to the set. Dropping audio files while a song is loaded copies them in next to its stems as new tracks, starting at bar 1 (export new parts from the song's start). A name clash gets a number, so nothing is overwritten.
- **Tempo changes follow automatically.** When a song's tempo jumps or ramps, the app reads every beat off the Click track (eighth-note clicks included), or from the Ableton set's tempo automation when there's no click. The grid, the ruler, the built-in click, "Next bar" jumps, bar/beat, bars left and the guide's calls all follow it, and the Ableton markers are placed where they fall in the audio. A steady song (within 1%) keeps its simple grid. The Song panel lists the changes ("bar 9: 84"); setting a tempo by hand, or reading the guide, goes back to one steady tempo.
- **The time signature is found automatically.** From the Ableton set: the master's setting or, if the set uses time signature markers, the one at the start of the song (Live stores 6/8 as 302). Without one, from the click's accents: the strong click every 6 pulses is 6/8; every 3 is 3/4, or 6/8 when the pulse is too fast for quarter notes; a 4/4 eighth-note click is never read as 6/8. In 6/8 the grid counts eighth notes (six to a bar, the click on every eighth with beat 1 accented) and the tempo is shown in the same unit as the folder's number (MultiTracks gives 6/8 songs per quarter note: "More Than Able" shows 73), or in dotted quarters when the folder has no number. A song with a tempo map shows one number: the tempo at the playhead, moving as the song speeds up or slows down (like Playback and Ableton). The folder's number may be per dotted quarter, quarter or eighth; the app takes whichever matches the set or the click. Songs an earlier version set up as 4/4 are set up again in 6/8 when they load. Song → Song details has Beats per bar and Beat note to set it by hand.
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
- **Tell it what to change** (box beside the section chips, empty until you type), in plain words the way you'd tell the band. **The app reads these itself, free and offline** (`ui/arrange.js`, tested in CI with 37 phrasings): double / repeat / "chorus 2 twice" / "c2 x2"; cut / drop / skip / take out / "no instrumental"; "cut all instrumentals" / "no tags" / "every interlude" removes every numbered one (Instrumental 1 and 2); add ("a tag at the end", "verse 3 after chorus 2", "at the start"); move / put ("the bridge after chorus 3"); swap; "the last chorus", "the second chorus"; and runs: "after verse 2 do chorus 2, repeat it, then go straight into the bridge" (what follows the bridge stays). Several changes in one sentence work ("double verse 1 and drop the bridge"). A section the song doesn't have is reported, never guessed. The result shows as a preview with Apply / Cancel; nothing changes before Apply; Cmd+Z undoes. Blocks keep the mutes, trims and notes of the block they came from. A plain list ("I, V1, C1 x2, B, O") applies directly. **Optional:** with an Anthropic API key (Setup → Arrangement assistant), requests the app doesn't understand go to Claude (claude-opus-5-5; section names and order only, no audio; about a cent each; needs the internet). Without a key nothing is ever sent anywhere.
- **Zoom** (+/−, = and - keys) keeps the playhead where it is on screen.
- **Change stems per block:** for example, drums out in verse 1 or keys only in the bridge.
- **Tempo and key.** Pitch-shifting full stems gets audibly worse past about ±2 semitones, so for bigger key changes we recommend downloading the stems in the right key from MultiTracks.
- **Clip editing** on the timeline: drag a clip's left or right edge to trim that track inside that block (snaps to the beat; Alt for fine). Drag the small handle at a clip's top corner for a fade-in or fade-out. Backspace on a selected clip removes its trim. Trims move with their block and work while playing.

## Songs and transitions

- **Stems come from MultiTracks and Loop Community downloads.**
- **A jump asked for late still names where it goes.** If you jump (Next, or tap a section) with less time left than that section's call takes, the call plays from its start the moment you ask, so the band hears "Bridge…" rather than only the end of the count. A jump asked for in good time lines the call up to end on the downbeat, as before.
- **The guide stem is cut by section.** Each section's call (the cue and its count, from just before the speech to the section's downbeat, at most two bars) belongs to that section. In the last stretch of any block, the guide plays the call for whatever block actually comes next: the planned next block, a live jump, a repeat or a vamp. Duplicate an Outro and the guide says "Outro" before the second one, then "Ending" after it. A call recorded there for a section that isn't coming is silenced. A section with no call gets silence, never a wrong call.
- **Each song has its own ending and transition**, set once (Live page fades bar, or Song → Ending & next song):
  - **Ending:** play out, or fade out from bar N of the last block over M bars (for long endings like See a Victory's). The click fades with it, and the song ends when the fade does.
  - **Into the next song:** Stop (wait), Segue (the next song starts the moment this one ends, exact to the sample), or Crossfade over N bars (the next song starts N bars before the end while this one fades out). At the handoff the old song's click and guide go silent, so the in-ears only hear the new song's click. A stop or a vamp never starts the next song.
  - **Pad under ending:** the pad comes in (this song's key) when the last block starts and carries into the next song.
  - **Fade out now:** fades the song over 2 bars and stops (also a learnable MIDI button).
- **The next song is always loaded in the background,** so a segue is instant and switching songs after a stop doesn't wait.

## Staying connected

- **Unplugged interface:** the set stays loaded; a red banner says so; the app checks every 1.5 s and reconnects when it's back (or when the interface you chose appears after starting on a stand-in like the laptop speakers, but never in the middle of a song). Playback waits, stopped at the block that was playing. It never falls back to the laptop speakers on its own, so front of house never gets the in-ear mix.
- **Unplugged MIDI controller:** same: banner, reconnect when it's back. The Live page Mixer covers the faders meanwhile.
- **Routing is by bus, in Setup → Outputs, one master setting for every song** (decided 2026-11-05): every track goes where its bus goes; there are no per-track or per-song outputs. Bus, built-in click and pad outputs are remembered for each interface by name. A new interface starts from defaults for its number of outputs (4+: Click & guide on 3-4, every other bus on 1-2; 2: everything on 1-2).

## Mixes and notes

- **Each song remembers its mix:** bus levels and mutes (Live → Mixer or the Edit page bus headers) are saved with the song. A song you haven't mixed uses the default mix. "Reset to default" and "Make default" are in Live → Mixer.
- **Notes per block** (✎ on a rail block): "WL repeats 2x", "drums out". The Live page shows the current block's note under its name and the next block's under Next.

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
3. ~~Dynamic guide cues~~ *Done in 0.6.0.*
4. ~~Live page and Edit page~~ *Done in 0.6.1.*
5. ~~Transitions per song, ending fades~~ *Done in 0.6.2.*
6. ~~Clip trimming and clip fades; tempo changes~~ *Done in 0.6.2.* Still to do: volume automation drawn over a whole lane.
7. Song and key changes stay instant: preload the next song in the set, so switching never waits.
8. Planning Center sync.

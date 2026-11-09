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
  - **Mixer:** vertical faders (drag the cap, scroll, or arrow keys; bus and master faders in dB up to +10, double-click = 0 dB) with level meters and a mute per bus, plus the built-in click and pad levels. With more buses than fit, swipe sideways. At the right end, always in view, is the **master fader**: one level on everything the app sends out (songs, built-in click, pad), with its own meter. It has no mute button on purpose (one mis-tap would silence the band), double-click sets 100%, moves are smoothed so they never click, and it's remembered between launches. It can be learned to a controller fader (Setup → MIDI → Master fader). Track-by-track mixing stays on the Edit page.
  - **Pads:** on/off, level, and any of the 12 keys (the song's key is marked). Picking a key overrides the song's key until the song changes.
  - **Click (free click):** a click with no song, for announcements, between songs or a rehearsal with no tracks. Start/stop, BPM −/+, tap tempo, meter, "song tempo". It goes to the click output at the click level and stops by itself when a song starts. Also a learnable MIDI button.
  - **Key −/+** sits on the tab bar (stopped only, since the stems are re-rendered).
- **Fade out now (0.11.0)** fades the tracks and the guide over 2 bars and keeps playing, so the click (click tracks and the built-in click) carries on until you stop, like MultiTracks Playback. The button reads "Tracks out · click on" until you stop. Song transitions (crossfade) still fade everything.
- **APC mini mk2 ready-made layout (0.17.0).** Brian's controller is an APC mini mk2, not a MIDImix. Choosing its *Control* port with nothing assigned sets it up (Setup → MIDI → "Use the APC mini layout" resets to it; picking the *Notes* port shows a warning):
  - Top row: Song 1-8. Rows 7-6: Sections 1-16 of the arrangement (jump on the Now/Bar/End setting). Row 5: Play/stop, Next block, Repeat, Vamp, Cancel jump, Fade out now, Pad on/off, Free click. Row 4: reserved for the Drummer. Row 3: Live tonight (Drums, Bass, Piano, Organ, Electric, Acoustic). Track buttons 1-8: bus mutes. Faders 1-8: buses, fader 9: master (soft takeover, dB law). Scene 1-2: previous / next song.
  - **The pads light up** from the app's state (only changes are sent): current song green, next yellow, a song cut pulsing; sections dim in their colours, the playing block green, a queued jump pulsing; play, vamp, fade, pad and free click lit when on; Live tonight parts orange; track buttons lit when the bus is muted.
  - New MIDI actions: Section N (GotoBlock), Live tonight part on/off. Assumes factory settings (pads notes 0-63 from the bottom-left, faders CC 48-56); to be confirmed on the real unit.
- **Drummer (planned, from Brian's answers 2026-11-09):** a built-in live drummer for flow moments when there's no drummer (about once a month). Steady grooves, dynamics from nothing to full, cinematic swells, builds and drops. Built-in sampled acoustic kit (DRSKit, CC BY 4.0: 13 mics, up to 49 hits per drum), held to Apple Drummer quality. Intensity fader, buttons for Fill / Build / Drop / Hit / Half-time / Tap / Faster, and it follows Brian's playing from the Nord and MainStage controller over USB (moving the fader or a button overrides). Song tempo, tap tempo, speeding up. Click stays in ears, drums to front of house. From a song (and back into a section or the next song) or standalone between songs. Demos to approve before it ships.
- **Faster song loads (0.16.0)**, after Brian found going back to a song took a few seconds. Measured on Worthy (21 stems, set +1): almost all of the time was re-rendering the key change on every load, then resampling 44.1 kHz files to the interface's 48 kHz.
  - **Renders are kept on disk:** a stem that needed a key change, tempo change or resampling is saved once at the interface's rate (16-bit with its own scale, dithered) and read straight back next time. Worthy: 48.9 s → 2.2 s on a 4-core test machine; faster on an M5. Up to 12 GB of renders, oldest-used removed first. A changed file, key, tempo or interface rate makes a new render.
  - **The whole set is rendered ahead of time** in the background, one stem at a time and only while nothing plays, so even the first load of a song is fast.
  - **Tapping the song that's loaded goes to its top** without loading anything.
  - Waveforms for the Edit page are worked out in parallel.
  - Not done: keeping songs you've left in memory. Each loaded song takes about 3 GB of RAM (32-bit audio), so only the current and next song are held.
- **Sunday safety (0.15.0)**, picked by Brian from the suggestions (not doing yet: a separate in-ear mix, rehearsal mode):
  - **Live tonight** (Live page, under the set): Drums, Bass, Piano, Organ, Electric, Acoustic. Tap the parts the band plays live; their tracks go quiet in every song (each song's own mix is left alone, so untapping brings them back as they were). Only what a player covers: the drum kit (not percussion or loops), bass guitar (not synth bass), piano/Rhodes/Wurli (not synths or pads, so "Keys 1-6" stay). Saved between launches. The Edit page marks those lanes LIVE. Smooth transitions measure the mix without them, like B3/C3.
  - **Check the set** (button under Live tonight): the interface is the one chosen and connected; click track, guide and built-in click don't share front of house's outputs; the master fader isn't down; the controller is connected; the Mac isn't on battery; and for every song: folder and audio files there, a click (track or built-in), a guide, the Click fader not muted, and a pad file in its key if it uses the pad. Problems first, then things to look at, then what's fine. Changes nothing.
  - **Back after a crash:** while a song plays the app notes the song and block. If it closes without quitting normally, the next launch loads that song at that block, stopped, and says so; press play to carry on. A normal quit or an update leaves nothing to pick up.
  - **MIDImix ready-made layout** (Setup → MIDI): choosing a MIDImix with nothing assigned sets it up; "Use the MIDImix layout" resets to it. Faders 1-8 = the first 8 buses (Click, Guide, Drums, Bass, Guitars, Keys, Other, Vocals), master fader = master; Mute 1-8 = mute that bus; hold SOLO + Mute 1-8 = Song 1-8; Rec arm 1-8 = Play/stop, Next block, Repeat, Vamp, Cancel jump, **Fade out now**, Pad on/off, Free click; Bank ◀ ▶ = previous/next song; bottom knobs 1-2 = built-in click level, pad level. Controls are listed by their printed names. Assumes the MIDImix's factory settings (channel 1); anything reprogrammed in Akai's editor can be learned.
  - **New MIDI actions** (learnable on any controller): bus mute, previous song, cancel jump (also cancels a song cut), pad level, built-in click level.
  - **Controller faders follow the screen:** bus and master faders on the controller use the same dB law as the screen (unity at three-quarters, +10 dB at the top), and **take over only once moved to where the screen's fader is** (soft takeover), so a new song's mix or a fader left low never makes a level jump mid-song.
- **Levels and click (0.14.0)**, after Brian found MultiTracks click tracks too quiet with no room to turn them up:
  - **The click track has its own fader** (bus "Click"), separate from **Guide**. Existing routing carries over: both follow wherever Click & guide went. Learnable to a controller fader like any bus.
  - **Click and guide files are evened out on load** (peak to -3 dBFS), so a quiet MultiTracks click and a loud one start at the same place. Existing click levels may sound different the first time; set once.
  - **Bus and master faders read in dB, up to +10 dB**: unity sits three-quarters up (double-click = 0 dB), marks at +10/+5/0/-5/-10/-20/-40. The built-in click and pad keep their % sliders.
  - **Nothing distorts:** a soft limiter on every output is transparent below -1 dBFS and never lets the signal pass full scale, so pushing the click fader up gets louder, not crunchy.
  - **Built-in click subdivisions (Live → Click):** 1/4, 1/8, triplets or 1/16 per beat, saved with the song, so a 68 BPM song doesn't need a doubled tempo. In-between clicks are softer and lower; the downbeat stays accented. The free click has the same choice. The count-in stays in quarter notes.
  - **After this song (Stop only):** Stay, Cue next, or Top of set. When the song plays to its end, the set selects the next song or the first one, ready for play, like Playback. Saved with the song; on the fades bar and in Song → Ending & next song.
- **Smooth transitions (0.13.0)**, from a test render Brian approved (We Want Revival in C#m into Worthy's Instrumental, B3/C3). *Into the next song → Smooth*, and every cut to another song mid-song:
  - The outgoing song **settles**: when it's much louder than where it's landing, the music (never the click or guide) eases down over its last 2 bars, by most of the difference (up to 8 dB). Measured from the actual mix, muted buses left out.
  - The next song lands **on the downbeat** (count-in/pickup blocks skipped; a medley's entry if set) at the **old tempo for 2 bars**, then **eases into its own tempo, arriving by the time its guide first speaks** (at least 2 beats, at most 2 bars, inside the entry block). The click glides with it. Pre-rendered in the background while the song plays (Signalsmith, pitch kept); until it's ready a plain segue/cut is queued.
  - **By role** (from track names): vocals, rhythm (drums, perc, loops, bass, acoustic), click and guide stop cleanly on the line; sustained parts (keys, pads, guitars, strings, organ, FX) ring on for 2 beats, high-passed so the low end never muddies, into a **reverb wash** of the last beat that blooms over the seam; the new song's sustained parts swell in over its first beat.
  - **Announce (Setup, on by default):** the Mac's built-in voice says where it's going ("Worthy. Instrumental.") in the guide's output, ending a beat before the change, for whoever runs tracks without calling cues. Generated once and cached; skipped where there's no voice.
  - Inside a song, jumps' ring-outs (Blend) are now by role too: only sustained parts ring on, everything else cuts cleanly with a 5 ms fade.
- **Blend (0.12.0, Setup → Audio output; replaces Smooth jumps):** Cut / Short (about ⅓ s, default) / 1 beat / 1 bar. Inside a song, where the arrangement skips ahead or you jump, repeat or vamp, the music you leave rings out under the new part for that long (click and guide excluded). When you cut to another song mid-song, it's how long the old song fades under the new one.
- **Cut to another song mid-song (0.12.0):** while a song plays, tap another song in the Live set list (or press its Song button on the controller). It loads in the background, then comes in on the next bar of the current song, which fades under it (Blend). The Next song card reads "Cutting to …"; tap the song again to cancel, and stopping cancels it too. Only the new song's click and guide are heard from the cut.
- **Medley (0.12.0):** when a song segues or crossfades into the next, a Medley row appears with the ending/transition controls (Live page and Song panel): *leave after* a block of this song (instead of playing to the end) and *start the next song at* a block of its arrangement (instead of the top). Saved with the song. Jumping past the exit point plays on to the real end. The click switches to the new song's tempo at the hand-off (no tempo glide).
- **Switching songs by hand keeps the pad (0.11.0):** a pad that's playing stays in its key when you pick another song; it moves to the new song's key when you press play (or turn it off). A segue or crossfade follows the new song's key straight away.
- **Pad level (0.11.0):** pad files are evened out to one moderate loudness when loaded (packs come mastered loud), and the level slider is an audio taper (level squared: 50% ≈ -12 dB, 8% is quiet). Existing pad levels will sound quieter; set to taste.
- **Keep their pitch (0.11.0, Song → Key & tempo):** on a key change, click and guide always stay as recorded, and so do effects tracks by name (FX, SFX, risers, sweeps, impacts, noise). Tap any track to keep its pitch or make it follow the key; saved with the song.
- **MIDI song buttons (0.11.0):** Song 1 to Song 10 go straight to that place in the set (when stopped).
- **Live / Edit switch:** big, at the bottom centre of the window on both pages (and L on the keyboard, or a MIDI button).
- **Set list (Live page):** each song shows its key (as played) on the right. Under the list, a **Next song** card: name, key, tempo, and how it starts (on play / segues / crossfades in, and its pad start).
- **MIDI (Setup → MIDI controller):** no pedals; controller buttons and faders. Buttons: play/stop, next block, repeat, vamp, skip ahead, next song, fade out, panic stop, pad only, free click, Live/Edit. Faders: the **master and each bus** only (like mapping it in Ableton); tracks follow their bus. A bus fader on the controller does exactly what that bus's fader on screen does (saved with the song's mix). Older assignments (track or click faders, tap/hold) keep working.
- **Top bar buttons are icons with small captions** (Set, Next, Repeat, Vamp, Click, Count-in, Pad, Song, Setup), like hardware. The song box has its own fixed space (long names end in "…"), so nothing in the top bar can overlap at any window size.
- **Top bar song box** shows tempo, meter and the key as played (no "+1"); the song name glows on its screen.
- **Pads tab (0.9.6):** *Sounds*: every pad folder you add stays in a list; tap one to switch (a playing pad crossfades to it in the same key); "+ Add sound" adds another; right-click removes one from the list (files stay). *This song*: Start = No pad / Pad through intro (comes in when you press play, fades out when the intro ends: the opening Count-in / Pickup / Intro blocks, or the first block if there's no intro) / Pad whole song; End = Pad under ending (moved here from the ending row on the Live page; still on the Song page). Saved with the song.
- **Click tab (0.9.6):** Song click (built-in click on/off, count-in, click level, big beat lights following the song) beside the click with no song (start/stop, tempo on a screen, tap, song tempo, meter). The click's output stays in Setup.
- **Key change is staged (0.9.6):** − / + pick the key (shown in yellow, "from B") without touching the tracks; "Change key" applies it once; Cancel goes back. The tracks can only be re-pitched while stopped.
- **Live page layout (0.9.4):** Now and Next are one compact row (about half their old height) so the mixer gets the room. Now shows the section, bar/tempo/key, bars left, beat lights and a clock (time in the song / length). The ending and transition controls (Ending, Into the next song, Pad under ending, Fade out now) sit in one line across the top of the mixer panel. On the right of the panel, like Playback: a column with the Mixer / Pads / Click page buttons and the Key change, then the master fader, which stays on screen whichever page is showing.
- **Edit page:** the section rail and the arrangement view below. Grid lines run down every lane like Ableton: a line on each bar (every few bars when zoomed far out), fainter beat lines when zoomed in, and a stronger line where each section starts. They sit on the song's real downbeats, so they follow tempo changes.

## Look

- **Five looks** (Setup → Look), like Ableton's themes: Graphite (default), Midnight, Studio (Ableton-style grey), Sanctuary (warm) and Daylight (light, for bright rooms). Every colour comes from the theme, the arrangement view's included. Both pages share the same top bar, type and colours, so Live and Edit read as one program.
- **Play is green, stop is red.**
- **Screens:** the song box in the top bar, the time display (Edit page) and the Live page clock look like hardware displays: dark glass, a bezel, faint scanlines and glowing text. They stay dark screens in every look, Daylight included.
- **Finish details** (the small things that make it feel like an instrument, not a web page): buttons have a soft top light and sink when pressed; Next / Repeat / Vamp carry small icons; Vamp glows while it's looping; Click, Count-in and Pad have an indicator light; the top display has a beat light per beat (the one is orange); meters are LED segments with a peak-hold line; faders sit in a groove with dB marks (+6, 0, -6, -12, off) and a ridged cap; clips and section blocks have soft corners. Track names on the Edit page sit on their track's colour, every other four-bar phrase is shaded so phrases can be counted at a glance, and each Live mixer bus wears the colour of its first track (name cap and fader line), so the two pages match. Nothing moved.
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
- **Controller:** Akai APC mini mk2 (8x8 RGB pads, 9 faders), confirmed 2026-11-09. Brian also runs MainStage on the same MacBook (pads and sounds from a USB controller; piano from a Nord, also on USB).
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

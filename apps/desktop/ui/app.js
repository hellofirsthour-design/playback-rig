// Playback Rig UI. All audio runs in Rust; this file draws state and sends commands.
(() => {
  const T = window.__TAURI__;
  const invoke = (c, a) => T.core.invoke(c, a);
  const $ = s => document.querySelector(s);
  const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };

  let V = null;            // AppView from Rust
  let snap = null;         // engine Snapshot
  let mode = 'live';
  let draftBlocks = null;  // Build mode: [{section, muted: []}]
  let draftMarkers = null; // Build mode: [{name, bar}]
  let draftFor = null;     // song folder the drafts belong to
  let learnWait = null;    // index of the Learn row waiting for MIDI
  let busy = false;

  /* ---------- helpers ---------- */
  let toastT = 0;
  function toast(msg) {
    const t = $('#toast'); t.textContent = msg; t.hidden = false;
    clearTimeout(toastT); toastT = setTimeout(() => { t.hidden = true; }, 4000);
  }
  async function call(cmd, args) {
    try {
      const r = await invoke(cmd, args);
      if (r && r.outputs) { V = r; render(); }
      return r;
    } catch (e) { toast(String(e)); throw e; }
  }
  const send = c => invoke('cmd', { c }).catch(e => toast(String(e)));
  const routeKey = r => (r.Pair !== undefined ? 'p:' + r.Pair : 'm:' + r.Mono);
  const keyRoute = k => { const [t, n] = k.split(':'); return t === 'p' ? { Pair: +n } : { Mono: +n }; };
  function routeOptions(sel, current) {
    const n = V.output ? V.output.channels : 2;
    sel.innerHTML = '';
    for (let i = 0; i + 1 < n; i += 2) {
      const tag = n >= 4 ? (i === 0 ? ' · Front of house' : i === 2 ? ' · In-ears' : '') : ' · Stereo';
      sel.append(new Option(`Out ${i + 1}-${i + 2}${tag}`, 'p:' + i));
    }
    for (let i = 0; i < n; i++) sel.append(new Option(n === 2 ? (i ? 'Right only' : 'Left only') : `Out ${i + 1} only`, 'm:' + i));
    sel.value = routeKey(current);
    if (sel.selectedIndex < 0) sel.value = 'p:0';
  }
  const dB = g => (g <= 0.001 ? '−∞' : ((20 * Math.log10(g) >= 0 ? '+' : '') + (20 * Math.log10(g)).toFixed(1)));
  const barFrames = () => (V && V.song ? V.song.sample_rate * 60 / V.song.bpm * V.song.beats_per_bar : 1);
  const playing = () => !!(snap && snap.playing);

  /* ---------- render ---------- */
  function render() {
    if (!V) return;
    const song = V.song;
    if (song && draftFor !== song.folder + '|' + JSON.stringify(song.blocks) + JSON.stringify(song.markers)) {
      draftFor = song.folder + '|' + JSON.stringify(song.blocks) + JSON.stringify(song.markers);
      draftBlocks = song.blocks.map(b => ({ section: b.section, muted: [...b.muted] }));
      draftMarkers = song.markers.map(m => ({ ...m }));
    }
    renderHeader(); renderSetlist(); renderLive(); renderBuild(); renderSetup(); tick();
  }

  function renderHeader() {
    const o = V.output, info = $('#outinfo'); info.innerHTML = '';
    if (o) { const b = el('b', null, o.name); info.append(b, ` · ${o.channels} outputs · ${(o.sample_rate / 1000).toFixed(1)} kHz`); }
    else info.append('No audio output. Choose one in Setup.');
    info.append(el('br'), V.midi ? `MIDI: ${V.midi}` : 'No pedals or controller connected');
  }

  function renderSetlist() {
    const ol = $('#songs'); ol.innerHTML = '';
    if (!V.setlist.length) ol.append(el('li', 'empty', 'No songs yet.'));
    V.setlist.forEach((s, i) => {
      const li = el('li', 'song' + (i === V.current ? ' cur' : '') + (s.found ? '' : ' missing'));
      li.tabIndex = 0; li.title = s.found ? s.path : `Folder not found: ${s.path}`;
      li.append(el('span', 'n', String(i + 1)), el('span', 'nm', s.name));
      const mv = el('span', 'mv');
      [['↑', -1, 'Move up'], ['↓', 1, 'Move down']].forEach(([t, d, l]) => {
        const b = el('button', null, t); b.title = l; b.setAttribute('aria-label', l);
        b.onclick = e => { e.stopPropagation(); call('move_song', { index: i, delta: d }); }; mv.append(b);
      });
      const x = el('button', null, '✕'); x.title = 'Remove from set'; x.setAttribute('aria-label', 'Remove from set');
      x.onclick = e => { e.stopPropagation(); call('remove_song', { index: i }); }; mv.append(x);
      li.append(mv);
      li.onclick = () => selectSong(i);
      li.onkeydown = e => { if (e.key === 'Enter') selectSong(i); };
      ol.append(li);
    });
  }

  async function selectSong(i) {
    if (busy) return;
    if (playing()) { toast('Stop playback to change songs.'); return; }
    busy = true; $('#songline').textContent = 'Loading stems…';
    try { await call('select_song', { index: i }); } catch (e) { /* toast shown */ } finally { busy = false; render(); }
  }

  function renderLive() {
    const s = V.song, sl = $('#songline'); sl.innerHTML = '';
    if (s) { sl.append(el('b', null, s.name), ` · ${+s.bpm.toFixed(2)} BPM · ${s.beats_per_bar}/4${s.key ? ' · Key of ' + s.key : ''}`); }
    else sl.textContent = V.setlist.length ? 'Pick a song from the set.' : 'No song loaded. Add songs to the set, then pick one.';
    const strip = $('#strip'); strip.innerHTML = '';
    if (s) s.blocks.forEach((b, i) => {
      const c = el('button', 'chip'); c.dataset.i = i;
      c.append(el('span', 'cn', b.section), el('span', 'cs', `${i < 9 ? i + 1 + ' · ' : ''}${Math.round(b.bars)} bars${b.muted.length ? ' · −' + b.muted.join(', −') : ''}`));
      c.onclick = () => send({ type: 'Goto', block: i });
      strip.append(c);
    });
    strip.hidden = !s;
    document.querySelectorAll('#quant button').forEach(b => b.classList.toggle('on', b.dataset.q === V.quantize));
    $('#clickBtn').classList.toggle('on', !!(s && s.click)); $('#clickBtn').disabled = !s;
    $('#countBtn').classList.toggle('on', V.count_in);
  }

  function tick() {
    const s = V && V.song;
    const p = playing();
    $('#tally').classList.toggle('on', p);
    $('#playBtn').classList.toggle('on', p);
    $('#playBtn').querySelector('span').textContent = p ? 'Stop' : 'Play';
    $('#vampBtn').classList.toggle('on', !!(snap && snap.vamp));
    if (!s || !snap) {
      $('#blockname').textContent = s ? (s.blocks[0] ? s.blocks[0].section : '—') : '—';
      $('#call').textContent = ''; $('#bb').textContent = '–'; $('#barsleft').textContent = ''; $('#countin').textContent = '';
      return;
    }
    const b = s.blocks[snap.item];
    $('#blockname').textContent = b ? b.section : '—';
    const call = $('#call'); call.innerHTML = ''; call.className = 'call';
    const bf = barFrames();
    if (snap.pending != null && s.blocks[snap.pending]) {
      const bars = snap.frames_to_jump / bf;
      call.append('Going to ', el('b', null, s.blocks[snap.pending].section), bars < 0.05 ? ' now' : ` in ${bars < 1 ? (bars * s.beats_per_bar).toFixed(0) + ' beats' : bars.toFixed(1) + ' bars'}`);
    } else if (snap.vamp && b) {
      call.className = 'call vamp'; call.append('Vamping ', el('b', null, b.section));
    } else if (s.blocks[snap.item + 1]) {
      call.append('Next: ', el('b', null, s.blocks[snap.item + 1].section));
    } else if (snap.finished) {
      call.textContent = 'Song finished.';
    } else call.textContent = 'Last block';
    $('#bb').textContent = snap.counting_in ? '–' : `${snap.bar}.${snap.beat}`;
    const left = (snap.section_frames - snap.frame_in_section) / bf;
    $('#barsleft').textContent = p && !snap.counting_in ? `${Math.max(0, Math.ceil(left - 0.001))} bars left in block` : '';
    $('#countin').textContent = snap.counting_in ? 'Count-in' : '';
    document.querySelectorAll('#strip .chip').forEach(c => {
      const i = +c.dataset.i;
      c.classList.toggle('cur', i === snap.item);
      c.classList.toggle('done', p && i < snap.item);
      c.classList.toggle('queued', snap.pending === i);
    });
  }

  /* ---------- build ---------- */
  function renderBuild() {
    const s = V.song;
    $('#buildEmpty').hidden = !!s; $('#buildBody').hidden = !s;
    if (!s) return;
    // arrangement
    const ol = $('#blocks'); ol.innerHTML = '';
    draftBlocks.forEach((b, i) => {
      const li = el('li', 'blk');
      const sec = s.sections.find(x => x.name === b.section);
      const name = el('div', 'bn', b.section); name.append(el('small', null, sec ? `${Math.round(sec.bars)} bars · from bar ${sec.bar}` : 'section missing'));
      const tags = el('div', 'stemtags');
      s.stems.forEach(st => {
        const off = b.muted.includes(st.name);
        const t = el('button', off ? 'off' : '', st.name); t.title = off ? `Unmute ${st.name} in this block` : `Mute ${st.name} in this block`;
        t.onclick = () => { b.muted = off ? b.muted.filter(m => m !== st.name) : [...b.muted, st.name]; renderBuild(); };
        tags.append(t);
      });
      const ctl = el('div', 'ctl');
      [['↑', 'Move up', () => i > 0 && ([draftBlocks[i - 1], draftBlocks[i]] = [draftBlocks[i], draftBlocks[i - 1]])],
       ['↓', 'Move down', () => i < draftBlocks.length - 1 && ([draftBlocks[i + 1], draftBlocks[i]] = [draftBlocks[i], draftBlocks[i + 1]])],
       ['⧉', 'Repeat this block', () => draftBlocks.splice(i + 1, 0, { section: b.section, muted: [...b.muted] })],
       ['✕', 'Remove block', () => draftBlocks.length > 1 ? draftBlocks.splice(i, 1) : toast('An arrangement needs at least one block.')]
      ].forEach(([t, l, f]) => { const x = el('button', null, t); x.title = l; x.setAttribute('aria-label', l); x.onclick = () => { f(); renderBuild(); }; ctl.append(x); });
      li.append(el('span', 'i', String(i + 1)), name, tags, ctl);
      ol.append(li);
    });
    const sel = $('#addSection'); const keep = sel.value; sel.innerHTML = '';
    s.sections.forEach(x => sel.append(new Option(`${x.name} (bar ${x.bar})`, x.name)));
    if (keep) sel.value = keep;
    // song form (don't clobber what's being typed)
    if (!document.activeElement || !document.activeElement.closest('#mode-build .form')) {
      $('#f-name').value = s.name; $('#f-bpm').value = s.bpm; $('#f-bpb').value = s.beats_per_bar; $('#f-key').value = s.key; $('#f-off').value = Math.round(s.offset_ms);
    }
    renderMarkers();
    renderMixer();
  }

  function renderMarkers() {
    const box = $('#markers');
    if (box.contains(document.activeElement)) return;
    box.innerHTML = '';
    draftMarkers.forEach((m, i) => {
      const r = el('div', 'mk');
      const n = el('input'); n.value = m.name; n.id = 'mk-n-' + i; n.setAttribute('aria-label', 'Section name');
      n.oninput = () => { m.name = n.value; };
      const b = el('input'); b.type = 'number'; b.min = 1; b.value = m.bar; b.id = 'mk-b-' + i; b.setAttribute('aria-label', 'Start bar');
      b.oninput = () => { m.bar = Math.max(1, Math.round(+b.value || 1)); };
      const x = el('button', 'btn danger', 'Remove'); x.onclick = () => { draftMarkers.splice(i, 1); renderMarkers(); };
      r.append(n, b, x); box.append(r);
    });
  }

  function renderMixer() {
    const s = V.song, box = $('#strips');
    if (box.contains(document.activeElement) && document.activeElement.type === 'range') return;
    box.innerHTML = '';
    s.stems.forEach((st, i) => {
      const r = el('div', 'ch');
      const sel = el('select'); sel.setAttribute('aria-label', `${st.name} output`); routeOptions(sel, st.route);
      sel.onchange = () => { st.route = keyRoute(sel.value); send({ type: 'StemRoute', stem: i, route: st.route }); };
      const m = el('button', 'm' + (st.muted ? ' on' : ''), 'M'); m.title = 'Mute'; m.setAttribute('aria-pressed', st.muted);
      m.onclick = () => { st.muted = !st.muted; send({ type: 'StemMute', stem: i, muted: st.muted }); renderMixer(); };
      const f = el('input'); f.type = 'range'; f.min = 0; f.max = 1.5; f.step = 0.01; f.value = st.gain; f.id = 'fader-' + i; f.setAttribute('aria-label', `${st.name} level`);
      const d = el('span', 'db', dB(st.gain));
      f.oninput = () => { st.gain = +f.value; d.textContent = dB(st.gain); send({ type: 'StemGain', stem: i, gain: st.gain }); };
      r.append(el('span', 'sn', st.name), sel, m, f, d); box.append(r);
    });
  }

  $('#addBlock').onclick = () => { const v = $('#addSection').value; if (v) { draftBlocks.push({ section: v, muted: [] }); renderBuild(); } };
  $('#arrSave').onclick = () => call('set_arrangement', { blocks: draftBlocks }).then(() => toast('Arrangement saved.')).catch(() => {});
  $('#arrReset').onclick = () => call('set_arrangement', { blocks: [] }).then(() => toast('Back to the song as recorded.')).catch(() => {});
  $('#addMarker').onclick = () => {
    const last = draftMarkers.reduce((m, x) => Math.max(m, x.bar), 0);
    draftMarkers.push({ name: 'Section ' + (draftMarkers.length + 1), bar: last + 8 }); renderMarkers();
  };
  $('#songSave').onclick = () => {
    const names = draftMarkers.map(m => m.name.trim().toLowerCase()).filter(Boolean);
    if (new Set(names).size !== names.length) { toast('Two sections have the same name. Give each one its own name (e.g. Chorus 1, Chorus 2).'); return; }
    call('update_song', { meta: {
      name: $('#f-name').value, bpm: +$('#f-bpm').value || V.song.bpm, beats_per_bar: Math.round(+$('#f-bpb').value || 4),
      offset_ms: +$('#f-off').value || 0, key: $('#f-key').value, markers: draftMarkers.map(m => ({ name: m.name.trim(), bar: m.bar })) } })
      .then(() => toast('Song saved.')).catch(() => {});
  };

  /* ---------- setup ---------- */
  const LEARN = [
    { label: 'Pedal 1', tap: 'PlayStop', hold: 'Panic', desc: 'Tap: play / stop · Hold: panic stop' },
    { label: 'Pedal 2', tap: 'Next', hold: 'SkipAhead', desc: 'Tap: next block · Hold: skip ahead' },
    { label: 'Pedal 3', tap: 'Repeat', hold: 'ToggleVamp', desc: 'Tap: repeat block · Hold: vamp' },
    { label: 'Pedal 4', tap: 'NextSong', hold: 'PadOnly', desc: 'Tap: next song · Hold: pad only' },
    { label: 'Play / stop button', tap: 'PlayStop' },
    { label: 'Next button', tap: 'Next' },
    { label: 'Repeat button', tap: 'Repeat' },
    { label: 'Vamp button', tap: 'ToggleVamp' },
    { label: 'Next song button', tap: 'NextSong' },
    { label: 'Click fader', tap: 'ClickLevel' },
  ];
  function actionLabel(a) {
    if (a && a.StemLevel !== undefined) return `${V.song && V.song.stems[a.StemLevel] ? V.song.stems[a.StemLevel].name : 'Stem ' + (a.StemLevel + 1)} fader`;
    return ({ PlayStop: 'Play / stop', Panic: 'Panic stop', Next: 'Next block', SkipAhead: 'Skip ahead', Repeat: 'Repeat block', ToggleVamp: 'Vamp',
      NextSong: 'Next song', PadOnly: 'Pad only', ClickLevel: 'Click fader' })[a] || String(a);
  }
  function controlLabel(c) {
    if (c.Note) return `Button / note ${c.Note.key} (ch ${c.Note.ch + 1})`;
    if (c.Cc) return `Control ${c.Cc.num} (ch ${c.Cc.ch + 1})`;
    return '?';
  }
  function renderSetup() {
    const os = $('#outSel'); os.innerHTML = '';
    V.outputs.forEach(o => os.append(new Option(`${o.name} (${o.channels} outputs)${o.is_default ? ' · system default' : ''}`, o.name)));
    if (V.output) os.value = V.output.name;
    routeOptions($('#clickRoute'), V.click_route);
    if (document.activeElement !== $('#clickGain')) $('#clickGain').value = V.click_gain;
    const n = V.output ? V.output.channels : 0;
    $('#outHint').textContent = n >= 4
      ? 'Stems start on Out 1-2 (front of house). Click and guide go to Out 3-4 (in-ears). Change any of them per song in Build → Mixer.'
      : 'This output has only 2 channels. Pick your audio interface above to send click and guide to the in-ears separately.';
    const ms = $('#midiSel'); ms.innerHTML = ''; ms.append(new Option('None', ''));
    V.midi_inputs.forEach(m => ms.append(new Option(m, m)));
    ms.value = V.midi || '';
    const rows = [...LEARN, ...(V.song ? V.song.stems.map((st, i) => ({ label: `${st.name} fader`, tap: { StemLevel: i } })) : [])];
    const box = $('#learn'); box.innerHTML = '';
    rows.forEach((r, i) => {
      const d = el('div', 'lrow'); const t = el('div'); t.append(el('b', null, r.label));
      if (r.desc) t.append(el('small', null, r.desc));
      const b = el('button', 'btn' + (learnWait === i ? ' wait' : ''), learnWait === i ? 'Press it now…' : 'Learn');
      b.onclick = async () => {
        if (!V.midi) { toast('Choose your MIDI input first.'); return; }
        if (learnWait === i) { await invoke('midi_cancel'); learnWait = null; renderSetup(); return; }
        await invoke('midi_learn', { tap: r.tap, hold: r.hold || null }); learnWait = i; renderSetup();
      };
      d.append(t, b); box.append(d);
    });
    const bl = $('#bindings'); bl.innerHTML = '';
    if (!V.bindings.length) bl.append(el('p', 'empty', 'Nothing assigned yet.'));
    V.bindings.forEach((bd, i) => {
      const r = el('div', 'bind'); const t = el('span');
      t.append(el('code', null, controlLabel(bd.control)), ' → ' + actionLabel(bd.tap) + (bd.hold ? ` (hold: ${actionLabel(bd.hold)})` : ''));
      const x = el('button', 'btn danger', 'Remove'); x.onclick = () => call('midi_unbind', { index: i });
      r.append(t, x); bl.append(r);
    });
  }
  $('#outSel').onchange = e => {
    if (playing()) { toast('Stop playback before changing the audio output.'); renderSetup(); return; }
    call('choose_output', { name: e.target.value }).then(() => toast('Audio output changed.')).catch(() => {});
  };
  $('#clickRoute').onchange = e => { V.click_route = keyRoute(e.target.value); send({ type: 'ClickRoute', route: V.click_route }); };
  $('#clickGain').oninput = e => { V.click_gain = +e.target.value; send({ type: 'ClickGain', gain: V.click_gain }); };
  $('#midiSel').onchange = e => call('choose_midi', { name: e.target.value || null }).catch(() => {});

  /* ---------- controls ---------- */
  $('#playBtn').onclick = () => send({ type: 'PlayStop' });
  document.querySelectorAll('[data-cmd]').forEach(b => { b.onclick = () => send({ type: b.dataset.cmd }); });
  $('#quant').onclick = e => { const b = e.target.closest('[data-q]'); if (!b) return; V.quantize = b.dataset.q; send({ type: 'Quantize', q: V.quantize }); renderLive(); };
  $('#clickBtn').onclick = () => { if (!V.song) return; V.song.click = !V.song.click; send({ type: 'Click', on: V.song.click }); renderLive(); };
  $('#countBtn').onclick = () => { V.count_in = !V.count_in; send({ type: 'CountIn', on: V.count_in }); renderLive(); };
  $('#addSongs').onclick = () => call('add_songs').catch(() => {});
  document.querySelectorAll('.tabs button').forEach(b => {
    b.onclick = () => {
      mode = b.dataset.mode;
      document.querySelectorAll('.tabs button').forEach(x => x.classList.toggle('on', x === b));
      ['live', 'build', 'setup'].forEach(m => { $('#mode-' + m).hidden = m !== mode; });
      document.body.classList.toggle('build', mode === 'build');
      if (mode === 'setup') invoke('app_state').then(v => { V = v; render(); });
    };
  });
  document.addEventListener('keydown', e => {
    if (e.target.closest('input,select,textarea') || e.metaKey || e.ctrlKey || e.altKey) return;
    const k = e.key;
    if (k === ' ') { e.preventDefault(); send({ type: 'PlayStop' }); }
    else if (k === 'n' || k === 'N' || k === 'ArrowRight') send({ type: 'Next' });
    else if (k === 'r' || k === 'R') send({ type: 'Repeat' });
    else if (k === 'v' || k === 'V') send({ type: 'Vamp' });
    else if (k === 'Escape') send({ type: 'CancelJump' });
    else if (/^[1-9]$/.test(k) && V && V.song && V.song.blocks[+k - 1]) send({ type: 'Goto', block: +k - 1 });
  });

  /* ---------- events from Rust ---------- */
  T.event.listen('stem-level', e => {
    const [i, g] = e.payload; if (!V || !V.song || !V.song.stems[i]) return;
    V.song.stems[i].gain = g; const f = $('#fader-' + i);
    if (f) { f.value = g; f.nextElementSibling.textContent = dB(g); }
  });
  T.event.listen('click-level', e => { if (V) { V.click_gain = e.payload; if (mode === 'setup') $('#clickGain').value = e.payload; } });
  T.event.listen('midi-learned', () => { learnWait = null; invoke('app_state').then(v => { V = v; render(); toast('Control assigned.'); }); });
  T.event.listen('audio-error', e => toast('Audio problem: ' + e.payload));
  T.event.listen('pedal', e => {
    if (e.payload === 'next-song') {
      if (playing()) { toast('Next song is ready when this one stops.'); return; }
      if (V && V.current != null && V.current + 1 < V.setlist.length) selectSong(V.current + 1);
    } else if (e.payload === 'pad-only') toast('Pads arrive in the next update.');
  });

  /* ---------- status loop ---------- */
  setInterval(async () => {
    try {
      const s = await invoke('status');
      snap = s.snapshot;
      if (learnWait != null && !s.learning) { learnWait = null; V = await invoke('app_state'); render(); }
      else if (V) tick();
    } catch (e) { /* app closing */ }
  }, 40);

  (async () => {
    V = await invoke('app_state');
    render();
    if (V.current != null && !V.song && V.setlist[V.current] && V.setlist[V.current].found) selectSong(V.current);
  })();
})();

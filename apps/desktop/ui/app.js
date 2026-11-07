// Playback Rig UI. Audio runs in Rust; this draws state and sends commands.
(() => {
  const T = window.__TAURI__;
  const invoke = (c, a) => T.core.invoke(c, a);
  const $ = s => document.querySelector(s);
  const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };

  let V = null;            // AppView from Rust
  let snap = null;         // engine Snapshot
  let peaks = null;        // { per_sec, stems: [Uint8Array] }
  let blocks = [];         // arrangement being shown: [{ uid, section, muted: [] }]
  let songKey = null;      // folder of the loaded song
  let undo = [];
  const solo = new Set();
  let sel = null;          // selected clip { block, stem }
  const zoom = { pps: 20, scroll: 0, follow: true };
  let learnWait = null, analyzing = false, loading = false, uidSeq = 1, dirty = true;
  const LANE_H = 46, RULER_H = 20, BUS_H = 46;
  // Folded buses, remembered on this computer.
  let collapsed = new Set();
  try { collapsed = new Set(JSON.parse(localStorage.getItem('rig.collapsed') || '[]')); } catch (e) { /* storage off */ }
  const saveCollapsed = () => { try { localStorage.setItem('rig.collapsed', JSON.stringify([...collapsed])); } catch (e) { /* storage off */ } };
  // Track-list rows, top to bottom: a bus header, then its stems (unless folded).
  function rows() {
    const out = []; let y = 0;
    if (!V || !V.song) return out;
    const buses = V.buses && V.buses.length ? V.buses : [{ id: 'all', name: 'Tracks', gain: 1, muted: false, route: null, custom: false, stems: V.song.stems.map((_, i) => i) }];
    for (const b of buses) {
      out.push({ kind: 'bus', b, y, h: BUS_H }); y += BUS_H;
      if (collapsed.has(b.id)) continue;
      for (const i of b.stems) { out.push({ kind: 'stem', i, b, y, h: LANE_H }); y += LANE_H; }
    }
    return out;
  }
  const rowsHeight = R => (R.length ? R[R.length - 1].y + R[R.length - 1].h : 0);

  /* ---------- helpers ---------- */
  let toastT = 0;
  function toast(msg, ms = 4500) { const t = $('#toast'); t.textContent = msg; t.hidden = false; clearTimeout(toastT); toastT = setTimeout(() => { t.hidden = true; }, ms); }
  const err = e => toast(String(e));
  const playing = () => !!(snap && snap.playing);
  const send = c => invoke('cmd', { c }).catch(err);
  const routeKey = r => (r.Pair !== undefined ? 'p:' + r.Pair : 'm:' + r.Mono);
  const keyRoute = k => { const [t, n] = k.split(':'); return t === 'p' ? { Pair: +n } : { Mono: +n }; };
  const fmt = s => { s = Math.max(0, s); const m = Math.floor(s / 60); return m + ':' + String(Math.floor(s % 60)).padStart(2, '0'); };
  const playBpm = () => (V && V.song ? V.song.play_bpm || V.song.bpm : 120);
  const barSec = () => (V && V.song ? 60 / playBpm() * V.song.beats_per_bar : 2);
  // Key spellings as worship charts write them.
  const MAJOR = ['C', 'Db', 'D', 'Eb', 'E', 'F', 'F#', 'G', 'Ab', 'A', 'Bb', 'B'];
  const MINOR = ['Cm', 'C#m', 'Dm', 'Ebm', 'Em', 'Fm', 'F#m', 'Gm', 'G#m', 'Am', 'Bbm', 'Bm'];
  const PC = { C: 0, D: 2, E: 4, F: 5, G: 7, A: 9, B: 11 };
  // "A" + 2 -> "B", "A" + 1 -> "Bb", "F#m" - 1 -> "Fm". Unknown key -> "+2".
  function keyName(key, semis) {
    const m = /^([A-G])([#b]?)(m?)$/.exec((key || '').trim());
    if (!m) return semis ? (semis > 0 ? '+' : '') + semis : (key || '—');
    const i = (((PC[m[1]] + (m[2] === '#' ? 1 : m[2] === 'b' ? -1 : 0) + semis) % 12) + 12) % 12;
    return (m[3] ? MINOR : MAJOR)[i];
  }
  const copyTrims = b => (b.trims || []).map(t => ({ ...t }));
  const strip = bs => bs.map(b => ({ section: b.section, muted: [...b.muted], trims: copyTrims(b), note: b.note || '' }));
  const withUid = bs => bs.map(b => ({ uid: uidSeq++, section: b.section, muted: [...b.muted], trims: copyTrims(b), fade_out: b.fade_out || null, note: b.note || '' }));

  function secColor(name) {
    const n = name.toLowerCase();
    if (/pre.?chorus/.test(n)) return '#20c997';
    if (/post.?chorus/.test(n)) return '#ff8f6b';
    if (/chorus|refrain|hook/.test(n)) return '#ff5c5c';
    if (/verse/.test(n)) return '#3d8bfd';
    if (/bridge/.test(n)) return '#a26bfa';
    if (/tag|vamp/.test(n)) return '#ffb020';
    if (/intro/.test(n)) return '#8796a8';
    if (/outro|ending|\bend\b/.test(n)) return '#66727f';
    if (/instrumental|interlude|turn|solo/.test(n)) return '#18b4c9';
    if (/break|build/.test(n)) return '#e05dae';
    if (/^top$|count/.test(n)) return '#4a4f58';
    return '#9aa0a8';
  }
  const STEM_FALLBACK = ['#4c9dff', '#f2c94c', '#36c9b0', '#ff6fae', '#b07cff', '#ff7a45'];
  function stemColor(name, i) {
    const n = name.toLowerCase();
    if (/click|guide|cue/.test(n)) return '#6b7079';
    if (/drum|perc|kick|snare|loop|hat|tom|cymbal/.test(n)) return '#ff7a45';
    if (/bass|sub/.test(n)) return '#b07cff';
    if (/key|piano|synth|pad|organ|rhodes|wurli/.test(n)) return '#4c9dff';
    if (/gtr|guitar|\beg\b|\bag\b|elec|acou/.test(n)) return '#f2c94c';
    if (/voc|vox|bgv|choir|sing/.test(n)) return '#ff6fae';
    if (/string|orch|horn|brass|cello|violin/.test(n)) return '#36c9b0';
    return STEM_FALLBACK[i % STEM_FALLBACK.length];
  }
  // "Out 3-4" / "Out 5" for a route, as in the output menus.
  function routeLabel(r) {
    if (!r) return '';
    if (r.Pair != null) return `Out ${r.Pair + 1}-${r.Pair + 2}`;
    if (r.Mono != null) return `Out ${r.Mono + 1}`;
    return '';
  }
  function routeOptions(sel, current) {
    const n = V.output ? V.output.channels : 2;
    sel.innerHTML = '';
    for (let i = 0; i + 1 < n; i += 2) sel.append(new Option(`Out ${i + 1}-${i + 2}${n >= 4 ? (i === 0 ? ' · FOH' : i === 2 ? ' · In-ears' : '') : ''}`, 'p:' + i));
    for (let i = 0; i < n; i++) sel.append(new Option(n === 2 ? (i ? 'Right only' : 'Left only') : `Out ${i + 1} only`, 'm:' + i));
    sel.value = routeKey(current);
    if (sel.selectedIndex < 0) sel.value = 'p:0';
  }

  /* ---------- layout of the arrangement in time ---------- */
  // The song's real downbeats (seconds in the audio as it plays) inside [a, b).
  // Bars left in the block: from the engine, which follows tempo changes.
  const barsLeft = sn => (sn.bars_left != null ? sn.bars_left : (sn.section_frames - sn.frame_in_section) / V.song.sample_rate / barSec());
  // "77.5 BPM", or "70 → 84 BPM" for a song whose tempo changes.
  // Tempo as musicians count it: in 6/8 that's the dotted quarter (the grid counts eighths).
  const felt = (s, bpm) => bpm / ((s && s.felt_per_beat) || 1);
  function tempoText(s) {
    const tc = (s.tempo_changes || []).map(([b, t]) => [b, felt(s, t)]);
    if (tc.length < 2) return `${+felt(s, playBpm()).toFixed(2)} BPM`;
    // A tempo map: just the tempo at the playhead, moving with the song (like Playback and Ableton).
    const at = snap && snap.bpm ? snap.bpm : tc[0][1] * ((s.felt_per_beat) || 1);
    return `${+felt(s, at).toFixed(1)} BPM`;
  }
  function downbeats(a, b) {
    // Songs whose tempo changes: the server sends every bar line.
    const db = V.song.downbeats;
    if (db && db.length) return db.filter(t => t >= a - 1e-6 && t < b - 1e-6);
    const bs = barSec(), off = V.song.offset_ms / 1000 * (V.song.bpm / playBpm());
    const out = [];
    for (let n = Math.ceil((a - off) / bs - 1e-6); off + n * bs < b - 1e-6; n++) out.push(off + n * bs);
    return out;
  }
  function layout() {
    if (!V || !V.song) return [];
    let t = 0, bar = 1;
    return blocks.map((b, i) => {
      const sec = V.song.sections.find(x => x.name === b.section) || { start_sec: 0, secs: 0, bars: 0, bar: 1 };
      // A block with an ending fade stops where the fade does.
      const secs = b.fade_out ? Math.min(sec.secs, (b.fade_out[0] + b.fade_out[1]) * barSec()) : sec.secs;
      const beats = downbeats(sec.start_sec, sec.start_sec + secs);
      const o = { i, b, sec, start: t, secs, bar, beats };
      t += secs; bar += beats.length;
      return o;
    });
  }
  function playTime(L) {
    if (!snap || !L.length || !V.song) return 0;
    const b = L[Math.min(snap.item, L.length - 1)];
    return b.start + snap.frame_in_section / V.song.sample_rate;
  }

  /* ---------- state sync ---------- */
  async function refresh(v) {
    V = v || await invoke('app_state');
    const s = V.song;
    const key = s ? s.folder : null;
    if (key !== songKey) {
      songKey = key; undo = []; solo.clear(); sel = null; peaks = null;
      blocks = s ? withUid(s.blocks) : [];
      if (s) {
        invoke('song_peaks').then(p => {
          if (!p || songKey !== key) return;
          peaks = { per_sec: p.per_sec, stems: p.stems.map(b => Uint8Array.from(atob(b), c => c.charCodeAt(0))) };
          dirty = true;
        });
        requestAnimationFrame(fit);
        if (s.needs_setup && (s.has_guide || s.has_click)) analyze(true);
      }
    } else if (s && JSON.stringify(strip(blocks)) !== JSON.stringify(strip(s.blocks))) {
      blocks = withUid(s.blocks); // changed elsewhere (e.g. guide re-read)
    } else if (s) {
      blocks.forEach((b, i) => { b.fade_out = s.blocks[i] ? s.blocks[i].fade_out : null; }); // the ending moves with the last block
    }
    render();
  }
  function render() {
    renderBar(); renderRail(); renderLive(); renderPalette(); renderHeads(); renderSetlist(); renderSongDrawer(); renderSetup(); renderStatus();
    $('#emptyArr').hidden = !!(V && V.song);
    dirty = true; tick();
  }

  /* ---------- top bar ---------- */
  function renderSongMeta() {
    const s = V && V.song;
    $('#songMeta').textContent = s ? `${tempoText(s)} · ${s.beats_per_bar}/${s.meter_den || 4}${s.key ? ' · ' + keyName(s.key, s.transpose) : ''}` : '';
  }
  function renderBar() {
    const s = V.song;
    $('#songTitle').textContent = loading ? 'Loading…' : s ? s.name : (V.setlist.length ? 'Pick a song' : 'No songs yet');
    renderSongMeta();
    document.querySelectorAll('#quant button').forEach(b => b.classList.toggle('on', b.dataset.q === V.quantize));
    $('#clickBtn').classList.toggle('on', !!(s && s.click)); $('#clickBtn').disabled = !s;
    $('#countBtn').classList.toggle('on', V.count_in);
    const pd = V.pads;
    $('#padBtn').classList.toggle('on', pd.on);
    $('#padBtn span').textContent = pd.on && pd.song_key ? `Pad · ${pd.song_key}` : 'Pad';
  }
  function tick() {
    if (!V) return;
    if (V.song && (V.song.tempo_changes || []).length > 1) renderSongMeta(); // the playing tempo follows the song
    const p = playing();
    const pb = $('#playBtn'); pb.classList.toggle('on', p); pb.setAttribute('aria-label', p ? 'Stop' : 'Play');
    $('#vampBtn').classList.toggle('on', !!(snap && snap.vamp));
    $('#prevSong').disabled = p || V.current == null || V.current <= 0;
    $('#nextSong').disabled = p || V.current == null || V.current >= V.setlist.length - 1;
    const s = V.song, L = layout();
    const cur = snap && L.length ? Math.min(snap.item, L.length - 1) : -1;
    $('#lcdSec').textContent = cur >= 0 ? L[cur].b.section : '—';
    $('#lcdBB').textContent = !snap || !s ? '–' : snap.counting_in ? 'Count' : `${snap.bar}.${snap.beat}`;
    // A light per beat, like a hardware beat counter: the one is orange.
    const bd = $('#lcdBeats'), nb = s ? Math.min(12, s.beats_per_bar || 4) : 0;
    if (bd.childElementCount !== nb) { bd.innerHTML = ''; for (let i = 0; i < nb; i++) bd.append(el('i')); }
    const lit = p && snap ? snap.beat : 0;
    for (let i = 0; i < nb; i++) bd.children[i].className = i + 1 === lit ? (i ? 'on' : 'on one') : '';
    const total = L.length ? L[L.length - 1].start + L[L.length - 1].secs : 0, now = playTime(L);
    $('#lcdTime').textContent = s ? `${fmt(now)} / ${fmt(total)}` : '';
    const left = cur >= 0 && snap ? barsLeft(snap) : 0;
    $('#lcdLeft').textContent = p && !snap.counting_in ? `${Math.max(0, Math.ceil(left - 0.001))} bars left` : '';
    const call = $('#lcdCall'); call.className = 'lcd-call'; call.innerHTML = '';
    if (snap && s && cur >= 0) {
      if (snap.pending != null && L[snap.pending]) {
        const bars = snap.frames_to_jump / s.sample_rate / barSec();
        call.append('Going to ', el('b', null, L[snap.pending].b.section), bars < 0.05 ? ' now' : ` in ${bars < 1 ? Math.max(1, Math.round(bars * s.beats_per_bar)) + ' beats' : bars.toFixed(1) + ' bars'}`);
      } else if (snap.vamp) { call.className = 'lcd-call vamp'; call.append('Vamping ', el('b', null, L[cur].b.section)); }
      else if (L[cur + 1]) call.append('Next: ', el('b', null, L[cur + 1].b.section));
      else call.textContent = snap.finished ? 'Song finished' : 'Last block';
    }
    document.querySelectorAll('#rail .blk').forEach(b => {
      const i = +b.dataset.i;
      b.classList.toggle('cur', i === cur);
      b.classList.toggle('done', p && i < cur);
      b.classList.toggle('queued', !!snap && snap.pending === i);
      const pr = b.querySelector('.prog');
      pr.style.width = i === cur && snap ? Math.min(100, snap.frame_in_section / Math.max(1, snap.section_frames) * 100) + '%' : '0';
    });
    followRail(cur, p);
    tickLive(L, cur, p);
  }

  // The rail follows the playhead: the playing block (and the one before it) stay in
  // view. If you scroll the rail yourself it leaves you alone for a few seconds.
  let railFollowed = -1, railHandScroll = 0;
  function followRail(cur, p) {
    const rail = $('#rail');
    if (!p || cur < 0 || drag || Date.now() - railHandScroll < 4000) { if (!p) railFollowed = -1; return; }
    const b = rail.querySelector(`.blk[data-i="${cur}"]`);
    if (!b) return;
    const left = b.offsetLeft, right = left + b.offsetWidth;
    const out = left < rail.scrollLeft || right > rail.scrollLeft + rail.clientWidth;
    if (cur === railFollowed && !out) return;
    railFollowed = cur;
    const prev = rail.querySelector(`.blk[data-i="${cur - 1}"]`);
    const want = Math.max(0, (prev ? prev.offsetLeft : left) - 8);
    if (Math.abs(rail.scrollLeft - want) > 2) rail.scrollTo({ left: want, behavior: 'smooth' });
  }
  document.addEventListener('wheel', e => { if (e.target.closest && e.target.closest('#rail')) railHandScroll = Date.now(); }, { passive: true });

  /* ---------- Look (themes) ---------- */
  const THEMES = [['graphite', 'Graphite', '#17181b', '#ff6a1a'], ['midnight', 'Midnight', '#101724', '#4c9dff'], ['studio', 'Studio', '#3b3b3b', '#ff9a3c'],
    ['sanctuary', 'Sanctuary', '#1b1612', '#e8913a'], ['daylight', 'Daylight', '#f3f3ef', '#e5560b']];
  let theme = 'graphite';
  try { theme = localStorage.getItem('rig.theme') || 'graphite'; } catch (e) { /* storage off */ }
  function setTheme(t) {
    theme = THEMES.some(x => x[0] === t) ? t : 'graphite';
    if (theme === 'graphite') delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = theme;
    try { localStorage.setItem('rig.theme', theme); } catch (e) { /* storage off */ }
    palCache = null; dirty = true;
    const box = $('#themes'); if (!box) return;
    box.innerHTML = '';
    THEMES.forEach(([id, name, bg, accent]) => {
      const b = el('button', 'theme' + (id === theme ? ' on' : '')); b.setAttribute('role', 'radio'); b.setAttribute('aria-checked', id === theme);
      const sw = el('span', 'sw'); sw.style.background = bg; const dot = el('i'); dot.style.background = accent; sw.append(dot);
      b.append(sw, el('span', null, name)); b.onclick = () => setTheme(id);
      box.append(b);
    });
  }

  /* ---------- Live page / Edit page ---------- */
  let page = 'edit';
  try { page = localStorage.getItem('rig.page') === 'live' ? 'live' : 'edit'; } catch (e) { /* storage off */ }
  function setPage(pg) {
    page = pg;
    try { localStorage.setItem('rig.page', pg); } catch (e) { /* storage off */ }
    document.body.classList.toggle('live-mode', pg === 'live');
    $('#live').hidden = pg !== 'live';
    document.querySelectorAll('#viewSeg button').forEach(b => b.classList.toggle('on', b.dataset.view === pg));
    if (pg === 'edit') requestAnimationFrame(() => window.dispatchEvent(new Event('resize')));
    tick();
  }
  document.querySelectorAll('#viewSeg button').forEach(b => { b.onclick = () => setPage(b.dataset.view); });
  function renderLive() {
    const box = $('#lvBlocks'); box.innerHTML = '';
    layout().forEach(o => {
      const t = el('button', 'ltile'); t.dataset.i = o.i; t.style.setProperty('--c', secColor(o.b.section));
      const bars = o.b.fade_out ? o.b.fade_out[0] + o.b.fade_out[1] : o.sec.bars;
      t.style.width = Math.round(Math.max(120, Math.min(300, 70 + bars * 12))) + 'px';
      t.append(el('b', null, o.b.section), el('small', null, `${o.i + 1} · ${o.sec.bars < 0.95 ? 'pickup' : Math.round(bars) + ' bars'}${o.b.muted.length ? ' · no ' + o.b.muted.join(', ') : ''}`));
      if (o.b.fade_out) { const f = el('i', 'tfade'); f.style.width = Math.min(100, o.b.fade_out[1] / bars * 100) + '%'; t.append(f); }
      const tags = (o.b.trims && o.b.trims.length ? '✂' : '') + (o.b.fade_out ? ' fade' : '');
      if (tags) t.append(el('span', 'tags', tags.trim()));
      if (o.b.note) t.append(el('span', 'tnote', o.b.note));
      t.append(el('i', 'tprog'));
      t.onclick = () => send({ type: 'Goto', block: o.i });
      box.append(t);
    });
    renderFades($('#lvFades'));
    renderLiveTab();
    { const k = V.song ? `${V.current}:${V.song.name}` : null; if (keyFor !== k) { keyFor = k; keyTarget = null; } }
    showKey();
    liveFollowed = -1;
    const set = $('#lvSet'); set.innerHTML = '';
    (V ? V.setlist : []).forEach((s, i) => {
      const arrow = s.transition === 'Segue' ? '  →' : s.transition === 'Crossfade' ? '  ⤳' : '';
      const li = el('li', (i === V.current ? 'cur' : i === V.current + 1 ? 'next' : '') + (s.found ? '' : ' missing'));
      li.append(el('span', 'snm', s.name + (i + 1 < V.setlist.length ? arrow : '')));
      if (s.key) li.append(el('span', 'skey', s.key));
      if (V.jump_to === i) li.classList.add('cut');
      li.onclick = () => { if (playing()) { cutTo(i); return; } selectSong(i); };
      set.append(li);
    });
    // Next song: what's coming, before you get there.
    const cutting = V && V.jump_to != null ? V.setlist[V.jump_to] : null;
    const nc = $('#lvNextSong'), nx = cutting || (V && V.current != null ? V.setlist[V.current + 1] : null);
    nc.hidden = !nx; nc.innerHTML = ''; nc.classList.toggle('cutting', !!cutting);
    if (cutting) {
      nc.append(el('div', 'lbl', 'Cutting to'), el('div', 'nsn', cutting.name));
      const facts = el('div', 'nsf'); if (cutting.key) facts.append(el('b', 'nsk', cutting.key)); if (cutting.bpm) facts.append(el('span', null, `${+cutting.bpm.toFixed(1)} BPM`));
      nc.append(facts, el('div', 'nsh', (V.next_ready ? 'At the next bar' : 'Loading… then at the next bar') + ' · tap it again to cancel'));
    } else if (nx) {
      const how = { Segue: 'Segues in', Crossfade: 'Crossfades in' }[(V.setlist[V.current] || {}).transition] || 'Starts on play';
      const pad = { Intro: 'pad through intro', Song: 'pad whole song' }[nx.pad_start];
      nc.append(el('div', 'lbl', 'Next song'), el('div', 'nsn', nx.name));
      const facts = el('div', 'nsf');
      if (nx.key) facts.append(el('b', 'nsk', nx.key));
      if (nx.bpm) facts.append(el('span', null, `${+nx.bpm.toFixed(1)} BPM`));
      nc.append(facts, el('div', 'nsh', how + (pad ? ' · ' + pad : '')));
    }
  }
  /* Live tabs: Mixer (buses), Pads, Click */
  let liveTab = 'mix';
  try { liveTab = localStorage.getItem('rig.liveTab') || 'mix'; } catch (e) { /* storage off */ }
  document.querySelectorAll('.vtabs [role=tab]').forEach(b => { b.onclick = () => { liveTab = b.dataset.tab; try { localStorage.setItem('rig.liveTab', liveTab); } catch (e) { /* storage off */ } renderLiveTab(); }; });
  // A vertical fader (Playback-style): drag the cap, scroll, arrow keys; double-click = 100%.
  // `meter` names what its level meter shows ("bus:drums", "stem:3"), or null.
  // Fader laws. 'db' (buses, master): a desk-style taper, unity at three-quarters of the
  // travel, up to +10 dB; shown in dB. A number: plain 0..max, shown in %.
  const DB_MAX = 3.1623;
  const dbLaw = {
    max: DB_MAX,
    toPos: g => { if (g <= 0) return 0; const d = Math.max(-60, Math.min(10, 20 * Math.log10(g))); return d >= 0 ? 0.75 + d / 40 : 0.75 * (1 - Math.sqrt(-d / 60)); },
    fromPos: p => { p = Math.max(0, Math.min(1, p)); if (p <= 0.001) return 0; const d = p >= 0.75 ? (p - 0.75) * 40 : -60 * (1 - p / 0.75) ** 2; return 10 ** (d / 20); },
    label: g => { if (g <= 0.001) return '−∞'; const d = 20 * Math.log10(g); return (Math.abs(d) < 0.05 ? '0.0' : (d > 0 ? '+' : '') + d.toFixed(1)) + ' dB'; },
    marks: [[3.1623, '+10'], [1.7783, '+5'], [1, '0'], [0.5623, '-5'], [0.3162, '-10'], [0.1, '-20'], [0, '∞']],
    step: 0.01,
  };
  const linLaw = max => ({ max, toPos: g => g / max, fromPos: p => Math.max(0, Math.min(1, p)) * max, label: g => Math.round(g * 100) + '%', marks: [[2, '+6'], [1, '0'], [0.5, '-6'], [0.25, '-12'], [0, '∞']].filter(([g]) => g <= max + 1e-6), step: 0.02 });
  function faderStrip(name, value, max, onInput, muted, onMute, label, meter) {
    const law = max === 'db' ? dbLaw : linLaw(max);
    const r = el('div', 'vstrip' + (muted ? ' off' : ''));
    const v = el('span', 'vval', law.label(value));
    const body = el('div', 'vbody');
    const mt = el('div', 'vmeter'); mt.append(el('i'));
    if (meter) mt.dataset.meter = meter; else mt.classList.add('none');
    const fd = el('div', 'vfader'); fd.tabIndex = 0; fd.setAttribute('role', 'slider'); fd.setAttribute('aria-label', label || `${name} level`);
    fd.setAttribute('aria-valuemin', 0); fd.setAttribute('aria-valuemax', 100);
    const track = el('div', 'vtrack'), fill = el('div', 'vfill'), unity = el('div', 'vunity'), cap = el('div', 'vcap');
    track.append(fill); fd.append(track, unity, cap);
    unity.style.bottom = `calc(8px + ${law.toPos(1)} * (100% - 16px))`;
    let val = value;
    const show = () => { const f = Math.max(0, Math.min(1, law.toPos(val))); cap.style.bottom = `calc(8px + ${f} * (100% - 16px))`; fill.style.height = (f * 100) + '%'; v.textContent = law.label(val); fd.setAttribute('aria-valuenow', Math.round(f * 100)); fd.setAttribute('aria-valuetext', law.label(val)); };
    const set = x => { val = Math.max(0, Math.min(law.max, Math.round(x * 1000) / 1000)); show(); onInput(val); };
    const setPos = p => set(law.fromPos(p));
    const fromY = e => { const rc = track.getBoundingClientRect(); return 1 - (e.clientY - rc.top) / rc.height; };
    fd.onpointerdown = e => { if (e.button !== 0) return; e.preventDefault(); fd.setPointerCapture(e.pointerId); fd.focus(); fd.dataset.drag = '1'; setPos(fromY(e)); };
    fd.onpointermove = e => { if (fd.dataset.drag) setPos(fromY(e)); };
    fd.onpointerup = fd.onpointercancel = () => { delete fd.dataset.drag; };
    fd.ondblclick = () => set(1);
    fd.onwheel = e => { if (Math.abs(e.deltaY) > Math.abs(e.deltaX)) { e.preventDefault(); setPos(law.toPos(val) - e.deltaY * 0.002); } };
    fd.onkeydown = e => { const k = { ArrowUp: law.step, ArrowDown: -law.step, PageUp: 0.1, PageDown: -0.1 }[e.key]; if (k) { e.preventDefault(); e.stopPropagation(); setPos(law.toPos(val) + k); } };
    // Scale marks beside the fader, in dB like a desk.
    const sc = el('div', 'vscale');
    law.marks.forEach(([g, t]) => {
      const k = el('span', g === 1 ? 'u' : null, t); k.style.bottom = `calc(8px + ${law.toPos(g)} * (100% - 16px))`; sc.append(k);
    });
    body.append(mt, sc, fd);
    const m = el('button', 'm' + (muted ? ' on' : ''), 'M'); m.title = `Mute ${name}`; m.setAttribute('aria-pressed', !!muted);
    if (onMute) m.onclick = onMute; else m.style.visibility = 'hidden';
    const nm = el('div', 'vname', name); nm.title = name;
    r.append(nm, body, v, m);
    show();
    return r;
  }
  // A bus strip wears the colour of its first track, so it matches the Edit page.
  function tint(b, strip) {
    const i = (b.stems || [])[0];
    if (i != null && V.song && V.song.stems[i]) { strip.style.setProperty('--c', stemColor(V.song.stems[i].name, i)); strip.classList.add('tinted'); }
    return strip;
  }
  function renderMaster() {
    // Master: everything the app sends out (songs, click, pad), on screen whatever the tab.
    // No mute button here, on purpose: one mis-tap would silence the band.
    const mb = $('#lvMaster');
    if (mb && V && !mb.querySelector('.vfader[data-drag]')) {
      const ms = faderStrip('Master', V.master_gain == null ? 1 : V.master_gain, 'db', x => { V.master_gain = x; send({ type: 'MasterGain', gain: x }); }, false, null, 'Master level', 'master');
      ms.classList.add('master'); ms.title = 'Master: everything the app sends out. Double-click for 100%.';
      mb.replaceChildren(ms);
    }
  }
  // A level slider being dragged (pointer down on it): the tab isn't rebuilt under the finger.
  let rangeDrag = false;
  document.addEventListener('pointerdown', e => { if (e.target && e.target.type === 'range') rangeDrag = true; }, true);
  document.addEventListener('pointerup', e => { rangeDrag = false; if (e.target && e.target.type === 'range') e.target.blur(); }, true);
  document.addEventListener('pointercancel', () => { rangeDrag = false; }, true);
  // A slider let go keeps no keyboard focus, so Space / N / R / V keep working.
  document.addEventListener('change', e => { if (e.target && e.target.type === 'range') e.target.blur(); }, true);
  function renderLiveTab() {
    document.querySelectorAll('.vtabs [role=tab]').forEach(b => { const on = b.dataset.tab === liveTab; b.classList.toggle('on', on); b.setAttribute('aria-selected', on); });
    renderMaster();
    const box = $('#lvTab'); if (!box || box.querySelector('.vfader[data-drag]') || rangeDrag) return;
    box.innerHTML = '';
    if (liveTab === 'mix') {
      if (V.song) {
        const h = el('div', 'mixhead');
        h.append(el('span', null, V.song_mix ? `Bus levels saved with ${V.song.name}.` : 'Bus levels are saved with each song. This one uses the default mix.'));
        if (V.song_mix) {
          const r = el('button', 'btn', 'Reset to default'); r.title = 'Use the default mix for this song'; r.onclick = () => invoke('song_mix', { action: 'reset' }).then(refresh).catch(err);
          const d = el('button', 'btn', 'Make default'); d.title = "Use this song's levels for songs you haven't mixed"; d.onclick = () => invoke('song_mix', { action: 'default' }).then(v => { refresh(v); toast('Default mix updated.'); }).catch(err);
          h.append(r, d);
        }
        box.append(h);
      }
      const g = el('div', 'strips');
      (V.buses || []).forEach(b => g.append(tint(b, faderStrip(b.name, b.gain, 'db', x => { b.gain = x; send({ type: 'BusGain', bus: b.id, gain: x }); if (V.song && !V.song_mix) { V.song_mix = true; } }, b.muted,
        async () => { b.muted = !b.muted; await send({ type: 'BusMute', bus: b.id, muted: b.muted }); renderHeads(); renderLiveTab(); dirty = true; }, null, 'bus:' + b.id))));
      g.append(faderStrip('Click (built-in)', V.click_gain, 1.5, x => { V.click_gain = x; send({ type: 'ClickGain', gain: x }); }, null, null));
      g.append(faderStrip('Pad', V.pads.level, 1.5, x => { V.pads.level = x; invoke('pad_mix', { level: x, route: null }).catch(err); }, !V.pads.on, () => togglePad()));
      if (!(V.buses || []).length) box.append(el('p', 'hint', 'Load a song to see its buses.'));
      box.append(g);
    } else if (liveTab === 'pads') {
      const pd = V.pads;
      if (!pd.folder) {
        box.append(el('p', 'hint', 'Choose a folder with one pad per key (MultiTracks pad packs work). Add as many sounds as you like; each one stays in the list.'));
        const b = el('button', 'btn', 'Choose pads folder'); b.onclick = () => invoke('choose_pads_folder').then(refresh).catch(err); box.append(b);
        return;
      }
      // Sounds: every pad folder added stays here; tap one to switch (a playing pad crossfades).
      const snd = el('div', 'psounds');
      snd.append(el('span', 'lbl', 'Sound'));
      (pd.sounds || []).forEach((n, i) => {
        const b = el('button', 'psnd' + (i === pd.sound ? ' on' : ''), n); b.title = i === pd.sound ? `${n} (in use)` : `Switch to ${n}`;
        b.onclick = () => { if (i !== pd.sound) invoke('pad_sound', { index: i, remove: null }).then(refresh).catch(err); };
        b.oncontextmenu = e => { e.preventDefault(); if (confirm(`Remove "${n}" from the pad sounds? (The files stay where they are.)`)) invoke('pad_sound', { index: i, remove: true }).then(refresh).catch(err); };
        snd.append(b);
      });
      const add = el('button', 'psnd add', '+ Add sound'); add.title = 'Add another pad folder (one file per key). Right-click a sound to remove it.';
      add.onclick = () => invoke('choose_pads_folder').then(refresh).catch(err);
      snd.append(add);
      box.append(snd);
      const row = el('div', 'prow');
      const on = el('button', 'btn big tog pad' + (pd.on ? ' on' : ''), pd.on ? 'Pad on' : 'Pad off'); on.onclick = () => togglePad();
      const songBtn = el('button', 'btn big' + (pd.on && !pd.chosen ? ' on' : ''), pd.song_key ? `Song key (${pd.song_key})` : 'Song key');
      songBtn.onclick = () => invoke('pad_key', { key: null }).then(refresh).catch(err);
      const lv = el('input'); lv.type = 'range'; lv.min = 0; lv.max = 1.5; lv.step = 0.01; lv.value = pd.level; lv.setAttribute('aria-label', 'Pad level');
      lv.oninput = () => { pd.level = +lv.value; invoke('pad_mix', { level: pd.level, route: null }).catch(err); };
      row.append(on, songBtn, el('span', 'lbl', 'Level'), lv);
      box.append(row);
      const keys = el('div', 'padkeys');
      const playing = pd.on ? (pd.chosen || pd.song_key) : null;
      ['C', 'Db', 'D', 'Eb', 'E', 'F', 'F#', 'G', 'Ab', 'A', 'Bb', 'B'].forEach(k => {
        const b = el('button', (k === playing ? 'on' : '') + (k === pd.song_key ? ' song' : ''), k);
        b.disabled = !pd.keys.includes(k); b.title = b.disabled ? `No ${k} pad in this sound` : `Play the ${k} pad`;
        b.onclick = () => invoke('pad_key', { key: k }).then(refresh).catch(err);
        keys.append(b);
      });
      box.append(keys);
      // This song: how the pad starts and ends it (saved with the song, done for you while it plays).
      if (V.song) {
        const tr = { ...V.song.transition };
        const card = el('div', 'psong');
        card.append(el('div', 'lbl', 'This song'));
        const r1 = el('div', 'prow');
        r1.append(el('span', 'plbl', 'Start'), seg([['Off', 'No pad'], ['Intro', 'Pad through intro'], ['Song', 'Pad whole song']], tr.pad_start || 'Off', v => {
          if (v !== 'Off' && !pd.folder) { toast('Choose your pads folder first.'); return; }
          setTransition({ ...tr, pad_start: v });
        }, 'How the pad starts this song'));
        const r2 = el('div', 'prow');
        const end = el('button', 'tog pad' + (tr.pad ? ' on' : ''), 'Pad under ending');
        end.title = "Brings the pad in when the last block starts and keeps it going into the next song";
        end.onclick = () => setTransition({ ...tr, pad: !tr.pad });
        r2.append(el('span', 'plbl', 'End'), end);
        const hint = { Off: 'The song starts with no pad.', Intro: 'The pad comes in when you press play and fades out when the intro ends.', Song: 'The pad comes in when you press play and stays under the whole song.' }[tr.pad_start || 'Off'];
        card.append(r1, el('p', 'hint', hint), r2);
        box.append(card);
      }
    } else {
      const fc = V.free_click || { on: false, bpm: 120, beats_per_bar: 4, div: 1 };
      const cards = el('div', 'ccards');
      // The song's click: the same switches as the top bar, plus level, output and big beat lights.
      const sc = el('div', 'ccard');
      sc.append(el('div', 'lbl', V.song ? `Song click · ${V.song.name}` : 'Song click'));
      const beats = el('div', 'cbeats'); const nb = V.song ? Math.min(12, V.song.beats_per_bar) : 4;
      for (let i = 0; i < nb; i++) beats.append(el('i', i ? null : 'one'));
      sc.append(beats);
      const r1 = el('div', 'crow');
      const bc = el('button', 'tog big' + (V.song && V.song.click ? ' on' : ''), 'Built-in click'); bc.disabled = !V.song;
      bc.title = "The app's own click for this song, in time with the tracks (the song's Click Track is on the Click & guide bus)";
      bc.onclick = () => { $('#clickBtn').click(); renderLiveTab(); };
      const ci = el('button', 'tog big' + (V.count_in ? ' on' : ''), 'Count-in'); ci.title = 'One bar of click before the song starts';
      ci.onclick = () => { $('#countBtn').click(); renderLiveTab(); };
      r1.append(bc, ci);
      const r2 = el('div', 'crow');
      const lv = el('input'); lv.type = 'range'; lv.min = 0; lv.max = 1.5; lv.step = 0.01; lv.value = V.click_gain; lv.setAttribute('aria-label', 'Click level');
      const lvv = el('span', 'cval', Math.round(V.click_gain * 100) + '%');
      lv.oninput = () => { V.click_gain = +lv.value; lvv.textContent = Math.round(V.click_gain * 100) + '%'; send({ type: 'ClickGain', gain: V.click_gain }); };
      r2.append(el('span', 'plbl', 'Level'), lv, lvv);
      const divs = [[1, '1/4'], [2, '1/8'], [3, 'Triplets'], [4, '1/16']];
      const r3 = el('div', 'crow');
      const sd = seg(divs, V.song ? (V.song.click_div || 1) : 1, v => { V.song.click_div = v; send({ type: 'ClickDiv', div: v }); renderLiveTab(); }, 'Song click subdivision');
      if (!V.song) sd.querySelectorAll('button').forEach(x => { x.disabled = true; });
      r3.append(el('span', 'plbl', 'Clicks'), sd);
      sc.append(r1, r2, r3, el('p', 'hint', 'Clicks per beat, saved with the song: 1/8 for a slow song so you don\'t have to double the tempo. The in-between clicks are softer. Its output is set once for every song in Setup → Outputs.'));
      // The free click: a click with no song.
      const c = el('div', 'ccard');
      c.append(el('div', 'lbl', 'Click with no song'));
      const set = (on, bpm, bpb, div) => invoke('free_click', { on, bpm, beatsPerBar: bpb, div: div || fc.div || 1 }).then(refresh).catch(err);
      const dn = el('button', 'ib', '−'), up = el('button', 'ib', '+');
      dn.setAttribute('aria-label', 'Slower'); up.setAttribute('aria-label', 'Faster');
      dn.onclick = () => set(fc.on, Math.max(30, fc.bpm - 1), fc.beats_per_bar);
      up.onclick = () => set(fc.on, Math.min(300, fc.bpm + 1), fc.beats_per_bar);
      const tap = el('button', 'btn big', 'Tap');
      tap.onclick = () => {
        const now = performance.now(); taps = taps.filter(t => now - t < 2500); taps.push(now);
        if (taps.length >= 3) { const iv = (taps[taps.length - 1] - taps[0]) / (taps.length - 1); set(fc.on, Math.round(60000 / iv * 2) / 2, fc.beats_per_bar); }
      };
      const go = el('button', 'btn go' + (fc.on ? ' on' : ''), fc.on ? 'Stop click' : 'Start click');
      go.onclick = () => set(!fc.on, fc.bpm, fc.beats_per_bar);
      const meter = seg([[2, '2/4'], [3, '3/4'], [4, '4/4'], [6, '6/8']], fc.beats_per_bar, v => set(fc.on, fc.bpm, v), 'Beats per bar');
      const song = el('button', 'btn', 'Song tempo'); song.disabled = !V.song; song.onclick = () => set(fc.on, Math.round(playBpm() * 100) / 100, V.song.beats_per_bar);
      const tempo = el('div', 'clk');
      tempo.append(dn, el('span', 'bpm screen', String(+fc.bpm.toFixed(2))), up, el('span', 'lbl', 'BPM'));
      const ctl = el('div', 'clk');
      ctl.append(go, tap, song);
      const fdiv = seg(divs, fc.div || 1, v => set(fc.on, fc.bpm, fc.beats_per_bar, v), 'Click subdivision');
      c.append(tempo, el('div', 'crow', null), ctl, meter, fdiv, el('p', 'hint', 'Between songs, during announcements, or for a song with no tracks. Plays on the click output at the click level and stops when a song starts. Also on a MIDI button (Setup → MIDI).'));
      cards.append(sc, c);
      box.append(cards);
    }
  }
  let taps = [];
  // Key change, staged: tap − / + as many times as you like (nothing happens to the tracks
  // yet), then "Change key" re-pitches them once. Cancel goes back.
  let keyTarget = null, keyFor = null;
  const keyLabel = (s, t) => s.key ? keyName(s.key, t) : (t ? (t > 0 ? '+' : '') + t : '—');
  function showKey() {
    const s = V && V.song, pend = !!s && keyTarget != null && keyTarget !== s.transpose;
    $('#lvKey').textContent = s ? keyLabel(s, pend ? keyTarget : s.transpose) : '—';
    $('#lvKey').classList.toggle('pending', pend);
    $('#lvKeyFrom').textContent = pend ? `from ${keyLabel(s, s.transpose)}` : '';
    $('#lvKeyApply').hidden = $('#lvKeyCancel').hidden = !pend;
  }
  function liveKey(d) {
    if (!V.song) return;
    keyTarget = Math.max(-12, Math.min(12, (keyTarget == null ? V.song.transpose : keyTarget) + d));
    showKey();
  }
  async function applyKey() {
    if (!V.song || keyTarget == null) return;
    if (playing()) { toast('Stop playback to change the key.'); return; }
    const t = keyTarget;
    showTask('Changing key', 0.3);
    try { const v = await invoke('set_key_tempo', { transpose: t, bpm: playBpm() }); keyTarget = null; songKey = null; await refresh(v); toast(`Now in ${keyName(V.song.key, V.song.transpose)}.`); }
    catch (e) { err(e); } finally { hideTask(); showKey(); }
  }
  $('#lvKeyDown').onclick = () => liveKey(-1);
  $('#lvKeyUp').onclick = () => liveKey(1);
  $('#lvKeyApply').onclick = applyKey;
  $('#lvKeyCancel').onclick = () => { keyTarget = null; showKey(); };

  function tickLive(L, cur, p) {
    if (page !== 'live') return;
    const s = V.song, now = $('.lnow');
    const o = cur >= 0 ? L[cur] : null;
    $('#lvSec').textContent = o ? o.b.section : (s ? s.name : 'No song');
    const setNote = (id, txt) => { const n = $(id); n.hidden = !txt; if (n.textContent !== (txt || '')) n.textContent = txt || ''; };
    setNote('#lvNote', o && o.b.note);
    now.style.setProperty('--c', o ? secColor(o.b.section) : 'var(--go)');
    const left = o && snap ? barsLeft(snap) : 0;
    const counting = snap && snap.counting_in;
    $('#lvBars').textContent = !p ? '–' : counting ? 'Count' : String(Math.max(0, Math.ceil(left - 0.001)));
    $('#lvBarsLbl').textContent = p && !counting ? (Math.ceil(left - 0.001) === 1 ? 'bar left' : 'bars left') : '';
    $('#lvProg').style.width = o && snap ? Math.min(100, snap.frame_in_section / Math.max(1, snap.section_frames) * 100) + '%' : '0';
    const bpb = s ? s.beats_per_bar : 4, beats = $('#lvBeats');
    if (beats.children.length !== bpb) { beats.innerHTML = ''; for (let i = 0; i < bpb; i++) beats.append(el('i')); }
    [...beats.children].forEach((d, i) => { d.className = (p && snap && snap.beat === i + 1 ? 'on' : '') + (i === 0 ? ' one' : ''); });
    const cb = document.querySelector('#lvTab .cbeats');
    if (cb) [...cb.children].forEach((d, i) => { d.className = (p && snap && snap.beat === i + 1 ? 'on' : '') + (i === 0 ? ' one' : ''); });
    const fb = $('#fadeNowBtn'), out = !!(snap && snap.tracks_out && p);
    if (fb && fb.classList.contains('out') !== out) { fb.classList.toggle('out', out); fb.textContent = out ? 'Tracks out · click on' : 'Fade out now'; }
    // The Live page clock: time in the song as it plays, and the song's length.
    const lt = $('#lvTime'), ltot = $('#lvTotal'), tEnd = L.length ? L[L.length - 1].start + L[L.length - 1].secs : 0;
    const tNow = s ? fmt(playTime(L)) : '0:00', tTot = '/ ' + fmt(tEnd);
    if (lt.textContent !== tNow) lt.textContent = tNow;
    if (ltot.textContent !== tTot) ltot.textContent = tTot;
    $('#lvPos').textContent = s ? `${p && snap && !counting ? snap.bar + '.' + snap.beat + ' · ' : ''}${+felt(s, (p && snap && snap.bpm) || (s.tempo_changes && s.tempo_changes.length ? s.tempo_changes[0][1] : playBpm())).toFixed(1)} BPM${s.key ? ' · ' + keyName(s.key, s.transpose) : ''}` : '';
    const nx = $('#lvNext'); let lbl = 'Next', name = '—'; nx.className = 'lnext';
    if (s && o) {
      if (snap && snap.pending != null && L[snap.pending]) {
        const bars = snap.frames_to_jump / s.sample_rate / barSec();
        const bt = Math.max(1, Math.round(bars * bpb));
        lbl = bars < 0.05 ? 'Going to' : `In ${bars < 1 ? bt + (bt === 1 ? ' beat' : ' beats') : bars.toFixed(1) + ' bars'}`;
        name = L[snap.pending].b.section; nx.classList.add('going');
      } else if (snap && snap.vamp) { lbl = 'Vamping'; name = o.b.section; nx.classList.add('vamp'); }
      else if (L[cur + 1]) name = L[cur + 1].b.section;
      else { lbl = 'Then'; const ns = V.setlist[V.current + 1]; name = snap && snap.finished ? 'Finished' : ns ? ns.name : 'End of set'; }
    }
    $('#lvNextLbl').textContent = lbl; $('#lvNextSec').textContent = name;
    const nxi = snap && snap.pending != null ? snap.pending : (snap && snap.vamp ? cur : cur + 1);
    setNote('#lvNextNote', o && L[nxi] && L[nxi].b.note);
    document.querySelectorAll('#lvBlocks .ltile').forEach(t => {
      const i = +t.dataset.i;
      t.classList.toggle('cur', i === cur); t.classList.toggle('done', p && i < cur); t.classList.toggle('queued', !!snap && snap.pending === i);
      t.querySelector('.tprog').style.width = i === cur && snap ? Math.min(100, snap.frame_in_section / Math.max(1, snap.section_frames) * 100) + '%' : '0';
    });
    // The strip follows the song: the playing block sits near the left, the one before it still in view.
    const strip = $('#lvBlocks'), ct = strip.querySelector(`.ltile[data-i="${cur}"]`);
    if (p && ct && cur !== liveFollowed) {
      liveFollowed = cur;
      const prev = strip.querySelector(`.ltile[data-i="${cur - 1}"]`);
      strip.scrollTo({ left: Math.max(0, (prev ? prev.offsetLeft + prev.offsetWidth * 0.5 : ct.offsetLeft) - 8), behavior: 'smooth' });
    }
    if (!p) liveFollowed = -1;
  }
  let liveFollowed = -1;

  /* ---------- typed arrangement ---------- */
  // "I, V1, PC, C1 x2, B, O" -> blocks. Abbreviations and number words work; offline, no AI.
  const ABBR = { i: 'intro', in: 'intro', int: 'intro', v: 'verse', vs: 'verse', c: 'chorus', ch: 'chorus', chor: 'chorus', pc: 'pre-chorus', pre: 'pre-chorus', prechorus: 'pre-chorus',
    b: 'bridge', br: 'bridge', t: 'tag', tg: 'tag', o: 'outro', out: 'outro', e: 'ending', end: 'ending', inst: 'instrumental', it: 'interlude', int2: 'interlude', ta: 'turnaround', turn: 'turnaround',
    ci: 'count-in', count: 'count-in', countin: 'count-in', r: 'refrain', ref: 'refrain', pst: 'post-chorus', post: 'post-chorus', postchorus: 'post-chorus', vamp: 'vamp', solo: 'solo', rap: 'rap', top: 'count-in' };
  const NUMW = { one: 1, two: 2, three: 3, four: 4, five: 5, six: 6, first: 1, second: 2, third: 3 };
  const normName = n => n.toLowerCase().replace(/\s+/g, ' ').replace(/pre ?chorus/, 'pre-chorus').replace(/post ?chorus/, 'post-chorus').replace(/count ?in/, 'count-in').trim();
  function parseArrangement(text, names) {
    const norm = names.map(n => [normName(n), n]);
    const out = [], bad = [];
    for (let raw of text.split(/[,;\n→>]+|->|\bthen\b/i)) {
      raw = raw.trim().toLowerCase(); if (!raw) continue;
      let times = 1;
      const rep = raw.match(/\s*(?:x\s*(\d+)|(\d+)\s*x|\*\s*(\d+)|\btwice\b)\s*$/);
      if (rep) { times = rep[0].includes('twice') ? 2 : +(rep[1] || rep[2] || rep[3]); raw = raw.slice(0, rep.index).trim(); }
      let t = raw.replace(/\b(one|two|three|four|five|six|first|second|third)\b/g, w => NUMW[w]);
      let m = t.match(/^([a-z][a-z \-]*?)\s*(\d+)?$/);
      if (!m) { bad.push(raw); continue; }
      let base = m[1].trim().replace(/\s+/g, ''), num = m[2];
      const full = ABBR[base] || normName(m[1].trim());
      const want = num ? `${full} ${num}` : full;
      let hit = norm.find(([n]) => n === want) || (!num && norm.find(([n]) => n === full + ' 1')) || (!num && norm.find(([n]) => n.startsWith(full + ' ')));
      if (!hit) { bad.push(raw); continue; }
      for (let k = 0; k < Math.max(1, Math.min(8, times)); k++) out.push(hit[1]);
    }
    return { names: out, bad };
  }
  function renderTypeArr() { /* the box stays empty: you say what to change */ }
  // Plain words go to Claude (with a preview to apply); a plain list is applied right away, offline.
  async function applyTyped() {
    if (!V.song) return;
    const text = $('#arrText').value.trim(); if (!text) return;
    const looksLikeList = /,|→|->/.test(text) && !/\b(double|repeat|remove|cut|drop|add|after|before|instead|then go|go straight|move|swap|take out|skip)\b/i.test(text);
    if (looksLikeList) {
      const { names, bad } = parseArrangement(text, V.song.sections.map(s => s.name));
      if (!bad.length && names.length) {
        // Keep each section's per-block edits (muted tracks, trims, notes) in order of use.
        const pool = blocks.map(b => ({ ...b, muted: [...b.muted], trims: copyTrims(b) }));
        const nb = names.map(n => { const k = pool.findIndex(b => b.section === n); return k >= 0 ? pool.splice(k, 1)[0] : { uid: uidSeq++, section: n, muted: [], trims: [], note: '' }; });
        $('#arrText').value = ''; $('#arrText').blur();
        commit(nb); return;
      }
    }
    // Plain words: worked out right here (free, offline, instant)…
    const local = window.interpretArrangement ? window.interpretArrangement(text, V.song.sections.filter(s => s.secs > 0.05).map(s => s.name), blocks) : { error: '' };
    if (!local.error) { showProposal(local); return; }
    // …and only if that didn't understand it, Claude, when you've added a key.
    if (!V.has_api_key) { toast(local.error, 7000); return; }
    const box = $('#proposal');
    box.hidden = false; box.className = 'proposal busy'; box.textContent = 'Working out the new arrangement…';
    $('#arrApply').disabled = true;
    try {
      const p = await invoke('ask_arrangement', { request: text, blocks: blocks.map(b => ({ section: b.section, note: b.note || '', muted: [...b.muted] })) });
      showProposal(p);
    } catch (e) { box.hidden = true; err(e); }
    finally { $('#arrApply').disabled = false; }
  }
  function showProposal(p) {
    const box = $('#proposal'); box.className = 'proposal'; box.innerHTML = ''; box.hidden = false;
    box.append(el('div', 'pmsg', p.message || 'Here is the new arrangement.'));
    const chips = el('div', 'pchips');
    p.blocks.forEach(b => { const c = el('span', b.from == null ? 'new' : null, b.section); c.style.setProperty('--c', secColor(b.section)); chips.append(c); });
    const row = el('div', 'prow2');
    const ok = el('button', 'btn accent', 'Apply'), no = el('button', 'btn', 'Cancel');
    no.onclick = () => { box.hidden = true; };
    ok.onclick = () => {
      // Each block keeps the edits of the block it came from; a block's first copy keeps its identity
      // (so a live change carries on from the block that's playing).
      const used = new Set();
      const nb = p.blocks.map(b => {
        const src = b.from != null ? blocks[b.from] : null;
        if (!src) return { uid: uidSeq++, section: b.section, muted: [], trims: [], note: '' };
        const first = !used.has(b.from); used.add(b.from);
        return { ...src, uid: first ? src.uid : uidSeq++, section: b.section, muted: [...src.muted], trims: copyTrims(src), note: first ? (src.note || '') : '' };
      });
      box.hidden = true; $('#arrText').value = '';
      commit(nb);
    };
    row.append(ok, no, el('span', 'hint', 'Cmd+Z undoes it after.'));
    box.append(chips, row);
  }
  $('#arrApply').onclick = applyTyped;
  $('#arrText').addEventListener('keydown', e => { if (e.key === 'Enter') { e.preventDefault(); applyTyped(); } else if (e.key === 'Escape') { e.target.value = ''; $('#proposal').hidden = true; e.target.blur(); } e.stopPropagation(); });

  /* ---------- fades: ending and the way into the next song ---------- */
  function setTransition(tr, ending) {
    invoke('set_transition', { transition: tr, ending: ending === undefined ? V.song.ending : ending }).then(v => refresh(v)).catch(err);
  }
  function stepper(val, fmt, onChange, label) {
    const w = el('span', 'num');
    const dn = el('button', 'ib', '−'), up = el('button', 'ib', '+');
    dn.setAttribute('aria-label', label + ' down'); up.setAttribute('aria-label', label + ' up');
    dn.onclick = () => onChange(-1); up.onclick = () => onChange(1);
    w.append(dn, el('b', null, fmt(val)), up);
    return w;
  }
  function seg(opts, cur, onPick, label) {
    const g = el('div', 'seg'); g.setAttribute('role', 'group'); g.setAttribute('aria-label', label);
    opts.forEach(([v, t]) => { const b = el('button', v === cur ? 'on' : null, t); b.onclick = () => { if (v !== cur) onPick(v); }; g.append(b); });
    return g;
  }
  // Cut to another song while one plays: it comes in on the next bar, blended (Setup → Blend).
  function cutTo(i) {
    if (!V || !V.setlist[i] || i === V.current && V.jump_to == null) return;
    invoke('cut_to_song', { index: i }).then(v => { refresh(v); toast(v.jump_to === i ? `${V.setlist[i].name} comes in at the next bar.` : 'Cut cancelled.'); }).catch(err);
  }
  function renderFades(box) {
    box.innerHTML = '';
    const s = V && V.song;
    box.hidden = !s;
    if (!s) return;
    const tr = { ...s.transition }, en = s.ending;
    const last = blocks[blocks.length - 1], lastSec = last && V.song.sections.find(x => x.name === last.section);
    const lastBars = lastSec ? Math.max(1, Math.round(lastSec.bars)) : 4;
    // Ending
    const g1 = el('div', 'fgrp');
    g1.append(el('span', 'flbl', 'Ending'));
    g1.append(seg([['full', 'Play out'], ['fade', 'Fade out']], en ? 'fade' : 'full', v => {
      if (playing()) { toast('Stop playback to change the ending.'); return; }
      setTransition(tr, v === 'fade' ? { at: Math.max(0, lastBars - 4), bars: Math.min(4, lastBars) } : null);
    }, 'Ending'));
    if (en) {
      const set = e2 => { if (playing()) { toast('Stop playback to change the ending.'); return; } setTransition(tr, e2); };
      g1.append(el('span', 'fst', `${last ? last.section : 'last block'}: from bar`));
      g1.append(stepper(en.at + 1, v => String(Math.round(v)), d => { const at = Math.max(0, Math.min(lastBars - 1, en.at + d)); set({ at, bars: Math.max(1, Math.min(en.bars, lastBars - at)) }); }, 'Fade start'));
      g1.append(el('span', 'fst', 'over'));
      g1.append(stepper(en.bars, v => String(v), d => set({ ...en, bars: Math.max(1, Math.min(lastBars - en.at, en.bars + d)) }), 'Fade length'));
      g1.append(el('span', 'fst', 'bars'));
    }
    box.append(g1);
    // Into the next song
    const nx = V.current != null ? V.setlist[V.current + 1] : null;
    const g2 = el('div', 'fgrp');
    g2.append(el('span', 'flbl', nx ? 'Into ' + nx.name : 'Last song'));
    if (nx) {
      g2.append(seg([['Stop', 'Stop'], ['Segue', 'Segue'], ['Crossfade', 'Crossfade'], ['Smooth', 'Smooth']], tr.mode, v => setTransition({ ...tr, mode: v }), 'Into the next song'));
      if (tr.mode === 'Smooth') g2.append(el('span', 'fst', 'downbeat · tempo glide · wash'));
      if (tr.mode === 'Crossfade') {
        g2.append(stepper(tr.bars, v => String(v), d => setTransition({ ...tr, bars: Math.max(1, Math.min(16, tr.bars + d)) }), 'Crossfade length'));
        g2.append(el('span', 'fst', 'bars'));
      }
      if (tr.mode !== 'Stop') g2.append(el('span', 'fst' + (V.next_ready ? ' ok' : ''), V.next_ready ? '● ready' : 'loading…'));
    }
    // When it stops: stay here, cue the next song, or go back to the top of the set.
    if (!nx || tr.mode === 'Stop') {
      const opts = nx ? [['Stay', 'Stay'], ['CueNext', 'Cue next'], ['Top', 'Top of set']] : [['Stay', 'Stay'], ['Top', 'Top of set']];
      let cur = tr.after || 'Stay'; if (!nx && cur === 'CueNext') cur = 'Stay';
      g2.append(el('span', 'fst', 'then'), seg(opts, cur, v => setTransition({ ...tr, after: v }), 'After this song'));
    }
    box.append(g2);
    // Medley: leave this song partway and enter the next one partway (with a segue or crossfade).
    if (nx && tr.mode !== 'Stop') {
      const gm = el('div', 'fgrp medley');
      gm.append(el('span', 'flbl', 'Medley'), el('span', 'fst', 'leave after'));
      const out = el('select'); out.setAttribute('aria-label', 'Leave this song after');
      out.append(new Option('the end', ''));
      blocks.forEach((b, i) => out.append(new Option(`${i + 1}. ${b.section}`, String(i))));
      out.value = tr.leave_after != null ? String(tr.leave_after) : '';
      out.onchange = () => setTransition({ ...tr, leave_after: out.value === '' ? null : +out.value });
      const into = el('select'); into.setAttribute('aria-label', `Start ${nx.name} at`);
      into.append(new Option('the top', ''));
      (nx.blocks || []).forEach((b, i) => into.append(new Option(`${i + 1}. ${b}`, String(i))));
      into.value = tr.enter_at != null ? String(tr.enter_at) : '';
      into.onchange = () => setTransition({ ...tr, enter_at: into.value === '' ? null : +into.value });
      gm.append(out, el('span', 'fst', `start ${nx.name} at`), into);
      box.append(gm);
    }
    // Pad underneath
    const g3 = el('div', 'fgrp padend');
    const pad = el('button', 'tog pad' + (tr.pad ? ' on' : ''), 'Pad under ending');
    pad.title = "Brings the pad in (this song's key) when the last block starts, and keeps it going into the next song";
    pad.onclick = () => { if (!V.pads.folder && !tr.pad) { toast('Choose your pads folder in Setup → Pads first.'); return; } setTransition({ ...tr, pad: !tr.pad }); };
    g3.append(pad);
    // Pad start (also on the Live page's Pads tab)
    g3.append(el('span', 'flbl', 'Pad start'), seg([['Off', 'No pad'], ['Intro', 'Through intro'], ['Song', 'Whole song']], tr.pad_start || 'Off', v => {
      if (v !== 'Off' && !V.pads.folder) { toast('Choose your pads folder in Setup → Pads first.'); return; }
      setTransition({ ...tr, pad_start: v });
    }, 'How the pad starts this song'));
    box.append(g3);
    const now = el('button', 'btn fadenow', 'Fade out now');
    if (box.id === 'lvFades') now.id = 'fadeNowBtn';
    now.title = 'Fades the tracks and guide over 2 bars; the click keeps going until you stop (also on a MIDI button)';
    now.onclick = () => { if (playing()) send({ type: 'FadeOut', bars: 2 }); };
    box.append(now);
  }

  /* ---------- section rail (drag to arrange) ---------- */
  function renderRail() {
    renderTypeArr();
    const rail = $('#rail'); rail.innerHTML = '';
    const L = layout();
    L.forEach(o => {
      const d = el('div', 'blk'); d.dataset.i = o.i; d.style.setProperty('--c', secColor(o.b.section));
      d.style.width = Math.max(96, o.b.section.length * 11 + 28, Math.min(220, o.sec.bars * 7)) + 'px';
      d.append(el('div', 'prog'), el('div', 'nm', o.b.section));
      if (o.b.note) d.append(el('div', 'note', o.b.note));
      d.append(el('div', 'sub', `${o.sec.bars < 0.95 ? 'pickup' : Math.round(o.sec.bars) + ' bars'}${o.b.muted.length ? ' · no ' + o.b.muted.join(', ') : ''}`));
      d.append(el('span', 'num', String(o.i + 1)));
      const tools = el('div', 'tools');
      const dup = el('button', null, '⧉'); dup.title = 'Play this section twice'; dup.setAttribute('aria-label', 'Duplicate');
      dup.onpointerdown = e => e.stopPropagation();
      dup.onclick = e => { e.stopPropagation(); const nb = blocks.slice(); nb.splice(o.i + 1, 0, { uid: uidSeq++, section: o.b.section, muted: [...o.b.muted], trims: copyTrims(o.b), note: '' }); commit(nb); };
      const x = el('button', null, '✕'); x.title = 'Remove from the arrangement'; x.setAttribute('aria-label', 'Remove');
      x.onpointerdown = e => e.stopPropagation();
      x.onclick = e => { e.stopPropagation(); commit(blocks.filter((_, j) => j !== o.i)); };
      const nt = el('button', null, '✎'); nt.title = 'Note for this block (shows on the Live page)'; nt.setAttribute('aria-label', 'Note');
      nt.onpointerdown = e => e.stopPropagation();
      nt.onclick = e => { e.stopPropagation(); editNote(d, o.i); };
      tools.append(nt, dup, x); d.append(tools);
      d.onpointerdown = e => startDrag(e, { kind: 'block', from: o.i, el: d, onClick: () => send({ type: 'Goto', block: o.i }) });
      rail.append(d);
    });
    tick();
  }
  // Inline note editor on a rail block: Enter saves, Escape cancels.
  function editNote(d, i) {
    const inp = el('input', 'noteedit'); inp.value = blocks[i].note || ''; inp.placeholder = 'e.g. WL repeats 2x · drums out';
    inp.setAttribute('aria-label', 'Note for ' + blocks[i].section);
    const old = d.querySelector('.note'); if (old) old.remove();
    d.querySelector('.nm').after(inp);
    inp.onpointerdown = e => e.stopPropagation();
    let done = false;
    const finish = save => {
      if (done) return; done = true;
      if (save && inp.value.trim() !== (blocks[i].note || '')) {
        const nb = blocks.map(b => ({ ...b, muted: [...b.muted], trims: copyTrims(b) }));
        nb[i].note = inp.value.trim().slice(0, 120); commit(nb);
      } else renderRail();
    };
    inp.onkeydown = e => { e.stopPropagation(); if (e.key === 'Enter') finish(true); else if (e.key === 'Escape') finish(false); };
    inp.onblur = () => finish(true);
    inp.focus();
  }
  function renderPalette() {
    const box = $('#palette'); box.innerHTML = '';
    if (!V.song) return;
    V.song.sections.filter(s => s.secs > 0.05).forEach(s => {
      const c = el('div', 'chip'); c.style.setProperty('--c', secColor(s.name));
      c.append(el('i'), document.createTextNode(s.name));
      const add = el('button', null, '+'); add.title = `Add ${s.name} at the end`; add.setAttribute('aria-label', `Add ${s.name}`);
      add.onpointerdown = e => e.stopPropagation();
      add.onclick = () => commit([...blocks, { uid: uidSeq++, section: s.name, muted: [], trims: [] }]);
      c.append(add);
      c.onpointerdown = e => startDrag(e, { kind: 'chip', section: s.name, el: c });
      box.append(c);
    });
  }

  let drag = null;
  function startDrag(e, d) {
    if (e.button !== 0) return;
    drag = { ...d, sx: e.clientX, sy: e.clientY, started: false, at: null, out: false };
    window.addEventListener('pointermove', dragMove);
    window.addEventListener('pointerup', dragUp, { once: true });
  }
  function dropIndex(x) {
    const bs = [...document.querySelectorAll('#rail .blk')];
    for (const b of bs) { const r = b.getBoundingClientRect(); if (x < r.left + r.width / 2) return +b.dataset.i; }
    return bs.length;
  }
  function dragMove(e) {
    if (!drag) return;
    if (!drag.started) {
      if (Math.hypot(e.clientX - drag.sx, e.clientY - drag.sy) < 6) return;
      drag.started = true;
      drag.ghost = drag.el.cloneNode(true); drag.ghost.classList.add('ghost');
      const r = drag.el.getBoundingClientRect(); drag.ghost.style.width = r.width + 'px'; drag.dx = e.clientX - r.left; drag.dy = e.clientY - r.top;
      document.body.append(drag.ghost);
      if (drag.kind === 'block') drag.el.classList.add('dragging');
    }
    drag.ghost.style.left = e.clientX - drag.dx + 'px'; drag.ghost.style.top = e.clientY - drag.dy + 'px';
    const rr = $('#rail').getBoundingClientRect();
    const inside = e.clientY > rr.top - 30 && e.clientY < rr.bottom + 30;
    document.querySelectorAll('#rail .drop').forEach(x => x.remove());
    drag.out = drag.kind === 'block' && !inside;
    drag.ghost.style.opacity = drag.out ? '0.35' : '0.9';
    drag.at = inside ? dropIndex(e.clientX) : null;
    if (drag.at != null) {
      const bs = document.querySelectorAll('#rail .blk');
      const ind = el('div', 'drop');
      if (drag.at < bs.length) $('#rail').insertBefore(ind, bs[drag.at]); else $('#rail').append(ind);
    }
  }
  function dragUp() {
    window.removeEventListener('pointermove', dragMove);
    const d = drag; drag = null;
    if (!d) return;
    if (!d.started) { if (d.onClick) d.onClick(); return; }
    d.ghost.remove(); d.el.classList.remove('dragging');
    document.querySelectorAll('#rail .drop').forEach(x => x.remove());
    if (d.kind === 'block') {
      if (d.out) { commit(blocks.filter((_, j) => j !== d.from)); return; }
      if (d.at == null || d.at === d.from || d.at === d.from + 1) return;
      const nb = blocks.slice(); const [m] = nb.splice(d.from, 1); nb.splice(d.at > d.from ? d.at - 1 : d.at, 0, m); commit(nb);
    } else if (d.at != null) {
      const nb = blocks.slice(); nb.splice(d.at, 0, { uid: uidSeq++, section: d.section, muted: [], trims: [] }); commit(nb);
    }
  }

  async function commit(nb, noUndo) {
    if (!V.song) return;
    if (!nb.length) { toast('The arrangement needs at least one section.'); renderRail(); return; }
    let keep = null;
    if (playing()) {
      const curUid = blocks[snap.item] && blocks[snap.item].uid;
      keep = nb.findIndex(b => b.uid === curUid);
      if (keep < 0) { toast("That's the section playing now. Remove it after it finishes."); renderRail(); return; }
    }
    if (!noUndo) { undo.push(blocks); if (undo.length > 60) undo.shift(); }
    const prev = blocks; blocks = nb; renderRail(); dirty = true;
    try {
      const v = await invoke('set_arrangement', { blocks: strip(nb), keep });
      if (keep != null && snap) snap = { ...snap, item: keep, pending: null };
      await refresh(v);
    }
    catch (e) { blocks = prev; if (!noUndo) undo.pop(); renderRail(); err(e); }
  }
  function doUndo() {
    if (!undo.length) { toast('Nothing to undo.'); return; }
    const prev = undo.pop();
    commit(prev.map(b => ({ ...b })), true);
  }
  function toggleClip(bi, stemName) {
    const nb = blocks.map(b => ({ ...b, muted: [...b.muted], trims: copyTrims(b) }));
    const b = nb[bi]; if (!b) return;
    b.muted = b.muted.includes(stemName) ? b.muted.filter(m => m !== stemName) : [...b.muted, stemName];
    commit(nb);
  }

  /* ---------- track headers ---------- */
  function applySolo() {
    V.song.stems.forEach((st, i) => send({ type: 'StemMuteLive', stem: i, muted: st.muted || (solo.size > 0 && !solo.has(i)) }));
  }
  function renderHeads() {
    const box = $('#heads');
    if (box.contains(document.activeElement) && (document.activeElement.type === 'range' || document.activeElement.tagName === 'SELECT')) return;
    box.innerHTML = '';
    if (!V.song) return;
    rows().forEach(row => {
      if (row.kind === 'bus') {
        const b = row.b, r = el('div', 'busrow' + (b.muted ? ' off' : '')); r.dataset.bus = b.id;
        const fold = el('button', 'fold', collapsed.has(b.id) ? '▸' : '▾');
        fold.title = collapsed.has(b.id) ? `Show ${b.name}` : `Fold ${b.name}`; fold.setAttribute('aria-label', fold.title);
        fold.onclick = () => { collapsed.has(b.id) ? collapsed.delete(b.id) : collapsed.add(b.id); saveCollapsed(); renderHeads(); dirty = true; };
        const nm = el('b', null, b.name);
        nm.title = `${b.stems.length} track${b.stems.length === 1 ? '' : 's'} · double-click to rename · drag a track here to move it to this bus`;
        nm.ondblclick = () => renameBus(b, nm);
        const f = el('input'); f.type = 'range'; f.min = 0; f.max = 1.5; f.step = 0.01; f.value = b.gain; f.id = 'bus-' + b.id; f.setAttribute('aria-label', `${b.name} bus level`);
        f.oninput = () => { b.gain = +f.value; send({ type: 'BusGain', bus: b.id, gain: b.gain }); };
        const m = el('button', 'm' + (b.muted ? ' on' : ''), 'M'); m.title = `Mute all of ${b.name}`; m.setAttribute('aria-pressed', b.muted);
        m.onclick = async () => { b.muted = !b.muted; await send({ type: 'BusMute', bus: b.id, muted: b.muted }); if (solo.size) applySolo(); renderHeads(); dirty = true; };
        const top = el('div', 'bt'); top.append(fold, nm);
        if (b.custom) { const x = el('button', 'del', '✕'); x.title = `Delete ${b.name} (its tracks go back to their usual bus)`; x.setAttribute('aria-label', x.title); x.onclick = () => busEdit({ type: 'Delete', bus: b.id }); top.append(x); }
        else top.append(el('span'));
        top.append(m);
        const bot = el('div', 'bb'); bot.append(f, el('span', 'busout', routeLabel(b.route)));
        r.append(top, bot);
        box.append(r);
        return;
      }
      const i = row.i, st = V.song.stems[i];
      const r = el('div', 'lane'); r.style.setProperty('--c', stemColor(st.name, i));
      const ln = el('div', 'ln'); const tn = el('b', 'grab', st.name); tn.title = 'Drag onto a bus header to move this track to that bus'; ln.append(tn);
      tn.onpointerdown = e => startTrackDrag(e, st.name, r);
      const f = el('input'); f.type = 'range'; f.min = 0; f.max = 1.5; f.step = 0.01; f.value = st.gain; f.id = 'fader-' + i; f.setAttribute('aria-label', `${st.name} level`);
      f.oninput = () => { st.gain = +f.value; send({ type: 'StemGain', stem: i, gain: st.gain }); };
      ln.append(f);
      const ms = el('div', 'ms');
      const m = el('button', 'm' + (st.muted ? ' on' : ''), 'M'); m.title = 'Mute'; m.setAttribute('aria-pressed', st.muted);
      m.onclick = async () => { st.muted = !st.muted; await send({ type: 'StemMute', stem: i, muted: st.muted }); if (solo.size) applySolo(); renderHeads(); dirty = true; };
      const sb = el('button', 's' + (solo.has(i) ? ' on' : ''), 'S'); sb.title = 'Solo'; sb.setAttribute('aria-pressed', solo.has(i));
      sb.onclick = () => { solo.has(i) ? solo.delete(i) : solo.add(i); applySolo(); renderHeads(); dirty = true; };
      ms.append(m, sb);
      const hm = el('div', 'hmeter'); hm.dataset.meter = 'stem:' + i; hm.append(el('i'));
      r.append(el('div', 'sw'), ln, ms, hm);
      box.append(r);
    });
    const nb = el('button', 'newbus', '+ New bus'); nb.title = 'Make a bus, then drag tracks onto it';
    nb.onclick = () => busEdit({ type: 'Create', name: 'New bus' }).then(() => { const last = [...document.querySelectorAll('#heads .busrow')].pop(); const nm = last && last.querySelector('b'); const b = V.buses.find(x => x.id === (last && last.dataset.bus)); if (b && nm) renameBus(b, nm); });
    box.append(nb);
  }

  /* ---------- buses: rename, move tracks ---------- */
  function busEdit(edit) { return invoke('bus_edit', { edit }).then(refresh).catch(err); }
  function renameBus(b, nameEl) {
    const inp = el('input', 'rename'); inp.value = b.name; inp.setAttribute('aria-label', 'Bus name');
    nameEl.replaceWith(inp); inp.focus(); inp.select();
    let done = false;
    const finish = save => { if (done) return; done = true; const v = inp.value.trim(); if (save && v && v !== b.name) busEdit({ type: 'Rename', bus: b.id, name: v }); else renderHeads(); };
    inp.onkeydown = e => { if (e.key === 'Enter') finish(true); else if (e.key === 'Escape') finish(false); e.stopPropagation(); };
    inp.onblur = () => finish(true);
  }
  let trackDrag = null;
  function startTrackDrag(e, track, rowEl) {
    if (e.button !== 0) return;
    trackDrag = { track, rowEl, sx: e.clientX, sy: e.clientY, started: false, over: null };
    window.addEventListener('pointermove', trackDragMove);
    window.addEventListener('pointerup', trackDragUp, { once: true });
  }
  function trackDragMove(e) {
    const d = trackDrag; if (!d) return;
    if (!d.started) { if (Math.hypot(e.clientX - d.sx, e.clientY - d.sy) < 6) return; d.started = true; d.ghost = el('div', 'trackghost', d.track); document.body.append(d.ghost); }
    d.ghost.style.left = e.clientX + 12 + 'px'; d.ghost.style.top = e.clientY - 10 + 'px';
    const target = document.elementFromPoint(e.clientX, e.clientY);
    const busRow = target && target.closest && target.closest('.busrow');
    document.querySelectorAll('.busrow.dropto').forEach(x => x.classList.remove('dropto'));
    d.over = busRow ? busRow.dataset.bus : null;
    if (busRow) busRow.classList.add('dropto');
  }
  function trackDragUp() {
    window.removeEventListener('pointermove', trackDragMove);
    const d = trackDrag; trackDrag = null; if (!d || !d.started) return;
    d.ghost.remove(); document.querySelectorAll('.busrow.dropto').forEach(x => x.classList.remove('dropto'));
    if (d.over && d.over !== 'all') busEdit({ type: 'Assign', track: d.track, bus: d.over }).then(() => toast(`${d.track} now goes to ${(V.buses.find(b => b.id === d.over) || {}).name || 'that bus'} (in every song).`));
  }

  /* ---------- arrangement canvases ---------- */
  // The arrangement view's colours come from the theme (CSS variables), read once per theme.
  let palCache = null;
  function pal() {
    if (palCache) return palCache;
    const cs = getComputedStyle(document.documentElement), v = n => cs.getPropertyValue(n).trim();
    palCache = { ruler: v('--cv-ruler'), lanes: v('--cv-lanes'), laneAlt: v('--cv-lane-alt'), laneLine: v('--cv-lane-line'), bus: v('--cv-bus'),
      tick: v('--cv-tick'), tickStrong: v('--cv-tick-strong'), text: v('--muted'), dim: v('--dim'), fg: v('--fg'), go: v('--go'),
      grid: v('--cv-grid'), bar: v('--cv-bar'), beat: v('--cv-beat'), band: v('--cv-band'), scrub: v('--cv-scrub'), shade: v('--cv-shade') };
    return palCache;
  }
  const cvTop = $('#cvTop'), cvLanes = $('#cvLanes');
  function size(cv, h) {
    const dpr = window.devicePixelRatio || 1, w = cv.clientWidth;
    if (cv.width !== Math.round(w * dpr) || cv.height !== Math.round(h * dpr)) { cv.width = Math.round(w * dpr); cv.height = Math.round(h * dpr); cv.style.height = h + 'px'; }
    const g = cv.getContext('2d'); g.setTransform(dpr, 0, 0, dpr, 0, 0); return [g, w];
  }
  const X = t => (t - zoom.scroll) * zoom.pps;
  // Filled rectangle with soft corners (plain rectangle where the browser can't round).
  function rrect(c, x, y, w, hh, r) {
    if (c.roundRect && w > 2 * r) { c.beginPath(); c.roundRect(x, y, w, hh, r); c.fill(); } else c.fillRect(x, y, w, hh);
  }
  function total(L) { return L.length ? L[L.length - 1].start + L[L.length - 1].secs : 0; }
  function clampScroll(L, w) { zoom.scroll = Math.max(0, Math.min(zoom.scroll, Math.max(0, total(L) - (w - 40) / zoom.pps))); }
  function fit() {
    const L = layout(), w = cvTop.clientWidth;
    if (!L.length || !w) return;
    zoom.pps = Math.max(2, (w - 16) / Math.max(1, total(L))); zoom.scroll = 0; dirty = true;
  }
  function draw() {
    const L = layout(), s = V && V.song;
    const [g, w] = size(cvTop, 50);
    const C = pal(); g.fillStyle = C.ruler; g.fillRect(0, 0, w, 50);
    const R = rows(), H = rowsHeight(R);
    const [h, w2] = size(cvLanes, Math.max(1, H));
    h.fillStyle = C.lanes; h.fillRect(0, 0, w2, H);
    if (!s || !L.length) return;
    const p = playing(), now = playTime(L), bs = barSec();
    if (p && zoom.follow) { const x = X(now); if (x > w * 0.72 || x < 0) zoom.scroll = now - (w * 0.2) / zoom.pps; }
    clampScroll(L, w);
    const cur = snap ? Math.min(snap.item, L.length - 1) : -1;
    // ruler
    const step = [1, 2, 4, 8, 16, 32, 64].find(n => n * bs * zoom.pps >= 34) || 64;
    g.font = '10px ui-monospace, Menlo, monospace'; g.textBaseline = 'middle';
    // Bar lines sit on the audio's own downbeats, so every bar is the same length
    // and a section that doesn't start on a downbeat shows up instead of hiding.
    L.forEach(o => {
      for (let k = 0; k < o.beats.length; k++) {
        const x = X(o.start + (o.beats[k] - o.sec.start_sec)); if (x < -2 || x > w + 2) continue;
        const bar = o.bar + k;
        g.fillStyle = (bar - 1) % step === 0 ? C.tickStrong : C.tick;
        g.fillRect(Math.round(x), (bar - 1) % step === 0 ? 4 : 13, 1, (bar - 1) % step === 0 ? 16 : 7);
        if ((bar - 1) % step === 0) { g.fillStyle = C.text; g.fillText(String(bar), x + 3, 10); }
      }
    });
    // section lane
    L.forEach(o => {
      const x0 = X(o.start), x1 = X(o.start + o.secs); if (x1 < 0 || x0 > w) return;
      g.fillStyle = secColor(o.b.section); g.globalAlpha = o.i === cur ? 1 : 0.78;
      rrect(g, x0 + 1, RULER_H + 2, Math.max(1, x1 - x0 - 2), 26, 4); g.globalAlpha = 1;
      if (snap && snap.pending === o.i && Math.floor(performance.now() / 250) % 2) { g.strokeStyle = '#ffd23f'; g.lineWidth = 2; g.strokeRect(x0 + 2, RULER_H + 3, x1 - x0 - 4, 24); }
      g.save(); g.beginPath(); g.rect(x0 + 1, RULER_H, Math.max(0, x1 - x0 - 4), 30); g.clip();
      g.fillStyle = '#0b0b0c'; g.font = '800 12px ui-rounded, -apple-system, system-ui, sans-serif';
      g.fillText((snap && snap.vamp && o.i === cur ? '↻ ' : '') + o.b.section.toUpperCase(), x0 + 7, RULER_H + 15); g.restore();
    });
    // lanes
    const per = peaks ? peaks.per_sec : 100;
    R.forEach(row => {
      if (row.kind === 'bus') {
        h.fillStyle = C.bus; h.fillRect(0, row.y, w2, BUS_H);
        h.fillStyle = C.tick; h.fillRect(0, row.y + BUS_H - 1, w2, 1);
        return;
      }
      const r = row.i, st = s.stems[r], y = row.y, col = stemColor(st.name, r);
      const laneOff = st.muted || row.b.muted || (solo.size > 0 && !solo.has(r));
      h.fillStyle = r % 2 ? C.laneAlt : C.lanes; h.fillRect(0, y, w2, LANE_H);
      h.fillStyle = C.laneLine; h.fillRect(0, y + LANE_H - 1, w2, 1);
      const pk = peaks && peaks.stems[r];
      L.forEach(o => {
        const x0 = X(o.start), x1 = X(o.start + o.secs); if (x1 < 0 || x0 > w2) return;
        const off = laneOff || o.b.muted.includes(st.name);
        const top = y + 3, ch = LANE_H - 7;
        h.globalAlpha = off ? 0.08 : 0.2; h.fillStyle = off ? C.text : col; rrect(h, x0 + 1, top, Math.max(1, x1 - x0 - 2), ch, 3);
        h.globalAlpha = off ? 0.3 : 1; h.fillRect(x0 + 1, top, Math.max(1, x1 - x0 - 2), 2);
        if (pk) {
          h.globalAlpha = off ? 0.25 : 0.9; h.fillStyle = off ? C.text : col;
          const mid = top + 2 + (ch - 2) / 2, amp = (ch - 6) / 2;
          const a = Math.max(0, Math.floor(x0) + 1), b = Math.min(w2, Math.ceil(x1) - 1);
          for (let px = a; px < b; px++) {
            const t0 = o.sec.start_sec + (px - x0) / zoom.pps, t1 = t0 + 1 / zoom.pps;
            let i0 = Math.floor(t0 * per), i1 = Math.max(i0 + 1, Math.ceil(t1 * per)), m = 0;
            for (let i = i0; i < i1 && i < pk.length; i++) if (pk[i] > m) m = pk[i];
            const hh = Math.max(0.5, Math.sqrt(m / 255) * amp);
            h.fillRect(px, mid - hh, 1, hh * 2);
          }
        }
        h.globalAlpha = 1;
        // Trim: the parts of the block where this track is out are shaded; fades are ramps.
        const tr = !off && clipTrim(o, st.name);
        if (tr && !tr.full) {
          const bp = beatSec() * zoom.pps, xs = x0 + tr.from * bp, xe = x0 + tr.to * bp;
          h.fillStyle = C.shade;
          if (xs > x0 + 1) h.fillRect(x0 + 1, top, xs - x0 - 1, ch);
          if (xe < x1 - 1) h.fillRect(xe, top, x1 - 1 - xe, ch);
          h.fillStyle = col; h.fillRect(Math.round(xs), top, 2, ch); h.fillRect(Math.round(xe) - 2, top, 2, ch);
          const fi = tr.fade_in * bp, fo = tr.fade_out * bp;
          h.fillStyle = 'rgba(0,0,0,.5)'; h.strokeStyle = C.fg; h.lineWidth = 1;
          if (fi > 0) { h.beginPath(); h.moveTo(xs, top); h.lineTo(xs + fi, top); h.lineTo(xs, top + ch); h.closePath(); h.fill(); h.beginPath(); h.moveTo(xs, top + ch); h.lineTo(xs + fi, top); h.stroke(); }
          if (fo > 0) { h.beginPath(); h.moveTo(xe, top); h.lineTo(xe - fo, top); h.lineTo(xe, top + ch); h.closePath(); h.fill(); h.beginPath(); h.moveTo(xe - fo, top); h.lineTo(xe, top + ch); h.stroke(); }
        }
        if (tr && x1 - x0 > 30) {
          // fade handles at the top corners of the clip (or of its trim)
          const bp = beatSec() * zoom.pps, xs = x0 + tr.from * bp + tr.fade_in * bp, xe = x0 + tr.to * bp - tr.fade_out * bp;
          h.fillStyle = 'rgba(236,236,236,.7)'; h.fillRect(xs - 2, top, 5, 5); h.fillRect(xe - 3, top, 5, 5);
        }
        if (off && x1 - x0 > 50) { h.fillStyle = C.dim; h.font = '10px -apple-system, system-ui, sans-serif'; h.fillText(laneOff ? '' : 'off', x0 + 6, top + 13); }
        if (sel && sel.block === o.i && sel.stem === r) { h.strokeStyle = C.fg; h.lineWidth = 1.5; h.strokeRect(x0 + 1.5, top + 0.5, x1 - x0 - 3, ch); }
      });
    });
    // A block that fades out (the song's ending): a ramp over every lane.
    L.forEach(o => {
      if (!o.b.fade_out) return;
      const xa = X(o.start + o.b.fade_out[0] * bs), xb = X(o.start + o.secs);
      if (xb < 0 || xa > w2) return;
      h.fillStyle = 'rgba(0,0,0,.45)'; h.beginPath(); h.moveTo(xa, 0); h.lineTo(xb, 0); h.lineTo(xb, H); h.closePath(); h.fill();
      h.strokeStyle = '#ffd23f'; h.lineWidth = 1.5; h.beginPath(); h.moveTo(xa, 0); h.lineTo(xb, H); h.stroke();
      g.fillStyle = '#ffd23f'; g.font = '10px ui-monospace, Menlo, monospace'; g.fillText('fade', xa + 3, RULER_H - 6);
    });
    // Grid lines down every lane, like Ableton: a line on every bar (every few bars when
    // zoomed far out) and fainter beat lines once there's room. They sit on the song's
    // own downbeats, so they follow tempo changes.
    L.forEach(o => {
      const x0 = X(o.start), x1 = X(o.start + o.secs); if (x1 < 0 || x0 > w2) return;
      const n = o.beats.length, bpb = s.beats_per_bar || 4;
      const every = [1, 2, 4, 8, 16, 32, 64].find(m => m * bs * zoom.pps >= 8) || 64;
      for (let k = 0; k < n; k++) {
        const t = o.start + (o.beats[k] - o.sec.start_sec), x = X(t);
        const next = k + 1 < n ? X(o.start + (o.beats[k + 1] - o.sec.start_sec)) : x + bs * zoom.pps;
        const bar = o.bar + k;
        if (x > x0 + 1 && x < x1 - 1 && x >= 0 && x <= w2 && (bar - 1) % every === 0) { h.fillStyle = C.bar; h.fillRect(Math.round(x), 0, 1, H); }
        const bw = (next - x) / bpb;
        if (bw >= 9) {
          h.fillStyle = C.beat;
          for (let j = 1; j < bpb; j++) { const bx = x + j * bw; if (bx > x0 + 1 && bx < x1 - 1 && bx >= 0 && bx <= w2) h.fillRect(Math.round(bx), 0, 1, H); }
        }
      }
    });
    // block boundaries + playhead + queued jump
    // Every other four-bar phrase is shaded a touch, so phrases can be counted at a glance.
    h.fillStyle = C.band;
    L.forEach(o => {
      const x0 = X(o.start), x1 = X(o.start + o.secs); if (x1 < 0 || x0 > w2) return;
      for (let k = 0; k < o.beats.length; k++) {
        if (Math.floor((o.bar + k - 1) / 4) % 2 !== 1) continue;
        const a = Math.max(x0, X(o.start + (o.beats[k] - o.sec.start_sec)));
        const b2 = k + 1 < o.beats.length ? X(o.start + (o.beats[k + 1] - o.sec.start_sec)) : x1;
        h.fillRect(a, 0, Math.min(b2, x1) - a, H);
      }
    });
    h.fillStyle = C.grid; L.forEach(o => { const x = X(o.start); if (x >= 0 && x <= w2) h.fillRect(Math.round(x), 0, 1, H); });
    if (snap && snap.pending != null && p) {
      const jx = X(now + snap.frames_to_jump / s.sample_rate);
      [g, h].forEach((c, k) => { c.strokeStyle = '#ffd23f'; c.setLineDash([4, 3]); c.lineWidth = 1.5; c.beginPath(); c.moveTo(jx, k ? 0 : RULER_H); c.lineTo(jx, k ? H : 50); c.stroke(); c.setLineDash([]); });
    }
    if (scrub != null) {
      const sx = X(scrub);
      g.fillStyle = h.fillStyle = C.scrub;
      g.fillRect(sx - 1, 0, 2, 50); h.fillRect(sx - 1, 0, 2, H);
    }
    const px = X(now);
    g.fillStyle = h.fillStyle = p ? C.go : C.fg;
    g.fillRect(px - 1, 0, 2, 50); h.fillRect(px - 1, 0, 2, H);
    g.beginPath(); g.moveTo(px - 6, 0); g.lineTo(px + 6, 0); g.lineTo(px, 8); g.fill();
  }
  function loop() { if (V && (dirty || playing() || scrub != null || (snap && snap.pending != null))) { draw(); dirty = false; } requestAnimationFrame(loop); }
  requestAnimationFrame(loop);
  window.addEventListener('resize', () => { dirty = true; });

  const beatSec = () => 60 / playBpm();
  /** This track's trim in a block, in beats from the block's start ({ full: true } if untrimmed). */
  function clipTrim(o, name) {
    const full = o.secs / beatSec();
    const t = (o.b.trims || []).find(t => t.track === name);
    return t ? { ...t, to: Math.min(t.to, full) } : { track: name, from: 0, to: full, fade_in: 0, fade_out: 0, full: true };
  }
  // Trim a clip by dragging its edges; drag the handle at a top corner for a fade.
  let trimDrag = null;
  function trimTarget(e) {
    if (!V || !V.song) return null;
    const { o, y } = hit(e, cvLanes); if (!o) return null;
    const row = rows().find(r => y >= r.y && y < r.y + r.h);
    if (!row || row.kind !== 'stem') return null;
    const st = V.song.stems[row.i]; if (o.b.muted.includes(st.name)) return null;
    const x = e.clientX - cvLanes.getBoundingClientRect().left, top = row.y + 3, bp = beatSec() * zoom.pps, x0 = X(o.start);
    const tr = clipTrim(o, st.name), xs = x0 + tr.from * bp, xe = x0 + tr.to * bp;
    const near = (a, n = 6) => Math.abs(x - a) <= n;
    let mode = null;
    if (y < top + 10 && near(xs + tr.fade_in * bp)) mode = 'fade_in';
    else if (y < top + 10 && near(xe - tr.fade_out * bp)) mode = 'fade_out';
    else if (near(xs)) mode = 'from';
    else if (near(xe)) mode = 'to';
    return mode && { o, name: st.name, mode, tr };
  }
  cvLanes.addEventListener('pointerdown', e => {
    if (e.button !== 0 || lanesScrub) return;
    const r = cvLanes.getBoundingClientRect();
    if (V && V.song && Math.abs(e.clientX - r.left - X(playTime(layout()))) <= 6) return; // the playhead wins
    const t = trimTarget(e); if (!t) return;
    e.preventDefault(); e.stopImmediatePropagation(); cvLanes.setPointerCapture(e.pointerId);
    trimDrag = { ...t, before: blocks.map(b => ({ ...b, muted: [...b.muted], trims: copyTrims(b) })) };
  }, true);
  function applyTrim(e) {
    const d = trimDrag, o = d.o, full = o.secs / beatSec();
    const tb = (zoom.scroll + (e.clientX - cvLanes.getBoundingClientRect().left) / zoom.pps - o.start) / beatSec();
    const q = e.altKey ? 1 / 16 : 1; // snap to the beat (Alt: fine)
    const sn = v => Math.round(v / q) * q;
    const t = { ...d.tr }; delete t.full;
    if (d.mode === 'from') t.from = Math.max(0, Math.min(t.to - q, sn(tb)));
    if (d.mode === 'to') t.to = Math.min(full, Math.max(t.from + q, sn(tb)));
    if (d.mode === 'fade_in') t.fade_in = Math.max(0, Math.min(t.to - t.from - t.fade_out, sn(tb - t.from)));
    if (d.mode === 'fade_out') t.fade_out = Math.max(0, Math.min(t.to - t.from - t.fade_in, sn(t.to - tb)));
    t.fade_in = Math.min(t.fade_in, t.to - t.from); t.fade_out = Math.min(t.fade_out, t.to - t.from - t.fade_in);
    const b = blocks[o.i];
    const rest = (b.trims || []).filter(x => x.track !== d.name);
    const isFull = t.from <= 0 && t.to >= full - 1e-6 && !t.fade_in && !t.fade_out;
    blocks[o.i] = { ...b, trims: isFull ? rest : [...rest, t] };
    dirty = true;
  }
  cvLanes.addEventListener('pointermove', e => {
    if (trimDrag) { applyTrim(e); e.stopImmediatePropagation(); cvLanes.style.cursor = 'ew-resize'; return; }
    if (!lanesScrub && e.buttons === 0 && trimTarget(e)) { cvLanes.style.cursor = 'ew-resize'; e.stopImmediatePropagation(); }
  }, true);
  cvLanes.addEventListener('pointerup', e => {
    if (!trimDrag) return;
    e.stopImmediatePropagation();
    const d = trimDrag; trimDrag = null; suppressClick = true;
    const nb = blocks; blocks = d.before; // commit() records the undo step from the state before the drag
    if (JSON.stringify(strip(nb)) !== JSON.stringify(strip(d.before))) commit(nb.map(b => ({ ...b }))); else dirty = true;
  }, true);
  function clearTrim(bi, name) {
    const b = blocks[bi]; if (!b || !(b.trims || []).some(t => t.track === name)) return false;
    const nb = blocks.map(x => ({ ...x, trims: copyTrims(x) }));
    nb[bi].trims = nb[bi].trims.filter(t => t.track !== name);
    commit(nb); return true;
  }

  if (typeof window.__rigTest === 'function') window.__rigTest({ layout, X, rows }); // UI tests only

  function hit(e, cv) {
    const r = cv.getBoundingClientRect(), x = e.clientX - r.left, t = zoom.scroll + x / zoom.pps;
    const L = layout(); const o = L.find(o => t >= o.start && t < o.start + o.secs);
    return { o, y: e.clientY - r.top };
  }
  // Ruler (bar numbers): click or drag to move the playhead, snapped to the beat.
  // Section strip below it: click to jump to that section (using the Jump timing).
  let scrub = null; // arrangement time being dragged to
  function rulerTime(e) {
    const r = cvTop.getBoundingClientRect(), L = layout();
    let t = zoom.scroll + (e.clientX - r.left) / zoom.pps;
    const beat = 60 / playBpm();
    const o = L.find(o => t >= o.start && t < o.start + o.secs) || L[L.length - 1];
    if (!o) return null;
    // snap to the beat grid of the block's own audio
    const src = o.sec.start_sec + (t - o.start), off = V.song.offset_ms / 1000 * (V.song.bpm / playBpm());
    const snapped = off + Math.round((src - off) / beat) * beat;
    t = Math.max(o.start, Math.min(o.start + o.secs - 0.01, o.start + (snapped - o.sec.start_sec)));
    return Math.max(0, t);
  }
  function seekTo(t) {
    const L = layout(); const o = L.find(o => t >= o.start && t < o.start + o.secs); if (!o) return;
    send({ type: 'Seek', block: o.i, frame: Math.max(0, Math.round((t - o.start) * V.song.sample_rate)) });
  }
  cvTop.addEventListener('pointerdown', e => {
    if (!V || !V.song || e.button !== 0) return;
    const y = e.clientY - cvTop.getBoundingClientRect().top;
    if (y > RULER_H) { const { o } = hit(e, cvTop); if (o) send({ type: 'Goto', block: o.i }); return; }
    cvTop.setPointerCapture(e.pointerId);
    scrub = rulerTime(e); zoom.follow = false; dirty = true;
    if (!playing()) seekTo(scrub);
  });
  cvTop.addEventListener('pointermove', e => {
    const y = e.clientY - cvTop.getBoundingClientRect().top;
    cvTop.style.cursor = scrub != null ? 'ew-resize' : y <= RULER_H ? 'ew-resize' : 'pointer';
    if (scrub == null) return;
    scrub = rulerTime(e); dirty = true;
    if (!playing()) seekTo(scrub);
  });
  const endScrub = () => { if (scrub == null) return; const t = scrub; scrub = null; seekTo(t); zoom.follow = true; renderStatus(); dirty = true; };
  cvTop.addEventListener('pointerup', endScrub);
  cvTop.addEventListener('pointercancel', () => { scrub = null; dirty = true; });
  // Grab the playhead line itself anywhere in the lanes and drag it.
  cvLanes.addEventListener('pointerdown', e => {
    if (!V || !V.song || e.button !== 0) return;
    const r = cvLanes.getBoundingClientRect();
    if (Math.abs(e.clientX - r.left - X(playTime(layout()))) > 6) return;
    e.preventDefault(); cvLanes.setPointerCapture(e.pointerId);
    scrub = rulerTime(e); zoom.follow = false; dirty = true; lanesScrub = true;
  });
  let lanesScrub = false;
  cvLanes.addEventListener('pointermove', e => {
    const r = cvLanes.getBoundingClientRect();
    if (lanesScrub) { scrub = rulerTime(e); dirty = true; if (!playing()) seekTo(scrub); return; }
    cvLanes.style.cursor = V && V.song && Math.abs(e.clientX - r.left - X(playTime(layout()))) <= 6 ? 'ew-resize' : 'default';
  });
  cvLanes.addEventListener('pointerup', () => { if (lanesScrub) { lanesScrub = false; endScrub(); suppressClick = true; } });
  let suppressClick = false;
  cvLanes.addEventListener('click', e => {
    if (suppressClick) { suppressClick = false; return; }
    const { o, y } = hit(e, cvLanes); if (!o) return;
    const row = rows().find(r => y >= r.y && y < r.y + r.h);
    if (row && row.kind === 'stem') { sel = { block: o.i, stem: row.i }; dirty = true; }
  });
  cvLanes.addEventListener('dblclick', e => {
    const { o, y } = hit(e, cvLanes); const row = rows().find(r => y >= r.y && y < r.y + r.h); const st = row && row.kind === 'stem' && V.song.stems[row.i];
    if (o && st) toggleClip(o.i, st.name);
  });
  function onWheel(e) {
    const L = layout();
    if (e.ctrlKey || e.metaKey) {
      e.preventDefault();
      const r = cvTop.getBoundingClientRect(), x = e.clientX - r.left, t = zoom.scroll + x / zoom.pps;
      zoom.pps = Math.max(1, Math.min(400, zoom.pps * Math.exp(-e.deltaY * 0.01)));
      zoom.scroll = t - x / zoom.pps; clampScroll(L, r.width); dirty = true;
    } else if (Math.abs(e.deltaX) > Math.abs(e.deltaY) || e.shiftKey || e.currentTarget === cvTop) {
      e.preventDefault();
      zoom.scroll += (e.shiftKey && !e.deltaX ? e.deltaY : e.deltaX || e.deltaY) / zoom.pps; zoom.follow = false; clampScroll(L, cvTop.clientWidth); dirty = true; renderStatus();
    }
  }
  cvTop.addEventListener('wheel', onWheel, { passive: false });
  cvLanes.addEventListener('wheel', onWheel, { passive: false });
  // Zoom around the playhead: it stays where it is on screen (or comes into view).
  const zoomBy = f => {
    const w = cvTop.clientWidth, L = layout(), t = playTime(L);
    let x = (t - zoom.scroll) * zoom.pps;
    if (x < 0 || x > w) x = w * 0.3;
    zoom.pps = Math.max(1, Math.min(400, zoom.pps * f)); zoom.scroll = t - x / zoom.pps; clampScroll(L, w); dirty = true;
  };
  $('#zoomIn').onclick = () => zoomBy(1.5); $('#zoomOut').onclick = () => zoomBy(1 / 1.5); $('#zoomFit').onclick = fit;
  $('#followBtn').onclick = () => { zoom.follow = !zoom.follow; renderStatus(); };

  /* ---------- status bar ---------- */
  function renderStatus() {
    $('#stOut').textContent = V.output ? `${V.output.name} · ${V.output.channels} outs · ${(V.output.sample_rate / 1000).toFixed(1)} kHz` : 'No audio output';
    $('#stMidi').textContent = V.midi ? `MIDI: ${V.midi}` : 'No MIDI controller';
    $('#followBtn').classList.toggle('on', zoom.follow);
  }
  function showTask(text, p) { $('#stTask').hidden = false; $('#stTaskText').textContent = text; $('#stTaskBar').style.width = Math.round(p * 100) + '%'; }
  const hideTask = () => { $('#stTask').hidden = true; };

  /* ---------- songs ---------- */
  async function selectSong(i) {
    if (loading) return;
    if (playing()) { toast('Stop playback to change songs.'); return; }
    loading = true; renderBar(); showTask('Loading stems…', 0.3);
    try { await refresh(await invoke('select_song', { index: i })); } catch (e) { err(e); } finally { loading = false; hideTask(); renderBar(); }
  }
  let heard = [];
  async function analyze(auto) {
    if (analyzing || !V.song) return;
    if (playing()) { if (!auto) toast('Stop playback before reading the guide.'); return; }
    analyzing = true; renderSongDrawer(); showTask(auto ? 'Setting up this song…' : 'Reading the guide…', 0);
    try {
      const r = await invoke('analyze_song');
      heard = r.heard;
      await refresh(r.view);
      const s = V.song;
      toast(r.sections ? `Found ${r.sections} sections${r.found_tempo ? ` and the tempo (${+s.bpm.toFixed(2)} BPM)` : ''}. Drag sections in the rail to rearrange.`
        : r.found_tempo ? `Found the tempo (${+s.bpm.toFixed(2)} BPM). No section cues were found on the guide; add sections in Song.`
        : 'Couldn\'t read this song\'s click or guide. Set the tempo and sections in Song.', 7000);
    } catch (e) { err(e); } finally { analyzing = false; hideTask(); renderSongDrawer(); }
  }
  T.event.listen('analysis', e => { if (analyzing) showTask(e.payload.stage + '…', e.payload.progress); });

  function renderSetlist() {
    const ol = $('#songs'); ol.innerHTML = '';
    if (!V.setlist.length) ol.append(el('li', 'hint', 'No songs yet.'));
    V.setlist.forEach((s, i) => {
      const li = el('li', 'song' + (i === V.current ? ' cur' : '') + (s.found ? '' : ' missing')); li.tabIndex = 0;
      li.title = s.found ? s.path : `Folder not found: ${s.path}`;
      li.append(el('span', 'n', String(i + 1)), el('span', 'nm', s.name));
      const mv = el('span', 'mv');
      [['↑', -1, 'Move up'], ['↓', 1, 'Move down']].forEach(([t, d, l]) => { const b = el('button', null, t); b.title = l; b.setAttribute('aria-label', l); b.onclick = e => { e.stopPropagation(); invoke('move_song', { index: i, delta: d }).then(refresh).catch(err); }; mv.append(b); });
      const x = el('button', null, '✕'); x.title = 'Remove from set'; x.setAttribute('aria-label', 'Remove from set'); x.onclick = e => { e.stopPropagation(); invoke('remove_song', { index: i }).then(refresh).catch(err); }; mv.append(x);
      li.append(mv);
      li.onclick = () => selectSong(i); li.onkeydown = e => { if (e.key === 'Enter') selectSong(i); };
      ol.append(li);
    });
  }

  /* ---------- song drawer ---------- */
  let draftMarkers = null, draftFor = null;
  function renderSongDrawer() {
    const s = V.song;
    $('#songEmpty').hidden = !!s; $('#songBody').hidden = !s;
    if (!s) return;
    renderFades($('#songFades'));
    const tc = s.tempo_changes || [];
    $('#tempoMap').hidden = tc.length < 2;
    $('#tempoMap').textContent = tc.length < 2 ? '' : 'Tempo changes, read from the click (or the Ableton set): ' + tc.map(([b, t]) => `bar ${b}: ${+t.toFixed(1)}`).join(' · ') + '. The grid, click and bar jumps follow them. Changing the tempo by hand sets one steady tempo.';
    const sig = s.folder + JSON.stringify(s.markers);
    if (draftFor !== sig) { draftFor = sig; draftMarkers = s.markers.map(m => ({ ...m })); }
    $('#analyzeBtn').disabled = analyzing || !(s.has_guide || s.has_click);
    $('#analyzeBtn').textContent = analyzing ? 'Reading…' : 'Read guide';
    $('#analyzeHint').textContent = s.markers_from === 'ableton' ? 'Sections came from the Ableton set in this folder. Read guide replaces them with what the guide says.' : !(s.has_guide || s.has_click) ? 'No Click or Guide stem in this song. Add sections by hand below.'
      : `Reads the ${[s.has_click && 'Click (tempo)', s.has_guide && 'Guide (section cues)'].filter(Boolean).join(' and ')}. This replaces the markers below and resets the arrangement to the song as recorded.`;
    const hl = $('#heard'); hl.innerHTML = '';
    heard.forEach(([t, w]) => { const li = el('li'); li.append(fmt(t) + '  ', el('b', null, w || '…')); hl.append(li); });
    if (!document.activeElement || !document.activeElement.closest('#songDrawer .form')) {
      $('#f-name').value = s.name; $('#f-bpm').value = s.bpm; $('#f-bpb').value = s.beats_per_bar; $('#f-den').value = String(s.meter_den || 4); $('#f-key').value = s.key; $('#f-off').value = Math.round(s.offset_ms);
    }
    renderMarkers();
    renderKeyTempo();
  }
  let kt = null; // { transpose, bpm } being edited
  // Tracks that keep their pitch on a key change (click and guide always do).
  function renderKeepPitch() {
    const box = $('#kpChips'); if (!box) return; box.innerHTML = '';
    if (!V.song) return;
    V.song.stems.forEach(st => {
      if (/click|guide|cue/i.test(st.name)) return;
      const b = el('button', 'kpc' + (st.keeps_pitch ? ' on' : ''), st.name); b.setAttribute('aria-pressed', !!st.keeps_pitch);
      b.title = st.keeps_pitch ? `${st.name} stays as recorded when the key changes` : `${st.name} follows the key`;
      b.onclick = async () => {
        if (playing()) { toast('Stop playback to change this.'); return; }
        showTask('Updating', 0.3);
        try { await refresh(await invoke('set_keep_pitch', { stem: st.name, keep: !st.keeps_pitch })); } catch (e) { err(e); } finally { hideTask(); }
      };
      box.append(b);
    });
  }
  function renderKeyTempo() {
    renderKeepPitch();
    const s = V.song; if (!s) return;
    if (!kt || kt.folder !== s.folder) kt = { folder: s.folder, transpose: s.transpose, bpm: playBpm() };
    $('#keyVal').textContent = keyName(s.key, kt.transpose) + (kt.transpose ? ` (${kt.transpose > 0 ? '+' : ''}${kt.transpose})` : '');
    $('#keyOrig').textContent = s.key ? `Recorded in ${s.key}` : 'Add the recorded key under Song details to see key names';
    if (document.activeElement !== $('#bpmVal')) $('#bpmVal').value = +felt(s, kt.bpm).toFixed(2);
    $('#bpmOrig').textContent = `Recorded at ${+felt(s, s.bpm).toFixed(2)}${(s.felt_per_beat || 1) > 1 ? ' (dotted quarter)' : ''}`;
    const changed = kt.transpose !== s.transpose || Math.abs(kt.bpm - playBpm()) > 0.004;
    $('#ktApply').disabled = !changed || applyingKt;
    $('#ktApply').textContent = applyingKt ? 'Applying…' : 'Apply';
  }
  let applyingKt = false;
  $('#keyDown').onclick = () => { kt.transpose = Math.max(-12, kt.transpose - 1); renderKeyTempo(); };
  $('#keyUp').onclick = () => { kt.transpose = Math.min(12, kt.transpose + 1); renderKeyTempo(); };
  // The tempo boxes count in felt beats (dotted quarters in 6/8); kt.bpm is the grid's.
  const fpb = () => (V.song && V.song.felt_per_beat) || 1;
  const stepBpm = d => { const f = fpb(); const v = Math.round((kt.bpm / f + d) * 2) / 2; if (v >= 20 && v <= 300) kt.bpm = v * f; renderKeyTempo(); };
  $('#bpmDown').onclick = () => stepBpm(-0.5);
  $('#bpmUp').onclick = () => stepBpm(0.5);
  $('#bpmVal').oninput = e => { const v = +e.target.value; if (v >= 20 && v <= 300) { kt.bpm = v * fpb(); renderKeyTempo(); } };
  $('#ktReset').onclick = () => { kt.transpose = 0; kt.bpm = V.song.bpm; renderKeyTempo(); };
  $('#ktApply').onclick = async () => {
    if (playing()) { toast('Stop playback before changing the key or tempo.'); return; }
    applyingKt = true; renderKeyTempo(); showTask('Changing key and tempo', 0.3);
    try {
      const v = await invoke('set_key_tempo', { transpose: kt.transpose, bpm: kt.bpm });
      songKey = null; // stems changed: reload waveforms
      await refresh(v); kt = null;
      const s = V.song; toast(`Now in ${keyName(s.key, s.transpose)} at ${+felt(s, playBpm()).toFixed(2)} BPM.`);
    } catch (e) { err(e); } finally { applyingKt = false; hideTask(); renderKeyTempo(); }
  };

  function renderMarkers() {
    const box = $('#markers'); if (box.contains(document.activeElement)) return;
    box.innerHTML = '';
    draftMarkers.forEach((m, i) => {
      const r = el('div', 'mk'); const sw = el('i'); sw.style.setProperty('--c', secColor(m.name));
      const n = el('input'); n.value = m.name; n.id = 'mk-n-' + i; n.setAttribute('aria-label', 'Section name'); n.oninput = () => { m.name = n.value; };
      const b = el('input'); b.type = 'number'; b.min = 1; b.value = m.bar; b.id = 'mk-b-' + i; b.setAttribute('aria-label', 'Start bar'); b.oninput = () => { m.bar = Math.max(1, Math.round(+b.value || 1)); };
      const x = el('button', 'btn danger', '✕'); x.setAttribute('aria-label', 'Remove section'); x.onclick = () => { draftMarkers.splice(i, 1); renderMarkers(); };
      r.append(sw, n, b, x); box.append(r);
    });
  }
  $('#analyzeBtn').onclick = () => analyze(false);
  $('#addMarker').onclick = () => { const last = draftMarkers.reduce((m, x) => Math.max(m, x.bar), 0); draftMarkers.push({ name: 'Section ' + (draftMarkers.length + 1), bar: last + 8 }); renderMarkers(); };
  // Grid tools: save straight away, keeping each section on the same bar number.
  function saveGrid(patch, msg) {
    if (playing()) { toast('Stop playback before moving the grid.'); return; }
    const s = V.song;
    const meta = { name: s.name, bpm: s.bpm, beats_per_bar: s.beats_per_bar, offset_ms: s.offset_ms, key: s.key, markers: s.markers.map(m => ({ ...m })) };
    patch(meta);
    meta.offset_ms = Math.max(0, meta.offset_ms);
    invoke('update_song', { meta }).then(v => { refresh(v); toast(msg(meta)); }).catch(err);
  }
  const beatMs = () => 60000 / V.song.bpm;
  $('#gBeatL').onclick = () => saveGrid(m => { m.offset_ms -= beatMs(); }, m => `Grid moved a beat earlier (first downbeat ${Math.round(m.offset_ms)} ms).`);
  $('#gBeatR').onclick = () => saveGrid(m => { m.offset_ms += beatMs(); }, m => `Grid moved a beat later (first downbeat ${Math.round(m.offset_ms)} ms).`);
  $('#gMsL').onclick = () => saveGrid(m => { m.offset_ms -= 10; }, m => `First downbeat ${Math.round(m.offset_ms)} ms.`);
  $('#gMsR').onclick = () => saveGrid(m => { m.offset_ms += 10; }, m => `First downbeat ${Math.round(m.offset_ms)} ms.`);
  // Halving the tempo makes bars twice as long: section N bars in becomes N/2 bars in.
  $('#gHalf').onclick = () => saveGrid(m => { m.bpm = m.bpm / 2; m.markers.forEach(x => { x.bar = Math.round((x.bar - 1) / 2) + 1; }); }, m => `Tempo is now ${+m.bpm.toFixed(2)} BPM.`);
  $('#gDouble').onclick = () => saveGrid(m => { m.bpm = m.bpm * 2; m.markers.forEach(x => { x.bar = (x.bar - 1) * 2 + 1; }); }, m => `Tempo is now ${+m.bpm.toFixed(2)} BPM.`);

  $('#songSave').onclick = () => {
    const names = draftMarkers.map(m => m.name.trim().toLowerCase()).filter(Boolean);
    if (new Set(names).size !== names.length) { toast('Two sections have the same name. Give each its own (e.g. Chorus 1, Chorus 2).'); return; }
    invoke('update_song', { meta: { name: $('#f-name').value, bpm: +$('#f-bpm').value || V.song.bpm, beats_per_bar: Math.round(+$('#f-bpb').value || 4), meter_den: +$('#f-den').value || 4, offset_ms: +$('#f-off').value || 0, key: $('#f-key').value, markers: draftMarkers.map(m => ({ name: m.name.trim(), bar: m.bar })) } })
      .then(v => { refresh(v); toast('Song saved.'); }).catch(err);
  };

  /* ---------- setup drawer ---------- */
  const LEARN = [
    { label: 'Play / stop', tap: 'PlayStop' }, { label: 'Next block', tap: 'Next' }, { label: 'Repeat block', tap: 'Repeat' },
    { label: 'Vamp', tap: 'ToggleVamp' }, { label: 'Skip ahead', tap: 'SkipAhead', desc: 'Skips the next block too' }, { label: 'Next song', tap: 'NextSong' },
    { label: 'Fade out', tap: 'FadeOut', desc: 'Fades the song out over 2 bars and stops' }, { label: 'Panic stop', tap: 'Panic', desc: 'Stops everything at once' },
    { label: 'Pad only', tap: 'PadOnly', desc: 'Stops the song, keeps the pad' }, { label: 'Free click', tap: 'FreeClick', desc: 'Starts or stops the click with no song' },
    { label: 'Live / Edit page', tap: 'ToggleView' },
    { label: 'Master fader', tap: 'MasterLevel', desc: 'Everything the app sends out', fader: true },
  ];
  // Song buttons: one per place in the set (Song 1 = the first song, whatever it is that day).
  const SONG_BUTTONS = 10;
  function actionLabel(a) {
    if (a && a.Song !== undefined) return `Song ${a.Song + 1}`;
    if (a && a.BusLevel !== undefined) { const b = (V.all_buses || [])[a.BusLevel]; return `${b ? b.name : 'Bus ' + (a.BusLevel + 1)} fader`; }
    if (a && a.StemLevel !== undefined) return `${V.song && V.song.stems[a.StemLevel] ? V.song.stems[a.StemLevel].name : 'Stem ' + (a.StemLevel + 1)} fader`;
    return ({ PlayStop: 'Play / stop', Panic: 'Panic stop', Next: 'Next block', SkipAhead: 'Skip ahead', Repeat: 'Repeat block', ToggleVamp: 'Vamp', NextSong: 'Next song', PadOnly: 'Pad only', ClickLevel: 'Click fader', MasterLevel: 'Master fader', ToggleView: 'Live / Edit page', FadeOut: 'Fade out', FreeClick: 'Free click' })[a] || String(a);
  }
  const controlLabel = c => (c.Note ? `Note ${c.Note.key} · ch ${c.Note.ch + 1}` : c.Cc ? `CC ${c.Cc.num} · ch ${c.Cc.ch + 1}` : '?');
  function renderSetup() {
    const os = $('#outSel'); os.innerHTML = '';
    V.outputs.forEach(o => os.append(new Option(`${o.name} (${o.channels} outputs)${o.is_default ? ' · default' : ''}`, o.name)));
    if (V.output) os.value = V.output.name;
    routeOptions($('#clickRoute'), V.click_route);
    $('#apiKeyState').textContent = V.has_api_key ? 'A key is saved on this computer: Claude helps with requests the app does not understand.' : 'No key: everything works; the app reads common requests itself.';
    $('#apiKeyDel').hidden = !V.has_api_key;
    // Outputs: every bus, one master setting (per interface).
    const br = $('#busRoutes'); br.innerHTML = '';
    (V.all_buses || []).forEach(b => {
      const r = el('div', 'rt'); const sel = el('select'); sel.setAttribute('aria-label', `${b.name} output`); routeOptions(sel, b.route);
      sel.onchange = () => { b.route = keyRoute(sel.value); send({ type: 'BusRoute', bus: b.id, route: b.route }).then(() => refresh()); };
      r.append(el('span', null, b.name), sel); br.append(r);
    });
    if (document.activeElement !== $('#clickGain')) $('#clickGain').value = V.click_gain;
    $('#announceChk').checked = V.announce !== false;
    const bs = $('#blendSeg'); bs.innerHTML = '';
    bs.append(seg([['Cut', 'Cut'], ['Short', 'Short'], ['Beat', '1 beat'], ['Bar', '1 bar']], V.blend || 'Short', v => { V.blend = v; send({ type: 'Blend', blend: v }).then(() => renderSetup()); }, 'Blend'));
    $('#outHint').textContent = ((V.output && V.output.channels >= 4) ? 'By default Click & guide go to Out 3-4 (in-ears) and every other bus to Out 1-2 (front of house).' : 'Only 2 outputs, so everything plays on 1-2. Pick your audio interface to send click and guide to the in-ears.')
      + ' Plug in another interface and it gets its own routing; plug this one back in and yours comes back. If it is unplugged mid-service, the set stays loaded and it reconnects by itself.';
    renderPads();
    const ms = $('#midiSel'); ms.innerHTML = ''; ms.append(new Option('None', '')); V.midi_inputs.forEach(m => ms.append(new Option(m, m))); ms.value = V.midi || '';
    // Buttons, then the faders: master and each bus (tracks follow their bus).
    const songs = Array.from({ length: SONG_BUTTONS }, (_, i) => ({ label: `Song ${i + 1}`, tap: { Song: i }, desc: V.setlist[i] ? `Now: ${V.setlist[i].name}` : 'Goes to this place in the set', song: true }));
    const rows = [...LEARN.filter(r => !r.fader), ...songs, ...LEARN.filter(r => r.fader), ...(V.all_buses || []).map((b, i) => ({ label: `${b.name} fader`, tap: { BusLevel: i }, fader: true }))];
    const box = $('#learn'); box.innerHTML = '';
    rows.forEach((r, i) => {
      const d = el('div', 'lrow' + ((r.fader && !(rows[i - 1] || {}).fader) || (r.song && !(rows[i - 1] || {}).song) ? ' first-fader' : '')); const t = el('div'); t.append(el('b', null, r.label)); if (r.desc) t.append(el('small', null, r.desc));
      const b = el('button', 'btn' + (learnWait === i ? ' wait' : ''), learnWait === i ? 'Press it now…' : 'Learn');
      b.onclick = async () => {
        if (!V.midi) { toast('Choose your MIDI input first.'); return; }
        if (learnWait === i) { await invoke('midi_cancel'); learnWait = null; renderSetup(); return; }
        await invoke('midi_learn', { tap: r.tap, hold: r.hold || null }); learnWait = i; renderSetup();
      };
      d.append(t, b); box.append(d);
    });
    const bl = $('#bindings'); bl.innerHTML = '';
    if (!V.bindings.length) bl.append(el('p', 'hint', 'Nothing assigned yet.'));
    V.bindings.forEach((bd, i) => {
      const r = el('div', 'bind'); const t = el('span'); t.append(el('code', null, controlLabel(bd.control)), ' → ' + actionLabel(bd.tap) + (bd.hold ? ` (hold: ${actionLabel(bd.hold)})` : ''));
      const x = el('button', 'btn danger', 'Remove'); x.onclick = () => invoke('midi_unbind', { index: i }).then(refresh).catch(err);
      r.append(t, x); bl.append(r);
    });
  }
  function renderPads() {
    const p = V.pads;
    $('#padHint').textContent = p.folder ? `${p.folder} · keys: ${p.keys.join(', ') || 'none found'}` : 'Pads are sustained tonic drones, one audio file per key (like MultiTracks pad packs). The key goes in each file\'s name, e.g. "A Pad.wav".';
    $('#padFolder').textContent = p.folder ? 'Change pads folder' : 'Choose pads folder';
    if (document.activeElement !== $('#padLevel')) $('#padLevel').value = p.level;
    routeOptions($('#padRoute'), p.route);
  }
  $('#padFolder').onclick = () => invoke('choose_pads_folder').then(v => { refresh(v); toast('Pads folder set.'); }).catch(err);
  $('#padLevel').oninput = e => { V.pads.level = +e.target.value; invoke('pad_mix', { level: V.pads.level, route: null }).catch(err); };
  $('#padRoute').onchange = e => { V.pads.route = keyRoute(e.target.value); invoke('pad_mix', { level: null, route: V.pads.route }).catch(err); };

  $('#outSel').onchange = e => { if (playing()) { toast('Stop playback before changing the audio output.'); renderSetup(); return; } invoke('choose_output', { name: e.target.value }).then(v => { refresh(v); toast('Audio output changed.'); }).catch(err); };
  $('#announceChk').onchange = e => { V.announce = e.target.checked; send({ type: 'Announce', on: V.announce }); };
  $('#clickRoute').onchange = e => { V.click_route = keyRoute(e.target.value); send({ type: 'ClickRoute', route: V.click_route }); };
  $('#clickGain').oninput = e => { V.click_gain = +e.target.value; send({ type: 'ClickGain', gain: V.click_gain }); };
  $('#midiSel').onchange = e => invoke('choose_midi', { name: e.target.value || null }).then(refresh).catch(err);

  /* ---------- drawers ---------- */
  // Side panels open below the top bar, so the transport and panel buttons stay usable.
  const placeDrawers = () => document.documentElement.style.setProperty('--bar-b', Math.round(document.querySelector('.bar').getBoundingClientRect().bottom) + 'px');
  window.addEventListener('resize', placeDrawers); placeDrawers();
  const drawers = { setBtn: '#setDrawer', songBtn: '#songDrawer', setupBtn: '#setupDrawer' };
  function openDrawer(id) { placeDrawers(); Object.values(drawers).forEach(d => { $(d).hidden = d !== id || !$(d).hidden; }); if (id === '#setupDrawer') invoke('app_state').then(refresh); }
  Object.entries(drawers).forEach(([b, d]) => { $('#' + b).onclick = () => openDrawer(d); });
  document.querySelectorAll('[data-close]').forEach(b => { b.onclick = () => { b.closest('.drawer').hidden = true; }; });
  const anyDrawer = () => Object.values(drawers).some(d => !$(d).hidden);

  /* ---------- transport ---------- */
  $('#playBtn').onclick = () => send({ type: 'PlayStop' });
  document.querySelectorAll('[data-cmd]').forEach(b => { b.onclick = () => send({ type: b.dataset.cmd }); });
  $('#quant').onclick = e => { const b = e.target.closest('[data-q]'); if (!b) return; V.quantize = b.dataset.q; send({ type: 'Quantize', q: V.quantize }); renderBar(); };
  $('#clickBtn').onclick = () => { if (!V.song) return; V.song.click = !V.song.click; send({ type: 'Click', on: V.song.click }); renderBar(); };
  async function togglePad() {
    try { await refresh(await invoke('pad_set', { on: !V.pads.on })); }
    catch (e) { err(e); if (!V.pads.folder) openDrawer('#setupDrawer'); }
  }
  $('#padBtn').onclick = togglePad;
  $('#countBtn').onclick = () => { V.count_in = !V.count_in; send({ type: 'CountIn', on: V.count_in }); renderBar(); };
  $('#prevSong').onclick = () => V.current != null && selectSong(V.current - 1);
  $('#nextSong').onclick = () => V.current != null && selectSong(V.current + 1);
  $('#addSongs').onclick = () => invoke('add_songs').then(refresh).catch(err);

  /* ---------- new click song ---------- */
  let cSecs = [];
  function renderClickSecs() {
    const box = $('#c-secs'); box.innerHTML = '';
    cSecs.forEach((sec, i) => {
      const r = el('div', 'cs');
      const n = el('input'); n.value = sec[0]; n.id = 'cs-n-' + i; n.setAttribute('aria-label', 'Section name'); n.oninput = () => { sec[0] = n.value; };
      const b = el('input'); b.type = 'number'; b.min = 1; b.value = sec[1]; b.id = 'cs-b-' + i; b.setAttribute('aria-label', 'Bars'); b.oninput = () => { sec[1] = Math.max(1, Math.round(+b.value || 1)); updateClickTotal(); };
      const x = el('button', 'btn danger', '✕'); x.type = 'button'; x.setAttribute('aria-label', 'Remove section'); x.onclick = () => { cSecs.splice(i, 1); renderClickSecs(); };
      r.append(n, b, el('span', null, 'bars'), x); box.append(r);
    });
    updateClickTotal();
  }
  function updateClickTotal() {
    const bars = cSecs.reduce((a, s) => a + s[1], 0), bpm = +$('#c-bpm').value || 72, bpb = +$('#c-bpb').value || 4;
    $('#c-total').textContent = `${bars} bars + 1 count bar · ${fmt((bars + 1) * bpb * 60 / bpm)}`;
  }
  $('#newClick').onclick = () => {
    cSecs = [['Intro', 4], ['Verse 1', 8], ['Chorus', 8], ['Verse 2', 8], ['Chorus', 8], ['Bridge', 8], ['Chorus', 8], ['Outro', 4]];
    $('#c-name').value = ''; renderClickSecs(); $('#clickModal').hidden = false; $('#c-name').focus();
  };
  $('#c-add').onclick = () => { cSecs.push(['Section', 8]); renderClickSecs(); };
  $('#clickCancel').onclick = () => { $('#clickModal').hidden = true; };
  $('#c-bpm').oninput = updateClickTotal; $('#c-bpb').oninput = updateClickTotal;
  $('#clickForm').onsubmit = async e => {
    e.preventDefault();
    const spec = { name: $('#c-name').value.trim(), bpm: +$('#c-bpm').value, beats_per_bar: Math.round(+$('#c-bpb').value), key: $('#c-key').value.trim(),
      sections: cSecs.filter(s => s[0].trim()).map(s => [s[0].trim(), s[1]]), guide: $('#c-guide').checked };
    if (!spec.name) { toast('Give the song a name.'); return; }
    $('#c-go').disabled = true; $('#c-go').textContent = 'Creating…';
    try {
      const i = await invoke('create_click_song', { spec });
      $('#clickModal').hidden = true; await refresh();
      if (!playing()) await selectSong(i);
      toast(`${spec.name} is ready. It's in Music → Playback Rig → Click Songs.`);
    } catch (e2) { err(e2); } finally { $('#c-go').disabled = false; $('#c-go').textContent = 'Create song'; }
  };
  document.addEventListener('keydown', e => {
    // In a form control the keys are its own, except Escape on a switch, picker or slider:
    // that still closes the open panel.
    if (e.target.closest('input,select,textarea')) {
      const t = e.target, text = t.tagName === 'TEXTAREA' || (t.tagName === 'INPUT' && !['checkbox', 'radio', 'range', 'button'].includes(t.type));
      if (e.key !== 'Escape' || text) return;
      t.blur();
    }
    const k = e.key;
    if ((e.metaKey || e.ctrlKey) && (k === 'z' || k === 'Z')) { e.preventDefault(); doUndo(); return; }
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (k === ' ') { e.preventDefault(); send({ type: 'PlayStop' }); }
    else if (k === 'n' || k === 'N' || k === 'ArrowRight') send({ type: 'Next' });
    else if (k === 'r' || k === 'R') send({ type: 'Repeat' });
    else if (k === 'v' || k === 'V') send({ type: 'Vamp' });
    else if (k === 'l' || k === 'L') setPage(page === 'live' ? 'edit' : 'live');
    else if (k === 'f' || k === 'F') { zoom.follow = !zoom.follow; renderStatus(); }
    else if (k === '=' || k === '+') zoomBy(1.5);
    else if (k === '-') zoomBy(1 / 1.5);
    else if (k === '0' && sel && V.song && V.song.stems[sel.stem]) toggleClip(sel.block, V.song.stems[sel.stem].name);
    else if ((k === 'Backspace' || k === 'Delete') && sel && V.song && V.song.stems[sel.stem]) { if (!clearTrim(sel.block, V.song.stems[sel.stem].name)) toast('That clip isn\'t trimmed.'); }
    else if (k === 'Escape') { if (anyDrawer()) Object.values(drawers).forEach(d => { $(d).hidden = true; }); else send({ type: 'CancelJump' }); }
    else if (/^[1-9]$/.test(k) && blocks[+k - 1]) send({ type: 'Goto', block: +k - 1 });
  });

  /* ---------- events from Rust ---------- */
  T.event.listen('stem-level', e => { const [i, g] = e.payload; if (!V || !V.song || !V.song.stems[i]) return; V.song.stems[i].gain = g; const f = $('#fader-' + i); if (f) f.value = g; });
  T.event.listen('click-level', e => { if (V) V.click_gain = e.payload; });
  // A bus fader moved on the controller: the Live mixer follows (redrawn once per frame at most).
  let busRedraw = 0;
  T.event.listen('bus-level', e => {
    const [id, g] = e.payload; if (!V) return;
    [V.buses || [], V.all_buses || []].forEach(l => l.forEach(b => { if (b.id === id) b.gain = g; }));
    if (page === 'live' && liveTab === 'mix' && !busRedraw) busRedraw = requestAnimationFrame(() => { busRedraw = 0; renderLiveTab(); });
  });
  T.event.listen('master-level', e => { if (V) { V.master_gain = e.payload; if (page === 'live') renderMaster(); } });
  T.event.listen('midi-learned', () => { learnWait = null; refresh().then(() => toast('Control assigned.')); });
  T.event.listen('audio-error', e => toast('Audio problem: ' + e.payload));
  // Unplugged and plugged back in: the app reconnects by itself.
  let connWas = '';
  function renderConn(s) {
    const msgs = [];
    if (s.audio_lost) msgs.push(`Audio interface disconnected${V && V.output ? '' : ''}. Your set is still loaded; plug it back in and it reconnects by itself.`);
    if (s.midi_lost) msgs.push(`MIDI controller${V && V.midi ? ' (' + V.midi + ')' : ''} disconnected. It reconnects when plugged back in; use the Live page Mixer meanwhile.`);
    const txt = msgs.join(' ');
    if (txt === connWas) return;
    connWas = txt;
    $('#conn').hidden = !txt; $('#conn').textContent = txt;
  }
  T.event.listen('connection', e => {
    if (e.payload === 'audio-back') { toast('Audio interface reconnected. Press play to carry on.'); songKey = null; refresh(); }
    else if (e.payload === 'midi-back') toast('MIDI controller reconnected.');
  });
  T.event.listen('dropped', e => {
    const r = e.payload;
    songKey = null; // stems may have changed: reload waveforms
    refresh().then(() => {
      const parts = [];
      if (r.songs_added) parts.push(`Added ${r.songs_added} song${r.songs_added > 1 ? 's' : ''} to the set.`);
      if (r.tracks_added.length) parts.push(`Added ${r.tracks_added.join(', ')} to this song.`);
      if (r.message) parts.push(r.message);
      if (parts.length) toast(parts.join(' '), 6000);
      if (r.songs_added && V.current == null && V.setlist.length) selectSong(V.setlist.length - r.songs_added);
    });
  });
  T.event.listen('pedal', e => {
    if (e.payload === 'next-song') { if (playing()) { toast('Next song loads when this one stops.'); return; } if (V && V.current != null && V.current + 1 < V.setlist.length) selectSong(V.current + 1); }
    else if (typeof e.payload === 'string' && e.payload.startsWith('song:')) {
      // A song button on the controller: straight to that song in the set.
      const i = +e.payload.slice(5); if (!V || !V.setlist[i]) { toast(`There's no song ${i + 1} in the set.`); return; }
      if (playing()) { cutTo(i); return; }
      if (i !== V.current) selectSong(i);
    }
    else if (e.payload === 'pad-only') togglePad();
    else if (e.payload === 'toggle-view') setPage(page === 'live' ? 'edit' : 'live');
    else if (e.payload === 'refresh') refresh();
  });

  /* ---------- updates ---------- */
  let update = null, updating = false;
  async function checkUpdate(manual) {
    if (manual) { $('#checkUpd').disabled = true; $('#checkUpd').textContent = 'Checking…'; }
    try { update = await invoke('check_update'); }
    catch (e) { if (manual) err(e); }
    finally { $('#checkUpd').disabled = false; $('#checkUpd').textContent = 'Check for updates'; }
    renderUpdate();
    if (manual && !update) toast("You're on the latest version.");
  }
  function renderUpdate() {
    $('#stUpd').hidden = !update;
    if (!update) return;
    $('#updText').textContent = `Version ${update.version} is ready`;
    $('#updBtn').textContent = updating ? 'Installing…' : 'Install & restart';
    $('#updBtn').disabled = updating || playing();
    $('#updBtn').title = playing() ? 'Stop playback to update' : `Update from ${update.current} to ${update.version}`;
  }
  $('#updBtn').onclick = async () => {
    if (playing()) { toast('Stop playback before updating.'); return; }
    updating = true; renderUpdate(); showTask('Downloading update', 0);
    try { await invoke('install_update'); } // restarts the app on success
    catch (e) { err(e); updating = false; hideTask(); renderUpdate(); }
  };
  $('#checkUpd').onclick = () => checkUpdate(true);
  $('#apiKeySave').onclick = () => { const k = $('#apiKey').value.trim(); if (!k) { toast('Paste your key first.'); return; } invoke('set_api_key', { key: k }).then(v => { $('#apiKey').value = ''; refresh(v); toast('Key saved.'); }).catch(err); };
  $('#apiKeyDel').onclick = () => invoke('set_api_key', { key: '' }).then(v => { refresh(v); toast('Key removed.'); }).catch(err);
  T.event.listen('update-progress', e => showTask('Downloading update', e.payload));
  invoke('app_version').then(v => { $('#verText').textContent = `Playback Rig ${v}. Updates install only when playback is stopped.`; }).catch(() => {});
  setTimeout(() => checkUpdate(false), 4000);
  setInterval(() => { if (!update) checkUpdate(false); }, 6 * 3600 * 1000);

  // Level meters: each shows the loudest of its tracks, in dB (-60..+6), falling back smoothly.
  const meterShown = new Map();
  const meterHold = new Map();
  function showMeters(levels, master) {
    document.querySelectorAll('[data-meter]').forEach(m => {
      const [kind, id] = m.dataset.meter.split(':');
      let pk = 0;
      if (kind === 'stem') pk = levels[+id] || 0;
      else if (kind === 'master') pk = master || 0;
      else { const b = (V.buses || []).find(x => x.id === id); if (b) b.stems.forEach(i => { pk = Math.max(pk, levels[i] || 0); }); }
      const db = pk > 0 ? 20 * Math.log10(pk) : -90;
      const target = Math.max(0, Math.min(1, (db + 60) / 66));
      const prev = meterShown.get(m.dataset.meter) || 0;
      const shown = target >= prev ? target : Math.max(target, prev - 0.035);
      meterShown.set(m.dataset.meter, shown);
      // Peak hold: a thin line that stays at the loudest point for a moment, then falls.
      const now = performance.now(), hk = meterHold.get(m.dataset.meter) || { v: 0, t: 0 };
      if (shown >= hk.v) { hk.v = shown; hk.t = now; } else if (now - hk.t > 1200) hk.v = Math.max(shown, hk.v - 0.012);
      meterHold.set(m.dataset.meter, hk);
      m.style.setProperty('--pk', hk.v);
      const bar = m.firstChild; if (bar) { bar.style.setProperty('--lv', shown); bar.classList.toggle('clip', db > 0); }
    });
  }
  let readyPoll = 0;
  setInterval(async () => {
    try {
      const s = await invoke('status'); snap = s.snapshot;
      if (s.song_changed) { toast('Next song: ' + ((V.setlist[(V.current || 0) + 1] || {}).name || '')); await refresh(); return; }
      if (s.pad_changed) { await refresh(); return; }
      renderConn(s);
      if (s.levels) showMeters(s.levels, s.master_level);
      if (V && V.free_click && s.free_click !== V.free_click.on) { V.free_click.on = s.free_click; if (page === 'live') renderLiveTab(); }
      // The next song loads in the background: show when it's ready.
      if (V && V.song && !V.next_ready && V.current != null && V.setlist[V.current + 1] && ++readyPoll % 50 === 0) await refresh();
      if (update && !updating) $('#updBtn').disabled = playing();
      if (learnWait != null && !s.learning) { learnWait = null; await refresh(); } else tick();
    } catch (e) { /* closing */ }
  }, 40);

  (async () => {
    setTheme(theme);
    setPage(page);
    await refresh();
    if (V.current != null && !V.song && V.setlist[V.current] && V.setlist[V.current].found) selectSong(V.current);
  })();
})();

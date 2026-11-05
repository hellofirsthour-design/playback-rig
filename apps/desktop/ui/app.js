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
  const LANE_H = 46, RULER_H = 20;

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
  const strip = bs => bs.map(b => ({ section: b.section, muted: [...b.muted] }));
  const withUid = bs => bs.map(b => ({ uid: uidSeq++, section: b.section, muted: [...b.muted] }));

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
    if (/^top$/.test(n)) return '#4a4f58';
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
  function routeOptions(sel, current) {
    const n = V.output ? V.output.channels : 2;
    sel.innerHTML = '';
    for (let i = 0; i + 1 < n; i += 2) sel.append(new Option(`Out ${i + 1}-${i + 2}${n >= 4 ? (i === 0 ? ' · FOH' : i === 2 ? ' · In-ears' : '') : ''}`, 'p:' + i));
    for (let i = 0; i < n; i++) sel.append(new Option(n === 2 ? (i ? 'Right only' : 'Left only') : `Out ${i + 1} only`, 'm:' + i));
    sel.value = routeKey(current);
    if (sel.selectedIndex < 0) sel.value = 'p:0';
  }

  /* ---------- layout of the arrangement in time ---------- */
  function layout() {
    if (!V || !V.song) return [];
    let t = 0, bar = 1;
    return blocks.map((b, i) => {
      const sec = V.song.sections.find(x => x.name === b.section) || { start_sec: 0, secs: 0, bars: 0, bar: 1 };
      const o = { i, b, sec, start: t, secs: sec.secs, bar };
      t += sec.secs; bar += Math.round(sec.bars);
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
    }
    render();
  }
  function render() {
    renderBar(); renderRail(); renderPalette(); renderHeads(); renderSetlist(); renderSongDrawer(); renderSetup(); renderStatus();
    $('#emptyArr').hidden = !!(V && V.song);
    dirty = true; tick();
  }

  /* ---------- top bar ---------- */
  function renderBar() {
    const s = V.song;
    $('#songTitle').textContent = loading ? 'Loading…' : s ? s.name : (V.setlist.length ? 'Pick a song' : 'No songs yet');
    $('#songMeta').textContent = s ? `${+playBpm().toFixed(2)} BPM · ${s.beats_per_bar}/4${s.key || s.transpose ? ' · ' + keyName(s.key, s.transpose) : ''}${s.transpose ? ` (${s.transpose > 0 ? '+' : ''}${s.transpose})` : ''}` : '';
    document.querySelectorAll('#quant button').forEach(b => b.classList.toggle('on', b.dataset.q === V.quantize));
    $('#clickBtn').classList.toggle('on', !!(s && s.click)); $('#clickBtn').disabled = !s;
    $('#countBtn').classList.toggle('on', V.count_in);
  }
  function tick() {
    if (!V) return;
    const p = playing();
    const pb = $('#playBtn'); pb.classList.toggle('on', p); pb.setAttribute('aria-label', p ? 'Stop' : 'Play');
    $('#vampBtn').classList.toggle('on', !!(snap && snap.vamp));
    $('#prevSong').disabled = p || V.current == null || V.current <= 0;
    $('#nextSong').disabled = p || V.current == null || V.current >= V.setlist.length - 1;
    const s = V.song, L = layout();
    const cur = snap && L.length ? Math.min(snap.item, L.length - 1) : -1;
    $('#lcdSec').textContent = cur >= 0 ? L[cur].b.section : '—';
    $('#lcdBB').textContent = !snap || !s ? '–' : snap.counting_in ? 'Count' : `${snap.bar}.${snap.beat}`;
    const total = L.length ? L[L.length - 1].start + L[L.length - 1].secs : 0, now = playTime(L);
    $('#lcdTime').textContent = s ? `${fmt(now)} / ${fmt(total)}` : '';
    const left = cur >= 0 && snap ? (snap.section_frames - snap.frame_in_section) / s.sample_rate / barSec() : 0;
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
  }

  /* ---------- section rail (drag to arrange) ---------- */
  function renderRail() {
    const rail = $('#rail'); rail.innerHTML = '';
    const L = layout();
    L.forEach(o => {
      const d = el('div', 'blk'); d.dataset.i = o.i; d.style.setProperty('--c', secColor(o.b.section));
      d.style.width = Math.max(96, o.b.section.length * 11 + 28, Math.min(220, o.sec.bars * 7)) + 'px';
      d.append(el('div', 'prog'), el('div', 'nm', o.b.section));
      d.append(el('div', 'sub', `${Math.round(o.sec.bars)} bars${o.b.muted.length ? ' · no ' + o.b.muted.join(', ') : ''}`));
      d.append(el('span', 'num', String(o.i + 1)));
      const tools = el('div', 'tools');
      const dup = el('button', null, '⧉'); dup.title = 'Play this section twice'; dup.setAttribute('aria-label', 'Duplicate');
      dup.onpointerdown = e => e.stopPropagation();
      dup.onclick = e => { e.stopPropagation(); const nb = blocks.slice(); nb.splice(o.i + 1, 0, { uid: uidSeq++, section: o.b.section, muted: [...o.b.muted] }); commit(nb); };
      const x = el('button', null, '✕'); x.title = 'Remove from the arrangement'; x.setAttribute('aria-label', 'Remove');
      x.onpointerdown = e => e.stopPropagation();
      x.onclick = e => { e.stopPropagation(); commit(blocks.filter((_, j) => j !== o.i)); };
      tools.append(dup, x); d.append(tools);
      d.onpointerdown = e => startDrag(e, { kind: 'block', from: o.i, el: d, onClick: () => send({ type: 'Goto', block: o.i }) });
      rail.append(d);
    });
    tick();
  }
  function renderPalette() {
    const box = $('#palette'); box.innerHTML = '';
    if (!V.song) return;
    V.song.sections.filter(s => s.secs > 0.05).forEach(s => {
      const c = el('div', 'chip'); c.style.setProperty('--c', secColor(s.name));
      c.append(el('i'), document.createTextNode(s.name));
      const add = el('button', null, '+'); add.title = `Add ${s.name} at the end`; add.setAttribute('aria-label', `Add ${s.name}`);
      add.onpointerdown = e => e.stopPropagation();
      add.onclick = () => commit([...blocks, { uid: uidSeq++, section: s.name, muted: [] }]);
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
      const nb = blocks.slice(); nb.splice(d.at, 0, { uid: uidSeq++, section: d.section, muted: [] }); commit(nb);
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
    const nb = blocks.map(b => ({ ...b, muted: [...b.muted] }));
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
    if (box.contains(document.activeElement) && document.activeElement.type === 'range') return;
    box.innerHTML = '';
    if (!V.song) return;
    V.song.stems.forEach((st, i) => {
      const r = el('div', 'lane'); r.style.setProperty('--c', stemColor(st.name, i));
      const ln = el('div', 'ln'); ln.append(el('b', null, st.name));
      const f = el('input'); f.type = 'range'; f.min = 0; f.max = 1.5; f.step = 0.01; f.value = st.gain; f.id = 'fader-' + i; f.setAttribute('aria-label', `${st.name} level`);
      f.oninput = () => { st.gain = +f.value; send({ type: 'StemGain', stem: i, gain: st.gain }); };
      ln.append(f);
      const ms = el('div', 'ms');
      const m = el('button', 'm' + (st.muted ? ' on' : ''), 'M'); m.title = 'Mute'; m.setAttribute('aria-pressed', st.muted);
      m.onclick = async () => { st.muted = !st.muted; await send({ type: 'StemMute', stem: i, muted: st.muted }); if (solo.size) applySolo(); renderHeads(); dirty = true; };
      const sb = el('button', 's' + (solo.has(i) ? ' on' : ''), 'S'); sb.title = 'Solo'; sb.setAttribute('aria-pressed', solo.has(i));
      sb.onclick = () => { solo.has(i) ? solo.delete(i) : solo.add(i); applySolo(); renderHeads(); dirty = true; };
      ms.append(m, sb);
      r.append(el('div', 'sw'), ln, ms);
      box.append(r);
    });
  }

  /* ---------- arrangement canvases ---------- */
  const cvTop = $('#cvTop'), cvLanes = $('#cvLanes');
  function size(cv, h) {
    const dpr = window.devicePixelRatio || 1, w = cv.clientWidth;
    if (cv.width !== Math.round(w * dpr) || cv.height !== Math.round(h * dpr)) { cv.width = Math.round(w * dpr); cv.height = Math.round(h * dpr); cv.style.height = h + 'px'; }
    const g = cv.getContext('2d'); g.setTransform(dpr, 0, 0, dpr, 0, 0); return [g, w];
  }
  const X = t => (t - zoom.scroll) * zoom.pps;
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
    g.fillStyle = '#141518'; g.fillRect(0, 0, w, 50);
    const nStems = s ? s.stems.length : 0;
    const [h, w2] = size(cvLanes, Math.max(1, nStems * LANE_H));
    h.fillStyle = '#111214'; h.fillRect(0, 0, w2, nStems * LANE_H);
    if (!s || !L.length) return;
    const p = playing(), now = playTime(L), bs = barSec();
    if (p && zoom.follow) { const x = X(now); if (x > w * 0.72 || x < 0) zoom.scroll = now - (w * 0.2) / zoom.pps; }
    clampScroll(L, w);
    const cur = snap ? Math.min(snap.item, L.length - 1) : -1;
    // ruler
    const step = [1, 2, 4, 8, 16, 32, 64].find(n => n * bs * zoom.pps >= 34) || 64;
    g.font = '10px ui-monospace, Menlo, monospace'; g.textBaseline = 'middle';
    L.forEach(o => {
      const n = Math.round(o.sec.bars);
      for (let k = 0; k < n; k++) {
        const x = X(o.start + k * bs); if (x < -2 || x > w + 2) continue;
        const bar = o.bar + k;
        g.fillStyle = (bar - 1) % step === 0 ? '#555a62' : '#2c2f35';
        g.fillRect(Math.round(x), (bar - 1) % step === 0 ? 4 : 13, 1, (bar - 1) % step === 0 ? 16 : 7);
        if ((bar - 1) % step === 0) { g.fillStyle = '#8d9198'; g.fillText(String(bar), x + 3, 10); }
      }
    });
    // section lane
    L.forEach(o => {
      const x0 = X(o.start), x1 = X(o.start + o.secs); if (x1 < 0 || x0 > w) return;
      g.fillStyle = secColor(o.b.section); g.globalAlpha = o.i === cur ? 1 : 0.78;
      g.fillRect(x0 + 1, RULER_H + 2, Math.max(1, x1 - x0 - 2), 26); g.globalAlpha = 1;
      if (snap && snap.pending === o.i && Math.floor(performance.now() / 250) % 2) { g.strokeStyle = '#ffd23f'; g.lineWidth = 2; g.strokeRect(x0 + 2, RULER_H + 3, x1 - x0 - 4, 24); }
      g.save(); g.beginPath(); g.rect(x0 + 1, RULER_H, Math.max(0, x1 - x0 - 4), 30); g.clip();
      g.fillStyle = '#0b0b0c'; g.font = '800 12px ui-rounded, -apple-system, system-ui, sans-serif';
      g.fillText((snap && snap.vamp && o.i === cur ? '↻ ' : '') + o.b.section.toUpperCase(), x0 + 7, RULER_H + 15); g.restore();
    });
    // lanes
    const per = peaks ? peaks.per_sec : 100;
    s.stems.forEach((st, r) => {
      const y = r * LANE_H, col = stemColor(st.name, r);
      const laneOff = st.muted || (solo.size > 0 && !solo.has(r));
      h.fillStyle = r % 2 ? '#131417' : '#111214'; h.fillRect(0, y, w2, LANE_H);
      h.fillStyle = '#22252a'; h.fillRect(0, y + LANE_H - 1, w2, 1);
      const pk = peaks && peaks.stems[r];
      L.forEach(o => {
        const x0 = X(o.start), x1 = X(o.start + o.secs); if (x1 < 0 || x0 > w2) return;
        const off = laneOff || o.b.muted.includes(st.name);
        const top = y + 3, ch = LANE_H - 7;
        h.globalAlpha = off ? 0.08 : 0.2; h.fillStyle = off ? '#8d9198' : col; h.fillRect(x0 + 1, top, Math.max(1, x1 - x0 - 2), ch);
        h.globalAlpha = off ? 0.3 : 1; h.fillRect(x0 + 1, top, Math.max(1, x1 - x0 - 2), 2);
        if (pk) {
          h.globalAlpha = off ? 0.25 : 0.9; h.fillStyle = off ? '#8d9198' : col;
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
        if (off && x1 - x0 > 50) { h.fillStyle = '#5d6168'; h.font = '10px -apple-system, system-ui, sans-serif'; h.fillText(laneOff ? '' : 'off', x0 + 6, top + 13); }
        if (sel && sel.block === o.i && sel.stem === r) { h.strokeStyle = '#ececec'; h.lineWidth = 1.5; h.strokeRect(x0 + 1.5, top + 0.5, x1 - x0 - 3, ch); }
      });
    });
    // block boundaries + playhead + queued jump
    h.fillStyle = 'rgba(255,255,255,.06)'; L.forEach(o => { const x = X(o.start); if (x >= 0 && x <= w2) h.fillRect(Math.round(x), 0, 1, nStems * LANE_H); });
    if (snap && snap.pending != null && p) {
      const jx = X(now + snap.frames_to_jump / s.sample_rate);
      [g, h].forEach((c, k) => { c.strokeStyle = '#ffd23f'; c.setLineDash([4, 3]); c.lineWidth = 1.5; c.beginPath(); c.moveTo(jx, k ? 0 : RULER_H); c.lineTo(jx, k ? nStems * LANE_H : 50); c.stroke(); c.setLineDash([]); });
    }
    if (scrub != null) {
      const sx = X(scrub);
      g.fillStyle = h.fillStyle = 'rgba(255,255,255,.55)';
      g.fillRect(sx - 1, 0, 2, 50); h.fillRect(sx - 1, 0, 2, nStems * LANE_H);
    }
    const px = X(now);
    g.fillStyle = h.fillStyle = p ? '#ff6a1a' : '#ececec';
    g.fillRect(px - 1, 0, 2, 50); h.fillRect(px - 1, 0, 2, nStems * LANE_H);
    g.beginPath(); g.moveTo(px - 6, 0); g.lineTo(px + 6, 0); g.lineTo(px, 8); g.fill();
  }
  function loop() { if (V && (dirty || playing() || scrub != null || (snap && snap.pending != null))) { draw(); dirty = false; } requestAnimationFrame(loop); }
  requestAnimationFrame(loop);
  window.addEventListener('resize', () => { dirty = true; });

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
    sel = { block: o.i, stem: Math.floor(y / LANE_H) }; dirty = true;
  });
  cvLanes.addEventListener('dblclick', e => {
    const { o, y } = hit(e, cvLanes); const st = V.song && V.song.stems[Math.floor(y / LANE_H)];
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
  const zoomBy = f => { const w = cvTop.clientWidth, c = zoom.scroll + w / 2 / zoom.pps; zoom.pps = Math.max(1, Math.min(400, zoom.pps * f)); zoom.scroll = c - w / 2 / zoom.pps; clampScroll(layout(), w); dirty = true; };
  $('#zoomIn').onclick = () => zoomBy(1.5); $('#zoomOut').onclick = () => zoomBy(1 / 1.5); $('#zoomFit').onclick = fit;
  $('#followBtn').onclick = () => { zoom.follow = !zoom.follow; renderStatus(); };

  /* ---------- status bar ---------- */
  function renderStatus() {
    $('#stOut').textContent = V.output ? `${V.output.name} · ${V.output.channels} outs · ${(V.output.sample_rate / 1000).toFixed(1)} kHz` : 'No audio output';
    $('#stMidi').textContent = V.midi ? `MIDI: ${V.midi}` : 'No pedals connected';
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
    const sig = s.folder + JSON.stringify(s.markers);
    if (draftFor !== sig) { draftFor = sig; draftMarkers = s.markers.map(m => ({ ...m })); }
    $('#analyzeBtn').disabled = analyzing || !(s.has_guide || s.has_click);
    $('#analyzeBtn').textContent = analyzing ? 'Reading…' : 'Read guide';
    $('#analyzeHint').textContent = s.markers_from === 'ableton' ? 'Sections came from the Ableton set in this folder. Read guide replaces them with what the guide says.' : !(s.has_guide || s.has_click) ? 'No Click or Guide stem in this song. Add sections by hand below.'
      : `Reads the ${[s.has_click && 'Click (tempo)', s.has_guide && 'Guide (section cues)'].filter(Boolean).join(' and ')}. This replaces the markers below and resets the arrangement to the song as recorded.`;
    const hl = $('#heard'); hl.innerHTML = '';
    heard.forEach(([t, w]) => { const li = el('li'); li.append(fmt(t) + '  ', el('b', null, w || '…')); hl.append(li); });
    if (!document.activeElement || !document.activeElement.closest('#songDrawer .form')) {
      $('#f-name').value = s.name; $('#f-bpm').value = s.bpm; $('#f-bpb').value = s.beats_per_bar; $('#f-key').value = s.key; $('#f-off').value = Math.round(s.offset_ms);
    }
    renderMarkers();
    renderKeyTempo();
    const rb = $('#routes'); rb.innerHTML = '';
    s.stems.forEach((st, i) => {
      const r = el('div', 'rt'); const sel = el('select'); sel.setAttribute('aria-label', `${st.name} output`); routeOptions(sel, st.route);
      sel.onchange = () => { st.route = keyRoute(sel.value); send({ type: 'StemRoute', stem: i, route: st.route }); };
      r.append(el('span', null, st.name), sel); rb.append(r);
    });
  }
  let kt = null; // { transpose, bpm } being edited
  function renderKeyTempo() {
    const s = V.song; if (!s) return;
    if (!kt || kt.folder !== s.folder) kt = { folder: s.folder, transpose: s.transpose, bpm: playBpm() };
    $('#keyVal').textContent = keyName(s.key, kt.transpose) + (kt.transpose ? ` (${kt.transpose > 0 ? '+' : ''}${kt.transpose})` : '');
    $('#keyOrig').textContent = s.key ? `Recorded in ${s.key}` : 'Add the recorded key under Song details to see key names';
    if (document.activeElement !== $('#bpmVal')) $('#bpmVal').value = +kt.bpm.toFixed(2);
    $('#bpmOrig').textContent = `Recorded at ${+s.bpm.toFixed(2)}`;
    const changed = kt.transpose !== s.transpose || Math.abs(kt.bpm - playBpm()) > 0.004;
    $('#ktApply').disabled = !changed || applyingKt;
    $('#ktApply').textContent = applyingKt ? 'Applying…' : 'Apply';
  }
  let applyingKt = false;
  $('#keyDown').onclick = () => { kt.transpose = Math.max(-12, kt.transpose - 1); renderKeyTempo(); };
  $('#keyUp').onclick = () => { kt.transpose = Math.min(12, kt.transpose + 1); renderKeyTempo(); };
  $('#bpmDown').onclick = () => { kt.bpm = Math.max(30, Math.round((kt.bpm - 0.5) * 2) / 2); renderKeyTempo(); };
  $('#bpmUp').onclick = () => { kt.bpm = Math.min(300, Math.round((kt.bpm + 0.5) * 2) / 2); renderKeyTempo(); };
  $('#bpmVal').oninput = e => { const v = +e.target.value; if (v >= 30 && v <= 300) { kt.bpm = v; renderKeyTempo(); } };
  $('#ktReset').onclick = () => { kt.transpose = 0; kt.bpm = V.song.bpm; renderKeyTempo(); };
  $('#ktApply').onclick = async () => {
    if (playing()) { toast('Stop playback before changing the key or tempo.'); return; }
    applyingKt = true; renderKeyTempo(); showTask('Changing key and tempo', 0.3);
    try {
      const v = await invoke('set_key_tempo', { transpose: kt.transpose, bpm: kt.bpm });
      songKey = null; // stems changed: reload waveforms
      await refresh(v); kt = null;
      const s = V.song; toast(`Now in ${keyName(s.key, s.transpose)} at ${+playBpm().toFixed(2)} BPM.`);
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
  $('#songSave').onclick = () => {
    const names = draftMarkers.map(m => m.name.trim().toLowerCase()).filter(Boolean);
    if (new Set(names).size !== names.length) { toast('Two sections have the same name. Give each its own (e.g. Chorus 1, Chorus 2).'); return; }
    invoke('update_song', { meta: { name: $('#f-name').value, bpm: +$('#f-bpm').value || V.song.bpm, beats_per_bar: Math.round(+$('#f-bpb').value || 4), offset_ms: +$('#f-off').value || 0, key: $('#f-key').value, markers: draftMarkers.map(m => ({ name: m.name.trim(), bar: m.bar })) } })
      .then(v => { refresh(v); toast('Song saved.'); }).catch(err);
  };

  /* ---------- setup drawer ---------- */
  const LEARN = [
    { label: 'Pedal 1', tap: 'PlayStop', hold: 'Panic', desc: 'Tap: play / stop · Hold: panic stop' },
    { label: 'Pedal 2', tap: 'Next', hold: 'SkipAhead', desc: 'Tap: next block · Hold: skip ahead' },
    { label: 'Pedal 3', tap: 'Repeat', hold: 'ToggleVamp', desc: 'Tap: repeat block · Hold: vamp' },
    { label: 'Pedal 4', tap: 'NextSong', hold: 'PadOnly', desc: 'Tap: next song · Hold: pad only' },
    { label: 'Play / stop button', tap: 'PlayStop' }, { label: 'Next button', tap: 'Next' }, { label: 'Repeat button', tap: 'Repeat' },
    { label: 'Vamp button', tap: 'ToggleVamp' }, { label: 'Next song button', tap: 'NextSong' }, { label: 'Click fader', tap: 'ClickLevel' },
  ];
  function actionLabel(a) {
    if (a && a.StemLevel !== undefined) return `${V.song && V.song.stems[a.StemLevel] ? V.song.stems[a.StemLevel].name : 'Stem ' + (a.StemLevel + 1)} fader`;
    return ({ PlayStop: 'Play / stop', Panic: 'Panic stop', Next: 'Next block', SkipAhead: 'Skip ahead', Repeat: 'Repeat block', ToggleVamp: 'Vamp', NextSong: 'Next song', PadOnly: 'Pad only', ClickLevel: 'Click fader' })[a] || String(a);
  }
  const controlLabel = c => (c.Note ? `Note ${c.Note.key} · ch ${c.Note.ch + 1}` : c.Cc ? `CC ${c.Cc.num} · ch ${c.Cc.ch + 1}` : '?');
  function renderSetup() {
    const os = $('#outSel'); os.innerHTML = '';
    V.outputs.forEach(o => os.append(new Option(`${o.name} (${o.channels} outputs)${o.is_default ? ' · default' : ''}`, o.name)));
    if (V.output) os.value = V.output.name;
    routeOptions($('#clickRoute'), V.click_route);
    if (document.activeElement !== $('#clickGain')) $('#clickGain').value = V.click_gain;
    $('#outHint').textContent = (V.output && V.output.channels >= 4) ? 'Stems start on Out 1-2 (front of house); click and guide on Out 3-4 (in-ears). Change any stem in Song → Outputs.' : 'Only 2 outputs. Pick your audio interface to send click and guide to the in-ears separately.';
    const ms = $('#midiSel'); ms.innerHTML = ''; ms.append(new Option('None', '')); V.midi_inputs.forEach(m => ms.append(new Option(m, m))); ms.value = V.midi || '';
    const rows = [...LEARN, ...(V.song ? V.song.stems.map((st, i) => ({ label: `${st.name} fader`, tap: { StemLevel: i } })) : [])];
    const box = $('#learn'); box.innerHTML = '';
    rows.forEach((r, i) => {
      const d = el('div', 'lrow'); const t = el('div'); t.append(el('b', null, r.label)); if (r.desc) t.append(el('small', null, r.desc));
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
  $('#outSel').onchange = e => { if (playing()) { toast('Stop playback before changing the audio output.'); renderSetup(); return; } invoke('choose_output', { name: e.target.value }).then(v => { refresh(v); toast('Audio output changed.'); }).catch(err); };
  $('#clickRoute').onchange = e => { V.click_route = keyRoute(e.target.value); send({ type: 'ClickRoute', route: V.click_route }); };
  $('#clickGain').oninput = e => { V.click_gain = +e.target.value; send({ type: 'ClickGain', gain: V.click_gain }); };
  $('#midiSel').onchange = e => invoke('choose_midi', { name: e.target.value || null }).then(refresh).catch(err);

  /* ---------- drawers ---------- */
  const drawers = { setBtn: '#setDrawer', songBtn: '#songDrawer', setupBtn: '#setupDrawer' };
  function openDrawer(id) { Object.values(drawers).forEach(d => { $(d).hidden = d !== id || !$(d).hidden; }); if (id === '#setupDrawer') invoke('app_state').then(refresh); }
  Object.entries(drawers).forEach(([b, d]) => { $('#' + b).onclick = () => openDrawer(d); });
  document.querySelectorAll('[data-close]').forEach(b => { b.onclick = () => { b.closest('.drawer').hidden = true; }; });
  const anyDrawer = () => Object.values(drawers).some(d => !$(d).hidden);

  /* ---------- transport ---------- */
  $('#playBtn').onclick = () => send({ type: 'PlayStop' });
  document.querySelectorAll('[data-cmd]').forEach(b => { b.onclick = () => send({ type: b.dataset.cmd }); });
  $('#quant').onclick = e => { const b = e.target.closest('[data-q]'); if (!b) return; V.quantize = b.dataset.q; send({ type: 'Quantize', q: V.quantize }); renderBar(); };
  $('#clickBtn').onclick = () => { if (!V.song) return; V.song.click = !V.song.click; send({ type: 'Click', on: V.song.click }); renderBar(); };
  $('#countBtn').onclick = () => { V.count_in = !V.count_in; send({ type: 'CountIn', on: V.count_in }); renderBar(); };
  $('#prevSong').onclick = () => V.current != null && selectSong(V.current - 1);
  $('#nextSong').onclick = () => V.current != null && selectSong(V.current + 1);
  $('#addSongs').onclick = () => invoke('add_songs').then(refresh).catch(err);
  document.addEventListener('keydown', e => {
    if (e.target.closest('input,select,textarea')) return;
    const k = e.key;
    if ((e.metaKey || e.ctrlKey) && (k === 'z' || k === 'Z')) { e.preventDefault(); doUndo(); return; }
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (k === ' ') { e.preventDefault(); send({ type: 'PlayStop' }); }
    else if (k === 'n' || k === 'N' || k === 'ArrowRight') send({ type: 'Next' });
    else if (k === 'r' || k === 'R') send({ type: 'Repeat' });
    else if (k === 'v' || k === 'V') send({ type: 'Vamp' });
    else if (k === 'f' || k === 'F') { zoom.follow = !zoom.follow; renderStatus(); }
    else if (k === '=' || k === '+') zoomBy(1.5);
    else if (k === '-') zoomBy(1 / 1.5);
    else if (k === '0' && sel && V.song && V.song.stems[sel.stem]) toggleClip(sel.block, V.song.stems[sel.stem].name);
    else if (k === 'Escape') { if (anyDrawer()) Object.values(drawers).forEach(d => { $(d).hidden = true; }); else send({ type: 'CancelJump' }); }
    else if (/^[1-9]$/.test(k) && blocks[+k - 1]) send({ type: 'Goto', block: +k - 1 });
  });

  /* ---------- events from Rust ---------- */
  T.event.listen('stem-level', e => { const [i, g] = e.payload; if (!V || !V.song || !V.song.stems[i]) return; V.song.stems[i].gain = g; const f = $('#fader-' + i); if (f) f.value = g; });
  T.event.listen('click-level', e => { if (V) V.click_gain = e.payload; });
  T.event.listen('midi-learned', () => { learnWait = null; refresh().then(() => toast('Control assigned.')); });
  T.event.listen('audio-error', e => toast('Audio problem: ' + e.payload));
  T.event.listen('pedal', e => {
    if (e.payload === 'next-song') { if (playing()) { toast('Next song loads when this one stops.'); return; } if (V && V.current != null && V.current + 1 < V.setlist.length) selectSong(V.current + 1); }
    else if (e.payload === 'pad-only') toast('Pads arrive in a later update.');
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
  T.event.listen('update-progress', e => showTask('Downloading update', e.payload));
  invoke('app_version').then(v => { $('#verText').textContent = `Playback Rig ${v}. Updates install only when playback is stopped.`; }).catch(() => {});
  setTimeout(() => checkUpdate(false), 4000);
  setInterval(() => { if (!update) checkUpdate(false); }, 6 * 3600 * 1000);

  setInterval(async () => {
    try {
      const s = await invoke('status'); snap = s.snapshot;
      if (update && !updating) $('#updBtn').disabled = playing();
      if (learnWait != null && !s.learning) { learnWait = null; await refresh(); } else tick();
    } catch (e) { /* closing */ }
  }, 40);

  (async () => {
    await refresh();
    if (V.current != null && !V.song && V.setlist[V.current] && V.setlist[V.current].found) selectSong(V.current);
  })();
})();

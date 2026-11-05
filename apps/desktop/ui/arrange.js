// Plain-words arrangement changes, worked out on the computer itself: free, offline,
// instant. Understands the moves a music director actually says:
//   double verse 2 · repeat the last chorus · chorus 2 twice
//   cut / drop / remove / skip / take out the instrumental
//   add a tag at the end · add verse 3 after chorus 2 · put the bridge before chorus 3
//   move the bridge after chorus 2 · swap verse 2 and chorus 1
//   after verse 2 do chorus 2, repeat it, then go straight into the bridge
// Returns { blocks: [{ section, from }], message } (from = index of the block it came
// from, or null for a new one), or { error }.
(function (root) {
  const NUMW = { one: 1, two: 2, three: 3, four: 4, five: 5, six: 6, first: 1, second: 2, third: 3, fourth: 4, '1st': 1, '2nd': 2, '3rd': 3, '4th': 4 };
  // Short names musicians use. Single letters only count when a number follows (v2, c1).
  const SHORT = { v: 'verse', c: 'chorus', ch: 'chorus', b: 'bridge', br: 'bridge', pc: 'pre-chorus', pre: 'pre-chorus', prechorus: 'pre-chorus', inst: 'instrumental',
    instr: 'instrumental', ta: 'turnaround', turnaround: 'turnaround', interlude: 'interlude', intro: 'intro', outro: 'outro', tag: 'tag', ending: 'ending',
    postchorus: 'post-chorus', refrain: 'refrain', vamp: 'vamp', solo: 'solo', countin: 'count-in', i: 'intro', o: 'outro', t: 'tag' };
  // (Everyday words like "end", "top", "count", "it" are left out on purpose: "add a tag at the end".)
  const norm = s => s.toLowerCase().replace(/pre[\s-]?chorus/g, 'pre-chorus').replace(/post[\s-]?chorus/g, 'post-chorus').replace(/count[\s-]?in/g, 'count-in').replace(/\s+/g, ' ').trim();
  const baseOf = n => norm(n).replace(/\s*\d+$/, '');
  const numOf = n => { const m = norm(n).match(/(\d+)$/); return m ? +m[1] : null; };

  // Every section mention in a piece of text, in order: { section, pos, last, nth }.
  function mentions(text, names) {
    const t = ' ' + norm(text).replace(/[^a-z0-9\- ]/g, ' ') + ' ';
    const bases = [...new Set([...names.map(baseOf), ...Object.values(SHORT)])];
    const words = new Map(); // spoken word -> base
    bases.forEach(b => words.set(b, b));
    bases.forEach(b => { if (b.endsWith('s') === false) words.set(b + 's', b); });
    Object.entries(SHORT).forEach(([k, v]) => { if (!words.has(k)) words.set(k, v); });
    const keys = [...words.keys()].sort((a, b) => b.length - a.length).map(k => k.replace(/[-]/g, '\\-'));
    const re = new RegExp(`(?:\\b(all(?: the)?|every|both(?: the)?|any|last|first|second|third|final|other)\\s+)?\\b(${keys.join('|')})(?![a-z])\\s*(?:(\\d+)\\b|\\b(one|two|three|four|five|six)\\b)?`, 'g');
    const out = [];
    let m;
    while ((m = re.exec(t))) {
      const word = m[2], base = words.get(word);
      const num = m[3] ? +m[3] : m[4] ? NUMW[m[4]] : null;
      if (word.length <= 2 && !SHORT_OK(word, num)) continue; // "i want", "a tag" are not sections
      // "all instrumentals", "every tag", "both bridges", or a plural with no number: every one of that kind
      const every = /^(all|every|both|any)/.test(m[1] || '') || (num == null && word !== base && word === base + 's');
      out.push({ base, num, pos: m.index, which: every ? null : m[1] || null, every });
    }
    return out;
  }
  function SHORT_OK(word, num) {
    if (['pc', 'ta', 'ch', 'br'].includes(word)) return true;
    return num != null; // v2, c1, b1
  }
  // A mention -> a section name of this song.
  function resolve(m, names) {
    const cands = names.filter(n => baseOf(n) === m.base);
    if (!cands.length) return null;
    if (m.num != null) return cands.find(n => numOf(n) === m.num) || (m.num === 1 ? cands.find(n => numOf(n) == null) : null) || null;
    return cands.find(n => numOf(n) == null) || cands[0];
  }
  // Index of the block a mention points at: "last chorus" -> the last one; otherwise the
  // first at or after `near` (so "then the bridge" means the next bridge), else the first.
  function locate(arr, sec, m, near) {
    const idx = arr.map((b, i) => (b.section === sec ? i : -1)).filter(i => i >= 0);
    if (!idx.length) return -1;
    if (m && (m.which === 'last' || m.which === 'final')) return idx[idx.length - 1];
    if (m && m.which === 'second') return idx[1] ?? -1;
    if (m && m.which === 'third') return idx[2] ?? -1;
    const after = idx.find(i => i >= near);
    return after != null ? after : idx[0];
  }

  const RX = {
    remove: /\b(cut|drop|remove|skip|delete|lose|take out|leave out|get rid of|no more|without)\b|\b(take|leave|cut) .+ out\b|^no\b/,
    repeat: /\b(double|repeat|twice|again|2x|x2|two times)\b/,
    add: /\b(add|insert|put in|throw in|tack on|stick in)\b/,
    move: /\b(move|put)\b/,
    swap: /\b(swap|switch|trade|flip)\b/,
    after: /\bafter\b/, before: /\bbefore\b/,
    end: /\b(at the end|to the end|end with|finish with|last thing|after everything)\b/,
    start: /\b(at the (start|top|beginning)|to the (start|top|beginning)|start with|open with)\b/,
  };

  function interpret(text, names, current) {
    let arr = current.map((b, i) => ({ section: b.section, from: i }));
    const did = [];
    // A new clause at punctuation, "then", "so", "but", or "and" before a new action
    // ("double verse 1 and drop the bridge" is two changes).
    const VERB = '(?:double|repeat|cut|drop|remove|skip|delete|lose|add|insert|put|move|swap|switch|take|leave|go|do|play|after|before|no|get rid)';
    const clauses = norm(text).split(new RegExp(`\\s*(?:[,;.]|\\bthen\\b|\\band then\\b|\\bso\\b|\\bbut\\b|\\band(?= ${VERB}\\b)|\\balso\\b)\\s*`)).map(s => s.trim()).filter(Boolean);
    if (!clauses.length) return { error: 'Type what to change.' };
    let cursor = 0, understood = 0;
    // "after verse 2 ..." sets a place; the sections named after it replace what came next.
    let seq = null; // { at, items: [section], seen }
    const flushSeq = () => {
      if (!seq || !seq.items.length) { seq = null; return; }
      const lastSec = seq.items[seq.items.length - 1];
      // Replace from just after the anchor up to the next block of the last named section;
      // if that section isn't coming, the named sections go in right after the anchor.
      let endAt = arr.findIndex((b, i) => i > seq.at && b.section === lastSec);
      const repl = seq.items.map(s => ({ section: s, from: null }));
      // reuse the original blocks' edits where they exist
      repl.forEach(r => { const k = arr.findIndex((b, i) => i > seq.at && b.section === r.section && !b.used); if (k >= 0) { r.from = arr[k].from; arr[k].used = true; } });
      arr.forEach(b => delete b.used);
      if (endAt < 0) arr.splice(seq.at + 1, 0, ...repl);
      else arr.splice(seq.at + 1, endAt - seq.at, ...repl);
      did.push(`after ${arr[seq.at].section}: ${seq.items.join(', ')}`);
      cursor = seq.at + seq.items.length;
      seq = null;
    };
    for (const c of clauses) {
      // An unnumbered name ("the chorus", "the last chorus") means the one in the arrangement
      // the words point to: last / first, else the next one from here.
      const pick = m => {
        const hits = arr.map((b, i) => [b, i]).filter(([b]) => (m.num != null ? b.section === resolve(m, names) : baseOf(b.section) === m.base));
        const sec = m.num != null ? resolve(m, names) : null;
        if (!hits.length) return { sec: sec || resolve(m, names), idx: -1 };
        let h;
        if (m.which === 'last' || m.which === 'final') h = hits[hits.length - 1];
        else if (m.which === 'first') h = hits[0];
        else if (m.which === 'second') h = hits[1];
        else if (m.which === 'third') h = hits[2];
        else h = hits.find(([, i]) => i >= cursor) || hits[0];
        if (!h) return { sec: null, idx: -1 };
        return { sec: h[0].section, idx: h[1] };
      };
      const ms = mentions(c, names).map(m => ({ ...m, ...pick(m) }));
      const unknown = ms.filter(m => !m.sec);
      if (unknown.length) return { error: `This song has no ${unknown[0].base}${unknown[0].num ? ' ' + unknown[0].num : ''}. Its sections: ${names.join(', ')}.` };
      const secs = ms.map(m => m.sec);
      // Removing
      if (RX.remove.test(c) && secs.length) {
        if (seq) flushSeq();
        // "the second chorus" removes that one block; "the instrumental" removes it everywhere.
        const one = ms.filter(m => m.which && m.idx >= 0).map(m => m.idx).sort((a, b) => b - a);
        one.forEach(i => arr.splice(i, 1));
        ms.filter(m => !m.which && !m.every).forEach(m => { arr = arr.filter(b => b.section !== m.sec); });
        ms.filter(m => m.every).forEach(m => { arr = arr.filter(b => baseOf(b.section) !== m.base); });
        did.push(`removed ${ms.map(m => (m.every ? 'every ' + m.base : m.sec)).join(', ')}`); understood++; continue;
      }
      // "after X ..." starts a run of sections
      if (RX.after.test(c) && !RX.add.test(c) && !RX.move.test(c) && secs.length && c.indexOf('after') < (ms[0].pos + 2)) {
        if (seq) flushSeq();
        const i = locate(arr, secs[0], ms[0], cursor);
        if (i < 0) return { error: `There's no ${secs[0]} in the arrangement to go after.` };
        seq = { at: i, items: [] };
        secs.slice(1).forEach(s => seq.items.push(s));
        if (RX.repeat.test(c) && seq.items.length) seq.items.push(seq.items[seq.items.length - 1]);
        understood++; continue;
      }
      if (seq) {
        // inside an "after X" run: each clause names the next sections; "repeat it" repeats
        // "chorus 2 twice" = two; "repeat chorus 2" / "again" = one more of what was just named.
        const twice = /\b(twice|double|2x|x2|two times)\b/.test(c), again = /\b(repeat|again)\b/.test(c);
        if (secs.length) {
          secs.forEach(s => {
            const already = seq.items[seq.items.length - 1] === s;
            if (twice) seq.items.push(s, s);
            else if (again && already) seq.items.push(s);
            else seq.items.push(s);
          });
        } else if ((twice || again) && seq.items.length) seq.items.push(seq.items[seq.items.length - 1]);
        if (secs.length || RX.repeat.test(c)) { understood++; continue; }
        if (/^(and |we |just |go |straight |right )*$/.test(c)) continue;
      }
      // Swapping two sections
      if (RX.swap.test(c) && secs.length >= 2) {
        const a = locate(arr, secs[0], ms[0], 0), b = locate(arr, secs[1], ms[1], 0);
        if (a < 0 || b < 0) return { error: `Both need to be in the arrangement to swap them.` };
        [arr[a], arr[b]] = [arr[b], arr[a]];
        did.push(`swapped ${secs[0]} and ${secs[1]}`); understood++; continue;
      }
      // Moving: "move the bridge after chorus 2", "put the tag at the end"
      if (RX.move.test(c) && !RX.add.test(c) && secs.length >= 1 && (RX.after.test(c) || RX.before.test(c) || RX.end.test(c) || RX.start.test(c))) {
        const i = locate(arr, secs[0], ms[0], 0);
        if (i < 0) { /* not there: treat as an add */ }
        else {
          const [blk] = arr.splice(i, 1);
          let at;
          if (RX.end.test(c)) at = arr.length;
          else if (RX.start.test(c)) at = 0;
          else { const j = locate(arr, secs[1], ms[1], 0); if (j < 0) return { error: `There's no ${secs[1]} in the arrangement.` }; at = RX.after.test(c) ? j + 1 : j; }
          arr.splice(at, 0, blk);
          did.push(`moved ${secs[0]}`); cursor = at + 1; understood++; continue;
        }
      }
      // Adding: "add a tag at the end", "add verse 3 after chorus 2"
      if ((RX.add.test(c) || RX.move.test(c)) && secs.length >= 1) {
        const s = secs[0];
        const times = RX.repeat.test(c) ? 2 : 1;
        let at = arr.length;
        if (RX.start.test(c)) at = 0;
        else if (secs.length >= 2 && (RX.after.test(c) || RX.before.test(c))) {
          const j = locate(arr, secs[1], ms[1], 0);
          if (j < 0) return { error: `There's no ${secs[1]} in the arrangement.` };
          at = RX.after.test(c) ? j + 1 : j;
        }
        const src = arr.find(b => b.section === s);
        for (let k = 0; k < times; k++) arr.splice(at, 0, { section: s, from: src ? src.from : null });
        did.push(`added ${s}${times > 1 ? ' twice' : ''}`); cursor = at + times; understood++; continue;
      }
      // Doubling: "double verse 2", "repeat the last chorus", "chorus 2 twice"
      if (RX.repeat.test(c) && secs.length) {
        secs.forEach((s, k) => {
          const i = locate(arr, s, ms[k], cursor);
          if (i >= 0) { arr.splice(i + 1, 0, { section: s, from: arr[i].from }); cursor = i + 2; }
          else { arr.push({ section: s, from: null }, { section: s, from: null }); }
        });
        did.push(`doubled ${secs.join(', ')}`); understood++; continue;
      }
      // "... then the tag": from where the last change left off, go straight to it.
      if (secs.length && understood && cursor > 0 && !RX.add.test(c) && !RX.move.test(c)) {
        seq = { at: Math.min(cursor, arr.length) - 1, items: [...secs] };
        if (RX.repeat.test(c)) seq.items.push(secs[secs.length - 1]);
        understood++; continue;
      }
      // Filler words ("let's", "and", "please") are fine; anything else wasn't understood.
      if (!secs.length && /^(ok|okay|please|lets|let s|let's|and|so|um|uh|yeah|then|just)( |$)/.test(c)) continue;
      if (!secs.length && c.split(' ').length <= 3) continue;
      return { error: `I didn't catch "${c}". Try things like: double verse 2 · cut the instrumental · add a tag at the end · move the bridge after chorus 2 · after verse 2 do chorus 2 twice then the bridge.` };
    }
    if (seq) flushSeq();
    if (!understood) return { error: 'I didn\'t find a change in that. Try: double verse 2 · cut the instrumental · add a tag at the end.' };
    if (!arr.length) return { error: 'That would leave nothing in the arrangement.' };
    const msg = did.map(s => s[0].toUpperCase() + s.slice(1)).join('. ') + '.';
    return { blocks: arr.map(b => ({ section: b.section, from: b.from })), message: msg };
  }

  root.interpretArrangement = interpret;
  if (typeof module !== 'undefined') module.exports = { interpret };
})(typeof window !== 'undefined' ? window : globalThis);

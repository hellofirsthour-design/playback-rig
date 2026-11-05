// Tests for the offline plain-words arrangement reader (run by check.js in CI).
const { interpret } = require('./arrange.js');
const names = ['Count-in', 'Intro', 'Verse 1', 'Chorus 1', 'Verse 2', 'Chorus 2', 'Instrumental 1', 'Bridge', 'Chorus 3', 'Tag', 'Outro'];
const cur = names.map(section => ({ section }));
const order = r => (r.error ? 'ERROR: ' + r.error : r.blocks.map(b => b.section).join(', '));
const L = 'Count-in, Intro, Verse 1, Chorus 1, ';
const cases = [
  ['double verse 2', L + 'Verse 2, Verse 2, Chorus 2, Instrumental 1, Bridge, Chorus 3, Tag, Outro'],
  ['Double verse two', L + 'Verse 2, Verse 2, Chorus 2, Instrumental 1, Bridge, Chorus 3, Tag, Outro'],
  ['repeat v2', L + 'Verse 2, Verse 2, Chorus 2, Instrumental 1, Bridge, Chorus 3, Tag, Outro'],
  ['chorus 2 twice', L + 'Verse 2, Chorus 2, Chorus 2, Instrumental 1, Bridge, Chorus 3, Tag, Outro'],
  ["let's double verse 2 and double the instrumental", L + 'Verse 2, Verse 2, Chorus 2, Instrumental 1, Instrumental 1, Bridge, Chorus 3, Tag, Outro'],
  ['cut the instrumental', L + 'Verse 2, Chorus 2, Bridge, Chorus 3, Tag, Outro'],
  ['drop the tag and the outro', L + 'Verse 2, Chorus 2, Instrumental 1, Bridge, Chorus 3'],
  ['take out the instrumental', L + 'Verse 2, Chorus 2, Bridge, Chorus 3, Tag, Outro'],
  ['add a tag at the end', L + 'Verse 2, Chorus 2, Instrumental 1, Bridge, Chorus 3, Tag, Outro, Tag'],
  ['add chorus 3 after the bridge again', L + 'Verse 2, Chorus 2, Instrumental 1, Bridge, Chorus 3, Chorus 3, Chorus 3, Tag, Outro'],
  ['move the bridge after chorus 3', L + 'Verse 2, Chorus 2, Instrumental 1, Chorus 3, Bridge, Tag, Outro'],
  ['swap verse 2 and chorus 2', L + 'Chorus 2, Verse 2, Instrumental 1, Bridge, Chorus 3, Tag, Outro'],
  ['repeat the last chorus', L + 'Verse 2, Chorus 2, Instrumental 1, Bridge, Chorus 3, Chorus 3, Tag, Outro'],
  // the user's own example
  ['after verse 2, let the arrangement be just chorus 2, repeat chorus 2, and then we go right into the bridge, so remove that instrumental',
    L + 'Verse 2, Chorus 2, Chorus 2, Bridge, Chorus 3, Tag, Outro'],
  ['after verse 2 do chorus 2 twice then go straight to the bridge', L + 'Verse 2, Chorus 2, Chorus 2, Bridge, Chorus 3, Tag, Outro'],
  ['after the bridge go to the tag', L + 'Verse 2, Chorus 2, Instrumental 1, Bridge, Tag, Outro'],
  ['double verse 1 and drop the bridge', 'Count-in, Intro, Verse 1, Verse 1, Chorus 1, Verse 2, Chorus 2, Instrumental 1, Chorus 3, Tag, Outro'],
  ['cut the instrumental and add a tag at the end', L + 'Verse 2, Chorus 2, Bridge, Chorus 3, Tag, Outro, Tag'],
  ['skip the bridge', L + 'Verse 2, Chorus 2, Instrumental 1, Chorus 3, Tag, Outro'],
  ['c2 x2', L + 'Verse 2, Chorus 2, Chorus 2, Instrumental 1, Bridge, Chorus 3, Tag, Outro'],
  ['take the intro out', 'Count-in, Verse 1, Chorus 1, Verse 2, Chorus 2, Instrumental 1, Bridge, Chorus 3, Tag, Outro'],
  ['bridge 2x then tag', L + 'Verse 2, Chorus 2, Instrumental 1, Bridge, Bridge, Tag, Outro'],
  ['remove the second chorus', L + 'Verse 2, Instrumental 1, Bridge, Chorus 3, Tag, Outro'],
  ['no instrumental', L + 'Verse 2, Chorus 2, Bridge, Chorus 3, Tag, Outro'],
  ['put the tag after the outro', L + 'Verse 2, Chorus 2, Instrumental 1, Bridge, Chorus 3, Outro, Tag'],
  ['after verse two chorus two two times then bridge', L + 'Verse 2, Chorus 2, Chorus 2, Bridge, Chorus 3, Tag, Outro'],
];
let bad = 0;
for (const [text, want] of cases) {
  const got = order(interpret(text, names, cur));
  if (got !== want) { bad++; console.log(`FAIL  "${text}"\n  got:  ${got}\n  want: ${want}`); }
}
// things it must refuse rather than guess
for (const text of ['double verse 7', 'make it sound better', 'add a key change', 'double the pre chorus']) {
  const r = interpret(text, names, cur);
  if (!r.error) { bad++; console.log(`FAIL  "${text}" should not be understood, got ${order(r)}`); }
}
// edits carry over: a doubled block comes from the original
const d = interpret('double verse 2', names, cur);
if (d.blocks[4].from !== 4 || d.blocks[5].from !== 4) { bad++; console.log('FAIL  doubled block should come from the original', d.blocks.slice(3, 7)); }
if (bad) { console.log(`${bad} arrangement case(s) failed`); process.exit(1); }
console.log(`arrange.js OK (${cases.length + 5} cases)`);

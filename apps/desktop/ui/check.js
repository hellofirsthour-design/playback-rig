// Quick checks for the UI script, run in CI: it must parse, and no function name may be
// declared twice (a second declaration silently replaces the first — that blanked the
// arrangement view in 0.4.0/0.4.1).
const fs = require('fs');
const path = require('path');
const src = fs.readFileSync(path.join(__dirname, 'app.js'), 'utf8');
new Function(src); // throws on a syntax error
const seen = new Map();
let bad = 0;
src.split('\n').forEach((line, i) => {
  const m = /^\s*(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\(/.exec(line);
  if (!m) return;
  if (seen.has(m[1])) { console.error(`app.js:${i + 1}: function ${m[1]} is already declared at line ${seen.get(m[1])}`); bad++; }
  else seen.set(m[1], i + 1);
});
if (bad) process.exit(1);
console.log(`app.js OK (${seen.size} functions)`);
require('./arrange.test.js'); // the offline plain-words arrangement reader

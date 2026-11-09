"""Builds Playback Rig's drum kit from DRSKit (DrumGizmo team / DRSDrums, CC BY 4.0).

Each recorded hit has 13 microphones. They're mixed here into one stereo sample per hit,
the way an engineer would mix a live kit from the drummer's side: close mics panned where
the drums sit, overheads and room around them, the tom mics gated (turned down on hits
that aren't toms, so they don't ring under every snare), polarity checked against the main
mic. Hits are sorted by how hard they were played and their loudness is measured, so the
app can pick the right hit for any velocity and match its level exactly.

Usage: python3 -I make-drum-kit.py <DRSKit folder: .../DrumGizmo/DRSKit> <out folder>
Writes <out>/kit.json and <out>/<piece>/<n>.flac (stereo, 44.1 kHz, 16-bit at full scale plus
a gain per hit).
"""
import glob
import json
import os
import sys

import numpy as np
import soundfile as sf

SRC, OUT = sys.argv[1], sys.argv[2]
SAMPLES = os.path.join(SRC, 'Samples')

# Our pieces: (name, DRSKit articulation, the piece's own close mic(s), choke group)
PIECES = [
    ('kick', 'Kdrum_without_contact', ['Kdrum_front', 'Kdrum_back'], None),
    ('kick_tight', 'Kdrum_with_contact', ['Kdrum_front', 'Kdrum_back'], None),
    ('snare', 'Snare', ['Snare_top', 'Snare_bottom'], None),
    ('cross_stick', 'Snare_rim', ['Snare_top'], None),
    ('tom_high', 'Tom1', ['Tom1'], None),
    ('tom_mid', 'Tom2', ['Tom2'], None),
    ('tom_floor', 'Tom3', ['Tom3'], None),
    ('hat_closed', 'Hihat_closed', ['Hihat'], 'hat'),
    ('hat_pedal', 'Hihat_foot', ['Hihat'], 'hat'),
    ('hat_half', 'Hihat_semi_open', ['Hihat'], 'hat'),
    ('hat_open', 'Hihat_open', ['Hihat'], 'hat'),
    ('ride', 'Ride_tip', ['Ride'], None),
    ('ride_crash', 'Ride_shank', ['Ride'], None),
    ('ride_bell', 'Ride_shank_bell', ['Ride'], None),
    ('crash_left', 'Crash_left_shank', [], None),
    ('crash_right', 'Crash_right_shank', [], None),
]

# Mic: (gain, pan -1 left .. 1 right), drummer's perspective.
MICS = {
    'Kdrum_front': (0.9, 0.0), 'Kdrum_back': (0.55, 0.0),
    'Snare_top': (0.8, 0.0), 'Snare_bottom': (0.3, 0.0),
    'Hihat': (0.3, -0.45), 'Ride': (0.3, 0.5),
    'Tom1': (0.7, -0.2), 'Tom2': (0.7, 0.3), 'Tom3': (0.7, 0.5),
    'OHL': (0.6, -0.85), 'OHR': (0.6, 0.85),
    'AmbL': (0.6, -1.0), 'AmbR': (0.6, 1.0),
}
CLOSE = {'Kdrum_front', 'Kdrum_back', 'Snare_top', 'Snare_bottom', 'Tom1', 'Tom2', 'Tom3'}
GATED = 0.15  # a close mic on a hit that isn't its drum


def hits(art):
    names = glob.glob(os.path.join(SAMPLES, art, f'*-{art}-OHL.flac'))
    return sorted(int(os.path.basename(n).split('-')[0]) for n in names)


def read(art, n, mic):
    x, sr = sf.read(os.path.join(SAMPLES, art, f'{n}-{art}-{mic}.flac'), dtype='float32')
    return x, sr


def polarity(a, b):
    """+1 or -1: whether mic b lines up with mic a over the attack."""
    n = min(len(a), len(b), 2205)
    c = float(np.dot(a[:n], b[:n]))
    return -1.0 if c < 0 else 1.0


os.makedirs(OUT, exist_ok=True)
kit = {'source': 'DRSKit by the DrumGizmo team and Jes Eiler of DRSDrums, CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/). Mixed to stereo by Playback Rig.',
       'sample_rate': 44100, 'pieces': {}}
for name, art, own, choke in PIECES:
    ns = hits(art)
    out = []
    flip = {}
    for n in ns:
        mics = {m: read(art, n, m)[0] for m in MICS}
        sr = read(art, n, 'OHL')[1]
        ref = mics[own[0]] if own else mics['OHL']
        L = np.zeros(max(len(v) for v in mics.values()), dtype=np.float64)
        R = L.copy()
        for m, (g, pan) in MICS.items():
            x = mics[m].astype(np.float64)
            if m in CLOSE and m not in own:
                g *= GATED
            if m in ('Kdrum_back', 'Snare_bottom') and m not in flip:
                flip[m] = polarity(mics['Kdrum_front' if m == 'Kdrum_back' else 'Snare_top'], x) if m in own else 1.0
            g *= flip.get(m, 1.0)
            a = (pan + 1) * np.pi / 4
            L[:len(x)] += x * g * np.cos(a)
            R[:len(x)] += x * g * np.sin(a)
        y = np.stack([L, R], axis=1)
        # trim the silent tail, keep a short fade so nothing clicks
        e = np.abs(y).max(axis=1)
        last = int(np.max(np.nonzero(e > 1e-4)[0])) + 1 if (e > 1e-4).any() else len(e)
        y = y[:last + int(0.02 * sr)]
        f = min(len(y), int(0.02 * sr))
        y[-f:] *= np.linspace(1, 0, f)[:, None]
        # loudness of the attack (first 60 ms), what velocity is matched against
        a = y[: int(0.06 * sr)]
        rms = float(np.sqrt(np.mean(a ** 2)))
        out.append((rms, n, y))
    out.sort(key=lambda t: t[0])
    os.makedirs(os.path.join(OUT, name), exist_ok=True)
    entries = []
    for i, (rms, n, y) in enumerate(out):
        rel = f'{name}/{i + 1}.flac'
        # Each hit is stored at full scale (16-bit keeps its detail that way) with the gain
        # that puts it back where it was recorded.
        peak = float(np.abs(y).max())
        scale = peak / 0.99 if peak > 0 else 1.0
        sf.write(os.path.join(OUT, rel), (y / scale).astype(np.float32), 44100, subtype='PCM_16')
        entries.append({'file': rel, 'rms': round(rms, 7), 'scale': round(scale, 7), 'len': round(len(y) / 44100, 3)})
    kit['pieces'][name] = {'hits': entries, 'choke': choke}
    print(name, len(entries), 'hits', 'flip', flip, flush=True)

json.dump(kit, open(os.path.join(OUT, 'kit.json'), 'w'), indent=1)
print('done')

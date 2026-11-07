//! Rendered stems, kept on disk: a stem that had to be resampled (44.1 kHz files on a
//! 48 kHz interface) or re-pitched / re-timed (a key or tempo change) is saved once at
//! the interface's rate, so the next time the song loads it's read straight back
//! instead of rendered again. 16-bit with a per-stem scale (as good as the 16-bit
//! source files), dithered. The oldest renders are removed past `CAP` bytes.

use rig_engine::StemAudio;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"RIGREND1";
/// Bump when rendering changes, so old renders are never used.
const VERSION: u64 = 1;
/// Room on disk for renders (about 7 songs with a key change).
pub const CAP: u64 = 12 * 1024 * 1024 * 1024;

fn fnv(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= *b as u64;
        *h = h.wrapping_mul(0x100000001b3);
    }
}

/// The render's name: the source file (path, size, modified time) and what was done to it.
/// None if the file can't be read.
pub fn key(path: &Path, sample_rate: u32, length: f64, semitones: f32) -> Option<String> {
    let m = std::fs::metadata(path).ok()?;
    let modified = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    let mut h = 0xcbf29ce484222325u64;
    fnv(&mut h, path.to_string_lossy().as_bytes());
    fnv(&mut h, &m.len().to_le_bytes());
    fnv(&mut h, &modified.to_le_bytes());
    fnv(&mut h, &sample_rate.to_le_bytes());
    fnv(&mut h, &length.to_bits().to_le_bytes());
    fnv(&mut h, &semitones.to_bits().to_le_bytes());
    fnv(&mut h, &VERSION.to_le_bytes());
    Some(format!("{h:016x}.rnd"))
}

pub fn get(dir: &Path, key: &str) -> Option<StemAudio> {
    let p = dir.join(key);
    let mut f = std::fs::File::open(&p).ok()?;
    let mut head = [0u8; 24];
    f.read_exact(&mut head).ok()?;
    if &head[..8] != MAGIC {
        return None;
    }
    let ch = u32::from_le_bytes(head[8..12].try_into().ok()?) as usize;
    let scale = f32::from_le_bytes(head[12..16].try_into().ok()?);
    let n = u64::from_le_bytes(head[16..24].try_into().ok()?) as usize;
    if ch == 0 || ch > 2 {
        return None;
    }
    let mut raw = vec![0u8; n * 2];
    f.read_exact(&mut raw).ok()?;
    let k = scale / 32767.0;
    let data: Vec<f32> = raw.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 * k).collect();
    // Touch it, so the oldest-used renders are the ones removed.
    let _ = std::fs::File::options().append(true).open(&p).and_then(|f| f.set_modified(std::time::SystemTime::now()));
    Some(StemAudio { channels: ch, data: data.into() })
}

pub fn put(dir: &Path, key: &str, a: &StemAudio) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let peak = a.data.iter().fold(0f32, |m, x| m.max(x.abs()));
    let scale = peak.max(1.0);
    let k = 32767.0 / scale;
    // TPDF dither from a small LCG: cheap and good enough at 16 bits.
    let mut seed = 0x2545F4914F6CDD1Du64;
    let mut rnd = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((seed >> 40) as f32) / (1u64 << 24) as f32
    };
    let mut out = Vec::with_capacity(24 + a.data.len() * 2);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(a.channels as u32).to_le_bytes());
    out.extend_from_slice(&scale.to_le_bytes());
    out.extend_from_slice(&(a.data.len() as u64).to_le_bytes());
    for x in a.data.iter() {
        let d = rnd() - rnd();
        let v = (x * k + d).round().clamp(-32767.0, 32767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    // Written under a temporary name and renamed, so a half-written file is never read.
    let tmp = dir.join(format!("{key}.tmp"));
    let ok = std::fs::File::create(&tmp).and_then(|mut f| f.write_all(&out)).is_ok();
    if ok {
        let _ = std::fs::rename(&tmp, dir.join(key));
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Removes the least recently used renders until they fit in `cap` bytes, keeping `keep`.
pub fn trim(dir: &Path, cap: u64, keep: &[String]) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort_by_key(|f| f.0);
    for (_, len, p) in files {
        if total <= cap {
            break;
        }
        if keep.iter().any(|k| p.file_name().and_then(|n| n.to_str()) == Some(k.as_str())) {
            continue;
        }
        if std::fs::remove_file(&p).is_ok() {
            total -= len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_render_comes_back_as_it_went_in_and_old_ones_are_trimmed() {
        let dir = std::env::temp_dir().join(format!("rig-render-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src.wav");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&src, b"x").unwrap();
        let k = key(&src, 48_000, 1.0, 1.0).unwrap();
        assert_ne!(Some(k.clone()), key(&src, 48_000, 1.0, 2.0), "another key change is another render");
        assert_ne!(Some(k.clone()), key(&src, 44_100, 1.0, 1.0), "another rate is another render");
        // Louder than full scale (a pitch shift can overshoot): kept, not clipped.
        let x: Vec<f32> = (0..20_000).map(|i| (i as f32 * 0.01).sin() * 1.3).collect();
        let a = StemAudio::stereo_interleaved(x.clone());
        put(&dir.join("r"), &k, &a);
        let b = get(&dir.join("r"), &k).unwrap();
        assert_eq!(b.channels, 2);
        assert_eq!(b.data.len(), x.len());
        let err = x.iter().zip(b.data.iter()).fold(0f32, |m, (p, q)| m.max((p - q).abs()));
        assert!(err < 1.3 * 2.0 / 32767.0 + 1e-6, "{err}");
        assert!(get(&dir.join("r"), "missing.rnd").is_none());
        trim(&dir.join("r"), 0, &[]);
        assert!(get(&dir.join("r"), &k).is_none(), "trimmed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

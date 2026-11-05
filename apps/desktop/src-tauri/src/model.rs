//! The offline speech model used to read guide tracks. Downloaded once (about
//! 150 MB) into the app's data folder; nothing is sent anywhere after that.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin";
const FILE: &str = "ggml-base.en.bin";
const MIN_BYTES: u64 = 100_000_000;

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("models").join(FILE)
}

pub fn is_ready(data_dir: &Path) -> bool {
    std::fs::metadata(path(data_dir)).map_or(false, |m| m.len() > MIN_BYTES)
}

/// Downloads the model if it isn't there yet. `progress` gets 0.0-1.0.
pub fn ensure(data_dir: &Path, mut progress: impl FnMut(f32)) -> Result<PathBuf, String> {
    let dest = path(data_dir);
    if is_ready(data_dir) {
        return Ok(dest);
    }
    std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
    let resp = ureq::get(URL)
        .call()
        .map_err(|e| format!("Couldn't download the speech model. Check the internet connection and try again. ({e})"))?;
    let total: u64 = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(148_000_000);
    let part = dest.with_extension("part");
    let mut f = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut r = resp.into_reader();
    let mut buf = vec![0u8; 1 << 16];
    let mut got = 0u64;
    let mut last = 0.0f32;
    loop {
        let n = r.read(&mut buf).map_err(|e| format!("Download interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        got += n as u64;
        let p = got as f32 / total as f32;
        if p - last > 0.01 {
            last = p;
            progress(p.min(1.0));
        }
    }
    drop(f);
    if got < MIN_BYTES {
        let _ = std::fs::remove_file(&part);
        return Err("The speech model download was incomplete. Try again.".into());
    }
    std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

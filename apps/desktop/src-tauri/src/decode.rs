//! Decodes stem files (WAV, AIFF, MP3, M4A/AAC, FLAC, ALAC) to f32 at the
//! interface's sample rate. Runs off the audio thread, one thread per stem.

use rig_engine::StemAudio;
use rubato::{FftFixedInOut, Resampler};
use std::fs::File;
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

pub const AUDIO_EXTS: [&str; 8] = ["wav", "aif", "aiff", "mp3", "m4a", "aac", "flac", "caf"];

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn decode(path: &Path, target_sr: u32) -> Result<StemAudio, String> {
    decode_native(path, target_sr).map(|(a, _)| a)
}

/// A file's own sample rate, from its header only.
pub fn native_rate(path: &Path) -> Option<u32> {
    let file = File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe().format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default()).ok()?;
    let sr = probed.format.default_track()?.codec_params.sample_rate;
    sr
}

/// Like `decode`, also giving the file's own sample rate.
pub fn decode_native(path: &Path, target_sr: u32) -> Result<(StemAudio, u32), String> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
    let file = File::open(path).map_err(|e| format!("{name}: can't open ({e})"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("{name}: not an audio file this app can read ({e})"))?;
    let mut format = probed.format;
    let track = format.default_track().ok_or(format!("{name}: no audio track"))?;
    let track_id = track.id;
    let sr = track.codec_params.sample_rate.ok_or(format!("{name}: unknown sample rate"))?;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("{name}: unsupported codec ({e})"))?;

    let mut chans = 0usize;
    let mut data: Vec<f32> = Vec::new();
    let mut sbuf: Option<SampleBuffer<f32>> = None;
    let mut cap = 0usize;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(Error::ResetRequired) => break,
            Err(e) => return Err(format!("{name}: read error ({e})")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                let spec = *buf.spec();
                chans = spec.channels.count();
                if sbuf.is_none() || buf.capacity() > cap {
                    cap = buf.capacity();
                    sbuf = Some(SampleBuffer::new(cap as u64, spec));
                }
                let sb = sbuf.as_mut().unwrap();
                sb.copy_interleaved_ref(buf);
                data.extend_from_slice(sb.samples());
            }
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(format!("{name}: decode error ({e})")),
        }
    }
    if chans == 0 || data.is_empty() {
        return Err(format!("{name}: no audio in file"));
    }

    // Keep mono as mono; anything wider becomes stereo (first two channels).
    let out_ch = if chans == 1 { 1 } else { 2 };
    let frames = data.len() / chans;
    let mut planar: Vec<Vec<f32>> = vec![Vec::with_capacity(frames); out_ch];
    for f in 0..frames {
        let fr = &data[f * chans..(f + 1) * chans];
        for (c, p) in planar.iter_mut().enumerate() {
            p.push(fr[c]);
        }
    }
    drop(data);

    if sr != target_sr {
        planar = resample(&planar, sr, target_sr).map_err(|e| format!("{name}: resampling failed ({e})"))?;
    }
    let n = planar[0].len();
    if out_ch == 1 {
        Ok((StemAudio::mono(planar.pop().unwrap()), sr))
    } else {
        let mut inter = Vec::with_capacity(n * 2);
        for i in 0..n {
            inter.push(planar[0][i]);
            inter.push(planar[1][i]);
        }
        Ok((StemAudio::stereo_interleaved(inter), sr))
    }
}

/// High-quality FFT resampling (e.g. 44.1 kHz stems on a 48 kHz interface).
pub fn resample(planar: &[Vec<f32>], from: u32, to: u32) -> Result<Vec<Vec<f32>>, String> {
    let ch = planar.len();
    let n = planar[0].len();
    let mut r = FftFixedInOut::<f32>::new(from as usize, to as usize, 1024, ch).map_err(|e| e.to_string())?;
    let delay = r.output_delay();
    let want = (n as f64 * to as f64 / from as f64).round() as usize;
    let mut out: Vec<Vec<f32>> = (0..ch).map(|_| Vec::with_capacity(want + delay + 4096)).collect();
    let push = |o: Vec<Vec<f32>>, out: &mut Vec<Vec<f32>>| {
        for (d, s) in out.iter_mut().zip(o) {
            d.extend_from_slice(&s);
        }
    };
    let mut pos = 0;
    while pos + r.input_frames_next() <= n {
        let k = r.input_frames_next();
        let chunk: Vec<&[f32]> = planar.iter().map(|c| &c[pos..pos + k]).collect();
        let o = r.process(&chunk, None).map_err(|e| e.to_string())?;
        push(o, &mut out);
        pos += k;
    }
    // rubato rejects an empty partial chunk, so only send a tail when there is one.
    if pos < n {
        let tail: Vec<&[f32]> = planar.iter().map(|c| &c[pos..]).collect();
        let o = r.process_partial(Some(tail.as_slice()), None).map_err(|e| e.to_string())?;
        push(o, &mut out);
    }
    while out[0].len() < want + delay {
        let o = r.process_partial(None::<&[&[f32]]>, None).map_err(|e| e.to_string())?;
        if o[0].is_empty() {
            break;
        }
        push(o, &mut out);
    }
    for c in out.iter_mut() {
        c.drain(..delay.min(c.len()));
        c.resize(want, 0.0);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_keeps_length_and_pitch() {
        let sr_in = 44_100u32;
        let n = sr_in as usize; // 1 s of 441 Hz
        let sig: Vec<f32> = (0..n).map(|i| (std::f32::consts::TAU * 441.0 * i as f32 / sr_in as f32).sin()).collect();
        let out = resample(&[sig], sr_in, 48_000).unwrap();
        assert_eq!(out[0].len(), 48_000);
        // Count zero crossings in the middle: 441 Hz -> ~441 rising crossings per second.
        let mid = &out[0][4800..43200];
        let rising = mid.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((rising as i64 - 353).abs() <= 2, "got {rising}"); // 0.8 s * 441
    }

    #[test]
    fn resample_handles_lengths_that_end_on_a_chunk_boundary() {
        // 44.1 kHz -> 16 kHz uses 1323-frame chunks; these lengths used to fail.
        for n in [1323usize, 1323 * 7, 1323 * 40, 1, 1322, 1324] {
            let sig: Vec<f32> = (0..n).map(|i| (i as f32 * 0.01).sin()).collect();
            let out = resample(&[sig], 44_100, 16_000).unwrap_or_else(|e| panic!("length {n}: {e}"));
            assert_eq!(out[0].len(), (n as f64 * 16_000.0 / 44_100.0).round() as usize);
        }
    }

    #[test]
    fn decodes_a_wav_file() {
        let dir = std::env::temp_dir().join(format!("rig-decode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Click.wav");
        // 16-bit stereo WAV, 0.1 s at 48 kHz, written by hand.
        let frames = 4800u32;
        let mut b = Vec::new();
        let data_len = frames * 4;
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&48_000u32.to_le_bytes());
        b.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
        b.extend_from_slice(&4u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..frames {
            b.extend_from_slice(&((i % 100) as i16 * 100).to_le_bytes());
            b.extend_from_slice(&(-1000i16).to_le_bytes());
        }
        std::fs::write(&path, b).unwrap();
        let a = decode(&path, 48_000).unwrap();
        assert_eq!(a.channels, 2);
        assert_eq!(a.frames(), 4800);
        let (l, r) = a.frame(5);
        assert!((l - 500.0 / 32768.0).abs() < 1e-4 && (r + 1000.0 / 32768.0).abs() < 1e-4);
        std::fs::remove_dir_all(dir).ok();
    }
}

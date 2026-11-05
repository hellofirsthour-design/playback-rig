//! The audio interface. The cpal stream lives on its own thread (it isn't Send on
//! macOS). The engine lives inside the audio callback; everything reaches it
//! through lock-free queues, and replaced engines are sent back out to be freed
//! off the audio thread.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rig_engine::{Arrangement, Command, Engine, Snapshot};
use serde::Serialize;
use std::sync::{mpsc, Arc, Mutex};

pub enum ToAudio {
    Cmd(Command),
    Swap(Box<Engine>),
    /// A new arrangement while playing, and the new index of the block playing now.
    Arrange(Box<Arrangement>, usize),
    /// Crossfade to this pad.
    Pad(Box<crate::pads::PadVoice>),
    PadOn(bool),
    PadLevel(f32),
    PadRoute(rig_engine::Route),
}

/// Things the audio thread hands back to be freed elsewhere. Only ever dropped.
#[allow(dead_code)]
pub enum Garbage {
    Engine(Box<Engine>),
    Arrangement(Box<Arrangement>),
    Pad(Box<crate::pads::PadVoice>),
}

#[derive(Clone, Debug, Serialize)]
pub struct OutputInfo {
    pub name: String,
    pub channels: usize,
    pub is_default: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct OpenOutput {
    pub name: String,
    pub channels: usize,
    pub sample_rate: u32,
}

pub struct Output {
    pub info: OpenOutput,
    pub tx: rtrb::Producer<ToAudio>,
    garbage: rtrb::Consumer<Garbage>,
    stop: mpsc::Sender<()>,
}

impl Output {
    pub fn send(&mut self, m: ToAudio) -> bool {
        self.tx.push(m).is_ok()
    }
    /// Frees engines the audio thread has finished with.
    pub fn collect_garbage(&mut self) {
        while self.garbage.pop().is_ok() {}
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

fn f32_channels(dev: &cpal::Device) -> usize {
    dev.supported_output_configs()
        .map(|cs| cs.filter(|c| c.sample_format() == cpal::SampleFormat::F32).map(|c| c.channels() as usize).max().unwrap_or(0))
        .unwrap_or(0)
}

pub fn list_outputs() -> Vec<OutputInfo> {
    let host = cpal::default_host();
    let default = host.default_output_device().and_then(|d| d.name().ok());
    let Ok(devs) = host.output_devices() else { return vec![] };
    devs.filter_map(|d| {
        let name = d.name().ok()?;
        let channels = f32_channels(&d);
        (channels > 0).then(|| OutputInfo { is_default: Some(&name) == default.as_ref(), name, channels })
    })
    .collect()
}

/// Opens an output (by name, or the system default) using all of its channels.
pub fn open(name: Option<String>, status: Arc<Mutex<Option<Snapshot>>>, on_error: impl Fn(String) + Send + 'static) -> Result<Output, String> {
    let (tx, mut rx) = rtrb::RingBuffer::<ToAudio>::new(1024);
    let (mut gtx, garbage) = rtrb::RingBuffer::<Garbage>::new(32);
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<OpenOutput, String>>();

    std::thread::Builder::new()
        .name("audio-output".into())
        .spawn(move || {
            let host = cpal::default_host();
            let dev = match &name {
                Some(n) => host.output_devices().ok().and_then(|mut ds| ds.find(|d| d.name().ok().as_deref() == Some(n.as_str()))),
                None => host.default_output_device(),
            };
            let Some(dev) = dev else {
                let _ = ready_tx.send(Err(match name {
                    Some(n) => format!("\"{n}\" isn't connected. Plug it in, or pick another output."),
                    None => "No audio output found.".into(),
                }));
                return;
            };
            let dev_name = dev.name().unwrap_or_else(|_| "Audio output".into());
            let configs: Vec<_> = match dev.supported_output_configs() {
                Ok(c) => c.filter(|c| c.sample_format() == cpal::SampleFormat::F32).collect(),
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("{dev_name}: {e}")));
                    return;
                }
            };
            let max_ch = configs.iter().map(|c| c.channels()).max().unwrap_or(0);
            let pick = configs
                .into_iter()
                .filter(|c| c.channels() == max_ch)
                .find_map(|c| [48_000, 44_100].into_iter().find_map(|sr| c.clone().try_with_sample_rate(cpal::SampleRate(sr))).or_else(|| Some(c.with_max_sample_rate())));
            let Some(cfg) = pick else {
                let _ = ready_tx.send(Err(format!("{dev_name} has no 32-bit float output mode.")));
                return;
            };
            let config: cpal::StreamConfig = cfg.config();
            let channels = config.channels as usize;
            let sample_rate = config.sample_rate.0;

            let mut engine: Option<Box<Engine>> = None;
            let mut pads = crate::pads::PadPlayer::new(sample_rate);
            let stream = dev.build_output_stream(
                &config,
                move |data: &mut [f32], _| {
                    while let Ok(m) = rx.pop() {
                        match m {
                            ToAudio::Cmd(c) => {
                                if let Some(e) = engine.as_mut() {
                                    e.handle(c)
                                }
                            }
                            ToAudio::Swap(e) => {
                                if let Some(old) = engine.replace(e) {
                                    let _ = gtx.push(Garbage::Engine(old));
                                }
                            }
                            ToAudio::Arrange(mut arr, keep) => {
                                if let Some(e) = engine.as_mut() {
                                    e.swap_arrangement(&mut arr, keep);
                                }
                                let _ = gtx.push(Garbage::Arrangement(arr));
                            }
                            ToAudio::Pad(v) => {
                                if let Some(old) = pads.swap(v) {
                                    let _ = gtx.push(Garbage::Pad(old));
                                }
                            }
                            ToAudio::PadOn(on) => pads.on = on,
                            ToAudio::PadLevel(l) => pads.level = l.clamp(0.0, 2.0),
                            ToAudio::PadRoute(r) => pads.route = r,
                        }
                    }
                    match engine.as_mut() {
                        Some(e) => {
                            e.process(data);
                            if let Ok(mut s) = status.try_lock() {
                                *s = Some(e.snapshot());
                            }
                        }
                        None => data.fill(0.0),
                    }
                    if let Some(done) = pads.process(data, channels) {
                        let _ = gtx.push(Garbage::Pad(done));
                    }
                },
                move |err| on_error(err.to_string()),
                None,
            );
            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("{dev_name}: couldn't start ({e})")));
                    return;
                }
            };
            if let Err(e) = stream.play() {
                let _ = ready_tx.send(Err(format!("{dev_name}: couldn't start ({e})")));
                return;
            }
            let _ = ready_tx.send(Ok(OpenOutput { name: dev_name, channels, sample_rate }));
            let _ = stop_rx.recv(); // until the Output is dropped
            drop(stream);
        })
        .map_err(|e| e.to_string())?;

    let info = ready_rx.recv().map_err(|_| "The audio thread stopped unexpectedly.".to_string())??;
    Ok(Output { info, tx, garbage, stop: stop_tx })
}

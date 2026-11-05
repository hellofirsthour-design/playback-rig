//! MIDI input ports (pedals, the Akai controller). Each open port lives on its own
//! thread, so the connection never has to cross threads. Messages go through the
//! engine crate's `MidiMap`, which handles learn and tap/hold.

use midir::MidiInput;
use std::sync::mpsc;

pub fn inputs() -> Vec<String> {
    let Ok(m) = MidiInput::new("Playback Rig") else { return vec![] };
    m.ports().iter().filter_map(|p| m.port_name(p).ok()).collect()
}

/// Lists MIDI inputs again and again without making a new MIDI client each time.
pub struct Watcher(Option<MidiInput>);

impl Watcher {
    pub fn new() -> Self {
        Watcher(MidiInput::new("Playback Rig watch").ok())
    }
    pub fn names(&self) -> Vec<String> {
        match &self.0 {
            Some(m) => m.ports().iter().filter_map(|p| m.port_name(p).ok()).collect(),
            None => inputs(),
        }
    }
}

/// Closes the port when dropped.
pub struct MidiPort {
    pub name: String,
    stop: mpsc::Sender<()>,
}

impl Drop for MidiPort {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

pub fn open(name: &str, mut on_msg: impl FnMut(&[u8]) + Send + 'static) -> Result<MidiPort, String> {
    let (stop, stop_rx) = mpsc::channel::<()>();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let port_name = name.to_string();
    std::thread::Builder::new()
        .name("midi-in".into())
        .spawn(move || {
            let conn = (|| {
                let m = MidiInput::new("Playback Rig").map_err(|e| e.to_string())?;
                let port = m
                    .ports()
                    .into_iter()
                    .find(|p| m.port_name(p).ok().as_deref() == Some(port_name.as_str()))
                    .ok_or_else(|| format!("\"{port_name}\" isn't connected."))?;
                m.connect(&port, "playback-rig-in", move |_, bytes, _| on_msg(bytes), ())
                    .map_err(|e| format!("Couldn't open \"{port_name}\": {e}"))
            })();
            match conn {
                Ok(c) => {
                    let _ = ready_tx.send(Ok(()));
                    let _ = stop_rx.recv();
                    c.close();
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        })
        .map_err(|e| e.to_string())?;
    ready_rx.recv().map_err(|_| "The MIDI thread stopped unexpectedly.".to_string())??;
    Ok(MidiPort { name: name.to_string(), stop })
}

//! Keeping the rig connected: if the audio interface or the MIDI controller is unplugged
//! (or its cable gets kicked), the set stays loaded and the app reconnects by itself as
//! soon as it comes back. These are the decisions; `watch` in main.rs carries them out.

#[derive(Debug, PartialEq)]
pub enum Step {
    /// All good.
    Fine,
    /// The open device is gone: close it and wait.
    Drop,
    /// Open this device (None = the system default).
    Open(Option<String>),
    /// Nothing to open yet.
    Wait,
}

/// Audio output. `want` is the interface chosen in Setup, `open` the one in use,
/// `failed` whether the open stream reported the device gone, `listed` what's plugged in.
/// While a song plays nothing is switched, except dropping a device that's gone.
pub fn audio_step(want: Option<&str>, open: Option<&str>, failed: bool, listed: &[String], playing: bool) -> Step {
    let here = |n: &str| listed.iter().any(|l| l == n);
    match open {
        Some(o) if failed || !here(o) => Step::Drop,
        Some(o) => match want {
            // On a stand-in (e.g. the laptop speakers) and the interface is back: switch,
            // but never in the middle of a song.
            Some(w) if w != o && here(w) && !playing => Step::Open(Some(w.to_string())),
            _ => Step::Fine,
        },
        None => match want {
            Some(w) if here(w) => Step::Open(Some(w.to_string())),
            Some(_) => Step::Wait, // wait for the interface; don't blast front of house out of the laptop
            None if !listed.is_empty() => Step::Open(None),
            None => Step::Wait,
        },
    }
}

/// MIDI input: `want` is the controller chosen in Setup, `connected` whether a port is open.
pub fn midi_step(want: Option<&str>, connected: bool, listed: &[String]) -> Step {
    let Some(w) = want else { return if connected { Step::Drop } else { Step::Fine } };
    let here = listed.iter().any(|l| l == w);
    match (connected, here) {
        (true, true) => Step::Fine,
        (true, false) => Step::Drop,
        (false, true) => Step::Open(Some(w.to_string())),
        (false, false) => Step::Wait,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn l(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn audio_unplugged_and_plugged_back_in() {
        let umc = Some("UMC1820");
        let both = l(&["MacBook Air Speakers", "UMC1820"]);
        let laptop = l(&["MacBook Air Speakers"]);
        assert_eq!(audio_step(umc, umc, false, &both, true), Step::Fine);
        assert_eq!(audio_step(umc, umc, false, &laptop, true), Step::Drop, "cable pulled mid-song");
        assert_eq!(audio_step(umc, umc, true, &both, false), Step::Drop, "the stream said the device is gone");
        assert_eq!(audio_step(umc, None, false, &laptop, false), Step::Wait, "never falls back to the laptop on its own");
        assert_eq!(audio_step(umc, None, false, &both, false), Step::Open(Some("UMC1820".into())), "back: reopen it");
    }

    #[test]
    fn a_stand_in_output_switches_back_only_when_stopped() {
        let both = l(&["MacBook Air Speakers", "UMC1820"]);
        let sp = Some("MacBook Air Speakers");
        assert_eq!(audio_step(Some("UMC1820"), sp, false, &both, true), Step::Fine);
        assert_eq!(audio_step(Some("UMC1820"), sp, false, &both, false), Step::Open(Some("UMC1820".into())));
        assert_eq!(audio_step(None, sp, false, &both, false), Step::Fine);
        assert_eq!(audio_step(None, None, false, &both, false), Step::Open(None));
    }

    #[test]
    fn midi_controller_reconnects() {
        let mm = Some("MIDImix");
        assert_eq!(midi_step(mm, true, &l(&["MIDImix"])), Step::Fine);
        assert_eq!(midi_step(mm, true, &l(&[])), Step::Drop);
        assert_eq!(midi_step(mm, false, &l(&[])), Step::Wait);
        assert_eq!(midi_step(mm, false, &l(&["IAC Bus", "MIDImix"])), Step::Open(Some("MIDImix".into())));
        assert_eq!(midi_step(None, false, &l(&["MIDImix"])), Step::Fine);
    }
}

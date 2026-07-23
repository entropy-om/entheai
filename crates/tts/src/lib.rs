//! Speaks entheai's assistant responses aloud via the OS-native TTS engine
//! (AVSpeechSynthesizer/NSSpeechSynthesizer on macOS via the `tts` crate) —
//! no models to fetch, no network access, no external tool.
//!
//! Constructing a [`Voice`] never fails hard: if the platform engine can't be
//! initialized (headless environments, CI, an unsupported OS), `speak`/`stop`
//! silently become no-ops rather than erroring the caller.

#[cfg(feature = "speech")]
pub struct Voice {
    inner: Option<tts::Tts>,
}

#[cfg(feature = "speech")]
impl Voice {
    /// Initialize the OS speech engine.
    pub fn new() -> Voice {
        Voice {
            inner: tts::Tts::default().ok(),
        }
    }

    /// Speak `text` aloud, interrupting anything currently speaking.
    pub fn speak(&mut self, text: &str) {
        if let Some(tts) = &mut self.inner {
            let _ = tts.speak(text, true);
        }
    }

    /// Stop any speech in progress.
    pub fn stop(&mut self) {
        if let Some(tts) = &mut self.inner {
            let _ = tts.stop();
        }
    }
}

#[cfg(not(feature = "speech"))]
pub struct Voice;

#[cfg(not(feature = "speech"))]
impl Voice {
    pub fn new() -> Voice {
        Voice
    }

    pub fn speak(&mut self, _text: &str) {}

    pub fn stop(&mut self) {}
}

impl Default for Voice {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_never_panics() {
        let _ = Voice::new();
    }

    #[test]
    fn speak_and_stop_are_safe_without_a_device() {
        let mut v = Voice::new();
        v.speak("test");
        v.stop();
    }
}

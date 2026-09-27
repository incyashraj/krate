//! Speaking text aloud with the voices the computer already has.
//!
//! The answer half of every voice app: the microphone and transcription let
//! an app listen, and until this nothing let it reply. It is the first item
//! of the capability roadmap's Tier 1.
//!
//! One worker thread per app owns the platform synthesizer, and every call
//! is a message to it. On macOS that is `AVSpeechSynthesizer`, which is not
//! thread-safe and was measured to work on a plain thread with no run loop
//! (speaking 40 ms after the call, `isSpeaking` true until the last word).
//! Linux speaks through `espeak-ng` and Windows through System.Speech in a
//! hidden PowerShell: helper processes, so `speaking` is simply whether the
//! helper is still running -- exact, not a guess from having started it.
//!
//! Speech belongs to the app: `say` replaces anything it was still saying,
//! and it stops when the app does.

use std::sync::mpsc;

/// Longest text one `say` accepts, in characters. A paragraph read aloud is
/// about a minute; this is several, and bounds what a guest can queue.
pub const MAX_TEXT_CHARS: usize = 4_000;
/// Slowest and fastest rate, as a multiple of normal speech.
pub const MIN_RATE: f32 = 0.5;
pub const MAX_RATE: f32 = 2.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SayError {
    InvalidRequest(String),
    Unsupported(String),
    Platform(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Voice {
    pub id: String,
    pub name: String,
    pub language: String,
}

/// Check a request before it reaches any synthesizer, and return the rate
/// to use. The same answer on every platform, whether or not it can speak.
pub fn validate(text: &str, rate: Option<f32>) -> Result<f32, SayError> {
    if text.trim().is_empty() {
        return Err(SayError::InvalidRequest("there is no text to say".into()));
    }
    let chars = text.chars().count();
    if chars > MAX_TEXT_CHARS {
        return Err(SayError::InvalidRequest(format!(
            "{chars} characters is more than one say takes ({MAX_TEXT_CHARS}); split it"
        )));
    }
    let rate = rate.unwrap_or(1.0);
    if !(MIN_RATE..=MAX_RATE).contains(&rate) {
        return Err(SayError::InvalidRequest(format!(
            "rate {rate} is outside {MIN_RATE} to {MAX_RATE}"
        )));
    }
    Ok(rate)
}

enum Command {
    Voices(mpsc::Sender<Result<Vec<Voice>, SayError>>),
    Say {
        text: String,
        voice: Option<String>,
        rate: f32,
        reply: mpsc::Sender<Result<(), SayError>>,
    },
    Stop(mpsc::Sender<()>),
    Speaking(mpsc::Sender<bool>),
}

/// What a platform synthesizer does. Lives on the worker thread only.
trait Backend {
    fn voices(&mut self) -> Result<Vec<Voice>, SayError>;
    fn say(&mut self, text: &str, voice: Option<&str>, rate: f32) -> Result<(), SayError>;
    fn stop(&mut self);
    fn speaking(&mut self) -> bool;
}

/// One app's speech. Starts its worker on first use, so an app that never
/// speaks costs nothing.
#[derive(Default)]
pub struct SpeechSynthesis {
    worker: Option<mpsc::Sender<Command>>,
}

impl SpeechSynthesis {
    fn send(&mut self, command: Command) -> Result<(), SayError> {
        if self.worker.is_none() {
            let (tx, rx) = mpsc::channel::<Command>();
            std::thread::Builder::new()
                .name("krate-speech".into())
                .spawn(move || serve(rx))
                .map_err(|err| SayError::Platform(format!("could not start speech: {err}")))?;
            self.worker = Some(tx);
        }
        let gone = || SayError::Platform("the speech worker stopped".into());
        self.worker
            .as_ref()
            .ok_or_else(gone)?
            .send(command)
            .map_err(|_| gone())
    }

    pub fn voices(&mut self) -> Result<Vec<Voice>, SayError> {
        let (reply, answer) = mpsc::channel();
        self.send(Command::Voices(reply))?;
        answer
            .recv()
            .map_err(|_| SayError::Platform("the speech worker stopped".into()))?
    }

    pub fn say(
        &mut self,
        text: &str,
        voice: Option<&str>,
        rate: Option<f32>,
    ) -> Result<(), SayError> {
        let rate = validate(text, rate)?;
        let (reply, answer) = mpsc::channel();
        self.send(Command::Say {
            text: text.to_string(),
            voice: voice.map(str::to_string),
            rate,
            reply,
        })?;
        answer
            .recv()
            .map_err(|_| SayError::Platform("the speech worker stopped".into()))?
    }

    /// Stop now. Never an error: with no worker there is nothing sounding.
    pub fn stop(&mut self) {
        if self.worker.is_none() {
            return;
        }
        let (reply, answer) = mpsc::channel();
        if self.send(Command::Stop(reply)).is_ok() {
            let _ = answer.recv();
        }
    }

    pub fn speaking(&mut self) -> bool {
        if self.worker.is_none() {
            return false;
        }
        let (reply, answer) = mpsc::channel();
        self.send(Command::Speaking(reply)).is_ok() && answer.recv().unwrap_or(false)
    }
}

impl Drop for SpeechSynthesis {
    /// The app is gone, so is its voice. Dropping the sender ends the
    /// worker's loop, which stops the synthesizer on its way out.
    fn drop(&mut self) {
        self.stop();
    }
}

/// The worker: build the backend where it will live, then answer commands
/// until the app's handle is dropped.
fn serve(commands: mpsc::Receiver<Command>) {
    let mut backend = platform_backend();
    for command in commands {
        match (&mut backend, command) {
            (Ok(backend), Command::Voices(reply)) => {
                let _ = reply.send(backend.voices());
            }
            (
                Ok(backend),
                Command::Say {
                    text,
                    voice,
                    rate,
                    reply,
                },
            ) => {
                let _ = reply.send(backend.say(&text, voice.as_deref(), rate));
            }
            (Ok(backend), Command::Stop(reply)) => {
                backend.stop();
                let _ = reply.send(());
            }
            (Ok(backend), Command::Speaking(reply)) => {
                let _ = reply.send(backend.speaking());
            }
            (Err(why), Command::Voices(reply)) => {
                let _ = reply.send(Err(why.clone()));
            }
            (Err(why), Command::Say { reply, .. }) => {
                let _ = reply.send(Err(why.clone()));
            }
            (Err(_), Command::Stop(reply)) => {
                let _ = reply.send(());
            }
            (Err(_), Command::Speaking(reply)) => {
                let _ = reply.send(false);
            }
        }
    }
    if let Ok(backend) = &mut backend {
        backend.stop();
    }
}

#[cfg(target_os = "macos")]
fn platform_backend() -> Result<Box<dyn Backend>, SayError> {
    macos::AvSpeech::new().map(|b| Box::new(b) as Box<dyn Backend>)
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn platform_backend() -> Result<Box<dyn Backend>, SayError> {
    Ok(Box::new(helper::Helper::default()))
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn platform_backend() -> Result<Box<dyn Backend>, SayError> {
    Err(SayError::Unsupported(
        "speaking aloud is not available on this system yet".into(),
    ))
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{Backend, SayError, Voice};
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::{NSArray, NSString};

    /// `AVSpeechSynthesizer`, reached by name: it lives in AVFAudio, which
    /// the AVFoundation the camera already links re-exports, and the
    /// generated crate this tree uses does not wrap it.
    pub(super) struct AvSpeech {
        synthesizer: Retained<AnyObject>,
        utterance: &'static AnyClass,
        voice: &'static AnyClass,
    }

    /// `AVSpeechUtteranceDefaultSpeechRate`, `...MaximumSpeechRate`.
    const DEFAULT_RATE: f32 = 0.5;
    const MAXIMUM_RATE: f32 = 1.0;
    /// `AVSpeechBoundaryImmediate`.
    const IMMEDIATE: isize = 0;

    impl AvSpeech {
        pub(super) fn new() -> Result<Self, SayError> {
            // A reference to anything in AVFoundation keeps the framework
            // linked, so the classes below are registered by name.
            let _ = unsafe { objc2_av_foundation::AVMediaTypeAudio };
            let class = |name: &core::ffi::CStr| {
                AnyClass::get(name).ok_or_else(|| {
                    SayError::Unsupported(format!(
                        "this Mac has no {} (AVFAudio)",
                        name.to_string_lossy()
                    ))
                })
            };
            let synthesizer_class = class(c"AVSpeechSynthesizer")?;
            let utterance = class(c"AVSpeechUtterance")?;
            let voice = class(c"AVSpeechSynthesisVoice")?;
            let synthesizer: Retained<AnyObject> = unsafe { msg_send![synthesizer_class, new] };
            Ok(Self {
                synthesizer,
                utterance,
                voice,
            })
        }
    }

    impl Backend for AvSpeech {
        fn voices(&mut self) -> Result<Vec<Voice>, SayError> {
            let voices: Retained<NSArray<AnyObject>> =
                unsafe { msg_send![self.voice, speechVoices] };
            Ok((0..voices.count())
                .map(|i| {
                    let voice = voices.objectAtIndex(i);
                    let text = |value: Retained<NSString>| value.to_string();
                    Voice {
                        id: text(unsafe { msg_send![&*voice, identifier] }),
                        name: text(unsafe { msg_send![&*voice, name] }),
                        language: text(unsafe { msg_send![&*voice, language] }),
                    }
                })
                .collect())
        }

        fn say(&mut self, text: &str, voice: Option<&str>, rate: f32) -> Result<(), SayError> {
            let chosen: Option<Retained<AnyObject>> = match voice {
                None => None,
                Some(id) => {
                    let id_ns = NSString::from_str(id);
                    let found: Option<Retained<AnyObject>> =
                        unsafe { msg_send![self.voice, voiceWithIdentifier: &*id_ns] };
                    Some(found.ok_or_else(|| {
                        SayError::InvalidRequest(format!("this Mac has no voice {id}"))
                    })?)
                }
            };
            let text = NSString::from_str(text);
            let utterance: Retained<AnyObject> =
                unsafe { msg_send![self.utterance, speechUtteranceWithString: &*text] };
            let rate = (DEFAULT_RATE * rate).clamp(0.0, MAXIMUM_RATE);
            unsafe {
                let _: () = msg_send![&*utterance, setRate: rate];
                if let Some(chosen) = &chosen {
                    let _: () = msg_send![&*utterance, setVoice: &**chosen];
                }
                let _: bool = msg_send![&*self.synthesizer, stopSpeakingAtBoundary: IMMEDIATE];
                let _: () = msg_send![&*self.synthesizer, speakUtterance: &*utterance];
            }
            Ok(())
        }

        fn stop(&mut self) {
            let _: bool =
                unsafe { msg_send![&*self.synthesizer, stopSpeakingAtBoundary: IMMEDIATE] };
        }

        fn speaking(&mut self) -> bool {
            unsafe { msg_send![&*self.synthesizer, isSpeaking] }
        }
    }
}

/// Linux and Windows: a helper process per line, killed to stop.
#[cfg(any(target_os = "linux", target_os = "windows"))]
mod helper {
    use super::{Backend, SayError, Voice};
    use std::io::Write;
    use std::process::{Child, Command, Stdio};

    #[derive(Default)]
    pub(super) struct Helper {
        child: Option<Child>,
        /// Listed once: on Windows each listing starts PowerShell, which is
        /// most of a second, and a chosen voice is checked on every `say`.
        voices: Option<Vec<Voice>>,
    }

    impl Backend for Helper {
        fn voices(&mut self) -> Result<Vec<Voice>, SayError> {
            if self.voices.is_none() {
                self.voices = Some(platform::voices()?);
            }
            Ok(self.voices.clone().unwrap_or_default())
        }

        fn say(&mut self, text: &str, voice: Option<&str>, rate: f32) -> Result<(), SayError> {
            if let Some(id) = voice {
                if !self.voices()?.iter().any(|v| v.id == id) {
                    return Err(SayError::InvalidRequest(format!(
                        "this computer has no voice {id}"
                    )));
                }
            }
            self.stop();
            let mut child = platform::command(voice, rate)?
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(platform::spawn_error)?;
            // The text goes in on stdin, never on a command line: nothing a
            // guest writes can become an argument or a script.
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(text.as_bytes())
                    .map_err(|err| SayError::Platform(err.to_string()))?;
            }
            self.child = Some(child);
            Ok(())
        }

        fn stop(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }

        fn speaking(&mut self) -> bool {
            match self.child.as_mut().map(|child| child.try_wait()) {
                Some(Ok(None)) => true,
                Some(_) => {
                    self.child = None;
                    false
                }
                None => false,
            }
        }
    }

    fn run(command: &mut Command) -> Result<String, SayError> {
        let out = command
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(platform::spawn_error)?;
        if !out.status.success() {
            return Err(SayError::Platform(format!(
                "listing voices failed ({})",
                out.status
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    #[cfg(target_os = "linux")]
    mod platform {
        use super::{run, SayError, Voice};
        use std::process::Command;

        /// espeak-ng's normal speed, words per minute.
        const NORMAL_WPM: f32 = 175.0;

        pub(super) fn spawn_error(err: std::io::Error) -> SayError {
            if err.kind() == std::io::ErrorKind::NotFound {
                SayError::Unsupported(
                    "this computer has no espeak-ng, which Krate speaks through on Linux; \
                     install it with your package manager"
                        .into(),
                )
            } else {
                SayError::Platform(err.to_string())
            }
        }

        pub(super) fn voices() -> Result<Vec<Voice>, SayError> {
            let listing = run(Command::new("espeak-ng").arg("--voices"))?;
            Ok(parse_voices(&listing))
        }

        /// `Pty Language Age/Gender VoiceName File Other Languages`, one
        /// voice a line after the header.
        pub(super) fn parse_voices(listing: &str) -> Vec<Voice> {
            listing
                .lines()
                .skip(1)
                .filter_map(|line| {
                    let cols: Vec<&str> = line.split_whitespace().collect();
                    (cols.len() >= 5).then(|| Voice {
                        id: cols[4].to_string(),
                        name: cols[3].replace('_', " "),
                        language: cols[1].to_string(),
                    })
                })
                .collect()
        }

        pub(super) fn command(voice: Option<&str>, rate: f32) -> Result<Command, SayError> {
            let mut command = Command::new("espeak-ng");
            if let Some(voice) = voice {
                command.arg("-v").arg(voice);
            }
            command
                .arg("-s")
                .arg(format!("{}", (NORMAL_WPM * rate).round() as u32))
                .arg("--stdin");
            Ok(command)
        }

        #[cfg(test)]
        mod tests {
            #[test]
            fn espeak_voices_are_read_by_column() {
                let listing = "Pty Language       Age/Gender VoiceName          File                 Other Languages\n \
                    5  af              --/M      Afrikaans          gmw/af\n \
                    2  en-gb           --/M      English_(Great_Britain) gmw/en            (en 2)\n";
                let voices = super::parse_voices(listing);
                assert_eq!(voices.len(), 2);
                assert_eq!(voices[1].id, "gmw/en");
                assert_eq!(voices[1].name, "English (Great Britain)");
                assert_eq!(voices[1].language, "en-gb");
            }
        }
    }

    #[cfg(target_os = "windows")]
    mod platform {
        use super::{run, SayError, Voice};
        use std::os::windows::process::CommandExt;
        use std::process::Command;

        /// No console window flashes up for the helper.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        pub(super) fn spawn_error(err: std::io::Error) -> SayError {
            SayError::Unsupported(format!(
                "Windows speech could not start (PowerShell: {err})"
            ))
        }

        fn powershell(script: &str) -> Command {
            let mut command = Command::new("powershell.exe");
            command
                .args(["-NoProfile", "-NonInteractive", "-Command", script])
                .creation_flags(CREATE_NO_WINDOW);
            command
        }

        pub(super) fn voices() -> Result<Vec<Voice>, SayError> {
            let listing = run(&mut powershell(
                "Add-Type -AssemblyName System.Speech; \
                 (New-Object System.Speech.Synthesis.SpeechSynthesizer).GetInstalledVoices() | \
                 Where-Object { $_.Enabled } | \
                 ForEach-Object { $_.VoiceInfo.Name + \"`t\" + $_.VoiceInfo.Culture.Name }",
            ))?;
            Ok(listing
                .lines()
                .filter_map(|line| {
                    let (name, language) = line.trim_end().split_once('\t')?;
                    Some(Voice {
                        id: name.to_string(),
                        name: name.to_string(),
                        language: language.to_string(),
                    })
                })
                .collect())
        }

        /// The voice and rate reach the script as environment variables and
        /// the text on stdin, so nothing the app supplies is ever parsed as
        /// PowerShell.
        pub(super) fn command(voice: Option<&str>, rate: f32) -> Result<Command, SayError> {
            // System.Speech's Rate runs -10..10 and is roughly logarithmic:
            // +10 is about twice as fast, -10 about half.
            let steps = (rate.log2() * 10.0).round().clamp(-10.0, 10.0) as i32;
            let mut command = powershell(
                "Add-Type -AssemblyName System.Speech; \
                 $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
                 if ($env:KRATE_SAY_VOICE) { $s.SelectVoice($env:KRATE_SAY_VOICE) }; \
                 $s.Rate = [int]$env:KRATE_SAY_RATE; \
                 $s.Speak([Console]::In.ReadToEnd())",
            );
            command
                .env("KRATE_SAY_VOICE", voice.unwrap_or(""))
                .env("KRATE_SAY_RATE", steps.to_string());
            Ok(command)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_checked_the_same_way_everywhere() {
        assert_eq!(validate("hello", None), Ok(1.0));
        assert_eq!(validate("hello", Some(0.5)), Ok(0.5));
        assert_eq!(validate("hello", Some(2.0)), Ok(2.0));
        for bad in ["", "   ", "\n"] {
            assert!(matches!(
                validate(bad, None),
                Err(SayError::InvalidRequest(_))
            ));
        }
        for rate in [0.49, 2.01, f32::NAN, f32::INFINITY, -1.0] {
            assert!(
                matches!(validate("hi", Some(rate)), Err(SayError::InvalidRequest(_))),
                "rate {rate} must be refused"
            );
        }
        let long = "a".repeat(MAX_TEXT_CHARS + 1);
        assert!(matches!(
            validate(&long, None),
            Err(SayError::InvalidRequest(_))
        ));
        // Characters, not bytes: 4,000 of a two-byte letter is allowed.
        assert_eq!(validate(&"é".repeat(MAX_TEXT_CHARS), None), Ok(1.0));
    }

    #[test]
    fn stopping_or_asking_before_speaking_starts_nothing() {
        let mut speech = SpeechSynthesis::default();
        speech.stop();
        assert!(!speech.speaking());
        assert!(
            speech.worker.is_none(),
            "no worker for an app that never spoke"
        );
    }

    /// Makes sound, so it is run by hand: `cargo test -p krate-runtime
    /// speech_synthesis -- --ignored`.
    #[test]
    #[ignore]
    fn a_line_is_spoken_and_then_is_not() {
        let mut speech = SpeechSynthesis::default();
        let voices = speech.voices().expect("voices");
        assert!(!voices.is_empty(), "this computer lists voices");
        speech.say("Krate can talk.", None, None).expect("say");
        let start = std::time::Instant::now();
        while !speech.speaking() && start.elapsed().as_secs() < 3 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(speech.speaking(), "speaking once started");
        while speech.speaking() && start.elapsed().as_secs() < 10 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!speech.speaking(), "and finished on its own");
        speech
            .say("This line is cut short.", None, Some(1.5))
            .expect("say");
        std::thread::sleep(std::time::Duration::from_millis(300));
        speech.stop();
        assert!(!speech.speaking(), "stop is immediate");
    }
}

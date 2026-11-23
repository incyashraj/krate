//! Audio recorder (a core plugin): the ribbon's microphone and two
//! commands start and stop a recording; it is saved where attachments go
//! as "Recording YYYYMMDDHHmmss" and embedded at the cursor of the note in
//! front, or opened when no note is. The notes app records with the
//! browser's MediaRecorder (.m4a or .webm); Vault takes the microphone's
//! samples from Krate (krate:audio/capture) and writes them as a .wav,
//! which needs no encoder and every player opens.

use crate::app::App;
use crate::dates::When;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// What the guest is asked to do with the microphone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    Start,
    Stop,
}

/// A recording's file name, without its extension.
pub fn recording_name(w: When) -> String {
    format!(
        "Recording {:04}{:02}{:02}{:02}{:02}{:02}",
        w.year, w.month, w.day, w.hour, w.minute, w.second
    )
}

/// Little-endian 16-bit samples as a .wav file.
pub fn wav(pcm: &[u8], rate: u32, channels: u16) -> Vec<u8> {
    let data = (pcm.len() & !1) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    let block = channels * 2;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * u32::from(block)).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    out.extend_from_slice(&pcm[..data as usize]);
    out
}

impl App {
    /// The ribbon's "Start/stop recording".
    pub fn record_toggle(&mut self) {
        self.record_want = Some(if self.mic_on { Want::Stop } else { Want::Start });
    }

    /// The microphone could not be opened: why, in the notes app's words
    /// where it has them.
    pub fn record_failed(&mut self, why: &str) {
        self.mic_on = false;
        self.say(why);
    }

    /// A finished recording: saved where attachments go, embedded at the
    /// cursor of the note in front, or opened.
    pub fn save_recording(&mut self, file: &[u8], ext: &str) {
        self.mic_on = false;
        let base = recording_name(self.local);
        let note = self.doc().map(|d| d.path.clone());
        let setting = String::from(self.settings.app.str_or("attachmentFolderPath", "/"));
        let folder = crate::workspace::attachment_folder(&setting, note.as_deref().unwrap_or(""));
        let made = self.vault.as_mut().map(|v| -> Result<String, String> {
            if !folder.is_empty() && !v.fs.exists(&folder) {
                v.fs.mkdir(&folder)?;
            }
            let path = v.free_name(&folder, &base, ext);
            v.fs.write_bytes(&path, file)?;
            v.scan();
            Ok(path)
        });
        match made {
            Some(Ok(path)) => {
                self.explorer_dirty();
                if note.is_some() {
                    let link = format!("\n![[{}]]\n", crate::vfs::file_name(&path));
                    let now = self.now;
                    if let Some(d) = self.doc_mut() {
                        d.insert(&link, now);
                    }
                } else {
                    self.open_file(&path, false);
                }
            }
            Some(Err(why)) => self.say(&format!("Could not save the recording: {why}")),
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::Vault;
    use crate::vfs::{MemFs, Vfs};
    use alloc::boxed::Box;

    #[test]
    fn a_recording_is_a_wav_named_and_embedded_as_the_notes_app_does() {
        let w = wav(&[1, 0, 2, 0, 3], 48_000, 1);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes([w[24], w[25], w[26], w[27]]), 48_000);
        assert_eq!(u32::from_le_bytes([w[40], w[41], w[42], w[43]]), 4);
        assert_eq!(w.len(), 48);
        let when = When {
            year: 2026,
            month: 11,
            day: 23,
            hour: 9,
            minute: 5,
            second: 7,
        };
        assert_eq!(recording_name(when), "Recording 20261123090507");

        let mut fs = MemFs::new();
        fs.write("Home.md", "Hello").unwrap();
        fs.write(
            ".obsidian/app.json",
            "{\"attachmentFolderPath\":\"Attachments\"}",
        )
        .unwrap();
        let mut a = App::new();
        a.open_vault(Vault::open("v", Box::new(fs)));
        a.local = when;
        a.open_file("Home.md", false);
        if let Some(d) = a.doc_mut() {
            d.cur.col = 5;
        }
        a.record_toggle();
        assert_eq!(a.record_want, Some(Want::Start));
        // A folder in memory keeps text only: the file's bytes here are
        // a stand-in; its encoding is checked above.
        a.save_recording(b"RIFF", ".wav");
        let text = a.doc().unwrap().buf.text();
        assert!(
            text.contains("\n![[Recording 20261123090507.wav]]\n"),
            "{text}"
        );
        assert!(a
            .vault
            .as_mut()
            .unwrap()
            .fs
            .exists("Attachments/Recording 20261123090507.wav"));
    }
}

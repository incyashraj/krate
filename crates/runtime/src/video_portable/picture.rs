//! A video's picture on a system without a native player (K-1348): the
//! H.264 track of an MP4, MOV or M4V file, demuxed by `mp4` and decoded by
//! OpenH264, handed over as RGBA in presentation order.
//!
//! The sample table is read once from the track's boxes (when each sample
//! is shown, which are key frames), so a seek finds its key frame without
//! reading a byte of the picture. A decoder hands frames back in the order
//! they are shown, which is not the order they are stored when the video
//! has B-frames; each frame takes the earliest presentation time still
//! waiting, which is the time that order gives it.

use std::collections::BTreeMap;
use std::io::{Read, Seek};

use krate_adapter_common::ui::ImagePixels;
use openh264::decoder::{DecodedYUV, Decoder, DecoderConfig, Flush};
use openh264::formats::YUVSource;
use openh264::OpenH264API;

/// One sample of the track, in stored (decode) order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Sample {
    /// When it is shown, in milliseconds from the start.
    pub millis: f64,
    /// A key frame: decoding may start here.
    pub key: bool,
}

/// A decoded picture: when it is shown, and its pixels unless it was
/// already late when it was decoded (a late frame is decoded, since later
/// frames need it, but never converted).
pub(super) struct Frame {
    pub millis: f64,
    pub pixels: Option<ImagePixels>,
}

/// What a file's picture is, when it has one Krate cannot decode here.
pub(super) enum Found<R> {
    Picture(Box<Picture<R>>),
    /// The file has a picture in a format OpenH264 does not decode.
    Unplayable(String),
    None,
}

pub(super) struct Picture<R> {
    reader: mp4::Mp4Reader<R>,
    track: u32,
    samples: Vec<Sample>,
    /// SPS and PPS as Annex B, fed before the first frame after a seek.
    headers: Vec<u8>,
    length_size: usize,
    decoder: Decoder,
    /// The next sample to feed, zero-based.
    next: usize,
    /// Presentation times fed and not yet handed back, as a multiset of
    /// whole microseconds (so it can be ordered).
    waiting: BTreeMap<i64, usize>,
    flushed: std::collections::VecDeque<Frame>,
    drained: bool,
    pub width: u32,
    pub height: u32,
    pub duration_millis: f64,
    packet: Vec<u8>,
}

/// True when the bytes start like an ISO media file (MP4, MOV, M4V, M4A).
pub(super) fn looks_like_mp4(head: &[u8]) -> bool {
    head.len() >= 8
        && matches!(
            &head[4..8],
            b"ftyp" | b"moov" | b"mdat" | b"free" | b"wide" | b"skip" | b"pnot"
        )
}

/// Convert one MP4 sample (NAL units each led by a `length_size`-byte
/// length) into Annex B (each led by a start code), appended to `out`.
/// A length that runs past the sample is refused, not trusted.
pub(super) fn annex_b(sample: &[u8], length_size: usize, out: &mut Vec<u8>) -> Result<(), String> {
    if !(1..=4).contains(&length_size) {
        return Err("this video's picture has an unknown layout".into());
    }
    let mut at = 0usize;
    while at < sample.len() {
        let Some(head) = sample.get(at..at + length_size) else {
            return Err("a picture in this video is cut short".into());
        };
        let length = head
            .iter()
            .fold(0usize, |length, byte| (length << 8) | usize::from(*byte));
        at += length_size;
        let Some(unit) = at.checked_add(length).and_then(|end| sample.get(at..end)) else {
            return Err("a picture in this video is cut short".into());
        };
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(unit);
        at += length;
    }
    Ok(())
}

/// A track's timing boxes, as plain numbers.
#[derive(Debug, Clone, Default)]
pub(super) struct Timing {
    /// The media's ticks a second (mdhd).
    pub timescale: u32,
    /// How many samples the track has (stsz).
    pub count: u32,
    /// (samples, ticks each) runs of decode time (stts).
    pub deltas: Vec<(u32, u32)>,
    /// (samples, ticks) runs of composition offset (ctts).
    pub offsets: Vec<(u32, i32)>,
    /// One-based key frame numbers (stss); `None` when every one is a key.
    pub keys: Option<Vec<u32>>,
    /// Where the presentation starts in the media's time (elst).
    pub start: Option<i64>,
}

/// The sample table: when each sample is shown (decode time plus its
/// composition offset, less the edit list's start) and which are key frames.
pub(super) fn sample_table(timing: &Timing) -> Vec<Sample> {
    let timescale = f64::from(timing.timescale.max(1));
    let count: usize = timing
        .deltas
        .iter()
        .map(|(count, _)| *count as usize)
        .sum::<usize>()
        .min(timing.count as usize);
    let mut decode = Vec::with_capacity(count);
    let mut time = 0i64;
    'stts: for (samples, delta) in &timing.deltas {
        for _ in 0..*samples {
            if decode.len() >= count {
                break 'stts;
            }
            decode.push(time);
            time += i64::from(*delta);
        }
    }
    let mut offsets = vec![0i64; count];
    let mut index = 0usize;
    'ctts: for (samples, offset) in &timing.offsets {
        for _ in 0..*samples {
            let Some(slot) = offsets.get_mut(index) else {
                break 'ctts;
            };
            *slot = i64::from(*offset);
            index += 1;
        }
    }
    // Encoders with B-frames start the presentation a frame or two into the
    // media; without the edit list every frame would show that much late.
    let start = timing.start.unwrap_or_else(|| {
        decode
            .iter()
            .zip(&offsets)
            .take(16)
            .map(|(decode, offset)| decode + offset)
            .min()
            .unwrap_or(0)
    });
    let keys: Option<std::collections::BTreeSet<u32>> = timing
        .keys
        .as_ref()
        .map(|keys| keys.iter().copied().collect());
    decode
        .iter()
        .zip(&offsets)
        .enumerate()
        .map(|(index, (decode, offset))| Sample {
            millis: (decode + offset - start) as f64 * 1000.0 / timescale,
            key: keys
                .as_ref()
                .is_none_or(|keys| keys.contains(&(index as u32 + 1))),
        })
        .collect()
}

/// Where to start decoding to show `millis`: the last key frame shown at or
/// before it (the first sample when there is none).
pub(super) fn key_before(samples: &[Sample], millis: f64) -> usize {
    samples
        .iter()
        .enumerate()
        .filter(|(_, sample)| sample.key && sample.millis <= millis)
        .map(|(index, _)| index)
        .next_back()
        .unwrap_or(0)
}

fn micros(millis: f64) -> i64 {
    (millis * 1000.0).round() as i64
}

fn new_decoder() -> Result<Decoder, String> {
    Decoder::with_api_config(
        OpenH264API::from_source(),
        // No flush after each frame: a video with B-frames needs the
        // decoder to hold frames back until their turn to be shown.
        DecoderConfig::new().flush_after_decode(Flush::NoFlush),
    )
    .map_err(|error| format!("the video decoder could not start: {error}"))
}

fn rgba(yuv: &DecodedYUV<'_>) -> Option<ImagePixels> {
    let (width, height) = yuv.dimensions();
    let mut rgba = vec![0u8; width.checked_mul(height)?.checked_mul(4)?];
    yuv.write_rgba8(&mut rgba);
    ImagePixels::new(width as u32, height as u32, rgba).ok()
}

/// The words for a picture format with no decoder here.
fn unplayable(kind: &str) -> String {
    format!(
        "this video's picture is {kind}, which Krate plays on this computer only as H.264; its sound plays"
    )
}

/// The four-letter names of the sample entries in the file's index (moov):
/// what each track is coded as. Reads the index only, at most 16 MiB of it,
/// and leaves the reader at the start.
fn sample_entries<R: Read + Seek>(reader: &mut R, size: u64) -> Vec<[u8; 4]> {
    const MAX_INDEX: u64 = 16 * 1024 * 1024;
    let mut found = Vec::new();
    let mut at = 0u64;
    let mut moov = None;
    while at + 8 <= size {
        let mut head = [0u8; 16];
        if reader.seek(std::io::SeekFrom::Start(at)).is_err()
            || reader.read_exact(&mut head[..8]).is_err()
        {
            break;
        }
        let mut length = u64::from(u32::from_be_bytes([head[0], head[1], head[2], head[3]]));
        let mut header = 8;
        if length == 1 {
            if reader.read_exact(&mut head[8..16]).is_err() {
                break;
            }
            length = u64::from_be_bytes(head[8..16].try_into().expect("eight bytes"));
            header = 16;
        } else if length == 0 {
            length = size - at;
        }
        if length < header {
            break;
        }
        if &head[4..8] == b"moov" {
            moov = Some((at + header, (length - header).min(MAX_INDEX)));
            break;
        }
        at += length;
    }
    if let Some((from, length)) = moov {
        let mut index = vec![0u8; length as usize];
        if reader.seek(std::io::SeekFrom::Start(from)).is_ok()
            && reader.read_exact(&mut index).is_ok()
        {
            // An stsd box: size, "stsd", version and flags, entry count,
            // then each entry's size and name.
            let mut i = 0;
            while let Some(found_at) = index[i..].windows(4).position(|w| w == b"stsd") {
                let entry = i + found_at + 4 + 8 + 4;
                if let Some(name) = index.get(entry..entry + 4) {
                    found.push(name.try_into().expect("four bytes"));
                }
                i += found_at + 4;
            }
        }
    }
    let _ = reader.seek(std::io::SeekFrom::Start(0));
    found
}

impl<R: Read + Seek> Picture<R> {
    /// The file's H.264 picture, if it has a picture track at all.
    pub(super) fn find(mut reader: R, size: u64) -> Result<Found<R>, String> {
        let entries = sample_entries(&mut reader, size);
        let reader = mp4::Mp4Reader::read_header(reader, size)
            .map_err(|error| format!("this video's file could not be read: {error}"))?;
        let mut video = None;
        for (id, track) in reader.tracks() {
            if !matches!(track.track_type(), Ok(mp4::TrackType::Video)) {
                continue;
            }
            let stsd = &track.trak.mdia.minf.stbl.stsd;
            if stsd.avc1.is_some() {
                video = Some(*id);
                break;
            }
            // The mp4 reader knows hev1 and vp09 only; an iPhone writes
            // hvc1, so the names come from the file's own boxes.
            let kind =
                if stsd.hev1.is_some() || entries.iter().any(|e| e == b"hvc1" || e == b"hev1") {
                    "HEVC (H.265)"
                } else if stsd.vp09.is_some() || entries.iter().any(|e| e == b"vp09") {
                    "VP9"
                } else if entries.iter().any(|e| e == b"av01") {
                    "AV1"
                } else {
                    "in a format with no decoder here"
                };
            return Ok(Found::Unplayable(unplayable(kind)));
        }
        let Some(track) = video else {
            return Ok(Found::None);
        };
        let trak = reader.tracks()[&track].trak.clone();
        let avc1 = trak
            .mdia
            .minf
            .stbl
            .stsd
            .avc1
            .as_ref()
            .expect("chosen for its avc1");
        let mut headers = Vec::new();
        for unit in avc1
            .avcc
            .sequence_parameter_sets
            .iter()
            .chain(&avc1.avcc.picture_parameter_sets)
        {
            headers.extend_from_slice(&[0, 0, 0, 1]);
            headers.extend_from_slice(&unit.bytes);
        }
        let stbl = &trak.mdia.minf.stbl;
        let timing = Timing {
            timescale: trak.mdia.mdhd.timescale,
            count: stbl.stsz.sample_count,
            deltas: stbl
                .stts
                .entries
                .iter()
                .map(|entry| (entry.sample_count, entry.sample_delta))
                .collect(),
            offsets: stbl.ctts.as_ref().map_or_else(Vec::new, |ctts| {
                ctts.entries
                    .iter()
                    .map(|entry| (entry.sample_count, entry.sample_offset))
                    .collect()
            }),
            keys: stbl.stss.as_ref().map(|stss| stss.entries.clone()),
            start: trak
                .edts
                .as_ref()
                .and_then(|edts| edts.elst.as_ref())
                .and_then(|elst| {
                    // An empty edit is written as -1 (all ones in its width).
                    elst.entries.iter().find(|entry| {
                        entry.media_time != u64::MAX && entry.media_time != u64::from(u32::MAX)
                    })
                })
                .map(|entry| entry.media_time as i64),
        };
        let samples = sample_table(&timing);
        // The last frame shows for one frame's time.
        let step = timing.deltas.first().map_or(0.0, |(_, delta)| {
            f64::from(*delta) * 1000.0 / f64::from(timing.timescale.max(1))
        });
        let duration_millis = samples
            .iter()
            .map(|sample| sample.millis)
            .fold(0.0f64, f64::max)
            + step;
        Ok(Found::Picture(Box::new(Self {
            reader,
            track,
            headers,
            length_size: usize::from(avc1.avcc.length_size_minus_one & 3) + 1,
            decoder: new_decoder()?,
            next: 0,
            waiting: BTreeMap::new(),
            flushed: Default::default(),
            drained: false,
            width: u32::from(avc1.width),
            height: u32::from(avc1.height),
            duration_millis,
            samples,
            packet: Vec::new(),
        })))
    }

    /// Start again from the key frame before `millis`. The frames before
    /// `millis` still come out of `next`, marked late by the caller.
    pub(super) fn seek(&mut self, millis: f64) -> Result<(), String> {
        self.decoder = new_decoder()?;
        self.next = key_before(&self.samples, millis);
        self.waiting.clear();
        self.flushed.clear();
        self.drained = false;
        Ok(())
    }

    fn take_time(&mut self) -> f64 {
        let Some((&time, _)) = self.waiting.iter().next() else {
            return 0.0;
        };
        let count = self.waiting.get_mut(&time).expect("present");
        *count -= 1;
        if *count == 0 {
            self.waiting.remove(&time);
        }
        time as f64 / 1000.0
    }

    /// The next frame in presentation order, converted to RGBA when
    /// `wanted(millis)` says it will be shown, or `None` at the end.
    pub(super) fn next(&mut self, wanted: impl Fn(f64) -> bool) -> Result<Option<Frame>, String> {
        loop {
            if let Some(frame) = self.flushed.pop_front() {
                return Ok(Some(frame));
            }
            if self.drained {
                return Ok(None);
            }
            let Some(sample) = self.samples.get(self.next).copied() else {
                // Every sample fed: what the decoder still holds comes out
                // now, in order.
                self.drained = true;
                let mut times = Vec::new();
                for (time, count) in std::mem::take(&mut self.waiting) {
                    times.extend(std::iter::repeat_n(time as f64 / 1000.0, count));
                }
                let frames = self
                    .decoder
                    .flush_remaining()
                    .map_err(|error| format!("the video could not be decoded: {error}"))?;
                for (yuv, millis) in frames.iter().zip(times) {
                    let pixels = if wanted(millis) { rgba(yuv) } else { None };
                    self.flushed.push_back(Frame { millis, pixels });
                }
                continue;
            };
            let index = self.next;
            self.next += 1;
            let Some(read) = self
                .reader
                .read_sample(self.track, index as u32 + 1)
                .map_err(|error| format!("this video's file could not be read: {error}"))?
            else {
                continue;
            };
            self.packet.clear();
            if sample.key {
                self.packet.extend_from_slice(&self.headers);
            }
            if annex_b(&read.bytes, self.length_size, &mut self.packet).is_err() {
                // One damaged picture: skip it, as a player does.
                continue;
            }
            *self.waiting.entry(micros(sample.millis)).or_default() += 1;
            let decoded = match self.decoder.decode(&self.packet) {
                Ok(Some(yuv)) => {
                    // The frame takes the earliest time waiting; convert it
                    // only if it will be shown.
                    let millis = self
                        .waiting
                        .keys()
                        .next()
                        .map_or(0.0, |time| *time as f64 / 1000.0);
                    Some(if wanted(millis) { rgba(&yuv) } else { None })
                }
                Ok(None) => None,
                Err(_) => {
                    // A frame the decoder could not read hands nothing
                    // back; its time must not shift every later frame.
                    let time = micros(sample.millis);
                    if let Some(count) = self.waiting.get_mut(&time) {
                        *count -= 1;
                        if *count == 0 {
                            self.waiting.remove(&time);
                        }
                    }
                    None
                }
            };
            if let Some(pixels) = decoded {
                let millis = self.take_time();
                return Ok(Some(Frame { millis, pixels }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const CLIP: &[u8] = include_bytes!("fixtures/clip.mp4");
    const HEVC: &[u8] = include_bytes!("fixtures/hevc.mp4");

    fn clip() -> Picture<Cursor<&'static [u8]>> {
        match Picture::find(Cursor::new(CLIP), CLIP.len() as u64).expect("reads") {
            Found::Picture(picture) => *picture,
            _ => panic!("the clip has an H.264 picture"),
        }
    }

    #[test]
    fn annex_b_leads_each_unit_with_a_start_code_and_refuses_a_lie() {
        let mut out = Vec::new();
        annex_b(&[0, 0, 0, 2, 9, 9, 0, 0, 0, 1, 7], 4, &mut out).unwrap();
        assert_eq!(out, [0, 0, 0, 1, 9, 9, 0, 0, 0, 1, 7]);
        // A length that runs past the sample, and a header cut in two.
        for bad in [&[0u8, 0, 0, 9, 1, 2][..], &[0, 0, 0, 1, 5, 0, 0][..]] {
            assert!(annex_b(bad, 4, &mut Vec::new()).is_err(), "{bad:?}");
        }
        assert!(annex_b(&[0, 0, 1, 2], 4, &mut Vec::new()).is_err());
        assert!(annex_b(&[0, 0], 0, &mut Vec::new()).is_err());
    }

    #[test]
    fn the_clip_decodes_every_frame_in_order_at_its_size() {
        assert!(looks_like_mp4(CLIP));
        let mut picture = clip();
        assert_eq!((picture.width, picture.height), (64, 64));
        assert!(
            (picture.duration_millis - 1000.0).abs() < 1.0,
            "{}",
            picture.duration_millis
        );
        let mut times = Vec::new();
        while let Some(frame) = picture.next(|_| true).expect("decodes") {
            let pixels = frame.pixels.expect("wanted, so converted");
            if times.is_empty() {
                // The colours ffmpeg decodes for the same frame (fixtures/
                // README.md), within the rounding of two YUV conversions.
                for ((x, y), want) in [
                    ((8, 8), [54, 52, 0]),
                    ((32, 32), [0, 8, 56]),
                    ((56, 8), [21, 61, 77]),
                    ((8, 56), [252, 0, 4]),
                    ((56, 56), [3, 254, 252]),
                    ((32, 10), [178, 126, 98]),
                ] {
                    let at = (y * 64 + x) * 4;
                    let got = &pixels.rgba[at..at + 4];
                    for (got, want) in got.iter().zip(want) {
                        assert!(got.abs_diff(want) <= 12, "({x},{y}): {got:?} for {want:?}");
                    }
                    assert_eq!(got[3], 255, "opaque");
                }
            }
            assert_eq!((pixels.width, pixels.height), (64, 64));
            assert_eq!(pixels.rgba.len(), 64 * 64 * 4);
            times.push(frame.millis);
        }
        assert_eq!(times.len(), 30, "one second at 30 frames: {times:?}");
        assert!(
            times.windows(2).all(|pair| pair[1] > pair[0]),
            "shown in order though stored with B-frames: {times:?}"
        );
        assert!(times[0].abs() < 1.0, "the first frame is at the start");
    }

    #[test]
    fn a_seek_lands_on_the_frame_asked_for() {
        let mut picture = clip();
        picture.seek(500.0).unwrap();
        let mut first = None;
        while let Some(frame) = picture.next(|millis| millis >= 499.0).expect("decodes") {
            if frame.pixels.is_some() {
                first = Some(frame.millis);
                break;
            }
        }
        let first = first.expect("a frame at or after 500 ms");
        assert!((500.0..534.0).contains(&first), "{first}");
        assert!(key_before(&picture.samples, 500.0) > 0);
        assert_eq!(key_before(&picture.samples, 0.0), 0);
    }

    #[test]
    fn a_late_frame_is_decoded_but_never_converted() {
        let mut picture = clip();
        let mut converted = 0;
        while let Some(frame) = picture.next(|millis| millis >= 900.0).unwrap() {
            converted += usize::from(frame.pixels.is_some());
        }
        assert_eq!(converted, 3, "900, 933 and 967 ms only");
    }

    #[test]
    fn a_picture_with_no_decoder_here_is_named() {
        match Picture::find(Cursor::new(HEVC), HEVC.len() as u64).expect("reads") {
            Found::Unplayable(words) => assert!(words.contains("HEVC"), "{words}"),
            _ => panic!("HEVC has no decoder here"),
        }
    }
}

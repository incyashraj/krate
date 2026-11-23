//! Where a portable video's bytes come from (K-1348): a file, or an address
//! read in parts through Krate's own HTTP client.
//!
//! The host has already judged the source -- a bundled asset, a picked
//! folder, the app's own folder, or an address the app may reach with
//! `net.connect` -- so this only reads it. An address is read with HTTP
//! ranges, a block at a time, keeping at most `MAX_BLOCKS` blocks: a long
//! film streams in a bounded amount of memory. A server that will not answer
//! a range is read whole, up to `MAX_WHOLE`. Redirects are not followed: the
//! grant names a host, and a redirect could send the read anywhere.

use std::collections::VecDeque;
use std::io::{self, Read, Seek, SeekFrom};
use std::time::Duration;

use symphonia::core::io::MediaSource;

use crate::video::Source;

/// One HTTP read.
const BLOCK: u64 = 256 * 1024;
/// Blocks kept per reader: 16 MiB.
const MAX_BLOCKS: usize = 64;
/// The most a server that ignores ranges may send.
const MAX_WHOLE: u64 = 64 * 1024 * 1024;

/// A reader over the source, opened fresh: the sound and the picture each
/// read with their own.
pub(super) fn open(source: &Source) -> Result<Box<dyn MediaSource>, String> {
    match source {
        Source::File(path) => std::fs::File::open(path)
            .map(|file| Box::new(file) as Box<dyn MediaSource>)
            .map_err(|error| format!("the video could not be read: {error}")),
        Source::Url(url) => Ok(Box::new(Http::open(url)?)),
    }
}

/// The source's length in bytes.
pub(super) fn length(reader: &mut dyn MediaSource) -> Result<u64, String> {
    if let Some(length) = reader.byte_len() {
        return Ok(length);
    }
    let end = reader
        .seek(SeekFrom::End(0))
        .map_err(|error| error.to_string())?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    Ok(end)
}

/// A file's name ending, lower case, as a hint for the sound's format.
pub(super) fn extension(source: &Source) -> Option<String> {
    let name = match source {
        Source::File(path) => path.file_name()?.to_str()?.to_string(),
        Source::Url(url) => url
            .split(['?', '#'])
            .next()?
            .rsplit('/')
            .next()?
            .to_string(),
    };
    let (_, ext) = name.rsplit_once('.')?;
    Some(ext.to_ascii_lowercase())
}

struct Http {
    url: String,
    agent: ureq::Agent,
    length: u64,
    at: u64,
    /// Blocks read, newest last, by block number.
    blocks: VecDeque<(u64, Vec<u8>)>,
    /// The whole body, from a server that ignores ranges.
    whole: Option<Vec<u8>>,
}

impl Http {
    fn open(url: &str) -> Result<Self, String> {
        let agent = krate_http::builder(url)
            .redirects(0)
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(30))
            .build();
        let mut http = Self {
            url: url.to_string(),
            agent,
            length: 0,
            at: 0,
            blocks: VecDeque::new(),
            whole: None,
        };
        let (status, total, body) = http.get(0, BLOCK - 1)?;
        match (status, total) {
            (206, Some(total)) => {
                http.length = total;
                http.keep(0, body);
            }
            (200, _) => {
                // The server sent everything: the body is the file, read
                // up to the bound.
                http.length = body.len() as u64;
                http.whole = Some(body);
            }
            (206, None) => {
                return Err("the server sent part of the video without saying its length".into())
            }
            (status, _) => return Err(format!("the server answered {status}")),
        }
        Ok(http)
    }

    /// GET `from..=to`: the status, the total length a range answer names,
    /// and the body.
    fn get(&self, from: u64, to: u64) -> Result<(u16, Option<u64>, Vec<u8>), String> {
        let response = match self
            .agent
            .get(&self.url)
            .set("Range", &format!("bytes={from}-{to}"))
            .call()
        {
            Ok(response) => response,
            Err(ureq::Error::Status(status, _)) => {
                return Err(format!("the server answered {status}"))
            }
            Err(error) => {
                let words = error.to_string();
                return Err(krate_http::explain(&words).unwrap_or(words));
            }
        };
        let status = response.status();
        if (300..400).contains(&status) {
            return Err(
                "the server sent the video to another address, and an app's video follows no redirect: use the address it ends at"
                    .into(),
            );
        }
        let total = response
            .header("content-range")
            .and_then(|range| range.rsplit('/').next())
            .and_then(|total| total.trim().parse::<u64>().ok());
        let limit = if status == 206 { BLOCK } else { MAX_WHOLE };
        let mut body = Vec::new();
        response
            .into_reader()
            .take(limit + 1)
            .read_to_end(&mut body)
            .map_err(|error| format!("the video stopped arriving: {error}"))?;
        if body.len() as u64 > limit {
            return Err(if status == 206 {
                "the server sent more than it was asked for".into()
            } else {
                "the video is larger than 64 MB and its server does not let Krate read it in parts"
                    .into()
            });
        }
        Ok((status, total, body))
    }

    fn keep(&mut self, block: u64, bytes: Vec<u8>) {
        if self.blocks.len() >= MAX_BLOCKS {
            self.blocks.pop_front();
        }
        self.blocks.push_back((block, bytes));
    }

    fn block(&mut self, block: u64) -> io::Result<&[u8]> {
        if let Some(index) = self.blocks.iter().position(|(at, _)| *at == block) {
            // The most recently used stays longest.
            let kept = self.blocks.remove(index).expect("index is in range");
            self.blocks.push_back(kept);
        } else {
            let from = block * BLOCK;
            let to = (from + BLOCK).min(self.length) - 1;
            let (status, _, body) = self.get(from, to).map_err(io::Error::other)?;
            if status != 206 {
                return Err(io::Error::other("the server stopped answering in parts"));
            }
            self.keep(block, body);
        }
        Ok(&self.blocks.back().expect("just kept").1)
    }
}

impl Read for Http {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.at >= self.length || out.is_empty() {
            return Ok(0);
        }
        let at = self.at;
        let count = if let Some(whole) = &self.whole {
            let start = at as usize;
            let count = out.len().min(whole.len() - start);
            out[..count].copy_from_slice(&whole[start..start + count]);
            count
        } else {
            let block = self.block(at / BLOCK)?;
            let offset = (at % BLOCK) as usize;
            if offset >= block.len() {
                return Err(io::Error::other(
                    "the server sent less than it was asked for",
                ));
            }
            let count = out.len().min(block.len() - offset);
            out[..count].copy_from_slice(&block[offset..offset + count]);
            count
        };
        self.at += count as u64;
        Ok(count)
    }
}

impl Seek for Http {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let at = match to {
            SeekFrom::Start(at) => Some(at),
            SeekFrom::End(back) => self.length.checked_add_signed(back),
            SeekFrom::Current(by) => self.at.checked_add_signed(by),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before the start"))?;
        self.at = at;
        Ok(at)
    }
}

impl MediaSource for Http {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.length)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A server on 127.0.0.1 that answers ranges of `body` (or the whole of
    /// it, when `ranges` is false), counting the requests it answered.
    pub(in crate::video_portable) fn serve(
        body: Vec<u8>,
        ranges: bool,
    ) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = format!("http://{}/clip.mp4", listener.local_addr().expect("addr"));
        let count = Arc::new(AtomicUsize::new(0));
        let counted = count.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let body = body.clone();
                let counted = counted.clone();
                std::thread::spawn(move || loop {
                    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                    let mut range = None;
                    let mut first = String::new();
                    if reader.read_line(&mut first).unwrap_or(0) == 0 {
                        return;
                    }
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                            break;
                        }
                        if let Some(value) = line.to_ascii_lowercase().strip_prefix("range: bytes=")
                        {
                            let (from, to) = value.trim().split_once('-').expect("range");
                            range = Some((
                                from.parse::<usize>().expect("from"),
                                to.parse::<usize>().expect("to"),
                            ));
                        }
                    }
                    counted.fetch_add(1, Ordering::SeqCst);
                    let reply = match range.filter(|_| ranges) {
                        Some((from, to)) => {
                            let to = to.min(body.len() - 1);
                            let part = &body[from..=to];
                            let mut head = format!(
                                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {from}-{to}/{}\r\n\r\n",
                                part.len(),
                                body.len()
                            )
                            .into_bytes();
                            head.extend_from_slice(part);
                            head
                        }
                        None => {
                            let mut head = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                                body.len()
                            )
                            .into_bytes();
                            head.extend_from_slice(&body);
                            head
                        }
                    };
                    if stream.write_all(&reply).is_err() {
                        return;
                    }
                });
            }
        });
        (address, count)
    }

    #[test]
    fn an_address_is_read_in_bounded_parts_and_seeks() {
        let body: Vec<u8> = (0..(BLOCK as usize * 3 + 1000))
            .map(|i| (i % 251) as u8)
            .collect();
        let (url, asked) = serve(body.clone(), true);
        let mut reader = open(&Source::Url(url)).expect("opens");
        assert_eq!(length(reader.as_mut()).unwrap(), body.len() as u64);
        // The end first, as an MP4 whose index is at its end is read.
        reader.seek(SeekFrom::End(-10)).unwrap();
        let mut tail = Vec::new();
        reader.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, body[body.len() - 10..]);
        reader.seek(SeekFrom::Start(BLOCK - 5)).unwrap();
        let mut across = [0u8; 10];
        reader.read_exact(&mut across).unwrap();
        assert_eq!(across, body[BLOCK as usize - 5..BLOCK as usize + 5]);
        // The first block (kept from opening), the last, and the one after
        // the boundary: nothing read twice, nothing read whole.
        assert_eq!(asked.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_server_without_ranges_is_read_whole() {
        let body = b"not ranged".to_vec();
        let (url, _) = serve(body.clone(), false);
        let mut reader = open(&Source::Url(url)).expect("opens");
        let mut all = Vec::new();
        reader.read_to_end(&mut all).unwrap();
        assert_eq!(all, body);
    }

    #[test]
    fn an_address_that_is_not_there_says_so() {
        let error = match open(&Source::Url("http://127.0.0.1:9/none.mp4".into())) {
            Ok(_) => panic!("nothing listens there"),
            Err(error) => error,
        };
        assert!(!error.is_empty());
    }

    #[test]
    fn a_name_ending_is_a_hint() {
        assert_eq!(
            extension(&Source::Url("https://x.test/a/Song.MP3?x=1".into())).as_deref(),
            Some("mp3")
        );
        assert_eq!(
            extension(&Source::File("/v/clip.mp4".into())).as_deref(),
            Some("mp4")
        );
        assert_eq!(extension(&Source::File("/v/noext".into())), None);
    }
}

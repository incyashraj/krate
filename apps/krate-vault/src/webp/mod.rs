//! WebP pictures (lossy VP8, lossless VP8L, alpha, animation), decoded in
//! the app. The decoder is image-webp 0.2.4 (MIT OR Apache-2.0, see
//! LICENSE-image-webp.txt here), ported to `no_std` + `alloc`: its use of
//! `std::io` goes through the small [`io`] module below instead, reading
//! from bytes in memory.

#![allow(clippy::all, clippy::pedantic)]

mod alpha_blending;
pub mod decoder;
mod extended;
mod huffman;
mod loop_filter;
mod lossless;
mod lossless_transform;
mod transform;
mod vp8;
mod vp8_arithmetic_decoder;
mod yuv;

pub use decoder::{DecodingError, WebPDecoder};

/// Just enough of `std::io` and `byteorder` for the decoder, over bytes in
/// memory.
pub mod io {
    use alloc::vec::Vec;
    use core::marker::PhantomData;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ErrorKind {
        UnexpectedEof,
        InvalidData,
        InvalidInput,
        Other,
    }

    #[derive(Debug)]
    pub struct Error {
        kind: ErrorKind,
    }

    impl Error {
        pub fn new<T>(kind: ErrorKind, _msg: T) -> Error {
            Error { kind }
        }

        pub fn kind(&self) -> ErrorKind {
            self.kind
        }
    }

    impl From<ErrorKind> for Error {
        fn from(kind: ErrorKind) -> Error {
            Error { kind }
        }
    }

    pub type Result<T> = core::result::Result<T, Error>;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum SeekFrom {
        Start(u64),
        End(i64),
        Current(i64),
    }

    pub trait Read {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize>;

        fn read_exact(&mut self, mut buf: &mut [u8]) -> Result<()> {
            while !buf.is_empty() {
                let n = self.read(buf)?;
                if n == 0 {
                    return Err(Error::from(ErrorKind::UnexpectedEof));
                }
                buf = &mut buf[n..];
            }
            Ok(())
        }

        fn read_to_end(&mut self, out: &mut Vec<u8>) -> Result<usize> {
            let mut chunk = [0u8; 4096];
            let start = out.len();
            loop {
                let n = self.read(&mut chunk)?;
                if n == 0 {
                    return Ok(out.len() - start);
                }
                out.extend_from_slice(&chunk[..n]);
            }
        }

        fn take(self, limit: u64) -> Take<Self>
        where
            Self: Sized,
        {
            Take { inner: self, limit }
        }

        fn by_ref(&mut self) -> &mut Self
        where
            Self: Sized,
        {
            self
        }
    }

    pub trait BufRead: Read {
        fn fill_buf(&mut self) -> Result<&[u8]>;
        fn consume(&mut self, amt: usize);
    }

    pub trait Seek {
        fn seek(&mut self, pos: SeekFrom) -> Result<u64>;

        fn seek_relative(&mut self, offset: i64) -> Result<()> {
            self.seek(SeekFrom::Current(offset)).map(|_| ())
        }

        fn stream_position(&mut self) -> Result<u64> {
            self.seek(SeekFrom::Current(0))
        }
    }

    impl<R: Read + ?Sized> Read for &mut R {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            (**self).read(buf)
        }
    }

    impl<R: BufRead + ?Sized> BufRead for &mut R {
        fn fill_buf(&mut self) -> Result<&[u8]> {
            (**self).fill_buf()
        }
        fn consume(&mut self, amt: usize) {
            (**self).consume(amt)
        }
    }

    impl<R: Seek + ?Sized> Seek for &mut R {
        fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
            (**self).seek(pos)
        }
    }

    impl Read for &[u8] {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            let n = buf.len().min(self.len());
            buf[..n].copy_from_slice(&self[..n]);
            *self = &self[n..];
            Ok(n)
        }
    }

    impl BufRead for &[u8] {
        fn fill_buf(&mut self) -> Result<&[u8]> {
            Ok(self)
        }
        fn consume(&mut self, amt: usize) {
            *self = &self[amt.min(self.len())..];
        }
    }

    /// Bytes in memory read in order, with a position that can be moved.
    #[derive(Debug, Clone)]
    pub struct Cursor<T> {
        inner: T,
        pos: u64,
    }

    impl<T> Cursor<T> {
        pub fn new(inner: T) -> Cursor<T> {
            Cursor { inner, pos: 0 }
        }

        pub fn position(&self) -> u64 {
            self.pos
        }

        pub fn set_position(&mut self, pos: u64) {
            self.pos = pos;
        }

        pub fn get_ref(&self) -> &T {
            &self.inner
        }

        pub fn into_inner(self) -> T {
            self.inner
        }
    }

    impl<T: AsRef<[u8]>> Cursor<T> {
        fn rest(&self) -> &[u8] {
            let b = self.inner.as_ref();
            let p = (self.pos as usize).min(b.len());
            &b[p..]
        }
    }

    impl<T: AsRef<[u8]>> Read for Cursor<T> {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            let rest = self.rest();
            let n = buf.len().min(rest.len());
            buf[..n].copy_from_slice(&rest[..n]);
            self.pos += n as u64;
            Ok(n)
        }
    }

    impl<T: AsRef<[u8]>> BufRead for Cursor<T> {
        fn fill_buf(&mut self) -> Result<&[u8]> {
            Ok(self.rest())
        }
        fn consume(&mut self, amt: usize) {
            self.pos += amt as u64;
        }
    }

    impl<T: AsRef<[u8]>> Seek for Cursor<T> {
        fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
            let len = self.inner.as_ref().len() as i64;
            let p = match pos {
                SeekFrom::Start(p) => p as i64,
                SeekFrom::End(d) => len + d,
                SeekFrom::Current(d) => self.pos as i64 + d,
            };
            if p < 0 {
                return Err(Error::from(ErrorKind::InvalidInput));
            }
            self.pos = p as u64;
            Ok(self.pos)
        }
    }

    pub struct Take<R> {
        inner: R,
        limit: u64,
    }

    impl<R> Take<R> {
        pub fn limit(&self) -> u64 {
            self.limit
        }

        pub fn into_inner(self) -> R {
            self.inner
        }

        pub fn get_mut(&mut self) -> &mut R {
            &mut self.inner
        }
    }

    impl<R: Read> Read for Take<R> {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            if self.limit == 0 {
                return Ok(0);
            }
            let max = (buf.len() as u64).min(self.limit) as usize;
            let n = self.inner.read(&mut buf[..max])?;
            self.limit -= n as u64;
            Ok(n)
        }
    }

    impl<R: BufRead> BufRead for Take<R> {
        fn fill_buf(&mut self) -> Result<&[u8]> {
            if self.limit == 0 {
                return Ok(&[]);
            }
            let limit = self.limit as usize;
            let buf = self.inner.fill_buf()?;
            let n = buf.len().min(limit);
            Ok(&buf[..n])
        }
        fn consume(&mut self, amt: usize) {
            let amt = (amt as u64).min(self.limit) as usize;
            self.limit -= amt as u64;
            self.inner.consume(amt);
        }
    }

    /// `byteorder`'s little-endian marker.
    pub enum LittleEndian {}

    pub struct Order<B>(PhantomData<B>);

    /// `byteorder`'s readers; every WebP number is little-endian.
    pub trait ReadBytesExt: Read {
        fn read_u8(&mut self) -> Result<u8> {
            let mut b = [0u8; 1];
            self.read_exact(&mut b)?;
            Ok(b[0])
        }

        fn read_i8(&mut self) -> Result<i8> {
            Ok(self.read_u8()? as i8)
        }

        fn read_u16<B>(&mut self) -> Result<u16> {
            let mut b = [0u8; 2];
            self.read_exact(&mut b)?;
            Ok(u16::from_le_bytes(b))
        }

        fn read_u24<B>(&mut self) -> Result<u32> {
            let mut b = [0u8; 3];
            self.read_exact(&mut b)?;
            Ok(u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16)
        }

        fn read_u32<B>(&mut self) -> Result<u32> {
            let mut b = [0u8; 4];
            self.read_exact(&mut b)?;
            Ok(u32::from_le_bytes(b))
        }
    }

    impl<R: Read + ?Sized> ReadBytesExt for R {}
}

use alloc::vec::Vec;

/// A decoded WebP: its size, RGBA pixels of each frame and each frame's
/// time on screen in milliseconds (one frame and 0 for a still picture).
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub frames: Vec<(Vec<u8>, u32)>,
}

/// Decode a WebP file in memory.
pub fn decode(bytes: &[u8]) -> Result<Decoded, alloc::string::String> {
    let mut d = WebPDecoder::new(io::Cursor::new(bytes))
        .map_err(|e| alloc::format!("The WebP could not be read ({e:?})"))?;
    let (w, h) = d.dimensions();
    let has_alpha = d.has_alpha();
    let size = d
        .output_buffer_size()
        .ok_or_else(|| alloc::string::String::from("The WebP is too large"))?;
    let to_rgba = |buf: Vec<u8>| -> Vec<u8> {
        if has_alpha {
            return buf;
        }
        let mut out = Vec::with_capacity(buf.len() / 3 * 4);
        for px in buf.chunks_exact(3) {
            out.extend_from_slice(&[px[0], px[1], px[2], 255]);
        }
        out
    };
    let mut frames = Vec::new();
    if d.is_animated() {
        let n = d.num_frames().min(400);
        for _ in 0..n {
            let mut buf = alloc::vec![0u8; size];
            match d.read_frame(&mut buf) {
                Ok(delay) => frames.push((to_rgba(buf), delay)),
                Err(_) => break,
            }
        }
    }
    if frames.is_empty() {
        let mut buf = alloc::vec![0u8; size];
        d.read_image(&mut buf)
            .map_err(|e| alloc::format!("The WebP could not be read ({e:?})"))?;
        frames.push((to_rgba(buf), 0));
    }
    Ok(Decoded {
        width: w,
        height: h,
        frames,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn lossless_lossy_and_animated_webp_decode() {
        let l = super::decode(include_bytes!("fixtures/lossless.webp")).unwrap();
        assert_eq!((l.width, l.height, l.frames.len()), (16, 8, 1));
        let px = |f: &[u8], x: usize, y: usize| -> [u8; 4] {
            let i = (y * 16 + x) * 4;
            [f[i], f[i + 1], f[i + 2], f[i + 3]]
        };
        assert_eq!(px(&l.frames[0].0, 0, 0), [255, 0, 0, 255]);
        assert_eq!(px(&l.frames[0].0, 15, 7), [0, 0, 255, 128]);
        let y = super::decode(include_bytes!("fixtures/lossy.webp")).unwrap();
        let p = px(&y.frames[0].0, 2, 2);
        assert!(p[0] > 200 && p[2] < 60 && p[3] == 255, "{p:?}");
        let a = super::decode(include_bytes!("fixtures/anim.webp")).unwrap();
        assert_eq!(a.frames.len(), 3);
        assert_eq!(
            a.frames.iter().map(|f| f.1).collect::<alloc::vec::Vec<_>>(),
            [100, 200, 300]
        );
        let g = &a.frames[1].0;
        assert!(
            g[0] < 10 && g[1] > 240 && g[2] < 10,
            "the second frame is green"
        );
        assert!(super::decode(b"RIFF\0\0\0\0WEBPjunk").is_err());
    }
}

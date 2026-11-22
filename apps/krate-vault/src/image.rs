//! Pictures in notes: PNG, JPEG, GIF (every frame, for playing), BMP,
//! WebP (lossy, lossless, animated) and SVG decoded to RGBA in the app,
//! pure Rust and `no_std`, so `![[photo.jpg]]` shows the photo with
//! nothing from the system. AVIF is named in the error the note shows:
//! there is no AV1 decoder that runs in the app (K-1345).

use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;
use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;

/// A decoded picture, row by row from the top left, four bytes a pixel.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// An animation's frames and how long each shows (ms), the first being
    /// `rgba`; empty for a still picture.
    pub anim: Vec<(Vec<u8>, u32)>,
    /// An SVG's text, drawn by the caller over the picture.
    pub texts: Vec<crate::svg::Text>,
    /// The size it is shown at before any |width (an SVG is rasterised
    /// at twice its size, to stay sharp, and shown at its own).
    pub logical: (u32, u32),
}

impl Picture {
    pub fn still(width: u32, height: u32, rgba: Vec<u8>) -> Picture {
        Picture {
            width,
            height,
            rgba,
            anim: Vec::new(),
            texts: Vec::new(),
            logical: (width, height),
        }
    }
}

/// The widest or tallest a picture is kept at; a bigger one is scaled
/// down, as a note column never shows more.
pub const MAX_SIDE: u32 = 2048;

fn err(s: &str) -> String {
    String::from(s)
}

pub fn decode(bytes: &[u8]) -> Result<Picture, String> {
    let pic = if bytes.starts_with(b"\x89PNG") {
        png(bytes)?
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        jpeg(bytes)?
    } else if bytes.starts_with(b"GIF8") {
        gif(bytes)?
    } else if bytes.starts_with(b"BM") {
        bmp(bytes)?
    } else if bytes.get(8..12) == Some(b"WEBP") {
        let d = crate::webp::decode(bytes)?;
        let mut p = Picture::still(d.width, d.height, Vec::new());
        if d.frames.len() > 1 {
            p.anim = d.frames;
            p.rgba = p.anim.first().map(|f| f.0.clone()).unwrap_or_default();
        } else {
            p.rgba = d.frames.into_iter().next().map(|f| f.0).unwrap_or_default();
        }
        p
    } else if bytes
        .get(4..12)
        .is_some_and(|b| b.starts_with(b"ftypavi") || b.starts_with(b"ftypavis"))
    {
        return Err(err("AVIF pictures cannot be shown in Vault yet (K-1345)"));
    } else if bytes
        .iter()
        .take(1024)
        .copied()
        .collect::<Vec<u8>>()
        .windows(4)
        .any(|w| w == b"<svg")
    {
        svg(bytes)?
    } else {
        return Err(err("This file is not a picture Vault can read"));
    };
    Ok(shrink(pic))
}

/// An SVG rasterised at its own size (at most MAX_SIDE), twice that when
/// it is small, so it stays sharp on a high-density screen.
fn svg(bytes: &[u8]) -> Result<Picture, String> {
    let text = core::str::from_utf8(bytes).map_err(|_| err("The SVG is not text"))?;
    let doc = crate::svg::parse(text)?;
    let scale = if doc.w.max(doc.h) <= 1024.0 { 2.0 } else { 1.0 };
    let fit = (MAX_SIDE as f32 / (doc.w.max(doc.h) * scale)).min(1.0);
    let w = ((doc.w * scale * fit).round() as u32).max(1);
    let h = ((doc.h * scale * fit).round() as u32).max(1);
    let (rgba, texts) = crate::svg::rasterise(&doc, w, h);
    let mut p = Picture::still(w, h, rgba);
    p.texts = texts;
    p.logical = (
        (doc.w * fit).round().max(1.0) as u32,
        (doc.h * fit).round().max(1.0) as u32,
    );
    Ok(p)
}

fn png(bytes: &[u8]) -> Result<Picture, String> {
    let opts = DecoderOptions::default()
        .png_set_add_alpha_channel(true)
        .png_set_strip_to_8bit(true)
        .set_max_width(1 << 15)
        .set_max_height(1 << 15);
    let mut d = zune_png::PngDecoder::new_with_options(ZCursor::new(bytes), opts);
    let raw = d
        .decode_raw()
        .map_err(|_| err("The PNG could not be read"))?;
    let (w, h) = d.dimensions().ok_or_else(|| err("The PNG has no size"))?;
    let cs = d.colorspace().unwrap_or(ColorSpace::RGBA);
    Ok(Picture::still(w as u32, h as u32, to_rgba(raw, cs, w * h)?))
}

fn jpeg(bytes: &[u8]) -> Result<Picture, String> {
    let opts = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGBA)
        .set_max_width(1 << 15)
        .set_max_height(1 << 15);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), opts);
    let raw = d.decode().map_err(|_| err("The JPEG could not be read"))?;
    let (w, h) = d.dimensions().ok_or_else(|| err("The JPEG has no size"))?;
    let cs = d.output_colorspace().unwrap_or(ColorSpace::RGBA);
    Ok(Picture::still(w as u32, h as u32, to_rgba(raw, cs, w * h)?))
}

fn to_rgba(raw: Vec<u8>, cs: ColorSpace, pixels: usize) -> Result<Vec<u8>, String> {
    let n = cs.num_components();
    if raw.len() < pixels * n {
        return Err(err("The picture ended early"));
    }
    if n == 4 && cs == ColorSpace::RGBA {
        return Ok(raw);
    }
    let mut out = Vec::with_capacity(pixels * 4);
    for px in raw.chunks_exact(n).take(pixels) {
        let g = |i: usize| px.get(i).copied().unwrap_or(255);
        match n {
            1 => out.extend_from_slice(&[g(0), g(0), g(0), 255]),
            2 => out.extend_from_slice(&[g(0), g(0), g(0), g(1)]),
            3 => out.extend_from_slice(&[g(0), g(1), g(2), 255]),
            _ => out.extend_from_slice(&[g(0), g(1), g(2), g(3)]),
        }
    }
    Ok(out)
}

/// Scale down (box filter) so neither side passes [`MAX_SIDE`].
fn shrink(p: Picture) -> Picture {
    let big = p.width.max(p.height);
    if big <= MAX_SIDE || p.width == 0 || p.height == 0 {
        return p;
    }
    let step = big.div_ceil(MAX_SIDE).max(2) as usize;
    let (w, h) = (p.width as usize, p.height as usize);
    let (nw, nh) = (w / step, h / step);
    let small = |rgba: &[u8]| -> Vec<u8> {
        let mut out = Vec::with_capacity(nw * nh * 4);
        for y in 0..nh {
            for x in 0..nw {
                let mut acc = [0u32; 4];
                for dy in 0..step {
                    for dx in 0..step {
                        let i = ((y * step + dy) * w + x * step + dx) * 4;
                        for (k, a) in acc.iter_mut().enumerate() {
                            *a += u32::from(rgba.get(i + k).copied().unwrap_or(0));
                        }
                    }
                }
                let n = (step * step) as u32;
                out.extend(acc.iter().map(|a| (a / n) as u8));
            }
        }
        out
    };
    let mut out = Picture::still(nw as u32, nh as u32, small(&p.rgba));
    out.anim = p.anim.iter().map(|(f, d)| (small(f), *d)).collect();
    out.texts = p.texts;
    out.logical = p.logical;
    out
}

// ---- GIF: every frame -----------------------------------------------------------------

fn le16(b: &[u8], at: usize) -> usize {
    usize::from(b.get(at).copied().unwrap_or(0))
        | (usize::from(b.get(at + 1).copied().unwrap_or(0)) << 8)
}

fn gif(b: &[u8]) -> Result<Picture, String> {
    let bad = || err("The GIF could not be read");
    let w = le16(b, 6);
    let h = le16(b, 8);
    if w == 0 || h == 0 || w * h > (1 << 26) {
        return Err(bad());
    }
    let flags = b.get(10).copied().ok_or_else(bad)?;
    let mut at = 13;
    let mut global: Vec<[u8; 3]> = Vec::new();
    if flags & 0x80 != 0 {
        let n = 2usize << (flags & 7);
        for i in 0..n {
            let p = at + i * 3;
            global.push([
                b.get(p).copied().unwrap_or(0),
                b.get(p + 1).copied().unwrap_or(0),
                b.get(p + 2).copied().unwrap_or(0),
            ]);
        }
        at += n * 3;
    }
    let mut transparent: Option<u8> = None;
    let mut disposal = 0u8;
    let mut delay = 0u32;
    let mut out = alloc::vec![0u8; w * h * 4];
    let mut frames: Vec<(Vec<u8>, u32)> = Vec::new();
    let done = |frames: Vec<(Vec<u8>, u32)>| -> Result<Picture, String> {
        let first = frames.first().map(|f| f.0.clone()).ok_or_else(bad)?;
        let mut p = Picture::still(w as u32, h as u32, first);
        if frames.len() > 1 {
            p.anim = frames;
        }
        Ok(p)
    };
    loop {
        match b.get(at).copied() {
            Some(0x21) => {
                // An extension: a graphics control block says how long the
                // next frame shows, how it is cleared, and its transparent
                // colour.
                let label = b.get(at + 1).copied().unwrap_or(0);
                at += 2;
                if label == 0xF9 && b.get(at).copied() == Some(4) {
                    let packed = b.get(at + 1).copied().unwrap_or(0);
                    disposal = (packed >> 2) & 7;
                    delay = le16(b, at + 2) as u32 * 10;
                    transparent = (packed & 1 != 0).then(|| b.get(at + 4).copied()).flatten();
                }
                while let Some(len) = b.get(at).copied() {
                    at += 1 + usize::from(len);
                    if len == 0 {
                        break;
                    }
                }
            }
            Some(0x2C) => {
                let fx = le16(b, at + 1);
                let fy = le16(b, at + 3);
                let fw = le16(b, at + 5);
                let fh = le16(b, at + 7);
                let f = b.get(at + 9).copied().unwrap_or(0);
                at += 10;
                let mut table = global.clone();
                if f & 0x80 != 0 {
                    let n = 2usize << (f & 7);
                    table.clear();
                    for i in 0..n {
                        let p = at + i * 3;
                        table.push([
                            b.get(p).copied().unwrap_or(0),
                            b.get(p + 1).copied().unwrap_or(0),
                            b.get(p + 2).copied().unwrap_or(0),
                        ]);
                    }
                    at += n * 3;
                }
                let interlaced = f & 0x40 != 0;
                let min_code = usize::from(b.get(at).copied().ok_or_else(bad)?);
                at += 1;
                let mut data: Vec<u8> = Vec::new();
                while let Some(len) = b.get(at).copied() {
                    at += 1;
                    if len == 0 {
                        break;
                    }
                    data.extend_from_slice(b.get(at..at + usize::from(len)).ok_or_else(bad)?);
                    at += usize::from(len);
                }
                let idx = lzw(&data, min_code, fw * fh).ok_or_else(bad)?;
                let rows: Vec<usize> = if interlaced {
                    let mut r = Vec::new();
                    for (start, step) in [(0, 8), (4, 8), (2, 4), (1, 2)] {
                        let mut y = start;
                        while y < fh {
                            r.push(y);
                            y += step;
                        }
                    }
                    r
                } else {
                    (0..fh).collect()
                };
                let before = if disposal == 3 {
                    Some(out.clone())
                } else {
                    None
                };
                for (k, y) in rows.iter().enumerate() {
                    for x in 0..fw {
                        let Some(&c) = idx.get(k * fw + x) else {
                            continue;
                        };
                        if Some(c) == transparent {
                            continue;
                        }
                        let (px, py) = (fx + x, fy + y);
                        if px >= w || py >= h {
                            continue;
                        }
                        let rgb = table.get(usize::from(c)).copied().unwrap_or([0, 0, 0]);
                        let i = (py * w + px) * 4;
                        if let Some(dst) = out.get_mut(i..i + 4) {
                            dst.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
                        }
                    }
                }
                // As browsers do, a frame asking for 10 ms or less shows
                // for 100 ms.
                frames.push((out.clone(), if delay <= 10 { 100 } else { delay }));
                if frames.len() >= 300 {
                    return done(frames);
                }
                match disposal {
                    2 => {
                        for y in fy..(fy + fh).min(h) {
                            for x in fx..(fx + fw).min(w) {
                                let i = (y * w + x) * 4;
                                if let Some(dst) = out.get_mut(i..i + 4) {
                                    dst.copy_from_slice(&[0, 0, 0, 0]);
                                }
                            }
                        }
                    }
                    3 => {
                        if let Some(b0) = before {
                            out = b0;
                        }
                    }
                    _ => {}
                }
                transparent = None;
                disposal = 0;
                delay = 0;
            }
            Some(0x3B) | None => return done(frames),
            _ => {
                if frames.is_empty() {
                    return Err(bad());
                }
                return done(frames);
            }
        }
    }
}

/// GIF's variable-width LZW: colour indexes, at most `want` of them.
fn lzw(data: &[u8], min_code: usize, want: usize) -> Option<Vec<u8>> {
    if !(2..=11).contains(&min_code) {
        return None;
    }
    let clear = 1usize << min_code;
    let end = clear + 1;
    let mut prefix: Vec<u16> = alloc::vec![0; 4096];
    let mut suffix: Vec<u8> = alloc::vec![0; 4096];
    let mut first: Vec<u8> = alloc::vec![0; 4096];
    for i in 0..clear {
        suffix[i] = i as u8;
        first[i] = i as u8;
    }
    let mut size = min_code + 1;
    let mut next = end + 1;
    let mut prev: Option<usize> = None;
    let mut out: Vec<u8> = Vec::with_capacity(want);
    let mut bits = 0u32;
    let mut nbits = 0usize;
    let mut stack: Vec<u8> = Vec::new();
    for byte in data {
        bits |= u32::from(*byte) << nbits;
        nbits += 8;
        while nbits >= size {
            let code = (bits & ((1 << size) - 1)) as usize;
            bits >>= size;
            nbits -= size;
            if code == clear {
                size = min_code + 1;
                next = end + 1;
                prev = None;
                continue;
            }
            if code == end {
                return Some(out);
            }
            let Some(p) = prev else {
                if code >= clear {
                    return Some(out);
                }
                out.push(code as u8);
                prev = Some(code);
                continue;
            };
            let known = code < next;
            let walk = if known { code } else { p };
            stack.clear();
            let mut c = walk;
            while c >= clear + 2 {
                stack.push(*suffix.get(c)?);
                c = usize::from(*prefix.get(c)?);
            }
            stack.push(c as u8);
            let head = *stack.last()?;
            out.extend(stack.iter().rev());
            if !known {
                out.push(head);
            }
            if next < 4096 {
                prefix[next] = p as u16;
                suffix[next] = if known { *first.get(code)? } else { head };
                first[next] = *first.get(p)?;
                next += 1;
                if next == (1 << size) && size < 12 {
                    size += 1;
                }
            }
            prev = Some(code);
            if out.len() >= want {
                return Some(out);
            }
        }
    }
    Some(out)
}

// ---- BMP ----------------------------------------------------------------------------------

fn bmp(b: &[u8]) -> Result<Picture, String> {
    let bad = || err("The BMP could not be read");
    let le32 = |at: usize| -> i64 {
        let mut v = 0u32;
        for i in 0..4 {
            v |= u32::from(b.get(at + i).copied().unwrap_or(0)) << (8 * i);
        }
        i64::from(v as i32)
    };
    let off = le32(10) as usize;
    let w = le32(18);
    let hh = le32(22);
    let bpp = le16(b, 28);
    if w <= 0 || hh == 0 || !(bpp == 24 || bpp == 32) || w * hh.abs() > (1 << 26) {
        return Err(bad());
    }
    let (w, h) = (w as usize, hh.unsigned_abs() as usize);
    let bytes = bpp / 8;
    let stride = (w * bytes).div_ceil(4) * 4;
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let src_y = if hh > 0 { h - 1 - y } else { y };
        for x in 0..w {
            let i = off + src_y * stride + x * bytes;
            let px = b.get(i..i + bytes).ok_or_else(bad)?;
            let a = if bytes == 4 {
                px.get(3).copied().unwrap_or(255)
            } else {
                255
            };
            out.extend_from_slice(&[px[2], px[1], px[0], a]);
        }
    }
    Ok(Picture::still(w as u32, h as u32, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x1 PNG: one red pixel, one half-transparent green one.
    fn tiny_png() -> Vec<u8> {
        let mut raw = alloc::vec![0u8];
        raw.extend_from_slice(&[255, 0, 0, 255, 0, 255, 0, 128]);
        // Stored (uncompressed) deflate block inside a zlib stream.
        let mut z = alloc::vec![0x78, 0x01, 1];
        let len = raw.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(&raw);
        let mut a: u32 = 1;
        let mut bsum: u32 = 0;
        for x in &raw {
            a = (a + u32::from(*x)) % 65521;
            bsum = (bsum + a) % 65521;
        }
        z.extend_from_slice(&((bsum << 16) | a).to_be_bytes());
        let mut png = alloc::vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10];
        let chunk = |png: &mut Vec<u8>, t: &[u8], d: &[u8]| {
            png.extend_from_slice(&(d.len() as u32).to_be_bytes());
            let mut c = Vec::from(t);
            c.extend_from_slice(d);
            png.extend_from_slice(&c);
            png.extend_from_slice(&crc(&c).to_be_bytes());
        };
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(&mut png, b"IHDR", &ihdr);
        chunk(&mut png, b"IDAT", &z);
        chunk(&mut png, b"IEND", &[]);
        png
    }

    fn crc(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for b in data {
            c ^= u32::from(*b);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }

    #[test]
    fn png_decodes_to_rgba() {
        let p = decode(&tiny_png()).unwrap();
        assert_eq!((p.width, p.height), (2, 1));
        assert_eq!(p.rgba, [255, 0, 0, 255, 0, 255, 0, 128]);
    }

    #[test]
    fn gif_first_frame_decodes() {
        // A 2x2 GIF with a 4-colour table: red, green, blue, white.
        let gif: [u8; 49] = [
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x02, 0x00, 0x02, 0x00, 0x81, 0x00, 0x00, 0xFF,
            0x00, 0x00, 0x00, 0xFF, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0x2C, 0x00, 0x00,
            0x00, 0x00, 0x02, 0x00, 0x02, 0x00, 0x00, 0x02, 0x03, 0x44, 0x34, 0x05, 0x00, 0x3B, 0,
            0, 0, 0, 0, 0, 0,
        ];
        let p = decode(&gif).unwrap();
        assert_eq!((p.width, p.height), (2, 2));
        let px: Vec<&[u8]> = p.rgba.chunks(4).collect();
        assert_eq!(px[0], [255, 0, 0, 255]);
        assert_eq!(px[1], [0, 255, 0, 255]);
        assert_eq!(px[2], [0, 0, 255, 255]);
        assert_eq!(px[3], [255, 255, 255, 255]);
    }

    #[test]
    fn animated_gifs_keep_every_frame() {
        let p = decode(include_bytes!("webp/fixtures/anim.gif")).unwrap();
        assert_eq!((p.width, p.height, p.anim.len()), (6, 4, 3));
        let delays: Vec<u32> = p.anim.iter().map(|f| f.1).collect();
        assert_eq!(delays, [50, 120, 100], "a 0 ms frame shows for 100 ms");
        assert_eq!(&p.anim[1].0[..4], &[0, 0, 255, 255]);
        assert_eq!(p.rgba, p.anim[0].0);
    }

    #[test]
    fn bmp_and_unknown_formats() {
        // A 1x1 24-bit BMP, blue.
        let mut b = alloc::vec![0u8; 58];
        b[0] = b'B';
        b[1] = b'M';
        b[10] = 54;
        b[14] = 40;
        b[18] = 1;
        b[22] = 1;
        b[28] = 24;
        b[54] = 255;
        let p = decode(&b).unwrap();
        assert_eq!(p.rgba, [0, 0, 255, 255]);
        let mut webp = Vec::from(&b"RIFF\0\0\0\0WEBPVP8 "[..]);
        webp.extend_from_slice(&[0; 8]);
        assert!(decode(&webp).is_err());
        let s = decode(br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2"><rect width="2" height="2" fill="red"/></svg>"#).unwrap();
        assert_eq!((s.width, s.height), (8, 4), "drawn at twice its size");
        assert_eq!(s.logical, (4, 2), "shown at its own");
        assert_eq!(&s.rgba[..4], &[255, 0, 0, 255]);
        assert_eq!(s.rgba[7 * 4 + 3], 0, "the right half is empty");
        assert!(decode(b"hello").is_err());
        assert!(decode(b"\x89PNG broken").is_err());
    }

    #[test]
    fn big_pictures_are_scaled_down() {
        let p = Picture::still(4100, 10, alloc::vec![200; 4100 * 10 * 4]);
        let s = shrink(p);
        assert!(s.width <= MAX_SIDE && s.height >= 1);
        assert_eq!(s.rgba.len(), (s.width * s.height * 4) as usize);
        assert_eq!(s.rgba[0], 200);
    }
}

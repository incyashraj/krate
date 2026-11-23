//! PDF pages as pictures (krate:gfx/pdf, K-1342).
//!
//! The app hands over a PDF's bytes and asks for pages; hayro lays each
//! page out on the CPU, the same on every system, and the pixels go back to
//! be drawn like any picture. Bounded: a run keeps at most
//! [`MAX_OPEN`] PDFs of at most [`MAX_BYTES`] each, and a page is never
//! drawn bigger than [`MAX_EDGE`] pixels on its longest side.

use std::collections::BTreeMap;

/// How many PDFs one run keeps open.
pub(crate) const MAX_OPEN: usize = 16;
/// The largest PDF one `open` takes.
pub(crate) const MAX_BYTES: usize = 256 * 1024 * 1024;
/// The longest side of a rendered page, in pixels.
pub(crate) const MAX_EDGE: f32 = 8192.0;

/// One word on a page and its box, in points from the page's top-left
/// corner (the same space as `page_size`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TextSpan {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// The glyphs a page shows, as (text, x, baseline y, em size) in points
/// from the top-left: a device that collects text instead of drawing.
struct GlyphCollector(Vec<(String, f64, f64, f64)>);

impl<'a> hayro::hayro_interpret::Device<'a> for GlyphCollector {
    fn draw_path(
        &mut self,
        _: &hayro::kurbo::BezPath,
        _: hayro::hayro_interpret::DrawProps<'a>,
        _: &hayro::hayro_interpret::DrawMode,
    ) {
    }
    fn push_clip_path(&mut self, _: &hayro::hayro_interpret::ClipPath) {}
    fn push_transparency_group(
        &mut self,
        _: f32,
        _: Option<hayro::hayro_interpret::SoftMask<'a>>,
        _: hayro::hayro_interpret::BlendMode,
    ) {
    }
    fn draw_glyph_run(
        &mut self,
        run: &hayro::hayro_interpret::font::GlyphRun<'_, 'a>,
        props: hayro::hayro_interpret::DrawProps<'a>,
        _: &hayro::hayro_interpret::DrawMode,
    ) {
        use hayro::hayro_interpret::hayro_cmap::BfString;
        for glyph in run.glyphs() {
            let Some(text) = glyph.as_unicode() else {
                continue;
            };
            let text = match text {
                BfString::Char(c) => c.to_string(),
                BfString::String(s) => s,
            };
            let t = props.transform * glyph.transform();
            let at = t * hayro::kurbo::Point::ZERO;
            let [_, _, c, d, _, _] = t.as_coeffs();
            let size = (c * c + d * d).sqrt() * 1000.0; // glyph space is 1000 units an em
            self.0.push((text, at.x, at.y, size));
        }
    }
    fn draw_image(
        &mut self,
        _: hayro::hayro_interpret::Image<'a, '_>,
        _: hayro::hayro_interpret::ImageDrawProps<'a>,
    ) {
    }
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

/// Glyphs in reading order joined into words: a new word at a space, at a
/// new line (the baseline moved by half a size), or at a gap wider than a
/// third of the size.
fn words(glyphs: &[(String, f64, f64, f64)]) -> Vec<TextSpan> {
    let mut out: Vec<TextSpan> = Vec::new();
    let mut word: Vec<(String, f64, f64, f64)> = Vec::new();
    let flush =
        |word: &mut Vec<(String, f64, f64, f64)>, out: &mut Vec<TextSpan>, end: Option<f64>| {
            if let (Some(first), Some(last)) = (word.first(), word.last()) {
                let size = word.iter().map(|g| g.3).fold(0.0, f64::max);
                let right = end.unwrap_or(last.1 + last.3 * 0.5).max(last.1);
                out.push(TextSpan {
                    text: word.iter().map(|g| g.0.as_str()).collect(),
                    x: first.1 as f32,
                    y: (first.2 - size * 0.8) as f32,
                    width: (right - first.1).max(0.0) as f32,
                    height: size as f32,
                });
            }
            word.clear();
        };
    for (i, g) in glyphs.iter().enumerate() {
        let next = glyphs.get(i + 1);
        if g.0.trim().is_empty() {
            flush(&mut word, &mut out, Some(g.1));
            continue;
        }
        if let Some(prev) = word.last() {
            let new_line = (g.2 - prev.2).abs() > prev.3.max(g.3) * 0.5;
            let gap = g.1 - prev.1 > prev.3.max(g.3) * 1.2;
            if new_line || gap || g.1 < prev.1 {
                flush(&mut word, &mut out, None);
            }
        }
        word.push(g.clone());
        // The word's right edge: where the next glyph on this line starts.
        if next.is_none_or(|n| (n.2 - g.2).abs() > g.3 * 0.5 || n.1 < g.1) {
            flush(&mut word, &mut out, None);
        }
    }
    flush(&mut word, &mut out, None);
    out
}

/// One rendered page, straight-alpha RGBA.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PagePixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The PDFs a run has open, by handle.
#[derive(Default)]
pub(crate) struct PdfDocs {
    docs: BTreeMap<u64, hayro::hayro_syntax::Pdf>,
    next: u64,
}

impl PdfDocs {
    pub(crate) fn open(&mut self, bytes: Vec<u8>) -> Result<u64, String> {
        if bytes.len() > MAX_BYTES {
            return Err(format!(
                "a PDF over {} MB is too big to open",
                MAX_BYTES >> 20
            ));
        }
        if self.docs.len() >= MAX_OPEN {
            return Err(format!("{MAX_OPEN} PDFs are open already; close one first"));
        }
        let pdf = hayro::hayro_syntax::Pdf::new(std::sync::Arc::new(bytes))
            .map_err(|e| format!("this is not a PDF Krate can read ({e:?})"))?;
        self.next += 1;
        self.docs.insert(self.next, pdf);
        Ok(self.next)
    }

    fn doc(&self, doc: u64) -> Result<&hayro::hayro_syntax::Pdf, String> {
        self.docs
            .get(&doc)
            .ok_or_else(|| "no such PDF is open".to_string())
    }

    pub(crate) fn page_count(&self, doc: u64) -> Result<u32, String> {
        Ok(self.doc(doc)?.pages().len() as u32)
    }

    pub(crate) fn page_size(&self, doc: u64, page: u32) -> Result<(f32, f32), String> {
        let pdf = self.doc(doc)?;
        let pages = pdf.pages();
        let page = pages
            .get(page as usize)
            .ok_or_else(|| format!("there is no page {page}"))?;
        Ok(page.render_dimensions())
    }

    pub(crate) fn render(&self, doc: u64, page: u32, scale: f32) -> Result<PagePixels, String> {
        use hayro::vello_cpu::color::palette::css::WHITE;
        let pdf = self.doc(doc)?;
        let pages = pdf.pages();
        let page = pages
            .get(page as usize)
            .ok_or_else(|| format!("there is no page {page}"))?;
        let (w, h) = page.render_dimensions();
        if !(w > 0.0 && h > 0.0) {
            return Err("the page has no size".to_string());
        }
        let wanted = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let scale = wanted.min(MAX_EDGE / w.max(h));
        let settings = hayro::PixmapSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
        };
        let cache = hayro::RenderCache::new();
        let pixmap = hayro::render(
            page,
            &cache,
            &hayro::hayro_interpret::InterpreterSettings::default(),
            &hayro::RenderSettings::default(),
            &settings,
        );
        let (width, height) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
        let rgba = pixmap.take_rgba8(hayro::vello_cpu::peniko::ImageAlphaType::Alpha);
        Ok(PagePixels {
            width,
            height,
            rgba,
        })
    }

    /// The words on a page, in reading order, with their boxes in points
    /// from the top-left: what an app needs to select, copy and search a
    /// PDF's text. Empty for a scanned page (pictures of text).
    pub(crate) fn page_text(&self, doc: u64, page: u32) -> Result<Vec<TextSpan>, String> {
        use hayro::hayro_interpret::util::TransformExt;
        let pdf = self.doc(doc)?;
        let pages = pdf.pages();
        let page = pages
            .get(page as usize)
            .ok_or_else(|| format!("there is no page {page}"))?;
        let transform = page.initial_transform(true).to_kurbo();
        let (w, h) = page.render_dimensions();
        let cache = hayro::hayro_interpret::InterpreterCache::new();
        let mut ctx = hayro::hayro_interpret::Context::new(
            transform,
            hayro::kurbo::Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
            &cache,
            page.xref(),
            hayro::hayro_interpret::InterpreterSettings::default(),
        );
        let mut collector = GlyphCollector(Vec::new());
        hayro::hayro_interpret::interpret_page(page, &mut ctx, &mut collector);
        Ok(words(&collector.0))
    }

    pub(crate) fn close(&mut self, doc: u64) {
        self.docs.remove(&doc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page PDF with a black square on it, written by hand: the
    /// smallest file that proves a page is laid out, not just parsed.
    fn square_pdf() -> Vec<u8> {
        let content = b"0 0 0 rg 50 50 100 100 re f";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Contents 4 0 R >>".to_string(),
            format!(
                "<< /Length {} >>\nstream\n{}\nendstream",
                content.len(),
                String::from_utf8_lossy(content)
            ),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    /// A one-page PDF with two lines in Helvetica 12 pt: "Hello World" at
    /// (72, 700) and "Second line" at (72, 680), from the bottom-left.
    fn text_pdf() -> Vec<u8> {
        let content = "BT /F1 12 Tf 72 700 Td (Hello World) Tj 0 -20 Td (Second line) Tj ET";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    /// A PDF's words come back in reading order with boxes where the page
    /// shows them: what selecting and searching need.
    #[test]
    fn a_pages_words_come_back_with_their_boxes() {
        let mut docs = PdfDocs::default();
        let doc = docs.open(text_pdf()).expect("a PDF");
        let words = docs.page_text(doc, 0).expect("text");
        let text: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(text, ["Hello", "World", "Second", "line"], "{words:?}");
        let hello = &words[0];
        assert!((hello.x - 72.0).abs() < 0.5, "{hello:?}");
        assert!((hello.height - 12.0).abs() < 0.5, "{hello:?}");
        // Baseline 700 from the bottom of a 792 page is 92 from the top; the
        // box's top sits about an ascent above it.
        assert!(hello.y > 75.0 && hello.y < 92.0, "{hello:?}");
        assert!(hello.width > 20.0 && hello.width < 40.0, "{hello:?}");
        let world = &words[1];
        assert!(world.x > hello.x + hello.width, "{world:?} after {hello:?}");
        let second = &words[2];
        assert!(
            (second.y - hello.y - 20.0).abs() < 0.5,
            "the next line is 20 pt lower: {second:?}"
        );
        // A page of shapes has no words.
        let shapes = docs.open(square_pdf()).expect("a PDF");
        assert!(docs.page_text(shapes, 0).expect("text").is_empty());
    }

    #[test]
    fn a_page_is_drawn_at_the_scale_asked_on_white() {
        let mut docs = PdfDocs::default();
        let doc = docs.open(square_pdf()).expect("a PDF");
        assert_eq!(docs.page_count(doc), Ok(1));
        assert_eq!(docs.page_size(doc, 0), Ok((200.0, 300.0)));
        let page = docs.render(doc, 0, 2.0).expect("drawn");
        assert_eq!((page.width, page.height), (400, 600));
        assert_eq!(page.rgba.len(), 400 * 600 * 4);
        let at = |x: u32, y: u32| {
            let i = ((y * page.width + x) * 4) as usize;
            &page.rgba[i..i + 4]
        };
        assert_eq!(at(5, 5), &[255, 255, 255, 255], "the page is white");
        // The square spans 50..150 points from the bottom-left: pixels
        // 100..300 across and, flipped, 300..500 down.
        assert_eq!(at(200, 400), &[0, 0, 0, 255], "the square is black");
        assert!(docs.render(doc, 1, 1.0).is_err(), "there is no second page");
        docs.close(doc);
        assert!(docs.page_count(doc).is_err());
    }

    #[test]
    fn what_is_not_a_pdf_and_what_is_too_big_are_refused() {
        let mut docs = PdfDocs::default();
        assert!(docs.open(b"hello".to_vec()).is_err());
        let doc = docs.open(square_pdf()).expect("a PDF");
        let huge = docs.render(doc, 0, 1000.0).expect("held to the edge");
        assert!(huge.height <= MAX_EDGE as u32 && huge.width <= MAX_EDGE as u32);
        for _ in 1..MAX_OPEN {
            docs.open(square_pdf()).expect("room");
        }
        assert!(
            docs.open(square_pdf()).is_err(),
            "a run keeps at most MAX_OPEN"
        );
    }
}

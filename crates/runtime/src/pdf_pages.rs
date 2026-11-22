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

//! Printing what an app's window shows (`krate:ui/print`, the capability
//! roadmap's `ui.print`).
//!
//! The host paints the window at print resolution with the same painter a
//! screenshot uses, makes the pages one PDF, and hands it to the system:
//!
//! - macOS shows its own print panel over the PDF (PDFKit's print
//!   operation, scaled to the paper), where the person picks a printer,
//!   copies, or Save as PDF -- or cancels. The answer comes back exactly.
//! - Windows and Linux open the PDF in the system's viewer, to print from
//!   there, and say so: `opened-in-viewer`, never a claim that it printed.
//!
//! Nothing reaches a printer unless the person says so in a dialog the
//! system draws, which is why any app with a window may ask.
//!
//! `KRATE_PRINT_TO=<file.pdf>` writes the finished document there instead
//! of showing anything -- on macOS through the same print operation, as a
//! save job -- so a test can check what would have printed.

/// Most pages one document may hold.
pub const MAX_PAGES: usize = 100;
/// Pixels per logical unit when a window is painted for paper: a 600-wide
/// window becomes 1,500 pixels, about 200 dots per inch across a page.
pub const PRINT_SCALE: f32 = 2.5;

/// One painted page: packed ARGB, as the painter writes it, and the logical
/// size it stands for (the page's size in points).
pub struct Page {
    pub width: u32,
    pub height: u32,
    pub argb: Vec<u32>,
    pub points: (f32, f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Printed,
    Cancelled,
    /// Windows and Linux; macOS always has its print panel.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    OpenedInViewer,
}

/// The pages as a PDF: one image per page, composited onto white (paper has
/// no transparency), Flate-compressed.
pub fn pdf(title: &str, pages: &[Page]) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    fn object(out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]) {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    // 1 catalog, 2 page tree, 3 info, then page / contents / image per page.
    object(&mut out, &mut offsets, b"<< /Type /Catalog /Pages 2 0 R >>");
    let kids: Vec<String> = (0..pages.len())
        .map(|i| format!("{} 0 R", 4 + i * 3))
        .collect();
    object(
        &mut out,
        &mut offsets,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            pages.len()
        )
        .as_bytes(),
    );
    object(
        &mut out,
        &mut offsets,
        format!("<< /Title {} /Producer (Krate) >>", pdf_string(title)).as_bytes(),
    );
    for (i, page) in pages.iter().enumerate() {
        let id = 4 + i * 3;
        let (pw, ph) = page.points;
        object(
            &mut out,
            &mut offsets,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.2} {ph:.2}] \
                 /Contents {} 0 R /Resources << /XObject << /P {} 0 R >> >> >>",
                id + 1,
                id + 2
            )
            .as_bytes(),
        );
        let draw = format!("q {pw:.2} 0 0 {ph:.2} 0 0 cm /P Do Q");
        object(
            &mut out,
            &mut offsets,
            format!("<< /Length {} >>\nstream\n{draw}\nendstream", draw.len()).as_bytes(),
        );
        let mut rgb = Vec::with_capacity(page.argb.len() * 3);
        for px in &page.argb {
            let a = (px >> 24) & 0xFF;
            for shift in [16, 8, 0] {
                let c = (px >> shift) & 0xFF;
                // Over white: c*a + 255*(1-a).
                rgb.push(((c * a + 255 * (255 - a) + 127) / 255) as u8);
            }
        }
        let packed = miniz_oxide::deflate::compress_to_vec_zlib(&rgb, 6);
        let mut image = format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB \
             /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
            page.width,
            page.height,
            packed.len()
        )
        .into_bytes();
        image.extend_from_slice(&packed);
        image.extend_from_slice(b"\nendstream");
        object(&mut out, &mut offsets, &image);
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// A PDF literal string: parentheses and backslashes escaped, non-ASCII
/// and control characters dropped, so a title can never end the string.
fn pdf_string(text: &str) -> String {
    let mut out = String::from("(");
    for c in text.chars().take(200) {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            ' '..='~' => out.push(c),
            _ => {}
        }
    }
    out.push(')');
    out
}

/// Hand the document to the system and wait for the person.
pub fn print(title: &str, document: &[u8]) -> Result<Outcome, String> {
    if let Some(path) = std::env::var_os("KRATE_PRINT_TO") {
        return to_file(title, document, std::path::Path::new(&path));
    }
    platform::print(title, document)
}

#[cfg(target_os = "macos")]
fn to_file(title: &str, document: &[u8], path: &std::path::Path) -> Result<Outcome, String> {
    platform::save(title, document, path).map(|_| Outcome::Printed)
}

#[cfg(not(target_os = "macos"))]
fn to_file(_title: &str, document: &[u8], path: &std::path::Path) -> Result<Outcome, String> {
    std::fs::write(path, document).map_err(|err| err.to_string())?;
    Ok(Outcome::OpenedInViewer)
}

#[cfg(target_os = "macos")]
mod platform {
    use super::Outcome;
    use objc2::msg_send;
    use objc2::rc::{Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::{NSData, NSString, NSURL};

    #[link(name = "PDFKit", kind = "framework")]
    extern "C" {}

    /// `kPDFPrintPageScaleToFit`.
    const SCALE_TO_FIT: isize = 1;

    fn class(name: &core::ffi::CStr) -> Result<&'static AnyClass, String> {
        AnyClass::get(name).ok_or_else(|| format!("this Mac has no {}", name.to_string_lossy()))
    }

    /// PDFKit's print operation over the document, with the job's title.
    fn operation(
        title: &str,
        document: &[u8],
        configure: impl FnOnce(&AnyObject),
    ) -> Result<Retained<AnyObject>, String> {
        unsafe {
            let data = NSData::with_bytes(document);
            let allocated: Allocated<AnyObject> = msg_send![class(c"PDFDocument")?, alloc];
            let doc: Option<Retained<AnyObject>> = msg_send![allocated, initWithData: &*data];
            let doc = doc.ok_or("the pages could not be made into a document")?;
            let shared: Retained<AnyObject> = msg_send![class(c"NSPrintInfo")?, sharedPrintInfo];
            let info: Retained<AnyObject> = msg_send![&*shared, copy];
            configure(&info);
            let op: Option<Retained<AnyObject>> = msg_send![
                &*doc,
                printOperationForPrintInfo: &*info,
                scalingMode: SCALE_TO_FIT,
                autoRotate: true
            ];
            let op = op.ok_or("the system would not print this document")?;
            let job = NSString::from_str(title);
            let _: () = msg_send![&*op, setJobTitle: &*job];
            Ok(op)
        }
    }

    pub(super) fn print(title: &str, document: &[u8]) -> Result<Outcome, String> {
        let op = operation(title, document, |_| {})?;
        // Modal: returns when the person prints or cancels.
        let printed: bool = unsafe {
            let _: () = msg_send![&*op, setShowsPrintPanel: true];
            msg_send![&*op, runOperation]
        };
        Ok(if printed {
            Outcome::Printed
        } else {
            Outcome::Cancelled
        })
    }

    /// The same operation as a save job to `path`, with no panel.
    pub(super) fn save(title: &str, document: &[u8], path: &std::path::Path) -> Result<(), String> {
        let target = path.to_string_lossy().to_string();
        let op = operation(title, document, |info| unsafe {
            let dict: Retained<AnyObject> = msg_send![info, dictionary];
            let url = NSURL::fileURLWithPath(&NSString::from_str(&target));
            let key = NSString::from_str("NSJobSavingURL");
            let _: () = msg_send![&*dict, setObject: &*url, forKey: &*key];
            let disposition = NSString::from_str("NSPrintSaveJob");
            let _: () = msg_send![info, setJobDisposition: &*disposition];
        })?;
        let done: bool = unsafe {
            let _: () = msg_send![&*op, setShowsPrintPanel: false];
            let _: () = msg_send![&*op, setShowsProgressPanel: false];
            msg_send![&*op, runOperation]
        };
        if done && path.is_file() {
            Ok(())
        } else {
            Err(format!("the print job did not write {}", path.display()))
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::Outcome;

    /// Write the document where the person's viewer can open it, and open
    /// it. The file is the app's own print job and nothing else; it is left
    /// for the viewer, which may still be reading it when this returns.
    pub(super) fn print(title: &str, document: &[u8]) -> Result<Outcome, String> {
        let safe: String = title
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .take(60)
            .collect();
        let path = std::env::temp_dir().join(format!(
            "krate-print-{}-{}.pdf",
            if safe.trim_matches('-').is_empty() {
                "document"
            } else {
                safe.trim_matches('-')
            },
            std::process::id()
        ));
        std::fs::write(&path, document).map_err(|err| err.to_string())?;
        open(&path)?;
        Ok(Outcome::OpenedInViewer)
    }

    #[cfg(target_os = "windows")]
    fn open(path: &std::path::Path) -> Result<(), String> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
        let verb = wide(std::ffi::OsStr::new("open"));
        let file = wide(path.as_os_str());
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        // ShellExecute reports success as a value above 32.
        if result as usize > 32 {
            Ok(())
        } else {
            Err("no program on this computer opens PDF files".to_string())
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn open(path: &std::path::Path) -> Result<(), String> {
        std::process::Command::new("xdg-open")
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|err| format!("could not open the document to print it ({err})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(w: u32, h: u32, argb: u32) -> Page {
        Page {
            width: w,
            height: h,
            argb: vec![argb; (w * h) as usize],
            points: (w as f32 / PRINT_SCALE, h as f32 / PRINT_SCALE),
        }
    }

    #[test]
    fn the_document_has_one_page_per_page_and_a_readable_structure() {
        let doc = pdf(
            "Rate card (v2)",
            &[page(10, 20, 0xFF00_00FF), page(10, 20, 0)],
        );
        let text = String::from_utf8_lossy(&doc);
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.contains("/Count 2"));
        assert_eq!(text.matches("/Type /Page ").count(), 2);
        assert!(
            text.contains("/Title (Rate card \\(v2\\))"),
            "the title is escaped"
        );
        assert!(text.trim_end().ends_with("%%EOF"));
        // Every xref offset points at the object it names.
        let xref = text.rfind("xref\n").expect("xref");
        for (n, line) in text[xref..].lines().skip(3).take(9).enumerate() {
            let offset: usize = line[..10].parse().expect("offset");
            assert!(
                text[offset..].starts_with(&format!("{} 0 obj", n + 1)),
                "object {} is where the table says",
                n + 1
            );
        }
    }

    #[test]
    fn transparency_prints_as_paper_white() {
        let doc = pdf("t", &[page(1, 1, 0x0000_0000)]);
        let image = doc
            .windows(15)
            .position(|w| w == b"/Subtype /Image")
            .expect("the image object");
        let rest = &doc[image..];
        let open = rest
            .windows(7)
            .position(|w| w == b"stream\n")
            .expect("its stream");
        let body = &rest[open + 7..];
        let end = body
            .windows(10)
            .position(|w| w == b"\nendstream")
            .expect("end");
        let rgb = miniz_oxide::inflate::decompress_to_vec_zlib(&body[..end]).expect("inflate");
        assert_eq!(rgb, vec![255, 255, 255]);
    }

    #[test]
    fn a_title_cannot_break_out_of_its_string() {
        assert_eq!(pdf_string("a) /Evil (b\\"), "(a\\) /Evil \\(b\\\\)");
        assert_eq!(pdf_string("é\u{1}ok"), "(ok)");
    }
}

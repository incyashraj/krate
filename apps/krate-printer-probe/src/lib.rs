// The smallest app that prints to a printer the person picks (krate:print,
// IC-986), so the capability is proved through the binary a person runs.
//
//   krate run printer-probe.krate -- <check>
//
// Each check prints one line, so a test sees exactly what an app is told:
//
//   label       the remembered printer, or a pick; then a ZPL label, twice,
//               byte for byte (it carries a NUL and a 0xFF to prove it)
//   document    the same printer; then a one-page PDF, three copies, on a
//               100 x 150 mm label
//   remembered  the printer remembered for this app, never asking
//   pick        ask the person, always
//   forge       print to handles nobody gave
//   big         a label one byte past the 16 MiB bound
#![no_std]
extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use krate::{
    io::{args, stdio, streams::OutputStreamExt},
    print::{self, PaperMm, PaperSize, PrintError, PrintOutcome, Printer},
    Guest,
};

/// A ZPL label with two bytes no text path would carry unchanged.
const ZPL: &[u8] = b"^XA\r\n^FO50,50^ADN,36,20^FDWO-1042\x00\xff^FS\r\n^XZ\r\n";

/// The smallest PDF a printer accepts: one empty page.
const PDF: &[u8] = b"%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 283 425]>>endobj\n\
trailer<</Root 1 0 R>>\n%%EOF\n";

struct Component;

fn refusal(err: &PrintError) -> String {
    let tag = match err {
        PrintError::NotAllowed(_) => "not-allowed",
        PrintError::Unavailable(_) => "unavailable",
        PrintError::UnknownHandle => "unknown-handle",
        PrintError::BadJob(_) => "bad-job",
    };
    format!("{tag}: {}", print::words(err))
}

fn outcome(outcome: &PrintOutcome) -> String {
    match outcome {
        PrintOutcome::Queued(job) => format!("queued {job}"),
        PrintOutcome::Failed(words) => format!("failed: {words}"),
        PrintOutcome::Gone(words) => format!("gone: {words}"),
    }
}

fn shown(found: Result<Option<Printer>, PrintError>) -> String {
    match found {
        Ok(Some(printer)) => String::from(printer.name()),
        Ok(None) => String::from("none"),
        Err(err) => refusal(&err),
    }
}

fn with_printer(check: &str, job: impl Fn(&Printer) -> Result<PrintOutcome, PrintError>) -> String {
    match print::printer("Choose the label printer") {
        Ok(Some(printer)) => match job(&printer) {
            Ok(done) => format!("{check}: {}: {}", printer.name(), outcome(&done)),
            Err(err) => format!("{check}: {}: {}", printer.name(), refusal(&err)),
        },
        Ok(None) => format!("{check}: none"),
        Err(err) => format!("{check}: {}", refusal(&err)),
    }
}

impl Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let check = raw.split('\n').find(|w| !w.is_empty()).unwrap_or("label");
        let line = match check {
            "label" => with_printer("label", |p| p.raw(ZPL, 2, "WO-1042")),
            "document" => with_printer("document", |p| {
                p.pdf(
                    PDF,
                    3,
                    PaperSize::Custom(PaperMm {
                        width: 100,
                        height: 150,
                    }),
                    "Traveller WO-1042",
                )
            }),
            "remembered" => format!("remembered: {}", shown(print::remembered())),
            "pick" => format!("pick: {}", shown(print::pick("Choose the label printer"))),
            "forge" => {
                use krate::bindings::krate::print::printers::print_raw;
                let tag = |handle: u64| match print_raw(handle, ZPL, 1, "forged") {
                    Ok(done) => outcome(&done),
                    Err(err) => refusal(&err),
                };
                format!("forge: zero={} next={}", tag(0), tag(1))
            }
            "big" => with_printer("big", |p| {
                p.raw(&vec![b'x'; 16 * 1024 * 1024 + 1], 1, "big")
            }),
            other => format!("unknown check {other}"),
        };
        let out = stdio::stdout();
        let _ = out.write_line(&line);
        let _ = out.flush();
        0
    }
}

krate::export!(Component);

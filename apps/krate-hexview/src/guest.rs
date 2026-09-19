use crate::bindings::krate::{
    fs::{files, types::OpenMode},
    io::{stdio, streams},
};
use crate::{
    guest_io::{self, Read, Write},
    hexyl::*,
    options,
};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
pub struct Sink {
    stream: streams::OutputStream,
    buffer: Vec<u8>,
}
impl Sink {
    pub fn new() -> Self {
        Self {
            stream: stdio::stdout(),
            buffer: Vec::with_capacity(16384),
        }
    }
}
impl Write for Sink {
    fn write_all(&mut self, b: &[u8]) -> guest_io::Result<()> {
        for part in b.chunks(16384) {
            if self.buffer.len() + part.len() > 16384 {
                self.flush()?
            }
            self.buffer.extend_from_slice(part);
        }
        Ok(())
    }
    fn flush(&mut self) -> guest_io::Result<()> {
        if !self.buffer.is_empty() {
            self.stream
                .write_all(&self.buffer)
                .map_err(|e| format!("{e:?}"))?;
            self.buffer.clear()
        }
        self.stream.flush().map_err(|e| format!("{e:?}"))
    }
}
pub enum Input {
    File(files::File),
    Stdin(streams::InputStream),
}
impl Read for Input {
    fn read(&mut self, out: &mut [u8]) -> guest_io::Result<usize> {
        let n = out.len().min(65536) as u32;
        let data = match self {
            Self::File(f) => f.read(n).map_err(|e| format!("{e:?}"))?,
            Self::Stdin(s) => s.read(n).map_err(|e| format!("{e:?}"))?,
        };
        out[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }
}
struct Limited {
    input: Input,
    left: u64,
}
impl Read for Limited {
    fn read(&mut self, out: &mut [u8]) -> guest_io::Result<usize> {
        let n = (out.len() as u64).min(self.left) as usize;
        if n == 0 {
            return Ok(0);
        }
        let n = self.input.read(&mut out[..n])?;
        self.left -= n as u64;
        Ok(n)
    }
}
const HELP:&str="Krate Hexview 0.1 (experimental hexyl 0.17 renderer adaptation)\n\nOpen the bundle normally for the native file viewer.\nTerminal: krate run [grants] Hexview.krate -- --dump [options] [file|-]\n\n-n/-c/-l/--length N  -s/--skip N  -o/--display-offset N\n--block-size N  --panels N (1..16)  --terminal-width N  -g/--group-size 1|2|4|8\n--endianness big|little (-e)  -b/--base binary|octal|decimal|hexadecimal\n--color always|force|never  --color-scheme default|gradient\n--border unicode|ascii|none  --character-table default|ascii|braille\n-v/--no-squeezing  -P/--no-position  --no-characters  -C/--characters\n-p/--plain  -i/--include  -h/--help  -V/--version\n\nCounts accept bytes, 0xHEX, kB/MB/GB/TB, KiB/MiB/GiB/TiB and block units.\nNegative skip seeks from the end of a file; stdin supports forward skip.\nNo terminal-size/env-color detection, shell completions or CP437/1047 tables.\nPath access is deliberately restricted to the granted input/ directory.\nGUI Open uses a chosen-file token, not ambient directory permission.\n";
pub fn dump(args: &[&str]) -> i32 {
    match run_dump(args) {
        Ok(()) => 0,
        Err(e) => {
            let _ = stdio::stderr().write_all(format!("Hexview: {e}\n").as_bytes());
            1
        }
    }
}
fn run_dump(args: &[&str]) -> Result<(), String> {
    let o = options::parse(args)?;
    if o.help {
        return stdio::stdout()
            .write_all(HELP.as_bytes())
            .map_err(|e| format!("{e:?}"));
    }
    if o.version {
        return stdio::stdout()
            .write_all(b"Krate Hexview 0.1; hexyl renderer 0.17.0\n")
            .map_err(|e| format!("{e:?}"));
    }
    let mut input = match o.path.as_deref() {
        Some(p) if p != "-" => Input::File(
            files::open(p, OpenMode::Read).map_err(|e| format!("Cannot open {p}: {e:?}"))?,
        ),
        _ => Input::Stdin(stdio::stdin()),
    };
    let skipped = match &mut input {
        Input::File(f) => {
            let pos = if o.skip < 0 {
                let size = f.stat().map_err(|e| format!("{e:?}"))?.size;
                size.checked_sub(o.skip.unsigned_abs())
                    .ok_or("Skip is before the file start")?
            } else {
                o.skip as u64
            };
            f.seek_set(pos).map_err(|e| format!("{e:?}"))?
        }
        Input::Stdin(_) => {
            if o.skip < 0 {
                return Err("Cannot seek backward on stdin".into());
            }
            let mut remaining = o.skip as u64;
            let mut scratch = [0; 8192];
            let mut done = 0;
            while remaining > 0 {
                let n = (remaining as usize).min(scratch.len());
                let got = input.read(&mut scratch[..n])?;
                if got == 0 {
                    break;
                }
                remaining -= got as u64;
                done += got as u64;
            }
            done
        }
    };
    let offset = skipped
        .checked_add(o.offset)
        .ok_or("Display offset overflow")?;
    let include = if o.include {
        match o.path.as_deref() {
            None => IncludeMode::Stdin,
            Some("-") => IncludeMode::File("stdin".into()),
            Some(p) => IncludeMode::File(p.rsplit('/').next().unwrap_or("file").to_string()),
        }
    } else {
        IncludeMode::Off
    };
    let mut sink = Sink::new();
    let mut printer = PrinterBuilder::new(&mut sink)
        .show_color(o.color)
        .show_char_panel(o.chars)
        .show_position_panel(o.position)
        .with_border_style(o.border)
        .enable_squeezing(o.squeeze)
        .num_panels(o.panels)
        .group_size(o.group)
        .with_base(o.base)
        .endianness(o.endian)
        .character_table(o.table)
        .color_scheme(o.scheme)
        .include_mode(include)
        .build();
    printer.display_offset(offset);
    printer.print_all(Limited {
        input,
        left: o.length,
    })?;
    drop(printer);
    sink.flush()
}

//! Native, read-only companion UI. File handles are paged, never read wholesale.
use crate::bindings::krate::{
    fs::{files, types::OpenMode},
    io::stdio,
};
use crate::bindings::krate::{
    gfx::{canvas2d as c, types as g},
    ui::{self, dialog, events, tree, types as u, window},
};
use crate::{
    hexyl::{Byte, ByteCategory},
    options::count,
};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};

const W: f32 = 1100.;
const H: f32 = 760.;
const TOP: f32 = 154.;
const LINE: f32 = 25.;
const ROWS: usize = 20;
const COLS: usize = 16;
const HEX_X: f32 = 124.;
const FONT: f32 = 16.;
const SAMPLE: &[u8] =
    b"Krate Hexview / binary inspection\0\nA portable file, not a platform-specific installer.\n";
enum Source {
    Demo(Vec<u8>),
    File(files::File),
}
struct Search {
    pattern: Vec<u8>,
    cursor: u64,
}
struct Viewer {
    source: Source,
    name: String,
    size: u64,
    offset: u64,
    selected: u64,
    page: Vec<u8>,
    status: String,
    input: String,
    editing: u8,
    search: Option<Search>,
    selection_len: usize,
    advance: f32,
    visible_rows: usize,
    wheel_remainder: f32,
}
impl Viewer {
    fn demo() -> Self {
        let mut bytes = Vec::with_capacity(16384);
        bytes.extend_from_slice(
            b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\x04\0\0\0\x03\0\x08\x06\0\0\0",
        );
        bytes.extend_from_slice(SAMPLE);
        bytes.extend(0u8..=255);
        bytes.resize(512, 0);
        for n in 0..192 {
            bytes.extend_from_slice(
                format!("record {n:04}: Krate makes software portable.\n").as_bytes(),
            );
        }
        let size = bytes.len() as u64;
        Self {
            source: Source::Demo(bytes),
            name: "sample-bytes.bin (built-in fixture)".into(),
            size,
            offset: 0,
            selected: 0,
            page: Vec::new(),
            status: "Read-only. Open a real file to inspect its bytes.".into(),
            input: String::new(),
            editing: 0,
            search: None,
            selection_len: 1,
            advance: 9.6,
            visible_rows: ROWS,
            wheel_remainder: 0.,
        }
    }
    fn load(&mut self, f: files::File, name: String) -> Result<(), String> {
        let st = f.stat().map_err(|e| format!("{e:?}"))?;
        if st.is_dir {
            return Err("Choose a file, not a directory".into());
        }
        self.source = Source::File(f);
        self.name = name;
        self.size = st.size;
        self.offset = 0;
        self.selected = 0;
        self.selection_len = 1;
        self.search = None;
        self.input.clear();
        self.editing = 0;
        self.status = "Read-only file handle. Only the visible page is loaded.".into();
        self.refresh()
    }
    fn read_at(&self, offset: u64, n: usize) -> Result<Vec<u8>, String> {
        let want = (self.size.saturating_sub(offset)).min(n as u64) as usize;
        match &self.source {
            Source::Demo(d) => Ok(d
                [offset.min(d.len() as u64) as usize..offset.min(d.len() as u64) as usize + want]
                .to_vec()),
            Source::File(f) => {
                f.seek_set(offset).map_err(|e| format!("{e:?}"))?;
                let mut out = Vec::with_capacity(want);
                while out.len() < want {
                    let b = f
                        .read((want - out.len()) as u32)
                        .map_err(|e| format!("{e:?}"))?;
                    if b.is_empty() {
                        break;
                    }
                    out.extend_from_slice(&b)
                }
                Ok(out)
            }
        }
    }
    fn refresh(&mut self) -> Result<(), String> {
        self.page = self.read_at(self.offset, self.visible_rows * COLS)?;
        Ok(())
    }
    fn navigate(&mut self, offset: u64) -> Result<(), String> {
        let max = self.size.saturating_sub(1) / 16 * 16;
        self.offset = (offset / 16 * 16).min(max);
        self.refresh()
    }
    fn choose(&mut self, byte: u64) -> Result<(), String> {
        self.selected = byte.min(self.size.saturating_sub(1));
        self.selection_len = 1;
        if self.selected < self.offset
            || self.selected >= self.offset + (self.visible_rows * COLS) as u64
        {
            self.navigate(self.selected)?
        }
        Ok(())
    }
    fn start_search(&mut self) -> Result<(), String> {
        let p = if let Some(hex) = self.input.strip_prefix("hex:") {
            let clean: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
            if clean.len() % 2 != 0 || !clean.is_ascii() {
                return Err("Hex search needs pairs: hex: 89 50 4e 47".into());
            }
            let mut p = Vec::new();
            for b in clean.as_bytes().chunks(2) {
                let t = core::str::from_utf8(b).unwrap();
                p.push(u8::from_str_radix(t, 16).map_err(|_| "Invalid hex digit")?)
            }
            p
        } else {
            self.input.as_bytes().to_vec()
        };
        if p.is_empty() {
            return Err("Type UTF-8 text or hex: 89 50 4e 47".into());
        }
        if p.len() > 128 {
            return Err("Search is limited to 128 bytes".into());
        }
        self.search = Some(Search {
            pattern: p,
            cursor: 0,
        });
        self.editing = 0;
        self.status = "Searching from the start; Esc cancels.".into();
        Ok(())
    }
    fn search_tick(&mut self) -> Result<(), String> {
        let Some(s) = self.search.as_ref() else {
            return Ok(());
        };
        let pos = s.cursor;
        let len = s.pattern.len();
        let data = self.read_at(pos, 32768 + len.saturating_sub(1))?;
        let found = data.windows(len).position(|w| w == s.pattern);
        if let Some(i) = found {
            self.selected = pos + i as u64;
            self.selection_len = len;
            self.navigate(self.selected)?;
            self.search = None;
            self.status = format!("Found {len} bytes at 0x{:08x}.", self.selected);
        } else {
            let next = pos + 32768;
            if next >= self.size {
                self.search = None;
                self.status = "No match in this file.".into()
            } else {
                self.search.as_mut().unwrap().cursor = next;
                self.status = format!(
                    "Searching {:.0}% · Esc cancels",
                    next as f64 / self.size.max(1) as f64 * 100.
                )
            }
        }
        Ok(())
    }
    fn open(&mut self, win: u64) {
        match dialog::open_file(win, "Open a file to inspect", "") {
            Ok(Some(p)) => match files::open_chosen(&p.token, OpenMode::Read) {
                Ok(f) => {
                    if let Err(e) = self.load(f, p.name) {
                        self.status = e
                    }
                }
                Err(e) => self.status = format!("Open failed: {e:?}"),
            },
            Ok(None) => self.status = "Open cancelled; current file is unchanged.".into(),
            Err(e) => self.status = format!("File picker unavailable: {e:?}"),
        }
    }
    fn copy(&mut self) {
        let n = self.selection_len.min(128);
        match self.read_at(self.selected, n) {
            Ok(v) => {
                let text = v
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                self.status = match ui::clipboard::write_text(&text) {
                    Ok(()) => format!("Copied {} byte(s) as hex.", v.len()),
                    Err(e) => format!("Clipboard permission needed: {e:?}"),
                }
            }
            Err(e) => self.status = e,
        }
    }
    fn key(&mut self, key: &str, ctrl: bool) -> Result<(), String> {
        if ctrl {
            match key.to_ascii_lowercase().as_str() {
                "o" => return Ok(()),
                "f" => {
                    self.editing = 1;
                    self.input.clear();
                    return Ok(());
                }
                "g" => {
                    self.editing = 2;
                    self.input.clear();
                    return Ok(());
                }
                _ => {}
            }
        }
        if key == "Escape" {
            self.search = None;
            self.editing = 0;
            self.status = "Ready.".into();
            return Ok(());
        }
        if self.editing != 0 {
            match key {
                "Backspace" => {
                    self.input.pop();
                }
                "Enter" | "Return" => {
                    if self.editing == 1 {
                        self.start_search()?
                    } else {
                        let n = count(&self.input, 512)?;
                        if n < 0 || n as u64 >= self.size {
                            return Err("Offset must be inside the file".into());
                        }
                        self.choose(n as u64)?;
                        self.editing = 0;
                        self.status = format!("Jumped to 0x{n:08x}.");
                    }
                }
                _ => {}
            }
            return Ok(());
        }
        match key {
            "ArrowRight" => self.choose(self.selected.saturating_add(1))?,
            "ArrowLeft" => self.choose(self.selected.saturating_sub(1))?,
            "ArrowDown" => self.choose(self.selected.saturating_add(16))?,
            "ArrowUp" => self.choose(self.selected.saturating_sub(16))?,
            "PageDown" => {
                self.navigate(self.offset + (self.visible_rows * COLS) as u64)?;
                self.selected = self.offset;
            }
            "PageUp" => {
                self.navigate(
                    self.offset
                        .saturating_sub((self.visible_rows * COLS) as u64),
                )?;
                self.selected = self.offset;
            }
            "Home" => {
                self.navigate(0)?;
                self.selected = 0;
            }
            "End" => {
                self.choose(self.size.saturating_sub(1))?;
            }
            _ => {}
        }
        Ok(())
    }
    fn click(&mut self, x: f32, y: f32, win: u64) -> Result<(), String> {
        if y >= 14. && y < 48. {
            if x >= 16. && x < 136. {
                self.open(win)
            } else if x >= 148. && x < 276. {
                let advance = self.advance;
                let visible_rows = self.visible_rows;
                *self = Self::demo();
                self.advance = advance;
                self.visible_rows = visible_rows;
                self.refresh()?
            } else if x >= 290. && x < 456. {
                self.copy()
            }
            return Ok(());
        }
        if y >= 91. && y < 125. {
            if x >= 16. && x < 458. {
                self.editing = 1;
                self.input.clear()
            } else if x >= 470. && x < 640. {
                self.editing = 2;
                self.input.clear()
            } else if x >= 652. && x < 744. {
                self.key("PageUp", false)?
            } else if x >= 756. && x < 848. {
                self.key("PageDown", false)?
            }
            return Ok(());
        }
        if y >= TOP && y < TOP + self.visible_rows as f32 * LINE {
            let row = ((y - TOP) / LINE) as usize;
            let cell = self.advance * 3.;
            let asc_x = HEX_X + cell * 16. + 24.;
            let col = if x >= HEX_X && x < HEX_X + COLS as f32 * cell {
                Some(((x - HEX_X) / cell) as usize)
            } else if x >= asc_x && x < asc_x + 16. * self.advance {
                Some(((x - asc_x) / self.advance) as usize)
            } else {
                None
            };
            if let Some(col) = col {
                let index = row * 16 + col;
                if index < self.page.len() {
                    self.selected = self.offset + index as u64;
                    self.selection_len = 1;
                    self.editing = 0
                }
            }
        }
        Ok(())
    }
}
fn rgb(hex: u32) -> g::Color {
    g::Color {
        r: ((hex >> 16) & 255) as f32 / 255.,
        g: ((hex >> 8) & 255) as f32 / 255.,
        b: (hex & 255) as f32 / 255.,
        a: 1.,
    }
}
fn rect(x: f32, y: f32, width: f32, height: f32) -> g::Rect {
    g::Rect {
        x,
        y,
        width,
        height,
    }
}
fn box_(canvas: u64, x: f32, y: f32, w: f32, h: f32, color: u32) {
    let _ = c::fill_rect(canvas, rect(x, y, w, h), rgb(color));
}
fn text(canvas: u64, s: &str, x: f32, y: f32, size: f32, color: u32, mono: bool) {
    let _ = c::draw_text_styled(
        canvas,
        s,
        g::Point { x, y },
        size,
        rgb(color),
        g::TextStyle {
            weight: 400,
            italic: false,
            letter_spacing: 0.,
            family: if mono {
                g::FontFamily::Mono
            } else {
                g::FontFamily::Sans
            },
        },
    );
}
fn category(b: u8) -> usize {
    match Byte(b).category() {
        ByteCategory::Null => 0,
        ByteCategory::AsciiPrintable => 1,
        ByteCategory::AsciiWhitespace => 2,
        ByteCategory::AsciiOther => 3,
        ByteCategory::NonAscii => 4,
    }
}
const COLORS: [u32; 5] = [0x858b94, 0xe2e4e9, 0x83b8ef, 0xd8af79, 0xb1a1db];
fn button(canvas: u64, label: &str, x: f32, y: f32, width: f32) {
    box_(canvas, x, y, width, 34., 0x393c42);
    box_(canvas, x + 1., y + 1., width - 2., 32., 0x27292e);
    text(canvas, label, x + 11., y + 22., 13., 0xe2e4e9, false);
}
fn mono_style() -> g::TextStyle {
    g::TextStyle {
        weight: 400,
        italic: false,
        letter_spacing: 0.,
        family: g::FontFamily::Mono,
    }
}
fn draw(v: &mut Viewer, canvas: u64) -> Result<(), String> {
    let size = c::canvas_size(canvas).map_err(|e| format!("{e:?}"))?;
    let width = size.width;
    let height = size.height;
    let rows = ROWS.min(((height - TOP - 80.).max(LINE) / LINE) as usize);
    if rows != v.visible_rows {
        v.visible_rows = rows;
        v.refresh()?;
    }
    let cell = v.advance * 3.;
    let asc_x = HEX_X + cell * 16. + 24.;
    let grid_end = asc_x + v.advance * 16. + 20.;
    let side = grid_end + 22.;
    let has_inspector = width >= side + 220.;
    c::clear(canvas, rgb(0x1b1c20)).map_err(|e| format!("{e:?}"))?;
    box_(canvas, 0., 0., width, 59., 0x222428);
    box_(canvas, 0., 58., width, 1., 0x393c42);
    button(canvas, "Open file  ⌘O", 16., 14., 120.);
    button(canvas, "Sample bytes", 148., 14., 128.);
    button(canvas, "Copy bytes  ⌘C", 290., 14., 166.);
    if width > 900. {
        text(canvas, "Hexview", width - 170., 35., 14., 0xe2e4e9, false);
        text(canvas, "Read only", width - 98., 35., 12., 0xa6abb4, false);
    }
    let name: String = v.name.chars().take(75).collect();
    text(canvas, &name, 17., 79., 13., 0xd7dae1, false);
    if width > 900. {
        text(
            canvas,
            &format!("{} bytes", v.size),
            side,
            79.,
            13.,
            0xa6abb4,
            false,
        );
    }
    for (x, w, active) in [(16., 442., v.editing == 1), (470., 170., v.editing == 2)] {
        box_(
            canvas,
            x,
            91.,
            w,
            34.,
            if active { 0x769dc8 } else { 0x3d4047 },
        );
        box_(canvas, x + 1., 92., w - 2., 32., 0x16171b);
    }
    let input: String = v
        .input
        .chars()
        .rev()
        .take(if v.editing == 1 { 49 } else { 16 })
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    text(
        canvas,
        if v.editing == 1 {
            &input
        } else {
            "Find text or hex: 89 50 4e 47    ⌘F"
        },
        27.,
        113.,
        13.,
        0xbac0ca,
        false,
    );
    text(
        canvas,
        if v.editing == 2 {
            &input
        } else {
            "Go to offset  ⌘G"
        },
        481.,
        113.,
        13.,
        0xbac0ca,
        false,
    );
    button(canvas, "Previous", 652., 91., 92.);
    button(canvas, "Next", 756., 91., 92.);
    text(canvas, "Offset", 17., 145., 12., 0xa6abb4, false);
    for col in 0..COLS {
        text(
            canvas,
            &format!("{col:02X}"),
            HEX_X + col as f32 * cell,
            145.,
            FONT,
            0x868d99,
            true,
        );
    }
    text(canvas, "ASCII", asc_x, 145., 12., 0xa6abb4, false);
    box_(canvas, 0., TOP - 1., width, 1., 0x393c42);
    box_(
        canvas,
        HEX_X - 16.,
        TOP,
        1.,
        (height - TOP - 65.).max(0.),
        0x303238,
    );
    box_(
        canvas,
        asc_x - 13.,
        TOP,
        1.,
        (height - TOP - 65.).max(0.),
        0x303238,
    );
    for row in 0..v.visible_rows {
        let index = row * 16;
        let y = TOP + row as f32 * LINE;
        if index >= v.page.len() {
            continue;
        }
        let selected_row =
            v.selected >= v.offset + index as u64 && v.selected < v.offset + index as u64 + 16;
        if selected_row {
            box_(canvas, 0., y, grid_end, LINE, 0x25282e);
        }
        text(
            canvas,
            &format!("{:08x}", v.offset + index as u64),
            17.,
            y + 18.,
            14.,
            0x9097a4,
            true,
        );
        for j in 0..16 {
            let absolute = v.offset + (index + j) as u64;
            if index + j < v.page.len()
                && absolute >= v.selected
                && absolute < v.selected + v.selection_len as u64
            {
                box_(
                    canvas,
                    HEX_X + j as f32 * cell - 2.,
                    y + 2.,
                    v.advance * 2. + 4.,
                    LINE - 4.,
                    0x354d6a,
                );
                box_(
                    canvas,
                    asc_x + j as f32 * v.advance,
                    y + 2.,
                    v.advance,
                    LINE - 4.,
                    0x354d6a,
                );
            }
        }
        // At most ten shaped runs per row. Measure the font; do not assume an advance.
        for cat in 0..5 {
            let mut hex = String::new();
            let mut chars = String::new();
            let mut any = false;
            for j in 0..16 {
                if let Some(&b) = v.page.get(index + j) {
                    if category(b) == cat {
                        hex.push_str(&format!("{b:02x} "));
                        chars.push(if (32..=126).contains(&b) {
                            b as char
                        } else {
                            '.'
                        });
                        any = true;
                    } else {
                        hex.push_str("   ");
                        chars.push(' ');
                    }
                } else {
                    hex.push_str("   ");
                    chars.push(' ');
                }
            }
            if any {
                text(canvas, &hex, HEX_X, y + 18., FONT, COLORS[cat], true);
                text(canvas, &chars, asc_x, y + 18., FONT, COLORS[cat], true);
            }
        }
    }
    if v.page.is_empty() {
        text(
            canvas,
            "This file is empty.",
            HEX_X,
            TOP + 42.,
            15.,
            0xa6abb4,
            false,
        );
    }
    if has_inspector {
        box_(
            canvas,
            side - 16.,
            TOP,
            1.,
            (height - TOP - 65.).max(0.),
            0x393c42,
        );
        text(canvas, "Selection", side, TOP + 26., 14., 0xe2e4e9, false);
        if let Some(&b) = v.page.get(v.selected.saturating_sub(v.offset) as usize) {
            for (i, (label, value)) in [
                ("Offset", format!("0x{:08x}", v.selected)),
                ("Hex", format!("{b:02X}")),
                ("Decimal", format!("{b}")),
                ("Binary", format!("{b:08b}")),
                (
                    "Type",
                    [
                        "Null",
                        "Printable ASCII",
                        "Whitespace",
                        "ASCII control",
                        "Non-ASCII",
                    ][category(b)]
                    .to_string(),
                ),
                ("Length", format!("{} byte(s)", v.selection_len)),
            ]
            .iter()
            .enumerate()
            {
                let y = TOP + 59. + i as f32 * 51.;
                text(canvas, label, side, y, 12., 0x969da9, false);
                text(canvas, value, side, y + 21., 14., 0xe2e4e9, true);
            }
            text(
                canvas,
                "↑ ↓ ← →  Move selection",
                side,
                TOP + 396.,
                12.,
                0xa6abb4,
                false,
            );
            text(
                canvas,
                "Home / End  First / last byte",
                side,
                TOP + 420.,
                12.,
                0xa6abb4,
                false,
            );
            text(
                canvas,
                "Enter  Find / go to offset",
                side,
                TOP + 444.,
                12.,
                0xa6abb4,
                false,
            );
        }
    }
    // Anchor chrome to the window, never shrink the data's type to fit.
    box_(canvas, 0., height - 65., width, 65., 0x222428);
    box_(canvas, 0., height - 65., width, 1., 0x393c42);
    for (i, label) in ["Null", "ASCII", "Whitespace", "Control", "Non-ASCII"]
        .iter()
        .enumerate()
    {
        let x = 17. + i as f32 * 119.;
        box_(canvas, x, height - 48., 5., 5., COLORS[i]);
        text(canvas, label, x + 12., height - 42., 12., COLORS[i], false);
    }
    text(
        canvas,
        &format!("0x{:08x}  ·  {} / {} bytes", v.offset, v.page.len(), v.size),
        17.,
        height - 16.,
        12.,
        0xbac0ca,
        true,
    );
    if width > 860. {
        let status: String = v
            .status
            .chars()
            .take(((width - 450.) / 7.).max(0.) as usize)
            .collect();
        text(canvas, &status, 435., height - 16., 12., 0xa6abb4, false);
    }
    c::present(canvas).map_err(|e| format!("{e:?}"))
}
fn node(id: u64, parent: Option<u64>, kind: u::WidgetKind) -> u::WidgetNode {
    u::WidgetNode {
        id,
        parent,
        kind,
        label: None,
        role: None,
        style: u::Style {
            width: None,
            height: None,
            grow: 1.,
            padding: 0.,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}
fn app(args: &[&str]) -> Result<(), String> {
    let win = window::create(
        "Hexview · Krate",
        u::WindowSize {
            width: W as u32,
            height: H as u32,
        },
    )
    .map_err(|e| format!("{e:?}"))?;
    window::show(win).map_err(|e| format!("{e:?}"))?;
    tree::set_root(win, &node(1, None, u::WidgetKind::Stack)).map_err(|e| format!("{e:?}"))?;
    tree::upsert_node(win, &node(2, Some(1), u::WidgetKind::Canvas))
        .map_err(|e| format!("{e:?}"))?;
    let canvas = c::bind(win, 2).map_err(|e| format!("{e:?}"))?;
    let mut v = Viewer::demo();
    v.advance = c::measure_text_styled(canvas, "0000000000000000", FONT, mono_style())
        .map_err(|e| format!("{e:?}"))?
        .width
        / 16.;
    v.refresh()?;
    if let Some(i) = args.iter().position(|a| *a == "--file") {
        let p = args.get(i + 1).ok_or("Missing --file path")?;
        let f = files::open(p, OpenMode::Read).map_err(|e| format!("{e:?}"))?;
        v.load(f, p.to_string())?
    }
    if let Some(i) = args.iter().position(|a| *a == "--offset") {
        let n = count(args.get(i + 1).ok_or("Missing --offset")?, 512)?;
        if n < 0 || n as u64 >= v.size {
            return Err("Offset outside file".into());
        }
        v.choose(n as u64)?;
    }
    if let Some(i) = args.iter().position(|a| *a == "--find") {
        v.input = args.get(i + 1).ok_or("Missing --find")?.to_string();
        v.start_search()?;
    }
    if let Some(i) = args.iter().position(|a| *a == "--size") {
        let value = args.get(i + 1).ok_or("Missing --size WIDTHxHEIGHT")?;
        let (w, h) = value.split_once('x').ok_or("Expected WIDTHxHEIGHT")?;
        window::set_size(
            win,
            u::WindowSize {
                width: w.parse().map_err(|_| "Invalid width")?,
                height: h.parse().map_err(|_| "Invalid height")?,
            },
        )
        .map_err(|e| format!("{e:?}"))?;
    }
    if args.contains(&"--snapshot") {
        while v.search.is_some() {
            v.search_tick()?
        }
    }
    draw(&mut v, canvas)?;
    if args.contains(&"--probe") {
        let hex = v
            .page
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        stdio::stdout()
            .write_all(
                format!(
                    "size={} offset={} selected={} length={} page={}\n",
                    v.size, v.offset, v.selected, v.selection_len, hex
                )
                .as_bytes(),
            )
            .map_err(|e| format!("{e:?}"))?;
    }
    draw(&mut v, canvas)?;
    if args.contains(&"--self-test") {
        v.click(800., 108., win)?;
        if v.offset != 320 {
            return Err("next page failed".into());
        }
        v.click(HEX_X + v.advance * 3. + 1., TOP + 1., win)?;
        if v.selected != 321 {
            return Err("selection failed".into());
        }
        v.key("End", false)?;
        if v.selected != v.size - 1 {
            return Err("end failed".into());
        }
        v.key("Home", false)?;
        v.input = "hex: 89 50 4e 47".into();
        v.start_search()?;
        v.search_tick()?;
        if v.selected != 0 || v.selection_len != 4 {
            return Err("search failed".into());
        }
        v.editing = 2;
        v.input = "0x100".into();
        v.key("Enter", false)?;
        if v.selected != 256 {
            return Err("goto failed".into());
        }
        window::set_size(
            win,
            u::WindowSize {
                width: 880,
                height: 608,
            },
        )
        .map_err(|e| format!("{e:?}"))?;
        draw(&mut v, canvas)?;
        window::set_size(
            win,
            u::WindowSize {
                width: 1100,
                height: 760,
            },
        )
        .map_err(|e| format!("{e:?}"))?;
        v.key("Home", false)?;
        v.status = "Self-test passed: paging, selection, search, offsets and resize.".into();
        stdio::stdout()
            .write_all(b"hexview-ui:self-test:pass\n")
            .map_err(|e| format!("{e:?}"))?;
    }
    draw(&mut v, canvas)?;
    if args.contains(&"--snapshot") || args.contains(&"--self-test") {
        return Ok(());
    }
    loop {
        let event = events::wait(Some(if v.search.is_some() { 1 } else { 50 }));
        let mut changed = false;
        if let Some(e) = event {
            changed = true;
            let result = match e {
                u::Event::CloseRequested(id) => {
                    let _ = window::close(id);
                    break;
                }
                u::Event::Pointer(p) if p.pressed => v.click(p.x, p.y, win),
                u::Event::Key(k) if k.pressed => {
                    let ctrl = k.modifiers.meta || k.modifiers.control;
                    if ctrl && k.key.eq_ignore_ascii_case("o") {
                        v.open(win);
                        Ok(())
                    } else if ctrl && k.key.eq_ignore_ascii_case("c") {
                        v.copy();
                        Ok(())
                    } else {
                        v.key(&k.key, ctrl)
                    }
                }
                u::Event::TextInput(s) => {
                    if v.editing != 0 {
                        for ch in s.chars().filter(|ch| !ch.is_control()) {
                            if v.input.len() < 256 {
                                v.input.push(ch)
                            }
                        }
                    }
                    Ok(())
                }
                u::Event::Wheel(w) => {
                    v.wheel_remainder -= w.dy / 8.;
                    let rows = v.wheel_remainder as i64;
                    v.wheel_remainder -= rows as f32;
                    let target = if rows < 0 {
                        v.offset.saturating_sub(rows.unsigned_abs() * 16)
                    } else {
                        v.offset.saturating_add(rows as u64 * 16)
                    };
                    v.navigate(target)
                }
                _ => Ok(()),
            };
            if let Err(e) = result {
                v.status = e
            }
        }
        if v.search.is_some() {
            if let Err(e) = v.search_tick() {
                v.search = None;
                v.status = e
            }
            changed = true;
        }
        if changed {
            draw(&mut v, canvas)?;
        }
    }
    Ok(())
}
pub fn run(args: &[&str]) -> i32 {
    match app(args) {
        Ok(()) => 0,
        Err(e) => {
            let _ = stdio::stderr().write_all(format!("Hexview: {e}\n").as_bytes());
            1
        }
    }
}

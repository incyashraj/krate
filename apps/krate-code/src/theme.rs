//! The two looks: Dusk (dark) and Paper (light).
//!
//! Both are this app's own. Dusk is a blue-grey ground with warm accents --
//! coral keywords, honey numbers, sage strings, sea-teal types -- kept low in
//! saturation so a page of code reads calm rather than loud. Paper is the
//! same roles on a warm off-white, each colour darkened until it holds its
//! contrast. The mode colours are the loudest thing on screen on purpose:
//! a modal editor must never leave a person unsure which mode they are in.

use crate::syntax::Kind;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

pub const fn hex(v: u32) -> Rgba {
    Rgba {
        r: ((v >> 16) & 0xff) as f32 / 255.0,
        g: ((v >> 8) & 0xff) as f32 / 255.0,
        b: (v & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

impl Rgba {
    pub const fn with_alpha(self, a: f32) -> Rgba {
        Rgba { a, ..self }
    }

    /// A blend of `self` toward `other` by `t`, opaque.
    pub fn mix(self, other: Rgba, t: f32) -> Rgba {
        Rgba {
            r: self.r + (other.r - self.r) * t,
            g: self.g + (other.g - self.g) * t,
            b: self.b + (other.b - self.b) * t,
            a: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub dark: bool,
    /// Behind the code.
    pub editor: Rgba,
    /// Behind the tree, the tabs and the panel.
    pub chrome: Rgba,
    /// A raised surface: the palette, a prompt, the active tab.
    pub raised: Rgba,
    pub line: Rgba,
    pub current_line: Rgba,
    pub selection: Rgba,
    pub search_hit: Rgba,
    pub ink: Rgba,
    pub muted: Rgba,
    pub faint: Rgba,
    pub gutter: Rgba,
    pub gutter_current: Rgba,
    pub accent: Rgba,
    pub on_accent: Rgba,
    pub danger: Rgba,
    pub warn: Rgba,
    pub ok: Rgba,
    pub mode_normal: Rgba,
    pub mode_insert: Rgba,
    pub mode_visual: Rgba,
    pub mode_select: Rgba,
    pub mode_command: Rgba,
    pub on_mode: Rgba,
    pub cursor_normal: Rgba,
    pub cursor_insert: Rgba,
    pub breakpoint: Rgba,
    // Syntax.
    pub keyword: Rgba,
    pub special: Rgba,
    pub ty: Rgba,
    pub function: Rgba,
    pub mac: Rgba,
    pub lifetime: Rgba,
    pub string: Rgba,
    pub chr: Rgba,
    pub number: Rgba,
    pub comment: Rgba,
    pub doc: Rgba,
    pub attribute: Rgba,
    pub punct: Rgba,
    pub constant: Rgba,
}

pub const DUSK: Theme = Theme {
    dark: true,
    editor: hex(0x16191e),
    chrome: hex(0x111418),
    raised: hex(0x1f242b),
    line: hex(0x262c35),
    current_line: hex(0x1c2027),
    selection: hex(0x2b3d55),
    search_hit: hex(0x4a3f26),
    ink: hex(0xd6dbe2),
    muted: hex(0x9098a5),
    faint: hex(0x5b6472),
    gutter: hex(0x4a5260),
    gutter_current: hex(0xc9cfd8),
    accent: hex(0x86aee6),
    on_accent: hex(0x0f1216),
    danger: hex(0xe7867a),
    warn: hex(0xe8bb6c),
    ok: hex(0x86d0a6),
    mode_normal: hex(0x86aee6),
    mode_insert: hex(0x86d0a6),
    mode_visual: hex(0xe8bb6c),
    mode_select: hex(0xd5a0cf),
    mode_command: hex(0xe7967f),
    on_mode: hex(0x0f1216),
    cursor_normal: hex(0xe8bb6c),
    cursor_insert: hex(0x86d0a6),
    breakpoint: hex(0xe0675a),
    keyword: hex(0xe39582),
    special: hex(0xe39582),
    ty: hex(0x7ec6bf),
    function: hex(0xa9c4ee),
    mac: hex(0xa6acf2),
    lifetime: hex(0xd6a0c9),
    string: hex(0xb6cd8c),
    chr: hex(0xc9d68e),
    number: hex(0xe9b97a),
    comment: hex(0x6c7686),
    doc: hex(0x8592a3),
    attribute: hex(0x93a6ba),
    punct: hex(0x98a1ad),
    constant: hex(0xe9b97a),
};

pub const PAPER: Theme = Theme {
    dark: false,
    editor: hex(0xfbfaf7),
    chrome: hex(0xf1efea),
    raised: hex(0xffffff),
    line: hex(0xe2ded5),
    current_line: hex(0xf3f0e9),
    selection: hex(0xd3e0f2),
    search_hit: hex(0xf6e2a8),
    ink: hex(0x272b32),
    muted: hex(0x666c76),
    faint: hex(0x9aa0a8),
    gutter: hex(0xa7aab0),
    gutter_current: hex(0x3a3f47),
    accent: hex(0x2f5fa6),
    on_accent: hex(0xffffff),
    danger: hex(0xb43d2f),
    warn: hex(0x9a6512),
    ok: hex(0x2c7a4f),
    mode_normal: hex(0x2f5fa6),
    mode_insert: hex(0x2c7a4f),
    mode_visual: hex(0x9a6512),
    mode_select: hex(0x8a3f80),
    mode_command: hex(0xb4482f),
    on_mode: hex(0xffffff),
    cursor_normal: hex(0xb07412),
    cursor_insert: hex(0x2c7a4f),
    breakpoint: hex(0xc4402f),
    keyword: hex(0xb4482f),
    special: hex(0xb4482f),
    ty: hex(0x1d7570),
    function: hex(0x2d5ea6),
    mac: hex(0x4a50c0),
    lifetime: hex(0x963d83),
    string: hex(0x547a1e),
    chr: hex(0x6b7a12),
    number: hex(0x9c5e16),
    comment: hex(0x8b9099),
    doc: hex(0x5d6876),
    attribute: hex(0x5a6b7d),
    punct: hex(0x5c626c),
    constant: hex(0x9c5e16),
};

impl Theme {
    /// A terminal's sixteen colours (0-7 and their bright forms), tuned to
    /// read on this theme's panel.
    pub fn ansi(&self, i: u8) -> Rgba {
        const DARK: [u32; 16] = [
            0x2a2f3a, 0xe0787a, 0x9ec58b, 0xe6c07b, 0x7aa6e0, 0xc493d6, 0x6cc2c2, 0xc9ced6,
            0x5c6370, 0xf08f90, 0xb5dca0, 0xf2d38f, 0x96bdf2, 0xd7aee6, 0x8ad6d6, 0xf2f4f7,
        ];
        const LIGHT: [u32; 16] = [
            0x2b2b2b, 0xb4282d, 0x3c7a2a, 0x8a6200, 0x2457a8, 0x8a3aa0, 0x1f7a7a, 0x9a9a9a,
            0x5a5a5a, 0xd0383d, 0x4e9a37, 0xa67a00, 0x2f6cc7, 0xa64bbf, 0x2a9595, 0x6e6e6e,
        ];
        let table = if self.dark { &DARK } else { &LIGHT };
        hex(table[(i & 15) as usize])
    }

    pub fn for_dark(dark: bool) -> &'static Theme {
        if dark {
            &DUSK
        } else {
            &PAPER
        }
    }

    pub fn syntax(&self, kind: Kind) -> Rgba {
        match kind {
            Kind::Text => self.ink,
            Kind::Keyword => self.keyword,
            Kind::Special => self.special,
            Kind::Type => self.ty,
            Kind::Function => self.function,
            Kind::Macro => self.mac,
            Kind::Lifetime => self.lifetime,
            Kind::Str => self.string,
            Kind::Char => self.chr,
            Kind::Number => self.number,
            Kind::Comment => self.comment,
            Kind::DocComment => self.doc,
            Kind::Attribute => self.attribute,
            Kind::Punct => self.punct,
            Kind::Constant => self.constant,
        }
    }

    /// Comments are drawn in italic.
    pub fn italic(kind: Kind) -> bool {
        matches!(kind, Kind::Comment | Kind::DocComment)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG contrast ratio between two colours.
    fn contrast(a: Rgba, b: Rgba) -> f32 {
        fn lin(c: f32) -> f32 {
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        fn lum(c: Rgba) -> f32 {
            0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
        }
        let (la, lb) = (lum(a), lum(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    const ROLES: [Kind; 15] = [
        Kind::Text,
        Kind::Keyword,
        Kind::Special,
        Kind::Type,
        Kind::Function,
        Kind::Macro,
        Kind::Lifetime,
        Kind::Str,
        Kind::Char,
        Kind::Number,
        Kind::Comment,
        Kind::DocComment,
        Kind::Attribute,
        Kind::Punct,
        Kind::Constant,
    ];

    #[test]
    fn every_syntax_colour_reads_on_its_ground() {
        for theme in [&DUSK, &PAPER] {
            for kind in ROLES {
                let c = contrast(theme.syntax(kind), theme.editor);
                // Comments are allowed to be quieter, never unreadable.
                let floor = if matches!(kind, Kind::Comment) {
                    3.0
                } else {
                    4.5
                };
                assert!(
                    c >= floor,
                    "{kind:?} on {} is {c}",
                    if theme.dark { "dusk" } else { "paper" }
                );
            }
            assert!(contrast(theme.ink, theme.current_line) >= 7.0);
            assert!(contrast(theme.on_mode, theme.mode_normal) >= 4.5);
            assert!(contrast(theme.on_mode, theme.mode_insert) >= 4.5);
        }
    }
}

//! TeX math, laid out the way a notes app shows `$...$` and `$$...$$`.
//!
//! A real subset of TeX, not a fallback that shows the source: fractions,
//! sub- and superscripts, Greek and the common symbols, big operators with
//! limits (`\sum`, `\int`, `\lim`), roots, matrices and `cases`, `\left` and
//! `\right` delimiters that grow with what they hold, accents, `\text`,
//! `\mathbb` and spacing commands. What it does not know it shows in the
//! error colour, as KaTeX does, never as silence.
//!
//! The parser builds a small tree; [`layout`] turns it into boxes of drawing
//! operations relative to a baseline. Measuring text goes through
//! [`Measure`], so this runs natively in tests with a fake measure and in
//! the app with the canvas's.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

/// How a run of math text is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Font {
    /// Letters: serif italic.
    Italic,
    /// Digits, operators, function names, `\text`: serif upright.
    Roman,
    /// `\mathbf`.
    Bold,
}

/// The one thing layout needs from outside: how wide a string is.
pub trait Measure {
    fn width(&self, s: &str, size: f32, font: Font) -> f32;
}

/// One thing to draw, relative to the box's left edge and baseline (y grows
/// downward, so text above the baseline has negative y).
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Text {
        s: String,
        x: f32,
        y: f32,
        size: f32,
        font: Font,
        error: bool,
    },
    /// A filled rectangle (fraction bars, overlines).
    Rule { x: f32, y: f32, w: f32, h: f32 },
    /// A stroked segment (roots, tall delimiters, accents).
    Line {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        w: f32,
    },
}

/// A laid-out piece: its width, how far it rises above the baseline and
/// falls below it, and what to draw.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MathBox {
    pub w: f32,
    pub asc: f32,
    pub desc: f32,
    pub ops: Vec<Op>,
}

impl MathBox {
    fn place(&mut self, other: &MathBox, dx: f32, dy: f32) {
        for op in &other.ops {
            self.ops.push(shift(op, dx, dy));
        }
        self.asc = self.asc.max(other.asc - dy);
        self.desc = self.desc.max(other.desc + dy);
    }

    pub fn height(&self) -> f32 {
        self.asc + self.desc
    }
}

fn shift(op: &Op, dx: f32, dy: f32) -> Op {
    match op {
        Op::Text {
            s,
            x,
            y,
            size,
            font,
            error,
        } => Op::Text {
            s: s.clone(),
            x: x + dx,
            y: y + dy,
            size: *size,
            font: *font,
            error: *error,
        },
        Op::Rule { x, y, w, h } => Op::Rule {
            x: x + dx,
            y: y + dy,
            w: *w,
            h: *h,
        },
        Op::Line { x0, y0, x1, y1, w } => Op::Line {
            x0: x0 + dx,
            y0: y0 + dy,
            x1: x1 + dx,
            y1: y1 + dy,
            w: *w,
        },
    }
}

// ---- the tree ---------------------------------------------------------------------------

/// How an atom spaces itself from its neighbours (TeX's classes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Ord,
    Op,
    Bin,
    Rel,
    Open,
    Close,
    Punct,
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Atom {
        s: String,
        font: Font,
        class: Class,
        error: bool,
    },
    Group(Vec<Node>),
    Frac {
        num: Box<Node>,
        den: Box<Node>,
        bar: bool,
        /// `\dfrac` (true), `\tfrac` (false), or as the style says.
        display: Option<bool>,
    },
    Sqrt {
        index: Option<Box<Node>>,
        body: Box<Node>,
    },
    Scripts {
        base: Box<Node>,
        sup: Option<Box<Node>>,
        sub: Option<Box<Node>>,
    },
    /// `\sum`, `\int`, `\lim`: a big operator, with limits above and below
    /// in display style when `limits`.
    BigOp {
        s: String,
        font: Font,
        limits: bool,
        big: bool,
    },
    Delimited {
        open: String,
        body: Box<Node>,
        close: String,
    },
    Matrix {
        rows: Vec<Vec<Node>>,
        open: String,
        close: String,
        /// `cases` and `aligned` set their columns left-aligned.
        left: bool,
    },
    Accent {
        body: Box<Node>,
        kind: Accent,
    },
    Space(f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Accent {
    Hat,
    Bar,
    Vec,
    Dot,
    Ddot,
    Tilde,
    Under,
}

// ---- symbols ----------------------------------------------------------------------------

fn greek(name: &str) -> Option<&'static str> {
    Some(match name {
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "epsilon" => "ϵ",
        "varepsilon" => "ε",
        "zeta" => "ζ",
        "eta" => "η",
        "theta" => "θ",
        "vartheta" => "ϑ",
        "iota" => "ι",
        "kappa" => "κ",
        "lambda" => "λ",
        "mu" => "μ",
        "nu" => "ν",
        "xi" => "ξ",
        "omicron" => "ο",
        "pi" => "π",
        "varpi" => "ϖ",
        "rho" => "ρ",
        "varrho" => "ϱ",
        "sigma" => "σ",
        "varsigma" => "ς",
        "tau" => "τ",
        "upsilon" => "υ",
        "phi" => "ϕ",
        "varphi" => "φ",
        "chi" => "χ",
        "psi" => "ψ",
        "omega" => "ω",
        "Gamma" => "Γ",
        "Delta" => "Δ",
        "Theta" => "Θ",
        "Lambda" => "Λ",
        "Xi" => "Ξ",
        "Pi" => "Π",
        "Sigma" => "Σ",
        "Upsilon" => "Υ",
        "Phi" => "Φ",
        "Psi" => "Ψ",
        "Omega" => "Ω",
        _ => return None,
    })
}

/// A symbol command: its character and class.
fn symbol(name: &str) -> Option<(&'static str, Class)> {
    use Class::*;
    Some(match name {
        "leq" | "le" => ("≤", Rel),
        "geq" | "ge" => ("≥", Rel),
        "neq" | "ne" => ("≠", Rel),
        "approx" => ("≈", Rel),
        "equiv" => ("≡", Rel),
        "sim" => ("∼", Rel),
        "simeq" => ("≃", Rel),
        "cong" => ("≅", Rel),
        "propto" => ("∝", Rel),
        "ll" => ("≪", Rel),
        "gg" => ("≫", Rel),
        "to" | "rightarrow" => ("→", Rel),
        "leftarrow" | "gets" => ("←", Rel),
        "leftrightarrow" => ("↔", Rel),
        "Rightarrow" | "implies" => ("⇒", Rel),
        "Leftarrow" => ("⇐", Rel),
        "Leftrightarrow" | "iff" => ("⇔", Rel),
        "mapsto" => ("↦", Rel),
        "longrightarrow" => ("⟶", Rel),
        "in" => ("∈", Rel),
        "notin" => ("∉", Rel),
        "ni" => ("∋", Rel),
        "subset" => ("⊂", Rel),
        "supset" => ("⊃", Rel),
        "subseteq" => ("⊆", Rel),
        "supseteq" => ("⊇", Rel),
        "perp" => ("⊥", Rel),
        "parallel" => ("∥", Rel),
        "mid" => ("∣", Rel),
        "pm" => ("±", Bin),
        "mp" => ("∓", Bin),
        "times" => ("×", Bin),
        "cdot" => ("·", Bin),
        "div" => ("÷", Bin),
        "ast" => ("∗", Bin),
        "star" => ("⋆", Bin),
        "circ" => ("∘", Bin),
        "bullet" => ("∙", Bin),
        "oplus" => ("⊕", Bin),
        "otimes" => ("⊗", Bin),
        "cup" => ("∪", Bin),
        "cap" => ("∩", Bin),
        "setminus" => ("∖", Bin),
        "wedge" | "land" => ("∧", Bin),
        "vee" | "lor" => ("∨", Bin),
        "infty" => ("∞", Ord),
        "partial" => ("∂", Ord),
        "nabla" => ("∇", Ord),
        "forall" => ("∀", Ord),
        "exists" => ("∃", Ord),
        "nexists" => ("∄", Ord),
        "neg" | "lnot" => ("¬", Ord),
        "emptyset" | "varnothing" => ("∅", Ord),
        "prime" => ("′", Ord),
        "hbar" => ("ℏ", Ord),
        "ell" => ("ℓ", Ord),
        "Re" => ("ℜ", Ord),
        "Im" => ("ℑ", Ord),
        "aleph" => ("ℵ", Ord),
        "angle" => ("∠", Ord),
        "triangle" => ("△", Ord),
        "degree" => ("°", Ord),
        "ldots" | "dots" => ("…", Ord),
        "cdots" => ("⋯", Ord),
        "vdots" => ("⋮", Ord),
        "ddots" => ("⋱", Ord),
        "therefore" => ("∴", Rel),
        "because" => ("∵", Rel),
        "langle" => ("⟨", Open),
        "rangle" => ("⟩", Close),
        "lfloor" => ("⌊", Open),
        "rfloor" => ("⌋", Close),
        "lceil" => ("⌈", Open),
        "rceil" => ("⌉", Close),
        "lbrace" | "{" => ("{", Open),
        "rbrace" | "}" => ("}", Close),
        "vert" | "|" => ("|", Ord),
        "Vert" => ("‖", Ord),
        "%" => ("%", Ord),
        "$" => ("$", Ord),
        "&" => ("&", Ord),
        "#" => ("#", Ord),
        "_" => ("_", Ord),
        _ => return None,
    })
}

/// Big operators: the symbol, whether limits go above and below in display
/// style, and whether it is drawn large (named operators like `\lim` are
/// not).
fn big_op(name: &str) -> Option<(&'static str, bool, bool)> {
    Some(match name {
        "sum" => ("∑", true, true),
        "prod" => ("∏", true, true),
        "coprod" => ("∐", true, true),
        "bigcup" => ("⋃", true, true),
        "bigcap" => ("⋂", true, true),
        "bigoplus" => ("⨁", true, true),
        "int" => ("∫", false, true),
        "iint" => ("∬", false, true),
        "iiint" => ("∭", false, true),
        "oint" => ("∮", false, true),
        "lim" => ("lim", true, false),
        "limsup" => ("lim sup", true, false),
        "liminf" => ("lim inf", true, false),
        "max" => ("max", true, false),
        "min" => ("min", true, false),
        "sup" => ("sup", true, false),
        "inf" => ("inf", true, false),
        "det" => ("det", true, false),
        "gcd" => ("gcd", true, false),
        "argmax" => ("arg max", true, false),
        "argmin" => ("arg min", true, false),
        _ => return None,
    })
}

fn function_name(name: &str) -> bool {
    matches!(
        name,
        "sin"
            | "cos"
            | "tan"
            | "cot"
            | "sec"
            | "csc"
            | "arcsin"
            | "arccos"
            | "arctan"
            | "sinh"
            | "cosh"
            | "tanh"
            | "log"
            | "ln"
            | "lg"
            | "exp"
            | "arg"
            | "deg"
            | "dim"
            | "ker"
            | "hom"
            | "Pr"
            | "mod"
            | "bmod"
    )
}

fn double_struck(c: char) -> char {
    match c {
        'R' => 'ℝ',
        'N' => 'ℕ',
        'Z' => 'ℤ',
        'Q' => 'ℚ',
        'C' => 'ℂ',
        'P' => 'ℙ',
        'H' => 'ℍ',
        'E' => '𝔼',
        _ => c,
    }
}

// ---- parsing ----------------------------------------------------------------------------

struct Parser<'a> {
    s: &'a str,
    at: usize,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Cmd(String),
    Open,
    Close,
    Sup,
    Sub,
    Amp,
    Newline,
    Char(char),
    End,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Tok {
        let mut p = Parser {
            s: self.s,
            at: self.at,
        };
        p.next()
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.s.get(self.at..).and_then(|r| r.chars().next()) {
            if c.is_whitespace() {
                self.at += c.len_utf8();
            } else {
                break;
            }
        }
    }

    fn next(&mut self) -> Tok {
        self.skip_ws();
        let Some(c) = self.s.get(self.at..).and_then(|r| r.chars().next()) else {
            return Tok::End;
        };
        self.at += c.len_utf8();
        match c {
            '{' => Tok::Open,
            '}' => Tok::Close,
            '^' => Tok::Sup,
            '_' => Tok::Sub,
            '&' => Tok::Amp,
            '\\' => {
                let rest = self.s.get(self.at..).unwrap_or("");
                let n: usize = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphabetic())
                    .map(char::len_utf8)
                    .sum();
                if n == 0 {
                    // A one-character command: `\\`, `\,`, `\{`, `\|`.
                    let Some(c2) = rest.chars().next() else {
                        return Tok::Char('\\');
                    };
                    self.at += c2.len_utf8();
                    if c2 == '\\' {
                        return Tok::Newline;
                    }
                    let mut s = String::new();
                    s.push(c2);
                    return Tok::Cmd(s);
                }
                let name = String::from(rest.get(..n).unwrap_or(""));
                self.at += n;
                Tok::Cmd(name)
            }
            c => Tok::Char(c),
        }
    }

    /// A list of nodes up to `}`, `&`, `\\`, `\right`, `\end` or the end.
    fn list(&mut self) -> Vec<Node> {
        let mut out: Vec<Node> = Vec::new();
        loop {
            match self.peek() {
                Tok::End | Tok::Close | Tok::Amp | Tok::Newline => break,
                Tok::Cmd(ref c) if c == "right" || c == "end" => break,
                Tok::Sup | Tok::Sub => {
                    let sup = self.next() == Tok::Sup;
                    let arg = self.argument();
                    let base = out.pop().unwrap_or(Node::Group(Vec::new()));
                    let arg = Some(Box::new(arg));
                    let node = match base {
                        Node::Scripts {
                            base,
                            sup: s0,
                            sub: b0,
                        } if (sup && s0.is_none()) || (!sup && b0.is_none()) => {
                            let (s1, b1) = if sup { (arg, b0) } else { (s0, arg) };
                            Node::Scripts {
                                base,
                                sup: s1,
                                sub: b1,
                            }
                        }
                        other => {
                            let (s1, b1) = if sup { (arg, None) } else { (None, arg) };
                            Node::Scripts {
                                base: Box::new(other),
                                sup: s1,
                                sub: b1,
                            }
                        }
                    };
                    out.push(node);
                }
                _ => {
                    if let Some(n) = self.atom() {
                        out.push(n);
                    }
                }
            }
        }
        out
    }

    /// One argument: a `{group}` or a single token.
    fn argument(&mut self) -> Node {
        match self.peek() {
            Tok::Open => {
                self.next();
                let inner = self.list();
                if self.peek() == Tok::Close {
                    self.next();
                }
                Node::Group(inner)
            }
            Tok::End | Tok::Close => Node::Group(Vec::new()),
            _ => self.atom().unwrap_or(Node::Group(Vec::new())),
        }
    }

    /// The raw text of a `{...}` argument (for `\text`, `\begin{name}`).
    fn raw_argument(&mut self) -> String {
        self.skip_ws();
        let rest = self.s.get(self.at..).unwrap_or("");
        if !rest.starts_with('{') {
            // A single character.
            let c = rest.chars().next();
            if let Some(c) = c {
                self.at += c.len_utf8();
                let mut s = String::new();
                s.push(c);
                return s;
            }
            return String::new();
        }
        let mut depth = 0usize;
        for (i, c) in rest.char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let inner = String::from(rest.get(1..i).unwrap_or(""));
                        self.at += i + 1;
                        return inner;
                    }
                }
                _ => {}
            }
        }
        let inner = String::from(rest.get(1..).unwrap_or(""));
        self.at = self.s.len();
        inner
    }

    /// The delimiter after `\left`, `\right`, `\big` and friends.
    fn delimiter(&mut self) -> String {
        match self.next() {
            Tok::Char('.') => String::new(),
            Tok::Char(c) => {
                let mut s = String::new();
                s.push(c);
                s
            }
            Tok::Cmd(name) => match symbol(&name) {
                Some((s, _)) => String::from(s),
                None => match name.as_str() {
                    "|" => String::from("‖"),
                    _ => name,
                },
            },
            _ => String::new(),
        }
    }

    fn atom(&mut self) -> Option<Node> {
        let tok = self.next();
        Some(match tok {
            Tok::Open => {
                let inner = self.list();
                if self.peek() == Tok::Close {
                    self.next();
                }
                Node::Group(inner)
            }
            Tok::Char(c) => {
                let (font, class) = if c.is_ascii_alphabetic() {
                    (Font::Italic, Class::Ord)
                } else if c.is_ascii_digit() || c == '.' {
                    (Font::Roman, Class::Ord)
                } else {
                    let class = match c {
                        '+' | '-' | '*' => Class::Bin,
                        '=' | '<' | '>' | ':' => Class::Rel,
                        '(' | '[' => Class::Open,
                        ')' | ']' => Class::Close,
                        ',' | ';' => Class::Punct,
                        _ => Class::Ord,
                    };
                    (Font::Roman, class)
                };
                let mut s = String::new();
                s.push(match c {
                    '-' => '−',
                    '*' => '∗',
                    '\'' => '′',
                    _ => c,
                });
                if c == '~' {
                    return Some(Node::Space(0.28));
                }
                Node::Atom {
                    s,
                    font,
                    class,
                    error: false,
                }
            }
            Tok::Cmd(name) => return Some(self.command(&name)),
            Tok::Sup | Tok::Sub | Tok::Amp | Tok::Newline | Tok::Close | Tok::End => return None,
        })
    }

    fn command(&mut self, name: &str) -> Node {
        if let Some(g) = greek(name) {
            let upper = name.chars().next().is_some_and(|c| c.is_ascii_uppercase());
            return Node::Atom {
                s: String::from(g),
                font: if upper { Font::Roman } else { Font::Italic },
                class: Class::Ord,
                error: false,
            };
        }
        if let Some((s, class)) = symbol(name) {
            return Node::Atom {
                s: String::from(s),
                font: Font::Roman,
                class,
                error: false,
            };
        }
        if let Some((s, limits, big)) = big_op(name) {
            return Node::BigOp {
                s: String::from(s),
                font: Font::Roman,
                limits,
                big,
            };
        }
        if function_name(name) {
            return Node::Atom {
                s: String::from(name),
                font: Font::Roman,
                class: Class::Op,
                error: false,
            };
        }
        match name {
            "frac" | "dfrac" | "tfrac" | "cfrac" | "binom" | "dbinom" | "tbinom" => {
                let num = self.argument();
                let den = self.argument();
                let display = match name {
                    "dfrac" | "cfrac" | "dbinom" => Some(true),
                    "tfrac" | "tbinom" => Some(false),
                    _ => None,
                };
                let frac = Node::Frac {
                    num: Box::new(num),
                    den: Box::new(den),
                    bar: !name.ends_with("binom"),
                    display,
                };
                if name.ends_with("binom") {
                    return Node::Delimited {
                        open: String::from("("),
                        body: Box::new(frac),
                        close: String::from(")"),
                    };
                }
                frac
            }
            "sqrt" => {
                let mut index = None;
                self.skip_ws();
                if self.s.get(self.at..).is_some_and(|r| r.starts_with('[')) {
                    let rest = self.s.get(self.at + 1..).unwrap_or("");
                    if let Some(end) = rest.find(']') {
                        let inner = rest.get(..end).unwrap_or("");
                        let mut p = Parser { s: inner, at: 0 };
                        index = Some(Box::new(Node::Group(p.list())));
                        self.at += 1 + end + 1;
                    }
                }
                let body = self.argument();
                Node::Sqrt {
                    index,
                    body: Box::new(body),
                }
            }
            "left" => {
                let open = self.delimiter();
                let body = self.list();
                let mut close = String::new();
                if matches!(self.peek(), Tok::Cmd(ref c) if c == "right") {
                    self.next();
                    close = self.delimiter();
                }
                Node::Delimited {
                    open,
                    body: Box::new(Node::Group(body)),
                    close,
                }
            }
            "big" | "Big" | "bigg" | "Bigg" | "bigl" | "bigr" | "Bigl" | "Bigr" | "biggl"
            | "biggr" => {
                let d = self.delimiter();
                Node::Atom {
                    s: d,
                    font: Font::Roman,
                    class: if name.ends_with('r') {
                        Class::Close
                    } else {
                        Class::Open
                    },
                    error: false,
                }
            }
            "begin" => self.environment(),
            "text" | "textrm" | "mathrm" | "textup" | "operatorname" | "mbox" | "textnormal"
            | "textsf" | "mathsf" | "texttt" | "mathtt" => {
                let raw = self.raw_argument();
                if name == "mathrm" || name == "operatorname" {
                    let mut p = Parser { s: &raw, at: 0 };
                    return restyle(Node::Group(p.list()), Font::Roman);
                }
                Node::Atom {
                    s: raw,
                    font: Font::Roman,
                    class: if name == "operatorname" {
                        Class::Op
                    } else {
                        Class::Ord
                    },
                    error: false,
                }
            }
            "textbf" => Node::Atom {
                s: self.raw_argument(),
                font: Font::Bold,
                class: Class::Ord,
                error: false,
            },
            "textit" | "emph" => Node::Atom {
                s: self.raw_argument(),
                font: Font::Italic,
                class: Class::Ord,
                error: false,
            },
            "mathbf" | "boldsymbol" | "bm" => restyle(self.argument(), Font::Bold),
            "mathit" | "mathcal" | "mathscr" | "mathfrak" => restyle(self.argument(), Font::Italic),
            "mathbb" => {
                let raw = self.raw_argument();
                Node::Atom {
                    s: raw.chars().map(double_struck).collect(),
                    font: Font::Roman,
                    class: Class::Ord,
                    error: false,
                }
            }
            "hat" | "widehat" => self.accent(Accent::Hat),
            "bar" | "overline" => self.accent(Accent::Bar),
            "vec" | "overrightarrow" => self.accent(Accent::Vec),
            "dot" => self.accent(Accent::Dot),
            "ddot" => self.accent(Accent::Ddot),
            "tilde" | "widetilde" => self.accent(Accent::Tilde),
            "underline" => self.accent(Accent::Under),
            "," | "thinspace" => Node::Space(3.0 / 18.0),
            ":" | ">" | "medspace" => Node::Space(4.0 / 18.0),
            ";" | "thickspace" => Node::Space(5.0 / 18.0),
            "!" => Node::Space(-3.0 / 18.0),
            " " => Node::Space(0.28),
            "quad" => Node::Space(1.0),
            "qquad" => Node::Space(2.0),
            "displaystyle" | "textstyle" | "limits" | "nolimits" | "nonumber" | "notag" => {
                Node::Group(Vec::new())
            }
            "color" | "textcolor" => {
                let _ = self.raw_argument();
                if name == "textcolor" {
                    return self.argument();
                }
                Node::Group(Vec::new())
            }
            _ => {
                let mut s = String::from("\\");
                s.push_str(name);
                Node::Atom {
                    s,
                    font: Font::Roman,
                    class: Class::Ord,
                    error: true,
                }
            }
        }
    }

    fn accent(&mut self, kind: Accent) -> Node {
        let body = self.argument();
        Node::Accent {
            body: Box::new(body),
            kind,
        }
    }

    fn environment(&mut self) -> Node {
        let env = self.raw_argument();
        let base = env.trim_end_matches('*');
        if base == "array" {
            // The column spec says nothing a layout this simple uses.
            let _ = self.raw_argument();
        }
        let mut rows: Vec<Vec<Node>> = Vec::new();
        let mut row: Vec<Node> = Vec::new();
        loop {
            let cell = self.list();
            row.push(Node::Group(cell));
            match self.next() {
                Tok::Amp => {}
                Tok::Newline => {
                    rows.push(core::mem::take(&mut row));
                }
                Tok::Cmd(c) if c == "end" => {
                    let _ = self.raw_argument();
                    break;
                }
                Tok::Close => {}
                _ => break,
            }
        }
        if row
            .iter()
            .any(|c| !matches!(c, Node::Group(v) if v.is_empty()))
            || rows.is_empty()
        {
            rows.push(row);
        }
        let (open, close) = match base {
            "pmatrix" => ("(", ")"),
            "bmatrix" => ("[", "]"),
            "Bmatrix" => ("{", "}"),
            "vmatrix" => ("|", "|"),
            "Vmatrix" => ("‖", "‖"),
            "cases" => ("{", ""),
            _ => ("", ""),
        };
        Node::Matrix {
            rows,
            open: String::from(open),
            close: String::from(close),
            left: matches!(base, "cases" | "aligned" | "align" | "split" | "gathered"),
        }
    }
}

/// Set every atom in a node in `font` (`\mathrm`, `\mathbf`).
fn restyle(n: Node, font: Font) -> Node {
    match n {
        Node::Atom {
            s, class, error, ..
        } => Node::Atom {
            s,
            font,
            class,
            error,
        },
        Node::Group(v) => Node::Group(v.into_iter().map(|x| restyle(x, font)).collect()),
        Node::Scripts { base, sup, sub } => Node::Scripts {
            base: Box::new(restyle(*base, font)),
            sup,
            sub,
        },
        other => other,
    }
}

fn parse(src: &str) -> Node {
    let mut p = Parser { s: src, at: 0 };
    let mut all = Vec::new();
    // Stray closers and separators at the top level are dropped, not fatal.
    loop {
        all.extend(p.list());
        if p.next() == Tok::End {
            break;
        }
    }
    Node::Group(all)
}

// ---- layout -----------------------------------------------------------------------------

/// The style a piece is set in: its size and whether it is display style.
#[derive(Clone, Copy)]
struct Sty {
    size: f32,
    display: bool,
    /// Script depth: 0 for the main line, 1 in a script, 2 deeper.
    depth: u8,
}

impl Sty {
    fn script(self) -> Sty {
        Sty {
            size: if self.depth == 0 {
                self.size * 0.7
            } else {
                (self.size * 0.72).max(self.size * 0.5)
            },
            display: false,
            depth: self.depth.saturating_add(1).min(2),
        }
    }

    fn frac_part(self) -> Sty {
        if self.display {
            Sty {
                display: false,
                ..self
            }
        } else {
            self.script()
        }
    }
}

const ASC: f32 = 0.72;
const DESC: f32 = 0.22;
/// Where a fraction bar and the middle of a delimiter sit above the baseline.
const AXIS: f32 = 0.25;

fn text_box(s: &str, font: Font, size: f32, error: bool, m: &dyn Measure) -> MathBox {
    let w = m.width(s, size, font);
    // Italic letters lean past their advance; a sliver of room keeps a
    // superscript from touching them.
    let w = if font == Font::Italic {
        w + size * 0.04
    } else {
        w
    };
    MathBox {
        w,
        asc: size * ASC,
        desc: size * DESC,
        ops: alloc::vec![Op::Text {
            s: String::from(s),
            x: 0.0,
            y: 0.0,
            size,
            font,
            error,
        }],
    }
}

fn space(class_a: Class, class_b: Class, st: Sty) -> f32 {
    use Class::*;
    if st.depth > 0 {
        // Scripts only keep the thin space around operators.
        return match (class_a, class_b) {
            (Op, Ord) | (Ord, Op) => st.size * 3.0 / 18.0,
            _ => 0.0,
        };
    }
    let em = st.size;
    match (class_a, class_b) {
        (Bin, _) | (_, Bin) => em * 4.0 / 18.0,
        (Rel, _) | (_, Rel) => em * 5.0 / 18.0,
        (Punct, _) => em * 3.0 / 18.0,
        (Op, Ord) | (Ord, Op) | (Op, Op) | (Close, Op) => em * 3.0 / 18.0,
        _ => 0.0,
    }
}

fn class_of(n: &Node) -> Class {
    match n {
        Node::Atom { class, .. } => *class,
        Node::BigOp { .. } => Class::Op,
        Node::Scripts { base, .. } => class_of(base),
        Node::Delimited { .. } => Class::Ord,
        _ => Class::Ord,
    }
}

fn hlist(nodes: &[Node], st: Sty, m: &dyn Measure) -> MathBox {
    let mut out = MathBox {
        w: 0.0,
        asc: st.size * ASC,
        desc: st.size * DESC,
        ops: Vec::new(),
    };
    let mut prev: Option<Class> = None;
    for (i, n) in nodes.iter().enumerate() {
        let mut class = class_of(n);
        // A binary operator with nothing on its left is a sign: `-x`.
        if class == Class::Bin
            && matches!(
                prev,
                None | Some(Class::Bin | Class::Rel | Class::Open | Class::Punct | Class::Op)
            )
        {
            class = Class::Ord;
        }
        if class == Class::Bin && i + 1 == nodes.len() {
            class = Class::Ord;
        }
        if let Some(p) = prev {
            out.w += space(p, class, st);
        }
        let b = node(n, st, m);
        let x = out.w;
        out.place(&b, x, 0.0);
        out.w += b.w;
        prev = Some(class);
    }
    out
}

fn node(n: &Node, st: Sty, m: &dyn Measure) -> MathBox {
    match n {
        Node::Atom { s, font, error, .. } => text_box(s, *font, st.size, *error, m),
        Node::Group(v) => hlist(v, st, m),
        Node::Space(em) => MathBox {
            w: em * st.size,
            asc: 0.0,
            desc: 0.0,
            ops: Vec::new(),
        },
        Node::Frac {
            num,
            den,
            bar,
            display,
        } => {
            let st = match display {
                Some(d) => Sty { display: *d, ..st },
                None => st,
            };
            let part = st.frac_part();
            let a = node(num, part, m);
            let b = node(den, part, m);
            let pad = st.size * 0.12;
            let w = a.w.max(b.w) + pad * 2.0;
            let t = (st.size * 0.05).max(1.0);
            let gap = if st.display {
                st.size * 0.16
            } else {
                st.size * 0.1
            };
            let axis = -st.size * AXIS;
            let mut out = MathBox {
                w,
                asc: 0.0,
                desc: 0.0,
                ops: Vec::new(),
            };
            let num_base = axis - t * 0.5 - gap - a.desc;
            let den_base = axis + t * 0.5 + gap + b.asc;
            out.place(&a, (w - a.w) * 0.5, num_base);
            out.place(&b, (w - b.w) * 0.5, den_base);
            if *bar {
                out.ops.push(Op::Rule {
                    x: pad * 0.5,
                    y: axis - t * 0.5,
                    w: w - pad,
                    h: t,
                });
            }
            out
        }
        Node::Sqrt { index, body } => {
            let b = node(body, st, m);
            let t = (st.size * 0.05).max(1.0);
            let gap = st.size * 0.12;
            let top = -(b.asc + gap);
            let bottom = b.desc;
            let h = bottom - top;
            let sign_w = st.size * 0.35 + h * 0.12;
            let mut out = MathBox {
                w: 0.0,
                asc: 0.0,
                desc: 0.0,
                ops: Vec::new(),
            };
            let mut x0 = 0.0;
            if let Some(ix) = index {
                let ib = node(ix, st.script().script(), m);
                out.place(&ib, 0.0, top + h * 0.45 - ib.desc);
                x0 = (ib.w - sign_w * 0.45).max(0.0);
            }
            let lw = t * 1.1;
            // The tick, the long stroke down, the stroke up, the bar over.
            let mid = bottom - h * 0.42;
            out.ops.push(Op::Line {
                x0,
                y0: mid,
                x1: x0 + sign_w * 0.3,
                y1: mid - st.size * 0.06,
                w: lw,
            });
            out.ops.push(Op::Line {
                x0: x0 + sign_w * 0.3,
                y0: mid - st.size * 0.06,
                x1: x0 + sign_w * 0.55,
                y1: bottom,
                w: lw * 1.5,
            });
            out.ops.push(Op::Line {
                x0: x0 + sign_w * 0.55,
                y0: bottom,
                x1: x0 + sign_w,
                y1: top,
                w: lw,
            });
            let bx = x0 + sign_w;
            out.ops.push(Op::Rule {
                x: bx,
                y: top - t * 0.5,
                w: b.w + st.size * 0.1,
                h: t,
            });
            out.place(&b, bx + st.size * 0.05, 0.0);
            out.asc = out.asc.max(-top + t);
            out.desc = out.desc.max(bottom);
            out.w = bx + b.w + st.size * 0.15;
            out
        }
        Node::Scripts { base, sup, sub } => {
            if let Node::BigOp {
                s,
                font,
                limits: true,
                big,
            } = base.as_ref()
            {
                if st.display {
                    return limits_op(s, *font, *big, sup, sub, st, m);
                }
            }
            let b = node(base, st, m);
            scripts(b, sup.as_deref(), sub.as_deref(), st, m)
        }
        Node::BigOp { s, font, big, .. } => op_box(s, *font, *big, st, m),
        Node::Delimited { open, body, close } => {
            let b = node(body, st, m);
            delimited(open, b, close, st, m)
        }
        Node::Matrix {
            rows,
            open,
            close,
            left,
        } => {
            let b = matrix(rows, *left, st, m);
            if open.is_empty() && close.is_empty() {
                b
            } else {
                delimited(open, b, close, st, m)
            }
        }
        Node::Accent { body, kind } => {
            let b = node(body, st, m);
            accent(b, *kind, st)
        }
    }
}

fn op_box(s: &str, font: Font, big: bool, st: Sty, m: &dyn Measure) -> MathBox {
    if !big {
        return text_box(s, font, st.size, false, m);
    }
    let integral = s.contains('∫') || s.contains('∮') || s.contains('∬');
    let scale = match (st.display, integral) {
        (true, true) => 2.0,
        (true, false) => 1.6,
        (false, true) => 1.3,
        (false, false) => 1.15,
    };
    let size = st.size * scale;
    let mut b = text_box(s, font, size, false, m);
    // Centre the symbol on the maths axis.
    let centre = -(b.asc - b.desc) * 0.5;
    let dy = -st.size * AXIS - centre;
    let mut out = MathBox {
        w: b.w,
        asc: 0.0,
        desc: 0.0,
        ops: Vec::new(),
    };
    b.ops = b.ops.iter().map(|o| shift(o, 0.0, 0.0)).collect();
    out.place(&b, 0.0, dy);
    out
}

fn limits_op(
    s: &str,
    font: Font,
    big: bool,
    sup: &Option<Box<Node>>,
    sub: &Option<Box<Node>>,
    st: Sty,
    m: &dyn Measure,
) -> MathBox {
    let op = op_box(s, font, big, st, m);
    let sst = st.script();
    let up = sup.as_ref().map(|n| node(n, sst, m));
    let dn = sub.as_ref().map(|n| node(n, sst, m));
    let w =
        op.w.max(up.as_ref().map_or(0.0, |b| b.w))
            .max(dn.as_ref().map_or(0.0, |b| b.w));
    let gap = st.size * 0.12;
    let mut out = MathBox {
        w,
        asc: 0.0,
        desc: 0.0,
        ops: Vec::new(),
    };
    out.place(&op, (w - op.w) * 0.5, 0.0);
    if let Some(u) = up {
        let y = -op.asc - gap - u.desc;
        out.place(&u, (w - u.w) * 0.5, y);
    }
    if let Some(d) = dn {
        let y = op.desc + gap + d.asc;
        out.place(&d, (w - d.w) * 0.5, y);
    }
    out
}

fn scripts(
    b: MathBox,
    sup: Option<&Node>,
    sub: Option<&Node>,
    st: Sty,
    m: &dyn Measure,
) -> MathBox {
    let sst = st.script();
    let up = sup.map(|n| node(n, sst, m));
    let dn = sub.map(|n| node(n, sst, m));
    let mut out = MathBox {
        w: b.w,
        asc: 0.0,
        desc: 0.0,
        ops: Vec::new(),
    };
    out.place(&b, 0.0, 0.0);
    let x = b.w + st.size * 0.03;
    let mut w = b.w;
    // A tall base (a fraction, a big operator) lifts its scripts with it.
    let tall = b.asc > st.size * 0.9;
    if let Some(u) = &up {
        let shift_up = if tall {
            b.asc - u.asc * 0.6
        } else if dn.is_some() {
            st.size * 0.42
        } else {
            st.size * 0.38
        };
        out.place(u, x, -shift_up);
        w = w.max(x + u.w);
    }
    if let Some(d) = &dn {
        let shift_dn = if tall {
            b.desc + d.asc * 0.4
        } else if up.is_some() {
            st.size * 0.26
        } else {
            st.size * 0.18
        };
        out.place(d, x, shift_dn);
        w = w.max(x + d.w);
    }
    out.w = w + st.size * 0.04;
    out
}

/// A delimiter drawn to `h` tall around the axis: text when it is small, strokes
/// when it must grow.
fn delim(d: &str, asc: f32, desc: f32, st: Sty, m: &dyn Measure) -> MathBox {
    if d.is_empty() {
        return MathBox {
            w: st.size * 0.1,
            ..MathBox::default()
        };
    }
    let h = asc + desc;
    if h <= st.size * 1.2 {
        let mut b = text_box(d, Font::Roman, st.size, false, m);
        b.w += st.size * 0.04;
        return b;
    }
    let lw = (st.size * 0.06).max(1.1);
    let w = st.size * 0.42;
    let top = -asc;
    let bot = desc;
    let mid = (top + bot) * 0.5;
    let mut ops = Vec::new();
    let mut seg = |x0: f32, y0: f32, x1: f32, y1: f32| {
        ops.push(Op::Line {
            x0,
            y0,
            x1,
            y1,
            w: lw,
        })
    };
    let l = w * 0.25;
    let r = w * 0.75;
    match d {
        "(" | ")" => {
            // A bow from top to bottom, as a few chords of a curve.
            let left = d == "(";
            let n = 10;
            let mut px = 0.0;
            let mut py = 0.0;
            for i in 0..=n {
                let t = i as f32 / n as f32;
                let y = top + (bot - top) * t;
                let bulge = 1.0 - (2.0 * t - 1.0) * (2.0 * t - 1.0);
                let x = if left {
                    r - (r - l) * bulge
                } else {
                    l + (r - l) * bulge
                };
                if i > 0 {
                    seg(px, py, x, y);
                }
                px = x;
                py = y;
            }
        }
        "[" | "⌈" | "⌊" => {
            seg(l, top, l, bot);
            if d != "⌊" {
                seg(l, top, r, top);
            }
            if d != "⌈" {
                seg(l, bot, r, bot);
            }
        }
        "]" | "⌉" | "⌋" => {
            seg(r, top, r, bot);
            if d != "⌋" {
                seg(l, top, r, top);
            }
            if d != "⌉" {
                seg(l, bot, r, bot);
            }
        }
        "{" => {
            let c = w * 0.5;
            seg(r, top, c, top + h * 0.08);
            seg(c, top + h * 0.08, c, mid - h * 0.06);
            seg(c, mid - h * 0.06, l, mid);
            seg(l, mid, c, mid + h * 0.06);
            seg(c, mid + h * 0.06, c, bot - h * 0.08);
            seg(c, bot - h * 0.08, r, bot);
        }
        "}" => {
            let c = w * 0.5;
            seg(l, top, c, top + h * 0.08);
            seg(c, top + h * 0.08, c, mid - h * 0.06);
            seg(c, mid - h * 0.06, r, mid);
            seg(r, mid, c, mid + h * 0.06);
            seg(c, mid + h * 0.06, c, bot - h * 0.08);
            seg(c, bot - h * 0.08, l, bot);
        }
        "‖" => {
            seg(w * 0.4, top, w * 0.4, bot);
            seg(w * 0.6, top, w * 0.6, bot);
        }
        "⟨" => {
            seg(r, top, l, mid);
            seg(l, mid, r, bot);
        }
        "⟩" => {
            seg(l, top, r, mid);
            seg(r, mid, l, bot);
        }
        _ => seg(w * 0.5, top, w * 0.5, bot),
    }
    MathBox { w, asc, desc, ops }
}

fn delimited(open: &str, body: MathBox, close: &str, st: Sty, m: &dyn Measure) -> MathBox {
    // Delimiters are symmetric about the axis and cover the body.
    let axis = st.size * AXIS;
    let half = (body.asc - axis).max(body.desc + axis) + st.size * 0.08;
    let asc = half + axis;
    let desc = half - axis;
    let lb = delim(open, asc, desc, st, m);
    let rb = delim(close, asc, desc, st, m);
    let mut out = MathBox::default();
    out.place(&lb, 0.0, 0.0);
    out.place(&body, lb.w, 0.0);
    out.place(&rb, lb.w + body.w, 0.0);
    out.w = lb.w + body.w + rb.w;
    out
}

fn matrix(rows: &[Vec<Node>], left: bool, st: Sty, m: &dyn Measure) -> MathBox {
    let cell_st = Sty {
        display: false,
        ..st
    };
    let boxes: Vec<Vec<MathBox>> = rows
        .iter()
        .map(|r| r.iter().map(|c| node(c, cell_st, m)).collect())
        .collect();
    let cols = boxes.iter().map(Vec::len).max().unwrap_or(0);
    let mut col_w = alloc::vec![0.0f32; cols];
    for r in &boxes {
        for (i, b) in r.iter().enumerate() {
            if let Some(w) = col_w.get_mut(i) {
                *w = w.max(b.w);
            }
        }
    }
    let col_gap = if left { st.size * 1.0 } else { st.size * 0.9 };
    let row_gap = st.size * 0.25;
    let mut out = MathBox::default();
    let mut y = 0.0f32;
    let mut heights = Vec::new();
    for r in &boxes {
        let asc = r.iter().map(|b| b.asc).fold(st.size * ASC, f32::max);
        let desc = r.iter().map(|b| b.desc).fold(st.size * DESC, f32::max);
        heights.push((asc, desc));
    }
    let total: f32 = heights.iter().map(|(a, d)| a + d).sum::<f32>()
        + row_gap * (heights.len().saturating_sub(1)) as f32;
    // Centre the whole array on the axis.
    let top = -total * 0.5 - st.size * AXIS;
    for (ri, r) in boxes.iter().enumerate() {
        let (asc, desc) = heights.get(ri).copied().unwrap_or((0.0, 0.0));
        let base = top + y + asc;
        let mut x = st.size * 0.1;
        for (ci, b) in r.iter().enumerate() {
            let cw = col_w.get(ci).copied().unwrap_or(0.0);
            let dx = if left { 0.0 } else { (cw - b.w) * 0.5 };
            out.place(b, x + dx, base);
            x += cw + col_gap;
        }
        y += asc + desc + row_gap;
    }
    out.w = col_w.iter().sum::<f32>() + col_gap * (cols.saturating_sub(1)) as f32 + st.size * 0.2;
    out.asc = out.asc.max(-top);
    out.desc = out.desc.max(top + total);
    out
}

fn accent(b: MathBox, kind: Accent, st: Sty) -> MathBox {
    let mut out = MathBox {
        w: b.w,
        ..MathBox::default()
    };
    out.place(&b, 0.0, 0.0);
    let lw = (st.size * 0.05).max(1.0);
    let top = -b.asc - st.size * 0.06;
    let cx = b.w * 0.5 + if b.w < st.size { st.size * 0.06 } else { 0.0 };
    let hw = (b.w * 0.5).clamp(st.size * 0.15, st.size * 2.0);
    match kind {
        Accent::Bar => out.ops.push(Op::Rule {
            x: 0.0,
            y: top - lw,
            w: b.w,
            h: lw,
        }),
        Accent::Under => out.ops.push(Op::Rule {
            x: 0.0,
            y: b.desc + st.size * 0.05,
            w: b.w,
            h: lw,
        }),
        Accent::Hat => {
            let hh = st.size * 0.12;
            out.ops.push(Op::Line {
                x0: cx - hw * 0.8,
                y0: top,
                x1: cx,
                y1: top - hh,
                w: lw,
            });
            out.ops.push(Op::Line {
                x0: cx,
                y0: top - hh,
                x1: cx + hw * 0.8,
                y1: top,
                w: lw,
            });
        }
        Accent::Vec => {
            let y = top - st.size * 0.06;
            out.ops.push(Op::Line {
                x0: cx - hw,
                y0: y,
                x1: cx + hw,
                y1: y,
                w: lw,
            });
            let a = st.size * 0.1;
            out.ops.push(Op::Line {
                x0: cx + hw - a,
                y0: y - a * 0.7,
                x1: cx + hw,
                y1: y,
                w: lw,
            });
            out.ops.push(Op::Line {
                x0: cx + hw - a,
                y0: y + a * 0.7,
                x1: cx + hw,
                y1: y,
                w: lw,
            });
        }
        Accent::Dot | Accent::Ddot => {
            let d = st.size * 0.09;
            let y = top - d;
            let xs: &[f32] = if kind == Accent::Dot {
                &[0.0]
            } else {
                &[-0.14, 0.14]
            };
            for dx in xs {
                let x = cx + dx * st.size;
                out.ops.push(Op::Rule {
                    x: x - d * 0.5,
                    y: y - d * 0.5,
                    w: d,
                    h: d,
                });
            }
        }
        Accent::Tilde => {
            let y = top - st.size * 0.08;
            let a = st.size * 0.05;
            out.ops.push(Op::Line {
                x0: cx - hw * 0.8,
                y0: y + a,
                x1: cx - hw * 0.25,
                y1: y - a,
                w: lw,
            });
            out.ops.push(Op::Line {
                x0: cx - hw * 0.25,
                y0: y - a,
                x1: cx + hw * 0.25,
                y1: y + a,
                w: lw,
            });
            out.ops.push(Op::Line {
                x0: cx + hw * 0.25,
                y0: y + a,
                x1: cx + hw * 0.8,
                y1: y - a,
                w: lw,
            });
        }
    }
    out.asc = out.asc.max(-top + st.size * 0.2);
    out
}

/// Lay out `src` at `size` px, in display style (a `$$` block) or inline.
pub fn layout(src: &str, size: f32, display: bool, m: &dyn Measure) -> MathBox {
    let tree = parse(src);
    let st = Sty {
        size,
        display,
        depth: 0,
    };
    let mut b = node(&tree, st, m);
    if b.ops.is_empty() {
        b.w = b.w.max(size * 0.2);
    }
    b
}

/// Whether the TeX holds anything this renderer could not read (drawn in
/// the error colour).
pub fn has_errors(b: &MathBox) -> bool {
    b.ops
        .iter()
        .any(|o| matches!(o, Op::Text { error: true, .. }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every character is half the size wide; good enough to test geometry.
    struct Fixed;
    impl Measure for Fixed {
        fn width(&self, s: &str, size: f32, _font: Font) -> f32 {
            s.chars().count() as f32 * size * 0.5
        }
    }

    fn texts(b: &MathBox) -> Vec<(String, f32, f32, f32)> {
        b.ops
            .iter()
            .filter_map(|o| match o {
                Op::Text { s, x, y, size, .. } => Some((s.clone(), *x, *y, *size)),
                _ => None,
            })
            .collect()
    }

    fn find<'a>(t: &'a [(String, f32, f32, f32)], s: &str) -> &'a (String, f32, f32, f32) {
        t.iter().find(|x| x.0 == s).unwrap()
    }

    #[test]
    fn fractions_stack_over_a_bar() {
        let b = layout("\\frac{a}{b}", 20.0, true, &Fixed);
        let t = texts(&b);
        let a = find(&t, "a");
        let bb = find(&t, "b");
        assert!(a.2 < 0.0 && bb.2 > 0.0, "num above, den below: {t:?}");
        assert!(b.ops.iter().any(|o| matches!(o, Op::Rule { .. })));
        assert!(b.asc > 20.0 * ASC && b.desc > 20.0 * DESC);
        // Inline, the parts are smaller.
        let i = layout("\\frac{a}{b}", 20.0, false, &Fixed);
        assert!(find(&texts(&i), "a").3 < 20.0);
        assert!(!has_errors(&b));
    }

    #[test]
    fn scripts_rise_and_fall_smaller() {
        let b = layout("x_i^2", 20.0, false, &Fixed);
        let t = texts(&b);
        let x = find(&t, "x");
        let i = find(&t, "i");
        let two = find(&t, "2");
        assert!(two.2 < x.2 && i.2 > x.2);
        assert!(two.3 < 20.0 && i.3 < 20.0);
        assert!(
            two.1 > x.1 && (two.1 - i.1).abs() < 0.01,
            "stacked at one x"
        );
        // Nested: e^{i\pi} sets the exponent in a script size with Greek.
        let e = texts(&layout("e^{i\\pi} + 1 = 0", 20.0, false, &Fixed));
        assert!(e.iter().any(|x| x.0 == "π" && x.2 < 0.0));
        assert!(e.iter().any(|x| x.0 == "="));
    }

    #[test]
    fn big_operators_take_limits_in_display() {
        let b = layout("\\sum_{i=1}^{n} i", 20.0, true, &Fixed);
        let t = texts(&b);
        let sum = find(&t, "∑");
        let n = find(&t, "n");
        assert!(sum.3 > 20.0, "the sign is big");
        assert!(n.2 < sum.2 - 10.0, "upper limit above");
        assert!(
            (n.1 + 5.0 - (sum.1 + sum.3 * 0.25)).abs() < 12.0,
            "centred over it"
        );
        // Inline the limits sit to the side.
        let i = texts(&layout("\\sum_{i=1}^{n} i", 20.0, false, &Fixed));
        assert!(find(&i, "n").1 > find(&i, "∑").1);
        // Integrals keep side scripts even in display.
        let int = texts(&layout("\\int_0^\\infty e^{-x^2}\\,dx", 20.0, true, &Fixed));
        assert!(find(&int, "∞").1 > find(&int, "∫").1);
        let lim = texts(&layout("\\lim_{x \\to 0} f", 20.0, true, &Fixed));
        assert!(find(&lim, "→").2 > find(&lim, "lim").2);
    }

    #[test]
    fn roots_matrices_and_delimiters() {
        let r = layout("\\sqrt{\\pi}", 20.0, true, &Fixed);
        assert!(
            r.ops
                .iter()
                .filter(|o| matches!(o, Op::Line { .. }))
                .count()
                >= 3
        );
        assert!(r.ops.iter().any(|o| matches!(o, Op::Rule { .. })));
        let cube = texts(&layout("\\sqrt[3]{x}", 20.0, true, &Fixed));
        assert!(cube.iter().any(|t| t.0 == "3" && t.3 < 20.0));
        let m = layout(
            "\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}",
            20.0,
            true,
            &Fixed,
        );
        let t = texts(&m);
        let (a, b, c, d) = (find(&t, "a"), find(&t, "b"), find(&t, "c"), find(&t, "d"));
        assert!((a.2 - b.2).abs() < 0.01 && (c.2 - d.2).abs() < 0.01);
        assert!(c.2 > a.2 && b.1 > a.1 && (a.1 - c.1).abs() < 0.01);
        // Two rows are taller than one line: the parentheses are drawn.
        assert!(
            m.ops
                .iter()
                .filter(|o| matches!(o, Op::Line { .. }))
                .count()
                >= 10
        );
        let cases = texts(&layout(
            "f(x) = \\begin{cases} 1 & x > 0 \\\\ 0 & \\text{otherwise} \\end{cases}",
            20.0,
            true,
            &Fixed,
        ));
        assert!(cases.iter().any(|t| t.0 == "otherwise"));
        let lr = layout("\\left( \\frac{a}{b} \\right)", 20.0, true, &Fixed);
        assert!(lr.ops.iter().any(|o| matches!(o, Op::Line { .. })));
        assert!(!has_errors(&lr));
    }

    #[test]
    fn symbols_text_and_errors() {
        let t = texts(&layout(
            "\\alpha + \\beta \\leq \\gamma \\cdot \\mathbb{R} \\text{ if } x",
            20.0,
            false,
            &Fixed,
        ));
        for s in ["α", "β", "≤", "γ", "·", "ℝ", " if "] {
            assert!(t.iter().any(|x| x.0 == s), "{s} in {t:?}");
        }
        let bad = layout("\\notacommand x", 20.0, false, &Fixed);
        assert!(has_errors(&bad));
        // A relation gets more space than nothing: `a=b` is wider than `ab=`.
        let w1 = layout("a=b", 20.0, false, &Fixed).w;
        let w2 = layout("ab", 20.0, false, &Fixed).w;
        assert!(w1 > w2 + 20.0 * 0.5);
        // Unbalanced input does not panic.
        for src in [
            "\\frac{a",
            "x^",
            "}{",
            "\\left(",
            "\\begin{matrix} a &",
            "",
            "\\sqrt[",
        ] {
            let _ = layout(src, 16.0, true, &Fixed);
        }
    }

    #[test]
    fn accents_sit_above() {
        let b = layout(
            "\\hat{x} + \\vec{v} + \\bar{y} + \\dot{z}",
            20.0,
            false,
            &Fixed,
        );
        assert!(b.asc > 20.0 * ASC);
        assert!(!has_errors(&b));
    }
}

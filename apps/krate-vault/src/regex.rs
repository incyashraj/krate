//! A small regular-expression engine for find and replace in a note, the
//! JavaScript flavour the editor's search uses: literals, `.`, classes
//! (`[a-z]`, `[^...]`, `\d \w \s \D \W \S`), anchors `^ $ \b \B`, groups
//! `( )` and `(?: )` with `|`, and the quantifiers `* + ? {m} {m,} {m,n}`,
//! greedy or lazy (`*?`). Matching is backtracking over one line; the
//! replacement takes `$&`, `$1`..`$9` and `$$`.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Char(char),
    Any,
    Class(Vec<(char, char)>, bool),
    Start,
    End,
    WordB(bool),
    Group(Box<Node>, Option<usize>),
    Alt(Vec<Node>),
    Seq(Vec<Node>),
    Repeat(Box<Node>, usize, usize, bool),
}

#[derive(Debug, Clone)]
pub struct Regex {
    root: Node,
    groups: usize,
    ci: bool,
}

struct P<'a> {
    s: &'a [char],
    i: usize,
    groups: usize,
}

const DIGIT: &[(char, char)] = &[('0', '9')];
const WORD: &[(char, char)] = &[('a', 'z'), ('A', 'Z'), ('0', '9'), ('_', '_')];
const SPACE: &[(char, char)] = &[
    (' ', ' '),
    ('\t', '\t'),
    ('\n', '\n'),
    ('\r', '\r'),
    ('\u{b}', '\u{c}'),
];

impl<'a> P<'a> {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn alt(&mut self) -> Result<Node, String> {
        let mut arms = alloc::vec![self.seq()?];
        while self.peek() == Some('|') {
            self.i += 1;
            arms.push(self.seq()?);
        }
        Ok(if arms.len() == 1 {
            arms.remove(0)
        } else {
            Node::Alt(arms)
        })
    }

    fn seq(&mut self) -> Result<Node, String> {
        let mut v = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom()?;
            v.push(self.quant(atom)?);
        }
        Ok(Node::Seq(v))
    }

    fn num(&mut self) -> Option<usize> {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        let s: String = self.s.get(start..self.i)?.iter().collect();
        s.parse().ok()
    }

    fn quant(&mut self, atom: Node) -> Result<Node, String> {
        let (lo, hi) = match self.peek() {
            Some('*') => (0, usize::MAX),
            Some('+') => (1, usize::MAX),
            Some('?') => (0, 1),
            Some('{') => {
                let save = self.i;
                self.i += 1;
                let Some(lo) = self.num() else {
                    self.i = save;
                    return Ok(atom);
                };
                let hi = if self.peek() == Some(',') {
                    self.i += 1;
                    self.num().unwrap_or(usize::MAX)
                } else {
                    lo
                };
                if self.peek() != Some('}') {
                    self.i = save;
                    return Ok(atom);
                }
                (lo, hi)
            }
            _ => return Ok(atom),
        };
        if matches!(atom, Node::Start | Node::End | Node::WordB(_)) {
            return Err(String::from("Nothing to repeat"));
        }
        self.i += 1;
        let lazy = self.peek() == Some('?');
        if lazy {
            self.i += 1;
        }
        Ok(Node::Repeat(Box::new(atom), lo, hi, !lazy))
    }

    fn escape_class(c: char) -> Option<(Vec<(char, char)>, bool)> {
        Some(match c {
            'd' => (DIGIT.to_vec(), false),
            'D' => (DIGIT.to_vec(), true),
            'w' => (WORD.to_vec(), false),
            'W' => (WORD.to_vec(), true),
            's' => (SPACE.to_vec(), false),
            'S' => (SPACE.to_vec(), true),
            _ => return None,
        })
    }

    fn escaped_char(c: char) -> char {
        match c {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            _ => c,
        }
    }

    fn atom(&mut self) -> Result<Node, String> {
        let c = self.peek().ok_or_else(|| String::from("Unexpected end"))?;
        self.i += 1;
        Ok(match c {
            '.' => Node::Any,
            '^' => Node::Start,
            '$' => Node::End,
            '(' => {
                let cap = if self.s.get(self.i..self.i + 2) == Some(&['?', ':']) {
                    self.i += 2;
                    None
                } else {
                    self.groups += 1;
                    Some(self.groups)
                };
                let inner = self.alt()?;
                if self.peek() != Some(')') {
                    return Err(String::from("Unclosed group"));
                }
                self.i += 1;
                Node::Group(Box::new(inner), cap)
            }
            '[' => self.class()?,
            '\\' => {
                let e = self
                    .peek()
                    .ok_or_else(|| String::from("Trailing backslash"))?;
                self.i += 1;
                if let Some((r, neg)) = Self::escape_class(e) {
                    Node::Class(r, neg)
                } else if e == 'b' || e == 'B' {
                    Node::WordB(e == 'b')
                } else {
                    Node::Char(Self::escaped_char(e))
                }
            }
            '*' | '+' | '?' => return Err(String::from("Nothing to repeat")),
            c => Node::Char(c),
        })
    }

    fn class(&mut self) -> Result<Node, String> {
        let neg = self.peek() == Some('^');
        if neg {
            self.i += 1;
        }
        let mut ranges = Vec::new();
        let mut first = true;
        loop {
            let c = self.peek().ok_or_else(|| String::from("Unclosed class"))?;
            self.i += 1;
            if c == ']' && !first {
                break;
            }
            first = false;
            let lo = if c == '\\' {
                let e = self.peek().ok_or_else(|| String::from("Unclosed class"))?;
                self.i += 1;
                if let Some((r, false)) = Self::escape_class(e) {
                    ranges.extend(r);
                    continue;
                }
                Self::escaped_char(e)
            } else {
                c
            };
            if self.peek() == Some('-') && self.s.get(self.i + 1).is_some_and(|n| *n != ']') {
                self.i += 1;
                let mut hi = self.peek().unwrap_or(lo);
                self.i += 1;
                if hi == '\\' {
                    hi = Self::escaped_char(self.peek().unwrap_or('\\'));
                    self.i += 1;
                }
                ranges.push((lo, hi));
            } else {
                ranges.push((lo, lo));
            }
        }
        Ok(Node::Class(ranges, neg))
    }
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn is_word(c: Option<&char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric() || *c == '_')
}

struct M<'a> {
    s: &'a [char],
    ci: bool,
    caps: Vec<Option<(usize, usize)>>,
    steps: usize,
}

const STEP_LIMIT: usize = 200_000;

impl M<'_> {
    fn class_has(&self, r: &[(char, char)], c: char) -> bool {
        let hit = |c: char| r.iter().any(|(a, b)| c >= *a && c <= *b);
        hit(c) || (self.ci && (hit(fold(c)) || c.to_uppercase().any(hit)))
    }

    /// Match `n` at `i`, then the continuation `k`. Returns where the whole
    /// match ends.
    fn m(
        &mut self,
        n: &Node,
        i: usize,
        k: &mut dyn FnMut(&mut Self, usize) -> Option<usize>,
    ) -> Option<usize> {
        self.steps += 1;
        if self.steps > STEP_LIMIT {
            return None;
        }
        match n {
            Node::Char(c) => {
                let x = *self.s.get(i)?;
                let ok = x == *c || (self.ci && fold(x) == fold(*c));
                if ok {
                    k(self, i + 1)
                } else {
                    None
                }
            }
            Node::Any => {
                let x = *self.s.get(i)?;
                if x == '\n' {
                    return None;
                }
                k(self, i + 1)
            }
            Node::Class(r, neg) => {
                let x = *self.s.get(i)?;
                if self.class_has(r, x) != *neg {
                    k(self, i + 1)
                } else {
                    None
                }
            }
            Node::Start => {
                if i == 0 {
                    k(self, i)
                } else {
                    None
                }
            }
            Node::End => {
                if i == self.s.len() {
                    k(self, i)
                } else {
                    None
                }
            }
            Node::WordB(want) => {
                let before = i > 0 && is_word(self.s.get(i - 1));
                let after = is_word(self.s.get(i));
                if (before != after) == *want {
                    k(self, i)
                } else {
                    None
                }
            }
            Node::Group(inner, cap) => {
                let cap = *cap;
                self.m(inner, i, &mut |me: &mut Self, j| {
                    let saved = cap.and_then(|g| me.caps.get(g).copied().flatten());
                    if let Some(g) = cap {
                        if let Some(slot) = me.caps.get_mut(g) {
                            *slot = Some((i, j));
                        }
                    }
                    let r = k(me, j);
                    if r.is_none() {
                        if let Some(g) = cap {
                            if let Some(slot) = me.caps.get_mut(g) {
                                *slot = saved;
                            }
                        }
                    }
                    r
                })
            }
            Node::Alt(arms) => {
                for a in arms {
                    if let Some(e) = self.m(a, i, k) {
                        return Some(e);
                    }
                }
                None
            }
            Node::Seq(v) => self.seq(v, 0, i, k),
            Node::Repeat(inner, lo, hi, greedy) => {
                self.rep(inner, *lo, *hi, *greedy, 0, i, usize::MAX, k)
            }
        }
    }

    fn seq(
        &mut self,
        v: &[Node],
        at: usize,
        i: usize,
        k: &mut dyn FnMut(&mut Self, usize) -> Option<usize>,
    ) -> Option<usize> {
        match v.get(at) {
            None => k(self, i),
            Some(n) => self.m(n, i, &mut |me: &mut Self, j| me.seq(v, at + 1, j, k)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn rep(
        &mut self,
        n: &Node,
        lo: usize,
        hi: usize,
        greedy: bool,
        count: usize,
        i: usize,
        last: usize,
        k: &mut dyn FnMut(&mut Self, usize) -> Option<usize>,
    ) -> Option<usize> {
        // An empty iteration ends the loop, so `(a*)*` cannot spin.
        let can_more = count < hi && i != last;
        if count >= lo && !greedy {
            if let Some(e) = k(self, i) {
                return Some(e);
            }
        }
        if can_more {
            if let Some(e) = self.m(n, i, &mut |me: &mut Self, j| {
                me.rep(n, lo, hi, greedy, count + 1, j, i, k)
            }) {
                return Some(e);
            }
        }
        if count >= lo && greedy {
            return k(self, i);
        }
        None
    }
}

/// A match: where it starts and ends (in chars), and its groups.
pub type Found = (usize, usize, Vec<Option<(usize, usize)>>);

impl Regex {
    pub fn new(pattern: &str, case_insensitive: bool) -> Result<Regex, String> {
        let chars: Vec<char> = pattern.chars().collect();
        let mut p = P {
            s: &chars,
            i: 0,
            groups: 0,
        };
        let root = p.alt()?;
        if p.i < chars.len() {
            return Err(String::from("Unmatched )"));
        }
        Ok(Regex {
            root,
            groups: p.groups,
            ci: case_insensitive,
        })
    }

    /// The first match in `s` at or after char `from`.
    pub fn find_chars(&self, s: &[char], from: usize) -> Option<Found> {
        let mut m = M {
            s,
            ci: self.ci,
            caps: alloc::vec![None; self.groups + 1],
            steps: 0,
        };
        for start in from..=s.len() {
            m.caps.iter_mut().for_each(|c| *c = None);
            if let Some(end) = m.m(&self.root, start, &mut |_, j| Some(j)) {
                let mut caps = m.caps.clone();
                if let Some(c0) = caps.get_mut(0) {
                    *c0 = Some((start, end));
                }
                return Some((start, end, caps));
            }
            if m.steps > STEP_LIMIT {
                return None;
            }
        }
        None
    }

    /// Every match in a line, as byte ranges with their group texts.
    pub fn find_all(&self, line: &str) -> Vec<(usize, usize, Vec<Option<String>>)> {
        let chars: Vec<char> = line.chars().collect();
        let mut byte_at: Vec<usize> = line.char_indices().map(|(i, _)| i).collect();
        byte_at.push(line.len());
        let b = |i: usize| byte_at.get(i).copied().unwrap_or(line.len());
        let mut out = Vec::new();
        let mut from = 0;
        while from <= chars.len() {
            let Some((s, e, caps)) = self.find_chars(&chars, from) else {
                break;
            };
            let texts = caps
                .iter()
                .map(|c| c.map(|(a, z)| String::from(line.get(b(a)..b(z)).unwrap_or(""))))
                .collect();
            out.push((b(s), b(e), texts));
            from = if e > s { e } else { e + 1 };
            if out.len() > 10_000 {
                break;
            }
        }
        out
    }
}

/// The replacement for one match: `$&` the match, `$1`..`$9` its groups,
/// `$$` a dollar.
pub fn expand(template: &str, groups: &[Option<String>]) -> String {
    let mut out = String::new();
    let mut it = template.chars().peekable();
    while let Some(c) = it.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match it.peek().copied() {
            Some('$') => {
                it.next();
                out.push('$');
            }
            Some('&') => {
                it.next();
                out.push_str(groups.first().and_then(|g| g.as_deref()).unwrap_or(""));
            }
            Some(d) if d.is_ascii_digit() && d != '0' => {
                it.next();
                let i = (d as u8 - b'0') as usize;
                out.push_str(groups.get(i).and_then(|g| g.as_deref()).unwrap_or(""));
            }
            _ => out.push('$'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(p: &str, s: &str) -> Vec<String> {
        Regex::new(p, false)
            .unwrap()
            .find_all(s)
            .into_iter()
            .map(|(a, b, _)| String::from(&s[a..b]))
            .collect()
    }

    #[test]
    fn regexes_find_what_javascript_finds() {
        assert_eq!(all("a+", "caaab a"), ["aaa", "a"]);
        assert_eq!(all(r"\d{2,3}", "1 22 4444"), ["22", "444"]);
        assert_eq!(all(r"\bcat\b", "cat concat cat."), ["cat", "cat"]);
        assert_eq!(all("colou?r", "color colour"), ["color", "colour"]);
        assert_eq!(all("(a|b)c", "ac bc cc"), ["ac", "bc"]);
        assert_eq!(all("<.+?>", "<a><b>"), ["<a>", "<b>"]);
        assert_eq!(all("<.+>", "<a><b>"), ["<a><b>"]);
        assert_eq!(all("^#+ ", "## Head"), ["## "]);
        assert_eq!(all("end$", "the end"), ["end"]);
        assert_eq!(all("[^ ]+", "ab cd"), ["ab", "cd"]);
        assert_eq!(all(r"[\w-]+@\w+", "mail: a-b@x ok"), ["a-b@x"]);
        assert_eq!(all("é+", "ééa"), ["éé"]);
        assert_eq!(all("x*", "ab").len(), 3, "empty matches step on");
        assert_eq!(all("(a*)*b", "aaab"), ["aaab"]);
        let ci = Regex::new("hello", true).unwrap();
        assert_eq!(ci.find_all("HeLLo hello").len(), 2);
        for bad in ["(", "a)", "*a", "[a", "\\"] {
            assert!(Regex::new(bad, false).is_err(), "{bad}");
        }
    }

    #[test]
    fn replacements_use_groups() {
        let r = Regex::new(r"(\w+)@(\w+)", false).unwrap();
        let m = r.find_all("ann@home");
        assert_eq!(expand("$2/$1 ($&) $$", &m[0].2), "home/ann (ann@home) $");
        // A pathological pattern gives up rather than hanging.
        let slow = Regex::new("(a+)+b", false).unwrap();
        let _ = slow.find_all(&"a".repeat(40));
    }
}

//! Ports text/entities.ts: Mermaid's `#35;` / `#quot;` codes (hidden
//! behind placeholders by `encodeEntities`) and HTML entities, turned into
//! the characters they name.

use alloc::string::String;

use crate::pele::js;

const NAMED: &str = concat!(
    "amp:38,lt:60,gt:62,quot:34,apos:39,nbsp:160,shy:173,iexcl:161,cent:162,pound:163,curren:164,yen:165,sect:167,",
    "copy:169,laquo:171,not:172,reg:174,deg:176,plusmn:177,sup2:178,sup3:179,micro:181,para:182,middot:183,",
    "sup1:185,raquo:187,frac14:188,frac12:189,frac34:190,iquest:191,times:215,divide:247,ndash:8211,mdash:8212,",
    "lsquo:8216,rsquo:8217,sbquo:8218,ldquo:8220,rdquo:8221,bdquo:8222,dagger:8224,Dagger:8225,bull:8226,",
    "hellip:8230,permil:8240,prime:8242,Prime:8243,lsaquo:8249,rsaquo:8250,euro:8364,trade:8482,larr:8592,",
    "uarr:8593,rarr:8594,darr:8595,harr:8596,lArr:8656,uArr:8657,rArr:8658,dArr:8659,hArr:8660,forall:8704,",
    "part:8706,exist:8707,empty:8709,nabla:8711,isin:8712,notin:8713,ni:8715,prod:8719,sum:8721,minus:8722,",
    "radic:8730,infin:8734,and:8743,or:8744,cap:8745,cup:8746,int:8747,there4:8756,sim:8764,cong:8773,",
    "asymp:8776,ne:8800,equiv:8801,le:8804,ge:8805,sub:8834,sup:8835,sube:8838,supe:8839,oplus:8853,otimes:8855,",
    "perp:8869,sdot:8901,loz:9674,spades:9824,clubs:9827,hearts:9829,diams:9830,check:10003,cross:10007,",
    "AMP:38,LT:60,GT:62,QUOT:34,lsqb:91,rsqb:93,lcub:123,rcub:125,verbar:124,VerticalLine:124,midast:42,",
    "DiacriticalGrave:96,Hat:94,UnderBar:95,",
    "equals:61,colon:58,semi:59,num:35,percnt:37,lpar:40,rpar:41,lbrack:91,rbrack:93,lbrace:123,rbrace:125,",
    "vert:124,sol:47,bsol:92,quest:63,excl:33,ast:42,plus:43,comma:44,period:46,dollar:36,commat:64,grave:96,",
    "hat:94,lowbar:95,tilde:126,Alpha:913,Beta:914,Gamma:915,Delta:916,Epsilon:917,Zeta:918,Eta:919,Theta:920,",
    "Iota:921,Kappa:922,Lambda:923,Mu:924,Nu:925,Xi:926,Omicron:927,Pi:928,Rho:929,Sigma:931,Tau:932,",
    "Upsilon:933,Phi:934,Chi:935,Psi:936,Omega:937,alpha:945,beta:946,gamma:947,delta:948,epsilon:949,zeta:950,",
    "eta:951,theta:952,iota:953,kappa:954,lambda:955,mu:956,nu:957,xi:958,omicron:959,pi:960,rho:961,sigmaf:962,",
    "sigma:963,tau:964,upsilon:965,phi:966,chi:967,psi:968,omega:969"
);

/// Ports `lookup`.
fn lookup(name: &str) -> Option<char> {
    for pair in NAMED.split(',') {
        let (k, v) = pair.split_once(':')?;
        if k == name {
            return char::from_u32(v.parse().ok()?);
        }
    }
    None
}

/// Ports `codePoint`: the character, or `fallback` for no character.
fn code_point(n: f64, fallback: &str, out: &mut String) {
    if n > 0.0 && n <= 0x10ffff as f64 && !(0xd800 as f64..=0xdfff as f64).contains(&n) {
        if let Some(c) = char::from_u32(n as u32) {
            out.push(c);
            return;
        }
    }
    out.push_str(fallback);
}

/// The placeholder `encodeEntities` opens a code with (`OPEN`).
pub const OPEN: &str = "\u{fb02}\u{b0}";
/// ... and closes it with (`CLOSE`).
pub const CLOSE: &str = "\u{b6}\u{df}";

/// Ports `restorePlaceholders`: Mermaid's placeholders, from `ﬂ°` to `¶ß`
/// with a second `°` marking a numeric code, back into entities, so that
/// each code is decoded once.
fn restore_placeholders(text: &str) -> String {
    let mut out = String::new();
    let mut last = 0usize;
    let mut from = text.find(OPEN);
    while let Some(f) = from {
        let after = f + OPEN.len();
        let stop = match text[after..].find('\u{b6}') {
            Some(s) => after + s,
            None => break,
        };
        let close = stop + '\u{b6}'.len_utf8();
        if text[close..].starts_with('\u{df}') {
            let numeric = text[after..].starts_with('\u{b0}');
            out.push_str(&text[last..f]);
            out.push_str(if numeric { "&#" } else { "&" });
            let body_start = if numeric {
                after + '\u{b0}'.len_utf8()
            } else {
                after
            };
            // JS slices `from + 3` to `stop`; a body that starts past the stop is empty.
            if body_start <= stop {
                out.push_str(&text[body_start..stop]);
            }
            out.push(';');
            last = close + '\u{df}'.len_utf8();
        }
        from = text[close..].find(OPEN).map(|k| close + k);
    }
    if last == 0 {
        return String::from(text);
    }
    out.push_str(&text[last..]);
    out
}

/// Ports `writtenCodes`: placeholders written back as `#code;`, for error
/// messages.
pub fn written_codes(text: &str) -> String {
    if !text.contains(CLOSE) {
        return String::from(text);
    }
    // RE_PLACEHOLDER = /ﬂ°°?(\w+)¶ß/g, replaced with '#$1;'.
    let mut out = String::new();
    let mut i = 0;
    while i < text.len() {
        if text[i..].starts_with(OPEN) {
            let mut j = i + OPEN.len();
            if text[j..].starts_with('\u{b0}') {
                j += '\u{b0}'.len_utf8();
            }
            let w = text[j..]
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                .count();
            if w > 0 && text[j + w..].starts_with(CLOSE) {
                out.push('#');
                out.push_str(&text[j..j + w]);
                out.push(';');
                i = j + w + CLOSE.len();
                continue;
            }
        }
        let c = text[i..].chars().next().unwrap_or(' ');
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// Ports `decodeEntities`: Mermaid's `#35;` / `#quot;` codes (as hidden by
/// encodeEntities) and HTML entities, turned into characters.
pub fn decode_entities(text: &str) -> String {
    let restored;
    let text = if text.contains(CLOSE) {
        restored = restore_placeholders(text);
        restored.as_str()
    } else {
        text
    };
    if !text.contains('&') {
        return String::from(text);
    }
    // RE_ENTITY = /&(?:#(\d+)|#[xX]([0-9a-fA-F]+)|(\w+));/g
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'&' {
            i += 1;
            continue;
        }
        let mut matched: Option<(usize, u8, usize, usize)> = None; // end, kind, body start, body end
        if b.get(i + 1) == Some(&b'#') {
            let s = i + 2;
            let mut e = s;
            while e < b.len() && b[e].is_ascii_digit() {
                e += 1;
            }
            if e > s && b.get(e) == Some(&b';') {
                matched = Some((e + 1, 0, s, e));
            } else if matches!(b.get(i + 2), Some(b'x') | Some(b'X')) {
                let s = i + 3;
                let mut e = s;
                while e < b.len() && b[e].is_ascii_hexdigit() {
                    e += 1;
                }
                if e > s && b.get(e) == Some(&b';') {
                    matched = Some((e + 1, 1, s, e));
                }
            }
        }
        if matched.is_none() {
            let s = i + 1;
            let mut e = s;
            while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_') {
                e += 1;
            }
            if e > s && b.get(e) == Some(&b';') {
                matched = Some((e + 1, 2, s, e));
            }
        }
        let Some((end, kind, s, e)) = matched else {
            i += 1;
            continue;
        };
        out.push_str(&text[last..i]);
        let m = &text[i..end];
        let body = &text[s..e];
        match kind {
            0 => code_point(js::number(body), m, &mut out),
            1 => code_point(js::parse_int(body, 16), m, &mut out),
            _ => match lookup(body) {
                Some(c) => out.push(c),
                None => out.push_str(m),
            },
        }
        last = end;
        i = end;
    }
    out.push_str(&text[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes() {
        assert_eq!(
            decode_entities("a &amp; b &#35; &#x41; &nope; &#12a;"),
            "a & b # A &nope; &#12a;"
        );
        let hidden = alloc::format!("x{}quot{}y", OPEN, CLOSE);
        assert_eq!(decode_entities(&hidden), "x\"y");
        let num = alloc::format!("{}\u{b0}35{}", OPEN, CLOSE);
        assert_eq!(decode_entities(&num), "#");
        assert_eq!(written_codes(&num), "#35;");
    }
}

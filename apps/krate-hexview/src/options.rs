use crate::hexyl::{Base, BorderStyle, CharacterTable, ColorScheme, Endianness};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
pub struct Options {
    pub path: Option<String>,
    pub skip: i64,
    pub length: u64,
    pub offset: u64,
    pub panels: u64,
    pub group: u8,
    pub color: bool,
    pub chars: bool,
    pub position: bool,
    pub squeeze: bool,
    pub border: BorderStyle,
    pub base: Base,
    pub endian: Endianness,
    pub table: CharacterTable,
    pub scheme: ColorScheme,
    pub include: bool,
    pub help: bool,
    pub version: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            path: None,
            skip: 0,
            length: u64::MAX,
            offset: 0,
            panels: 2,
            group: 1,
            color: true,
            chars: true,
            position: true,
            squeeze: true,
            border: BorderStyle::Unicode,
            base: Base::Hexadecimal,
            endian: Endianness::Big,
            table: CharacterTable::Default,
            scheme: ColorScheme::Default,
            include: false,
            help: false,
            version: false,
        }
    }
}
pub fn count(s: &str, block: u64) -> Result<i64, String> {
    let (negative, s) = if let Some(s) = s.strip_prefix('-') {
        (true, s)
    } else {
        (false, s.strip_prefix('+').unwrap_or(s))
    };
    let lower = s.to_ascii_lowercase();
    let (number, mult) = if lower.starts_with("0x") {
        (
            u64::from_str_radix(&lower[2..], 16).map_err(|_| format!("Invalid byte count: {s}"))?,
            1,
        )
    } else {
        let end = lower
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(lower.len());
        let n = lower[..end]
            .parse::<u64>()
            .map_err(|_| format!("Invalid byte count: {s}"))?;
        let m = match &lower[end..] {
            "" => 1,
            "kb" => 1000,
            "mb" => 1_000_000,
            "gb" => 1_000_000_000,
            "tb" => 1_000_000_000_000,
            "kib" => 1024,
            "mib" => 1024 * 1024,
            "gib" => 1024 * 1024 * 1024,
            "tib" => 1024 * 1024 * 1024 * 1024,
            "block" | "blocks" => block,
            _ => return Err(format!("Unsupported byte unit: {s}")),
        };
        (n, m)
    };
    let n = number
        .checked_mul(mult)
        .filter(|v| *v <= i64::MAX as u64)
        .ok_or("Byte count overflow")? as i64;
    Ok(if negative { -n } else { n })
}
pub fn parse(args: &[&str]) -> Result<Options, String> {
    // Clap-compatible attached short values and flag clusters, without
    // importing a native CLI framework into the no_std component.
    let mut expanded: Vec<String> = Vec::new();
    let mut positional = false;
    for a in args {
        if *a == "--" {
            positional = true;
        }
        if !positional
            && a.starts_with('-')
            && !a.starts_with("--")
            && a.len() > 2
            && a.as_bytes()[1].is_ascii_alphabetic()
        {
            let bytes = a.as_bytes();
            let mut j = 1;
            while j < bytes.len() {
                let flag = bytes[j] as char;
                expanded.push(format!("-{flag}"));
                if "nclsogb".contains(flag) {
                    if j + 1 < bytes.len() {
                        expanded.push(a[j + 1..].trim_start_matches('=').into());
                    }
                    break;
                }
                j += 1;
            }
        } else {
            expanded.push(a.to_string());
        }
    }
    let args: Vec<&str> = expanded.iter().map(String::as_str).collect();
    let mut o = Options::default();
    let mut i = 0;
    let mut pending: Vec<(String, String)> = Vec::new();
    let mut block = 512;
    let mut plain = false;
    let mut explicit_color = false;
    let mut explicit_border = false;
    let mut explicit_panels = false;
    let mut terminal_width: Option<u64> = None;
    let mut explicit_endian = false;
    while i < args.len() {
        let a = args[i];
        i += 1;
        if a == "--" {
            for p in &args[i..] {
                if o.path.is_some() {
                    return Err("Only one input file is accepted".into());
                }
                o.path = Some(p.to_string())
            }
            break;
        }
        if a == "-" || !a.starts_with('-') {
            if o.path.replace(a.into()).is_some() {
                return Err("Only one input file is accepted".into());
            }
            continue;
        }
        match a {
            "-h" | "--help" => {
                o.help = true;
                continue;
            }
            "-V" | "--version" => {
                o.version = true;
                continue;
            }
            "-v" | "--no-squeezing" => {
                o.squeeze = false;
                continue;
            }
            "--no-characters" => {
                o.chars = false;
                continue;
            }
            "-C" | "--characters" => {
                o.chars = true;
                continue;
            }
            "-P" | "--no-position" => {
                o.position = false;
                continue;
            }
            "-p" | "--plain" => {
                plain = true;
                continue;
            }
            "-e" => {
                o.endian = Endianness::Little;
                explicit_endian = true;
                continue;
            }
            "-i" | "--include" => {
                o.include = true;
                continue;
            }
            _ => {}
        }
        let (k, v) = if let Some(pair) = a.split_once('=') {
            pair
        } else {
            let v = *args
                .get(i)
                .ok_or_else(|| format!("Missing value for {a}"))?;
            i += 1;
            (a, v)
        };
        match k {
            "-n" | "-c" | "-l" | "--length" | "--bytes" | "-s" | "--skip" | "-o"
            | "--display-offset" => pending.push((k.into(), v.into())),
            "--block-size" => {
                if v.to_ascii_lowercase().contains("block") {
                    return Err("Block size cannot use blocks".into());
                }
                block = count(v, 512)?
                    .try_into()
                    .map_err(|_| "Block size must be positive")?;
                if block == 0 {
                    return Err("Block size must be positive".into());
                }
            }
            "--panels" => {
                explicit_panels = true;
                o.panels = v
                    .parse()
                    .map_err(|_| "Use a numeric panel count, not terminal auto-detection")?;
                if !(1..=16).contains(&o.panels) {
                    return Err("Panels must be 1..16".into());
                }
            }
            "--terminal-width" => {
                let n: u64 = v.parse().map_err(|_| "Terminal width must be positive")?;
                if n == 0 {
                    return Err("Terminal width must be positive".into());
                }
                terminal_width = Some(n);
            }
            "-g" | "--group-size" | "--groupsize" => {
                o.group = v.parse().map_err(|_| "Invalid group size")?;
                if ![1, 2, 4, 8].contains(&o.group) {
                    return Err("Group size must be 1, 2, 4 or 8".into());
                }
            }
            "--color" => {
                explicit_color = true;
                o.color =
                    match v {
                        "always" | "force" => true,
                        "never" => false,
                        _ => return Err(
                            "Use --color always or never; terminal auto-detection is not exposed"
                                .into(),
                        ),
                    }
            }
            "--border" => {
                explicit_border = true;
                o.border = match v {
                    "unicode" => BorderStyle::Unicode,
                    "ascii" => BorderStyle::Ascii,
                    "none" => BorderStyle::None,
                    _ => return Err("Invalid border".into()),
                }
            }
            "--endianness" => {
                explicit_endian = true;
                o.endian = match v {
                    "little" => Endianness::Little,
                    "big" => Endianness::Big,
                    _ => return Err("Invalid endianness".into()),
                }
            }
            "--character-table" => {
                o.table = match v {
                    "default" => CharacterTable::Default,
                    "ascii" => CharacterTable::Ascii,
                    "braille" => CharacterTable::Braille,
                    _ => return Err("This build supports default, ascii and braille tables".into()),
                }
            }
            "--color-scheme" => {
                o.scheme = match v {
                    "default" => ColorScheme::Default,
                    "gradient" => ColorScheme::Gradient,
                    _ => return Err("Invalid color scheme".into()),
                }
            }
            "-b" | "--base" => {
                o.base = match v {
                    "2" | "b" | "bin" | "binary" => Base::Binary,
                    "8" | "o" | "oct" | "octal" => Base::Octal,
                    "10" | "d" | "dec" | "decimal" => Base::Decimal,
                    "16" | "x" | "hex" | "hexadecimal" => Base::Hexadecimal,
                    _ => return Err("Invalid base".into()),
                }
            }
            _ => return Err(format!("Unsupported option {k}; use --help")),
        }
    }
    if plain {
        o.chars = false;
        o.position = false;
        if !explicit_color {
            o.color = false;
        }
        if !explicit_border {
            o.border = BorderStyle::None;
        }
    }
    if o.include && explicit_endian {
        return Err("--include conflicts with endianness".into());
    }
    if let Some(width) = terminal_width {
        if explicit_panels {
            return Err("--terminal-width conflicts with --panels".into());
        }
        let digits = match o.base {
            Base::Binary => 8,
            Base::Octal | Base::Decimal => 3,
            Base::Hexadecimal => 2,
        };
        let column =
            (8 / o.group as u64) * (digits * o.group as u64 + 1) + 2 + if o.chars { 8 } else { 0 };
        o.panels = (width.saturating_sub(if o.position { 10 } else { 1 }) / column)
            .max(1)
            .min(16);
    }
    for (k, v) in pending {
        let n = count(&v, block)?;
        match k.as_str() {
            "-s" | "--skip" => o.skip = n,
            "-o" | "--display-offset" => {
                o.offset = n
                    .try_into()
                    .map_err(|_| "Display offset must be nonnegative")?
            }
            _ => o.length = n.try_into().map_err(|_| "Length must be nonnegative")?,
        }
    }
    Ok(o)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn units() {
        assert_eq!(count("4KiB", 512), Ok(4096));
        assert_eq!(count("-0x20", 512), Ok(-32));
        assert_eq!(count("2block", 1024), Ok(2048));
        assert_eq!(count("1TB", 512), Ok(1_000_000_000_000));
        assert_eq!(count("1TiB", 512), Ok(1_099_511_627_776));
        assert!(count("2b", 512).is_err());
        assert!(count("99999999999999999999999", 512).is_err());
    }
    #[test]
    fn explicit_settings_override_plain_in_either_order() {
        for args in [
            ["--color=always", "--border=ascii", "--plain"],
            ["--plain", "--color=always", "--border=ascii"],
        ] {
            let o = parse(&args).unwrap();
            assert!(o.color);
            assert!(matches!(o.border, BorderStyle::Ascii));
            assert!(!o.chars && !o.position);
        }
        let o = parse(&["-Pv", "-n32", "-g2", "file"]).unwrap();
        assert_eq!(o.length, 32);
        assert_eq!(o.group, 2);
        assert!(!o.position && !o.squeeze);
        assert_eq!(parse(&["--terminal-width=80", "-b2"]).unwrap().panels, 1);
    }
    #[test]
    fn ranges() {
        let o = parse(&["--skip=-16", "--length", "0xff", "--color=never", "file"]).unwrap();
        assert_eq!(o.skip, -16);
        assert_eq!(o.length, 255);
        assert!(!o.color);
    }
    #[test]
    fn reject() {
        assert!(parse(&["--panels=0"]).is_err());
        assert!(parse(&["-g", "3"]).is_err());
        assert!(parse(&["--color=auto"]).is_err());
        assert!(parse(&["--length=-2"]).is_err());
        assert!(parse(&["a", "b"]).is_err());
    }
}

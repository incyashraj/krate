//! Ports diagrams/common/generics.ts: `List~int~` as `List<int>`.

use alloc::string::String;
use alloc::vec::Vec;

/// Ports `tildes`.
fn tildes(text: &str) -> usize {
    text.matches('~').count()
}

/// Ports `pairTildes`: pairs tildes from the outside in, the first with
/// the last, the second with the one before it.
fn pair_tildes(text: &str) -> String {
    let count = tildes(text);
    if count <= 1 {
        return text.into();
    }
    let lead = !count.is_multiple_of(2) && text.starts_with('~');
    let mut chars: Vec<char> = text.chars().collect();
    let mut at: Vec<usize> = Vec::new();
    for (i, c) in chars.iter().enumerate().skip(if lead { 1 } else { 0 }) {
        if *c == '~' {
            at.push(i);
        }
    }
    let (mut lo, mut hi) = (0usize, at.len() as isize - 1);
    while (lo as isize) < hi {
        chars[at[lo]] = '<';
        chars[at[hi as usize]] = '>';
        lo += 1;
        hi -= 1;
    }
    chars.into_iter().collect()
}

/// Ports `parseGenericTypes`: turns `List~int~` into `List<int>`, with
/// the quirks of Mermaid's parseGenericTypes around commas.
pub fn parse_generic_types(input: &str) -> String {
    if !input.contains('~') {
        return input.into();
    }
    // input.split(/(,)/): the parts with each comma between them.
    let mut sets: Vec<&str> = Vec::new();
    for (i, part) in input.split(',').enumerate() {
        if i > 0 {
            sets.push(",");
        }
        sets.push(part);
    }
    let mut output: Vec<String> = Vec::new();
    let mut i = 0;
    while i < sets.len() {
        let mut set = String::from(sets[i]);
        if sets[i] == ","
            && i > 0
            && i + 1 < sets.len()
            && tildes(sets[i - 1]) == 1
            && tildes(sets[i + 1]) == 1
        {
            set = String::from(sets[i - 1]) + "," + sets[i + 1];
            i += 1;
            output.pop();
        }
        output.push(pair_tildes(&set));
        i += 1;
    }
    output.concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tildes_pair_as_mermaid_pairs_them() {
        assert_eq!(parse_generic_types("List~int~"), "List<int>");
        assert_eq!(parse_generic_types("Map~K, V~"), "Map<K, V>");
        assert_eq!(parse_generic_types("a~b~c~d~"), "a<b<c>d>");
        assert_eq!(parse_generic_types("plain"), "plain");
    }
}

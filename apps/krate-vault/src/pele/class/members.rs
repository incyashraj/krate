//! Ports diagrams/class/members.ts: a class member (attribute or method)
//! split into its parts.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::pele::common::chars::is_line_end;
use crate::pele::common::generics::parse_generic_types;
use crate::pele::js::{is_space, trim};

/// Ports `isVisibility`.
fn is_visibility(ch: char) -> bool {
    matches!(ch, '#' | '+' | '~' | '-')
}

/// Ports `class ClassMember`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassMember {
    pub id: String,
    /// "method" or "attribute".
    pub member_type: &'static str,
    pub visibility: String,
    pub classifier: String,
    pub parameters: String,
    pub return_type: String,
}

impl ClassMember {
    pub fn new(input: &str, member_type: &'static str) -> ClassMember {
        let mut m = ClassMember {
            id: String::new(),
            member_type,
            visibility: String::new(),
            classifier: String::new(),
            parameters: String::new(),
            return_type: String::new(),
        };
        // Mermaid throws a TypeError for a method with no opening parenthesis, such as `run)`.
        // It is kept here as an attribute, so it is shown as written.
        if member_type != "method" || !m.parse_method(input) {
            m.parse_attribute(input);
        }
        m.id = if m.id.starts_with(' ') {
            format!(" {}", trim(&m.id))
        } else {
            trim(&m.id).into()
        };
        m
    }

    /// Ports `parseMethod`: the same split as Mermaid's
    /// /([#+~-])?(.+)\((.*)\)([\s$*])?(.*)([$*])?/, found in one pass.
    fn parse_method(&mut self, input: &str) -> bool {
        let chars: Vec<char> = input.chars().collect();
        let n = chars.len();
        let s = |a: usize, b: usize| -> String { chars[a..b.max(a)].iter().collect() };
        let mut start = 0;
        while start < n {
            let mut end = start;
            while end < n && !is_line_end(chars[end] as u32) {
                end += 1;
            }
            let mut close = end as isize - 1;
            while close > start as isize && chars[close as usize] != ')' {
                close -= 1;
            }
            let mut open = close - 1;
            while open > start as isize && chars[open as usize] != '(' {
                open -= 1;
            }
            if open > start as isize {
                let (open, close) = (open as usize, close as usize);
                let first = chars[start];
                let visible = is_visibility(first) && open > start + 1;
                if visible {
                    self.visibility = first.into();
                }
                self.id = s(if visible { start + 1 } else { start }, open);
                self.parameters = trim(&s(open + 1, close)).into();
                let mut rest = close + 1;
                let mut classifier = String::new();
                if rest < n {
                    let next = chars[rest];
                    if next == '$' || next == '*' || is_space(next) {
                        if !is_space(next) {
                            classifier.push(next);
                        }
                        rest += 1;
                    }
                }
                let mut stop = rest;
                while stop < n && !is_line_end(chars[stop] as u32) {
                    stop += 1;
                }
                self.return_type = trim(&s(rest, stop)).into();
                if classifier.is_empty() {
                    if let Some(last) = self.return_type.chars().last() {
                        if last == '$' || last == '*' {
                            classifier.push(last);
                            self.return_type.pop();
                        }
                    }
                }
                self.classifier = classifier;
                return true;
            }
            start = end + 1;
        }
        self.member_type = "attribute";
        false
    }

    /// Ports `parseAttribute`.
    fn parse_attribute(&mut self, input: &str) {
        let chars: Vec<char> = input.chars().collect();
        if let Some(&first) = chars.first() {
            if is_visibility(first) {
                self.visibility = first.into();
            }
        }
        if let Some(&last) = chars.last() {
            if last == '$' || last == '*' {
                self.classifier = last.into();
            }
        }
        let a = if self.visibility.is_empty() { 0 } else { 1 };
        let b = if self.classifier.is_empty() {
            chars.len()
        } else {
            chars.len() - 1
        };
        // `substring` takes its ends in either order.
        let (a, b) = (a.min(b), a.max(b));
        self.id = chars[a..b].iter().collect();
    }

    /// Ports `getDisplayDetails`: the text shown, and its CSS style.
    pub fn display_details(&self) -> (String, &'static str) {
        let mut text = self.visibility.clone() + &parse_generic_types(&self.id);
        if self.member_type == "method" {
            text.push_str(&format!(
                "({})",
                parse_generic_types(trim(&self.parameters))
            ));
            if !self.return_type.is_empty() {
                text.push_str(" : ");
                text.push_str(&parse_generic_types(&self.return_type));
            }
        }
        (trim(&text).into(), self.parse_classifier())
    }

    /// Ports `parseClassifier`.
    pub fn parse_classifier(&self) -> &'static str {
        match self.classifier.as_str() {
            "*" => "font-style:italic;",
            "$" => "text-decoration:underline;",
            _ => "",
        }
    }
}

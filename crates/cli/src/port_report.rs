//! Working out why a port failed, and what that means for the person waiting.
//!
//! When a port fails the error dies in a terminal and we never learn anything.
//! But not every failure means the same thing, and the difference decides what
//! we can honestly promise:
//!
//! - The agent reached for something that does not exist, or wrote code that
//!   breaks the import rules. Krate could already do the job. hexyl failed this
//!   way: it called `stdio::write`, which was not in the SDK, though the byte
//!   write underneath it had been there all along. Fixes like that are small.
//! - The app needs a capability Krate genuinely does not have. That means a new
//!   interface, a host, and identical behaviour on macOS, Windows, and Linux.
//!   Weeks, not hours.
//!
//! Telling someone "24 hours" when the answer is the second kind is a promise
//! we break in public, in front of the developers we most need to keep. So the
//! classification is mechanical and the promise follows from it.

use std::collections::BTreeSet;

/// What kind of failure this was, and therefore what we can promise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// The agent used an API that does not exist, or misused one that does.
    /// Krate can already do this; the gap is in what the agent was told.
    UnknownApi,
    /// The candidate reached outside `krate:*`, usually by way of std.
    ImportViolation,
    /// The app needs a capability Krate does not have yet.
    MissingCapability,
    /// A plain compile error in the ported code: a type mismatch, a borrow, a
    /// typo. The agent has everything it needs and simply got it wrong.
    CodeError,
    /// Nothing matched. Reported as itself rather than guessed at, because a
    /// wrong classification produces a wrong promise.
    Unknown,
}

impl FailureKind {
    /// What to tell the person whose port just failed.
    ///
    /// Each of these is a promise, so each one has to be true. The fast answer
    /// is only offered where the fix really is small.
    pub fn promise(&self) -> &'static str {
        match self {
            Self::UnknownApi => {
                "Krate can already do this -- the AI reached for a name that does not \
                 exist. Gaps like this are usually fixed within a day or two."
            }
            Self::ImportViolation => {
                "The ported code reached outside Krate's own APIs, which a Krate app \
                 may not do. This is usually a small change to how the code is \
                 written, not a missing feature."
            }
            Self::MissingCapability => {
                "This app needs something Krate does not offer yet. Adding it means a \
                 new interface that behaves identically on macOS, Windows, and Linux, \
                 which takes weeks rather than days. We will not pretend otherwise."
            }
            Self::CodeError => {
                "The AI had everything it needed and still got the code wrong. \
                 Re-running the port often succeeds. If it keeps failing the same \
                 way, that is worth telling us about."
            }
            Self::Unknown => {
                "We could not tell from the error what went wrong. Sending the report \
                 is the only way we find out."
            }
        }
    }

    /// A short label for a listing.
    pub fn label(&self) -> &'static str {
        match self {
            Self::UnknownApi => "unknown API",
            Self::ImportViolation => "import violation",
            Self::MissingCapability => "missing capability",
            Self::CodeError => "code error",
            Self::Unknown => "unclassified",
        }
    }

    /// Whether this is the kind we can fix quickly.
    ///
    /// Used to decide what to promise, so it stays deliberately narrow: a kind
    /// counted as fast here becomes a deadline we have to meet.
    pub fn is_quick_fix(&self) -> bool {
        matches!(self, Self::UnknownApi | Self::ImportViolation)
    }
}

/// What we worked out about one failed port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortFailure {
    /// Which kind of failure this was.
    pub kind: FailureKind,
    /// The API names the agent used that do not exist, if any.
    pub unknown_names: Vec<String>,
    /// Non-`krate:*` imports the candidate carried, if any.
    pub foreign_imports: Vec<String>,
    /// The first line of the compiler error, kept for the report.
    pub headline: String,
}

/// Read a failed build's output and work out what kind of failure it was.
///
/// Order matters. A single failed build can produce several kinds of message at
/// once, and the most actionable one should win: an import violation is a fact
/// about the finished component, while an unknown name is a fact about what the
/// agent was told, and the latter is the one we can fix at the source.
pub fn classify(error_text: &str) -> PortFailure {
    let unknown_names = unknown_api_names(error_text);
    let foreign_imports = foreign_imports(error_text);

    let kind = if !unknown_names.is_empty() {
        FailureKind::UnknownApi
    } else if !foreign_imports.is_empty() {
        FailureKind::ImportViolation
    } else if mentions_missing_capability(error_text) {
        FailureKind::MissingCapability
    } else if mentions_compile_error(error_text) {
        FailureKind::CodeError
    } else {
        FailureKind::Unknown
    };

    PortFailure {
        kind,
        unknown_names,
        foreign_imports,
        headline: headline(error_text),
    }
}

/// Names the agent used that the SDK does not have.
///
/// rustc names the missing item in a quoted backtick, which is exactly the
/// string we want: `cannot find function \`write\` in module \`stdio\`` yields
/// `stdio::write`, the call hexyl invented.
fn unknown_api_names(text: &str) -> Vec<String> {
    let mut found = BTreeSet::new();

    for line in text.lines() {
        let line = line.trim();
        // `cannot find function `write` in module `stdio``
        if let Some(rest) = line.strip_prefix("error[E0425]: cannot find ") {
            if let Some((item, module)) = item_and_module(rest) {
                found.insert(match module {
                    Some(module) => format!("{module}::{item}"),
                    None => item,
                });
            }
            continue;
        }
        // `unresolved import `krate::foo``
        if let Some(rest) = line.strip_prefix("error[E0432]: unresolved import ") {
            if let Some(name) = backticked(rest) {
                found.insert(name);
            }
            continue;
        }
        // `no function or associated item named `x` found`
        if line.contains("error[E0599]") && line.contains("named ") {
            if let Some(name) = backticked(&line[line.find("named ").unwrap_or(0)..]) {
                found.insert(name);
            }
        }
    }

    found.into_iter().collect()
}

/// Pull `item` and its module out of a "cannot find X `item` in module `mod`".
fn item_and_module(rest: &str) -> Option<(String, Option<String>)> {
    let item = backticked(rest)?;
    let module = rest
        .find(" in module ")
        .and_then(|at| backticked(&rest[at..]));
    Some((item, module))
}

/// The first backtick-quoted run in a string.
fn backticked(text: &str) -> Option<String> {
    let start = text.find('`')? + 1;
    let rest = &text[start..];
    let end = rest.find('`')?;
    let name = rest[..end].trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Non-`krate:*` imports named in the failure.
///
/// The import check reports these when a component reaches outside Krate's own
/// interfaces, almost always because something pulled std's runtime in.
fn foreign_imports(text: &str) -> Vec<String> {
    let mut found = BTreeSet::new();
    for line in text.lines() {
        for token in line.split_whitespace() {
            let token = token.trim_matches(|c: char| !c.is_alphanumeric() && c != ':' && c != '/');
            // `wasi:` is the one that actually happens; anything non-krate that
            // reaches the import wall belongs in the report either way.
            if token.starts_with("wasi:") && token.len() > "wasi:".len() {
                found.insert(token.to_string());
            }
        }
    }
    found.into_iter().collect()
}

/// Whether the failure is the runtime saying a capability does not exist.
fn mentions_missing_capability(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("unknown capability")
        || lower.contains("unsupported capability")
        || (lower.contains("capability") && lower.contains("not supported"))
}

/// Whether this looks like an ordinary compile error.
fn mentions_compile_error(text: &str) -> bool {
    text.contains("error[E") || text.contains("could not compile")
}

/// The most useful single line, for a summary that has to fit on one.
fn headline(text: &str) -> String {
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("error[E") || line.starts_with("error:") {
            return line.to_string();
        }
    }
    text.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real failure, copied from the run that motivated all of this.
    const HEXYL_ERROR: &str = r#"
error[E0425]: cannot find function `write` in module `stdio`
   --> src/lib.rs:93:32
    |
 93 |                 let _ = stdio::write(bytes);
    |                                ^^^^^ not found in `stdio`
error: could not compile `hexyl` (lib) due to 1 previous error
"#;

    #[test]
    fn the_hexyl_failure_is_recognised_as_a_gap_we_can_close() {
        let failure = classify(HEXYL_ERROR);
        assert_eq!(failure.kind, FailureKind::UnknownApi);
        assert_eq!(failure.unknown_names, vec!["stdio::write".to_string()]);
        // Krate could already do this, so the promise may be a fast one.
        assert!(failure.kind.is_quick_fix());
        assert!(failure.kind.promise().contains("already do this"));
    }

    #[test]
    fn a_missing_capability_is_never_promised_as_a_quick_fix() {
        let failure = classify("error: unknown capability `bluetooth.connect`");
        assert_eq!(failure.kind, FailureKind::MissingCapability);
        // The whole point of classifying: this must not get the fast promise.
        assert!(!failure.kind.is_quick_fix());
        assert!(
            failure.kind.promise().contains("weeks"),
            "a missing capability must be promised honestly, got: {}",
            failure.kind.promise()
        );
        assert!(!failure.kind.promise().contains("24 hours"));
    }

    #[test]
    fn a_leaked_wasi_import_is_reported_as_one() {
        let failure = classify(
            "error: component imports wasi:filesystem/types@0.2.0 \
             which is not a krate: interface",
        );
        assert_eq!(failure.kind, FailureKind::ImportViolation);
        assert_eq!(
            failure.foreign_imports,
            vec!["wasi:filesystem/types@0.2.0".to_string()]
        );
    }

    #[test]
    fn an_ordinary_compile_error_is_not_mistaken_for_a_missing_feature() {
        // A borrow error means the agent had everything it needed. Calling this
        // a missing capability would put weeks on something that is not our bug.
        let failure = classify(
            "error[E0502]: cannot borrow `items` as mutable because it is also \
             borrowed as immutable",
        );
        assert_eq!(failure.kind, FailureKind::CodeError);
        assert!(!failure.kind.is_quick_fix());
        assert!(failure.unknown_names.is_empty());
    }

    #[test]
    fn an_unresolved_import_names_what_was_reached_for() {
        let failure = classify("error[E0432]: unresolved import `krate::process`");
        assert_eq!(failure.kind, FailureKind::UnknownApi);
        assert_eq!(failure.unknown_names, vec!["krate::process".to_string()]);
    }

    #[test]
    fn an_unrecognised_failure_says_so_rather_than_guessing() {
        // A wrong classification produces a wrong promise, so "I do not know"
        // has to be an available answer.
        let failure = classify("the machine caught fire");
        assert_eq!(failure.kind, FailureKind::Unknown);
        assert!(!failure.kind.is_quick_fix());
    }

    #[test]
    fn the_headline_is_the_error_not_the_first_blank_line() {
        let failure = classify(HEXYL_ERROR);
        assert!(failure.headline.starts_with("error[E0425]"));
        assert!(failure.headline.contains("stdio"));
    }

    #[test]
    fn every_kind_has_a_promise_we_can_keep() {
        for kind in [
            FailureKind::UnknownApi,
            FailureKind::ImportViolation,
            FailureKind::MissingCapability,
            FailureKind::CodeError,
            FailureKind::Unknown,
        ] {
            let promise = kind.promise();
            assert!(!promise.is_empty(), "{kind:?} has no promise");
            // No kind may carry a fixed deadline. A date we miss in public is
            // worse than no date, and only the quick kinds may even hint at one.
            assert!(
                !promise.contains("24 hours"),
                "{kind:?} promises a deadline we cannot guarantee"
            );
            if !kind.is_quick_fix() {
                assert!(
                    !promise.contains("within a day"),
                    "{kind:?} is not a quick fix but promises one"
                );
            }
        }
    }
}

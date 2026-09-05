//! Did the app we built actually do what was asked?
//!
//! Every step before this one is mechanical. The crate compiles, the imports
//! are `krate:*` only, the bundle packs, the app runs and exits zero, and it
//! refuses to run without its gating permission. All five can be true of an
//! app that has nothing to do with the request.
//!
//! That is not hypothetical. Asking the built-in maker for "a chess game that
//! enforces legal moves and detects checkmate" produces a checklist, and the
//! old transcript reported `"ok": true` and "authored a working,
//! permission-gated .krate". A todo list, reported as chess.
//!
//! So the verdicts are kept apart. Building is one claim, running is another,
//! and *serving the request* is a third that has to be earned on its own
//! evidence. A missing verdict is not a pass -- an unevaluated requirement
//! counts against the result, never for it. Overall success needs every
//! required verdict, and any one of them failing makes the whole result a
//! failure no matter how clean the build was.
//!
//! What this module does not do is judge quality. It reads the request for
//! the concrete nouns and behaviours it names, then looks for evidence of each
//! in the app that came out. That catches the failure that actually happens --
//! a starter shipped unchanged, or a template picked that has nothing to do
//! with what was asked -- without pretending to know whether the chess engine
//! is any good.

use serde::{Deserialize, Serialize};

/// One thing the request asked for, with a stable ID so a verdict can be
/// pinned to it across a revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requirement {
    /// Stable identifier, e.g. `req-1`. Stable within one request: the same
    /// request always yields the same IDs in the same order.
    pub id: String,
    /// The words from the request this came from, so a person reading a
    /// failure can see what was expected without re-reading the request.
    pub text: String,
    /// The lowercase terms whose presence in the app is the evidence.
    pub terms: Vec<String>,
}

/// The verdict on one requirement. There is deliberately no `Unknown` that
/// reads as success: test 1638 -- missing is not pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// Evidence for this requirement was found in the authored app.
    Pass,
    /// The app was built and this requirement is not served by it.
    Fail,
    /// Not applicable to this request, and so not counted either way.
    Skip,
}

/// A requirement and what became of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    /// Which requirement this answers.
    pub id: String,
    /// The requirement in the person's own words.
    pub text: String,
    /// Pass, fail or skip. Never absent.
    pub outcome: Outcome,
    /// Why, in one sentence, so a failure is actionable rather than a verdict
    /// the person has to take on faith.
    pub detail: String,
}

/// The whole request verdict: every requirement, and whether the app as a
/// whole serves the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Acceptance {
    /// One verdict per requirement, in requirement order.
    pub requirements: Vec<Verdict>,
    /// True only when every non-skipped requirement passed. Test 1640.
    pub accepted: bool,
    /// One sentence naming what is missing, when something is.
    pub summary: String,
}

impl Acceptance {
    /// The requirements that were not served, for a message that names them.
    pub fn failures(&self) -> Vec<&Verdict> {
        self.requirements
            .iter()
            .filter(|v| v.outcome == Outcome::Fail)
            .collect()
    }
}

/// Words that carry no evidence value: they appear in nearly every request
/// and finding them in an app proves nothing.
///
/// Three kinds are in here. Ordinary function words. Words that stand in for a
/// thing rather than naming one -- "the most frequent ones" says nothing an app
/// could implement under that word. And words for how the app presents itself:
/// a CLI "shows" by printing and a GUI by drawing, so the verb is never the
/// evidence, while the behaviour it introduces has its own terms in the clause.
///
/// "items" is deliberately absent. A checklist really does implement items, and
/// the word is the honest name for what it holds.
#[rustfmt::skip]
const NOISE: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "for", "with", "that", "this", "it", "its", "is",
    "are", "be", "can", "will", "make", "makes", "made", "build", "builds", "create", "creates",
    "app", "application", "program", "me", "my", "i", "want", "need", "please", "simple",
    "small", "nice", "good", "some", "any", "all", "of", "to", "in", "on", "at", "by", "from",
    "as", "so", "then", "when", "if", "you", "your", "we", "us", "let", "lets", "should",
    "would", "could", "has", "have", "had", "do", "does", "did", "get", "gets", "use", "uses",
    "using", "new", "one", "two", "up", "down", "out", "into", "over", "each", "every", "also",
    "just", "like", "very", "which", "what", "where", "who", "how", "there", "their", "them",
    "they", "he", "she", "his", "her", "ones", "thing", "things", "stuff", "most", "least",
    "more", "less", "many", "much", "same", "other", "another", "both", "such", "own", "show",
    "shows", "showing", "display", "displays", "displaying", "see", "view", "look", "print",
    "prints", "output", "outputs", "give", "gives",
];

/// Pull the concrete things a request asks for.
///
/// This is deliberately shallow. It takes the content words -- the nouns and
/// verbs that carry the request's meaning -- and groups them into
/// requirements, one per clause. It is not parsing English; it is picking out
/// the terms whose total absence from an app is proof the app is not the one
/// that was asked for.
pub fn requirements(request: &str) -> Vec<Requirement> {
    let mut out = Vec::new();
    // Clauses are the natural unit: "a chess game that enforces legal moves
    // and detects checkmate" is three things asked for, not one.
    for clause in split_clauses(request) {
        let terms = content_terms(&clause);
        if terms.is_empty() {
            continue;
        }
        out.push(Requirement {
            id: format!("req-{}", out.len() + 1),
            text: clause.trim().to_string(),
            terms,
        });
    }
    out
}

/// Break a request into the clauses that each ask for something.
fn split_clauses(request: &str) -> Vec<String> {
    let mut clauses = Vec::new();
    let mut current = String::new();
    // Splitting on words rather than characters keeps "and" inside a phrase
    // like "black and white" from starting a clause, since we only split when
    // the joiner stands between two runs that each have content of their own.
    for word in request.split_whitespace() {
        let bare: String = word
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-')
            .collect::<String>()
            .to_lowercase();
        let is_joiner = matches!(bare.as_str(), "and" | "that" | "which" | "then" | "plus")
            || word.ends_with(',')
            || word.ends_with(';');
        if is_joiner && !content_terms(&current).is_empty() {
            if !bare.is_empty()
                && !matches!(bare.as_str(), "and" | "that" | "which" | "then" | "plus")
            {
                // A trailing comma or semicolon: the word itself belongs to
                // the clause that is ending, not to the next one.
                current.push(' ');
                current.push_str(word);
            }
            clauses.push(std::mem::take(&mut current));
            continue;
        }
        current.push(' ');
        current.push_str(word);
    }
    if !content_terms(&current).is_empty() {
        clauses.push(current);
    }
    if clauses.is_empty() && !request.trim().is_empty() {
        clauses.push(request.to_string());
    }
    clauses
}

/// The words in a clause that carry meaning worth looking for.
fn content_terms(clause: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for word in clause.split_whitespace() {
        let bare: String = word
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-')
            .collect::<String>()
            .to_lowercase();
        // Two-letter words are almost all function words, and the few that
        // are not ("3d", "ai") survive because they contain a digit.
        if bare.len() < 3 && !bare.chars().any(|c| c.is_ascii_digit()) {
            continue;
        }
        if NOISE.contains(&bare.as_str()) {
            continue;
        }
        if !terms.contains(&bare) {
            terms.push(bare);
        }
    }
    terms
}

/// Judge an authored app against the request that asked for it.
///
/// `source` is the app's own source text -- what the authoring step actually
/// wrote. A term counts as served when it appears in the code: an app that
/// serves "checkmate" will contain the word somewhere in the code that
/// implements it, and an app that does not, will not.
///
/// `name` is taken for the record but is deliberately *not* evidence. The name
/// is derived from the request, so a checklist built for a chess request is
/// called `chess-game` and would vouch for itself. Only the code counts.
///
/// A term also counts as served when a close relative of it appears, so that
/// "moves" is satisfied by `fn apply_move`. That is a stem match, not a
/// synonym table -- being generous here is right, because a false failure
/// wastes a person's build and a false pass is the bug this exists to stop.
pub fn judge(request: &str, _name: &str, source: &str) -> Acceptance {
    let reqs = requirements(request);
    // Comments are stripped first. A no-op edit that adds
    // `// an image editor with layers and undo` above an untouched starter
    // changes the bytes and mentions every term, and it must not pass. Only
    // code counts as evidence that the app does something.
    let haystack = strip_comments(source).to_lowercase();

    let mut verdicts = Vec::new();
    for req in &reqs {
        // A clause is served when the app shows evidence of what it named.
        // Requiring every term would fail on wording ("a game of chess" vs
        // "chess game"); requiring one would pass anything. Requiring most of
        // them tracks whether the app is about this subject at all.
        let found: Vec<&String> = req
            .terms
            .iter()
            .filter(|term| mentions(&haystack, term))
            .collect();
        let needed = required_hits(req.terms.len());
        let outcome = if found.len() >= needed {
            Outcome::Pass
        } else {
            Outcome::Fail
        };
        let missing: Vec<&str> = req
            .terms
            .iter()
            .filter(|t| !found.contains(t))
            .map(String::as_str)
            .collect();
        let detail = match outcome {
            Outcome::Pass => format!("the app has {} of {}", found.len(), req.terms.len()),
            Outcome::Fail => format!("the app has nothing about {}", missing.join(", ")),
            Outcome::Skip => "not applicable".to_string(),
        };
        verdicts.push(Verdict {
            id: req.id.clone(),
            text: req.text.clone(),
            outcome,
            detail,
        });
    }

    // A subject clause that failed is set aside when the behaviour clauses
    // all passed: an app that enforces legal moves and detects checkmate is a
    // chess game whether or not its code ever says "chess". This never rescues
    // an app that failed a behaviour clause, and never applies when the
    // subject is the only thing the request said.
    let behaviours: Vec<usize> = (0..verdicts.len())
        .filter(|&i| !is_subject_clause(&reqs[i]))
        .collect();
    let behaviours_all_pass = !behaviours.is_empty()
        && behaviours
            .iter()
            .all(|&i| verdicts[i].outcome == Outcome::Pass);
    if behaviours_all_pass {
        for (i, req) in reqs.iter().enumerate() {
            if is_subject_clause(req) && verdicts[i].outcome == Outcome::Fail {
                verdicts[i].outcome = Outcome::Skip;
                verdicts[i].detail =
                    "the app does what this asked for, under its own names".to_string();
            }
        }
    }

    // An empty requirement list means the request had no content words to go
    // on. That is not evidence of success, so it is not accepted -- but the
    // caller decides whether such a request should have been built at all.
    let judged: Vec<&Verdict> = verdicts
        .iter()
        .filter(|v| v.outcome != Outcome::Skip)
        .collect();
    let accepted = !judged.is_empty() && judged.iter().all(|v| v.outcome == Outcome::Pass);

    let summary = if accepted {
        "the app serves everything the request asked for".to_string()
    } else if judged.is_empty() {
        "the request named nothing specific enough to check".to_string()
    } else {
        let missing: Vec<&str> = verdicts
            .iter()
            .filter(|v| v.outcome == Outcome::Fail)
            .map(|v| v.text.as_str())
            .collect();
        format!("the app does not do this: {}", missing.join("; "))
    };

    Acceptance {
        requirements: verdicts,
        accepted,
        summary,
    }
}

/// How many of a clause's terms must appear for the clause to count as served.
///
/// A bare majority is not enough. "a maze game you can walk through with arrow
/// keys" has six terms, and the generated checklist matched three of them --
/// `key` from its own store key, and two other incidental words -- which under
/// a half threshold read as a pass. A clause has to be mostly present, so the
/// bar is two thirds, and a long clause cannot ride in on scattered hits.
fn required_hits(total: usize) -> usize {
    match total {
        0 => 0,
        1..=2 => 1,
        // Two thirds, rounded up: 3->2, 4->3, 6->4, 9->6.
        n => (n * 2).div_ceil(3),
    }
}

/// Is this clause naming the subject rather than a behaviour?
///
/// "a chess game" names what the app is; "enforces legal moves" names what it
/// does. The distinction matters because correct code often never says its own
/// subject -- a real chess engine talks about pawns, boards and squares, and
/// may never contain the word "chess" -- while it must contain the substance
/// of every behaviour it implements.
///
/// So a subject clause is evidence when it is found and is not held against
/// the app when it is missing, *provided* the behaviour clauses passed. An app
/// with no behaviour clauses has nothing else to be judged on, so there the
/// subject is all there is and must be found.
fn is_subject_clause(req: &Requirement) -> bool {
    // A behaviour clause has a verb in it. Rather than tag parts of speech,
    // use the shape the request actually takes: a subject clause is the one
    // the request opens with, and it is short.
    req.id == "req-1" && req.terms.len() <= 3
}

/// Remove Rust comments, so prose about the app is not mistaken for the app.
///
/// String literals are kept: an app whose UI says "Checkmate!" really does
/// mention checkmate to the person using it, which is evidence. A comment is
/// only a note to the next reader, and the no-op edit E4 caught hid in one.
fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let bytes: Vec<char> = source.chars().collect();
    let mut i = 0;
    let mut in_string = false;
    let mut block_depth = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        let next = bytes.get(i + 1).copied();
        if block_depth > 0 {
            if c == '*' && next == Some('/') {
                block_depth -= 1;
                i += 2;
                continue;
            }
            if c == '/' && next == Some('*') {
                block_depth += 1;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if in_string {
            // A backslash escape cannot end the string.
            if c == '\\' {
                out.push(c);
                if let Some(n) = next {
                    out.push(n);
                }
                i += 2;
                continue;
            }
            if c == '"' {
                in_string = false;
            }
            out.push(c);
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '/' && next == Some('/') {
            while i < bytes.len() && bytes[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && next == Some('*') {
            block_depth = 1;
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Is this term present in the app, allowing for the endings code puts on it?
///
/// `checkmate` matches `checkmate`, `is_checkmate` and `CHECKMATE`; `moves`
/// matches `move` and `apply_move`. Matching the stem rather than the exact
/// word is what keeps ordinary code from failing a requirement it serves.
fn mentions(haystack: &str, term: &str) -> bool {
    if haystack.contains(term) {
        return true;
    }
    // Try the singular: "moves" -> "move", "boxes" -> "box".
    if let Some(stem) = term.strip_suffix("es") {
        if stem.len() >= 3 && haystack.contains(stem) {
            return true;
        }
    }
    if let Some(stem) = term.strip_suffix('s') {
        if stem.len() >= 3 && haystack.contains(stem) {
            return true;
        }
    }
    // And the verb endings: "enforces"/"enforcing"/"enforced" -> "enforc".
    for suffix in ["ing", "ed", "er", "ion"] {
        if let Some(stem) = term.strip_suffix(suffix) {
            if stem.len() >= 4 && haystack.contains(stem) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The E4 case, exactly: a chess request answered with a checklist. This
    /// is the false positive the whole module exists to stop, and it is the
    /// first thing tested. Test 1635.
    #[test]
    fn a_checklist_does_not_pass_as_a_chess_game() {
        // The real generated checklist source, in the shape that matters:
        // items, checkboxes, a saved list. No chess anywhere.
        let checklist = r#"
            struct Item { text: String, done: bool }
            fn toggle(items: &mut Vec<Item>, index: usize) { items[index].done = !items[index].done; }
            fn save(items: &[Item]) { store_kv_set("items", &encode(items)); }
        "#;
        let verdict = judge(
            "a chess game that enforces legal moves and detects checkmate",
            "chess-game",
            checklist,
        );
        assert!(
            !verdict.accepted,
            "a checklist was accepted as chess: {verdict:?}"
        );
        assert!(
            !verdict.failures().is_empty(),
            "the failure must name what is missing"
        );
        assert!(
            verdict.summary.contains("does not do this"),
            "summary must say what is missing, got: {}",
            verdict.summary
        );
    }

    /// The starter that prints `replace me` cannot pass a dashboard request.
    /// Test 1634.
    #[test]
    fn an_unchanged_starter_does_not_pass_a_dashboard_request() {
        let starter = r#"
            fn main() { println!("replace me"); }
        "#;
        let verdict = judge(
            "a dashboard showing live stock prices with a chart",
            "stock-dashboard",
            starter,
        );
        assert!(
            !verdict.accepted,
            "the untouched starter was accepted: {verdict:?}"
        );
    }

    /// A comment-only edit changes the bytes and nothing else. It must not
    /// satisfy a functional request. Test 1625.
    #[test]
    fn a_comment_only_edit_does_not_satisfy_a_functional_request() {
        let before = r#"fn main() { println!("replace me"); }"#;
        let after = r#"
            // an image editor with layers and undo
            fn main() { println!("replace me"); }
        "#;
        let request = "an image editor with layers and undo";
        // The comment mentions the very words, so a naive text search passes
        // it. Comments are stripped before judging for exactly this reason.
        assert!(
            !judge(request, "image-editor", after).accepted,
            "a comment naming the feature must not count as implementing it"
        );
        assert!(!judge(request, "image-editor", before).accepted);
    }

    /// An app that really does serve the request passes. Without this the
    /// module could pass its other tests by always failing.
    #[test]
    fn a_real_word_counter_passes_a_word_counter_request() {
        let source = r#"
            fn count_words(text: &str) -> BTreeMap<String, usize> {
                let mut counts = BTreeMap::new();
                for word in text.split_whitespace() {
                    *counts.entry(word.to_lowercase()).or_insert(0) += 1;
                }
                counts
            }
            fn most_frequent(counts: &BTreeMap<String, usize>) -> Vec<(String, usize)> { .. }
        "#;
        let verdict = judge(
            "count the words in a file and show the most frequent ones",
            "word-counter",
            source,
        );
        assert!(
            verdict.accepted,
            "a real word counter was rejected: {verdict:?}"
        );
    }

    /// A real chess implementation passes the same request the checklist
    /// failed, which is what makes the first test a test of the app rather
    /// than of the request's wording.
    #[test]
    fn a_real_chess_game_passes_the_chess_request() {
        let source = r#"
            enum Piece { Pawn, Knight, Bishop, Rook, Queen, King }
            fn legal_moves(board: &Board, from: Square) -> Vec<Square> { .. }
            fn is_checkmate(board: &Board, side: Color) -> bool { .. }
            fn enforce(board: &mut Board, mv: Move) -> Result<(), Illegal> { .. }
        "#;
        let verdict = judge(
            "a chess game that enforces legal moves and detects checkmate",
            "chess-game",
            source,
        );
        assert!(verdict.accepted, "real chess was rejected: {verdict:?}");
    }

    /// Every requirement gets a verdict. Nothing is left absent and read as
    /// success. Test 1638.
    #[test]
    fn every_requirement_gets_a_verdict() {
        let request = "a timer that counts down and beeps when it finishes";
        let reqs = requirements(request);
        assert!(reqs.len() >= 2, "clauses should split: {reqs:?}");
        let verdict = judge(request, "timer", "fn main() {}");
        assert_eq!(
            verdict.requirements.len(),
            reqs.len(),
            "every requirement must carry a verdict"
        );
        for v in &verdict.requirements {
            assert_ne!(v.detail, "", "a verdict without a reason is not a verdict");
        }
    }

    /// Requirement IDs are stable: the same request yields the same IDs, so a
    /// revision's verdicts can be compared against the previous ones.
    #[test]
    fn requirement_ids_are_stable_for_the_same_request() {
        let request = "a notes app that saves to disk and searches by tag";
        let first = requirements(request);
        let second = requirements(request);
        assert_eq!(first, second);
        assert_eq!(first[0].id, "req-1");
    }

    /// The app's name is not evidence about the app. The name is derived from
    /// the request, so letting it count would mean a checklist called
    /// `chess-game` vouches for itself -- which is how the first requirement
    /// of the chess case passed before this was fixed.
    #[test]
    fn the_apps_name_is_not_evidence_of_what_it_does() {
        let checklist = "struct Item { text: String, done: bool }";
        let named = judge("a chess game", "chess-game", checklist);
        let unnamed = judge("a chess game", "app", checklist);
        assert_eq!(
            named.requirements[0].outcome, unnamed.requirements[0].outcome,
            "naming the app after the request must not change the verdict"
        );
        assert_eq!(named.requirements[0].outcome, Outcome::Fail);
    }

    /// The subject-clause rule must never rescue an app that failed a
    /// behaviour. It exists so correct code that never says its own subject
    /// still passes -- not so a wrong app passes on one matching word.
    #[test]
    fn the_subject_rule_never_rescues_a_failed_behaviour() {
        // Says "chess" and "game" everywhere, implements neither behaviour.
        let fake = r#"
            struct ChessGame { items: Vec<String> }
            fn game_name() -> &'static str { "chess game" }
        "#;
        let verdict = judge(
            "a chess game that enforces legal moves and detects checkmate",
            "chess-game",
            fake,
        );
        assert!(
            !verdict.accepted,
            "matching the subject must not carry the behaviours: {verdict:?}"
        );
    }

    /// When the subject is all the request said, it must be found. There is
    /// no behaviour clause to stand in for it.
    #[test]
    fn a_subject_only_request_must_match_the_subject() {
        let checklist = "struct Item { text: String, done: bool }";
        assert!(!judge("a chess game", "chess-game", checklist).accepted);
    }

    /// The real generated checklist, judged against a spread of requests.
    ///
    /// The hand-written fixtures above prove the rule; this proves it against
    /// the source the pipeline actually produces. It guards both directions at
    /// once: a request the checklist genuinely serves must pass, and every
    /// request it does not must fail. A checker that only ever said no would
    /// pass the false-positive tests and fail here.
    #[test]
    fn the_real_generated_checklist_is_judged_correctly() {
        let request = crate::AppRequest::checklist("list");
        let app = crate::generate(&request, "..").expect("generate");
        let source: String = app
            .files
            .iter()
            .filter(|f| f.path.ends_with(".rs") && !f.path.ends_with("bindings.rs"))
            .map(|f| f.contents.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!source.is_empty(), "no source generated");

        // What a checklist really is. These must pass.
        for req in [
            "a checklist app where I can add items and check them off",
            "a todo list that saves my items",
        ] {
            let verdict = judge(req, "app", &source);
            assert!(
                verdict.accepted,
                "the checklist should serve {req:?}: {verdict:?}"
            );
        }

        // What it is not. These are the E4 false positives and their kin, and
        // every one of them must fail.
        for req in [
            "a chess game that enforces legal moves and detects checkmate",
            "a dashboard showing live stock prices with a chart",
            "an image editor with layers and undo",
            "a maze game you can walk through with arrow keys",
        ] {
            let verdict = judge(req, "app", &source);
            assert!(
                !verdict.accepted,
                "the checklist was accepted as {req:?}: {verdict:?}"
            );
        }
    }

    /// The noise list is hand-formatted (rustfmt is told to leave it alone, or
    /// it becomes one word per line), so nothing checks it for a slip. This
    /// does: every entry a single lowercase word, no duplicates, and none of
    /// the words an app really implements.
    #[test]
    fn the_noise_list_is_well_formed() {
        let mut seen = std::collections::BTreeSet::new();
        for word in NOISE {
            assert!(
                !word.contains(char::is_whitespace),
                "{word:?} is not a single word"
            );
            assert_eq!(*word, word.to_lowercase(), "{word:?} is not lowercase");
            assert!(seen.insert(*word), "{word:?} is in the list twice");
        }
        // Words a real app implements must never be discarded as noise, or an
        // app that does nothing would pass a request that named them.
        for real in [
            "items",
            "list",
            "chess",
            "checkmate",
            "undo",
            "layers",
            "chart",
        ] {
            assert!(
                !NOISE.contains(&real),
                "{real:?} is real behaviour, not noise"
            );
        }
    }

    /// A request with nothing concrete in it is not accepted by default --
    /// absence of evidence must not read as evidence.
    #[test]
    fn a_contentless_request_is_not_accepted() {
        let verdict = judge("make me an app", "thing", "fn main() {}");
        assert!(!verdict.accepted, "{verdict:?}");
    }
}

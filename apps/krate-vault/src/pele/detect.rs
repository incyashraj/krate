//! Ports detect.ts: Mermaid's detectors, in the order Mermaid registers
//! them. The first match wins.

use crate::pele::js::is_space;

/// Ports `interface Detected`: Pele's diagram type and the type as Mermaid
/// reports it (such as `flowchart-v2` for `graph TD`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detected {
    pub kind: &'static str,
    pub mermaid_type: &'static str,
}

/// How a detector's pattern ends after its words.
#[derive(Clone, Copy)]
enum Tail {
    /// Nothing more: a prefix.
    None,
    /// `\b`.
    Word,
    /// `(?:[\s:]|$)`.
    SpaceColonOrEnd,
    /// `(?:\s|$)`.
    SpaceOrEnd,
}

/// One detector: the type, Mermaid's type, the words any of which may
/// follow `^\s*`, case-insensitive or not, and the tail.
struct Detector {
    kind: &'static str,
    mermaid: &'static str,
    words: &'static [&'static str],
    ci: bool,
    tail: Tail,
}

const fn d(
    kind: &'static str,
    mermaid: &'static str,
    words: &'static [&'static str],
    ci: bool,
    tail: Tail,
) -> Detector {
    Detector {
        kind,
        mermaid,
        words,
        ci,
        tail,
    }
}

const DETECTORS: [Detector; 37] = [
    d(
        "flowchart",
        "flowchart-elk",
        &["flowchart-elk"],
        false,
        Tail::None,
    ),
    d("mindmap", "mindmap", &["mindmap"], false, Tail::None),
    d(
        "architecture",
        "architecture",
        &["architecture"],
        false,
        Tail::None,
    ),
    d(
        "agentflow",
        "agentflow",
        &["agentflow-beta"],
        false,
        Tail::Word,
    ),
    d(
        "c4",
        "c4",
        &[
            "C4Context",
            "C4Container",
            "C4Component",
            "C4Dynamic",
            "C4Deployment",
        ],
        false,
        Tail::None,
    ),
    d("kanban", "kanban", &["kanban"], false, Tail::None),
    d(
        "class",
        "classDiagram",
        &["classDiagram"],
        false,
        Tail::None,
    ),
    d("er", "er", &["erDiagram"], false, Tail::None),
    d("gantt", "gantt", &["gantt"], false, Tail::None),
    d("info", "info", &["info"], false, Tail::None),
    d("pie", "pie", &["pie"], false, Tail::None),
    d(
        "requirement",
        "requirement",
        &["requirement"],
        false,
        Tail::None,
    ),
    d(
        "sequence",
        "sequence",
        &["sequenceDiagram"],
        false,
        Tail::None,
    ),
    d(
        "swimlane",
        "swimlane",
        &["swimlane-beta"],
        false,
        Tail::Word,
    ),
    d(
        "flowchart",
        "flowchart-v2",
        &["graph", "flowchart"],
        false,
        Tail::None,
    ),
    d("timeline", "timeline", &["timeline"], false, Tail::None),
    d("gitGraph", "gitGraph", &["gitGraph"], false, Tail::None),
    d(
        "state",
        "stateDiagram",
        &["stateDiagram"],
        false,
        Tail::None,
    ),
    d("journey", "journey", &["journey"], false, Tail::None),
    d(
        "quadrantChart",
        "quadrantChart",
        &["quadrantChart"],
        false,
        Tail::None,
    ),
    d("sankey", "sankey", &["sankey"], false, Tail::None),
    d("packet", "packet", &["packet"], false, Tail::None),
    d("xychart", "xychart", &["xychart"], false, Tail::None),
    d("block", "block", &["block"], false, Tail::None),
    d(
        "eventmodeling",
        "eventmodeling",
        &["eventmodeling"],
        false,
        Tail::None,
    ),
    d(
        "treeView",
        "treeView",
        &["treeView-beta"],
        false,
        Tail::None,
    ),
    d("radar", "radar", &["radar-beta"], false, Tail::None),
    d(
        "ishikawa",
        "ishikawa",
        &["ishikawa-beta", "ishikawa"],
        true,
        Tail::Word,
    ),
    d("treemap", "treemap", &["treemap"], false, Tail::None),
    d("railroad", "railroad", &["railroad-beta"], true, Tail::None),
    d(
        "railroad",
        "railroadEbnf",
        &["railroad-ebnf-beta"],
        true,
        Tail::None,
    ),
    d(
        "railroad",
        "railroadAbnf",
        &["railroad-abnf-beta"],
        true,
        Tail::None,
    ),
    d(
        "railroad",
        "railroadPeg",
        &["railroad-peg-beta"],
        true,
        Tail::None,
    ),
    d("venn", "venn", &["venn-beta"], false, Tail::None),
    d("wardley", "wardley", &["wardley-beta"], true, Tail::None),
    d(
        "cynefin",
        "cynefin",
        &["cynefin-beta"],
        false,
        Tail::SpaceColonOrEnd,
    ),
    d(
        "usecase",
        "usecase",
        &["usecase-beta"],
        false,
        Tail::SpaceOrEnd,
    ),
];

fn word_char(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `/^\s*word.../.test(text)` for one detector.
fn matches(det: &Detector, text: &str) -> bool {
    let rest = text.trim_start_matches(is_space);
    det.words.iter().any(|w| {
        let head = rest.get(..w.len());
        let hit = match head {
            Some(h) if det.ci => h.eq_ignore_ascii_case(w),
            Some(h) => h == *w,
            None => false,
        };
        if !hit {
            return false;
        }
        let after = rest[w.len()..].chars().next();
        match det.tail {
            Tail::None => true,
            // The words end in a word character, so \b needs a non-word next.
            Tail::Word => !word_char(after),
            Tail::SpaceColonOrEnd => after.is_none_or(|c| is_space(c) || c == ':'),
            Tail::SpaceOrEnd => after.is_none_or(is_space),
        }
    })
}

/// Ports `detectDiagram`.
pub fn detect_diagram(cleaned: &str) -> Option<Detected> {
    DETECTORS
        .iter()
        .find(|det| matches(det, cleaned))
        .map(|det| Detected {
            kind: det.kind,
            mermaid_type: det.mermaid,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects() {
        assert_eq!(
            detect_diagram("graph TD\nA").map(|d| d.mermaid_type),
            Some("flowchart-v2")
        );
        assert_eq!(
            detect_diagram("  flowchart-elk LR").map(|d| d.mermaid_type),
            Some("flowchart-elk")
        );
        assert_eq!(
            detect_diagram("sequenceDiagram").map(|d| d.kind),
            Some("sequence")
        );
        assert_eq!(
            detect_diagram("Ishikawa-beta").map(|d| d.kind),
            Some("ishikawa")
        );
        assert_eq!(detect_diagram("nothing"), None);
    }
}

//! Every action the editor has, for the command palette.
//!
//! One list, so nothing is reachable only by a key a person has to already
//! know: each row says what it does and the keys that do it too.

use crate::text::fuzzy_score;
use crate::tools::Tool;
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    OpenFolder,
    NewProject,
    Tour,
    QuickOpen,
    Save,
    SaveAll,
    CloseFile,
    NextFile,
    PrevFile,
    NewFile,
    NewFolder,
    Rename,
    Delete,
    RevealInTree,
    FocusTree,
    SplitRight,
    ClosePane,
    NextPane,
    ToggleTree,
    TogglePanel,
    ShowProblems,
    ShowOutput,
    ShowTerminal,
    Find,
    Replace,
    GotoLine,
    Undo,
    Redo,
    ToggleBreakpoint,
    ThemeAuto,
    ThemeDark,
    ThemeLight,
    ClickMode,
    RelativeNumbers,
    BiggerText,
    SmallerText,
    Keys,
    FormatOnSave,
    ShowDebug,
    Tool(Tool),
}

#[derive(Debug, Clone, Copy)]
pub struct Command {
    pub action: Action,
    pub title: &'static str,
    pub keys: &'static str,
}

const fn c(action: Action, title: &'static str, keys: &'static str) -> Command {
    Command {
        action,
        title,
        keys,
    }
}

pub const COMMANDS: &[Command] = &[
    c(Action::QuickOpen, "Go to file", "Ctrl+P"),
    c(Action::Save, "Save file", "Ctrl+S  :w"),
    c(Action::SaveAll, "Save all files", ":wa"),
    c(Action::OpenFolder, "Open folder", "Ctrl+O"),
    c(Action::NewProject, "New Rust project", ""),
    c(Action::Tour, "Open the tour project", ""),
    c(Action::NewFile, "New file", "a in the tree"),
    c(Action::NewFolder, "New folder", "A in the tree"),
    c(Action::Rename, "Rename file", "r in the tree"),
    c(Action::Delete, "Delete file", "d in the tree"),
    c(Action::RevealInTree, "Show this file in the tree", ""),
    c(Action::FocusTree, "Focus the file tree", "Ctrl+Shift+E"),
    c(Action::CloseFile, "Close file", ":q  :bd"),
    c(Action::NextFile, "Next file", ":bn"),
    c(Action::PrevFile, "Previous file", ":bp"),
    c(Action::SplitRight, "Split editor right", "Ctrl+W v  :vsp"),
    c(Action::NextPane, "Focus the next split", "Ctrl+W w"),
    c(Action::ClosePane, "Close split", "Ctrl+W c  :close"),
    c(Action::ToggleTree, "Show or hide the file tree", "Ctrl+B"),
    c(
        Action::TogglePanel,
        "Show or hide the bottom panel",
        "Ctrl+J",
    ),
    c(Action::ShowProblems, "Show Problems", ""),
    c(Action::ShowOutput, "Show Output", ""),
    c(Action::ShowTerminal, "Show Terminal", "Ctrl+`"),
    c(Action::Find, "Find in file", "/  Ctrl+F"),
    c(Action::Replace, "Replace in file", ":%s/old/new/g"),
    c(Action::GotoLine, "Go to line", ":<line>"),
    c(Action::Undo, "Undo", "u  Ctrl+Z"),
    c(Action::Redo, "Redo", "Ctrl+R  Ctrl+Shift+Z"),
    c(
        Action::ToggleBreakpoint,
        "Toggle breakpoint on this line",
        "F9",
    ),
    c(Action::Tool(Tool::Build), Tool::Build.title(), "F7"),
    c(Action::Tool(Tool::Run), Tool::Run.title(), "F5"),
    c(Action::Tool(Tool::Test), Tool::Test.title(), ""),
    c(Action::Tool(Tool::Check), Tool::Check.title(), ""),
    c(Action::Tool(Tool::Clippy), Tool::Clippy.title(), ""),
    c(Action::Tool(Tool::Format), Tool::Format.title(), ""),
    c(
        Action::Tool(Tool::Complete),
        Tool::Complete.title(),
        "Ctrl+Space",
    ),
    c(Action::Tool(Tool::Hover), Tool::Hover.title(), "K"),
    c(
        Action::Tool(Tool::Definition),
        Tool::Definition.title(),
        "F12  gd",
    ),
    c(
        Action::Tool(Tool::References),
        Tool::References.title(),
        "Shift+F12  gr",
    ),
    c(
        Action::Tool(Tool::RenameSymbol),
        Tool::RenameSymbol.title(),
        "F2  :rename",
    ),
    c(
        Action::Tool(Tool::CodeAction),
        Tool::CodeAction.title(),
        "Ctrl+.  :actions",
    ),
    c(Action::Tool(Tool::Debug), Tool::Debug.title(), "Shift+F5"),
    c(Action::Tool(Tool::Continue), Tool::Continue.title(), "F5"),
    c(Action::Tool(Tool::StepOver), Tool::StepOver.title(), "F10"),
    c(Action::Tool(Tool::StepIn), Tool::StepIn.title(), "F11"),
    c(
        Action::Tool(Tool::StepOut),
        Tool::StepOut.title(),
        "Shift+F11",
    ),
    c(
        Action::Tool(Tool::StopDebug),
        Tool::StopDebug.title(),
        "Shift+F5",
    ),
    c(
        Action::ShowDebug,
        "Show the debugger's variables and call stack",
        "",
    ),
    c(
        Action::Tool(Tool::Terminal),
        Tool::Terminal.title(),
        "Ctrl+`  :term",
    ),
    c(Action::Tool(Tool::Stop), Tool::Stop.title(), ":stop"),
    c(
        Action::FormatOnSave,
        "Format on save on or off (rustfmt)",
        ":fos",
    ),
    c(Action::ThemeAuto, "Theme: follow the system", ":theme auto"),
    c(Action::ThemeDark, "Theme: Dusk (dark)", ":theme dark"),
    c(Action::ThemeLight, "Theme: Paper (light)", ":theme light"),
    c(
        Action::ClickMode,
        "Clicking text: start typing or stay in normal mode",
        "",
    ),
    c(
        Action::RelativeNumbers,
        "Relative line numbers on or off",
        ":set rnu",
    ),
    c(Action::BiggerText, "Bigger text", "Ctrl+="),
    c(Action::SmallerText, "Smaller text", "Ctrl+-"),
    c(Action::Keys, "Keys: a short guide to the modes", ":help"),
];

/// The commands matching `query`, best first (all of them, in list order,
/// for an empty query).
pub fn filter(query: &str) -> Vec<usize> {
    let mut scored: Vec<(i32, usize)> = COMMANDS
        .iter()
        .enumerate()
        .filter_map(|(i, cmd)| fuzzy_score(query, cmd.title).map(|s| (s, i)))
        .collect();
    if !query.trim().is_empty() {
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    }
    scored.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_is_in_the_palette() {
        for t in Tool::ALL {
            assert!(
                COMMANDS.iter().any(|c| c.action == Action::Tool(t)),
                "{t:?}"
            );
        }
    }

    #[test]
    fn filtering_ranks_the_obvious_one_first() {
        let first = |q: &str| COMMANDS[filter(q)[0]].action;
        assert_eq!(first("save"), Action::Save);
        assert_eq!(first("split"), Action::SplitRight);
        assert_eq!(first("build"), Action::Tool(Tool::Build));
        assert_eq!(first("theme light"), Action::ThemeLight);
        assert_eq!(filter("").len(), COMMANDS.len());
        assert!(filter("qqqzzz").is_empty());
    }
}

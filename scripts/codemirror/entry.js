// The pieces of CodeMirror 6 Studio's IDE uses, as one global (window.KrateCM).
import { EditorState, Compartment, StateEffect, StateField, RangeSetBuilder } from "@codemirror/state";
import { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter, drawSelection, dropCursor, rectangularSelection, crosshairCursor, highlightSpecialChars, Decoration, ViewPlugin, gutter, GutterMarker, hoverTooltip, closeHoverTooltips, showPanel } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab, toggleComment } from "@codemirror/commands";
import { indentOnInput, bracketMatching, foldGutter, foldKeymap, syntaxHighlighting, HighlightStyle, StreamLanguage, indentUnit } from "@codemirror/language";
import { searchKeymap, highlightSelectionMatches, gotoLine, openSearchPanel, search } from "@codemirror/search";
import { autocompletion, completionKeymap, closeBrackets, closeBracketsKeymap, snippetCompletion } from "@codemirror/autocomplete";
import { linter, lintGutter, lintKeymap, setDiagnostics, forceLinting } from "@codemirror/lint";
import { rust } from "@codemirror/lang-rust";
import { toml } from "@codemirror/legacy-modes/mode/toml";
import { tags } from "@lezer/highlight";
window.KrateCM = {
  EditorState, Compartment, StateEffect, StateField, RangeSetBuilder,
  EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter, drawSelection, dropCursor, rectangularSelection, crosshairCursor, highlightSpecialChars, Decoration, ViewPlugin, gutter, GutterMarker, hoverTooltip, closeHoverTooltips, showPanel,
  defaultKeymap, history, historyKeymap, indentWithTab, toggleComment,
  indentOnInput, bracketMatching, foldGutter, foldKeymap, syntaxHighlighting, HighlightStyle, StreamLanguage, indentUnit,
  searchKeymap, highlightSelectionMatches, gotoLine, openSearchPanel, search,
  autocompletion, completionKeymap, closeBrackets, closeBracketsKeymap, snippetCompletion,
  linter, lintGutter, lintKeymap, setDiagnostics, forceLinting,
  rust, toml: StreamLanguage.define(toml), tags,
};

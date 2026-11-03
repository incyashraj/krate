//! A Language Server Protocol client, for rust-analyzer over its stdio.
//!
//! Nothing here waits. The editor hands the client what the person did
//! ([`Lsp::sync`] a file's text, ask for [`Lsp::completion`] at a place) and
//! the client queues the bytes to send in [`Lsp::out`]; whatever the server
//! sends back is pushed into [`Lsp::receive`] when it arrives and comes out
//! as [`LspEvent`]s. A reply that arrives after the person moved on is
//! still delivered, tagged with where it was asked, and the editor drops it.
//!
//! Files are named as the editor names them, relative to the project; the
//! client turns them into the `file://` addresses a server needs from the
//! project's place on the computer, which cargo reports.
//!
//! Positions are lines and columns. The client offers UTF-8 columns (the
//! editor's own, bytes into the line) and falls back to the protocol's
//! default UTF-16 when a server does not take them; [`Lsp::to_lsp`] and
//! [`Lsp::from_lsp`] convert with the line's text.

use crate::json::{obj, Value};
use crate::rpc::{self, Framer};
use crate::tools::{Diagnostic, Severity, Source};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// A place in a file, in the server's columns until converted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// Relative to the project, or `None` when it is outside it (the
    /// standard library); then `outside` names it.
    pub path: Option<String>,
    pub outside: String,
    pub line: usize,
    pub col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

/// A change to text: replace `line:col .. end_line:end_col` with `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub line: usize,
    pub col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEdit {
    pub path: Option<String>,
    pub outside: String,
    pub edits: Vec<TextEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub detail: String,
    /// What to put in when there is no edit: the label otherwise.
    pub insert: String,
    pub edit: Option<TextEdit>,
    pub extra: Vec<TextEdit>,
    pub filter: String,
    pub sort: String,
    /// The protocol's kind number (2 method, 3 function, 6 variable, ...).
    pub kind: u8,
}

impl CompletionItem {
    /// A short word for the kind, shown beside the name.
    pub fn kind_word(&self) -> &'static str {
        match self.kind {
            2 => "method",
            3 => "fn",
            4 => "new",
            5 => "field",
            6 => "let",
            7 => "class",
            8 => "trait",
            9 => "mod",
            10 => "prop",
            13 => "enum",
            14 => "kw",
            15 => "snip",
            20 => "variant",
            21 => "const",
            22 => "struct",
            25 => "type",
            _ => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeAction {
    pub title: String,
    pub edits: Vec<FileEdit>,
}

/// Where a request was asked, so a late answer can be recognised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    pub path: String,
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LspEvent {
    /// The server answered `initialize`; requests go through now.
    Ready,
    Diagnostics {
        path: Option<String>,
        items: Vec<Diagnostic>,
    },
    Completion {
        at: Asked,
        items: Vec<CompletionItem>,
    },
    Hover {
        at: Asked,
        text: String,
    },
    Definition {
        at: Asked,
        found: Vec<Location>,
    },
    References {
        at: Asked,
        found: Vec<Location>,
    },
    Rename {
        at: Asked,
        edits: Vec<FileEdit>,
    },
    Signature {
        at: Asked,
        label: String,
        /// The active parameter's place in `label`, in bytes.
        active: Option<(usize, usize)>,
    },
    Actions {
        at: Asked,
        actions: Vec<CodeAction>,
    },
    /// Work the server is doing ("Indexing", "cargo check"), or `None`
    /// when it finished.
    Progress(Option<String>),
    /// Something to tell the person: a refused request, a server message.
    Say(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    Initialize,
    Completion(Asked),
    Hover(Asked),
    Definition(Asked),
    References(Asked),
    Rename(Asked),
    Signature(Asked),
    Actions(Asked),
    Shutdown,
}

pub struct Lsp {
    /// `file:///...` of the project folder, no trailing slash.
    root_uri: String,
    next_id: i64,
    pending: BTreeMap<i64, Pending>,
    pub ready: bool,
    /// Requests asked before the server was ready, sent when it is.
    held: Vec<Value>,
    /// Bytes for the server's stdin.
    pub out: Vec<u8>,
    framer: Framer,
    /// Files the server has open: their version and the text it has.
    docs: BTreeMap<String, (i64, String)>,
    /// Columns are UTF-8 bytes (otherwise UTF-16 units).
    pub utf8: bool,
    /// Characters that start completion by themselves (`.`, `:`).
    pub triggers: Vec<char>,
    /// The raw diagnostics last published for each file, for code actions.
    raw_diagnostics: BTreeMap<String, Vec<Value>>,
    /// Progress tokens still going, with their titles.
    progress: BTreeMap<String, String>,
}

impl Lsp {
    /// A client for the project at `root` (an absolute path on the
    /// computer, as cargo reports it). Queues `initialize`.
    pub fn new(root: &str) -> Self {
        let mut l = Lsp {
            root_uri: uri_from_path(root.trim_end_matches('/')),
            next_id: 0,
            pending: BTreeMap::new(),
            ready: false,
            held: Vec::new(),
            out: Vec::new(),
            framer: Framer::default(),
            docs: BTreeMap::new(),
            utf8: false,
            triggers: alloc::vec!['.', ':'],
            raw_diagnostics: BTreeMap::new(),
            progress: BTreeMap::new(),
        };
        let caps = obj([
            (
                "general",
                obj([(
                    "positionEncodings",
                    Value::Arr(alloc::vec!["utf-8".into(), "utf-16".into()]),
                )]),
            ),
            ("window", obj([("workDoneProgress", true.into())])),
            (
                "textDocument",
                obj([
                    (
                        "synchronization",
                        obj([
                            ("didSave", true.into()),
                            ("dynamicRegistration", false.into()),
                        ]),
                    ),
                    (
                        "completion",
                        obj([
                            (
                                "completionItem",
                                obj([
                                    ("snippetSupport", false.into()),
                                    ("insertReplaceSupport", false.into()),
                                ]),
                            ),
                            ("contextSupport", true.into()),
                        ]),
                    ),
                    (
                        "hover",
                        obj([(
                            "contentFormat",
                            Value::Arr(alloc::vec!["plaintext".into(), "markdown".into()]),
                        )]),
                    ),
                    ("definition", obj([("linkSupport", false.into())])),
                    ("references", obj([])),
                    ("rename", obj([("prepareSupport", false.into())])),
                    (
                        "signatureHelp",
                        obj([(
                            "signatureInformation",
                            obj([(
                                "parameterInformation",
                                obj([("labelOffsetSupport", true.into())]),
                            )]),
                        )]),
                    ),
                    (
                        "codeAction",
                        obj([(
                            "codeActionLiteralSupport",
                            obj([(
                                "codeActionKind",
                                obj([(
                                    "valueSet",
                                    Value::Arr(alloc::vec![
                                        "".into(),
                                        "quickfix".into(),
                                        "refactor".into(),
                                        "refactor.extract".into(),
                                        "refactor.inline".into(),
                                        "refactor.rewrite".into(),
                                        "source".into(),
                                    ]),
                                )]),
                            )]),
                        )]),
                    ),
                    (
                        "publishDiagnostics",
                        obj([("relatedInformation", false.into())]),
                    ),
                ]),
            ),
        ]);
        let name = l.root_uri.rsplit('/').next().unwrap_or("project");
        let params = obj([
            ("processId", Value::Null),
            (
                "clientInfo",
                obj([("name", "Krate Code".into()), ("version", "0.1".into())]),
            ),
            ("rootUri", l.root_uri.as_str().into()),
            (
                "workspaceFolders",
                Value::Arr(alloc::vec![obj([
                    ("uri", l.root_uri.as_str().into()),
                    ("name", name.into()),
                ])]),
            ),
            ("capabilities", caps),
        ]);
        let id = l.take_id(Pending::Initialize);
        let msg = obj([
            ("jsonrpc", "2.0".into()),
            ("id", id.into()),
            ("method", "initialize".into()),
            ("params", params),
        ]);
        l.out.extend_from_slice(&rpc::frame(&msg));
        l
    }

    fn take_id(&mut self, p: Pending) -> i64 {
        self.next_id += 1;
        self.pending.insert(self.next_id, p);
        self.next_id
    }

    fn send(&mut self, msg: Value) {
        if self.ready {
            self.out.extend_from_slice(&rpc::frame(&msg));
        } else {
            self.held.push(msg);
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(obj([
            ("jsonrpc", "2.0".into()),
            ("method", method.into()),
            ("params", params),
        ]));
    }

    fn request(&mut self, p: Pending, method: &str, params: Value) -> i64 {
        let id = self.take_id(p);
        self.send(obj([
            ("jsonrpc", "2.0".into()),
            ("id", id.into()),
            ("method", method.into()),
            ("params", params),
        ]));
        id
    }

    /// How many requests are waiting for an answer.
    pub fn waiting(&self) -> usize {
        self.pending.len()
    }

    pub fn uri(&self, path: &str) -> String {
        let mut u = self.root_uri.clone();
        u.push('/');
        u.push_str(&encode_path(path));
        u
    }

    /// The project path of a `file://` address, or `Err` with the address's
    /// own path when it is outside the project.
    pub fn path_of(&self, uri: &str) -> Result<String, String> {
        let full = decode_uri(uri);
        let root = decode_uri(&self.root_uri);
        for (a, b) in [
            (full.as_str(), root.as_str()),
            (
                full.trim_start_matches("/private"),
                root.trim_start_matches("/private"),
            ),
        ] {
            if let Some(rest) = a.strip_prefix(b) {
                if let Some(rel) = rest.strip_prefix('/') {
                    return Ok(String::from(rel));
                }
            }
        }
        Err(full)
    }

    fn doc_id(&self, path: &str) -> Value {
        obj([("uri", self.uri(path).into())])
    }

    fn position(line: usize, col: usize) -> Value {
        obj([("line", line.into()), ("character", col.into())])
    }

    /// A byte column on `line_text` in the server's columns.
    pub fn to_lsp(&self, line_text: &str, byte_col: usize) -> usize {
        if self.utf8 {
            return byte_col;
        }
        line_text
            .get(..byte_col.min(line_text.len()))
            .unwrap_or(line_text)
            .chars()
            .map(char::len_utf16)
            .sum()
    }

    /// A server column on `line_text` as a byte column.
    pub fn from_lsp(&self, line_text: &str, col: usize) -> usize {
        if self.utf8 {
            let mut c = col.min(line_text.len());
            while c > 0 && !line_text.is_char_boundary(c) {
                c -= 1;
            }
            return c;
        }
        let mut units = 0;
        for (i, ch) in line_text.char_indices() {
            if units >= col {
                return i;
            }
            units += ch.len_utf16();
        }
        line_text.len()
    }

    /// Tell the server what `path` holds now: opens it the first time, and
    /// afterwards sends only the part that changed.
    pub fn sync(&mut self, path: &str, text: &str) {
        if !path.ends_with(".rs") {
            return;
        }
        match self.docs.get(path) {
            None => {
                self.docs
                    .insert(String::from(path), (1, String::from(text)));
                let td = obj([
                    ("uri", self.uri(path).into()),
                    ("languageId", "rust".into()),
                    ("version", 1i64.into()),
                    ("text", text.into()),
                ]);
                self.notify("textDocument/didOpen", obj([("textDocument", td)]));
            }
            Some((v, old)) if old != text => {
                let v = *v + 1;
                let change = change_between(old, text, self.utf8);
                self.docs
                    .insert(String::from(path), (v, String::from(text)));
                let td = obj([("uri", self.uri(path).into()), ("version", v.into())]);
                let ((l0, c0), (l1, c1), new) = change;
                let ch = obj([
                    (
                        "range",
                        obj([
                            ("start", Self::position(l0, c0)),
                            ("end", Self::position(l1, c1)),
                        ]),
                    ),
                    ("text", new.into()),
                ]);
                self.notify(
                    "textDocument/didChange",
                    obj([
                        ("textDocument", td),
                        ("contentChanges", Value::Arr(alloc::vec![ch])),
                    ]),
                );
            }
            Some(_) => {}
        }
    }

    pub fn is_open(&self, path: &str) -> bool {
        self.docs.contains_key(path)
    }

    pub fn saved(&mut self, path: &str) {
        if self.docs.contains_key(path) {
            let td = self.doc_id(path);
            self.notify("textDocument/didSave", obj([("textDocument", td)]));
        }
    }

    pub fn closed(&mut self, path: &str) {
        if self.docs.remove(path).is_some() {
            let td = self.doc_id(path);
            self.notify("textDocument/didClose", obj([("textDocument", td)]));
        }
    }

    fn at_params(&self, at: &Asked) -> Value {
        obj([
            ("textDocument", self.doc_id(&at.path)),
            ("position", Self::position(at.line, at.col)),
        ])
    }

    /// `trigger` is the character typed that asked, if one did.
    pub fn completion(&mut self, at: Asked, trigger: Option<char>) -> i64 {
        let mut p = self.at_params(&at);
        let ctx = match trigger {
            Some(c) => {
                let mut s = String::new();
                s.push(c);
                obj([("triggerKind", 2i64.into()), ("triggerCharacter", s.into())])
            }
            None => obj([("triggerKind", 1i64.into())]),
        };
        p.set("context", ctx);
        self.request(Pending::Completion(at), "textDocument/completion", p)
    }

    pub fn hover(&mut self, at: Asked) -> i64 {
        let p = self.at_params(&at);
        self.request(Pending::Hover(at), "textDocument/hover", p)
    }

    pub fn definition(&mut self, at: Asked) -> i64 {
        let p = self.at_params(&at);
        self.request(Pending::Definition(at), "textDocument/definition", p)
    }

    pub fn references(&mut self, at: Asked) -> i64 {
        let mut p = self.at_params(&at);
        p.set("context", obj([("includeDeclaration", true.into())]));
        self.request(Pending::References(at), "textDocument/references", p)
    }

    pub fn rename(&mut self, at: Asked, new_name: &str) -> i64 {
        let mut p = self.at_params(&at);
        p.set("newName", new_name.into());
        self.request(Pending::Rename(at), "textDocument/rename", p)
    }

    pub fn signature(&mut self, at: Asked) -> i64 {
        let p = self.at_params(&at);
        self.request(Pending::Signature(at), "textDocument/signatureHelp", p)
    }

    pub fn code_actions(&mut self, at: Asked) -> i64 {
        // The diagnostics on this line go with the request, so the server
        // offers their fixes.
        let here: Vec<Value> = self
            .raw_diagnostics
            .get(&at.path)
            .map(|ds| {
                ds.iter()
                    .filter(|d| {
                        let s = d.at(&["range", "start", "line"]).uint().unwrap_or(0);
                        let e = d.at(&["range", "end", "line"]).uint().unwrap_or(s);
                        (s..=e).contains(&at.line)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let p = obj([
            ("textDocument", self.doc_id(&at.path)),
            (
                "range",
                obj([
                    ("start", Self::position(at.line, at.col)),
                    ("end", Self::position(at.line, at.col)),
                ]),
            ),
            ("context", obj([("diagnostics", Value::Arr(here))])),
        ]);
        self.request(Pending::Actions(at), "textDocument/codeAction", p)
    }

    /// Ask the server to stop; `exit` follows when it answers.
    pub fn shutdown(&mut self) {
        self.request(Pending::Shutdown, "shutdown", Value::Null);
    }

    /// Bytes from the server's stdout.
    pub fn receive(&mut self, bytes: &[u8]) -> Vec<LspEvent> {
        self.framer.push(bytes);
        let mut events = Vec::new();
        for m in self.framer.drain() {
            self.message(m, &mut events);
        }
        events
    }

    fn reply(&mut self, id: Value, result: Value) {
        let msg = obj([("jsonrpc", "2.0".into()), ("id", id), ("result", result)]);
        // Replies go out even before `initialized`: the server is waiting.
        self.out.extend_from_slice(&rpc::frame(&msg));
    }

    fn message(&mut self, m: Value, ev: &mut Vec<LspEvent>) {
        let method = m.get("method").str().map(String::from);
        let id = m.get("id").clone();
        match (method, id.is_null()) {
            (Some(method), false) => self.server_request(&method, id, &m),
            (Some(method), true) => self.notification(&method, m.get("params"), ev),
            (None, false) => {
                let Some(n) = id.int() else { return };
                let Some(p) = self.pending.remove(&n) else {
                    return;
                };
                if !m.get("error").is_null() {
                    // A cancelled or stale request is not worth a word.
                    let code = m.at(&["error", "code"]).int().unwrap_or(0);
                    if p == Pending::Initialize {
                        ev.push(LspEvent::Say(String::from(
                            "rust-analyzer could not start on this project",
                        )));
                    } else if code != -32800 && code != -32801 && code != -32802 {
                        if let Some(msg) = m.at(&["error", "message"]).str() {
                            let mut s = String::from("rust-analyzer: ");
                            s.push_str(msg);
                            ev.push(LspEvent::Say(s));
                        }
                    }
                    return;
                }
                self.response(p, m.get("result"), ev);
            }
            (None, true) => {}
        }
    }

    fn server_request(&mut self, method: &str, id: Value, m: &Value) {
        let result = match method {
            // One null per item asked: rust-analyzer's defaults.
            "workspace/configuration" => {
                let n = m.at(&["params", "items"]).arr().len();
                Value::Arr((0..n).map(|_| Value::Null).collect())
            }
            "workspace/applyEdit" => obj([("applied", false.into())]),
            _ => Value::Null,
        };
        self.reply(id, result);
    }

    fn notification(&mut self, method: &str, p: &Value, ev: &mut Vec<LspEvent>) {
        match method {
            "textDocument/publishDiagnostics" => {
                let uri = p.get("uri").str().unwrap_or("");
                let path = self.path_of(uri).ok();
                let raw: Vec<Value> = p.get("diagnostics").arr().to_vec();
                let text = path
                    .as_ref()
                    .and_then(|p| self.docs.get(p))
                    .map(|d| d.1.clone());
                let lines: Vec<&str> = text
                    .as_deref()
                    .map(|t| t.split('\n').collect())
                    .unwrap_or_default();
                let items = raw
                    .iter()
                    .map(|d| {
                        let sev = match d.get("severity").int() {
                            Some(1) | None => Severity::Error,
                            Some(2) => Severity::Warning,
                            _ => Severity::Note,
                        };
                        let r = d.get("range");
                        let l = r.at(&["start", "line"]).uint().unwrap_or(0);
                        let c = r.at(&["start", "character"]).uint().unwrap_or(0);
                        let el = r.at(&["end", "line"]).uint().unwrap_or(l);
                        let ec = r.at(&["end", "character"]).uint().unwrap_or(c);
                        // Diagnostics carry character columns.
                        let to_chars = |line: usize, col: usize| -> usize {
                            match lines.get(line) {
                                Some(t) => {
                                    let b = self.from_lsp(t, col);
                                    t.get(..b).map(|s| s.chars().count()).unwrap_or(col)
                                }
                                None => col,
                            }
                        };
                        let code = match d.get("code") {
                            Value::Str(s) => Some(s.clone()),
                            Value::Num(n) => Some(crate::text::num(*n as usize)),
                            _ => None,
                        };
                        Diagnostic {
                            file: path.clone().unwrap_or_default(),
                            line: l,
                            col: to_chars(l, c),
                            end_line: el,
                            end_col: to_chars(el, ec),
                            severity: sev,
                            message: String::from(
                                d.get("message")
                                    .str()
                                    .unwrap_or("")
                                    .lines()
                                    .next()
                                    .unwrap_or(""),
                            ),
                            code,
                            source: Source::Lsp,
                        }
                    })
                    .collect();
                if let Some(path) = &path {
                    self.raw_diagnostics.insert(path.clone(), raw);
                }
                ev.push(LspEvent::Diagnostics { path, items });
            }
            "$/progress" => {
                let token = match p.get("token") {
                    Value::Str(s) => s.clone(),
                    Value::Num(n) => crate::text::num(*n as usize),
                    _ => String::new(),
                };
                let v = p.get("value");
                match v.get("kind").str() {
                    Some("begin") => {
                        let mut t = String::from(v.get("title").str().unwrap_or("Working"));
                        if let Some(m) = v.get("message").str() {
                            t.push_str(": ");
                            t.push_str(m);
                        }
                        self.progress.insert(token, t);
                    }
                    Some("end") => {
                        self.progress.remove(&token);
                    }
                    _ => {}
                }
                ev.push(LspEvent::Progress(self.progress.values().next().cloned()));
            }
            "window/showMessage" if p.get("type").int().unwrap_or(4) <= 2 => {
                if let Some(m) = p.get("message").str() {
                    let mut s = String::from("rust-analyzer: ");
                    s.push_str(m.lines().next().unwrap_or(""));
                    ev.push(LspEvent::Say(s));
                }
            }
            _ => {}
        }
    }

    fn location(&self, v: &Value) -> Option<Location> {
        // A Location, or a LocationLink a server sends anyway.
        let uri = v.get("uri").str().or(v.get("targetUri").str())?;
        let r = if v.get("range").is_null() {
            v.get("targetSelectionRange")
        } else {
            v.get("range")
        };
        let l = r.at(&["start", "line"]).uint()?;
        let c = r.at(&["start", "character"]).uint()?;
        let (path, outside) = match self.path_of(uri) {
            Ok(p) => (Some(p), String::new()),
            Err(full) => (None, full),
        };
        Some(Location {
            path,
            outside,
            line: l,
            col: c,
            end_line: r.at(&["end", "line"]).uint().unwrap_or(l),
            end_col: r.at(&["end", "character"]).uint().unwrap_or(c),
        })
    }

    fn locations(&self, v: &Value) -> Vec<Location> {
        match v {
            Value::Arr(items) => items.iter().filter_map(|i| self.location(i)).collect(),
            Value::Null => Vec::new(),
            one => self.location(one).into_iter().collect(),
        }
    }

    fn workspace_edit(&self, v: &Value) -> Vec<FileEdit> {
        let mut out: Vec<FileEdit> = Vec::new();
        let mut add = |uri: &str, edits: &[Value]| {
            let (path, outside) = match self.path_of(uri) {
                Ok(p) => (Some(p), String::new()),
                Err(f) => (None, f),
            };
            let edits: Vec<TextEdit> = edits.iter().filter_map(text_edit).collect();
            out.push(FileEdit {
                path,
                outside,
                edits,
            });
        };
        for change in v.get("documentChanges").arr() {
            if let Some(uri) = change.at(&["textDocument", "uri"]).str() {
                add(uri, change.get("edits").arr());
            }
        }
        for (uri, edits) in v.get("changes").obj_items() {
            add(uri, edits.arr());
        }
        out
    }

    fn response(&mut self, p: Pending, r: &Value, ev: &mut Vec<LspEvent>) {
        match p {
            Pending::Initialize => {
                let caps = r.get("capabilities");
                self.utf8 = caps.get("positionEncoding").str() == Some("utf-8");
                let trig: Vec<char> = caps
                    .at(&["completionProvider", "triggerCharacters"])
                    .arr()
                    .iter()
                    .filter_map(|t| t.str().and_then(|s| s.chars().next()))
                    .collect();
                if !trig.is_empty() {
                    self.triggers = trig;
                }
                self.ready = true;
                let msg = obj([
                    ("jsonrpc", "2.0".into()),
                    ("method", "initialized".into()),
                    ("params", obj([])),
                ]);
                self.out.extend_from_slice(&rpc::frame(&msg));
                // Anything asked while starting was written with the
                // client's guess at columns; it goes now.
                for m in core::mem::take(&mut self.held) {
                    self.out.extend_from_slice(&rpc::frame(&m));
                }
                ev.push(LspEvent::Ready);
            }
            Pending::Completion(at) => {
                let list = if r.get("items").arr().is_empty() {
                    r.arr()
                } else {
                    r.get("items").arr()
                };
                let items = list
                    .iter()
                    .map(|i| {
                        let label = String::from(i.get("label").str().unwrap_or(""));
                        let te = i.get("textEdit");
                        let edit = if te.get("range").is_null() {
                            // An insert-and-replace edit: use its insert range.
                            let mut v = te.clone();
                            v.set("range", te.get("insert").clone());
                            text_edit(&v)
                        } else {
                            text_edit(te)
                        };
                        CompletionItem {
                            insert: String::from(i.get("insertText").str().unwrap_or(&label)),
                            detail: String::from(i.get("detail").str().unwrap_or("")),
                            edit,
                            extra: i
                                .get("additionalTextEdits")
                                .arr()
                                .iter()
                                .filter_map(text_edit)
                                .collect(),
                            filter: String::from(i.get("filterText").str().unwrap_or(&label)),
                            sort: String::from(i.get("sortText").str().unwrap_or(&label)),
                            kind: i.get("kind").uint().unwrap_or(0) as u8,
                            label,
                        }
                    })
                    .collect();
                ev.push(LspEvent::Completion { at, items });
            }
            Pending::Hover(at) => {
                let text = hover_text(r.get("contents"));
                ev.push(LspEvent::Hover { at, text });
            }
            Pending::Definition(at) => {
                let found = self.locations(r);
                ev.push(LspEvent::Definition { at, found });
            }
            Pending::References(at) => {
                let found = self.locations(r);
                ev.push(LspEvent::References { at, found });
            }
            Pending::Rename(at) => {
                let edits = self.workspace_edit(r);
                ev.push(LspEvent::Rename { at, edits });
            }
            Pending::Signature(at) => {
                let sigs = r.get("signatures").arr();
                let k = r.get("activeSignature").uint().unwrap_or(0);
                let Some(sig) = sigs.get(k).or(sigs.first()) else {
                    ev.push(LspEvent::Signature {
                        at,
                        label: String::new(),
                        active: None,
                    });
                    return;
                };
                let label = String::from(sig.get("label").str().unwrap_or(""));
                let ap = sig
                    .get("activeParameter")
                    .uint()
                    .or(r.get("activeParameter").uint())
                    .unwrap_or(0);
                let active = match sig.get("parameters").idx(ap).get("label") {
                    Value::Str(s) => label.find(s.as_str()).map(|i| (i, i + s.len())),
                    Value::Arr(pair) => {
                        // UTF-16 offsets into the label.
                        let a = pair.first().and_then(Value::uint);
                        let b = pair.get(1).and_then(Value::uint);
                        match (a, b) {
                            (Some(a), Some(b)) => {
                                let to_byte = |u: usize| {
                                    let mut units = 0;
                                    for (i, ch) in label.char_indices() {
                                        if units >= u {
                                            return i;
                                        }
                                        units += ch.len_utf16();
                                    }
                                    label.len()
                                };
                                Some((to_byte(a), to_byte(b)))
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                };
                ev.push(LspEvent::Signature { at, label, active });
            }
            Pending::Actions(at) => {
                let actions = r
                    .arr()
                    .iter()
                    .filter_map(|a| {
                        let edit = a.get("edit");
                        if edit.is_null() {
                            return None;
                        }
                        Some(CodeAction {
                            title: String::from(a.get("title").str().unwrap_or("Fix")),
                            edits: self.workspace_edit(edit),
                        })
                    })
                    .collect();
                ev.push(LspEvent::Actions { at, actions });
            }
            Pending::Shutdown => {
                let msg = obj([("jsonrpc", "2.0".into()), ("method", "exit".into())]);
                self.out.extend_from_slice(&rpc::frame(&msg));
            }
        }
    }
}

fn text_edit(v: &Value) -> Option<TextEdit> {
    let r = v.get("range");
    Some(TextEdit {
        line: r.at(&["start", "line"]).uint()?,
        col: r.at(&["start", "character"]).uint()?,
        end_line: r.at(&["end", "line"]).uint()?,
        end_col: r.at(&["end", "character"]).uint()?,
        text: String::from(v.get("newText").str()?),
    })
}

/// Hover contents as plain text: markdown's code fences dropped, the rest
/// kept as written.
pub fn hover_text(c: &Value) -> String {
    let raw = match c {
        Value::Str(s) => s.clone(),
        Value::Arr(items) => {
            let mut s = String::new();
            for i in items {
                if !s.is_empty() {
                    s.push_str("\n\n");
                }
                s.push_str(&hover_text(i));
            }
            s
        }
        Value::Obj(_) => String::from(c.get("value").str().unwrap_or("")),
        _ => String::new(),
    };
    let mut out = String::new();
    for line in raw.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            continue;
        }
        if t == "---" {
            out.push('\n');
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    let trimmed = out.trim();
    String::from(trimmed)
}

/// The one range of `old` that differs from `new`, in lines and columns of
/// `old`, and what replaces it.
pub fn change_between(
    old: &str,
    new: &str,
    utf8: bool,
) -> ((usize, usize), (usize, usize), String) {
    let (a, b) = (old.as_bytes(), new.as_bytes());
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
        pre += 1;
    }
    while pre > 0 && (!old.is_char_boundary(pre) || !new.is_char_boundary(pre)) {
        pre -= 1;
    }
    let mut suf = 0;
    while suf < a.len() - pre && suf < b.len() - pre && a[a.len() - 1 - suf] == b[b.len() - 1 - suf]
    {
        suf += 1;
    }
    while suf > 0 && (!old.is_char_boundary(a.len() - suf) || !new.is_char_boundary(b.len() - suf))
    {
        suf -= 1;
    }
    let start = pos_of(old, pre, utf8);
    let end = pos_of(old, a.len() - suf, utf8);
    (start, end, String::from(&new[pre..b.len() - suf]))
}

/// Line and column of byte `at` in `text`.
fn pos_of(text: &str, at: usize, utf8: bool) -> (usize, usize) {
    let before = &text[..at];
    let line = before.bytes().filter(|b| *b == b'\n').count();
    let start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let seg = &text[start..at];
    let col = if utf8 {
        seg.len()
    } else {
        seg.chars().map(char::len_utf16).sum()
    };
    (line, col)
}

pub fn uri_from_path(path: &str) -> String {
    let mut s = String::from("file://");
    if !path.starts_with('/') {
        // A Windows path: `C:\x` is `file:///C:/x`.
        s.push('/');
    }
    s.push_str(&encode_path(&path.replace('\\', "/")));
    s
}

fn encode_path(p: &str) -> String {
    let mut s = String::new();
    for b in p.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                s.push(b as char)
            }
            b':' if s.len() == 1 || s.len() == 2 => s.push(':'),
            _ => {
                s.push('%');
                let hex = b"0123456789ABCDEF";
                s.push(hex[(b >> 4) as usize] as char);
                s.push(hex[(b & 15) as usize] as char);
            }
        }
    }
    s
}

fn decode_uri(uri: &str) -> String {
    let rest = uri.strip_prefix("file://").unwrap_or(uri);
    let bytes = rest.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = |c: u8| (c as char).to_digit(16);
            if let (Some(a), Some(b)) = (h(bytes[i + 1]), h(bytes[i + 2])) {
                out.push((a * 16 + b) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    /// Every message the client queued, parsed back.
    fn sent(l: &mut Lsp) -> Vec<Value> {
        let mut f = Framer::default();
        f.push(&core::mem::take(&mut l.out));
        f.drain()
    }

    fn reply(id: i64, result: &str) -> Vec<u8> {
        let v = json::parse(&alloc::format!(
            r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#
        ))
        .unwrap();
        rpc::frame(&v)
    }

    fn notif(method: &str, params: &str) -> Vec<u8> {
        let v = json::parse(&alloc::format!(
            r#"{{"jsonrpc":"2.0","method":"{method}","params":{params}}}"#
        ))
        .unwrap();
        rpc::frame(&v)
    }

    fn ready() -> Lsp {
        let mut l = Lsp::new("/home/me/my proj");
        let init = sent(&mut l);
        assert_eq!(init[0].get("method").str(), Some("initialize"));
        assert_eq!(
            init[0].at(&["params", "rootUri"]).str(),
            Some("file:///home/me/my%20proj")
        );
        let ev = l.receive(&reply(1, r#"{"capabilities":{"positionEncoding":"utf-8","completionProvider":{"triggerCharacters":[":","."]}}}"#));
        assert_eq!(ev, alloc::vec![LspEvent::Ready]);
        assert!(l.utf8);
        let after = sent(&mut l);
        assert_eq!(after[0].get("method").str(), Some("initialized"));
        l
    }

    #[test]
    fn requests_wait_for_initialize_then_go_in_order() {
        let mut l = Lsp::new("/p");
        sent(&mut l);
        l.sync("src/main.rs", "fn main() {}\n");
        let id = l.hover(Asked {
            path: String::from("src/main.rs"),
            line: 0,
            col: 3,
        });
        assert!(
            sent(&mut l).is_empty(),
            "nothing before the server is ready"
        );
        l.receive(&reply(1, r#"{"capabilities":{}}"#));
        let msgs = sent(&mut l);
        let methods: Vec<&str> = msgs.iter().filter_map(|m| m.get("method").str()).collect();
        assert_eq!(
            methods,
            ["initialized", "textDocument/didOpen", "textDocument/hover"]
        );
        assert_eq!(msgs[2].get("id").int(), Some(id));
        assert!(!l.utf8, "no encoding answered means UTF-16");
    }

    #[test]
    fn changes_are_sent_as_the_part_that_changed() {
        let mut l = ready();
        l.sync("src/main.rs", "fn main() {\n    let x = 1;\n}\n");
        sent(&mut l);
        l.sync("src/main.rs", "fn main() {\n    let xy = 1;\n}\n");
        let m = sent(&mut l);
        assert_eq!(m[0].get("method").str(), Some("textDocument/didChange"));
        assert_eq!(
            m[0].at(&["params", "textDocument", "version"]).int(),
            Some(2)
        );
        let ch = m[0].at(&["params", "contentChanges"]).idx(0);
        assert_eq!(ch.at(&["range", "start", "line"]).int(), Some(1));
        assert_eq!(ch.at(&["range", "start", "character"]).int(), Some(9));
        assert_eq!(ch.at(&["range", "end", "character"]).int(), Some(9));
        assert_eq!(ch.get("text").str(), Some("y"));
        // The same text again sends nothing.
        l.sync("src/main.rs", "fn main() {\n    let xy = 1;\n}\n");
        assert!(sent(&mut l).is_empty());
        // Only Rust files go to rust-analyzer.
        l.sync("Cargo.toml", "[package]");
        assert!(sent(&mut l).is_empty());
    }

    #[test]
    fn change_ranges_in_utf16_and_utf8() {
        assert_eq!(
            change_between("aé😀b", "aé😀Xb", false),
            ((0, 4), (0, 4), String::from("X"))
        );
        assert_eq!(
            change_between("aé😀b", "aé😀Xb", true),
            ((0, 7), (0, 7), String::from("X"))
        );
        assert_eq!(
            change_between("one\ntwo\n", "one\n", true),
            ((1, 0), (2, 0), String::new())
        );
        assert_eq!(
            change_between("ab", "ab", true),
            ((0, 2), (0, 2), String::new())
        );
        let l = Lsp::new("/p");
        assert_eq!(l.to_lsp("é😀x", 6), 3);
        assert_eq!(l.from_lsp("é😀x", 3), 6);
    }

    #[test]
    fn the_server_asking_is_answered() {
        let mut l = ready();
        let req = json::parse(r#"{"jsonrpc":"2.0","id":7,"method":"workspace/configuration","params":{"items":[{"section":"rust-analyzer"},{}]}}"#).unwrap();
        l.receive(&rpc::frame(&req));
        let req = json::parse(r#"{"jsonrpc":"2.0","id":"tok","method":"window/workDoneProgress/create","params":{"token":"x"}}"#).unwrap();
        l.receive(&rpc::frame(&req));
        let m = sent(&mut l);
        assert_eq!(m[0].get("id").int(), Some(7));
        assert_eq!(m[0].get("result").arr().len(), 2);
        assert_eq!(m[1].get("id").str(), Some("tok"));
    }

    #[test]
    fn diagnostics_completion_hover_and_definition() {
        let mut l = ready();
        l.sync("src/main.rs", "fn main() {\n    let s = \"é\"; foo();\n}\n");
        sent(&mut l);
        let ev = l.receive(&notif(
            "textDocument/publishDiagnostics",
            r#"{"uri":"file:///home/me/my%20proj/src/main.rs","diagnostics":[{"range":{"start":{"line":1,"character":19},"end":{"line":1,"character":22}},"severity":1,"code":"E0425","message":"cannot find function `foo`\nmore"}]}"#,
        ));
        match &ev[0] {
            LspEvent::Diagnostics { path, items } => {
                assert_eq!(path.as_deref(), Some("src/main.rs"));
                // Byte 19 is character 18: the é takes two bytes.
                assert_eq!((items[0].line, items[0].col, items[0].end_col), (1, 18, 21));
                assert_eq!(items[0].message, "cannot find function `foo`");
                assert_eq!(items[0].code.as_deref(), Some("E0425"));
            }
            e => panic!("{e:?}"),
        }
        let at = Asked {
            path: String::from("src/main.rs"),
            line: 1,
            col: 4,
        };
        let id = l.completion(at.clone(), Some('.'));
        let m = sent(&mut l);
        assert_eq!(
            m[0].at(&["params", "context", "triggerCharacter"]).str(),
            Some(".")
        );
        let ev = l.receive(&reply(id, r#"{"isIncomplete":false,"items":[{"label":"len","kind":2,"detail":"fn(&self) -> usize","textEdit":{"range":{"start":{"line":1,"character":4},"end":{"line":1,"character":4}},"newText":"len()"}},{"label":"push_str","kind":2}]}"#));
        match &ev[0] {
            LspEvent::Completion { at: a, items } => {
                assert_eq!(a, &at);
                assert_eq!(items[0].kind_word(), "method");
                assert_eq!(items[0].edit.as_ref().unwrap().text, "len()");
                assert_eq!(items[1].insert, "push_str");
            }
            e => panic!("{e:?}"),
        }
        let id = l.hover(at.clone());
        sent(&mut l);
        let ev = l.receive(&reply(id, r#"{"contents":{"kind":"markdown","value":"```rust\nlet s: &str\n```\n---\nA string."}}"#));
        assert_eq!(
            ev[0],
            LspEvent::Hover {
                at: at.clone(),
                text: String::from("let s: &str\n\nA string.")
            }
        );
        let id = l.definition(at.clone());
        sent(&mut l);
        let ev = l.receive(&reply(id, r#"[{"uri":"file:///home/me/my%20proj/src/lib.rs","range":{"start":{"line":3,"character":7},"end":{"line":3,"character":10}}},{"uri":"file:///rust/lib/core/src/str.rs","range":{"start":{"line":1,"character":0},"end":{"line":1,"character":1}}}]"#));
        match &ev[0] {
            LspEvent::Definition { found, .. } => {
                assert_eq!(found[0].path.as_deref(), Some("src/lib.rs"));
                assert_eq!((found[0].line, found[0].col), (3, 7));
                assert_eq!(found[1].path, None);
                assert_eq!(found[1].outside, "/rust/lib/core/src/str.rs");
            }
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn rename_signature_and_code_actions() {
        let mut l = ready();
        let at = Asked {
            path: String::from("src/main.rs"),
            line: 0,
            col: 4,
        };
        let id = l.rename(at.clone(), "total");
        let m = sent(&mut l);
        assert_eq!(m[0].at(&["params", "newName"]).str(), Some("total"));
        let ev = l.receive(&reply(id, r#"{"documentChanges":[{"textDocument":{"uri":"file:///home/me/my%20proj/src/main.rs","version":3},"edits":[{"range":{"start":{"line":0,"character":4},"end":{"line":0,"character":7}},"newText":"total"}]}],"changes":{"file:///home/me/my%20proj/src/lib.rs":[{"range":{"start":{"line":2,"character":1},"end":{"line":2,"character":4}},"newText":"total"}]}}"#));
        match &ev[0] {
            LspEvent::Rename { edits, .. } => {
                assert_eq!(edits.len(), 2);
                assert_eq!(edits[0].path.as_deref(), Some("src/main.rs"));
                assert_eq!(edits[1].path.as_deref(), Some("src/lib.rs"));
                assert_eq!(edits[1].edits[0].text, "total");
            }
            e => panic!("{e:?}"),
        }
        let id = l.signature(at.clone());
        sent(&mut l);
        let ev = l.receive(&reply(id, r#"{"signatures":[{"label":"fn area(w: f64, h: f64) -> f64","parameters":[{"label":[8,14]},{"label":[16,22]}]}],"activeSignature":0,"activeParameter":1}"#));
        match &ev[0] {
            LspEvent::Signature { label, active, .. } => {
                assert_eq!(&label[active.unwrap().0..active.unwrap().1], "h: f64");
            }
            e => panic!("{e:?}"),
        }
        let id = l.code_actions(at);
        sent(&mut l);
        let ev = l.receive(&reply(id, r#"[{"title":"Remove the unused import","kind":"quickfix","edit":{"changes":{"file:///home/me/my%20proj/src/main.rs":[{"range":{"start":{"line":0,"character":0},"end":{"line":1,"character":0}},"newText":""}]}}},{"title":"Run","command":{"command":"rust-analyzer.runSingle"}}]"#));
        match &ev[0] {
            LspEvent::Actions { actions, .. } => {
                assert_eq!(
                    actions.len(),
                    1,
                    "a command-only action has nothing to apply"
                );
                assert_eq!(actions[0].title, "Remove the unused import");
            }
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn errors_and_cancellations() {
        let mut l = ready();
        let at = Asked {
            path: String::from("src/main.rs"),
            line: 0,
            col: 0,
        };
        let id = l.hover(at.clone());
        let id2 = l.definition(at);
        sent(&mut l);
        let cancelled = json::parse(&alloc::format!(r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":-32801,"message":"content modified"}}}}"#)).unwrap();
        assert!(l.receive(&rpc::frame(&cancelled)).is_empty());
        let failed = json::parse(&alloc::format!(
            r#"{{"jsonrpc":"2.0","id":{id2},"error":{{"code":-32603,"message":"no such thing"}}}}"#
        ))
        .unwrap();
        assert_eq!(
            l.receive(&rpc::frame(&failed)),
            alloc::vec![LspEvent::Say(String::from("rust-analyzer: no such thing"))]
        );
        assert_eq!(l.waiting(), 0);
        // Progress begins and ends.
        let ev = l.receive(&notif(
            "$/progress",
            r#"{"token":"rustAnalyzer/Indexing","value":{"kind":"begin","title":"Indexing"}}"#,
        ));
        assert_eq!(
            ev,
            alloc::vec![LspEvent::Progress(Some(String::from("Indexing")))]
        );
        let ev = l.receive(&notif(
            "$/progress",
            r#"{"token":"rustAnalyzer/Indexing","value":{"kind":"end"}}"#,
        ));
        assert_eq!(ev, alloc::vec![LspEvent::Progress(None)]);
        // Shutdown then exit.
        l.shutdown();
        let m = sent(&mut l);
        let sid = m[0].get("id").int().unwrap();
        l.receive(&reply(sid, "null"));
        assert_eq!(sent(&mut l)[0].get("method").str(), Some("exit"));
    }

    #[test]
    fn paths_and_addresses() {
        let l = Lsp::new("/private/var/t/p");
        assert_eq!(
            l.path_of("file:///var/t/p/src/a%20b.rs"),
            Ok(String::from("src/a b.rs"))
        );
        assert_eq!(l.uri("src/a b.rs"), "file:///private/var/t/p/src/a%20b.rs");
        assert_eq!(uri_from_path("C:\\Users\\me"), "file:///C:/Users/me");
        assert!(l.path_of("file:///elsewhere/x.rs").is_err());
    }
}

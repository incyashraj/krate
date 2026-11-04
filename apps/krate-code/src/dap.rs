//! A Debug Adapter Protocol client, for lldb-dap over its stdio.
//!
//! The same shape as the language client: the editor says what the person
//! wants ([`Dap::step_over`]), bytes to send collect in [`Dap::out`], and
//! what the adapter sends back goes into [`Dap::receive`] and comes out as
//! [`DapEvent`]s. Nothing waits.
//!
//! The sequence the protocol asks for is kept here: `initialize`, then
//! `launch` with the built program; when the adapter says `initialized`,
//! every breakpoint, then `configurationDone`. Each time the program stops,
//! the client asks for the call stack, the top frame's scopes and its local
//! variables, so the panel is full by the time the person looks.

use crate::json::{obj, Value};
use crate::rpc::{self, Framer};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DapState {
    Starting,
    Running,
    Stopped,
    Ended,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub id: i64,
    pub name: String,
    /// Relative to the project when the source is in it.
    pub path: Option<String>,
    /// Zero-based.
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Var {
    pub name: String,
    pub value: String,
    pub ty: String,
    /// Non-zero when it has children to ask for.
    pub children: i64,
    pub depth: usize,
    pub open: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DapEvent {
    /// The program stopped at `frame` (a breakpoint, a step, a panic).
    Stopped {
        reason: String,
    },
    Continued,
    /// The panel's stack or variables changed.
    Refreshed,
    /// The program printed this.
    Output(String),
    /// The program ended (with its exit code when known).
    Ended(Option<i64>),
    Say(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    Initialize,
    Launch,
    Breakpoints,
    Configured,
    Stack,
    Scopes,
    /// Variables for this reference, placed after row `parent` (or at the
    /// top) at `depth`.
    Variables {
        parent: Option<usize>,
        depth: usize,
    },
    Step,
    Disconnect,
}

pub struct Dap {
    seq: i64,
    pending: BTreeMap<i64, Pending>,
    framer: Framer,
    pub out: Vec<u8>,
    pub state: DapState,
    /// The project folder on the computer, for the adapter's paths.
    root: String,
    program: String,
    /// Breakpoint lines (zero-based) for each project file.
    breakpoints: BTreeMap<String, Vec<usize>>,
    pub thread: i64,
    pub frames: Vec<Frame>,
    pub selected: usize,
    pub vars: Vec<Var>,
    pub reason: String,
    pub exit_code: Option<i64>,
}

impl Dap {
    /// Debug `program` (an absolute path, as cargo reports it) with the
    /// project at `root` as its working folder.
    pub fn new(root: &str, program: &str, breakpoints: BTreeMap<String, Vec<usize>>) -> Self {
        let mut d = Dap {
            seq: 0,
            pending: BTreeMap::new(),
            framer: Framer::default(),
            out: Vec::new(),
            state: DapState::Starting,
            root: String::from(root.trim_end_matches('/')),
            program: String::from(program),
            breakpoints,
            thread: 0,
            frames: Vec::new(),
            selected: 0,
            vars: Vec::new(),
            reason: String::new(),
            exit_code: None,
        };
        let args = obj([
            ("clientID", "krate-code".into()),
            ("clientName", "Krate Code".into()),
            ("adapterID", "lldb-dap".into()),
            ("linesStartAt1", true.into()),
            ("columnsStartAt1", true.into()),
            ("pathFormat", "path".into()),
            ("supportsVariableType", true.into()),
            ("supportsRunInTerminalRequest", false.into()),
        ]);
        d.request(Pending::Initialize, "initialize", args);
        d
    }

    fn request(&mut self, p: Pending, command: &str, args: Value) -> i64 {
        self.seq += 1;
        let seq = self.seq;
        self.pending.insert(seq, p);
        let msg = obj([
            ("seq", seq.into()),
            ("type", "request".into()),
            ("command", command.into()),
            ("arguments", args),
        ]);
        self.out.extend_from_slice(&rpc::frame(&msg));
        seq
    }

    fn abs(&self, path: &str) -> String {
        let mut s = self.root.clone();
        s.push('/');
        s.push_str(path);
        s
    }

    fn rel(&self, abs: &str) -> Option<String> {
        for (a, r) in [
            (abs, self.root.as_str()),
            (
                abs.trim_start_matches("/private"),
                self.root.trim_start_matches("/private"),
            ),
        ] {
            if let Some(rest) = a.strip_prefix(r).and_then(|x| x.strip_prefix('/')) {
                return Some(String::from(rest));
            }
        }
        None
    }

    fn send_breakpoints(&mut self, path: &str) {
        let lines: Vec<Value> = self
            .breakpoints
            .get(path)
            .map(|ls| {
                ls.iter()
                    .map(|l| obj([("line", (*l + 1).into())]))
                    .collect()
            })
            .unwrap_or_default();
        let args = obj([
            (
                "source",
                obj([
                    ("path", self.abs(path).into()),
                    ("name", crate::project::file_name(path).into()),
                ]),
            ),
            ("breakpoints", Value::Arr(lines)),
        ]);
        self.request(Pending::Breakpoints, "setBreakpoints", args);
    }

    /// The person changed the breakpoints of `path` while debugging.
    pub fn set_breakpoints(&mut self, path: &str, lines: Vec<usize>) {
        self.breakpoints.insert(String::from(path), lines);
        if self.state != DapState::Starting && self.state != DapState::Ended {
            self.send_breakpoints(path);
        }
    }

    fn thread_args(&self) -> Value {
        obj([("threadId", self.thread.into())])
    }

    pub fn resume(&mut self) {
        if self.state == DapState::Stopped {
            let a = self.thread_args();
            self.request(Pending::Step, "continue", a);
        }
    }

    pub fn step_over(&mut self) {
        if self.state == DapState::Stopped {
            let a = self.thread_args();
            self.request(Pending::Step, "next", a);
        }
    }

    pub fn step_in(&mut self) {
        if self.state == DapState::Stopped {
            let a = self.thread_args();
            self.request(Pending::Step, "stepIn", a);
        }
    }

    pub fn step_out(&mut self) {
        if self.state == DapState::Stopped {
            let a = self.thread_args();
            self.request(Pending::Step, "stepOut", a);
        }
    }

    pub fn pause(&mut self) {
        if self.state == DapState::Running {
            let a = self.thread_args();
            self.request(Pending::Step, "pause", a);
        }
    }

    /// End the session and the program with it.
    pub fn stop(&mut self) {
        if self.state != DapState::Ended {
            self.request(
                Pending::Disconnect,
                "disconnect",
                obj([("terminateDebuggee", true.into())]),
            );
        }
    }

    /// Look at frame `i` of the stack: its variables.
    pub fn select_frame(&mut self, i: usize) {
        if let Some(f) = self.frames.get(i) {
            self.selected = i;
            let id = f.id;
            self.request(Pending::Scopes, "scopes", obj([("frameId", id.into())]));
        }
    }

    /// Open or close variable row `i`.
    pub fn toggle_var(&mut self, i: usize) {
        let Some(v) = self.vars.get(i).cloned() else {
            return;
        };
        if v.children == 0 {
            return;
        }
        if v.open {
            // Close: drop the rows below it that are deeper.
            let mut end = i + 1;
            while end < self.vars.len() && self.vars[end].depth > v.depth {
                end += 1;
            }
            self.vars.drain(i + 1..end);
            self.vars[i].open = false;
        } else {
            self.vars[i].open = true;
            self.request(
                Pending::Variables {
                    parent: Some(i),
                    depth: v.depth + 1,
                },
                "variables",
                obj([("variablesReference", v.children.into())]),
            );
        }
    }

    pub fn current(&self) -> Option<&Frame> {
        self.frames.get(self.selected)
    }

    pub fn receive(&mut self, bytes: &[u8]) -> Vec<DapEvent> {
        self.framer.push(bytes);
        let mut ev = Vec::new();
        for m in self.framer.drain() {
            match m.get("type").str() {
                Some("response") => {
                    let Some(seq) = m.get("request_seq").int() else {
                        continue;
                    };
                    let Some(p) = self.pending.remove(&seq) else {
                        continue;
                    };
                    if m.get("success").bool() == Some(false) {
                        self.failed(p, &m, &mut ev);
                    } else {
                        self.response(p, m.get("body"), &mut ev);
                    }
                }
                Some("event") => {
                    self.event(m.get("event").str().unwrap_or(""), m.get("body"), &mut ev)
                }
                Some("request") => {
                    // The adapter asking (runInTerminal): say no plainly.
                    let seq = m.get("seq").int().unwrap_or(0);
                    self.seq += 1;
                    let reply = obj([
                        ("seq", self.seq.into()),
                        ("type", "response".into()),
                        ("request_seq", seq.into()),
                        ("success", false.into()),
                        ("command", m.get("command").clone()),
                    ]);
                    self.out.extend_from_slice(&rpc::frame(&reply));
                }
                _ => {}
            }
        }
        ev
    }

    fn failed(&mut self, p: Pending, m: &Value, ev: &mut Vec<DapEvent>) {
        let why = m
            .get("message")
            .str()
            .or(m.at(&["body", "error", "format"]).str())
            .unwrap_or("it did not say why");
        match p {
            Pending::Initialize | Pending::Launch => {
                let mut s = String::from("The debugger could not start the program: ");
                s.push_str(why);
                ev.push(DapEvent::Say(s));
                self.state = DapState::Ended;
                ev.push(DapEvent::Ended(None));
            }
            Pending::Disconnect => {
                self.state = DapState::Ended;
                ev.push(DapEvent::Ended(self.exit_code));
            }
            _ => {}
        }
    }

    fn response(&mut self, p: Pending, body: &Value, ev: &mut Vec<DapEvent>) {
        match p {
            Pending::Initialize => {
                let args = obj([
                    ("program", self.program.as_str().into()),
                    ("cwd", self.root.as_str().into()),
                    ("args", Value::Arr(Vec::new())),
                    ("stopOnEntry", false.into()),
                ]);
                self.request(Pending::Launch, "launch", args);
            }
            Pending::Launch => {}
            Pending::Breakpoints => {
                let unverified = body
                    .get("breakpoints")
                    .arr()
                    .iter()
                    .filter(|b| b.get("verified").bool() == Some(false))
                    .count();
                if unverified > 0 {
                    ev.push(DapEvent::Say(String::from(
                        "A breakpoint is on a line with no code; it stops at the next line that has some, or not at all",
                    )));
                }
            }
            Pending::Configured => {
                if self.state == DapState::Starting {
                    self.state = DapState::Running;
                }
            }
            Pending::Stack => {
                self.frames = body
                    .get("stackFrames")
                    .arr()
                    .iter()
                    .map(|f| {
                        let path = f.at(&["source", "path"]).str().and_then(|p| self.rel(p));
                        Frame {
                            id: f.get("id").int().unwrap_or(0),
                            name: String::from(f.get("name").str().unwrap_or("?")),
                            path,
                            line: f.get("line").uint().unwrap_or(1).saturating_sub(1),
                        }
                    })
                    .collect();
                // Start at the first frame that is the project's own code:
                // a panic stops deep in the standard library.
                self.selected = self
                    .frames
                    .iter()
                    .position(|f| f.path.is_some())
                    .unwrap_or(0);
                if let Some(f) = self.frames.get(self.selected) {
                    let id = f.id;
                    self.request(Pending::Scopes, "scopes", obj([("frameId", id.into())]));
                }
                ev.push(DapEvent::Stopped {
                    reason: self.reason.clone(),
                });
            }
            Pending::Scopes => {
                self.vars.clear();
                // Locals first: the scope a person wants.
                let scopes = body.get("scopes").arr();
                let local = scopes
                    .iter()
                    .find(|s| {
                        s.get("name")
                            .str()
                            .is_some_and(|n| n.to_ascii_lowercase().contains("local"))
                    })
                    .or(scopes.first());
                if let Some(s) = local {
                    let r = s.get("variablesReference").int().unwrap_or(0);
                    if r != 0 {
                        self.request(
                            Pending::Variables {
                                parent: None,
                                depth: 0,
                            },
                            "variables",
                            obj([("variablesReference", r.into())]),
                        );
                    }
                }
                ev.push(DapEvent::Refreshed);
            }
            Pending::Variables { parent, depth } => {
                let rows: Vec<Var> = body
                    .get("variables")
                    .arr()
                    .iter()
                    .map(|v| Var {
                        name: String::from(v.get("name").str().unwrap_or("")),
                        value: String::from(v.get("value").str().unwrap_or("")),
                        ty: String::from(v.get("type").str().unwrap_or("")),
                        children: v.get("variablesReference").int().unwrap_or(0),
                        depth,
                        open: false,
                    })
                    .collect();
                let at = match parent {
                    Some(p) if p < self.vars.len() => p + 1,
                    Some(_) => return,
                    None => self.vars.len(),
                };
                for (k, r) in rows.into_iter().enumerate() {
                    self.vars.insert(at + k, r);
                }
                ev.push(DapEvent::Refreshed);
            }
            Pending::Step => {}
            Pending::Disconnect => {
                self.state = DapState::Ended;
                ev.push(DapEvent::Ended(self.exit_code));
            }
        }
    }

    fn event(&mut self, name: &str, body: &Value, ev: &mut Vec<DapEvent>) {
        match name {
            "initialized" => {
                let files: Vec<String> = self.breakpoints.keys().cloned().collect();
                for f in files {
                    self.send_breakpoints(&f);
                }
                self.request(Pending::Configured, "configurationDone", obj([]));
            }
            "stopped" => {
                self.state = DapState::Stopped;
                self.thread = body.get("threadId").int().unwrap_or(self.thread);
                let mut reason = String::from(body.get("reason").str().unwrap_or("paused"));
                if let Some(t) = body.get("description").str().or(body.get("text").str()) {
                    if !t.is_empty() && t != reason {
                        reason = String::from(t);
                    }
                }
                self.reason = reason;
                let args = obj([("threadId", self.thread.into()), ("levels", 40i64.into())]);
                self.request(Pending::Stack, "stackTrace", args);
            }
            "continued" => {
                self.state = DapState::Running;
                self.frames.clear();
                self.vars.clear();
                ev.push(DapEvent::Continued);
            }
            "output" => {
                let cat = body.get("category").str().unwrap_or("console");
                if cat != "telemetry" {
                    if let Some(t) = body.get("output").str() {
                        ev.push(DapEvent::Output(String::from(t)));
                    }
                }
            }
            "exited" => {
                self.exit_code = body.get("exitCode").int();
            }
            "terminated" if self.state != DapState::Ended => {
                self.state = DapState::Ended;
                self.frames.clear();
                self.vars.clear();
                ev.push(DapEvent::Ended(self.exit_code));
            }
            _ => {}
        }
    }

    /// A step or continue was asked and not yet answered by a stop.
    pub fn running(&self) -> bool {
        self.state == DapState::Running
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    fn sent(d: &mut Dap) -> Vec<Value> {
        let mut f = Framer::default();
        f.push(&core::mem::take(&mut d.out));
        f.drain()
    }

    fn resp(seq: i64, command: &str, body: &str) -> Vec<u8> {
        rpc::frame(&json::parse(&alloc::format!(r#"{{"seq":100,"type":"response","request_seq":{seq},"success":true,"command":"{command}","body":{body}}}"#)).unwrap())
    }

    fn event(name: &str, body: &str) -> Vec<u8> {
        rpc::frame(
            &json::parse(&alloc::format!(
                r#"{{"seq":101,"type":"event","event":"{name}","body":{body}}}"#
            ))
            .unwrap(),
        )
    }

    fn seq_of(msgs: &[Value], command: &str) -> i64 {
        msgs.iter()
            .find(|m| m.get("command").str() == Some(command))
            .and_then(|m| m.get("seq").int())
            .unwrap_or_else(|| panic!("no {command} in {msgs:?}"))
    }

    /// The whole handshake, a stop at a breakpoint with the panel filled,
    /// a step, and the end.
    #[test]
    fn launch_stop_inspect_step_end() {
        let mut bps = BTreeMap::new();
        bps.insert(String::from("src/main.rs"), alloc::vec![4]);
        let mut d = Dap::new("/home/me/p", "/home/me/p/target/debug/p", bps);
        let m = sent(&mut d);
        assert_eq!(m[0].get("command").str(), Some("initialize"));
        d.receive(&resp(1, "initialize", "{}"));
        let m = sent(&mut d);
        let launch = &m[0];
        assert_eq!(launch.get("command").str(), Some("launch"));
        assert_eq!(
            launch.at(&["arguments", "program"]).str(),
            Some("/home/me/p/target/debug/p")
        );
        d.receive(&event("initialized", "{}"));
        let m = sent(&mut d);
        assert_eq!(m[0].get("command").str(), Some("setBreakpoints"));
        assert_eq!(
            m[0].at(&["arguments", "source", "path"]).str(),
            Some("/home/me/p/src/main.rs")
        );
        assert_eq!(
            m[0].at(&["arguments", "breakpoints"])
                .idx(0)
                .get("line")
                .int(),
            Some(5)
        );
        assert_eq!(m[1].get("command").str(), Some("configurationDone"));
        d.receive(&resp(
            m[1].get("seq").int().unwrap(),
            "configurationDone",
            "{}",
        ));
        assert_eq!(d.state, DapState::Running);

        d.receive(&event(
            "output",
            r#"{"category":"stdout","output":"hello\n"}"#,
        ));
        let ev = d.receive(&event("stopped", r#"{"reason":"breakpoint","threadId":7}"#));
        assert!(ev.is_empty(), "the stop is reported once the stack is in");
        assert_eq!(d.state, DapState::Stopped);
        let m = sent(&mut d);
        let st = seq_of(&m, "stackTrace");
        assert_eq!(m[0].at(&["arguments", "threadId"]).int(), Some(7));
        let ev = d.receive(&resp(st, "stackTrace", r#"{"stackFrames":[{"id":11,"name":"core::panicking::panic","line":3,"source":{"path":"/rustc/abc/library/core/src/panicking.rs"}},{"id":12,"name":"p::main","line":5,"source":{"path":"/home/me/p/src/main.rs"}}]}"#));
        assert_eq!(
            ev,
            alloc::vec![DapEvent::Stopped {
                reason: String::from("breakpoint")
            }]
        );
        assert_eq!(d.selected, 1, "the project's own frame is shown first");
        assert_eq!(d.current().unwrap().path.as_deref(), Some("src/main.rs"));
        assert_eq!(d.current().unwrap().line, 4);
        let m = sent(&mut d);
        let sc = seq_of(&m, "scopes");
        assert_eq!(m[0].at(&["arguments", "frameId"]).int(), Some(12));
        d.receive(&resp(sc, "scopes", r#"{"scopes":[{"name":"Locals","variablesReference":100},{"name":"Globals","variablesReference":200}]}"#));
        let m = sent(&mut d);
        let vs = seq_of(&m, "variables");
        assert_eq!(
            m[0].at(&["arguments", "variablesReference"]).int(),
            Some(100)
        );
        d.receive(&resp(vs, "variables", r#"{"variables":[{"name":"total","value":"42","type":"i32","variablesReference":0},{"name":"shapes","value":"size=2","type":"Vec<Shape>","variablesReference":101}]}"#));
        assert_eq!(d.vars.len(), 2);
        assert_eq!(
            (d.vars[0].name.as_str(), d.vars[0].value.as_str()),
            ("total", "42")
        );

        // Open the vector: its items go under it, one deeper; close again.
        d.toggle_var(1);
        let m = sent(&mut d);
        let vs = seq_of(&m, "variables");
        d.receive(&resp(vs, "variables", r#"{"variables":[{"name":"[0]","value":"Circle","variablesReference":0},{"name":"[1]","value":"Square","variablesReference":0}]}"#));
        assert_eq!(d.vars.len(), 4);
        assert_eq!((d.vars[2].name.as_str(), d.vars[2].depth), ("[0]", 1));
        d.toggle_var(1);
        assert_eq!(d.vars.len(), 2);

        d.step_over();
        let m = sent(&mut d);
        assert_eq!(m[0].get("command").str(), Some("next"));
        assert_eq!(m[0].at(&["arguments", "threadId"]).int(), Some(7));
        assert_eq!(
            d.receive(&event("continued", r#"{"threadId":7}"#)),
            alloc::vec![DapEvent::Continued]
        );
        assert!(d.vars.is_empty());

        d.receive(&event("exited", r#"{"exitCode":0}"#));
        assert_eq!(
            d.receive(&event("terminated", "{}")),
            alloc::vec![DapEvent::Ended(Some(0))]
        );
        assert_eq!(d.state, DapState::Ended);
    }

    #[test]
    fn a_failed_launch_says_why() {
        let mut d = Dap::new("/p", "/p/target/debug/p", BTreeMap::new());
        sent(&mut d);
        d.receive(&resp(1, "initialize", "{}"));
        sent(&mut d);
        let fail = rpc::frame(&json::parse(r#"{"seq":5,"type":"response","request_seq":2,"success":false,"command":"launch","message":"the program does not exist"}"#).unwrap());
        let ev = d.receive(&fail);
        assert_eq!(
            ev[0],
            DapEvent::Say(String::from(
                "The debugger could not start the program: the program does not exist"
            ))
        );
        assert_eq!(d.state, DapState::Ended);
    }

    #[test]
    fn breakpoints_changed_mid_session_and_stop() {
        let mut d = Dap::new("/p", "/p/x", BTreeMap::new());
        d.receive(&resp(1, "initialize", "{}"));
        d.receive(&event("initialized", "{}"));
        sent(&mut d);
        d.state = DapState::Running;
        d.set_breakpoints("src/lib.rs", alloc::vec![1, 9]);
        let m = sent(&mut d);
        assert_eq!(m[0].at(&["arguments", "breakpoints"]).arr().len(), 2);
        d.stop();
        let m = sent(&mut d);
        assert_eq!(m[0].get("command").str(), Some("disconnect"));
        let ev = d.receive(&resp(m[0].get("seq").int().unwrap(), "disconnect", "{}"));
        assert_eq!(ev, alloc::vec![DapEvent::Ended(None)]);
    }
}

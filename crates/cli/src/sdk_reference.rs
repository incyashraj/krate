//! The list of functions a guest app can actually call.
//!
//! An agent asked to port a program was given rules and prohibitions -- no
//! growable `Vec`, no `format!`, only `krate:*` imports -- but never a list of
//! what exists. So it guessed. Porting hexyl, Claude wrote `stdio::write(bytes)`
//! three times; there is no such function, and the build failed on all three.
//!
//! Guessing is the predictable outcome of asking someone to write against an
//! API they cannot see. This generates the reference from the SDK source, so it
//! is accurate by construction and cannot drift the way a hand-written list
//! would.

/// The guest SDK source this binary ships, baked in at compile time.
///
/// Read from the same file the build script embeds, so the reference an agent
/// is handed always describes the SDK it will actually compile against.
pub const GUEST_SDK_SOURCE: &str = include_str!("../../bindings-rust/src/lib.rs");

/// One callable function in the guest SDK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkFunction {
    /// The module path a guest writes, e.g. `stdio`.
    pub module: String,
    /// The function name.
    pub name: String,
    /// Its parameters, as written.
    pub params: String,
    /// Its return type, trimmed.
    pub returns: String,
    /// True when it is called on a value rather than as a free function.
    pub is_method: bool,
    /// For a method, the type it is called on. Empty for a free function.
    pub receiver: String,
    /// Where it was found, used to tell a nested declaration from a sibling.
    offset: usize,
}

impl SdkFunction {
    /// How a guest would write the call, which is what an agent needs to see.
    pub fn signature(&self) -> String {
        if self.is_method {
            // Called on a value, not through a path -- writing it as
            // `io::streams::write_bytes(..)` would not compile.
            format!(
                "<{}>.{}({}) -> {}",
                self.receiver, self.name, self.params, self.returns
            )
        } else {
            format!(
                "{}::{}({}) -> {}",
                self.module, self.name, self.params, self.returns
            )
        }
    }
}

/// Parse the guest SDK's public surface out of its source.
///
/// Reads `pub fn` declarations inside each `pub mod`, which is exactly the set
/// a guest can call. Modules nest -- `stdio` lives inside `io`, so a guest
/// writes `io::stdio::println` -- and the full path is recorded, because a
/// reference that gives the wrong path fails the same way as no reference.
/// Private helpers are skipped because they are not reachable from an app.
pub fn parse_sdk(source: &str) -> Vec<SdkFunction> {
    let mut out = Vec::new();
    collect(source, "", &mut out);
    drop_shadowed_aliases(&mut out);
    out.sort_by(|a, b| (&a.module, &a.name).cmp(&(&b.module, &b.name)));
    out
}

/// Remove a nested function when its parent re-exposes the same call.
///
/// `time::now_millis` is a one-line wrapper over `time::clock::now_millis`.
/// Both work, but offering an agent two spellings of one call invites
/// inconsistent code for no gain, so only the shorter parent path is listed.
fn drop_shadowed_aliases(functions: &mut Vec<SdkFunction>) {
    let parents: Vec<(String, String)> = functions
        .iter()
        .filter(|f| !f.is_method)
        .map(|f| (f.module.clone(), f.name.clone()))
        .collect();

    // A trait can be re-exported into another module -- `OutputStreamExt` is
    // declared in `io::streams` and pulled into `fs`. The method is the same
    // call either way, identified by its receiver and name, not by the module
    // it was reached through.
    let mut seen_methods: Vec<(String, String)> = Vec::new();

    functions.retain(|func| {
        if func.is_method {
            let key = (func.receiver.clone(), func.name.clone());
            if seen_methods.contains(&key) {
                return false;
            }
            seen_methods.push(key);
            return true;
        }
        // Is there a shorter path in an ancestor module with this same name?
        let Some((parent, _)) = func.module.rsplit_once("::") else {
            return true;
        };
        !parents.iter().any(|(m, n)| m == parent && n == &func.name)
    });
}

/// Walk every `pub mod` in a body, recording its functions and recursing.
fn collect(body: &str, prefix: &str, out: &mut Vec<SdkFunction>) {
    for (start, name) in module_starts(body) {
        // `bindings` is the generated layer the SDK wraps, and `prelude` is a
        // re-export list. Neither is what a guest writes.
        if name == "bindings" || name == "prelude" {
            continue;
        }
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}::{name}")
        };
        let inner = module_body(body, start);
        for func in functions_in(&inner) {
            out.push(SdkFunction {
                module: path.clone(),
                ..func
            });
        }
        for func in trait_methods_in(&inner) {
            out.push(SdkFunction {
                module: path.clone(),
                ..func
            });
        }
        collect(&inner, &path, out);
    }
}

/// Byte offsets and names of every `pub mod X {` directly in this body.
///
/// Only the outermost level is returned; the caller recurses into each body, so
/// a nested module is not also reported as a sibling of its parent.
fn module_starts(source: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let needle = "pub mod ";
    let mut at = 0usize;

    while let Some(found) = source[at..].find(needle) {
        let idx = at + found;
        let rest = &source[idx + needle.len()..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if let Some(brace) = rest.find('{') {
            // A `pub mod x;` declaration has no body to read.
            if !rest[..brace].contains(';') && !name.is_empty() {
                let body_start = idx + needle.len() + brace + 1;
                let body = module_body(source, body_start);
                // Skip past this module's body so its children are found by the
                // recursive call, not treated as siblings here.
                at = body_start + body.len();
                out.push((body_start, name));
                continue;
            }
        }
        at = idx + needle.len();
    }

    out
}

/// The text between a module's opening brace and its match.
fn module_body(source: &str, start: usize) -> String {
    let mut depth = 1usize;
    let bytes = source.as_bytes();
    let mut i = start;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    source[start..i.saturating_sub(1)].to_string()
}

/// Public functions declared directly in a module body.
///
/// Functions inside a nested `pub mod` are left for the recursive walk, so that
/// `io::stdio::println` is not also reported as `io::println`.
fn functions_in(body: &str) -> Vec<SdkFunction> {
    let mut out = Vec::new();
    let mut skip: Vec<(usize, usize)> = module_starts(body)
        .into_iter()
        .map(|(start, _)| (start, start + module_body(body, start).len()))
        .collect();
    // An `impl` block re-declares the trait's methods with bodies. Counting
    // those too would list every stream method twice.
    skip.extend(block_spans(body, "impl "));

    for func in signatures(body, "pub fn ") {
        // Skip anything inside a nested module or an impl; those are reported
        // by the recursive walk and by `trait_methods_in`.
        if skip
            .iter()
            .any(|(s, e)| func.offset >= *s && func.offset < *e)
        {
            continue;
        }
        out.push(func);
    }

    out
}

/// Spans of every `<keyword> ... { .. }` block in a body.
fn block_spans(body: &str, keyword: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(found) = body[at..].find(keyword) {
        let idx = at + found;
        let rest = &body[idx + keyword.len()..];
        let Some(brace) = rest.find('{') else { break };
        let start = idx + keyword.len() + brace + 1;
        let len = module_body(body, start).len();
        out.push((start, start + len));
        at = start + len;
    }
    out
}

/// Parse every `<needle>name(params) -> ret` declaration in a body.
fn signatures(body: &str, needle: &str) -> Vec<SdkFunction> {
    let mut out = Vec::new();
    let mut at = 0usize;

    while let Some(found) = body[at..].find(needle) {
        let idx = at + found;
        let rest = &body[idx + needle.len()..];

        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();

        // Parameters, balanced so a nested type such as `&[u8; N]` survives.
        if let Some(open) = rest.find('(') {
            let mut depth = 0usize;
            let mut close = None;
            for (offset, ch) in rest[open..].char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            close = Some(open + offset);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if let Some(close) = close {
                let params = rest[open + 1..close].trim().to_string();
                let tail = &rest[close + 1..];
                // A free function's return type ends at its body; a trait
                // method has no body and ends at `;`. Take whichever comes
                // first, or the declaration reads as returning nothing.
                let end = [tail.find('{'), tail.find(';')].into_iter().flatten().min();
                let returns = match (tail.find("->"), end) {
                    (Some(arrow), Some(end)) if arrow < end => {
                        tail[arrow + 2..end].trim().to_string()
                    }
                    (Some(arrow), None) => tail[arrow + 2..].trim().to_string(),
                    _ => "()".to_string(),
                };
                if !name.is_empty() {
                    out.push(SdkFunction {
                        module: String::new(),
                        name,
                        params: collapse_whitespace(&params),
                        returns: collapse_whitespace(&returns),
                        is_method: false,
                        receiver: String::new(),
                        offset: idx,
                    });
                }
            }
        }

        at = idx + needle.len();
    }

    out
}

/// Methods declared in a `pub trait`, which a guest calls on a value.
///
/// `stdout().write_bytes(b"..")` is a real call an app makes, but the method
/// lives in a trait rather than as a free function, so scanning only for
/// `pub fn` would leave the whole streams API out of the reference.
fn trait_methods_in(body: &str) -> Vec<SdkFunction> {
    let mut out = Vec::new();
    let needle = "pub trait ";
    let mut at = 0usize;

    while let Some(found) = body[at..].find(needle) {
        let idx = at + found;
        let rest = &body[idx + needle.len()..];
        let Some(brace) = rest.find('{') else { break };

        let trait_name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        // What the method is actually called on. `OutputStreamExt` is
        // implemented for `OutputStream`, and that is the name a guest holds.
        let receiver = implementing_type(body, &trait_name).unwrap_or(trait_name);

        let start = idx + needle.len() + brace + 1;
        let inner = module_body(body, start);

        // Inside a trait, `fn` is public by definition -- there is no `pub`.
        for mut func in signatures(&inner, "fn ") {
            // The receiver is the value it is called on, not an argument.
            func.params = strip_receiver(&func.params);
            func.is_method = true;
            func.receiver = receiver.clone();
            out.push(func);
        }

        at = start + inner.len();
    }

    out
}

/// The type a trait is implemented for, read from `impl Trait for Type`.
fn implementing_type(body: &str, trait_name: &str) -> Option<String> {
    let needle = format!("impl {trait_name} for ");
    let at = body.find(&needle)? + needle.len();
    let name: String = body[at..]
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Drop `&self` / `&mut self` / `self` from a parameter list.
fn strip_receiver(params: &str) -> String {
    let rest = params
        .strip_prefix("&mut self")
        .or_else(|| params.strip_prefix("&self"))
        .or_else(|| params.strip_prefix("self"))
        .unwrap_or(params);
    rest.trim_start().trim_start_matches(',').trim().to_string()
}

/// A signature split across lines reads as one line in a reference.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Render the reference an agent reads, grouped by module.
/// List the widget kinds a window can build, for the porting agent.
///
/// These are not functions, so the SDK parser above never sees them -- it reads
/// the Rust guest SDK, which is phase2 and has no UI. That left the reference
/// listing every file and network call and not one widget, directly above a
/// line telling the agent that anything unlisted does not exist. An agent
/// following that instruction correctly would decide a windowed app cannot be
/// ported.
///
/// Generated from the same enum the hosts match on, so a widget cannot be added
/// to the system and stay missing from the page the agent reads.
fn render_widget_kinds() -> String {
    use krate_adapter_common::ui::WidgetKind;

    // Named individually rather than iterated: the enum carries no iterator,
    // and an explicit list means adding a kind fails to compile here until it
    // is documented. Each note says what the kind is *for*, which is the part
    // an agent cannot infer from the name.
    const KINDS: &[(WidgetKind, &str)] = &[
        (WidgetKind::Stack, "flex row or column; the usual root"),
        (WidgetKind::Grid, "wrapping grid"),
        (WidgetKind::Scroll, "scrolls its children"),
        (
            WidgetKind::Tabs,
            "tab strip; `selected` picks the visible panel",
        ),
        (WidgetKind::Button, "`label` is the title"),
        (WidgetKind::Checkbox, "`checked` is the state"),
        (WidgetKind::Radio, "one of a set; `checked` is the state"),
        (WidgetKind::Switch, "on/off; `checked` is the state"),
        (WidgetKind::Slider, "`value` is 0.0..=1.0"),
        (WidgetKind::Progress, "`value` is 0.0..=1.0"),
        (WidgetKind::Text, "static label"),
        (WidgetKind::TextField, "one line the person can type in"),
        (WidgetKind::TextArea, "many lines the person can type in"),
        (WidgetKind::ListView, "rows; `selected` is the chosen index"),
        (
            WidgetKind::TreeView,
            "nested rows; `selected` is the chosen index",
        ),
        (
            WidgetKind::Image,
            "a picture; fill it with `image::set_pixels`, see \"Showing a picture\"",
        ),
        (WidgetKind::Canvas, "a region the app positions children in"),
    ];

    let mut out = String::from("\n### Widget kinds\n\n");
    out.push_str(
        "Build a window from `types::WidgetNode` values. Every kind here draws on\n\
         macOS, Windows, and Linux -- there is no kind that works on one system only.\n\n",
    );
    for (kind, note) in KINDS {
        out.push_str(&format!("- `{kind:?}` -- {note}\n"));
    }
    out
}

pub fn render_reference(functions: &[SdkFunction]) -> String {
    let mut out = String::new();
    out.push_str("## Every function you can call\n\n");
    out.push_str(
        "This is the whole guest API. If something you want is not here, it does not\n\
         exist -- do not invent a call. Work with what is listed, or say in your report\n\
         that the behaviour cannot be ported.\n\n",
    );
    out.push_str(&render_widget_kinds());

    let mut current = String::new();
    for func in functions.iter().filter(|f| !f.is_method) {
        if func.module != current {
            current = func.module.clone();
            out.push_str(&format!("\n### `{current}`\n\n"));
        }
        out.push_str(&format!("- `{}`\n", func.signature()));
    }

    // Methods are called on a value, so they are grouped by the type that
    // value has, not by the module the type was imported from.
    let mut methods: Vec<&SdkFunction> = functions.iter().filter(|f| f.is_method).collect();
    methods.sort_by(|a, b| (&a.receiver, &a.name).cmp(&(&b.receiver, &b.name)));

    if !methods.is_empty() {
        out.push_str("\n### Methods, called on a value\n\n");
        out.push_str(
            "These are not paths. Get the value first -- `io::stdio::stdout()`,\n\
             `fs::open(..)` -- then call the method on it.\n\n",
        );
        let mut receiver = String::new();
        for func in methods {
            if func.receiver != receiver {
                receiver = func.receiver.clone();
                out.push_str(&format!("\n`{receiver}`\n\n"));
            }
            out.push_str(&format!("- `{}`\n", func.signature()));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped like the real SDK: `stdio` is nested inside `io`.
    const SAMPLE: &str = r#"
pub mod bindings { pub fn skipped() -> u32 { 0 } }

pub mod io {
    pub fn flush() -> Result<(), IoError> { todo!() }

    /// Text output.
    pub mod stdio {
        pub fn println(value: &str) -> Result<(), IoError> { todo!() }
        pub fn print(value: &str) -> Result<(), IoError> { todo!() }
        fn private_helper() -> u32 { 0 }
    }
}

pub mod store {
    pub fn get(key: &str) -> Result<Option<Vec<u8>>, StoreError> { todo!() }
}
"#;

    #[test]
    fn it_finds_what_a_guest_can_call() {
        let fns = parse_sdk(SAMPLE);
        let names: Vec<_> = fns.iter().map(|f| f.signature()).collect();
        assert!(names
            .iter()
            .any(|s| s.starts_with("io::stdio::println(value: &str)")));
        assert!(names.iter().any(|s| s.starts_with("store::get(key: &str)")));
    }

    #[test]
    fn a_nested_module_gets_the_path_a_guest_actually_writes() {
        // `stdio` lives inside `io`, so the call is `io::stdio::println`.
        // Reporting it as `stdio::println` would send the agent to a path that
        // does not resolve -- the same build failure, from the opposite cause.
        let fns = parse_sdk(SAMPLE);
        assert!(fns.iter().any(|f| f.module == "io::stdio"));
        assert!(
            !fns.iter().any(|f| f.module == "stdio"),
            "a nested module must not be reported at the top level"
        );
        // And a nested function is not also attributed to its parent.
        assert!(
            !fns.iter().any(|f| f.module == "io" && f.name == "println"),
            "io::stdio::println must not appear as io::println"
        );
        // The parent's own function still belongs to the parent.
        assert!(fns.iter().any(|f| f.module == "io" && f.name == "flush"));
    }

    #[test]
    fn the_reference_lists_every_widget_a_window_can_use() {
        // The reference is generated from the Rust guest SDK, which is phase2
        // and has no UI, so widgets were absent -- directly under a line
        // telling the agent that anything unlisted does not exist. An agent
        // reading that correctly would conclude a windowed app is unportable.
        let rendered = render_reference(&[]);
        for kind in [
            "Stack",
            "Grid",
            "Scroll",
            "Tabs",
            "Button",
            "Checkbox",
            "Radio",
            "Switch",
            "Slider",
            "Progress",
            "Text",
            "TextField",
            "TextArea",
            "ListView",
            "TreeView",
            "Image",
            "Canvas",
        ] {
            assert!(
                rendered.contains(&format!("`{kind}`")),
                "the agent reference must list the {kind} widget"
            );
        }
        // The picture field is the part an agent cannot guess from a kind name.
        assert!(rendered.contains("pixels"));
    }

    #[test]
    fn generated_bindings_and_private_helpers_are_left_out() {
        let fns = parse_sdk(SAMPLE);
        // `bindings` is the layer the SDK wraps, not something a guest writes.
        assert!(!fns.iter().any(|f| f.module == "bindings"));
        // A private helper is not callable from an app.
        assert!(!fns.iter().any(|f| f.name == "private_helper"));
    }

    #[test]
    fn the_reference_names_the_function_that_was_invented() {
        // The concrete failure this exists to prevent: an agent porting hexyl
        // wrote `stdio::write(bytes)` three times. There is no such function,
        // and nothing it had been given said so.
        let fns = parse_sdk(SAMPLE);
        let text = render_reference(&fns);
        assert!(text.contains("io::stdio::println"));
        assert!(!text.contains("io::stdio::write"));
        assert!(
            text.contains("it does not\nexist -- do not invent a call"),
            "the reference must say what to do when something is missing"
        );
    }

    /// The SDK this binary actually ships, not a sample of it.
    const REAL_SDK: &str = super::GUEST_SDK_SOURCE;

    #[test]
    fn the_real_sdk_parses_into_a_usable_reference() {
        let fns = parse_sdk(REAL_SDK);
        // A parser that quietly returns nothing on the real file would ship an
        // empty reference and leave the agent guessing exactly as before.
        assert!(
            fns.len() > 40,
            "expected the real SDK's surface, got {} functions",
            fns.len()
        );

        let paths: Vec<String> = fns.iter().map(|f| f.signature()).collect();
        let has = |needle: &str| paths.iter().any(|p| p.starts_with(needle));

        // One from each area a ported program actually reaches for.
        assert!(has("io::stdio::println("), "text output");
        assert!(has("io::args::raw("), "reading arguments");
        assert!(has("store::"), "key-value storage");
        assert!(has("fs::"), "files");

        // Every entry must be callable as written: a path and a real name.
        for func in &fns {
            assert!(!func.name.is_empty(), "unnamed function in {}", func.module);
            assert!(
                !func.module.is_empty(),
                "function {} has no module path",
                func.name
            );
        }
    }

    #[test]
    fn the_reference_covers_what_the_hexyl_port_reached_for() {
        // hexyl needed to write raw bytes. The agent invented `stdio::write`.
        // Whatever the answer is, the reference must contain a real function
        // that emits bytes -- otherwise the honest outcome is "cannot port",
        // and the reference must be what tells the agent that.
        let fns = parse_sdk(REAL_SDK);
        let byte_writers: Vec<&SdkFunction> = fns
            .iter()
            .filter(|f| f.params.contains("[u8]") && f.module.starts_with("io"))
            .collect();
        assert!(
            !byte_writers.is_empty(),
            "no io function takes bytes; a hex viewer cannot be ported and the \
             reference is what must say so. io functions present: {:?}",
            fns.iter()
                .filter(|f| f.module.starts_with("io"))
                .map(|f| f.signature())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_method_says_what_it_is_called_on_and_what_it_returns() {
        let fns = parse_sdk(REAL_SDK);
        let write = fns
            .iter()
            .find(|f| f.name == "write_bytes")
            .expect("write_bytes is in the SDK");

        // It is called on an OutputStream, not on the module. Naming the trait
        // or the module would send the agent to a call that does not compile.
        assert_eq!(write.receiver, "OutputStream");
        assert_eq!(
            write.signature(),
            "<OutputStream>.write_bytes(bytes: &[u8]) -> Result<(), IoError>"
        );

        // A trait method's declaration ends at `;` rather than a body, which
        // is what once made every method look like it returned nothing.
        assert!(
            !fns.iter().any(|f| f.is_method && f.returns == "()"),
            "a method lost its return type: {:?}",
            fns.iter()
                .filter(|f| f.is_method && f.returns == "()")
                .map(|f| f.signature())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn one_call_is_listed_one_way() {
        let fns = parse_sdk(REAL_SDK);
        // `time::now_millis` wraps `time::clock::now_millis`. Listing both
        // would offer two spellings of the same call.
        assert!(fns
            .iter()
            .any(|f| f.module == "time" && f.name == "now_millis"));
        assert!(
            !fns.iter()
                .any(|f| f.module == "time::clock" && f.name == "now_millis"),
            "the wrapped alias should not also be listed"
        );

        let mut seen: Vec<String> = fns.iter().map(|f| f.signature()).collect();
        seen.sort();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "the reference lists a call twice");
    }

    #[test]
    fn a_signature_reads_the_way_a_guest_writes_it() {
        let fns = parse_sdk(SAMPLE);
        let println = fns.iter().find(|f| f.name == "println").expect("println");
        assert_eq!(
            println.signature(),
            "io::stdio::println(value: &str) -> Result<(), IoError>"
        );
    }
}

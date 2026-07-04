use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use krate_manifest::supported_phase2_capability_specs;
use wit_parser::{
    Function, FunctionKind, Interface, Resolve, Type, TypeDefKind, TypeId, WorldItem,
};

fn main() -> Result<()> {
    let root = workspace_root();
    let wit_dir = root.join("wit/krate/phase2");
    let output = root.join("docs/book/src/reference/uapi/index.md");

    let mut resolve = Resolve::default();
    let (app_package, _) = resolve
        .push_dir(&wit_dir)
        .with_context(|| format!("parse WIT package at {}", wit_dir.display()))?;

    let world_id = resolve
        .select_world(&[app_package], Some("cli"))
        .context("select krate:app/cli world")?;

    let markdown = render_reference(&resolve, world_id)?;

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create output directory {}", parent.display()))?;
    }
    fs::write(&output, markdown).with_context(|| format!("write {}", output.display()))?;

    Ok(())
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn render_reference(resolve: &Resolve, world_id: wit_parser::WorldId) -> Result<String> {
    let world = &resolve.worlds[world_id];
    let package = world
        .package
        .map(|id| resolve.packages[id].name.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let mut out = String::new();
    writeln!(out, "# UAPI Reference")?;
    writeln!(out)?;
    writeln!(
        out,
        "> Generated from `wit/krate/phase2`. Do not edit this page by hand."
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "Krate Phase 2 exposes the `{}` world from `{}`.",
        world.name, package
    )?;
    writeln!(out)?;
    writeln!(out, "The current world imports these interfaces:")?;
    writeln!(out)?;

    for (key, item) in &world.imports {
        if let WorldItem::Interface { id, .. } = item {
            let interface = &resolve.interfaces[*id];
            let name = interface_name(resolve, interface);
            writeln!(out, "- `{name}`")?;
        } else {
            writeln!(out, "- `{}`", String::from(key.clone()))?;
        }
    }

    writeln!(out)?;
    writeln!(out, "The app exports:")?;
    writeln!(out)?;
    for (key, item) in &world.exports {
        match item {
            WorldItem::Function(function) => {
                writeln!(out, "- `{}`", render_function(resolve, function))?;
            }
            _ => writeln!(out, "- `{}`", String::from(key.clone()))?,
        }
    }

    let mut interfaces = world
        .imports
        .values()
        .filter_map(|item| match item {
            WorldItem::Interface { id, .. } => Some(*id),
            _ => None,
        })
        .collect::<Vec<_>>();
    interfaces.sort_by_key(|id| interface_name(resolve, &resolve.interfaces[*id]));

    for id in interfaces {
        render_interface(resolve, id, &mut out)?;
    }

    Ok(out)
}

fn render_interface(
    resolve: &Resolve,
    id: wit_parser::InterfaceId,
    out: &mut String,
) -> Result<()> {
    let interface = &resolve.interfaces[id];
    let name = interface_name(resolve, interface);

    writeln!(out)?;
    writeln!(out, "## `{name}`")?;
    writeln!(out)?;

    if let Some(summary) = interface_summary(&name) {
        writeln!(out, "{summary}")?;
        writeln!(out)?;
    }

    let capability_patterns = capability_patterns_for_interface(&name);
    let notes = capability_notes(&name);
    if !capability_patterns.is_empty() || !notes.is_empty() {
        writeln!(out, "### Capability Notes")?;
        writeln!(out)?;

        if !capability_patterns.is_empty() {
            writeln!(
                out,
                "Accepted capability strings for this module, generated from the runtime manifest table:"
            )?;
            writeln!(out)?;
            for pattern in capability_patterns {
                writeln!(out, "- `{}` - {}", pattern.capability, pattern.grant_kind)?;
            }
            writeln!(out)?;
        }

        for note in notes {
            writeln!(out, "- {note}")?;
        }
        if !notes.is_empty() {
            writeln!(out)?;
        }
    }

    if let Some(example) = rust_example(&name) {
        writeln!(out, "### Rust SDK Example")?;
        writeln!(out)?;
        writeln!(out, "```rust")?;
        write!(out, "{example}")?;
        writeln!(out, "```")?;
        writeln!(out)?;
    }

    let resource_type_ids = interface
        .types
        .values()
        .copied()
        .filter(|type_id| matches!(resolve.types[*type_id].kind, TypeDefKind::Resource))
        .collect::<Vec<_>>();

    let freestanding = interface
        .functions
        .values()
        .filter(|func| func.kind.resource().is_none())
        .collect::<Vec<_>>();

    if !freestanding.is_empty() {
        writeln!(out, "### Functions")?;
        writeln!(out)?;
        for func in freestanding {
            render_function_entry(resolve, &name, func, out)?;
        }
        writeln!(out)?;
    }

    let visible_types = interface
        .types
        .values()
        .copied()
        .filter(|type_id| is_renderable_type_def(resolve, *type_id))
        .collect::<Vec<_>>();

    if !visible_types.is_empty() {
        writeln!(out, "### Types")?;
        writeln!(out)?;
        for type_id in visible_types {
            render_type_def(resolve, type_id, out)?;
        }
    }

    for type_id in resource_type_ids {
        let methods = interface
            .functions
            .values()
            .filter(|func| func.kind.resource() == Some(type_id))
            .collect::<Vec<_>>();
        if methods.is_empty() {
            continue;
        }

        let name = resolve.types[type_id].name.as_deref().unwrap_or("resource");
        writeln!(out, "#### `{name}` methods")?;
        writeln!(out)?;
        for method in methods {
            render_function_entry(resolve, &interface_name(resolve, interface), method, out)?;
        }
        writeln!(out)?;
    }

    Ok(())
}

fn render_type_def(resolve: &Resolve, type_id: TypeId, out: &mut String) -> Result<()> {
    let ty = &resolve.types[type_id];
    let name = ty.name.as_deref().unwrap_or("anonymous");

    match &ty.kind {
        TypeDefKind::Record(record) => {
            writeln!(out, "#### `{name}` record")?;
            writeln!(out)?;
            write_docs(&ty.docs, out)?;
            for field in &record.fields {
                write_docs(&field.docs, out)?;
                writeln!(
                    out,
                    "- `{}`: `{}`",
                    field.name,
                    render_type(resolve, &field.ty)
                )?;
            }
            writeln!(out)?;
        }
        TypeDefKind::Enum(enum_) => {
            writeln!(out, "#### `{name}` enum")?;
            writeln!(out)?;
            write_docs(&ty.docs, out)?;
            for case in &enum_.cases {
                write_docs(&case.docs, out)?;
                writeln!(out, "- `{}`", case.name)?;
            }
            writeln!(out)?;
        }
        TypeDefKind::Variant(variant) => {
            writeln!(out, "#### `{name}` variant")?;
            writeln!(out)?;
            write_docs(&ty.docs, out)?;
            for case in &variant.cases {
                write_docs(&case.docs, out)?;
                if let Some(ty) = &case.ty {
                    writeln!(out, "- `{}`: `{}`", case.name, render_type(resolve, ty))?;
                } else {
                    writeln!(out, "- `{}`", case.name)?;
                }
            }
            writeln!(out)?;
        }
        TypeDefKind::Resource => {
            writeln!(out, "#### `{name}` resource")?;
            writeln!(out)?;
            write_docs(&ty.docs, out)?;
        }
        TypeDefKind::Type(inner) => {
            writeln!(out, "#### `{name}` type")?;
            writeln!(out)?;
            write_docs(&ty.docs, out)?;
            writeln!(out, "`{}`", render_type(resolve, inner))?;
            writeln!(out)?;
        }
        _ => {
            writeln!(out, "#### `{name}` {}", ty.kind.as_str())?;
            writeln!(out)?;
            write_docs(&ty.docs, out)?;
        }
    }

    Ok(())
}

fn render_function_entry(
    resolve: &Resolve,
    interface_name: &str,
    function: &Function,
    out: &mut String,
) -> Result<()> {
    write_docs(&function.docs, out)?;
    writeln!(out, "- `{}`", render_function(resolve, function))?;

    let function_name = function_note_key(resolve, function);
    for note in function_notes(interface_name, &function_name) {
        writeln!(out, "  - {note}")?;
    }

    Ok(())
}

fn function_note_key(resolve: &Resolve, function: &Function) -> String {
    let name = render_function_name(resolve, function);
    if let Some(resource_id) = function.kind.resource() {
        let resource_name = resolve.types[resource_id]
            .name
            .as_deref()
            .unwrap_or("resource");
        format!("{resource_name}.{name}")
    } else {
        name
    }
}

fn render_function(resolve: &Resolve, function: &Function) -> String {
    let mut rendered = String::new();
    rendered.push_str(&render_function_name(resolve, function));
    rendered.push('(');

    let params = function
        .params
        .iter()
        .filter(|(name, _)| !(function.kind.resource().is_some() && name == "self"))
        .collect::<Vec<_>>();

    for (index, (name, ty)) in params.iter().enumerate() {
        if index > 0 {
            rendered.push_str(", ");
        }
        rendered.push_str(name);
        rendered.push_str(": ");
        rendered.push_str(&render_type(resolve, ty));
    }

    rendered.push(')');

    if let Some(result) = &function.result {
        rendered.push_str(" -> ");
        rendered.push_str(&render_type(resolve, result));
    }

    if matches!(
        function.kind,
        FunctionKind::Constructor(_) | FunctionKind::Static(_) | FunctionKind::AsyncStatic(_)
    ) {
        rendered.push_str(" [resource]");
    }

    rendered
}

fn render_function_name(resolve: &Resolve, function: &Function) -> String {
    if let Some(resource_id) = function.kind.resource() {
        let resource_name = resolve.types[resource_id]
            .name
            .as_deref()
            .unwrap_or("resource");
        for prefix in [
            format!("[method]{resource_name}."),
            format!("[static]{resource_name}."),
        ] {
            if let Some(name) = function.name.strip_prefix(&prefix) {
                return name.to_string();
            }
        }
        if let Some(name) = function.name.strip_prefix("[constructor]") {
            return name.to_string();
        }
    }

    function.name.clone()
}

fn is_renderable_type_def(resolve: &Resolve, type_id: TypeId) -> bool {
    let ty = &resolve.types[type_id];
    match &ty.kind {
        TypeDefKind::Type(inner) => {
            let Some(name) = &ty.name else {
                return false;
            };
            render_type(resolve, inner) != *name
        }
        _ => true,
    }
}

fn write_docs(docs: &wit_parser::Docs, out: &mut String) -> Result<()> {
    let Some(contents) = docs.contents.as_deref() else {
        return Ok(());
    };

    let trimmed = contents.trim();
    if trimmed.is_empty() {
        return Ok(());
    }

    for line in trimmed.lines() {
        writeln!(out, "> {}", line.trim())?;
    }
    writeln!(out)?;

    Ok(())
}

fn interface_summary(interface: &str) -> Option<&'static str> {
    match interface {
        "krate:fs/files@0.1.0" => Some(
            "Filesystem entry points. All host file access should pass through these functions and resource methods.",
        ),
        "krate:fs/types@0.1.0" => {
            Some("Shared filesystem records, modes, and error shapes.")
        }
        "krate:io/args@0.1.0" => Some(
            "Raw Krate app arguments. These are the arguments passed after `--` in `krate run`.",
        ),
        "krate:io/log@0.1.0" => Some(
            "Structured app logs. Hosts can route these to native logs, developer consoles, or test captures.",
        ),
        "krate:io/stdio@0.1.0" => {
            Some("Standard input, output, and error streams for CLI-style apps.")
        }
        "krate:io/streams@0.1.0" => {
            Some("Byte streams used by stdio and other UAPI modules.")
        }
        "krate:io/types@0.1.0" => Some("Shared IO log and error types."),
        "krate:locale/format@0.1.0" => {
            Some("Host-backed date and number formatting.")
        }
        "krate:locale/info@0.1.0" => {
            Some("The host user's current locale and timezone.")
        }
        "krate:locale/types@0.1.0" => Some("Locale and formatting type definitions."),
        "krate:net/http-client@0.1.0" => {
            Some("HTTP client calls. Phase 2 starts with simple request and response bodies.")
        }
        "krate:net/types@0.1.0" => Some("Shared network request, response, and error types."),
        "krate:time/clock@0.1.0" => {
            Some("Wall-clock and monotonic clock reads.")
        }
        "krate:time/sleep@0.1.0" => Some("Blocking sleep for CLI-style components."),
        _ => None,
    }
}

fn capability_notes(interface: &str) -> &'static [&'static str] {
    match interface {
        "krate:fs/files@0.1.0" => &[
            "`open`, `stat`, and `list` require a matching `fs.read:PATH` grant for read-style access.",
            "Write, mkdir, remove, and rename operations are part of the Phase 2 shape, but the first runtime slice focuses on read grants.",
        ],
        "krate:io/args@0.1.0" => &[
            "`io.args` is granted by default for CLI apps.",
            "The current draft encodes args as newline-separated text.",
        ],
        "krate:io/stdio@0.1.0" | "krate:io/streams@0.1.0" => &[
            "`io.stdin`, `io.stdout`, and `io.stderr` are low-risk default grants for CLI apps.",
        ],
        "krate:io/log@0.1.0" => &["`io.log` is a low-risk default grant."],
        "krate:net/http-client@0.1.0" => &[
            "`get` and `fetch` require a matching `net.connect:HOST:PORT` grant before the adapter opens a socket.",
            "The current host adapter supports plain HTTP request framing first, with a 1 MiB full-response cap; HTTPS, redirects, streaming, and richer network behavior are still Phase 2 work.",
        ],
        "krate:time/clock@0.1.0" => {
            &["`time.clock` and `time.monotonic` are default grants."]
        }
        "krate:time/sleep@0.1.0" => &["`sleep-millis` requires `time.sleep`."],
        "krate:locale/info@0.1.0" | "krate:locale/format@0.1.0" => &[
            "Locale reads and formatting are default grants for CLI apps.",
        ],
        _ => &[],
    }
}

struct CapabilityPattern {
    capability: String,
    grant_kind: &'static str,
}

fn capability_patterns_for_interface(interface: &str) -> Vec<CapabilityPattern> {
    let Some(module) = capability_module_for_interface(interface) else {
        return Vec::new();
    };

    supported_phase2_capability_specs()
        .filter(|spec| spec.module() == module)
        .map(|spec| {
            let grant_kind = if spec.default_granted() {
                "default grant"
            } else {
                "manifest or session grant"
            };
            CapabilityPattern {
                capability: spec.display_pattern(),
                grant_kind,
            }
        })
        .collect()
}

fn capability_module_for_interface(interface: &str) -> Option<&str> {
    let (_, rest) = interface.split_once(':')?;
    let (module, interface_name) = rest.split_once('/')?;

    if interface_name.starts_with("types@") {
        return None;
    }

    match module {
        "io" | "fs" | "net" | "time" | "locale" => Some(module),
        _ => None,
    }
}

fn rust_example(interface: &str) -> Option<&'static str> {
    match interface {
        "krate:fs/files@0.1.0" => Some(
            "let text = krate::fs::read_to_string(\"notes.txt\")?;\nkrate::io::stdio::println(&text)?;\n",
        ),
        "krate:io/args@0.1.0" => Some(
            "let raw = krate::io::args::raw();\nlet first = krate::io::args::first_raw(&raw);\n",
        ),
        "krate:io/stdio@0.1.0" => Some(
            "krate::io::stdio::println(\"Hello from Krate\")?;\nkrate::io::stdio::eprintln(\"debug line\")?;\n",
        ),
        "krate:io/streams@0.1.0" => Some(
            "use krate::io::streams::OutputStreamExt;\n\nlet out = krate::io::stdio::stdout();\nout.write_line(\"ok\")?;\nout.flush()?;\n",
        ),
        "krate:net/http-client@0.1.0" => Some(
            "let body = krate::net::get_text(\"http://127.0.0.1:8080/data.txt\")?;\nkrate::io::stdio::println(&body)?;\n",
        ),
        "krate:time/clock@0.1.0" => Some(
            "let now = krate::time::now_millis();\nlet tick = krate::time::monotonic_nanos();\n",
        ),
        "krate:time/sleep@0.1.0" => Some("krate::time::sleep_millis(100);\n"),
        "krate:locale/info@0.1.0" => Some(
            "let locale = krate::locale::current();\nlet timezone = krate::locale::timezone();\n",
        ),
        "krate:locale/format@0.1.0" => Some(
            "let locale = krate::locale::current();\nlet text = krate::locale::format_number(42.0, krate::locale::NumberStyle::Decimal, &locale);\n",
        ),
        _ => None,
    }
}

fn function_notes(interface: &str, function: &str) -> &'static [&'static str] {
    match (interface, function) {
        ("krate:fs/files@0.1.0", "open") => &[
            "Opens a host file through Krate and returns a `file` handle.",
            "`read` needs `fs.read:PATH`; `write`, `append`, and `read-write` also need the matching write grant.",
        ],
        ("krate:fs/files@0.1.0", "stat") => &[
            "Reads file metadata without opening the file body.",
            "Requires `fs.read:PATH` for the path being inspected.",
        ],
        ("krate:fs/files@0.1.0", "list") => &[
            "Returns directory entry names for a granted directory.",
            "Requires `fs.list:PATH` before the adapter reads the directory.",
        ],
        ("krate:fs/files@0.1.0", "remove-file") => &[
            "Deletes one file.",
            "Requires `fs.remove:PATH`; missing grants fail before host deletion is attempted.",
        ],
        ("krate:fs/files@0.1.0", "remove-dir") => &[
            "Deletes one directory.",
            "Requires `fs.remove:PATH`; hosts can still reject non-empty directories.",
        ],
        ("krate:fs/files@0.1.0", "mkdir") => &[
            "Creates one directory.",
            "Requires `fs.mkdir:PATH` for the directory being created.",
        ],
        ("krate:fs/files@0.1.0", "rename") => &[
            "Moves or renames a file or directory.",
            "Requires grants for both sides: remove/write style access to the source and write style access to the destination.",
        ],
        ("krate:fs/files@0.1.0", "file.read") => &[
            "Reads up to `n` bytes from an opened file handle.",
            "The runtime rechecks the handle path before each adapter read.",
        ],
        ("krate:fs/files@0.1.0", "file.write") => &[
            "Writes bytes to an opened file handle and returns the number written.",
            "The runtime rechecks write permission before each adapter write.",
        ],
        ("krate:fs/files@0.1.0", "file.seek-set") => &[
            "Moves the file cursor to an absolute byte position.",
            "The handle must still be valid and backed by a granted file.",
        ],
        ("krate:fs/files@0.1.0", "file.seek-end") => &[
            "Moves the file cursor to the end and returns the new position.",
            "Useful before append-style writes or size checks.",
        ],
        ("krate:fs/files@0.1.0", "file.stat") => &[
            "Reads metadata for the opened file handle.",
            "The runtime rechecks the handle path before the adapter stat call.",
        ],
        ("krate:io/args@0.1.0", "raw") => &[
            "Returns the app arguments passed after `--` in `krate run`.",
            "Current encoding is newline-separated text, so SDK helpers should parse it for app code.",
        ],
        ("krate:io/log@0.1.0", "emit") => &[
            "Sends one structured log event to the host.",
            "Fields are plain key/value strings so native hosts can map them to their own log systems.",
        ],
        ("krate:io/stdio@0.1.0", "stdin") => &[
            "Returns an input stream connected to the host standard input.",
            "Granted by default for CLI apps.",
        ],
        ("krate:io/stdio@0.1.0", "stdout") => &[
            "Returns an output stream connected to host standard output.",
            "Use this for normal command output that other tools may read.",
        ],
        ("krate:io/stdio@0.1.0", "stderr") => &[
            "Returns an output stream connected to host standard error.",
            "Use this for diagnostics and permission errors.",
        ],
        ("krate:io/streams@0.1.0", "input-stream.read") => &[
            "Reads up to `n` bytes from an input stream.",
            "A short read is valid; an empty read means the stream has no more bytes right now or is closed.",
        ],
        ("krate:io/streams@0.1.0", "input-stream.read-to-string") => &[
            "Reads the stream as UTF-8 text.",
            "Invalid UTF-8 returns `io-error.invalid-utf8` instead of lossy text.",
        ],
        ("krate:io/streams@0.1.0", "output-stream.write") => &[
            "Writes bytes to an output stream and returns the number accepted.",
            "Apps that need all bytes written should use `write-all` or an SDK helper.",
        ],
        ("krate:io/streams@0.1.0", "output-stream.write-all") => &[
            "Writes the full byte buffer or returns an IO error.",
            "This is the right primitive for line-oriented CLI output.",
        ],
        ("krate:io/streams@0.1.0", "output-stream.flush") => &[
            "Asks the host to push buffered output through.",
            "Use it before exiting after important diagnostics or prompts.",
        ],
        ("krate:net/http-client@0.1.0", "get") => &[
            "Performs a simple HTTP GET and returns only the response body.",
            "Requires `net.connect:HOST:PORT`; Phase 2 currently supports the plain HTTP adapter path.",
        ],
        ("krate:net/http-client@0.1.0", "fetch") => &[
            "Performs a lower-level HTTP request and returns status, headers, and body.",
            "The plain HTTP adapter now forwards the method, app headers, and buffered body while keeping `Host`, `Connection`, and `Content-Length` under host control.",
            "Timeouts, oversized bodies, malformed responses, and missing grants are typed as `net-error` cases.",
        ],
        ("krate:time/clock@0.1.0", "now-millis") => &[
            "Reads host wall-clock time in milliseconds since Unix epoch.",
            "This value can move backward or forward if the host clock changes.",
        ],
        ("krate:time/clock@0.1.0", "monotonic-nanos") => &[
            "Reads a non-decreasing timer in nanoseconds.",
            "Use this for durations instead of wall-clock time.",
        ],
        ("krate:time/sleep@0.1.0", "sleep-millis") => &[
            "Blocks the calling component task for at least the requested milliseconds.",
            "Requires `time.sleep`; hosts may wake slightly later than requested.",
        ],
        ("krate:locale/info@0.1.0", "current") => &[
            "Returns the host user's preferred locale as a BCP 47 string.",
            "Good for display choices, not for security decisions.",
        ],
        ("krate:locale/info@0.1.0", "timezone") => &[
            "Returns the host timezone name.",
            "Expected form is an IANA name such as `Asia/Singapore` when the host can provide one.",
        ],
        ("krate:locale/format@0.1.0", "format-date") => &[
            "Formats a timestamp using a requested timezone, date style, and locale.",
            "The host owns the native formatting behavior so output can match the platform.",
        ],
        ("krate:locale/format@0.1.0", "format-number") => &[
            "Formats a number using a requested style and locale.",
            "Currency style is present in the shape, but richer currency-code handling remains future work.",
        ],
        _ => &[],
    }
}

fn render_type(resolve: &Resolve, ty: &Type) -> String {
    match ty {
        Type::Bool => "bool".to_string(),
        Type::U8 => "u8".to_string(),
        Type::U16 => "u16".to_string(),
        Type::U32 => "u32".to_string(),
        Type::U64 => "u64".to_string(),
        Type::S8 => "s8".to_string(),
        Type::S16 => "s16".to_string(),
        Type::S32 => "s32".to_string(),
        Type::S64 => "s64".to_string(),
        Type::F32 => "f32".to_string(),
        Type::F64 => "f64".to_string(),
        Type::Char => "char".to_string(),
        Type::String => "string".to_string(),
        Type::ErrorContext => "error-context".to_string(),
        Type::Id(id) => render_type_id(resolve, *id),
    }
}

fn render_type_id(resolve: &Resolve, id: TypeId) -> String {
    let ty = &resolve.types[id];
    if let Some(name) = &ty.name {
        return name.clone();
    }

    match &ty.kind {
        TypeDefKind::List(inner) => format!("list<{}>", render_type(resolve, inner)),
        TypeDefKind::Option(inner) => format!("option<{}>", render_type(resolve, inner)),
        TypeDefKind::Result(result) => {
            let ok = result
                .ok
                .as_ref()
                .map(|ty| render_type(resolve, ty))
                .unwrap_or_else(|| "_".to_string());
            let err = result
                .err
                .as_ref()
                .map(|ty| render_type(resolve, ty))
                .unwrap_or_else(|| "_".to_string());
            format!("result<{ok}, {err}>")
        }
        TypeDefKind::Tuple(tuple) => {
            let items = tuple
                .types
                .iter()
                .map(|ty| render_type(resolve, ty))
                .collect::<Vec<_>>()
                .join(", ");
            format!("tuple<{items}>")
        }
        TypeDefKind::Handle(handle) => match handle {
            wit_parser::Handle::Own(id) => format!("own<{}>", render_type_id(resolve, *id)),
            wit_parser::Handle::Borrow(id) => format!("borrow<{}>", render_type_id(resolve, *id)),
        },
        other => other.as_str().to_string(),
    }
}

fn interface_name(resolve: &Resolve, interface: &Interface) -> String {
    match (interface.package, interface.name.as_deref()) {
        (Some(package_id), Some(name)) => resolve.packages[package_id].name.interface_id(name),
        (_, Some(name)) => name.to_string(),
        _ => "anonymous-interface".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_reference_includes_human_context() {
        let root = workspace_root();
        let mut resolve = Resolve::default();
        let (app_package, _) = resolve
            .push_dir(root.join("wit/krate/phase2"))
            .expect("parse Phase 2 WIT");
        let world_id = resolve
            .select_world(&[app_package], Some("cli"))
            .expect("select cli world");

        let reference = render_reference(&resolve, world_id).expect("render reference");

        assert!(reference.contains("### Capability Notes"));
        assert!(reference.contains("### Rust SDK Example"));
        assert!(reference.contains("`net.connect:<host>:<port>`"));
        assert!(reference.contains("generated from the runtime manifest table"));
        assert!(reference.contains("let text = krate::fs::read_to_string"));
        assert!(reference.contains("Opens a host file through Krate"));
        assert!(reference.contains("Timeouts, oversized bodies, malformed responses"));
        assert!(reference.contains("> Milliseconds since Unix epoch."));
    }
}

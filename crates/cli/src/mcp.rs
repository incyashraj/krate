//! The Krate MCP server: authoring plus sandboxed execution, over stdio.
//!
//! Two families of tool, deliberately in one server, because a model that just
//! built an app should be able to run it without the user wiring up a second
//! connector:
//!
//!  - **Authoring** (`krate_schema`, `krate_examples`, `krate_start_build`,
//!    `krate_build_status`, `krate_check`, `krate_package`, `krate_run`), from
//!    the `krate-mcp` crate. These wrap the authoring loop -- the context pack,
//!    `krate create`, and the `check-app` oracle -- so someone can get a
//!    working `.krate` by talking.
//!  - **Execution** (`run_component`, `inspect_bundle`), below. An agent
//!    framework supplies a component and grants; Krate runs it inside the
//!    capability sandbox and returns a `krate.run.v1` report.
//!
//! This file owns only the execution half and the composition. The protocol
//! itself lives in `krate_mcp::protocol`, tested on its own.
//!
//! Transport: newline-delimited JSON-RPC 2.0 over stdio. See
//! `docs/mcp-setup.md` for the client configuration.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use krate_manifest::Manifest;
use krate_mcp::{KrateTools, ToolSet};
use krate_policy::SessionPolicy;
use krate_runtime::{embed, Config};
use serde_json::{json, Value};

use crate::audit::RecordDoor;
use crate::authoring_context;

pub fn serve() -> Result<()> {
    // The binary that runs builds and checks is this one. Resolving it here,
    // rather than trusting a bare `krate` on PATH, means the server always
    // drives the Krate it is part of -- an installed older release on PATH was
    // exactly how a "fixed" bug appeared to come back.
    let krate_bin = std::env::current_exe().context("locate the krate binary")?;
    let authoring = KrateTools::new(
        krate_bin.clone(),
        krate_mcp::mcp_root()?,
        authoring_context::generate,
    );
    let tools = KrateServer {
        authoring,
        krate_bin,
    };

    // Say what this is when a person runs it by hand.
    //
    // An MCP server reads JSON-RPC from stdin and writes it to stdout, so with
    // nothing connected it sits there producing nothing. That looks exactly
    // like a hang, and the first person to try it reasonably concluded it was
    // broken. When stdin is a terminal there is no client, so print the
    // explanation; when it is a pipe -- which is every real client -- print
    // nothing, because stray output on the wrong stream corrupts the protocol.
    // This goes to stderr regardless, which clients ignore.
    if std::io::stdin().is_terminal() {
        // Run by hand from a terminal, this is a dead end: it speaks JSON-RPC,
        // so anything typed here comes back as a parse error. A tester did
        // exactly that -- read the suggested command below, typed it into this
        // prompt, and got three parse errors in a row. So stop before serving
        // rather than sitting there looking like it wants input.
        eprintln!("This is not a command to run by hand.");
        eprintln!();
        eprintln!("`krate mcp` is the background server that Claude Desktop or Cursor");
        eprintln!("starts for you. It speaks JSON-RPC, so anything you type here comes");
        eprintln!("back as a parse error.");
        eprintln!();
        eprintln!("What you probably want:");
        eprintln!();
        eprintln!("  krate            make an app, open one, or publish one");
        eprintln!("  krate connect    set up Claude Desktop or Cursor to use Krate");
        eprintln!();
        return Ok(());
    }

    krate_mcp::serve_with(&tools, std::io::stdin().lock(), std::io::stdout().lock())
}

/// The whole server: the authoring tools plus the execution tools.
struct KrateServer {
    authoring: KrateTools,
    /// This binary, which `krate_open` and `krate_install` run as
    /// `krate launch` / `krate install` so they open an app exactly the way
    /// a double-click does.
    krate_bin: PathBuf,
}

impl ToolSet for KrateServer {
    fn server_name(&self) -> &str {
        "krate"
    }

    fn server_version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn instructions(&self) -> Option<String> {
        // The authoring instructions plus one line on the execution half, so a
        // model knows both halves exist before it starts.
        self.authoring.instructions().map(|text| {
            format!(
                "{text}\n\nWhen the app passes, `krate_package` with its `dir` makes the .krate, \
                 and `krate_open` opens it in a real window on this computer so the person sees \
                 it straight away; `krate_install` keeps it in their apps when they ask.\n\n\
                 This server can also run components directly: `inspect_bundle` shows \
                 what a .krate asks for without executing it, and `run_component` runs one with \
                 exactly the permissions you name and nothing else."
            )
        })
    }

    fn tools(&self) -> Vec<Value> {
        let mut tools = self.authoring.tools();
        tools.extend(execution_tools());
        tools
    }

    fn call(&self, name: &str, arguments: &Value) -> std::result::Result<Value, String> {
        match name {
            // A run that ends badly is still a successful *call*: the report is
            // the answer, and it carries the exit class and the remedy the
            // model needs. Only a call that could not produce a report at all
            // (a missing file, a bad URL) is an error.
            "run_component" => run_component_tool(arguments).map_err(|err| format!("{err:#}")),
            "inspect_bundle" => inspect_bundle_tool(arguments).map_err(|err| format!("{err:#}")),
            "krate_open" => open_tool(&self.krate_bin, arguments).map_err(|err| format!("{err:#}")),
            "krate_install" => {
                install_tool(&self.krate_bin, arguments).map_err(|err| format!("{err:#}"))
            }
            other => self.authoring.call(other, arguments),
        }
    }
}

fn execution_tools() -> Vec<Value> {
    vec![
        json!({
            "name": "krate_open",
            "title": "Open an app on this computer",
            "description": "Open a .krate in its own window on this computer, with its own name \
                and icon, exactly as double-clicking it does. Pass the `path` krate_package \
                returned, any .krate on this computer, or a krate.tech / hub app link. The first \
                time an app opens, Krate shows its own permission card in that window and the \
                person chooses there -- you cannot grant anything for them, so tell them to look \
                at the window. Use this after packaging so the person sees their app right away.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "app": {
                        "type": "string",
                        "description": "A path to a .krate, or an https link to one (a krate.tech or hub app link)."
                    }
                },
                "required": ["app"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "krate_install",
            "title": "Keep an app on this computer",
            "description": "Install a .krate so it stays on this computer like any other app: in \
                Applications and Launchpad on a Mac, in the Start menu on Windows, in the app \
                menu on Linux. The .krate file keeps working as the thing to send someone. Only \
                when the person asks to keep or install the app; opening it does not need this.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The .krate file on this computer, e.g. the path krate_package returned."
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "run_component",
            "description": "Run a Krate WebAssembly component inside the capability sandbox. \
                Grants are explicit: nothing is prompted, and the component cannot touch \
                files, network, or anything else that was not granted. Returns a \
                krate.run.v1 report with the exit classification, effective grants, \
                denied capabilities, duration, and captured stdout.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "bundle": {
                        "type": "string",
                        "description": "Path to a .krate bundle, or an https URL to one. A bundle carries its own manifest, so component_path and manifest_path are not needed with it. Fetching grants nothing: a downloaded app has the same authority a local one has, which is none until granted."
                    },
                    "insecure_http": {
                        "type": "boolean",
                        "description": "Allow fetching a bundle over plain http. Only for local test servers; https is required otherwise."
                    },
                    "component_path": {
                        "type": "string",
                        "description": "Path to the .wasm component file. Use this or `bundle`."
                    },
                    "manifest_path": {
                        "type": "string",
                        "description": "Path to the app's manifest.toml. Used for denial reporting."
                    },
                    "grants": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Capability strings to grant, e.g. \"fs.read:data/**\"."
                    },
                    "app_args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Arguments passed to the component."
                    },
                    "sandbox_root": {
                        "type": "string",
                        "description": "Directory that relative filesystem grants resolve against. Defaults to the manifest's directory, else the component's."
                    }
                },
                "required": []
            }
        }),
        json!({
            "name": "inspect_bundle",
            "description": "Read a .krate bundle without running it: its identities, its container \
                profile, every record with the class the format gives it, the extensions it \
                declares, its signature and the capabilities it requests. Use this before \
                run_component to decide whether an app should be executed at all, and which of \
                its requests to grant. Nothing is executed and nothing is granted.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "bundle": {
                        "type": "string",
                        "description": "Path to a .krate bundle, or an https URL to one."
                    },
                    "insecure_http": {
                        "type": "boolean",
                        "description": "Allow plain http. Only for local test servers."
                    }
                },
                "required": ["bundle"]
            }
        }),
    ]
}

/// How long opening may take: a link is downloaded first.
const OPEN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// `krate_open`: the app in a real window, through `krate launch`.
///
/// `krate launch` is the double-click path: on a Mac it wraps the app with
/// its own name and icon and opens it through LaunchServices (a bare child
/// process gets no window, K-110); elsewhere it starts a detached windowed
/// run. Both show Krate's permission card on first open. Nothing here
/// grants anything: the person answers in the app's window (IC-341).
fn open_tool(krate_bin: &Path, arguments: &Value) -> Result<Value> {
    let target = arguments
        .get("app")
        .or_else(|| arguments.get("path"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .context(
            "krate_open needs `app`: the .krate path krate_package returned, or an app link",
        )?;
    let name = local_app_name(target)?;
    run_door(krate_bin, &["launch", target], OPEN_TIMEOUT)?;
    let shown = name.as_deref().unwrap_or("The app");
    Ok(json!({
        "opened": true,
        "app": target,
        "name": name,
        "next": format!(
            "{shown} is open in its own window on this computer. The first time, Krate asks \
             there what it may use; the person chooses in that window, not here. Tell them it \
             is open, and that the .krate file is what they send to someone else."
        ),
    }))
}

/// `krate_install`: the app kept on this computer, through `krate install`.
fn install_tool(krate_bin: &Path, arguments: &Value) -> Result<Value> {
    let target = arguments
        .get("path")
        .or_else(|| arguments.get("app"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .context("krate_install needs `path`: a .krate file on this computer")?;
    if krate_bundle::is_url(target) {
        anyhow::bail!(
            "krate_install takes a .krate file on this computer. Open the link with krate_open \
             first, or package the app and pass its path."
        );
    }
    let name = local_app_name(target)?;
    let said = run_door(krate_bin, &["install", target], OPEN_TIMEOUT)?;
    Ok(json!({
        "installed": true,
        "name": name,
        "said": said.trim(),
        "next": "It is installed: the person can open it from Launchpad / Applications on a Mac \
                 or the Start menu on Windows, like any other app.",
    }))
}

/// The app's name for a local .krate, and a plain refusal for a path that
/// is not one. Links are named by the app once it opens.
fn local_app_name(target: &str) -> Result<Option<String>> {
    if krate_bundle::is_url(target) {
        return Ok(None);
    }
    let path = Path::new(target);
    if !path.exists() {
        anyhow::bail!("no file at {target}");
    }
    if path.is_dir() {
        anyhow::bail!(
            "{target} is a folder. Package it with krate_package (pass `dir`) and open the .krate it returns."
        );
    }
    let opened = krate_bundle::open(path)
        .with_context(|| format!("{target} is not a .krate Krate can open"))?;
    Ok(Some(opened.manifest().app.name.clone()))
}

/// Run one of this binary's own doors (`launch`, `install`) and return what
/// it said, or what went wrong in its own words.
fn run_door(krate_bin: &Path, args: &[&str], timeout: std::time::Duration) -> Result<String> {
    use std::process::Stdio;
    let mut child = std::process::Command::new(krate_bin)
        .args(args)
        // The MCP client owns this process's stdin and stdout; a child must
        // never read the one or write into the other.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("could not run {}", krate_bin.display()))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let read = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_string(&mut text);
            }
            text
        })
    };
    let out = read(stdout.map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    let err = read(stderr.map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!(
                "krate {} did not finish within {} seconds",
                args[0],
                timeout.as_secs()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let said = out.join().unwrap_or_default();
    let complaint = err.join().unwrap_or_default();
    if !status.success() {
        let words = if complaint.trim().is_empty() {
            said.trim()
        } else {
            complaint.trim()
        };
        anyhow::bail!("{}", words.trim_start_matches("Error: "));
    }
    Ok(said)
}

/// Report what a bundle is and what it wants, without running it.
///
/// This exists so an agent can decide *before* execution. Reading a bundle
/// executes no code and grants nothing.
fn inspect_bundle_tool(arguments: &Value) -> Result<Value> {
    let target = arguments
        .get("bundle")
        .and_then(Value::as_str)
        .context("inspect_bundle needs bundle")?;
    let allow_http = arguments
        .get("insecure_http")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    inspect_json(target, allow_http)
}

/// What a bundle is, without running it: its app, its identities, its
/// container profile, every record with the class the profile gives it,
/// the extensions it declares, its signature and its capability requests.
///
/// One builder for the MCP tool and `krate inspect`, over the one opener
/// every door uses, so an agent, a person at a terminal and the runtime
/// resolve the same record set (IC-714, test 1477). Reading a bundle
/// executes no code and grants nothing.
pub(crate) fn inspect_json(target: &str, allow_http: bool) -> Result<Value> {
    let (bundle, archive_bytes) = if krate_bundle::is_url(target) {
        (fetch_with_session(target, allow_http)?.bundle, None)
    } else {
        let path = Path::new(target);
        // Plain words for the two things a person actually types wrong.
        //
        // This was `read {target}`, so the OS error came straight through:
        // "read /does/not/exist.krate: No such file or directory (os error
        // 2)" and "read /tmp: Is a directory (os error 21)". A stranger who
        // mistyped a path was shown an errno. The damaged-file case a few
        // lines down already says the right kind of thing; these did not.
        if !path.exists() {
            anyhow::bail!("no file at {target}");
        }
        if path.is_dir() {
            anyhow::bail!("{target} is a folder. Point at the .krate file inside it.");
        }
        let bytes = std::fs::read(path).with_context(|| format!("could not read {target}"))?;
        (
            krate_bundle::open(path).with_context(|| format!("open {target}"))?,
            Some(bytes),
        )
    };

    let manifest = bundle.manifest();
    let requests: Vec<Value> = manifest
        .capabilities
        .iter()
        .map(|request| {
            json!({
                "capability": request.cap,
                "rationale": request.rationale,
                "required": request.required,
            })
        })
        .collect();

    // The three identities, as `run --json` reports them: the file's own
    // bytes, what runs, and what could be rebuilt (absent unless there is
    // genuinely more to rebuild than to run).
    let execution = bundle.digest().ok();
    let project = bundle.project_digest().ok().and_then(|project| {
        let execution = execution.as_ref()?;
        (project.entries.len() > execution.entries.len()).then_some(project.digest)
    });
    let archive = archive_bytes
        .as_deref()
        .map(|bytes| krate_bundle::provenance::digest_archive_bytes(bytes).digest);
    let signature = match bundle.signature_state() {
        Ok(krate_bundle::SignatureState::Absent) => "absent",
        Ok(krate_bundle::SignatureState::Damaged { .. }) => "damaged",
        Ok(krate_bundle::SignatureState::Signed(_)) => "present",
        Err(_) => "unreadable",
    };

    Ok(json!({
        "schema": "krate.inspect.v1",
        "source": target,
        "app": {
            "id": manifest.app.id,
            "name": manifest.app.name,
            "version": manifest.app.version,
            "world": manifest.app.world,
        },
        "profile": bundle.profile(),
        "identity": {
            "archive": archive,
            "execution": execution.map(|d| d.digest),
            "project": project,
        },
        "signature": signature,
        "release": bundle.release().ok().flatten(),
        "derived_from": bundle.derived_from().ok().flatten(),
        "records": bundle.records(),
        "extensions": bundle.extensions(),
        "closure": bundle.closure(),
        "provenance": bundle.build_provenance().ok().flatten(),
        "requests": requests,
        "note": "Nothing was executed and nothing was granted. Pass the capabilities \
                you decide to allow to run_component in `grants`.",
    }))
}

/// Fetch a link the way `krate run` does (IC-948 H): with the person's
/// session when the link is on a Krate hub, and a private app's refusal in
/// the same words, never "403".
fn fetch_with_session(target: &str, allow_http: bool) -> Result<krate_bundle::Fetched> {
    // A Player address copied from a browser names the same app (K-1166).
    let target = &*crate::org::bundle_address(target);
    let authorization = crate::org::link_authorization(target);
    krate_bundle::fetch_resolved_authorized(target, allow_http, authorization.as_deref()).map_err(
        |err| match crate::access::refused_words(&err, target) {
            Some(words) => anyhow::anyhow!("{words}"),
            None => anyhow::Error::new(err).context(format!("fetch {target}")),
        },
    )
}

/// Execute the component and build the krate.run.v1 report.
fn run_component_tool(arguments: &Value) -> Result<Value> {
    // `bundle` is the shareable form: one file, or a URL to one, carrying the
    // component and the permissions it asks for. It is resolved into the same
    // component + manifest pair the sidecar path uses, so everything below is
    // identical either way, and a fetched bundle gets no authority for having
    // been fetched.
    let bundle_target = arguments.get("bundle").and_then(Value::as_str);
    let mut fetched_sha: Option<String> = None;
    // The private app the organisation gate let through, for its team data
    // and company connections, as `krate run` gives them (K-1102).
    let mut org_opened: Option<crate::access::Opened> = None;
    let opened = match bundle_target {
        Some(target) if krate_bundle::is_url(target) => {
            let allow_http = arguments
                .get("insecure_http")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let fetched = fetch_with_session(target, allow_http)?;
            let subject =
                crate::fleet::Subject::new(Some(&fetched.bundle), None, Some(&fetched.sha256));
            // The machine policy first, as at every door (IC-949 P4).
            crate::fleet::admit(&subject, RecordDoor::Mcp)?;
            // The same check every other door makes before an app runs
            // (IC-948 G): an organisation's private app runs only for a
            // current member, and offline only on a lease that holds.
            org_opened = crate::fleet::org_gate_opened(
                &subject,
                crate::access::Bytes::Fetched {
                    sha256: &fetched.sha256,
                    link: target,
                },
                crate::access::mark_of(&fetched.bundle),
                RecordDoor::Mcp,
            )?;
            fetched_sha = Some(fetched.sha256.clone());
            Some(fetched.bundle)
        }
        Some(target) => {
            let bundle =
                krate_bundle::open(Path::new(target)).with_context(|| format!("open {target}"))?;
            let subject = crate::fleet::Subject::new(Some(&bundle), Some(Path::new(target)), None);
            crate::fleet::admit(&subject, RecordDoor::Mcp)?;
            org_opened = crate::fleet::org_gate_opened(
                &subject,
                crate::access::Bytes::File(Path::new(target)),
                crate::access::mark_of(&bundle),
                RecordDoor::Mcp,
            )?;
            Some(bundle)
        }
        None => None,
    };

    let (component_path, manifest_path) = match &opened {
        Some(bundle) => (
            bundle
                .component_path()
                .to_str()
                .context("bundle path is not utf8")?,
            bundle.manifest_path().to_str(),
        ),
        None => (
            arguments
                .get("component_path")
                .and_then(Value::as_str)
                .context("run_component needs component_path or bundle")?,
            arguments.get("manifest_path").and_then(Value::as_str),
        ),
    };
    // A model cannot grant an application real-world authority (IC-341).
    //
    // This tool used to take `auto_grant`, so the thing being asked to write
    // the code also decided what the finished app was allowed to do:
    // microphone, camera, network, the person's files. Those are two
    // different permissions -- "edit this project" and "let this app reach
    // the world" -- and joining them meant nobody with standing to consent
    // was ever asked.
    //
    // Removed from the schema, and refused here rather than ignored: a
    // caller that still sends it is working from an older idea of this tool,
    // and silently doing something narrower than it asked for is how a
    // security boundary becomes a surprise.
    if arguments.get("auto_grant").is_some() {
        anyhow::bail!(
            "auto_grant is not available: an AI may ask for a capability but cannot approve one. Pass explicit `grants`, or run the app yourself and answer the permission screen."
        );
    }
    let grants: Vec<String> = string_list(arguments.get("grants"))?;
    let app_args: Vec<String> = string_list(arguments.get("app_args"))?;

    let manifest = match manifest_path {
        Some(path) => {
            let source =
                std::fs::read_to_string(path).with_context(|| format!("read manifest {path}"))?;
            Some(Manifest::parse(&source).context("parse manifest")?)
        }
        None => None,
    };

    // Only what the caller named explicitly. The branch that folded in every
    // declared capability is gone with the argument that reached it.
    //
    // And only what the app DECLARED (K-387). Refusing `auto_grant` above
    // stops an agent approving a capability; without this it could still
    // name one the manifest never asked for and have it honoured at the
    // dispatch gate, which reaches the same place by a longer road. The
    // manifest is what the person running the app was shown, so a
    // capability outside it was shown to nobody.
    //
    // Where no manifest is available -- a bare component with no
    // manifest_path -- there is no declaration to check against, and the
    // older behaviour stands rather than refusing everything.
    let policy = match &manifest {
        Some(manifest) => {
            let declared: std::collections::BTreeSet<_> = manifest
                .declared_capabilities()
                .context("read declared capabilities")?
                .into_iter()
                .collect();
            SessionPolicy::from_cli_grants_declared(&grants, &declared).context("parse grants")?
        }
        None => SessionPolicy::from_cli_grants(&grants).context("parse grants")?,
    };
    // The organisation's ceiling on whatever was granted (IC-949 P4).
    let rules_app = crate::fleet::Subject::new(opened.as_ref(), None, None)
        .with_manifest(manifest.as_ref())
        .app_ref();
    let policy = crate::fleet::narrow(policy, &rules_app);

    // Who this is, for the policy and the open record. A loose component
    // is unsigned, so a computer that allows only signed apps refuses it
    // here, as it would a bundle.
    let subject = crate::fleet::Subject::new(
        opened.as_ref(),
        Some(Path::new(bundle_target.unwrap_or(component_path)))
            .filter(|_| !bundle_target.is_some_and(krate_bundle::is_url)),
        fetched_sha.as_deref(),
    )
    .with_manifest(manifest.as_ref());
    if opened.is_none() {
        crate::fleet::admit(&subject, RecordDoor::Mcp)?;
    }
    crate::fleet::admit_capabilities(&subject, RecordDoor::Mcp)?;

    let app = manifest.as_ref().map(|manifest| {
        json!({
            "id": manifest.app.id,
            "name": manifest.app.name,
            "version": manifest.app.version,
            "world": manifest.app.world,
        })
    });
    let granted: Vec<String> = policy.grants().iter().map(|cap| cap.to_string()).collect();

    // Refuse before running when required capabilities are missing, exactly
    // like the CLI, so agents see the denial as data instead of a trap.
    if let Some(manifest) = &manifest {
        let missing = policy
            .missing_required_with_reasons(manifest)
            .context("check required capabilities")?;
        if !missing.is_empty() {
            crate::fleet::note_refused(
                &subject,
                RecordDoor::Mcp,
                "permission-denied",
                "missing required capabilities",
            );
            let denied: Vec<String> = missing.iter().map(|(cap, _)| cap.to_string()).collect();
            // Why each one is missing, beside what is missing (CP2). Every
            // refusal here is a plain denial today -- this list is built from
            // the manifest's own required set, so the capability was
            // declared, and nothing in this path revokes or expires a grant.
            // Reporting the reason anyway is what stops a later caller from
            // reading "denied" into a refusal that is not one.
            let reasons: Vec<Value> = missing
                .iter()
                .map(|(cap, why)| json!({ "capability": cap.to_string(), "reason": why.as_str() }))
                .collect();
            // Only advise a retry for refusals a grant can actually fix. An
            // agent told to grant-and-retry an unavailable or expired
            // capability retries forever.
            let retryable: Vec<String> = missing
                .iter()
                .filter(|(_, why)| *why == krate_policy::Refusal::Denied)
                .map(|(cap, _)| cap.to_string())
                .collect();
            let remedy = if retryable.is_empty() {
                json!({
                    "action": "no-retry",
                    "grants": [],
                    "note": "None of these are fixed by granting them again. \
                            See `capabilities.reasons` for why each one was refused.",
                })
            } else {
                json!({
                    "action": "grant-and-retry",
                    "grants": retryable,
                    "note": "Call run_component again with these strings in `grants`. \
                            Each one is narrow: granting it allows only what it names.",
                })
            };
            return Ok(json!({
                "schema": "krate.run.v1",
                "app": app,
                "capabilities": {
                    "granted": granted,
                    "denied": denied.clone(),
                    "reasons": reasons,
                },
                "exit": {
                    "code": 5,
                    "class": "permission-denied",
                    "message": "missing required capabilities",
                },
                // A refusal an agent cannot act on is just a failure. Name the
                // exact retry so the model does not have to infer it.
                "remedy": remedy,
                "duration_ms": Value::Null,
                "stdout": "",
            }));
        }
    }

    let sandbox_root = arguments
        .get("sandbox_root")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| {
            manifest_path
                .map(Path::new)
                .or(Some(Path::new(component_path)))
                .and_then(|path| path.parent())
                .map(Path::to_path_buf)
        })
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("."));

    let component = std::fs::read(component_path)
        .with_context(|| format!("read component {component_path}"))?;

    // The organisation's team data and company connections, through the
    // same builders `krate run` uses (K-1102), and this computer's share
    // for the organisation beside the app, as at every other door (IC-958).
    let app_org = crate::org_run_access(org_opened.as_ref(), manifest.as_ref())?;
    let connections = crate::run_connections(manifest.as_ref(), &policy, opened.as_ref(), || {
        Some(match (&fetched_sha, bundle_target) {
            (Some(sha256), _) => sha256.clone(),
            (None, Some(target)) => krate_bundle::sha256_hex(&std::fs::read(target).ok()?),
            (None, None) => return None,
        })
    });
    if let Some(org) = org_opened.as_ref() {
        crate::org::in_background(
            org.hub.clone(),
            org.slug.clone(),
            org.authorization.clone(),
            org.principal.clone(),
        );
    }

    crate::fleet::opened(&subject, RecordDoor::Mcp, &policy);
    let config = Config {
        session_policy: policy,
        interpreter: crate::fleet::interpreter(),
        app_args,
        sandbox_root,
        app_org: Some(app_org),
        connections,
        declared_capabilities: crate::declared_set(manifest.as_ref()),
        ..Config::default()
    };

    let report = match embed::run_component(&component, &config) {
        Ok(outcome) => json!({
            "schema": "krate.run.v1",
            "app": app,
            "capabilities": { "granted": granted, "denied": [] },
            "exit": {
                "code": outcome.exit_code(),
                "class": outcome.exit_class().as_str(),
                "message": Value::Null,
            },
            "duration_ms": outcome.duration().as_millis() as u64,
            "stdout": outcome.stdout_lossy(),
        }),
        Err(err) => {
            let class = match &err {
                krate_runtime::RuntimeError::InvalidComponent(_) => "invalid-component",
                krate_runtime::RuntimeError::NoMatchingWorld(_) => "no-matching-world",
                krate_runtime::RuntimeError::Trap(_) => "trap",
                _ => "runtime-error",
            };
            json!({
                "schema": "krate.run.v1",
                "app": app,
                "capabilities": { "granted": granted, "denied": [] },
                "exit": {
                    "code": Value::Null,
                    "class": class,
                    "message": err.to_string(),
                },
                "duration_ms": Value::Null,
                "stdout": "",
            })
        }
    };

    Ok(report)
}

fn string_list(value: Option<&Value>) -> Result<Vec<String>> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .context("expected a string list")
            })
            .collect(),
        Some(_) => anyhow::bail!("expected a string list"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The composed server, pointed at a scratch root so nothing touches the
    /// user's real Krate directory.
    fn server(root: &Path) -> KrateServer {
        KrateServer {
            authoring: KrateTools::new(
                root.join("krate"),
                root.to_path_buf(),
                authoring_context::generate,
            ),
            krate_bin: root.join("krate"),
        }
    }

    fn call(server: &KrateServer, line: &str) -> Value {
        krate_mcp::protocol::handle_line(server, line).expect("response expected")
    }

    #[test]
    fn initialize_reports_the_protocol_version_and_the_server_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = server(dir.path());
        let init = call(
            &server,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}"#,
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "krate");
        assert_eq!(
            init["result"]["protocolVersion"],
            krate_mcp::PROTOCOL_VERSION
        );
        // The instructions are the one chance to steer a model before it
        // guesses, so they must actually arrive.
        assert!(init["result"]["instructions"]
            .as_str()
            .expect("instructions")
            .contains("krate_schema"));
    }

    #[test]
    fn both_families_of_tool_are_advertised_together() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = server(dir.path());
        let list = call(&server, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|tool| tool["name"].as_str().expect("name"))
            .collect();
        // Authoring: build an app by talking.
        assert!(names.contains(&"krate_schema"));
        assert!(names.contains(&"krate_start_build"));
        assert!(names.contains(&"krate_check"));
        // Execution: then run what you built, or inspect someone else's.
        assert!(names.contains(&"run_component"));
        assert!(names.contains(&"inspect_bundle"));
        // On this computer: open what was built, keep it (IC-1005).
        assert!(names.contains(&"krate_open"));
        assert!(names.contains(&"krate_install"));
        assert_eq!(names.len(), 11);
    }

    #[test]
    fn notifications_get_no_response() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = server(dir.path());
        assert!(krate_mcp::protocol::handle_line(
            &server,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
        )
        .is_none());
    }

    #[test]
    fn unknown_methods_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = server(dir.path());
        let response = call(&server, r#"{"jsonrpc":"2.0","id":3,"method":"nope"}"#);
        assert_eq!(response["error"]["code"], -32601);
    }

    #[test]
    fn invalid_component_is_classified_not_crashed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = server(dir.path());
        let wasm = dir.path().join("bogus.wasm");
        std::fs::write(&wasm, b"not a component").expect("write bogus wasm");

        let request = json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "run_component",
                "arguments": { "component_path": wasm.to_string_lossy() },
            },
        });
        let response = call(&server, &request.to_string());
        // Not a crash and not a protocol error: a report the model can read.
        let report: Value = response["result"]["structuredContent"].clone();
        assert_eq!(report["schema"], "krate.run.v1");
        assert_eq!(report["exit"]["class"], "invalid-component");
    }
}

#[cfg(test)]
mod bundle_tool_tests {
    use super::*;

    const MANIFEST: &str = r#"
[app]
id = "com.example.agent"
name = "Agent Demo"
version = "0.1.0"
entry = "code.wasm"
world = "krate:app/cli@0.1.0"

[[capabilities]]
cap = "fs.read:./data/**"
rationale = "Read the input file"
required = true
"#;

    fn packed_bundle(dir: &Path) -> std::path::PathBuf {
        let manifest = dir.join("manifest.toml");
        std::fs::write(&manifest, MANIFEST).expect("write manifest");
        let component = dir.join("code.wasm");
        std::fs::write(&component, crate::MINIMAL_COMPONENT).expect("write component");
        let bundle = dir.join("demo.krate");
        krate_bundle::pack(&manifest, &component, &bundle).expect("pack");
        bundle
    }

    #[test]
    fn the_execution_tools_are_valid_mcp_tool_definitions() {
        for tool in execution_tools() {
            let name = tool["name"].as_str().expect("tool name");
            assert!([
                "run_component",
                "inspect_bundle",
                "krate_open",
                "krate_install"
            ]
            .contains(&name));
            // inputSchema MUST be a JSON Schema object, never null, or a strict
            // client rejects the whole tool list.
            assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
            assert!(tool["description"].as_str().expect("description").len() > 80);
        }
    }

    #[test]
    fn inspect_reports_requests_without_running_anything() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle = packed_bundle(dir.path());

        let report = inspect_bundle_tool(&json!({ "bundle": bundle.to_str().expect("utf8") }))
            .expect("inspect");

        assert_eq!(report["app"]["id"], "com.example.agent");
        assert_eq!(report["requests"][0]["capability"], "fs.read:./data/**");
        assert_eq!(report["requests"][0]["rationale"], "Read the input file");
        // The component here does nothing at all. Inspect still reports the
        // requests, which is the point: deciding does not require executing.
        assert_eq!(report["schema"], "krate.inspect.v1");
    }

    #[test]
    fn a_denied_run_carries_the_retry_that_would_succeed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle = packed_bundle(dir.path());

        let report =
            run_component_tool(&json!({ "bundle": bundle.to_str().expect("utf8") })).expect("run");

        assert_eq!(report["exit"]["class"], "permission-denied");
        let denied = report["capabilities"]["denied"]
            .as_array()
            .expect("denied array");
        // The remedy must name exactly what was refused, so an agent can
        // re-issue the call without inferring anything. These are all plain
        // denials -- declared, required, never granted -- so granting them
        // and retrying is the correct advice and covers the whole list.
        assert_eq!(
            &report["remedy"]["grants"],
            &report["capabilities"]["denied"]
        );
        assert_eq!(report["remedy"]["action"], "grant-and-retry");
        assert!(!denied.is_empty());

        // And each one says WHY it is missing, not only that it is (CP2).
        // Without the reason an agent cannot tell a denial it can fix by
        // granting from a refusal that no grant will ever change, and the
        // only safe behaviour left to it is to retry until it gives up.
        let reasons = report["capabilities"]["reasons"]
            .as_array()
            .expect("reasons array");
        assert_eq!(
            reasons.len(),
            denied.len(),
            "every refused capability carries its reason"
        );
        for entry in reasons {
            assert_eq!(
                entry["reason"], "denied",
                "declared, required and ungranted is a plain denial: {entry}"
            );
            assert!(
                denied.contains(&entry["capability"]),
                "a reason must name a capability that was actually refused: {entry}"
            );
        }
    }

    #[test]
    fn open_says_what_is_wrong_with_a_path_in_plain_words() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join("krate");
        let missing = dir.path().join("nope.krate");
        let err = open_tool(&bin, &json!({ "app": missing.to_str().unwrap() }))
            .expect_err("a missing file is refused");
        assert!(err.to_string().contains("no file at"), "{err}");
        let err = open_tool(&bin, &json!({ "app": dir.path().to_str().unwrap() }))
            .expect_err("a folder is refused");
        assert!(err.to_string().contains("krate_package"), "{err}");
        let err = open_tool(&bin, &json!({})).expect_err("no app named");
        assert!(err.to_string().contains("`app`"), "{err}");
        let err = install_tool(&bin, &json!({ "path": "https://krate.tech/a/x" }))
            .expect_err("install takes a file");
        assert!(err.to_string().contains("krate_open"), "{err}");
    }

    /// `krate_open` is `krate launch` -- the double-click path, with its
    /// permission card -- run by this very binary, and its refusal comes
    /// back in its own words.
    #[cfg(unix)]
    #[test]
    fn open_runs_the_launch_door_of_this_binary() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle = packed_bundle(dir.path());
        let log = dir.path().join("argv.txt");
        let bin = dir.path().join("fake-krate");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n[ \"$1\" = launch ] && exit 0\necho 'Error: macOS refused' >&2\nexit 1\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        let opened = open_tool(&bin, &json!({ "app": bundle.to_str().unwrap() })).expect("open");
        assert_eq!(opened["opened"], true);
        assert_eq!(opened["name"], "Agent Demo");
        assert!(
            opened["next"].as_str().unwrap().contains("permission")
                || opened["next"].as_str().unwrap().contains("chooses")
        );
        let argv = std::fs::read_to_string(&log).unwrap();
        assert_eq!(argv, format!("launch\n{}\n", bundle.display()));

        let refused = install_tool(&bin, &json!({ "path": bundle.to_str().unwrap() }))
            .expect_err("the fake refuses install");
        assert_eq!(refused.to_string(), "macOS refused");
        let argv = std::fs::read_to_string(&log).unwrap();
        assert!(argv.starts_with("install\n"), "{argv}");
    }

    #[test]
    fn fetching_a_bundle_over_plain_http_is_refused() {
        let err = run_component_tool(&json!({ "bundle": "http://127.0.0.1:1/app.krate" }))
            .expect_err("plain http must be refused");
        assert!(err.to_string().contains("fetch"));
    }
}

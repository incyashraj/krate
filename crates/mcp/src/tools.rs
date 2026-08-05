//! The seven Krate tools, as an MCP `ToolSet`.
//!
//! The shape these take is forced by what a `.krate` actually is: a compiled
//! WebAssembly component built from hand-written `#![no_std]` Rust, where a
//! build takes minutes. So there is no `create_app` that returns a structure to
//! fill in. There is a teaching pair (`krate_schema`, `krate_examples`), an
//! async build pair (`krate_start_build`, `krate_build_status`), a way to get
//! the result out (`krate_package`), a way to look at it (`krate_run`), and --
//! the one that matters most -- the oracle (`krate_check`), which tells a model
//! whether code it wrote itself actually builds, imports only `krate:*`, runs,
//! and paints a frame.
//!
//! Everything slow shells out to the `krate` binary. That is deliberate: this
//! server is a caller of the authoring loop, not a second copy of it that can
//! drift, and every build happens on the user's own machine.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

use crate::examples;
use crate::jobs::{Author, BuildSpec, JobStore, Phase};
use crate::protocol::ToolSet;

/// How long a single non-build tool may take before we stop waiting. A
/// `check-app` compiles a crate, so this is generous; a hang beyond it is a
/// hang, and reporting that beats blocking the client forever.
const CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// Largest bundle we will inline as base64. Beyond this the local path is the
/// answer -- a multi-megabyte blob in a chat message helps nobody and may
/// exceed the client's message limit.
const MAX_INLINE_BYTES: u64 = 4 * 1024 * 1024;

/// The Krate tool set.
pub struct KrateTools {
    /// The `krate` binary that does the real work.
    krate_bin: PathBuf,
    /// Where build outputs and scratch app directories live.
    root: PathBuf,
    jobs: JobStore,
    /// Generates the authoring pack. Injected so this crate does not depend on
    /// the CLI (which depends on the runtime, wasmtime, and everything else).
    schema: fn(&Path) -> String,
}

impl KrateTools {
    pub fn new(krate_bin: PathBuf, root: PathBuf, schema: fn(&Path) -> String) -> Self {
        Self {
            krate_bin,
            root,
            jobs: JobStore::new(),
            schema,
        }
    }
}

impl ToolSet for KrateTools {
    fn server_name(&self) -> &str {
        "krate"
    }

    fn server_version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn instructions(&self) -> Option<String> {
        Some(INSTRUCTIONS.to_string())
    }

    fn tools(&self) -> Vec<Value> {
        tool_definitions()
    }

    fn call(&self, name: &str, arguments: &Value) -> Result<Value, String> {
        match name {
            "krate_schema" => self.schema(),
            "krate_examples" => self.examples(arguments),
            "krate_start_build" => self.start_build(arguments),
            "krate_build_status" => self.build_status(arguments),
            "krate_check" => self.check(arguments),
            "krate_package" => self.package(arguments),
            "krate_run" => self.run(arguments),
            // The protocol layer rejects unknown names before this, so reaching
            // here is a bug in this file rather than a bad client.
            other => Err(format!("`{other}` is not wired up in this server")),
        }
    }
}

/// What the model is told at connect time. This is the only text guaranteed to
/// reach it before it starts guessing, so it says the two things that most
/// change the outcome: what a .krate really is, and that builds are slow.
const INSTRUCTIONS: &str = "\
Krate builds real desktop apps that run in a capability sandbox and ship as one \
`.krate` file anyone can double-click.

Two things shape how to use these tools:

1. A `.krate` is a compiled WebAssembly component built from hand-written \
`#![no_std]` Rust. There is no app schema to fill in -- code has to be written. \
Call `krate_schema` first and read `krate_examples` before writing any; the API \
is small and specific and guessing at it does not work.

2. Builds take minutes. `krate_start_build` returns a job id straight away; poll \
`krate_build_status` and tell the person what stage it is at. Never claim an app \
is ready before status says `succeeded`.

The one hard rule an app must obey: it may import only `krate:*` interfaces. \
Reaching the operating system through `std` pulls in `wasi:*` imports and the \
app is rejected. `krate_check` is the oracle that tells you, for code you wrote, \
whether it builds, imports only `krate:*`, runs, and paints a frame.";

/// Every tool definition, in the order a model should meet them.
fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "krate_schema",
            "title": "Krate authoring reference",
            "description": "The complete Krate authoring pack: every `krate::*` function an app \
                can call, every capability a manifest may declare, the `#![no_std]` discipline \
                and why it exists, and the GUI world's ui/gfx/audio/speech interfaces. Generated \
                from the same WIT and SDK sources the runtime is built against, so it is exact \
                for this version. Call this first and read it before writing any Krate code -- \
                the API is small and specific, and code written from a guess about it will fail \
                the import check.",
            "inputSchema": { "type": "object", "additionalProperties": false },
        }),
        json!({
            "name": "krate_examples",
            "title": "Complete example apps",
            "description": "Two or three complete shipped Krate apps as full source: src/lib.rs, \
                Cargo.toml, and manifest.toml for each. These apps ship and pass CI, so they are \
                proven patterns to adapt rather than illustrations. Read the one closest in shape \
                to what you are building before writing code -- they teach the no_std shape, the \
                widget tree, canvas drawing, and the manifest far better than any description.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["cli", "gui"],
                        "description": "Limit to command-line apps or windowed apps. Omit for all of them.",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "krate_start_build",
            "title": "Start building an app",
            "description": "Start authoring and packaging a Krate app from a plain-English \
                description. Returns a job id IMMEDIATELY -- it does not wait for the build, \
                which takes minutes. Poll `krate_build_status` with the id until it reports \
                `succeeded` or `failed`, and tell the person what stage it is at while you wait. \
                The build runs on this machine using the local Rust toolchain. Do not tell anyone \
                their app is ready until status says `succeeded`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "description": {
                        "type": "string",
                        "description": "What the app should do, in plain words, e.g. \"a habit tracker that shows a streak calendar\". Be specific: this is what the author works from.",
                    },
                    "name": {
                        "type": "string",
                        "description": "Kebab-case name for the app, e.g. `habit-tracker`. Becomes the window title and the data folder the permission wall shows. Derived from the description when omitted. Must be dash-separated words that each start with a lowercase letter.",
                    },
                    "agent": {
                        "type": "string",
                        "description": "The local AI coding agent that writes the app, e.g. `claude`. This is the path that can build an arbitrary app. Omit to use Krate's built-in generator, which is fast but only knows a few templates (checklist, word-count, voice-prompter) and cannot write an arbitrary app.",
                    },
                },
                "required": ["description"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "krate_build_status",
            "title": "Check on a build",
            "description": "Where a build started by `krate_start_build` has got to: whether it \
                is running, succeeded, or failed, the latest progress line, how long it has been \
                going, and on failure the reason with what to do about it. Poll this every 15-30 \
                seconds and narrate real progress rather than guessing.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "job_id": { "type": "string", "description": "The id `krate_start_build` returned." },
                },
                "required": ["job_id"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "krate_check",
            "title": "Check app code (the oracle)",
            "description": "Run Krate's six-stage verdict on app source YOU wrote: layout, \
                manifest, build, imports, run, shoot. This is the tight feedback loop -- it \
                compiles the code, confirms the component imports only `krate:*` interfaces, and \
                runs it once headless. On failure it names the exact stage and the concrete fix, \
                including mapping a leaked `wasi:*` import back to the no_std discipline that \
                removes it. Use this after every edit and do not stop until it reports ok. \
                Pass either a directory that already holds the app, or the file contents \
                directly and it will be written to a scratch directory.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "files": {
                        "type": "object",
                        "description": "App files by relative path, e.g. {\"src/lib.rs\": \"...\", \"Cargo.toml\": \"...\", \"manifest.toml\": \"...\"}. All three are required for a complete check.",
                        "additionalProperties": { "type": "string" },
                    },
                    "dir": {
                        "type": "string",
                        "description": "An existing app directory to check instead of passing files. Use the `app_dir` a build reported.",
                    },
                    "job_id": {
                        "type": "string",
                        "description": "Check the app directory belonging to this build. Use this to fix up an app a build produced.",
                    },
                    "no_run": {
                        "type": "boolean",
                        "description": "Stop after the import check instead of running the app. Use when the app needs input a headless run cannot supply.",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "krate_package",
            "title": "Get the finished .krate",
            "description": "The finished `.krate` from a successful build: its path on this \
                machine, its size, what permissions it asks for, and -- for small bundles -- the \
                file itself as base64 for clients that cannot reach the filesystem. Tell the \
                person the local path; that is the file they send to someone else.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "job_id": { "type": "string", "description": "The id of a build that succeeded." },
                    "include_base64": {
                        "type": "boolean",
                        "description": "Include the bundle bytes inline as base64. Defaults to false; only useful when you cannot reach the local filesystem.",
                    },
                },
                "required": ["job_id"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "krate_run",
            "title": "Look at what was built",
            "description": "Render the app's first frame to a PNG, headless, and return it as an \
                image. Use this to LOOK at what was built and judge it: whether the layout is \
                right, the text fits, the colors work. A command-line app that opens no window \
                cannot be shot -- for those, the build's run stage is the evidence it works.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "job_id": { "type": "string", "description": "The id of a build that succeeded." },
                    "dir": { "type": "string", "description": "An app directory to render instead of a job's." },
                },
                "additionalProperties": false,
            },
        }),
    ]
}

impl KrateTools {
    fn schema(&self) -> Result<Value, String> {
        let pack = (self.schema)(&self.root);
        Ok(json!({
            "authoring_pack": pack,
            "note": "This is generated from the WIT and SDK this Krate was built against. If a \
                     function or capability is not in here, it does not exist -- do not invent it.",
        }))
    }

    fn examples(&self, arguments: &Value) -> Result<Value, String> {
        let kind = arguments.get("kind").and_then(Value::as_str);
        let chosen = examples::select(kind);
        let rendered: Vec<Value> = chosen
            .iter()
            .map(|example| {
                json!({
                    "name": example.name,
                    "kind": example.kind,
                    "teaches": example.teaches,
                    "files": {
                        "src/lib.rs": example.lib_rs,
                        "Cargo.toml": example.cargo_toml,
                        "manifest.toml": example.manifest_toml,
                    },
                })
            })
            .collect();
        Ok(json!({
            "examples": rendered,
            "note": "Every app here ships and passes CI. Adapt the closest one rather than \
                     starting from scratch -- especially its no_std shape and its manifest. \
                     Note that none of them use `format!`, `.unwrap()`, or indexing on a path \
                     that can fail: those pull in panic handling, which drags std's wasi:* \
                     imports in and fails the import check.",
        }))
    }

    fn start_build(&self, arguments: &Value) -> Result<Value, String> {
        let description = arguments
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| {
                "krate_start_build needs `description`: what the app should do, in plain words. \
                 For example \"a habit tracker that shows a streak calendar\"."
                    .to_string()
            })?;

        let name = match arguments.get("name").and_then(Value::as_str) {
            Some(name) => {
                let name = name.trim();
                validate_name(name)?;
                name.to_string()
            }
            None => derive_name(description),
        };

        let author = match arguments.get("agent").and_then(Value::as_str) {
            Some(agent) if !agent.trim().is_empty() => Author::Agent(agent.trim().to_string()),
            _ => Author::BuiltIn,
        };
        let using_builtin = matches!(author, Author::BuiltIn);

        let id = self.jobs.start(BuildSpec {
            krate_bin: self.krate_bin.clone(),
            description: description.to_string(),
            name: name.clone(),
            author,
            root: self.root.join("builds"),
        })?;

        let mut result = json!({
            "job_id": id,
            "name": name,
            "status": "running",
            "next": "Poll krate_build_status with this job_id. A build takes two to five \
                     minutes. Tell the person what stage it is at rather than waiting silently, \
                     and do not say the app is ready until status reports `succeeded`.",
        });
        if using_builtin {
            // Say this now rather than letting a person discover it from an app
            // that is not what they asked for. The built-in generator names the
            // app after the request but only knows a few shapes.
            result["warning"] = json!(
                "No `agent` was given, so Krate's built-in generator is authoring this. It only \
                 knows a few templates (checklist, word-count, voice-prompter) and cannot write \
                 an arbitrary app -- it will name the result after the request but the behaviour \
                 will be the closest template. To have an AI actually write this app, start the \
                 build again with `agent` set to a coding agent installed here, e.g. `claude`."
            );
        }
        Ok(result)
    }

    fn build_status(&self, arguments: &Value) -> Result<Value, String> {
        let id = arguments
            .get("job_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "krate_build_status needs `job_id`, a string".to_string())?;

        let job = self.jobs.get(id).ok_or_else(|| self.no_such_job(id))?;

        let seconds = job.elapsed().as_secs();
        let mut result = json!({
            "job_id": job.id,
            "status": job.phase.as_str(),
            "name": job.name,
            "description": job.description,
            "progress": job.progress,
            "recent": job.log.iter().rev().take(8).rev().collect::<Vec<_>>(),
            "elapsed_seconds": seconds,
            "app_dir": job.app_dir.display().to_string(),
        });

        match job.phase {
            Phase::Running => {
                result["next"] = json!(format!(
                    "Still building ({seconds}s so far). Poll again in 15-30 seconds. Do not \
                     tell anyone the app is ready yet."
                ));
            }
            Phase::Succeeded => {
                result["output"] = json!(job
                    .output
                    .as_ref()
                    .map(|path| path.display().to_string()));
                if let Some(transcript) = &job.transcript {
                    result["transcript"] = transcript.clone();
                    if let Some(caps) = transcript.get("requested_permissions") {
                        result["requested_permissions"] = caps.clone();
                    }
                }
                result["next"] = json!(
                    "Done. Call krate_package for the file's path, and krate_run to render its \
                     first frame so you can look at what was built before saying it is good."
                );
            }
            Phase::Failed => {
                let error = job
                    .error
                    .clone()
                    .unwrap_or_else(|| "the build failed without saying why".to_string());
                result["error"] = json!(error);
                result["next"] = json!(format!(
                    "The build failed. The app source is at {}. Read it, fix it, and call \
                     krate_check on that directory until it reports ok -- check-app names the \
                     exact stage and the fix.",
                    job.app_dir.display()
                ));
            }
        }

        Ok(result)
    }

    fn check(&self, arguments: &Value) -> Result<Value, String> {
        // Three ways to name what to check, in order of directness.
        let (dir, _scratch) = if let Some(dir) = arguments.get("dir").and_then(Value::as_str) {
            (PathBuf::from(dir), None)
        } else if let Some(id) = arguments.get("job_id").and_then(Value::as_str) {
            let job = self.jobs.get(id).ok_or_else(|| self.no_such_job(id))?;
            (job.app_dir, None)
        } else if let Some(files) = arguments.get("files") {
            let (dir, scratch) = self.materialize(files)?;
            (dir, Some(scratch))
        } else {
            return Err("krate_check needs one of `files` (the app source as a map of relative \
                        path to contents), `dir` (an existing app directory), or `job_id` (the \
                        app directory a build produced)."
                .to_string());
        };

        if !dir.is_dir() {
            return Err(format!(
                "{} is not a directory. Pass the folder that holds Cargo.toml, src/lib.rs, and \
                 manifest.toml.",
                dir.display()
            ));
        }

        let mut command = Command::new(&self.krate_bin);
        command.arg("check-app").arg(&dir).arg("--json");
        if arguments
            .get("no_run")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            command.arg("--no-run");
        }

        let output = run_with_timeout(command, CHECK_TIMEOUT)?;
        // check-app emits one JSON object on stdout for both pass and fail, and
        // its exit code names the stage (10-15). Parse the object; the exit
        // code is the fallback when something stopped it before it could speak.
        let text = String::from_utf8_lossy(&output.stdout);
        let parsed: Option<Value> = text
            .lines()
            .rev()
            .find(|line| line.trim_start().starts_with('{'))
            .and_then(|line| serde_json::from_str(line).ok());

        let Some(mut verdict) = parsed else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "check-app did not produce a verdict (exit {}). It said:\n{}",
                output
                    .status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "on a signal".to_string()),
                if stderr.trim().is_empty() {
                    text.trim()
                } else {
                    stderr.trim()
                }
            ));
        };

        // A scratch check is on a copy, so say where it is: the model needs a
        // real path to point krate_run at, and a person may want to look.
        verdict["checked_dir"] = json!(dir.display().to_string());
        if verdict.get("ok").and_then(Value::as_bool) == Some(true) {
            verdict["next"] = json!(
                "Every stage passed: it builds, imports only krate:* interfaces, and runs. Call \
                 krate_run with this `dir` to render its first frame and look at it."
            );
        } else {
            // The `fix` field is the load-bearing one. Make sure it is not
            // buried: a model that only reads `next` still gets pointed at it.
            verdict["next"] = json!(
                "It failed at the stage named in `stage`. Read `detail` for what went wrong and \
                 `fix` for what to do about it, make that change, and call krate_check again. \
                 Do not stop until `ok` is true."
            );
        }
        Ok(verdict)
    }

    fn package(&self, arguments: &Value) -> Result<Value, String> {
        let id = arguments
            .get("job_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "krate_package needs `job_id`, the id of a build that succeeded".to_string())?;
        let job = self.jobs.get(id).ok_or_else(|| self.no_such_job(id))?;

        match job.phase {
            Phase::Running => {
                return Err(format!(
                    "Build {id} is still running ({}s so far), so there is no file yet. Poll \
                     krate_build_status until it reports `succeeded`.",
                    job.elapsed().as_secs()
                ))
            }
            Phase::Failed => {
                return Err(format!(
                    "Build {id} failed, so there is no .krate to hand over. It failed because: \
                     {}. The source is at {} -- fix it and run krate_check on it.",
                    job.error.unwrap_or_else(|| "unknown".to_string()),
                    job.app_dir.display()
                ))
            }
            Phase::Succeeded => {}
        }

        let path = job
            .output
            .ok_or_else(|| format!("build {id} succeeded but recorded no output path"))?;
        let bytes = std::fs::read(&path)
            .map_err(|err| format!("could not read {}: {err}", path.display()))?;

        let mut result = json!({
            "job_id": job.id,
            "path": path.display().to_string(),
            "bytes": bytes.len(),
            "name": job.name,
            "next": format!(
                "The app is the single file at {}. Anyone can double-click it to run it -- they \
                 need Krate installed, but nothing else, and the app can only do what its \
                 permissions say.",
                path.display()
            ),
        });

        if let Some(transcript) = &job.transcript {
            if let Some(caps) = transcript.get("requested_permissions") {
                result["requested_permissions"] = caps.clone();
            }
            if let Some(gating) = transcript.get("gating_permission") {
                result["gating_permission"] = gating.clone();
            }
        }

        if arguments
            .get("include_base64")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            if bytes.len() as u64 > MAX_INLINE_BYTES {
                result["base64_omitted"] = json!(format!(
                    "The bundle is {} bytes, over the {MAX_INLINE_BYTES}-byte inline limit. Use \
                     the local path instead.",
                    bytes.len()
                ));
            } else {
                result["base64"] = json!(base64_encode(&bytes));
            }
        }

        Ok(result)
    }

    fn run(&self, arguments: &Value) -> Result<Value, String> {
        let dir = if let Some(dir) = arguments.get("dir").and_then(Value::as_str) {
            PathBuf::from(dir)
        } else if let Some(id) = arguments.get("job_id").and_then(Value::as_str) {
            let job = self.jobs.get(id).ok_or_else(|| self.no_such_job(id))?;
            job.app_dir
        } else {
            return Err(
                "krate_run needs `job_id` (a build that succeeded) or `dir` (an app directory)."
                    .to_string(),
            );
        };

        if !dir.is_dir() {
            return Err(format!(
                "{} is not a directory, so there is nothing to render.",
                dir.display()
            ));
        }

        let png = self.root.join("shots").join(format!(
            "{}-{}.png",
            dir.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "app".to_string()),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        if let Some(parent) = png.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("could not create {}: {err}", parent.display()))?;
        }

        // check-app --shoot is the rendering path: it rebuilds if needed, runs
        // the app headless, and paints the first frame. Reusing it means a
        // render can never disagree with a check.
        let mut command = Command::new(&self.krate_bin);
        command
            .arg("check-app")
            .arg(&dir)
            .arg("--shoot")
            .arg(&png)
            .arg("--json");
        let output = run_with_timeout(command, CHECK_TIMEOUT)?;

        if !png.exists() {
            let text = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let said = if stderr.trim().is_empty() {
                text.trim()
            } else {
                stderr.trim()
            };
            return Err(format!(
                "No frame was rendered. A command-line app that opens no window cannot be shot; \
                 for one of those, a passing krate_check is the evidence it works. Krate said:\n{said}"
            ));
        }

        let bytes = std::fs::read(&png)
            .map_err(|err| format!("could not read the rendered frame {}: {err}", png.display()))?;

        // An image content block, so the model can actually see it rather than
        // being told a file exists. This is the whole point of the tool.
        Ok(json!({
            "path": png.display().to_string(),
            "bytes": bytes.len(),
            "image_base64": base64_encode(&bytes),
            "mime_type": "image/png",
            "next": "This is the app's first frame. Look at it and judge it: is the layout right, \
                     does the text fit, do the colors work? If something is wrong, fix the source \
                     and call krate_check, then render again.",
        }))
    }

    /// Write a `files` map into a fresh scratch directory.
    fn materialize(&self, files: &Value) -> Result<(PathBuf, PathBuf), String> {
        let Some(map) = files.as_object() else {
            return Err(
                "`files` must be an object mapping relative paths to file contents, e.g. \
                 {\"src/lib.rs\": \"...\", \"Cargo.toml\": \"...\", \"manifest.toml\": \"...\"}."
                    .to_string(),
            );
        };
        if map.is_empty() {
            return Err("`files` is empty; pass at least Cargo.toml, src/lib.rs, and \
                        manifest.toml."
                .to_string());
        }

        let dir = self.root.join("checks").join(format!(
            "check-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir)
            .map_err(|err| format!("could not create {}: {err}", dir.display()))?;

        for (relative, contents) in map {
            let Some(text) = contents.as_str() else {
                return Err(format!("the contents of `{relative}` must be a string"));
            };
            // Containment: a path that escapes the scratch directory would let
            // model-written text overwrite anything the user can write to.
            // Refuse rather than sanitize, so the refusal is visible.
            let safe = safe_relative(relative).ok_or_else(|| {
                format!(
                    "`{relative}` is not a safe relative path. Use paths inside the app like \
                     `src/lib.rs`; absolute paths and `..` are refused."
                )
            })?;
            let destination = dir.join(safe);
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|err| format!("could not create {}: {err}", parent.display()))?;
            }
            std::fs::write(&destination, text)
                .map_err(|err| format!("could not write {relative}: {err}"))?;
        }

        Ok((dir.clone(), dir))
    }

    fn no_such_job(&self, id: &str) -> String {
        let known = self.jobs.ids();
        if known.is_empty() {
            format!(
                "There is no build `{id}`. No builds have been started in this session -- call \
                 krate_start_build first. Job ids do not survive a restart of this server."
            )
        } else {
            format!(
                "There is no build `{id}`. Builds in this session: {}.",
                known.join(", ")
            )
        }
    }
}

/// Whether a relative path stays inside the directory it is joined to.
fn safe_relative(path: &str) -> Option<PathBuf> {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for component in candidate.components() {
        match component {
            std::path::Component::Normal(part) => out.push(part),
            // A bare `./` is harmless noise; everything else escapes or is
            // meaningless in a relative app path.
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// Run a command, killing it if it outlives `timeout`.
///
/// A tool that hangs forever is worse than one that fails: the client waits,
/// the person waits, and nothing says why.
fn run_with_timeout(
    mut command: Command,
    timeout: std::time::Duration,
) -> Result<std::process::Output, String> {
    use std::process::Stdio;

    let mut child = command
        // The MCP client owns this process's stdin. A child must never read it.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("could not run krate: {err}"))?;

    // Drain both pipes on threads so a chatty child cannot fill a pipe and
    // block forever while we sit in try_wait.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_thread = stdout.map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = std::io::Read::read_to_end(&mut pipe, &mut buffer);
            buffer
        })
    });
    let err_thread = stderr.map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = std::io::Read::read_to_end(&mut pipe, &mut buffer);
            buffer
        })
    });

    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "krate did not finish within {} seconds and was stopped. A build that \
                         takes this long usually means the app loops forever or waits for input.",
                        timeout.as_secs()
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(err) => return Err(format!("could not wait for krate: {err}")),
        }
    };

    let stdout = out_thread.and_then(|t| t.join().ok()).unwrap_or_default();
    let stderr = err_thread.and_then(|t| t.join().ok()).unwrap_or_default();
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// Standard base64, no line breaks. Twenty lines beats a dependency for a
/// function this small and this well specified.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) as usize & 63] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Check a name survives becoming a WIT package label and a crate name.
///
/// This is checked here rather than left to the build because the build fails
/// with "invalid label" minutes later, long after the name looked accepted.
fn validate_name(name: &str) -> Result<(), String> {
    let bad = |reason: &str| {
        Err(format!(
            "the app name `{name}` cannot be used: {reason}. Use dash-separated words that each \
             start with a lowercase letter, like `habit-tracker`."
        ))
    };
    if name.is_empty() {
        return bad("it is empty");
    }
    for word in name.split('-') {
        if word.is_empty() {
            return bad("it has an empty word between dashes");
        }
        if !word.starts_with(|c: char| c.is_ascii_lowercase()) {
            return bad("a word does not start with a lowercase letter");
        }
        if !word
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        {
            return bad("a word has something other than lowercase letters and digits in it");
        }
    }
    Ok(())
}

/// Derive a kebab-case name from a plain-English request.
///
/// Mirrors what `krate create` does, so the name the model is told matches the
/// name the build uses.
fn derive_name(request: &str) -> String {
    const SKIP: &[&str] = &[
        "a", "an", "the", "make", "build", "create", "write", "me", "my", "app", "application",
        "simple", "small", "basic", "little", "some", "please", "that", "which", "for", "to",
        "with", "and", "of", "called", "named", "new", "i", "want",
    ];
    let mut words: Vec<String> = Vec::new();
    for raw in request.split_whitespace() {
        let word: String = raw
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_lowercase();
        if word.is_empty() {
            continue;
        }
        // A word starting with a digit cannot be in a WIT label, and a number
        // is detail rather than subject: it ends the name like a stop word.
        if !word.starts_with(|c: char| c.is_ascii_lowercase()) {
            if words.is_empty() {
                continue;
            }
            break;
        }
        if SKIP.contains(&word.as_str()) {
            if words.is_empty() {
                continue;
            }
            break;
        }
        words.push(word);
        if words.len() == 3 {
            break;
        }
    }
    if words.is_empty() {
        "krate-app".to_string()
    } else {
        words.join("-")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools(root: &Path) -> KrateTools {
        KrateTools::new(
            root.join("no-such-krate"),
            root.to_path_buf(),
            |_| "PACK".to_string(),
        )
    }

    #[test]
    fn every_tool_definition_is_a_valid_mcp_tool() {
        for tool in tool_definitions() {
            let name = tool["name"].as_str().expect("a name");
            // The spec's rules for tool names, so no client rejects one of ours.
            assert!(name.len() <= 128 && !name.is_empty());
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')),
                "{name} has characters clients may reject"
            );
            // inputSchema MUST be a JSON Schema object, never null.
            assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
            let description = tool["description"].as_str().expect("a description");
            assert!(
                description.len() > 80,
                "{name}'s description is too thin to steer a model"
            );
        }
    }

    #[test]
    fn the_seven_planned_tools_are_all_there() {
        let names: Vec<String> = tool_definitions()
            .iter()
            .map(|tool| tool["name"].as_str().expect("name").to_string())
            .collect();
        for expected in [
            "krate_schema",
            "krate_examples",
            "krate_start_build",
            "krate_build_status",
            "krate_check",
            "krate_package",
            "krate_run",
        ] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
        assert_eq!(names.len(), 7);
    }

    #[test]
    fn schema_returns_the_pack() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = tools(dir.path()).call("krate_schema", &json!({})).expect("ok");
        assert_eq!(result["authoring_pack"], "PACK");
    }

    #[test]
    fn examples_return_whole_files_not_snippets() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = tools(dir.path())
            .call("krate_examples", &json!({ "kind": "cli" }))
            .expect("ok");
        let list = result["examples"].as_array().expect("array");
        assert!(!list.is_empty());
        for example in list {
            assert_eq!(example["kind"], "cli");
            let lib = example["files"]["src/lib.rs"].as_str().expect("lib.rs");
            assert!(lib.contains("#![no_std]"));
            assert!(example["files"]["manifest.toml"]
                .as_str()
                .expect("manifest")
                .contains("[app]"));
        }
    }

    #[test]
    fn a_build_without_a_description_says_what_is_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = tools(dir.path())
            .call("krate_start_build", &json!({}))
            .expect_err("must fail");
        // The test is that the message teaches, not that it fails.
        assert!(err.contains("description"));
        assert!(err.contains("habit tracker"), "no example given: {err}");
    }

    #[test]
    fn a_build_with_a_bad_name_is_refused_before_the_build_not_during_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = tools(dir.path())
            .call(
                "krate_start_build",
                &json!({ "description": "a game", "name": "2048" }),
            )
            .expect_err("must fail");
        assert!(err.contains("2048"));
        assert!(err.contains("lowercase letter"));
    }

    #[test]
    fn a_build_without_an_agent_warns_that_the_template_generator_is_limited() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = tools(dir.path())
            .call("krate_start_build", &json!({ "description": "a pdf merger" }))
            .expect("start");
        assert_eq!(result["status"], "running");
        assert_eq!(result["name"], "pdf-merger");
        // Silence here is how a person ends up with a checklist named
        // "pdf-merger" and a model insisting it built what they asked for.
        let warning = result["warning"].as_str().expect("a warning");
        assert!(warning.contains("cannot write an arbitrary app"));
    }

    #[test]
    fn asking_about_an_unknown_job_names_the_jobs_that_do_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tools = tools(dir.path());
        let err = tools
            .call("krate_build_status", &json!({ "job_id": "build-99" }))
            .expect_err("must fail");
        assert!(err.contains("No builds have been started"));

        let started = tools
            .call("krate_start_build", &json!({ "description": "a timer" }))
            .expect("start");
        let real = started["job_id"].as_str().expect("id");
        let err = tools
            .call("krate_build_status", &json!({ "job_id": "build-99" }))
            .expect_err("must fail");
        assert!(err.contains(real), "should list the real job: {err}");
    }

    #[test]
    fn packaging_a_running_build_refuses_instead_of_returning_a_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tools = tools(dir.path());
        let started = tools
            .call("krate_start_build", &json!({ "description": "a timer" }))
            .expect("start");
        let id = started["job_id"].as_str().expect("id").to_string();

        // Either it is still running or it already failed (the binary is fake).
        // Both must refuse to hand over a file, and both must say why.
        let err = tools
            .call("krate_package", &json!({ "job_id": id }))
            .expect_err("must fail");
        assert!(
            err.contains("still running") || err.contains("failed"),
            "unclear refusal: {err}"
        );
    }

    #[test]
    fn check_without_any_target_says_what_it_needs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = tools(dir.path())
            .call("krate_check", &json!({}))
            .expect_err("must fail");
        assert!(err.contains("files"));
        assert!(err.contains("dir"));
        assert!(err.contains("job_id"));
    }

    #[test]
    fn check_refuses_a_path_that_would_escape_the_scratch_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Model-written text must never be able to name a path outside the
        // directory we chose for it.
        for escape in ["../evil.rs", "/etc/passwd", "src/../../evil.rs"] {
            let err = tools(dir.path())
                .call("krate_check", &json!({ "files": { escape: "x" } }))
                .expect_err("must refuse");
            assert!(err.contains("safe relative path"), "{escape}: {err}");
        }
    }

    #[test]
    fn check_writes_the_files_it_is_given_into_a_scratch_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tools = tools(dir.path());
        // The krate binary is fake, so the check itself fails -- but the files
        // must have been laid out correctly first.
        let _ = tools.call(
            "krate_check",
            &json!({ "files": { "src/lib.rs": "// code", "Cargo.toml": "[package]" } }),
        );
        let checks = dir.path().join("checks");
        let written: Vec<PathBuf> = std::fs::read_dir(&checks)
            .expect("checks dir")
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .collect();
        assert_eq!(written.len(), 1);
        assert_eq!(
            std::fs::read_to_string(written[0].join("src/lib.rs")).expect("lib.rs"),
            "// code"
        );
    }

    #[test]
    fn empty_files_is_refused_with_the_three_it_wants() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = tools(dir.path())
            .call("krate_check", &json!({ "files": {} }))
            .expect_err("must fail");
        assert!(err.contains("manifest.toml"));
    }

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        // A .krate is binary, so the high bytes have to be right too.
        assert_eq!(base64_encode(&[0x00, 0xff, 0x80]), "AP+A");
    }

    #[test]
    fn names_are_derived_the_same_way_the_cli_derives_them() {
        assert_eq!(derive_name("a reading list app to track books"), "reading-list");
        assert_eq!(derive_name("Make me a habit tracker"), "habit-tracker");
        // A number cannot start a WIT label, so it ends the name.
        assert_eq!(derive_name("pomodoro timer: 25 minute sessions"), "pomodoro-timer");
        // Nothing usable must still give a legal name rather than an empty one.
        assert_eq!(derive_name("the a an"), "krate-app");
        assert!(validate_name(&derive_name("2048 tile game")).is_ok());
    }

    #[test]
    fn safe_relative_accepts_app_paths_and_rejects_escapes() {
        assert!(safe_relative("src/lib.rs").is_some());
        assert!(safe_relative("./manifest.toml").is_some());
        assert!(safe_relative("..").is_none());
        assert!(safe_relative("a/../../b").is_none());
        assert!(safe_relative("/abs").is_none());
        assert!(safe_relative("").is_none());
    }
}

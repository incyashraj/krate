//! Direct tests for Studio's Tauri commands.
//!
//! Each command that can run without a window is called here the way the UI
//! calls it, and the test checks what it left behind: the files on disk, the
//! value it returned, the exact sentence of a refusal.
//!
//! Isolation, because these share one process:
//! - Studio's state lives under HOME, so every test that touches it takes a
//!   `TestHome`: a throwaway HOME, behind one lock, restored on drop.
//! - The engine is a shell script the test writes (`TestHome::engine`), so
//!   nothing needs a real build. With no script, KRATE_STUDIO_ENGINE points
//!   at a path that does not exist -- the missing-engine case -- and never
//!   falls through to a `krate` on PATH.
//! - The hub is a closed port on 127.0.0.1, or a stub on 127.0.0.1 when a
//!   test needs an answer. Nothing here leaves this machine, opens a window,
//!   or runs an AI.

#![cfg_attr(not(unix), allow(dead_code))]

use super::*;

use std::ffi::OsString;
use std::io::Write as _;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, MutexGuard};
use std::time::Duration;

/* ---- the shared test home ---------------------------------------------- */

static ENV: Mutex<()> = Mutex::new(());

/// The process environment is one thing shared by every test thread. Any
/// test that reads or sets HOME (or the engine and hub variables) holds this.
pub(crate) fn env_lock() -> MutexGuard<'static, ()> {
    ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

const VARS: [&str; 6] = [
    "HOME",
    "USERPROFILE",
    "KRATE_HUB_URL",
    "KRATE_STUDIO_ENGINE",
    "KRATE_STUDIO_AUTORUN",
    "KRATE_STUDIO_DEBUG",
];

/// A fresh HOME for one test, with the engine missing and the hub closed.
pub(crate) struct TestHome {
    root: PathBuf,
    saved: Vec<(&'static str, Option<OsString>)>,
    _dir: tempfile::TempDir,
    _lock: MutexGuard<'static, ()>,
}

impl TestHome {
    pub(crate) fn new() -> TestHome {
        let lock = env_lock();
        let saved = VARS.iter().map(|k| (*k, std::env::var_os(k))).collect();
        let dir = tempfile::tempdir().expect("temp dir");
        // Canonical, so it compares equal to what canonicalize() hands back
        // (/var is /private/var on macOS).
        let root = std::fs::canonicalize(dir.path()).expect("canonical temp dir");
        let home = root.join("home");
        std::fs::create_dir_all(&home).expect("home");
        std::env::set_var("HOME", &home);
        std::env::set_var("USERPROFILE", &home);
        std::env::set_var("KRATE_HUB_URL", dead_url());
        std::env::set_var("KRATE_STUDIO_ENGINE", root.join("no-engine").join("krate"));
        std::env::remove_var("KRATE_STUDIO_AUTORUN");
        std::env::remove_var("KRATE_STUDIO_DEBUG");
        TestHome {
            root,
            saved,
            _dir: dir,
            _lock: lock,
        }
    }

    /// Scratch space beside the home, not inside it.
    pub(crate) fn root(&self) -> PathBuf {
        self.root.clone()
    }

    pub(crate) fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    pub(crate) fn studio(&self) -> PathBuf {
        self.home().join(".krate").join("studio")
    }

    /// Point the hub at a stub.
    pub(crate) fn hub(&self, hub: &Hub) {
        std::env::set_var("KRATE_HUB_URL", &hub.url);
    }

    /// Sign this home in, the way the engine stores it.
    pub(crate) fn sign_in(&self, token: &str) {
        let dir = self.home().join(".krate");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("github.json"),
            serde_json::json!({ "login": "ada", "token": token }).to_string(),
        )
        .unwrap();
    }

    /// Write a stub engine. Every call records its arguments, one per line,
    /// to `args()`, its whole command line to `calls()`, and its working
    /// directory; then `body` runs as /bin/sh.
    #[cfg(unix)]
    pub(crate) fn engine(&self, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = self.root.join("engine");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("krate");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{args}'\necho \"$*\" >> '{calls}'\npwd -P > '{cwd}'\n{body}\n",
            args = self.root.join("args").display(),
            calls = self.root.join("calls").display(),
            cwd = self.root.join("cwd").display(),
        );
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("KRATE_STUDIO_ENGINE", &path);
        path
    }

    /// The arguments of the engine's last run; empty when it never ran.
    pub(crate) fn args(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.join("args"))
            .map(|t| t.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// Every engine command line, in order.
    pub(crate) fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.join("calls"))
            .map(|t| t.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    pub(crate) fn engine_ran(&self) -> bool {
        self.root.join("args").exists()
    }

    pub(crate) fn engine_cwd(&self) -> PathBuf {
        PathBuf::from(
            std::fs::read_to_string(self.root.join("cwd"))
                .unwrap_or_default()
                .trim(),
        )
    }

    /// What the missing-engine refusal says for this home.
    pub(crate) fn missing_engine(&self) -> String {
        format!(
            "KRATE_STUDIO_ENGINE points at {}, which does not exist",
            self.root.join("no-engine").join("krate").display()
        )
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        // A test may have made folders read-only; the temp dir must still go.
        #[cfg(unix)]
        make_writable(&self.root);
        for (key, value) in &self.saved {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }
}

#[cfg(unix)]
fn make_writable(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(meta) = std::fs::symlink_metadata(&path) {
                if meta.is_dir() {
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
                    make_writable(&path);
                }
            }
        }
    }
}

/// A local address nothing is listening on.
pub(crate) fn dead_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    format!("http://{addr}")
}

/// Run an async command to its end.
pub(crate) fn block<F: std::future::Future>(future: F) -> F::Output {
    tauri::async_runtime::block_on(future)
}

/// A .krate-shaped zip with these entries.
pub(crate) fn zip_with(path: &Path, files: &[(&str, &[u8])]) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    for (name, bytes) in files {
        zip.start_file(*name, opts).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}

/// A Krate project folder (Cargo.toml + manifest.toml + src/lib.rs), canonical.
pub(crate) fn make_project(parent: &Path, name: &str) -> PathBuf {
    let p = parent.join(name);
    std::fs::create_dir_all(p.join("src")).unwrap();
    std::fs::write(
        p.join("Cargo.toml"),
        "[package]\nname = \"proj\"\n\n[dependencies]\nkrate = { path = \"/sdk/root/crates/bindings-rust\" }\n",
    )
    .unwrap();
    std::fs::write(p.join("manifest.toml"), "[app]\nid = \"x\"\n").unwrap();
    std::fs::write(p.join("src/lib.rs"), "// hi\n").unwrap();
    std::fs::canonicalize(p).unwrap()
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really pixels";

fn a_session(id: &str, updated: u64, title: &str) -> Session {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "title": title,
        "created": 1,
        "updated": updated,
        "messages": [{ "role": "user", "text": "a tip splitter" }],
        "result": null,
    }))
    .expect("a session")
}

/* ---- a stub hub on 127.0.0.1 ------------------------------------------- */

#[derive(Clone, Debug, Default)]
pub(crate) struct Seen {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) head: String,
    pub(crate) body: String,
}

impl Seen {
    fn header(&self, name: &str) -> Option<String> {
        self.head.lines().find_map(|line| {
            let (k, v) = line.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_string())
        })
    }
}

pub(crate) struct Hub {
    pub(crate) url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

type Route = dyn Fn(&Seen) -> (u16, Vec<u8>) + Send + 'static;

impl Hub {
    pub(crate) fn start(route: impl Fn(&Seen) -> (u16, Vec<u8>) + Send + 'static) -> Hub {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub hub");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let route: Box<Route> = Box::new(route);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let Some(request) = read_request(&mut stream) else {
                    continue;
                };
                let (status, body) = route(&request);
                log.lock().unwrap().push(request);
                let head = format!(
                    "HTTP/1.1 {status} STUB\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        Hub { url, seen }
    }

    pub(crate) fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// Wait up to five seconds for a request matching `want`.
    fn wait_for(&self, want: impl Fn(&Seen) -> bool) -> Option<Seen> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(hit) = self.seen().into_iter().find(|s| want(s)) {
                return Some(hit);
            }
            if std::time::Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Seen> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let length = head
        .lines()
        .find_map(|line| {
            let (k, v) = line.split_once(':')?;
            if k.trim().eq_ignore_ascii_case("content-length") {
                v.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let mut first = head.lines().next().unwrap_or("").split_whitespace();
    Some(Seen {
        method: first.next()?.to_string(),
        path: first.next()?.to_string(),
        head: head.clone(),
        body: String::from_utf8_lossy(&body).to_string(),
    })
}

fn ok_json(value: serde_json::Value) -> (u16, Vec<u8>) {
    (200, value.to_string().into_bytes())
}

/* ---- settings ----------------------------------------------------------- */

#[test]
fn settings_on_a_fresh_home_are_the_defaults_and_nothing_is_written() {
    let t = TestHome::new();
    let s = settings_get();
    assert_eq!(PathBuf::from(&s.out_dir), t.home().join("Krate Apps"));
    assert_eq!(s.agent, "claude");
    assert!(
        !t.studio().join("settings.json").exists(),
        "reading settings must not write them"
    );
}

#[test]
fn settings_round_trip_a_folder_with_spaces_and_unicode() {
    let t = TestHome::new();
    let out = t.home().join("Mes Apps ünï 日本");
    settings_set(Settings {
        out_dir: out.display().to_string(),
        agent: "codex".to_string(),
    })
    .expect("save settings");
    let back = settings_get();
    assert_eq!(back.out_dir, out.display().to_string());
    assert_eq!(back.agent, "codex");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(t.studio().join("settings.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "settings are private (mode {mode:o})");
    }
}

#[test]
fn a_corrupted_settings_file_reads_as_the_defaults_and_is_left_on_disk() {
    let t = TestHome::new();
    let file = studio_dir().join("settings.json");
    std::fs::write(&file, "{\"out_dir\": \"/x\", ").unwrap();
    let s = settings_get();
    assert_eq!(PathBuf::from(&s.out_dir), t.home().join("Krate Apps"));
    assert_eq!(s.agent, "claude");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "{\"out_dir\": \"/x\", ",
        "a read must not overwrite what it could not parse"
    );
}

#[test]
fn the_old_documents_default_is_moved_out_and_saved() {
    let t = TestHome::new();
    settings_set(Settings {
        out_dir: t
            .home()
            .join("Documents")
            .join("Krate Apps")
            .display()
            .to_string(),
        agent: "gemini".to_string(),
    })
    .unwrap();
    let s = settings_get();
    assert_eq!(PathBuf::from(&s.out_dir), t.home().join("Krate Apps"));
    assert_eq!(s.agent, "gemini", "only the folder moves");
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(t.studio().join("settings.json")).unwrap())
            .unwrap();
    assert_eq!(
        saved["out_dir"],
        t.home().join("Krate Apps").display().to_string()
    );
}

#[test]
fn a_folder_the_person_chose_inside_documents_is_kept() {
    let t = TestHome::new();
    let mine = t.home().join("Documents").join("Krate Apps").join("mine");
    settings_set(Settings {
        out_dir: mine.display().to_string(),
        agent: "claude".to_string(),
    })
    .unwrap();
    assert_eq!(PathBuf::from(settings_get().out_dir), mine);
}

#[test]
fn settings_set_says_so_when_the_file_cannot_be_replaced_and_leaves_no_litter() {
    let t = TestHome::new();
    // A folder where the file should be: the rename cannot land.
    std::fs::create_dir_all(t.studio().join("settings.json")).unwrap();
    let err = settings_set(Settings::default()).unwrap_err();
    assert!(err.starts_with("could not save "), "{err}");
    let litter: Vec<String> = std::fs::read_dir(t.studio())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(litter.is_empty(), "staging left behind: {litter:?}");
}

/* ---- first run ---------------------------------------------------------- */

#[test]
fn onboarding_starts_unseen_sticks_and_a_corrupted_record_reads_as_unseen() {
    let t = TestHome::new();
    assert!(!onboarded_get());
    onboarded_set(true).unwrap();
    assert!(onboarded_get());
    onboarded_set(false).unwrap();
    assert!(!onboarded_get());
    std::fs::write(t.studio().join("first-run.json"), "{\"onboarded\": tr").unwrap();
    assert!(!onboarded_get());
}

/* ---- sessions ----------------------------------------------------------- */

#[test]
fn sessions_on_a_fresh_home_are_none() {
    let _t = TestHome::new();
    assert!(sessions_list().is_empty());
}

#[test]
fn sessions_come_back_newest_first_with_every_field_the_ui_saved() {
    let _t = TestHome::new();
    let mut failed = a_session("100-b", 50, "failed one");
    failed
        .extra
        .insert("failedRequest".to_string(), serde_json::json!("a timer"));
    session_save(a_session("100-a", 10, "old")).unwrap();
    session_save(failed).unwrap();
    session_save(a_session("100-c", 30, "middle")).unwrap();
    let list = sessions_list();
    let ids: Vec<&str> = list.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, vec!["100-b", "100-c", "100-a"]);
    assert_eq!(
        list[0].extra.get("failedRequest"),
        Some(&serde_json::json!("a timer")),
        "a field the struct does not name survives the round trip (K-203)"
    );
    assert_eq!(list[0].messages.len(), 1);
}

#[test]
fn a_corrupted_session_file_is_skipped_and_the_rest_still_list() {
    let t = TestHome::new();
    session_save(a_session("1", 5, "good")).unwrap();
    let dir = t.studio().join("sessions");
    std::fs::write(dir.join("2.json"), "{\"id\": \"2\", \"title\": ").unwrap();
    std::fs::write(dir.join("3.json"), "[]").unwrap();
    std::fs::write(dir.join("1.shot.png"), PNG).unwrap();
    let list = sessions_list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].title, "good");
}

#[test]
fn session_save_refuses_an_id_that_is_not_a_plain_name() {
    let t = TestHome::new();
    for bad in [
        "../escape",
        "a/b",
        "a.b",
        "x y",
        "ünï",
        "a\\b",
        "..",
        "id\0",
    ] {
        assert_eq!(
            session_save(a_session(bad, 1, "x")).unwrap_err(),
            "bad session id",
            "{bad:?}"
        );
        assert_eq!(session_shot(bad.to_string()).unwrap_err(), "bad session id");
        assert_eq!(
            session_delete(bad.to_string()).unwrap_err(),
            "bad session id"
        );
        assert_eq!(
            session_source_dir(bad.to_string()).unwrap_err(),
            "bad session id"
        );
    }
    assert!(!t.studio().join("escape.json").exists());
    assert!(std::fs::read_dir(t.studio().join("sessions"))
        .map(|mut d| d.next().is_none())
        .unwrap_or(true));
}

#[test]
fn an_empty_session_id_is_refused() {
    let t = TestHome::new();
    let other = t.studio().join("builds").join("someone-else");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("Cargo.toml"), "[package]\n").unwrap();
    assert_eq!(
        session_source_dir(String::new()),
        Err("bad session id".to_string()),
        "an empty id must not resolve to another app's source"
    );
    assert_eq!(
        session_save(a_session("", 1, "x")),
        Err("bad session id".to_string())
    );
    assert!(!t.studio().join("sessions").join(".json").exists());
}

#[test]
fn an_inline_screenshot_moves_beside_the_json_and_comes_back_the_same() {
    let t = TestHome::new();
    let url = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(PNG)
    );
    let mut s = a_session("200", 1, "with a picture");
    s.result = Some(serde_json::json!({ "path": "/x.krate", "shot": url }));
    session_save(s).unwrap();
    let dir = t.studio().join("sessions");
    assert_eq!(std::fs::read(dir.join("200.shot.png")).unwrap(), PNG);
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("200.json")).unwrap()).unwrap();
    assert_eq!(saved["result"]["shot"], "file");
    assert_eq!(session_shot("200".to_string()).unwrap(), url);
}

#[test]
fn a_screenshot_that_is_not_base64_stays_where_it_was() {
    let t = TestHome::new();
    let mut s = a_session("201", 1, "x");
    s.result = Some(serde_json::json!({ "shot": "data:image/png;base64,!!not base64!!" }));
    session_save(s).unwrap();
    assert!(!t.studio().join("sessions").join("201.shot.png").exists());
    let back = &sessions_list()[0];
    assert_eq!(
        back.result.as_ref().unwrap()["shot"],
        "data:image/png;base64,!!not base64!!"
    );
}

#[test]
fn session_shot_for_a_session_with_no_picture_says_so() {
    let _t = TestHome::new();
    assert_eq!(
        session_shot("300".to_string()).unwrap_err(),
        "no shot for this session"
    );
}

#[test]
fn session_delete_removes_the_json_and_the_picture_and_a_second_delete_is_fine() {
    let t = TestHome::new();
    let mut s = a_session("400", 1, "x");
    s.result = Some(serde_json::json!({
        "shot": format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(PNG))
    }));
    session_save(s).unwrap();
    let dir = t.studio().join("sessions");
    assert!(dir.join("400.json").exists() && dir.join("400.shot.png").exists());
    session_delete("400".to_string()).unwrap();
    assert!(!dir.join("400.json").exists());
    assert!(!dir.join("400.shot.png").exists());
    session_delete("400".to_string()).expect("deleting what is gone is fine");
    assert!(sessions_list().is_empty());
}

#[test]
fn a_session_id_too_long_for_a_file_name_is_an_error_not_a_panic() {
    let t = TestHome::new();
    let long = "a".repeat(300);
    let err = session_save(a_session(&long, 1, "x")).unwrap_err();
    assert!(err.starts_with("could not open"), "{err}");
    assert!(sessions_list().is_empty());
    let litter = std::fs::read_dir(t.studio().join("sessions"))
        .unwrap()
        .count();
    assert_eq!(litter, 0);
}

#[test]
fn a_signed_in_save_is_pushed_to_the_account() {
    let t = TestHome::new();
    let hub = Hub::start(|_| ok_json(serde_json::json!({})));
    t.hub(&hub);
    t.sign_in("tok-123");
    session_save(a_session("500", 7, "pushed")).unwrap();
    let push = hub
        .wait_for(|s| s.method == "POST" && s.path == "/sessions")
        .expect("the save reached the hub");
    assert_eq!(
        push.header("authorization").as_deref(),
        Some("Bearer tok-123")
    );
    let body: serde_json::Value = serde_json::from_str(&push.body).unwrap();
    assert_eq!(body["id"], "500");
    assert_eq!(body["title"], "pushed");
    assert!(
        t.studio().join("sessions").join("500.json").is_file(),
        "disk first"
    );
}

#[test]
fn a_signed_out_save_sends_nothing() {
    let t = TestHome::new();
    let hub = Hub::start(|_| ok_json(serde_json::json!({})));
    t.hub(&hub);
    session_save(a_session("501", 7, "local only")).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(hub.seen().is_empty(), "{:?}", hub.seen());
}

#[test]
fn sessions_pull_signed_out_is_zero_and_asks_nobody() {
    let t = TestHome::new();
    let hub = Hub::start(|_| ok_json(serde_json::json!({ "sessions": [] })));
    t.hub(&hub);
    assert_eq!(block(sessions_pull()), Ok(0));
    assert!(hub.seen().is_empty());
}

#[test]
fn sessions_pull_from_an_unreachable_or_garbled_hub_is_zero() {
    let t = TestHome::new();
    t.sign_in("tok");
    assert_eq!(block(sessions_pull()), Ok(0), "closed port");

    let hub = Hub::start(|_| (200, b"this is not json".to_vec()));
    t.hub(&hub);
    assert_eq!(block(sessions_pull()), Ok(0), "not json");

    let hub = Hub::start(|_| ok_json(serde_json::json!({ "sessions": [{ "nope": 1 }] })));
    t.hub(&hub);
    assert_eq!(block(sessions_pull()), Ok(0), "not sessions");
    assert!(sessions_list().is_empty());
}

#[test]
fn sessions_pull_takes_newer_remote_sessions_and_keeps_newer_local_ones() {
    let t = TestHome::new();
    session_save(a_session("1", 10, "local older")).unwrap();
    session_save(a_session("2", 50, "local newer")).unwrap();
    let hub = Hub::start(|seen| {
        if seen.method == "GET" && seen.path == "/sessions" {
            ok_json(serde_json::json!({ "sessions": [
                { "id": "1", "title": "remote newer", "created": 1, "updated": 20, "messages": [], "result": null },
                { "id": "2", "title": "remote older", "created": 1, "updated": 40, "messages": [], "result": null },
                { "id": "3", "title": "only remote", "created": 1, "updated": 5, "messages": [], "result": null },
                { "id": "../4", "title": "hostile id", "created": 1, "updated": 99, "messages": [], "result": null },
            ]}))
        } else {
            ok_json(serde_json::json!({}))
        }
    });
    t.hub(&hub);
    t.sign_in("tok");
    assert_eq!(block(sessions_pull()), Ok(2));
    let title = |id: &str| {
        sessions_list()
            .into_iter()
            .find(|s| s.id == id)
            .map(|s| s.title)
    };
    assert_eq!(title("1").as_deref(), Some("remote newer"));
    assert_eq!(title("2").as_deref(), Some("local newer"));
    assert_eq!(title("3").as_deref(), Some("only remote"));
    assert_eq!(sessions_list().len(), 3, "the hostile id was not written");
    assert!(!t.studio().join("4.json").exists());
}

#[test]
fn sessions_pull_fetches_a_web_made_app_into_the_apps_folder() {
    let t = TestHome::new();
    let hub = Hub::start(|seen| {
        let host = seen.header("host").unwrap_or_default();
        match (seen.method.as_str(), seen.path.as_str()) {
            ("GET", "/sessions") => ok_json(serde_json::json!({ "sessions": [{
                "id": "9", "title": "from the web", "created": 1, "updated": 2, "messages": [],
                "result": { "web": true, "name": "Tip Splitter!", "path": format!("http://{host}/files/tip.krate") }
            }]})),
            ("GET", "/files/tip.krate") => (200, b"PK the app".to_vec()),
            _ => ok_json(serde_json::json!({})),
        }
    });
    t.hub(&hub);
    t.sign_in("tok-web");
    assert_eq!(block(sessions_pull()), Ok(1));
    let file = t.home().join("Krate Apps").join("Tip-Splitter-.krate");
    assert_eq!(std::fs::read(&file).unwrap(), b"PK the app");
    let s = &sessions_list()[0];
    let result = s.result.as_ref().unwrap();
    assert_eq!(result["path"], file.display().to_string());
    assert_eq!(result["web"], false);
    assert!(result["imported_from"]
        .as_str()
        .unwrap()
        .ends_with("/files/tip.krate"));
    let fetch = hub
        .wait_for(|s| s.path == "/files/tip.krate")
        .expect("fetched");
    assert_eq!(
        fetch.header("authorization").as_deref(),
        Some("Bearer tok-web")
    );
}

/* ---- create/port helpers: the target is recorded, never overwritten ---- */

#[test]
fn the_target_app_is_recorded_in_its_session_before_the_build() {
    let t = TestHome::new();
    let mut s = a_session("600", 3, "x");
    s.extra
        .insert("buildStarted".to_string(), serde_json::json!(true));
    session_save(s).unwrap();
    let target = t.home().join("Krate Apps").join("tip splitter ü.krate");
    remember_target("600", &target);
    let back = sessions_list().remove(0);
    assert_eq!(
        back.extra.get("pending_path"),
        Some(&serde_json::json!(target.display().to_string()))
    );
    assert_eq!(
        back.extra.get("buildStarted"),
        Some(&serde_json::json!(true))
    );
}

#[test]
fn recording_a_target_ignores_a_bad_id_and_a_corrupted_session() {
    let t = TestHome::new();
    let dir = studio_dir().join("sessions");
    std::fs::write(dir.join("700.json"), "{ broken").unwrap();
    remember_target("700", Path::new("/x.krate"));
    assert_eq!(
        std::fs::read_to_string(dir.join("700.json")).unwrap(),
        "{ broken"
    );
    remember_target("../700", Path::new("/x.krate"));
    remember_target("", Path::new("/x.krate"));
    assert!(!t.studio().join("700.json").exists());
    assert!(!dir.join(".json").exists());
}

#[test]
fn a_second_app_from_the_same_words_gets_its_own_file() {
    let t = TestHome::new();
    let dir = t.root().join("apps ü");
    std::fs::create_dir_all(&dir).unwrap();
    let first = free_path(&dir, "habit-tracker");
    assert_eq!(first, dir.join("habit-tracker.krate"));
    std::fs::write(&first, "1").unwrap();
    let second = free_path(&dir, "habit-tracker");
    assert_eq!(second, dir.join("habit-tracker 2.krate"));
    std::fs::write(&second, "2").unwrap();
    assert_eq!(
        free_path(&dir, "habit-tracker"),
        dir.join("habit-tracker 3.krate")
    );
}

#[test]
fn file_names_from_any_request_are_short_safe_and_never_empty() {
    for request in [
        "",
        "   ",
        "日本語のアプリ",
        "- a pasted bullet\n- and another",
        "../../etc/passwd",
        &"x".repeat(10_000),
        &"word ".repeat(2_000),
        "a 🎲 dice roller",
    ] {
        let slug = slugify(request);
        assert!(!slug.is_empty(), "{request:?}");
        assert!(slug.len() <= 4 * 24 + 3, "{request:?} -> {slug}");
        assert!(
            slug.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "{request:?} -> {slug}"
        );
    }
    assert_eq!(slugify("日本語のアプリ"), "my-app");
    assert_eq!(slugify("a 🎲 dice roller"), "dice-roller");
}

/* ---- source folder ------------------------------------------------------ */

#[test]
fn the_source_folder_is_found_at_either_depth() {
    let t = TestHome::new();
    let builds = t.studio().join("builds");
    std::fs::create_dir_all(builds.join("s1")).unwrap();
    std::fs::write(builds.join("s1").join("Cargo.toml"), "").unwrap();
    std::fs::create_dir_all(builds.join("s2").join("tip-splitter")).unwrap();
    std::fs::write(
        builds.join("s2").join("tip-splitter").join("Cargo.toml"),
        "",
    )
    .unwrap();
    assert_eq!(
        session_source_dir("s1".to_string()).unwrap(),
        builds.join("s1").display().to_string()
    );
    assert_eq!(
        session_source_dir("s2".to_string()).unwrap(),
        builds.join("s2").join("tip-splitter").display().to_string()
    );
}

#[test]
fn the_source_folder_says_what_is_missing() {
    let t = TestHome::new();
    assert_eq!(
        session_source_dir("s9".to_string()).unwrap_err(),
        "no build workspace for this app"
    );
    std::fs::create_dir_all(t.studio().join("builds").join("s9").join("logs")).unwrap();
    assert_eq!(
        session_source_dir("s9".to_string()).unwrap_err(),
        "no source folder for this app"
    );
}

/* ---- pasted text -------------------------------------------------------- */

#[test]
fn a_paste_is_kept_as_a_file_with_its_exact_text() {
    let t = TestHome::new();
    let text = "fn main() {}\n// ünï 日本 🎲\n\0tail";
    let path = PathBuf::from(stash_pasted_text(text.to_string()).unwrap());
    assert!(
        path.starts_with(t.studio().join("pasted")),
        "{}",
        path.display()
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    let empty = PathBuf::from(stash_pasted_text(String::new()).unwrap());
    assert_eq!(std::fs::read(&empty).unwrap(), b"");
}

#[test]
fn a_paste_over_ten_megabytes_is_refused_and_exactly_ten_is_kept() {
    let t = TestHome::new();
    let limit = 10 * 1024 * 1024;
    assert_eq!(
        stash_pasted_text("a".repeat(limit + 1)).unwrap_err(),
        "That paste is over the 10 MB attachment limit."
    );
    assert!(
        !t.studio().join("pasted").exists()
            || std::fs::read_dir(t.studio().join("pasted"))
                .unwrap()
                .next()
                .is_none(),
        "nothing written for a refused paste"
    );
    let path = stash_pasted_text("a".repeat(limit)).unwrap();
    assert_eq!(std::fs::metadata(path).unwrap().len(), limit as u64);
}

#[test]
fn two_pastes_in_the_same_moment_do_not_overwrite_each_other() {
    let _t = TestHome::new();
    let pasted: Vec<(String, String)> = (0..20)
        .map(|i| {
            let text = format!("paste number {i}");
            (stash_pasted_text(text.clone()).unwrap(), text)
        })
        .collect();
    let mut paths: Vec<&String> = pasted.iter().map(|(p, _)| p).collect();
    paths.sort();
    paths.dedup();
    assert_eq!(paths.len(), 20, "every paste needs its own file");
    for (path, text) in &pasted {
        assert_eq!(&std::fs::read_to_string(path).unwrap(), text);
    }
}

/* ---- images ------------------------------------------------------------- */

#[test]
fn a_png_becomes_a_data_url_from_a_path_with_spaces_and_unicode() {
    let t = TestHome::new();
    let path = t.root().join("my shot ü 日本.png");
    std::fs::write(&path, PNG).unwrap();
    let url = block(read_image(path.display().to_string())).unwrap();
    let b64 = url.strip_prefix("data:image/png;base64,").unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap(),
        PNG
    );
}

#[test]
fn read_image_refuses_what_is_not_a_small_png_in_plain_words() {
    let t = TestHome::new();
    let r = t.root();
    let ask = |p: &Path| block(read_image(p.display().to_string()));

    assert_eq!(
        ask(&r.join("gone.png")).unwrap_err(),
        "gone.png is not there any more -- it may have been moved or deleted."
    );
    std::fs::create_dir_all(r.join("folder.png")).unwrap();
    assert_eq!(
        ask(&r.join("folder.png")).unwrap_err(),
        "that image could not be read"
    );
    std::fs::write(r.join("fake.png"), b"GIF89a not a png").unwrap();
    assert_eq!(
        ask(&r.join("fake.png")).unwrap_err(),
        "that file is not a PNG. Pick a .png image."
    );
    std::fs::write(r.join("short.png"), &PNG[..7]).unwrap();
    assert_eq!(
        ask(&r.join("short.png")).unwrap_err(),
        "that file is not a PNG. Pick a .png image."
    );
    let mut big = PNG.to_vec();
    big.resize(2 * 1024 * 1024 + 1, 0);
    std::fs::write(r.join("big.png"), &big).unwrap();
    assert_eq!(
        ask(&r.join("big.png")).unwrap_err(),
        "that image is over 2 MB; pick a smaller PNG"
    );
    big.truncate(2 * 1024 * 1024);
    std::fs::write(r.join("limit.png"), &big).unwrap();
    assert!(ask(&r.join("limit.png")).is_ok(), "exactly 2 MB is allowed");
    // `~` is not expanded here: paths come from the picker, already absolute.
    std::fs::write(t.home().join("home.png"), PNG).unwrap();
    assert_eq!(
        block(read_image("~/home.png".to_string())).unwrap_err(),
        "home.png is not there any more -- it may have been moved or deleted."
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(r.join("locked.png"), PNG).unwrap();
        std::fs::set_permissions(r.join("locked.png"), std::fs::Permissions::from_mode(0o000))
            .unwrap();
        assert_eq!(
            ask(&r.join("locked.png")).unwrap_err(),
            "that image could not be read"
        );
    }
}

/* ---- zip readers: contents and the agent session tag -------------------- */

#[test]
fn app_contents_of_a_missing_or_damaged_file_says_so() {
    let t = TestHome::new();
    assert_eq!(
        block(app_contents(
            t.root().join("gone.krate").display().to_string()
        ))
        .unwrap_err(),
        "gone.krate is not there any more -- it may have been moved or deleted."
    );
    std::fs::write(t.root().join("junk.krate"), b"not a zip").unwrap();
    assert_eq!(
        block(app_contents(
            t.root().join("junk.krate").display().to_string()
        ))
        .unwrap_err(),
        "that file could not be read as a Krate app"
    );
    let good = t.root().join("Mön App (2).krate");
    zip_with(
        &good,
        &[
            ("manifest.toml", b"id = \"x\"\n"),
            ("source/src/lib.rs", "// ünï\n".as_bytes()),
        ],
    );
    let list = block(app_contents(good.display().to_string())).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[1]["text"], "// ünï\n");
}

#[test]
fn the_agent_session_tag_is_read_from_the_store_by_the_app_id() {
    let t = TestHome::new();
    let app = t.root().join("tip.krate");
    zip_with(
        &app,
        &[(
            "manifest.toml",
            b"[app]\nid = \"dev.krate.tip\"\nname = \"Tip\"\n",
        )],
    );
    let store = t.home().join(".krate").join("store");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(
        store.join("dev.krate.tip.agent-session"),
        "claude:abc-123\n",
    )
    .unwrap();
    assert_eq!(
        agent_session_tag(app.display().to_string()).unwrap(),
        "claude:abc-123"
    );
}

#[test]
fn the_agent_session_tag_refuses_files_that_are_not_krate_apps() {
    let t = TestHome::new();
    let r = t.root();
    assert_eq!(
        agent_session_tag(r.join("gone.krate").display().to_string()).unwrap_err(),
        "gone.krate is not there any more -- it may have been moved or deleted."
    );
    std::fs::write(r.join("junk.krate"), b"junk").unwrap();
    assert_eq!(
        agent_session_tag(r.join("junk.krate").display().to_string()).unwrap_err(),
        "that file could not be read as a Krate app"
    );
    zip_with(&r.join("nomanifest.krate"), &[("code.wasm", b"\0asm")]);
    assert_eq!(
        agent_session_tag(r.join("nomanifest.krate").display().to_string()).unwrap_err(),
        "that file could not be read as a Krate app"
    );
    zip_with(
        &r.join("noid.krate"),
        &[("manifest.toml", b"[app]\nname = \"x\"\nidentity = \"y\"\n")],
    );
    assert_eq!(
        agent_session_tag(r.join("noid.krate").display().to_string()).unwrap_err(),
        "no app id in the bundle"
    );
}

#[test]
fn an_app_with_no_stored_tag_is_said_in_plain_words() {
    let t = TestHome::new();
    let app = t.root().join("tip.krate");
    zip_with(
        &app,
        &[("manifest.toml", b"[app]\nid = \"dev.krate.untagged\"\n")],
    );
    let err = agent_session_tag(app.display().to_string()).unwrap_err();
    assert!(
        !err.contains("os error"),
        "the io crate's words reached the person: {err:?}"
    );
}

/* ---- refusals that come before anything opens --------------------------- */

#[test]
fn opening_or_revealing_a_missing_app_refuses_before_anything_opens() {
    let t = TestHome::new();
    let gone = t.root().join("gone app ü.krate").display().to_string();
    let said = "gone app ü.krate is not there any more -- it may have been moved or deleted.";
    assert_eq!(open_app(gone.clone()).unwrap_err(), said);
    assert_eq!(reveal(gone.clone()).unwrap_err(), said);
    assert_eq!(block(diagnose_app(gone.clone())).unwrap_err(), said);
    assert_eq!(
        block(publish(gone.clone(), None, None, None, None, None)).unwrap_err(),
        said
    );
    assert_eq!(block(make_card(gone.clone())).unwrap_err(), said);
    assert_eq!(
        block(make_wrap(gone.clone(), "mac".to_string())).unwrap_err(),
        said
    );
    assert_eq!(block(app_info(gone)).unwrap_err(), said);
    assert!(!t.engine_ran());
}

#[test]
fn only_https_links_open_and_only_https_apps_run_from_the_cloud() {
    let t = TestHome::new();
    for url in [
        "http://krate.tech",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "",
        " https://krate.tech",
        "/Applications/Calculator.app",
    ] {
        assert_eq!(
            open_external(url.to_string()).unwrap_err(),
            "only https links open from here",
            "{url:?}"
        );
        assert_eq!(
            block(cloud_run(url.to_string())).unwrap_err(),
            "that is not a Krate Cloud link",
            "{url:?}"
        );
    }
    assert!(!t.engine_ran());
}

#[test]
fn an_update_version_that_could_reach_a_path_is_refused_before_any_download() {
    let t = TestHome::new();
    for version in [
        "../../evil",
        "1.0/../2",
        "1.0 beta",
        "ü",
        &"1".repeat(33),
        "1.0?x=1",
    ] {
        assert_eq!(
            install_update(version.to_string()).unwrap_err(),
            "that does not look like a version",
            "{version:?}"
        );
    }
    assert!(!t.studio().join("updates").exists());
}

#[test]
fn the_staged_update_record_must_be_whole_to_be_trusted() {
    let t = TestHome::new();
    let staged = t.root().join("updates");
    std::fs::create_dir_all(&staged).unwrap();
    std::fs::write(staged.join("verified.json"), "{\"file\": ").unwrap();
    assert_eq!(
        verified_update(&staged).unwrap_err(),
        "the record of the downloaded update could not be read"
    );
    std::fs::write(staged.join("verified.json"), "{\"file\": \"x.dmg\"}").unwrap();
    assert_eq!(
        verified_update(&staged).unwrap_err(),
        "the record of the downloaded update is incomplete"
    );
    std::fs::write(
        staged.join("verified.json"),
        "{\"file\": \"x.dmg\", \"sha256\": \"00\"}",
    )
    .unwrap();
    assert_eq!(
        verified_update(&staged).unwrap_err(),
        "the downloaded update is no longer there"
    );
}

#[test]
fn autorun_is_only_what_its_variable_says() {
    let _t = TestHome::new();
    assert_eq!(autorun(), None);
    std::env::set_var("KRATE_STUDIO_AUTORUN", "");
    assert_eq!(autorun(), None);
    std::env::set_var("KRATE_STUDIO_AUTORUN", "a tip splitter ü");
    assert_eq!(autorun().as_deref(), Some("a tip splitter ü"));
}

#[test]
fn terminal_status_reports_without_changing_anything() {
    let status = terminal_status();
    if cfg!(target_os = "macos") {
        assert_eq!(status["supported"], true);
        assert_eq!(status["path"], "/usr/local/bin/krate");
        assert_eq!(status["linked"], Path::new("/usr/local/bin/krate").exists());
    } else {
        assert_eq!(
            status,
            serde_json::json!({ "supported": false, "linked": false, "path": "" })
        );
    }
}

#[test]
fn the_ui_log_line_takes_any_text() {
    // stderr only; the assertion is that no input can take the shell down.
    dbg_log(String::new());
    dbg_log("ünï 日本 🎲 \0 \r\n".to_string());
    dbg_log("x".repeat(200_000));
}

/* ---- the account and the hub, offline or against a local stub ---------- */

#[test]
fn signed_out_billing_and_account_calls_ask_to_sign_in_and_send_nothing() {
    let t = TestHome::new();
    let hub = Hub::start(|_| ok_json(serde_json::json!({ "url": "https://checkout" })));
    t.hub(&hub);
    assert_eq!(
        block(billing_checkout("pro".to_string())).unwrap_err(),
        "Sign in first - the plan needs an account to land on."
    );
    assert_eq!(block(me_info()).unwrap_err(), "Sign in first.");
    assert!(hub.seen().is_empty());
}

#[test]
fn billing_info_with_no_hub_says_billing_is_not_live() {
    let t = TestHome::new();
    t.sign_in("tok");
    assert_eq!(block(billing_info()), serde_json::json!({ "live": false }));
}

#[test]
fn billing_info_merges_the_plan_of_a_signed_in_person() {
    let t = TestHome::new();
    let hub = Hub::start(|seen| match seen.path.as_str() {
        "/billing/config" => ok_json(serde_json::json!({ "live": true, "price": "$5" })),
        "/billing/status" => ok_json(serde_json::json!({ "plan": "pro", "active": true })),
        _ => (404, b"no".to_vec()),
    });
    t.hub(&hub);
    assert_eq!(
        block(billing_info()),
        serde_json::json!({ "live": true, "price": "$5" }),
        "signed out: config only"
    );
    assert!(hub.seen().iter().all(|s| s.path != "/billing/status"));
    t.sign_in("tok-b");
    assert_eq!(
        block(billing_info()),
        serde_json::json!({ "live": true, "price": "$5", "plan": "pro", "active": true })
    );
    let status = hub.wait_for(|s| s.path == "/billing/status").unwrap();
    assert_eq!(
        status.header("authorization").as_deref(),
        Some("Bearer tok-b")
    );
}

#[test]
fn support_calls_with_no_hub_say_to_check_the_connection() {
    let _t = TestHome::new();
    let offline = "could not reach krate.tech - check your connection";
    assert_eq!(
        block(support_new("s".into(), "m".into(), "a@b.c".into())).unwrap_err(),
        offline
    );
    assert_eq!(
        block(support_list(serde_json::json!(["k"]))).unwrap_err(),
        offline
    );
    assert_eq!(
        block(support_reply("1".into(), "k".into(), "m".into())).unwrap_err(),
        offline
    );
}

#[test]
fn a_support_refusal_is_the_hubs_own_words() {
    let t = TestHome::new();
    let hub = Hub::start(|_| (400, b"subject is too long".to_vec()));
    t.hub(&hub);
    assert_eq!(
        block(support_new("x".repeat(5000), "m".into(), "a@b.c".into())).unwrap_err(),
        "subject is too long"
    );
    let sent: serde_json::Value = serde_json::from_str(&hub.seen()[0].body).unwrap();
    assert_eq!(sent["text"], "m");
}

#[test]
fn make_for_me_sends_everything_a_person_needs_to_build_it() {
    let t = TestHome::new();
    let hub = Hub::start(|seen| {
        if seen.body.contains("\"email\":\"\"") {
            (400, b"an email is needed".to_vec())
        } else {
            ok_json(serde_json::json!({ "ok": true }))
        }
    });
    t.hub(&hub);
    block(make_for_me(
        "ada@example.com".into(),
        "a tip splitter ü".into(),
        "two people".into(),
        "claude".into(),
        "the build stalled".into(),
    ))
    .unwrap();
    let sent = &hub.seen()[0];
    assert_eq!(
        (sent.method.as_str(), sent.path.as_str()),
        ("POST", "/makeit")
    );
    let body: serde_json::Value = serde_json::from_str(&sent.body).unwrap();
    assert_eq!(body["request"], "a tip splitter ü");
    assert_eq!(body["answers"], "two people");
    assert_eq!(body["why"], "the build stalled");
    assert_eq!(
        block(make_for_me(
            "".into(),
            "r".into(),
            "".into(),
            "".into(),
            "".into()
        ))
        .unwrap_err(),
        "an email is needed"
    );
    std::env::set_var("KRATE_HUB_URL", dead_url());
    assert_eq!(
        block(make_for_me(
            "a@b.c".into(),
            "r".into(),
            "".into(),
            "".into(),
            "".into()
        ))
        .unwrap_err(),
        "could not reach krate.tech - check your connection"
    );
}

#[test]
fn the_gallery_search_is_escaped_into_one_parameter() {
    let t = TestHome::new();
    let hub = Hub::start(|_| (200, b"[{\"name\":\"tip\"}]".to_vec()));
    t.hub(&hub);
    assert_eq!(
        block(cloud_apps(Some("a&b=c d/ü".into()), Some(" games ".into()))).unwrap(),
        "[{\"name\":\"tip\"}]"
    );
    assert_eq!(
        hub.seen()[0].path,
        "/apps?q=a%26b%3Dc%20d%2F%C3%BC&cat=games"
    );
    block(cloud_apps(Some("   ".into()), None)).unwrap();
    assert_eq!(hub.seen()[1].path, "/apps", "blank search is no search");
    block(cloud_apps(None, Some("tools".into()))).unwrap();
    assert_eq!(hub.seen()[2].path, "/apps?cat=tools");
}

#[test]
fn the_gallery_offline_says_so_in_plain_words() {
    let _t = TestHome::new();
    assert_eq!(
        block(cloud_apps(None, None)).unwrap_err(),
        "Krate Cloud could not be reached. Check your connection."
    );
}

#[test]
fn a_sign_in_email_must_look_like_one_and_is_never_sent_otherwise() {
    let t = TestHome::new();
    let hub = Hub::start(|_| ok_json(serde_json::json!({})));
    t.hub(&hub);
    for email in [
        "",
        "   ",
        "not-an-email",
        &format!("{}@x.io", "a".repeat(250)),
    ] {
        assert_eq!(
            block(login_email(email.to_string())).unwrap_err(),
            "That doesn't look like an email yet.",
            "{email:?}"
        );
    }
    assert!(hub.seen().is_empty());
    assert!(
        !t.studio().join(PENDING_SIGN_IN).exists(),
        "a refused email starts no sign-in"
    );
}

#[test]
fn a_sign_in_email_with_no_hub_says_so_and_the_started_sign_in_is_kept() {
    let t = TestHome::new();
    assert_eq!(
        block(login_email(" ada@example.com ".to_string())).unwrap_err(),
        "Krate could not be reached. Check your connection."
    );
    assert_eq!(pending_sign_ins(&t.studio()).len(), 1);
}

/* ---- the free count ----------------------------------------------------- */

#[test]
fn the_free_count_starts_at_zero_and_counts_each_make_on_this_machine() {
    let t = TestHome::new();
    let first = plan_makes(None, None);
    assert_eq!(first["n"], 0);
    assert_eq!(first["month"], month_key_now());
    assert_eq!(plan_count_make()["n"], 1);
    assert_eq!(plan_count_make()["n"], 2);
    assert_eq!(plan_makes(None, None)["n"], 2);
    let stored: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(t.home().join(".krate").join("plan.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(stored["n"], 2);
    assert_eq!(stored["device"], device_hash());
}

#[test]
fn a_count_carried_over_from_the_browser_is_never_lost() {
    let _t = TestHome::new();
    assert_eq!(plan_makes(Some("2026-01".into()), Some(3))["n"], 3);
    assert_eq!(plan_makes(None, Some(1))["n"], 3, "the larger wins");
}

#[test]
fn a_count_written_by_another_machine_does_not_carry_over() {
    let t = TestHome::new();
    std::fs::create_dir_all(t.home().join(".krate")).unwrap();
    std::fs::write(
        t.home().join(".krate").join("plan.json"),
        "{\"device\":\"someone-else\",\"month\":\"2026-01\",\"n\":5}",
    )
    .unwrap();
    assert_eq!(plan_makes(None, None)["n"], 0);
}

#[test]
fn a_corrupted_count_falls_back_to_what_the_hub_remembers() {
    let t = TestHome::new();
    std::fs::create_dir_all(t.home().join(".krate")).unwrap();
    std::fs::write(t.home().join(".krate").join("plan.json"), "{{{").unwrap();
    assert_eq!(plan_makes(None, None)["n"], 0, "offline, nothing to go on");
    if device_hash().is_empty() {
        eprintln!("no device id on this machine; the hub half is not asked");
        return;
    }
    std::fs::write(t.home().join(".krate").join("plan.json"), "{{{").unwrap();
    let hub = Hub::start(|seen| {
        if seen.path == "/plan/get" {
            ok_json(serde_json::json!({ "n": 2, "machine": 1 }))
        } else {
            (404, Vec::new())
        }
    });
    t.hub(&hub);
    let answer = plan_makes(None, None);
    assert_eq!(answer["n"], 2);
    assert_eq!(answer["machine"], 1);
}

/* ---- the build slot (build_alive / stop_build) -------------------------- */

#[test]
fn nothing_running_is_not_alive_and_stop_is_fine() {
    let running = Running::fresh();
    assert_eq!(build_alive_in(&running), Ok(false));
    assert_eq!(stop_build_in(&running), Ok(()));
    running.1.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(build_alive_in(&running), Ok(true), "the waiter's flag");
}

#[cfg(unix)]
#[test]
fn a_dead_build_left_in_the_slot_is_cleared() {
    let mut child = Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    let running = Running::fresh();
    *running.0.lock().unwrap() = Some(pid);
    assert_eq!(build_alive_in(&running), Ok(false));
    assert_eq!(*running.0.lock().unwrap(), None, "the ghost is cleared");
}

#[cfg(unix)]
#[test]
fn stop_ends_a_live_build_and_everything_under_it() {
    use std::os::unix::process::CommandExt;
    let dir = tempfile::tempdir().unwrap();
    let grandchild_file = dir.path().join("grandchild");
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "sleep 30 & echo $! > '{}'; wait",
            grandchild_file.display()
        ))
        .process_group(0)
        .spawn()
        .unwrap();
    let pid = child.id();
    let running = Running::fresh();
    *running.0.lock().unwrap() = Some(pid);
    // Wait for the grandchild to exist.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let grandchild: u32 = loop {
        if let Some(n) = std::fs::read_to_string(&grandchild_file)
            .ok()
            .and_then(|t| t.trim().parse().ok())
        {
            break n;
        }
        assert!(std::time::Instant::now() < deadline, "no grandchild");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(build_alive_in(&running), Ok(true));
    stop_build_in(&running).unwrap();
    assert_eq!(*running.0.lock().unwrap(), None);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the build kept running"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while pid_alive(grandchild) {
        assert!(
            std::time::Instant::now() < deadline,
            "the agent under the build kept running (pid {grandchild})"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/* ---- IDE commands that need no window ----------------------------------- */

#[test]
fn ide_projects_lists_only_real_projects_in_the_apps_folder_and_the_recent_list() {
    let t = TestHome::new();
    let out = t.home().join("Krate Apps ü");
    settings_set(Settings {
        out_dir: out.display().to_string(),
        agent: "claude".into(),
    })
    .unwrap();
    let root = out.join("Projects");
    std::fs::create_dir_all(root.join("half")).unwrap();
    std::fs::write(root.join("half").join("Cargo.toml"), "").unwrap();
    std::fs::write(root.join("loose.txt"), "x").unwrap();
    let alpha = make_project(&root, "alpha");
    let elsewhere = make_project(&t.root(), "elsewhere ü");
    let gone = t.root().join("gone");
    std::fs::write(
        t.studio().join("ide-recent.json"),
        serde_json::json!([
            { "path": elsewhere.display().to_string(), "at": 5 },
            { "path": gone.display().to_string(), "at": 9 },
        ])
        .to_string(),
    )
    .unwrap();
    let mut paths: Vec<String> = block(ide::ide_projects())
        .unwrap()
        .into_iter()
        .map(|p| {
            serde_json::to_value(p).unwrap()["path"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    paths.sort();
    let mut want = vec![alpha.display().to_string(), elsewhere.display().to_string()];
    want.sort();
    assert_eq!(paths, want);
}

#[test]
fn ide_projects_on_a_fresh_home_or_with_a_corrupted_recent_list_still_answers() {
    let t = TestHome::new();
    assert!(block(ide::ide_projects()).unwrap().is_empty());
    let alpha = make_project(&t.home().join("Krate Apps").join("Projects"), "alpha");
    std::fs::write(t.studio().join("ide-recent.json"), "[{\"path\": ").unwrap();
    let list = block(ide::ide_projects()).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(
        serde_json::to_value(&list[0]).unwrap()["path"],
        alpha.display().to_string()
    );
}

#[test]
fn ide_commands_refuse_a_folder_that_is_not_an_open_project() {
    let t = TestHome::new();
    let plain = t.root().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let file = t.root().join("file.txt");
    std::fs::write(&file, "x").unwrap();
    let p = |path: &Path| path.display().to_string();
    let tree = |path: String| block(ide::ide_tree(path)).unwrap_err();
    assert_eq!(tree(String::new()), "no project is open");
    assert_eq!(tree("   ".into()), "no project is open");
    assert_eq!(
        tree(p(&t.root().join("nope"))),
        "nope is not there any more -- it may have been moved or deleted."
    );
    assert_eq!(tree(p(&file)), "file.txt is not a folder");
    assert_eq!(
        tree(p(&plain)),
        "plain is not a Krate project: it needs Cargo.toml and manifest.toml side by side."
    );
    let not_project =
        "plain is not a Krate project: it needs Cargo.toml and manifest.toml side by side.";
    assert_eq!(
        block(ide::ide_read(p(&plain), "x".into())).unwrap_err(),
        not_project
    );
    assert_eq!(
        block(ide::ide_write(p(&plain), "x".into(), "y".into())).unwrap_err(),
        not_project
    );
    assert!(!plain.join("x").exists());
    assert_eq!(
        block(ide::ide_rename(p(&plain), "a".into(), "b".into())).unwrap_err(),
        not_project
    );
    assert_eq!(
        block(ide::ide_delete(p(&plain), "a".into())).unwrap_err(),
        not_project
    );
    assert_eq!(
        block(ide::ide_apply(p(&plain), Vec::new())).unwrap_err(),
        not_project
    );
    assert_eq!(
        block(ide::ide_explain(p(&plain), "why".into(), None)).unwrap_err(),
        not_project
    );
}

#[test]
fn ide_tree_opens_a_tilde_path_with_spaces_and_unicode() {
    let t = TestHome::new();
    let proj = make_project(&t.home().join("Krate Apps"), "mön app");
    for typed in ["~/Krate Apps/mön app", "  ~/Krate Apps/mön app/  "] {
        let tree = block(ide::ide_tree(typed.to_string())).unwrap();
        let rels: Vec<String> = tree
            .iter()
            .map(|e| {
                serde_json::to_value(e).unwrap()["rel"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert!(
            rels.contains(&"src/lib.rs".to_string()),
            "{typed:?}: {rels:?}"
        );
    }
    assert_eq!(
        block(ide::ide_read(
            "~/Krate Apps/mön app".into(),
            "src/lib.rs".into()
        ))
        .unwrap(),
        "// hi\n"
    );
    assert!(proj.is_dir());
}

#[test]
fn ide_read_reads_inside_and_refuses_everything_else() {
    let t = TestHome::new();
    let proj = make_project(&t.root(), "proj");
    std::fs::write(t.root().join("secret.txt"), "secret").unwrap();
    std::fs::write(proj.join("src").join("naïve 日本.rs"), "// ü\n").unwrap();
    let path = proj.display().to_string();
    let read = |rel: &str| block(ide::ide_read(path.clone(), rel.to_string()));
    assert_eq!(read("src/naïve 日本.rs").unwrap(), "// ü\n");
    assert_eq!(read("./src//lib.rs").unwrap(), "// hi\n");
    assert_eq!(read("src").unwrap_err(), "src is a folder");
    assert_eq!(read("src/gone.rs").unwrap_err(), "src/gone.rs is not there");
    assert_eq!(read("").unwrap_err(), "no file named");
    for out in [
        "../secret.txt",
        "src/../../secret.txt",
        &t.root().join("secret.txt").display().to_string(),
        "C:\\secret.txt",
    ] {
        assert_eq!(
            read(out).unwrap_err(),
            "that file is outside the project",
            "{out}"
        );
    }
}

#[test]
fn ide_write_makes_folders_inside_and_never_writes_outside() {
    let t = TestHome::new();
    let proj = make_project(&t.root(), "proj");
    std::fs::write(t.root().join("secret.txt"), "secret").unwrap();
    let path = proj.display().to_string();
    let write =
        |rel: &str, text: &str| block(ide::ide_write(path.clone(), rel.into(), text.into()));
    write("src/deep/new ü.rs", "fn x() {}\n").unwrap();
    assert_eq!(
        std::fs::read_to_string(proj.join("src/deep/new ü.rs")).unwrap(),
        "fn x() {}\n"
    );
    write("src/lib.rs", "").unwrap();
    assert_eq!(
        std::fs::read_to_string(proj.join("src/lib.rs")).unwrap(),
        ""
    );
    assert_eq!(
        write("../secret.txt", "x").unwrap_err(),
        "that file is outside the project"
    );
    assert_eq!(write("", "x").unwrap_err(), "no file named");
    assert_eq!(write("src", "x").unwrap_err(), "src is a folder");
    assert_eq!(
        std::fs::read_to_string(t.root().join("secret.txt")).unwrap(),
        "secret"
    );
    // A large file is saved whole; the editor then declines to open it.
    let big = "a".repeat(3 * 1024 * 1024);
    write("big.txt", &big).unwrap();
    assert_eq!(
        std::fs::metadata(proj.join("big.txt")).unwrap().len(),
        big.len() as u64
    );
    assert_eq!(
        block(ide::ide_read(path.clone(), "big.txt".into())).unwrap_err(),
        "big.txt is 3.0 MB -- too big to open here (the limit is 1 MB)."
    );
}

#[cfg(unix)]
#[test]
fn ide_write_into_a_read_only_folder_says_so_and_leaves_the_file() {
    use std::os::unix::fs::PermissionsExt;
    let t = TestHome::new();
    let proj = make_project(&t.root(), "proj");
    std::fs::set_permissions(proj.join("src"), std::fs::Permissions::from_mode(0o555)).unwrap();
    let err = block(ide::ide_write(
        proj.display().to_string(),
        "src/lib.rs".into(),
        "changed".into(),
    ))
    .unwrap_err();
    std::fs::set_permissions(proj.join("src"), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(err.starts_with("could not save lib.rs: "), "{err}");
    assert_eq!(
        std::fs::read_to_string(proj.join("src/lib.rs")).unwrap(),
        "// hi\n"
    );
    let names: Vec<String> = std::fs::read_dir(proj.join("src"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, vec!["lib.rs"], "no staging litter");
}

#[test]
fn ide_rename_moves_inside_and_never_over_or_out() {
    let t = TestHome::new();
    let proj = make_project(&t.root(), "proj");
    std::fs::write(proj.join("src/extra.rs"), "// x\n").unwrap();
    let path = proj.display().to_string();
    let rename =
        |from: &str, to: &str| block(ide::ide_rename(path.clone(), from.into(), to.into()));
    rename("src/extra.rs", "src/more/hëlpers.rs").unwrap();
    assert_eq!(
        std::fs::read_to_string(proj.join("src/more/hëlpers.rs")).unwrap(),
        "// x\n"
    );
    assert!(!proj.join("src/extra.rs").exists());
    assert_eq!(
        rename("src/more/hëlpers.rs", "src/lib.rs").unwrap_err(),
        "src/lib.rs already exists"
    );
    assert_eq!(
        rename("src/more/hëlpers.rs", "src").unwrap_err(),
        "src already exists"
    );
    assert_eq!(
        rename("src/gone.rs", "src/x.rs").unwrap_err(),
        "src/gone.rs is not there"
    );
    assert_eq!(
        rename("src/more/hëlpers.rs", "../out.rs").unwrap_err(),
        "that file is outside the project"
    );
    assert_eq!(
        rename("src/more/hëlpers.rs", "").unwrap_err(),
        "no file named"
    );
    assert!(!t.root().join("out.rs").exists());
    assert_eq!(
        std::fs::read_to_string(proj.join("src/lib.rs")).unwrap(),
        "// hi\n"
    );
}

#[test]
fn ide_rename_keeps_the_files_that_make_it_a_project() {
    let t = TestHome::new();
    let proj = make_project(&t.root(), "proj");
    let path = proj.display().to_string();
    let err = block(ide::ide_rename(
        path.clone(),
        "Cargo.toml".into(),
        "Cargo.old".into(),
    ));
    assert!(
        err.is_err(),
        "renaming Cargo.toml away must be refused like deleting it"
    );
    assert!(proj.join("Cargo.toml").is_file());
    assert!(block(ide::ide_tree(path)).is_ok(), "still a project");
}

#[test]
fn ide_delete_moves_the_file_to_studios_backups_and_keeps_the_project_files() {
    let t = TestHome::new();
    let proj = make_project(&t.root(), "proj");
    std::fs::write(proj.join("src/old ü.rs"), "// old\n").unwrap();
    let path = proj.display().to_string();
    let delete = |rel: &str| block(ide::ide_delete(path.clone(), rel.into()));
    delete("src/old ü.rs").unwrap();
    assert!(!proj.join("src/old ü.rs").exists());
    let kept = walk(&t.studio().join("ide-backups"));
    assert!(
        kept.iter().any(|f| f.ends_with("src/old ü.rs")),
        "kept: {kept:?}"
    );
    for keep in ["Cargo.toml", "manifest.toml", "./Cargo.toml"] {
        let err = delete(keep).unwrap_err();
        assert!(
            err.ends_with("is what makes this a Krate project; it stays"),
            "{err}"
        );
    }
    assert_eq!(
        delete("src/gone.rs").unwrap_err(),
        "src/gone.rs is not there"
    );
    assert_eq!(
        delete("../x").unwrap_err(),
        "that file is outside the project"
    );
    assert_eq!(delete("").unwrap_err(), "no file named");
    std::fs::create_dir_all(proj.join("assets")).unwrap();
    std::fs::write(proj.join("assets/x.png"), PNG).unwrap();
    delete("assets").unwrap();
    assert!(!proj.join("assets").exists(), "a folder can go too");
    assert!(proj.join("Cargo.toml").is_file() && proj.join("manifest.toml").is_file());
}

/// True on a filesystem that treats `A` and `a` as one name (macOS and
/// Windows defaults).
fn case_insensitive(dir: &Path) -> bool {
    let probe = dir.join("CaseProbe");
    std::fs::write(&probe, "").unwrap();
    let same = dir.join("caseprobe").exists();
    let _ = std::fs::remove_file(probe);
    same
}

#[test]
fn ide_delete_keeps_cargo_toml_whatever_case_it_is_typed_in() {
    let t = TestHome::new();
    if !case_insensitive(&t.root()) {
        eprintln!("case-sensitive disk: the hazard does not exist here");
        return;
    }
    let proj = make_project(&t.root(), "proj");
    let path = proj.display().to_string();
    for rel in ["CARGO.TOML", "Manifest.toml"] {
        let _ = block(ide::ide_delete(path.clone(), rel.into()));
    }
    assert!(
        proj.join("Cargo.toml").is_file(),
        "Cargo.toml was moved away"
    );
    assert!(
        proj.join("manifest.toml").is_file(),
        "manifest.toml was moved away"
    );
}

/// Two calls of `f` that land inside one wall-clock second, so a backup
/// folder named by the second is shared. Retries if a second boundary falls
/// between them.
fn within_one_second(mut f: impl FnMut(u32)) {
    for _ in 0..5 {
        let before = unix_now();
        f(0);
        f(1);
        if unix_now() == before {
            return;
        }
    }
    panic!("could not fit two calls in one second");
}

#[test]
fn deleting_the_same_file_twice_keeps_both_copies() {
    let t = TestHome::new();
    let proj = make_project(&t.root(), "proj");
    let path = proj.display().to_string();
    within_one_second(|i| {
        std::fs::write(proj.join("src/a.rs"), format!("version {i}\n")).unwrap();
        block(ide::ide_delete(path.clone(), "src/a.rs".into())).unwrap();
    });
    let kept: Vec<String> = walk(&t.studio().join("ide-backups"))
        .iter()
        .map(|f| std::fs::read_to_string(f).unwrap())
        .collect();
    assert!(kept.contains(&"version 0\n".to_string()), "lost: {kept:?}");
    assert!(kept.contains(&"version 1\n".to_string()), "lost: {kept:?}");
}

#[test]
fn deleting_a_file_and_then_its_folder_in_one_second_works() {
    let t = TestHome::new();
    let proj = make_project(&t.root(), "proj");
    let path = proj.display().to_string();
    std::fs::write(proj.join("src/a.rs"), "// a\n").unwrap();
    let before = unix_now();
    block(ide::ide_delete(path.clone(), "src/a.rs".into())).unwrap();
    let second = block(ide::ide_delete(path, "src".into()));
    if unix_now() != before {
        eprintln!("a second boundary fell between the two deletes; nothing shown");
        return;
    }
    assert_eq!(second, Ok(()));
    assert!(!proj.join("src").exists());
}

#[test]
fn ide_apply_writes_text_and_bytes_and_keeps_what_was_there() {
    let t = TestHome::new();
    let proj = make_project(&t.root(), "proj");
    let path = proj.display().to_string();
    let files: Vec<ide::Accepted> = serde_json::from_value(serde_json::json!([
        { "rel": "src/lib.rs", "text": "// changed ü\n" },
        { "rel": "assets/blob.bin", "b64": base64::engine::general_purpose::STANDARD.encode([0u8, 159, 146, 150]) },
    ]))
    .unwrap();
    let changed = block(ide::ide_apply(path.clone(), files)).unwrap();
    assert_eq!(changed, vec!["assets/blob.bin", "src/lib.rs"]);
    assert_eq!(
        std::fs::read_to_string(proj.join("src/lib.rs")).unwrap(),
        "// changed ü\n"
    );
    assert_eq!(
        std::fs::read(proj.join("assets/blob.bin")).unwrap(),
        [0u8, 159, 146, 150]
    );
    let kept = walk(&t.studio().join("ide-backups"));
    let backup = kept
        .iter()
        .find(|f| f.ends_with("src/lib.rs"))
        .expect("backup");
    assert_eq!(std::fs::read_to_string(backup).unwrap(), "// hi\n");
    assert_eq!(
        block(ide::ide_apply(path, Vec::new())).unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn ide_apply_checks_every_file_before_writing_any() {
    let t = TestHome::new();
    let proj = make_project(&t.root(), "proj");
    let path = proj.display().to_string();
    let apply = |files: serde_json::Value| {
        block(ide::ide_apply(
            path.clone(),
            serde_json::from_value(files).unwrap(),
        ))
    };
    assert_eq!(
        apply(serde_json::json!([
            { "rel": "src/lib.rs", "text": "// changed\n" },
            { "rel": "../evil.rs", "text": "evil" },
        ]))
        .unwrap_err(),
        "../evil.rs: that file is outside the project; nothing was written"
    );
    assert_eq!(
        apply(serde_json::json!([
            { "rel": "src/lib.rs", "text": "// changed\n" },
            { "rel": "src/x.bin", "b64": "!!!" },
        ]))
        .unwrap_err(),
        "src/x.bin did not arrive whole; nothing was written"
    );
    assert_eq!(
        apply(serde_json::json!([{ "rel": "src/lib.rs" }])).unwrap_err(),
        "src/lib.rs has no contents; nothing was written"
    );
    assert_eq!(
        std::fs::read_to_string(proj.join("src/lib.rs")).unwrap(),
        "// hi\n"
    );
    assert!(!t.root().join("evil.rs").exists());
    assert!(!proj.join("src/x.bin").exists());
}

#[test]
fn two_applies_in_one_second_keep_the_original_in_the_backups() {
    let t = TestHome::new();
    let proj = make_project(&t.root(), "proj");
    let path = proj.display().to_string();
    within_one_second(|i| {
        let files = serde_json::from_value(serde_json::json!([
            { "rel": "src/lib.rs", "text": format!("// change {i}\n") }
        ]))
        .unwrap();
        block(ide::ide_apply(path.clone(), files)).unwrap();
    });
    let kept: Vec<String> = walk(&t.studio().join("ide-backups"))
        .iter()
        .map(|f| std::fs::read_to_string(f).unwrap())
        .collect();
    assert!(
        kept.contains(&"// hi\n".to_string()),
        "the original is gone from the backups: {kept:?}"
    );
}

#[test]
fn ide_stop_with_nothing_running_is_fine() {
    let t = TestHome::new();
    let proj = make_project(&t.root(), "proj");
    assert_eq!(ide::ide_stop(proj.display().to_string()), Ok(()));
    assert_eq!(
        ide::ide_stop(t.root().join("gone").display().to_string()),
        Ok(())
    );
    assert_eq!(ide::ide_stop(String::new()), Ok(()));
}

pub(crate) fn walk(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(read) = std::fs::read_dir(dir) {
        for e in read.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out
}

/* ---- commands that run the engine, against a stub engine ---------------- */

#[cfg(unix)]
mod with_engine {
    use super::*;

    #[test]
    fn with_no_engine_every_engine_command_names_the_missing_engine() {
        let t = TestHome::new();
        let missing = t.missing_engine();
        let app = t.root().join("app.krate");
        std::fs::write(&app, "x").unwrap();
        let app = app.display().to_string();
        let proj = make_project(&t.root(), "proj").display().to_string();
        assert_eq!(block(account_status()).unwrap_err(), missing);
        assert_eq!(block(account_logout()).unwrap_err(), missing);
        assert_eq!(block(agents()).err(), Some(missing.clone()));
        assert_eq!(block(refresh_agents()).err(), Some(missing.clone()));
        assert_eq!(block(api_keys()).err(), Some(missing.clone()));
        assert_eq!(
            block(api_key_set("anthropic".into(), "k".into())).unwrap_err(),
            missing
        );
        assert_eq!(
            block(api_key_forget("anthropic".into())).unwrap_err(),
            missing
        );
        assert_eq!(block(port_plan("/x".into())).unwrap_err(), missing);
        assert_eq!(block(report_collect("1".into())).unwrap_err(), missing);
        assert_eq!(block(diagnose_app(app.clone())).unwrap_err(), missing);
        assert_eq!(
            block(publish(app.clone(), None, None, None, None, None)).unwrap_err(),
            missing
        );
        assert_eq!(block(app_info(app.clone())).unwrap_err(), missing);
        assert_eq!(block(make_card(app.clone())).unwrap_err(), missing);
        assert_eq!(block(make_wrap(app, "linux".into())).unwrap_err(), missing);
        assert_eq!(block(sign_in_agent("claude".into())).unwrap_err(), missing);
        assert_eq!(
            plan_request_blocking("a timer".into(), Vec::new(), None).unwrap_err(),
            missing
        );
        assert_eq!(block(ide::ide_sdk()).unwrap_err(), missing);
        assert_eq!(
            block(ide::ide_explain(proj, "why".into(), None)).unwrap_err(),
            missing
        );
    }

    /* account */

    #[test]
    fn account_status_is_the_engines_json() {
        let t = TestHome::new();
        t.engine(r#"echo '{"signed_in":true,"login":"ada"}'"#);
        assert_eq!(
            block(account_status()).unwrap(),
            serde_json::json!({ "signed_in": true, "login": "ada" })
        );
        assert_eq!(t.args(), vec!["account", "--json"]);
    }

    #[test]
    fn account_status_without_json_is_the_engines_reason() {
        let t = TestHome::new();
        t.engine("echo 'the account file is damaged' >&2; exit 1");
        assert_eq!(
            block(account_status()).unwrap_err(),
            "the account file is damaged"
        );
    }

    #[test]
    fn account_logout_runs_the_engines_logout() {
        let t = TestHome::new();
        t.engine("exit 0");
        block(account_logout()).unwrap();
        assert_eq!(t.args(), vec!["account", "logout"]);
    }

    #[test]
    fn account_logout_reports_an_engine_that_could_not_sign_out() {
        let t = TestHome::new();
        t.engine("echo 'error: could not remove the sign-in' >&2; exit 1");
        assert!(
            block(account_logout()).is_err(),
            "the person was told they are signed out while still signed in"
        );
    }

    /* agents */

    #[test]
    fn agents_are_the_engines_list_with_the_gaps_filled() {
        let t = TestHome::new();
        t.engine(
            r#"echo '[{"name":"claude","label":"Claude","state":"ready","detail":"ok","remedy":"none"},{"name":"x"}]'"#,
        );
        let list = block(agents()).unwrap();
        assert_eq!(t.args(), vec!["ai", "--json"]);
        let v = serde_json::to_value(&list).unwrap();
        assert_eq!(v[0]["state"], "ready");
        assert_eq!(v[0]["remedy"], "none");
        assert_eq!(v[1]["name"], "x");
        assert_eq!(v[1]["state"], "missing", "no state reads as missing");
        assert_eq!(v[1]["label"], "");
        assert_eq!(v[1]["remedy"], serde_json::Value::Null);
    }

    #[test]
    fn agents_pass_on_a_failure_and_name_a_list_that_does_not_parse() {
        let t = TestHome::new();
        t.engine("echo 'error: unexpected argument' >&2; exit 2");
        assert_eq!(block(agents()).err().unwrap(), "error: unexpected argument");
        t.engine("echo 'hello'");
        assert!(block(agents())
            .err()
            .unwrap()
            .starts_with("the engine's agent list did not parse: "));
    }

    #[test]
    fn refreshing_agents_forgets_only_the_readiness_answers() {
        let t = TestHome::new();
        let cache = t.home().join(".krate").join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        for name in ["ai-probe-claude.json", "ai-probe-codex.txt", "other.json"] {
            std::fs::write(cache.join(name), "{}").unwrap();
        }
        t.engine(r#"echo '[{"name":"claude","state":"ready"}]'"#);
        let list = block(refresh_agents()).unwrap();
        assert_eq!(list.len(), 1);
        assert!(!cache.join("ai-probe-claude.json").exists());
        assert!(cache.join("ai-probe-codex.txt").exists());
        assert!(cache.join("other.json").exists());
    }

    /* api keys */

    #[test]
    fn api_keys_say_where_each_key_is_kept_and_never_carry_the_key() {
        let t = TestHome::new();
        t.engine(
            "printf 'anthropic  set, in the keychain (...abcd)\\nopenai     set, from the environment (OPENAI_API_KEY) (...wxyz)\\n'",
        );
        let infos = block(api_keys()).unwrap();
        let v = serde_json::to_value(&infos).unwrap();
        assert_eq!(t.args(), vec!["api-key", "status"]);
        assert_eq!(v[0]["vendor"], "anthropic");
        assert_eq!(v[0]["set"], true);
        assert_eq!(v[0]["where_kept"], "in your keychain");
        assert_eq!(v[0]["from_env"], false);
        assert_eq!(v[1]["where_kept"], "from the environment");
        assert_eq!(v[1]["from_env"], true);
        let text = v.to_string();
        assert!(!text.contains("abcd") && !text.contains("wxyz"), "{text}");

        t.engine("printf 'anthropic  set, encrypted (...abcd)\\nopenai     not set\\n'");
        let v = serde_json::to_value(block(api_keys()).unwrap()).unwrap();
        assert_eq!(v[0]["where_kept"], "encrypted on this machine");
        assert_eq!(v[1]["set"], false);
        assert_eq!(v[1]["where_kept"], "");

        t.engine("exit 3");
        let v = serde_json::to_value(block(api_keys()).unwrap()).unwrap();
        assert_eq!(
            v[0]["set"], false,
            "an engine with nothing to say is not set"
        );
    }

    #[test]
    fn an_api_key_goes_in_on_stdin_trimmed_and_never_as_an_argument() {
        let t = TestHome::new();
        let stdin = t.root().join("stdin");
        t.engine(&format!(
            "cat > '{}'; echo 'Anthropic key saved in the keychain.'",
            stdin.display()
        ));
        assert_eq!(
            block(api_key_set(
                "anthropic".into(),
                "  sk-ant-test-123  \n".into()
            ))
            .unwrap(),
            "Anthropic key saved in the keychain."
        );
        assert_eq!(t.args(), vec!["api-key", "set", "anthropic"]);
        assert_eq!(std::fs::read_to_string(stdin).unwrap(), "sk-ant-test-123\n");
        assert!(t.calls().iter().all(|c| !c.contains("sk-ant")));
    }

    #[test]
    fn an_api_key_refusal_is_the_engines_words_without_its_prefix() {
        let t = TestHome::new();
        t.engine(
            "cat > /dev/null; echo 'error: that does not look like an Anthropic key' >&2; exit 1",
        );
        assert_eq!(
            block(api_key_set("anthropic".into(), "nope".into())).unwrap_err(),
            "that does not look like an Anthropic key"
        );
    }

    #[test]
    fn forgetting_an_api_key_runs_forget_and_passes_on_a_failure() {
        let t = TestHome::new();
        t.engine("exit 0");
        block(api_key_forget("openai".into())).unwrap();
        assert_eq!(t.args(), vec!["api-key", "forget", "openai"]);
        t.engine("echo 'could not reach the keychain' >&2; exit 1");
        assert_eq!(
            block(api_key_forget("openai".into())).unwrap_err(),
            "could not reach the keychain"
        );
    }

    /* porting */

    #[test]
    fn the_port_plan_comes_back_even_when_the_engine_refuses_the_port() {
        let t = TestHome::new();
        t.engine(r#"echo '{"schema":"krate.port.plan.v1","verdict":"unsupported"}'; exit 1"#);
        let source = "-rf my folder ü";
        let plan = block(port_plan(source.into())).unwrap();
        assert!(plan.contains("\"unsupported\""));
        assert_eq!(
            t.args(),
            vec!["port", "--format", "json", "--", source],
            "the folder goes after --, so a name starting with - is a name"
        );
    }

    #[test]
    fn a_port_plan_with_no_plan_is_the_last_thing_the_engine_said() {
        let t = TestHome::new();
        t.engine("echo 'reading'; printf 'step\\nerror: no such folder\\n\\n' >&2; exit 2");
        assert_eq!(
            block(port_plan("/gone".into())).unwrap_err(),
            "error: no such folder"
        );
        t.engine("exit 2");
        assert_eq!(
            block(port_plan("/gone".into())).unwrap_err(),
            "the engine could not read that folder"
        );
    }

    /* plan_request (its engine half) */

    const PLAN_ENGINE: &str = r#"if [ "$1" = plan ] && [ "$2" = --help ]; then exit 0; fi
echo '{"kind":"ask","questions":["how many people?"]}'"#;

    #[test]
    fn the_plan_request_goes_after_the_double_dash_with_its_attachments() {
        let t = TestHome::new();
        t.engine(PLAN_ENGINE);
        let answer = plan_request_blocking(
            "- a bullet list ü".into(),
            vec!["/a b/notes.txt".into()],
            Some(String::new()),
        )
        .unwrap();
        assert_eq!(answer, r#"{"kind":"ask","questions":["how many people?"]}"#);
        assert_eq!(
            t.args(),
            vec![
                "plan",
                "--attach",
                "/a b/notes.txt",
                "--",
                "- a bullet list ü"
            ],
            "an empty agent is not passed"
        );
        plan_request_blocking("a timer".into(), Vec::new(), Some("codex".into())).unwrap();
        assert_eq!(t.args(), vec!["plan", "--agent", "codex", "--", "a timer"]);
    }

    #[test]
    fn a_plan_that_fails_or_says_nothing_is_an_error() {
        let t = TestHome::new();
        t.engine(&format!(
            "{}\n",
            r#"if [ "$1" = plan ] && [ "$2" = --help ]; then exit 0; fi
echo 'error: the AI is not signed in' >&2; exit 1"#
        ));
        assert_eq!(
            plan_request_blocking("x".into(), Vec::new(), None).unwrap_err(),
            "error: the AI is not signed in"
        );
        t.engine(
            r#"if [ "$1" = plan ] && [ "$2" = --help ]; then exit 0; fi
echo '   '"#,
        );
        assert_eq!(
            plan_request_blocking("x".into(), Vec::new(), None).unwrap_err(),
            "the plan step failed"
        );
    }

    /* support report */

    #[test]
    fn the_support_report_lists_what_is_inside_before_anything_is_sent() {
        let t = TestHome::new();
        let zip = t.root().join("report ü.zip");
        zip_with(&zip, &[("session.json", b"{}"), ("trace.jsonl", b"")]);
        t.engine(&format!("echo '{}'", zip.display()));
        let v = block(report_collect("1712-abc".into())).unwrap();
        assert_eq!(t.args(), vec!["support-report", "1712-abc"]);
        assert_eq!(v["path"], zip.display().to_string());
        assert_eq!(v["size"], std::fs::metadata(&zip).unwrap().len());
        assert_eq!(
            v["files"],
            serde_json::json!(["session.json", "trace.jsonl"])
        );
    }

    #[test]
    fn a_support_report_that_cannot_be_gathered_says_why() {
        let t = TestHome::new();
        t.engine("exit 1");
        assert_eq!(
            block(report_collect("1".into())).unwrap_err(),
            "could not gather this session"
        );
        t.engine("echo 'no such session' >&2; exit 1");
        assert_eq!(
            block(report_collect("1".into())).unwrap_err(),
            "no such session"
        );
    }

    /* diagnose, publish, details */

    #[test]
    fn diagnosing_an_app_runs_it_for_a_screenshot_and_never_grants_it_anything() {
        let t = TestHome::new();
        let app = t.root().join("my app ü.krate");
        std::fs::write(&app, "x").unwrap();
        t.engine("exit 0");
        assert_eq!(
            block(diagnose_app(app.display().to_string())).unwrap(),
            "ok"
        );
        let args = t.args();
        assert_eq!(args[..2], ["run".to_string(), app.display().to_string()]);
        assert!(args.contains(&"--for-screenshot".to_string()), "{args:?}");
        assert!(!args.iter().any(|a| a == "--auto-grant"), "{args:?}");
        assert_eq!(
            args[args.len() - 2..],
            ["--".to_string(), "quick".to_string()]
        );
    }

    #[test]
    fn a_failed_diagnosis_is_the_last_six_lines_the_runtime_said() {
        let t = TestHome::new();
        let app = t.root().join("app.krate");
        std::fs::write(&app, "x").unwrap();
        t.engine(
            "for i in 1 2 3 4 5; do echo out$i; done; echo; printf 'err1\\nerr2\\n' >&2; exit 1",
        );
        assert_eq!(
            block(diagnose_app(app.display().to_string())).unwrap(),
            "out2\nout3\nout4\nout5\nerr1\nerr2"
        );
    }

    #[test]
    fn publishing_hands_back_the_link_and_sends_only_fields_with_words() {
        let t = TestHome::new();
        let app = t.root().join("tip.krate");
        std::fs::write(&app, "x").unwrap();
        t.engine("echo 'Published.'; echo '  run it: https://krate.tech/r/abc123  '");
        let link = block(publish(
            app.display().to_string(),
            Some("   ".into()),
            Some(" Tip Splitter ü ".into()),
            None,
            Some(String::new()),
            Some(true),
        ))
        .unwrap();
        assert_eq!(link, "https://krate.tech/r/abc123");
        let p = app.display().to_string();
        assert_eq!(
            t.args(),
            vec![
                "publish",
                p.as_str(),
                "--name",
                "Tip Splitter ü",
                "--unlisted"
            ]
        );
    }

    #[test]
    fn a_publish_that_fails_or_returns_no_link_says_so() {
        let t = TestHome::new();
        let app = t.root().join("tip.krate");
        std::fs::write(&app, "x").unwrap();
        let path = app.display().to_string();
        t.engine("echo 'error: sign in first' >&2; exit 1");
        assert_eq!(
            block(publish(path.clone(), None, None, None, None, None)).unwrap_err(),
            "error: sign in first"
        );
        t.engine("echo 'done'");
        assert_eq!(
            block(publish(path, None, None, None, None, None)).unwrap_err(),
            "published, but no link came back"
        );
    }

    const DUMP_CAPS: &str = "printf 'Identity\\n  - sha256:abcd\\nEffective capabilities\\n  - gui.window\\n  - store.kv\\nThis app will ask for\\n  - save its own settings and data (store.kv)\\n  - something plain\\n'";

    #[test]
    fn app_details_read_identity_asks_and_capabilities() {
        let t = TestHome::new();
        let app = t.root().join("tip ü.krate");
        std::fs::write(&app, "12345").unwrap();
        t.engine(DUMP_CAPS);
        let v = block(app_info(app.display().to_string())).unwrap();
        let p = app.display().to_string();
        assert_eq!(t.args(), vec!["run", "--dump-caps", p.as_str()]);
        assert_eq!(v["identity"], "sha256:abcd");
        assert_eq!(
            v["capabilities"],
            serde_json::json!(["gui.window", "store.kv"])
        );
        assert_eq!(
            v["asks"],
            serde_json::json!([
                { "words": "save its own settings and data", "cap": "store.kv" },
                { "words": "something plain", "cap": "" },
            ])
        );
        assert_eq!(v["size"], 5);
    }

    #[test]
    fn details_of_a_damaged_file_are_an_error_not_an_empty_safe_list() {
        let t = TestHome::new();
        let app = t.root().join("broken.krate");
        std::fs::write(&app, "x").unwrap();
        t.engine("echo 'error: not a Krate bundle' >&2; exit 1");
        assert_eq!(
            block(app_info(app.display().to_string())).unwrap_err(),
            "not a Krate bundle"
        );
        t.engine("exit 1");
        assert_eq!(
            block(app_info(app.display().to_string())).unwrap_err(),
            "that file could not be read as a Krate app"
        );
    }

    #[test]
    fn details_of_a_cloud_link_go_straight_to_the_engine() {
        let t = TestHome::new();
        t.engine(DUMP_CAPS);
        let url = "https://hub.krate.tech/apps/tip.krate";
        let v = block(app_info(url.into())).unwrap();
        assert_eq!(v["path"], url);
        assert_eq!(v["size"], 0);
        assert_eq!(t.args().last().map(String::as_str), Some(url));
    }

    /* card and wrap */

    #[test]
    fn the_card_is_where_the_engine_says_it_landed() {
        let t = TestHome::new();
        let app = t.root().join("tip.krate");
        std::fs::write(&app, "x").unwrap();
        t.engine("echo 'Making a card'; echo 'Card written: /tmp/x/My App (copy) ü.png (12 KB)'");
        assert_eq!(
            block(make_card(app.display().to_string())).unwrap(),
            "/tmp/x/My App (copy) ü.png"
        );
        let p = app.display().to_string();
        assert_eq!(t.args(), vec!["card", p.as_str()]);
    }

    #[test]
    fn a_card_that_fails_or_lands_nowhere_says_so() {
        let t = TestHome::new();
        let app = t.root().join("tip.krate");
        std::fs::write(&app, "x").unwrap();
        let path = app.display().to_string();
        t.engine("echo 'no frame was drawn' >&2; exit 1");
        assert_eq!(
            block(make_card(path.clone())).unwrap_err(),
            "no frame was drawn"
        );
        t.engine("exit 1");
        assert_eq!(
            block(make_card(path.clone())).unwrap_err(),
            "the card could not be made"
        );
        t.engine("echo 'done'");
        assert_eq!(
            block(make_card(path)).unwrap_err(),
            "the engine did not say where the card landed"
        );
    }

    #[test]
    fn a_wrap_for_an_unknown_system_is_refused_before_anything_else() {
        let t = TestHome::new();
        t.engine("exit 0");
        for target in ["macos", "", "../x", "Mac"] {
            assert_eq!(
                block(make_wrap("/gone.krate".into(), target.into())).unwrap_err(),
                "unknown system",
                "{target:?}"
            );
        }
        assert!(!t.engine_ran());
    }

    #[test]
    fn a_wrap_reads_the_folder_for_mac_and_the_file_elsewhere() {
        let t = TestHome::new();
        let app = t.root().join("tip.krate");
        std::fs::write(&app, "x").unwrap();
        let path = app.display().to_string();
        t.engine("echo 'Gift written: /out/Tip for Mac ü'");
        assert_eq!(
            block(make_wrap(path.clone(), "mac".into())).unwrap(),
            "/out/Tip for Mac ü"
        );
        assert_eq!(t.args(), vec!["wrap", path.as_str(), "--for", "mac"]);
        t.engine("echo 'Wrap written: /out/Tip (1).exe (34 KB)'");
        assert_eq!(
            block(make_wrap(path.clone(), "windows".into())).unwrap(),
            "/out/Tip (1).exe"
        );
        t.engine("exit 1");
        assert_eq!(
            block(make_wrap(path.clone(), "linux".into())).unwrap_err(),
            "the wrap could not be made"
        );
        t.engine("echo nothing");
        assert_eq!(
            block(make_wrap(path, "linux".into())).unwrap_err(),
            "the engine did not say where the wrap landed"
        );
    }

    /* signing an AI in: only the refusals, never a terminal */

    #[test]
    fn signing_in_an_ai_refuses_a_command_line_or_a_tool_krate_does_not_know() {
        let t = TestHome::new();
        t.engine(r#"echo '[{"name":"claude"},{"name":"codex"}]'"#);
        for bad in [
            "claude; rm -rf ~",
            "claude login",
            "../claude",
            "$(id)",
            &"a".repeat(25),
        ] {
            assert_eq!(
                block(sign_in_agent(bad.to_string())).unwrap_err(),
                "that is not a tool name",
                "{bad:?}"
            );
        }
        assert!(!t.engine_ran(), "refused before the engine is asked");
        for unknown in ["nosuchtool", ""] {
            assert_eq!(
                block(sign_in_agent(unknown.to_string())).unwrap_err(),
                "Krate does not know that tool",
                "{unknown:?}"
            );
        }
        assert_eq!(t.args(), vec!["ai", "--json"]);
    }

    /* IDE: the engine-backed commands that need no window */

    #[test]
    fn the_sdk_reference_is_the_engines_json() {
        let t = TestHome::new();
        t.engine(r#"echo '{"functions":[{"name":"gui.text"}]}'"#);
        assert_eq!(
            block(ide::ide_sdk()).unwrap(),
            serde_json::json!({ "functions": [{ "name": "gui.text" }] })
        );
        assert_eq!(t.args(), vec!["sdk-reference"]);
        t.engine("exit 2");
        assert_eq!(
            block(ide::ide_sdk()).unwrap_err(),
            "this Krate engine is too old to describe its API"
        );
        t.engine("echo 'not json'");
        assert!(block(ide::ide_sdk())
            .unwrap_err()
            .starts_with("the API description did not parse: "));
    }

    /// The real engine of this worktree, when it has been built. Read-only:
    /// `sdk-reference` prints and exits.
    #[test]
    fn the_sdk_reference_from_the_real_engine_is_an_object() {
        let t = TestHome::new();
        let real = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("target")
            .join("debug")
            .join("krate");
        if !real.is_file() {
            eprintln!("no {} -- skipped", real.display());
            return;
        }
        std::env::set_var("KRATE_STUDIO_ENGINE", &real);
        let v = block(ide::ide_sdk()).unwrap();
        assert!(
            v.as_object().is_some_and(|o| !o.is_empty()),
            "{}",
            v.to_string().chars().take(200).collect::<String>()
        );
        drop(t);
    }

    #[test]
    fn explain_asks_in_the_project_folder_with_the_question_after_the_double_dash() {
        let t = TestHome::new();
        let proj = make_project(&t.root(), "proj ü");
        t.engine(r#"echo '{"answer":"It draws a list."}'"#);
        assert_eq!(
            block(ide::ide_explain(
                proj.display().to_string(),
                "-why does it flicker?".into(),
                Some("codex".into())
            ))
            .unwrap(),
            "It draws a list."
        );
        assert_eq!(
            t.args(),
            vec!["explain", "--agent", "codex", "--", "-why does it flicker?"]
        );
        assert_eq!(t.engine_cwd(), proj);
        // An agent that is not a name falls back to the person's setting.
        block(ide::ide_explain(
            proj.display().to_string(),
            "why".into(),
            Some("--dangerously".into()),
        ))
        .unwrap();
        assert_eq!(t.args(), vec!["explain", "--agent", "claude", "--", "why"]);
    }

    #[test]
    fn an_explanation_that_fails_says_why_in_plain_words() {
        let t = TestHome::new();
        let proj = make_project(&t.root(), "proj").display().to_string();
        let ask = || block(ide::ide_explain(proj.clone(), "why".into(), None));
        t.engine("echo 'error: claude is not signed in' >&2; exit 1");
        assert_eq!(ask().unwrap_err(), "claude is not signed in");
        t.engine("exit 1");
        assert_eq!(ask().unwrap_err(), "the AI could not answer");
        t.engine("echo 'half an ans'");
        assert_eq!(ask().unwrap_err(), "the answer did not come back whole");
        t.engine(r#"echo '{"nope":1}'"#);
        assert_eq!(
            ask().unwrap(),
            "",
            "no answer field reads as an empty answer"
        );
    }
}

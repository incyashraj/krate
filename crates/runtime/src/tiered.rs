//! A first launch that starts now, and a second that starts on the best code
//! (K-1375).
//!
//! Cold, a component is compiled before its first instruction runs, and with
//! Cranelift that is the whole gap to a native app: Vault's 4.8 MB component
//! took 382 ms on an M4 (Cranelift at no optimization at all still took
//! 331 ms), so a fresh install opened its window about 300 ms after
//! Obsidian's. Warm, the two were level.
//!
//! So a large component, the first time this machine sees it, is compiled by
//! Winch -- wasmtime's baseline compiler, one pass and no optimization: 77 ms
//! for the same component -- and runs on that code for the whole session.
//! Meanwhile the optimized build is made on threads below the app's and kept
//! here, and every later launch maps it straight from this folder.
//!
//! What is kept is what this machine compiled itself, from the bytes being
//! run. Nothing compiled elsewhere is ever loaded: a .krate is untrusted, and
//! code shipped inside one would be native code nobody checked.
//!
//! An app Winch cannot compile (one using wasm exceptions, which Winch does
//! not implement, or an instruction it lacks) is compiled the ordinary way,
//! and kept the same way.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};
use wasmtime::component::Component;
use wasmtime::Engine;

use crate::stage_mark;

/// The smallest component that starts on quick code. Below it the optimized
/// compile is short enough to just do: 35 ms for a 329 KB app (krate-leave),
/// 5 ms for hello-gui. A session on quick code runs slower, and a short
/// command-line run would end before its optimized build was kept.
const QUICK_FIRST_MIN_BYTES: usize = 512 * 1024;

/// How much kept code this folder holds before the least recently used goes.
/// Vault's optimized build is 11 MB.
const KEPT_LIMIT_BYTES: u64 = 512 * 1024 * 1024;

/// How long a run that has ended waits for its optimized build to be kept.
/// The window has closed by then; this is the process staying a moment so
/// the next launch is fast. The build itself takes about a second.
pub(crate) const FINISH_LIMIT: Duration = Duration::from_secs(15);

const KEPT_EXTENSION: &str = "cwasm";

pub(crate) struct Tiered {
    /// Where optimized builds are kept, one file per component and engine.
    dir: PathBuf,
    /// The quick compiler's settings, when this host has it: the optimized
    /// engine's own, compiled by Winch. Made into an engine only on a miss.
    quick: Option<wasmtime::Config>,
    /// [`QUICK_FIRST_MIN_BYTES`], except in tests.
    quick_min_bytes: usize,
    pending: Arc<Pending>,
}

/// Optimized builds still being made, so the process can wait for them.
#[derive(Default)]
struct Pending {
    count: Mutex<usize>,
    done: Condvar,
}

impl Pending {
    fn start(&self) {
        *self.count.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    }

    fn finish(&self) {
        let mut count = self.count.lock().unwrap_or_else(|e| e.into_inner());
        *count = count.saturating_sub(1);
        self.done.notify_all();
    }

    fn wait(&self, limit: Duration) {
        let count = self.count.lock().unwrap_or_else(|e| e.into_inner());
        if *count == 0 {
            return;
        }
        stage_mark("runtime: waiting for the optimized build to be kept");
        let _ = self
            .done
            .wait_timeout_while(count, limit, |count| *count > 0);
    }
}

impl Tiered {
    /// `optimized` is the engine's own settings; the quick ones are derived
    /// from them so the two engines can never disagree about what an app
    /// may contain (only Winch's own limits narrow it, and a component that
    /// meets one falls back to the optimized compile).
    pub(crate) fn new(dir: PathBuf, optimized: &wasmtime::Config) -> Self {
        Self {
            dir,
            quick: quick_settings(optimized),
            quick_min_bytes: QUICK_FIRST_MIN_BYTES,
            pending: Arc::default(),
        }
    }

    /// Wait, at most `limit`, for optimized builds still being made.
    pub(crate) fn wait_for_background(&self, limit: Duration) {
        self.pending.wait(limit);
    }

    /// The component, compiled by the fastest road that is right for it.
    pub(crate) fn load(&self, engine: &Engine, bytes: &[u8]) -> wasmtime::Result<Component> {
        // Anything but a binary is refused exactly as before: the precompile
        // below would also read the text format, and answer a file that is
        // not wasm with a text parser's complaint ("expected `(`").
        if !bytes.starts_with(b"\0asm") {
            return Component::from_binary(engine, bytes);
        }
        let kept = self
            .dir
            .join(format!("{}.{KEPT_EXTENSION}", key(engine, bytes)));

        // The optimized build from an earlier launch, mapped in place.
        if kept.is_file() {
            touch(&kept);
            // Safety: the file is one this machine wrote from these exact
            // bytes with this exact engine (the name says both), into the
            // user's own cache folder -- the same trust wasmtime's own
            // compile cache rests on. Nothing in it came from the bundle.
            // wasmtime still checks its header against the engine, and a
            // file it refuses is a miss, removed.
            match unsafe { Component::deserialize_file(engine, &kept) } {
                Ok(component) => {
                    stage_mark("runtime: optimized code mapped from the cache");
                    return Ok(component);
                }
                Err(error) => {
                    tracing::debug!(%error, path = %kept.display(), "kept code refused");
                    let _ = std::fs::remove_file(&kept);
                }
            }
        }

        if bytes.len() >= self.quick_min_bytes && quick_first_allowed() {
            if let Some(quick) = self.quick_engine() {
                match Component::from_binary(&quick, bytes) {
                    Ok(component) => {
                        stage_mark("runtime: quick code compiled, the optimized build follows");
                        self.optimize_in_background(engine.clone(), bytes.to_vec(), kept);
                        return Ok(component);
                    }
                    // Not an error to show: Winch's limits are not the app's.
                    // The optimized compiler below gives the real answer.
                    Err(error) => tracing::debug!(%error, "quick compile declined"),
                }
            }
        }

        // The ordinary compile, kept for the next launch.
        let artifact = engine.precompile_component(bytes)?;
        keep(&self.dir, &kept, &artifact);
        stage_mark("runtime: optimized code compiled");
        // Safety: the artifact was produced by this engine a moment ago.
        unsafe { Component::deserialize(engine, &artifact) }
    }

    fn quick_engine(&self) -> Option<Engine> {
        let config = self.quick.as_ref()?;
        match Engine::new(config) {
            Ok(engine) => Some(engine),
            Err(error) => {
                tracing::debug!(%error, "no quick compiler on this host");
                None
            }
        }
    }

    /// Make the optimized build on threads below the app's, and keep it.
    fn optimize_in_background(&self, engine: Engine, bytes: Vec<u8>, kept: PathBuf) {
        let pending = self.pending.clone();
        let dir = self.dir.clone();
        pending.start();
        let thread_pending = pending.clone();
        let spawned = std::thread::Builder::new()
            .name("krate-optimize".to_string())
            .spawn(move || {
                lower_this_thread();
                let threads = std::thread::available_parallelism()
                    .map(|n| n.get().saturating_sub(1).max(1))
                    .unwrap_or(1);
                // wasmtime compiles a component's functions in parallel on
                // whichever rayon pool it is called from; this one's threads
                // are lowered too, so the whole build yields to the app.
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .thread_name(|i| format!("krate-optimize-{i}"))
                    .start_handler(|_| lower_this_thread())
                    .build();
                let artifact = match pool {
                    Ok(pool) => pool.install(|| engine.precompile_component(&bytes)),
                    Err(_) => engine.precompile_component(&bytes),
                };
                match artifact {
                    Ok(artifact) => {
                        keep(&dir, &kept, &artifact);
                        stage_mark("runtime: optimized build kept for the next launch");
                    }
                    Err(error) => tracing::debug!(%error, "optimized build failed"),
                }
                thread_pending.finish();
            });
        if spawned.is_err() {
            pending.finish();
        }
    }
}

/// The optimized engine's settings, compiled by Winch -- when Winch exists
/// for this processor, and with the two proposals it does not implement left
/// off: exceptions (a component that throws is declined and compiled the
/// ordinary way) and threads (a component never has a shared memory; a
/// threaded app is a core module and never comes here).
fn quick_settings(optimized: &wasmtime::Config) -> Option<wasmtime::Config> {
    if !cfg!(any(target_arch = "x86_64", target_arch = "aarch64")) {
        return None;
    }
    let mut quick = optimized.clone();
    quick.strategy(wasmtime::Strategy::Winch);
    quick.wasm_exceptions(false);
    quick.wasm_threads(false);
    quick.shared_memory(false);
    // Quick code is never kept: the optimized build is what the next launch
    // wants.
    quick.cache(None);
    Some(quick)
}

/// `KRATE_QUICK_FIRST=0` compiles every first launch the optimized way, for
/// measuring the two against each other. Not documented for users.
fn quick_first_allowed() -> bool {
    std::env::var("KRATE_QUICK_FIRST").as_deref() != Ok("0")
}

/// The kept file's name: the component's bytes and everything about the
/// engine that decides whether code it compiled can be loaded.
fn key(engine: &Engine, bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut settings = std::collections::hash_map::DefaultHasher::new();
    engine.precompile_compatibility_hash().hash(&mut settings);
    let mut name = String::with_capacity(64 + 1 + 16);
    for byte in digest {
        name.push_str(&format!("{byte:02x}"));
    }
    name.push_str(&format!("-{:016x}", settings.finish()));
    name
}

/// Write the artifact whole or not at all: a reader maps either the old file
/// or the new one, never half of one.
fn keep(dir: &Path, kept: &Path, artifact: &[u8]) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let staging = kept.with_extension(format!("{}.tmp", std::process::id()));
    if std::fs::write(&staging, artifact).is_err() {
        let _ = std::fs::remove_file(&staging);
        return;
    }
    // Another launch may have kept the same build first, and on Windows a
    // file another process has mapped cannot be replaced: either way the
    // build is there.
    if std::fs::rename(&staging, kept).is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    sweep(dir, KEPT_LIMIT_BYTES);
}

/// Mark a kept build as used, so the sweep removes the ones that are not.
fn touch(kept: &Path) {
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(kept) {
        let _ = file.set_modified(SystemTime::now());
    }
}

/// Remove the least recently used kept builds until the folder is under
/// `limit`. A file that cannot be removed (mapped by a running app on
/// Windows) is left for the next sweep.
fn sweep(dir: &Path, limit: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut kept: Vec<(SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|e| e.to_str()) == Some(KEPT_EXTENSION))
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            Some((meta.modified().ok()?, meta.len(), entry.path()))
        })
        .collect();
    let mut total: u64 = kept.iter().map(|(_, len, _)| len).sum();
    if total <= limit {
        return;
    }
    kept.sort();
    for (_, len, path) in kept {
        if total <= limit {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
}

/// Put the calling thread below the app's own: on a Mac the utility class,
/// which the scheduler keeps off the cores the app is using when it can; a
/// higher nice value on Linux and Android, where it is per thread; below
/// normal on Windows.
fn lower_this_thread() {
    #[cfg(target_vendor = "apple")]
    // Safety: sets the calling thread's own QoS class; no pointers.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    // Safety: gettid and setpriority on the calling thread's own id.
    unsafe {
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        libc::setpriority(libc::PRIO_PROCESS, tid, 10);
    }
    #[cfg(target_os = "windows")]
    // Safety: the pseudo-handle for the calling thread, always valid.
    unsafe {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
        };
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Krate's own engine settings for a windowed app (epoch interruption
    /// on), and a tiered loader over a folder of the test's own that sends
    /// every component, however small, down the quick road.
    fn windowed() -> (Engine, Tiered, tempfile::TempDir) {
        let config = crate::Config {
            phase3_ui_mode: crate::phase3_ui::Phase3HostUiMode::NativeWithHeadlessFallback,
            ..crate::Config::default()
        };
        let mut settings = crate::Runtime::engine_settings(&config).expect("settings");
        settings.cache(None);
        let engine = Engine::new(&settings).expect("engine");
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tiered = Tiered::new(dir.path().join("optimized"), &settings);
        tiered.quick_min_bytes = 0;
        (engine, tiered, dir)
    }

    /// A component whose one export never returns.
    const SPINS: &str = r#"(component
        (core module $m (func (export "spin") (loop $l (br $l))))
        (core instance $i (instantiate $m))
        (func (export "spin") (canon lift (core func $i "spin"))))"#;

    /// The same, carrying a wasm exception handler, which Winch does not
    /// compile.
    const THROWS: &str = r#"(component
        (core module $m
          (tag $t)
          (func (export "spin")
            (block $done
              (try_table (catch $t $done) (loop $l (br $l))))))
        (core instance $i (instantiate $m))
        (func (export "spin") (canon lift (core func $i "spin"))))"#;

    fn kept_files(tiered: &Tiered) -> Vec<PathBuf> {
        std::fs::read_dir(&tiered.dir)
            .map(|entries| entries.flatten().map(|e| e.path()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn a_first_launch_runs_quick_code_and_the_next_maps_the_optimized_build() {
        let (engine, tiered, _dir) = windowed();
        let bytes = wat::parse_str(SPINS).expect("wat");

        let first = tiered.load(&engine, &bytes).expect("first launch");
        assert!(
            !Engine::same(first.engine(), &engine),
            "the first launch runs on the quick engine"
        );
        tiered.wait_for_background(Duration::from_secs(60));
        let kept = kept_files(&tiered);
        assert_eq!(kept.len(), 1, "the optimized build was kept: {kept:?}");
        assert_eq!(
            kept[0].extension().and_then(|e| e.to_str()),
            Some(KEPT_EXTENSION),
            "and no staging file is left beside it"
        );

        let second = tiered.load(&engine, &bytes).expect("second launch");
        assert!(
            Engine::same(second.engine(), &engine),
            "the second launch runs on the optimized engine, from the kept build"
        );
    }

    #[test]
    fn a_kept_build_that_will_not_load_is_a_miss_not_a_failure() {
        let (engine, tiered, _dir) = windowed();
        let bytes = wat::parse_str(SPINS).expect("wat");
        tiered.load(&engine, &bytes).expect("first launch");
        tiered.wait_for_background(Duration::from_secs(60));
        let kept = kept_files(&tiered).pop().expect("kept");
        std::fs::write(&kept, b"not compiled code").expect("damage it");

        let again = tiered.load(&engine, &bytes).expect("still loads");
        assert!(
            !Engine::same(again.engine(), &engine),
            "a damaged build is treated as none: the quick road again"
        );
        tiered.wait_for_background(Duration::from_secs(60));
        let rebuilt = std::fs::read(&kept).expect("kept again");
        assert_ne!(rebuilt, b"not compiled code", "and the build is made again");
    }

    #[test]
    fn a_component_winch_cannot_compile_is_compiled_the_ordinary_way_and_kept() {
        let (engine, tiered, _dir) = windowed();
        let bytes = wat::parse_str(THROWS).expect("wat");
        let loaded = tiered.load(&engine, &bytes).expect("exceptions still load");
        assert!(
            Engine::same(loaded.engine(), &engine),
            "an app using exceptions starts on the optimized engine"
        );
        assert_eq!(kept_files(&tiered).len(), 1, "kept without waiting");
        let again = tiered.load(&engine, &bytes).expect("second launch");
        assert!(Engine::same(again.engine(), &engine));
    }

    /// The window's close button and a stuck app's way out both ride on
    /// epoch interruption (K-032); wasmtime's notes say Winch lacks it, so
    /// this proves the quick code honours it.
    #[test]
    fn quick_code_is_interrupted_by_the_epoch() {
        let (engine, tiered, _dir) = windowed();
        let bytes = wat::parse_str(SPINS).expect("wat");
        let component = tiered.load(&engine, &bytes).expect("load");
        let quick = component.engine().clone();
        assert!(!Engine::same(&quick, &engine), "on the quick engine");

        let mut store = wasmtime::Store::new(&quick, ());
        store.set_epoch_deadline(1);
        store.epoch_deadline_callback(|_| Err(wasmtime::Error::msg("ended by the epoch")));
        let linker = wasmtime::component::Linker::new(&quick);
        let instance = linker
            .instantiate(&mut store, &component)
            .expect("instantiate");
        let spin = instance
            .get_typed_func::<(), ()>(&mut store, "spin")
            .expect("spin");
        let ticker = quick.weak();
        let ticking = std::thread::spawn(move || {
            for _ in 0..500 {
                std::thread::sleep(Duration::from_millis(10));
                match ticker.upgrade() {
                    Some(engine) => engine.increment_epoch(),
                    None => return,
                }
            }
        });
        // Run on its own thread with a deadline, so quick code that ignores
        // the epoch fails this test in seconds instead of hanging the suite.
        let (said, heard) = std::sync::mpsc::channel();
        let running = std::thread::spawn(move || {
            let ended = spin.call(&mut store, ());
            let _ = said.send(ended.is_err());
            drop(store);
        });
        let interrupted = heard
            .recv_timeout(Duration::from_secs(10))
            .expect("quick code ignored the epoch: the endless loop was still running after 10 s");
        assert!(interrupted, "the endless loop was interrupted");
        let _ = running.join();
        drop(component);
        drop(quick);
        let _ = ticking.join();
        tiered.wait_for_background(Duration::from_secs(60));
    }

    #[test]
    fn sweep_removes_the_least_recently_used_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        let old = dir.path().join(format!("old.{KEPT_EXTENSION}"));
        let new = dir.path().join(format!("new.{KEPT_EXTENSION}"));
        let other = dir.path().join("other.bin");
        std::fs::write(&old, vec![0u8; 100]).expect("old");
        std::fs::write(&new, vec![0u8; 100]).expect("new");
        std::fs::write(&other, vec![0u8; 1000]).expect("other");
        let past = SystemTime::now() - Duration::from_secs(3600);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&old)
            .and_then(|f| f.set_modified(past))
            .expect("age the old file");
        sweep(dir.path(), 150);
        assert!(!old.exists(), "the older build goes first");
        assert!(new.exists(), "the newer build stays");
        assert!(
            other.exists(),
            "a file that is not a kept build is never touched"
        );
    }

    #[test]
    fn the_key_names_the_bytes_and_the_engine() {
        let engine = Engine::default();
        let a = key(&engine, b"one");
        assert_eq!(a, key(&engine, b"one"), "the same bytes, the same name");
        assert_ne!(a, key(&engine, b"two"), "other bytes, another name");
        let mut config = wasmtime::Config::new();
        config.consume_fuel(true);
        let other = Engine::new(&config).expect("engine");
        assert_ne!(
            a,
            key(&other, b"one"),
            "an engine whose code cannot be shared names its own file"
        );
    }
}

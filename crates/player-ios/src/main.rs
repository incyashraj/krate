//! Krate for iOS: the app a person opens Krate apps with.
//!
//! A `.krate` someone sends -- in Messages, Mail, AirDrop or Files -- opens
//! here ("Open in Krate"). Krate keeps it in the person's library, asks
//! with a native iOS alert what the app may use, and runs it. Tapping the
//! Krate icon shows the library: every app received, and any app this
//! build carries.
//!
//! Two engines, one rule from Apple (2.5.2): code that arrives after
//! install may only be interpreted, so a received app runs on Pulley,
//! wasmtime's interpreter. An app compiled on our machines and built into
//! this binary (see build.rs) is native code signed with Krate, and runs
//! at full speed -- including when the same app arrives as a `.krate`:
//! its code is matched by hash, and the carried native code runs instead.
//!
//! Startup is the one genuinely iOS-shaped part. UIApplicationMain owns
//! the process and never returns, so the player's real main runs on its
//! own thread from the app delegate's first `applicationDidBecomeActive:`.
//! When the person leaves, the process exits -- the app is the process,
//! same as Android -- so every open is a fresh launch.

#[cfg(target_os = "ios")]
// The app-delegate URL callback: this app runs without a scene manifest, so
// that is where "Open in Krate" arrives, though UIKit marks its key type
// deprecated in favour of the scene lifecycle.
#[allow(deprecated)]
mod player {
    use crate::home::{field, human_size, HomeAnswer, Indexed};
    use crate::run_config::{sanitize, use_in_words};
    use objc2::{define_class, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_foundation::{NSDictionary, NSObject, NSObjectProtocol, NSString, NSURL};
    use objc2_ui_kit::{
        UIApplication, UIApplicationDelegate, UIApplicationOpenURLOptionsKey,
        UIDocumentPickerDelegate, UIViewController, UIWindow, UIWindowScene,
    };
    use std::cell::Cell;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    /// The app this build carries as native code, if any: compiled on our
    /// machines and linked into this binary's executable text, so it is
    /// signed with Krate and runs at native speed where iOS lets nothing
    /// become executable at run time. The 16 KB alignment is the iPhone's
    /// page size.
    #[cfg(krate_native_image)]
    mod native {
        const N: usize = include_bytes!(env!("KRATE_NATIVE_IMAGE")).len();
        #[repr(C, align(16384))]
        pub struct Aligned(pub [u8; N]);
        #[link_section = "__TEXT,__krate_app,regular,pure_instructions"]
        pub static IMAGE: Aligned = Aligned(*include_bytes!(env!("KRATE_NATIVE_IMAGE")));
        pub const MANIFEST: &str = include_str!(env!("KRATE_NATIVE_MANIFEST"));
        /// The SHA-256 of the app's WebAssembly, so the same app arriving
        /// as a `.krate` is recognised and runs as this native code.
        pub const SHA256: &str = env!("KRATE_NATIVE_SHA256");
    }

    const WALL_WASM: &[u8] =
        include_bytes!("../../../apps/krate-wall/target/wasm32-wasip1/release/krate_wall.wasm");
    const WALL_MANIFEST: &str = include_str!("../../../apps/krate-wall/manifest.toml");
    const HOME_WASM: &[u8] =
        include_bytes!("../../../apps/krate-home/target/wasm32-wasip1/release/krate_home.wasm");
    const HOME_MANIFEST: &str = include_str!("../../../apps/krate-home/manifest.toml");

    // iOS's sign-in sheet (ASWebAuthenticationSession), for signing in to
    // Krate without leaving it.
    #[link(name = "AuthenticationServices", kind = "framework")]
    extern "C" {}

    /// Larger than any Krate app a phone should be handed.
    const MAX_BUNDLE_BYTES: u64 = 64 * 1024 * 1024;

    /// A file someone opened with Krate, waiting for the guest thread.
    static OPENED: Mutex<Option<PathBuf>> = Mutex::new(None);

    pub struct DelegateState {
        started: Cell<bool>,
    }

    define_class!(
        // SAFETY:
        // - NSObject has no subclassing requirements for an app delegate.
        // - UIKit calls the delegate on the main thread.
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = DelegateState]
        struct KrateAppDelegate;

        unsafe impl NSObjectProtocol for KrateAppDelegate {}

        impl KrateAppDelegate {
            #[unsafe(method_id(init))]
            fn init(this: objc2::rc::Allocated<Self>) -> objc2::rc::Retained<Self> {
                let this = this.set_ivars(DelegateState {
                    started: Cell::new(false),
                });
                unsafe { objc2::msg_send![super(this), init] }
            }
        }

        unsafe impl UIApplicationDelegate for KrateAppDelegate {
            /// Be the notification center's delegate before launch ends, so
            /// a reminder tapped while Krate was closed opens its app
            /// (IC-921).
            #[unsafe(method(application:didFinishLaunchingWithOptions:))]
            fn did_finish_launching(
                &self,
                _application: &UIApplication,
                _options: Option<&NSDictionary<objc2::runtime::AnyObject, objc2::runtime::AnyObject>>,
            ) -> objc2::runtime::Bool {
                krate_adapter_ios::uikit_native::install_notice_delegate();
                objc2::runtime::Bool::YES
            }

            /// "Open in Krate" from Messages, Mail, AirDrop or Files: iOS
            /// copies the file into Documents/Inbox and hands over its URL,
            /// before the app becomes active.
            #[unsafe(method(application:openURL:options:))]
            fn open_url(
                &self,
                _application: &UIApplication,
                url: &NSURL,
                _options: &NSDictionary<UIApplicationOpenURLOptionsKey, objc2::runtime::AnyObject>,
            ) -> objc2::runtime::Bool {
                let Some(path) = url.path() else {
                    return objc2::runtime::Bool::NO;
                };
                *OPENED.lock().expect("opened lock") = Some(PathBuf::from(path.to_string()));
                // Krate stays alive in the background now, so this usually
                // arrives with the home screen or an app already open: close
                // it, and the player opens the file next. At a cold start
                // nothing is running yet and the player reads OPENED first.
                if self.ivars().started.get() {
                    krate_adapter_ios::uikit_native::close_for_opened_file();
                }
                objc2::runtime::Bool::YES
            }

            /// Leaving Krate (another app, the home screen, the lock) used
            /// to end the process: whatever the person had open, playing or
            /// half-typed was gone when they came back. Krate now waits in
            /// the background as a phone's own apps do, drawing nothing.
            #[unsafe(method(applicationDidEnterBackground:))]
            fn did_enter_background(&self, _application: &UIApplication) {
                krate_adapter_ios::uikit_native::set_background(true);
            }

            #[unsafe(method(applicationWillEnterForeground:))]
            fn will_enter_foreground(&self, _application: &UIApplication) {
                krate_adapter_ios::uikit_native::set_background(false);
            }

            #[unsafe(method(applicationDidBecomeActive:))]
            fn did_become_active(&self, _application: &UIApplication) {
                // Back on screen, however it got there: iOS does not promise
                // will-enter-foreground for every return, and a background
                // flag left set draws nothing ever again.
                krate_adapter_ios::uikit_native::set_background(false);
                if self.ivars().started.replace(true) {
                    return;
                }
                std::thread::Builder::new()
                    .name("krate-guest".to_string())
                    .stack_size(8 * 1024 * 1024)
                    .spawn(|| {
                        unsafe {
                            libc::pthread_set_qos_class_self_np(
                                libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE,
                                0,
                            );
                        }
                        run_player();
                        std::process::exit(0);
                    })
                    .expect("spawn the guest thread");
            }
        }
    );

    pub fn main() {
        let mtm =
            objc2::MainThreadMarker::new().expect("the iOS player's main runs on the main thread");
        let delegate_class = NSString::from_class(KrateAppDelegate::class());
        UIApplication::main(None, Some(&delegate_class), mtm);
    }

    /// An app the person can open.
    enum App {
        /// The app this build carries as native code.
        #[cfg(krate_native_image)]
        Carried,
        /// A `.krate` in the library.
        Received(PathBuf),
    }

    /// Krate's sound is media the person chose to start: it plays with the
    /// phone's silent switch on, as a music app's does. Left alone, iOS's
    /// default session is silenced by the switch -- Shelf played every note
    /// into a muted session and the person heard nothing.
    fn audio_session() {
        // SAFETY: the shared session exists for the life of the process;
        // setting a category and activating it are ordinary session calls.
        unsafe {
            let session = objc2_avf_audio::AVAudioSession::sharedInstance();
            if let Some(playback) = objc2_avf_audio::AVAudioSessionCategoryPlayback {
                if let Err(why) = session.setCategory_error(playback) {
                    eprintln!("krate: audio session category refused: {why:?}");
                }
            }
            let _ = session.setActive_error(true);
        }
    }

    /// While an app that may record runs, the session plays AND records:
    /// the play-only session refuses the microphone, so a voice memo made in
    /// Studio heard nothing on an iPhone (IC-918). Sound still comes out of
    /// the speaker, not the earpiece, and Bluetooth headphones still play.
    fn record_session(records: bool) {
        // SAFETY: ordinary category changes on the shared session.
        unsafe {
            let session = objc2_avf_audio::AVAudioSession::sharedInstance();
            let result = if records {
                objc2_avf_audio::AVAudioSessionCategoryPlayAndRecord.map(|category| {
                    // DefaultToSpeaker (0x8) | AllowBluetoothA2DP (0x20).
                    session.setCategory_withOptions_error(
                        category,
                        objc2_avf_audio::AVAudioSessionCategoryOptions(0x8 | 0x20),
                    )
                })
            } else {
                objc2_avf_audio::AVAudioSessionCategoryPlayback
                    .map(|category| session.setCategory_error(category))
            };
            if let Some(Err(why)) = result {
                eprintln!("krate: audio session category refused: {why:?}");
            }
            let _ = session.setActive_error(true);
        }
    }

    fn run_player() {
        audio_session();
        let data_root = std::env::var("HOME")
            .map(|home| PathBuf::from(home).join("Documents"))
            .unwrap_or_else(|_| PathBuf::from("."));
        let library = data_root.join("Library");
        let _ = std::fs::create_dir_all(&library);

        krate_adapter_ios::uikit_native::show_loading("Krate", "");
        let mut next = None;
        loop {
            // A file opened from Messages, Mail or Files -- at start, or
            // while something else was on screen -- comes first.
            if let Some(path) = OPENED.lock().expect("opened lock").take() {
                if let Some(app) = receive(&path, &library, &data_root) {
                    next = Some(app);
                }
            }
            // A reminder tapped: the app that set it.
            let reminded = REMINDED.lock().expect("reminded lock").take();
            if let Some(opens) = reminded.or_else(krate_adapter_ios::uikit_native::take_notice_open)
            {
                if let Some(app) = app_named(&opens, &library) {
                    next = Some(app);
                }
            }
            let app = match next.take() {
                Some(app) => app,
                None => match home(&library, &data_root) {
                    Some(app) => app,
                    // The home screen closed because a file was opened: go
                    // round and open it. Otherwise Krate's home was closed.
                    None if OPENED.lock().expect("opened lock").is_some() => continue,
                    None if next.is_none() && reminder_waiting() => continue,
                    None => return,
                },
            };
            launch(app, &data_root);
        }
    }

    /// The app a reminder names: "carried", or a file in the library.
    fn app_named(opens: &str, library: &Path) -> Option<App> {
        #[cfg(krate_native_image)]
        if opens == "carried" {
            return Some(App::Carried);
        }
        let path = PathBuf::from(opens);
        (path.parent() == Some(library) && path.is_file()).then_some(App::Received(path))
    }

    /// Whether a tapped reminder is waiting to open its app. Looked at, not
    /// taken: the loop takes it on its next turn.
    fn reminder_waiting() -> bool {
        match krate_adapter_ios::uikit_native::take_notice_open() {
            Some(opens) => {
                REMINDED.lock().expect("reminded lock").replace(opens);
                true
            }
            None => REMINDED.lock().expect("reminded lock").is_some(),
        }
    }

    /// A tapped reminder seen by `reminder_waiting`, for the loop to open.
    static REMINDED: Mutex<Option<String>> = Mutex::new(None);

    /// Keep a received `.krate` in the library, named for its app, so the
    /// home screen lists it and a newer copy replaces an older one.
    fn receive(path: &Path, library: &Path, data_root: &Path) -> Option<App> {
        let kept = (|| -> Result<PathBuf, String> {
            let size = std::fs::metadata(path)
                .map_err(|_| "The file is not there any more.".to_string())?
                .len();
            if size > MAX_BUNDLE_BYTES {
                return Err("This file is larger than any Krate app.".to_string());
            }
            let open = krate_bundle::open(path).map_err(|err| match err {
                krate_bundle::BundleError::NeedsNewerKrate { .. } => format!("{err}"),
                _ => {
                    "This file is not a Krate app, or it was damaged on the way. Ask for it again."
                        .to_string()
                }
            })?;
            let manifest = std::fs::read_to_string(open.manifest_path()).unwrap_or_default();
            let manifest = krate_manifest::Manifest::parse(&manifest)
                .map_err(|_| "This app's description could not be read.".to_string())?;
            let into = library.join(crate::home::library_file(&manifest.app.id));
            std::fs::copy(path, &into).map_err(|err| format!("Could not keep the app: {err}"))?;
            remember(library, &into, &manifest.app.name);
            Ok(into)
        })();
        // iOS put the file in Documents/Inbox for us; it is ours to remove.
        let _ = std::fs::remove_file(path);
        match kept {
            Ok(into) => Some(App::Received(into)),
            Err(why) => {
                notice(data_root, "Could not open this file", &why);
                None
            }
        }
    }

    /// The library's index: one line per kept app -- file name, app name,
    /// SHA-256, size -- so the home screen lists the library without
    /// unpacking every app to read its name each time it is drawn.
    fn remember(library: &Path, file: &Path, name: &str) {
        let Ok(bytes) = std::fs::read(file) else {
            return;
        };
        let file_name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let index = library.join("index.tsv");
        let text = std::fs::read_to_string(&index).unwrap_or_default();
        let kept = Indexed {
            name: name.to_string(),
            sha: krate_bundle::sha256_hex(&bytes),
            size: bytes.len() as u64,
        };
        let _ = std::fs::write(index, crate::home::index_with(&text, &file_name, &kept));
    }

    /// The library's apps, newest first: (file, name, sha, size). Apps kept
    /// before the index existed are read once and added to it.
    fn library_apps(library: &Path) -> Vec<(PathBuf, String, String, u64)> {
        let index = std::fs::read_to_string(library.join("index.tsv")).unwrap_or_default();
        let mut known: std::collections::HashMap<String, (String, String, u64)> =
            crate::home::read_index(&index)
                .into_iter()
                .map(|(file, kept)| (file, (kept.name, kept.sha, kept.size)))
                .collect();
        let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(library)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "krate"))
            .map(|path| {
                let at = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                (at, path)
            })
            .collect();
        files.sort_by_key(|f| std::cmp::Reverse(f.0));
        let mut out = Vec::new();
        for (_, path) in files {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if known.get(&name).is_none_or(|k| k.2 != size) {
                let app_name = krate_bundle::open(&path)
                    .ok()
                    .and_then(|open| std::fs::read_to_string(open.manifest_path()).ok())
                    .and_then(|text| krate_manifest::Manifest::parse(&text).ok())
                    .map(|manifest| manifest.app.name);
                let Some(app_name) = app_name else {
                    continue;
                };
                remember(library, &path, &app_name);
                let sha = std::fs::read(&path)
                    .map(|b| krate_bundle::sha256_hex(&b))
                    .unwrap_or_default();
                known.insert(name.clone(), (app_name, sha, size));
            }
            if let Some((app_name, sha, size)) = known.get(&name).cloned() {
                out.push((path, app_name, sha, size));
            }
        }
        out
    }

    /// A kept app's manifest, read from its bundle.
    fn manifest_of(path: &Path) -> Option<krate_manifest::Manifest> {
        let open = krate_bundle::open(path).ok()?;
        let text = std::fs::read_to_string(open.manifest_path()).ok()?;
        krate_manifest::Manifest::parse(&text).ok()
    }

    /// Where the sign-in is kept: Application Support, which the Files app
    /// never shows, not Documents, which a sharing build can.
    fn private_root() -> PathBuf {
        let root = std::env::var("HOME")
            .map(|home| {
                PathBuf::from(home)
                    .join("Library")
                    .join("Application Support")
                    .join("Krate")
            })
            .unwrap_or_else(|_| PathBuf::from("."));
        let _ = std::fs::create_dir_all(&root);
        root
    }

    /// The account the profile tab shows, as last fetched: refreshed from
    /// the hub off the guest thread's way, so the home screen never waits.
    static ACCOUNT: Mutex<Option<crate::home::Account>> = Mutex::new(None);

    fn refresh_account(private: &Path, art_dir: &Path) {
        let found = crate::account::account(private, art_dir);
        *ACCOUNT.lock().expect("account lock") = found;
    }

    /// The phone's name, for the profile tab. Since iOS 16 an app without
    /// Apple's entitlement is told the model ("iPhone"), which is still true.
    fn device_name() -> String {
        static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        NAME.get_or_init(|| {
            let (tx, rx) = std::sync::mpsc::channel();
            dispatch2::DispatchQueue::main().exec_async(move || {
                if let Some(mtm) = MainThreadMarker::new() {
                    let name = objc2_ui_kit::UIDevice::currentDevice(mtm).name();
                    let _ = tx.send(name.to_string());
                }
            });
            rx.recv_timeout(std::time::Duration::from_secs(2))
                .ok()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "This iPhone".to_string())
        })
        .clone()
    }

    define_class!(
        // SAFETY:
        // - NSObject has no subclassing requirements for a context provider.
        // - The sign-in sheet asks its provider on the main thread.
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        struct KrateSignInAnchor;

        unsafe impl NSObjectProtocol for KrateSignInAnchor {}

        impl KrateSignInAnchor {
            /// ASWebAuthenticationPresentationContextProviding: the window
            /// the sign-in sheet rises over -- Krate's own.
            #[unsafe(method_id(presentationAnchorForWebAuthenticationSession:))]
            fn anchor(&self, _session: &objc2::runtime::AnyObject) -> objc2::rc::Retained<UIWindow> {
                key_window(self.mtm())
            }
        }
    );

    /// Krate's window, for a system sheet to rise over.
    fn key_window(mtm: MainThreadMarker) -> objc2::rc::Retained<UIWindow> {
        let scene = UIApplication::sharedApplication(mtm)
            .connectedScenes()
            .allObjects()
            .into_iter()
            .find_map(|scene| scene.downcast::<UIWindowScene>().ok());
        if let Some(scene) = &scene {
            // SAFETY: UIWindowScene's keyWindow (iOS 15, Krate's oldest) and
            // windows, read on the main thread.
            let key: Option<objc2::rc::Retained<UIWindow>> =
                unsafe { objc2::msg_send![&**scene, keyWindow] };
            if let Some(key) = key {
                return key;
            }
            let all: objc2::rc::Retained<objc2_foundation::NSArray<UIWindow>> =
                unsafe { objc2::msg_send![&**scene, windows] };
            if let Some(first) = all.firstObject() {
                return first;
            }
            return UIWindow::initWithWindowScene(UIWindow::alloc(mtm), scene);
        }
        // SAFETY: a plain window, only ever an anchor.
        unsafe { objc2::msg_send![UIWindow::alloc(mtm), init] }
    }

    thread_local! {
        /// The sign-in sheet and its provider, alive until it answers: the
        /// session holds its provider weakly. Main thread only.
        static SIGNING_IN: std::cell::RefCell<
            Option<(objc2::rc::Retained<objc2::runtime::AnyObject>, objc2::rc::Retained<KrateSignInAnchor>)>,
        > = const { std::cell::RefCell::new(None) };
    }

    /// Krate's sign-in page in iOS's own sign-in sheet
    /// (ASWebAuthenticationSession), sharing Safari's sign-ins so a person
    /// already signed in to GitHub or Google is one tap from done. The sheet
    /// closes itself the moment the page hands back `krate://signed-in...`,
    /// and that address is the answer. None when the person cancelled.
    fn web_sign_in(url: &str) -> Result<Option<String>, String> {
        let url = url.to_string();
        let (tx, rx) = std::sync::mpsc::channel::<Result<Option<String>, String>>();
        dispatch2::DispatchQueue::main().exec_async(move || {
            if let Err(why) = start_sign_in(&url, tx.clone()) {
                let _ = tx.send(Err(why));
            }
        });
        // An email sign-in can take a while: a code to fetch from the inbox.
        rx.recv_timeout(std::time::Duration::from_secs(30 * 60))
            .unwrap_or(Ok(None))
    }

    fn start_sign_in(
        url: &str,
        tx: std::sync::mpsc::Sender<Result<Option<String>, String>>,
    ) -> Result<(), String> {
        let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
        let class = objc2::runtime::AnyClass::get(c"ASWebAuthenticationSession")
            .ok_or("this iPhone has no sign-in sheet")?;
        let link = NSURL::URLWithString(&NSString::from_str(url)).ok_or("a bad sign-in address")?;
        let tx = Mutex::new(Some(tx));
        let done = block2::RcBlock::new(
            move |back: *mut objc2::runtime::AnyObject, error: *mut objc2::runtime::AnyObject| {
                // SAFETY: the session hands an NSURL or an NSError (or
                // neither), valid for the call; read on the main thread.
                let answer = unsafe {
                    if let Some(back) = back.as_ref() {
                        let text: Option<objc2::rc::Retained<NSString>> =
                            objc2::msg_send![back, absoluteString];
                        Ok(text.map(|t| t.to_string()))
                    } else if let Some(error) = error.as_ref() {
                        // ASWebAuthenticationSessionErrorCodeCanceledLogin.
                        let code: isize = objc2::msg_send![error, code];
                        if code == 1 {
                            Ok(None)
                        } else {
                            let words: Option<objc2::rc::Retained<NSString>> =
                                objc2::msg_send![error, localizedDescription];
                            Err(words
                                .map(|w| w.to_string())
                                .unwrap_or_else(|| "the sign-in sheet stopped".to_string()))
                        }
                    } else {
                        Ok(None)
                    }
                };
                if let Some(tx) = tx.lock().expect("sign-in lock").take() {
                    let _ = tx.send(answer);
                }
                // Let the session go, once it has finished calling us.
                dispatch2::DispatchQueue::main().exec_async(|| {
                    SIGNING_IN.with(|slot| slot.borrow_mut().take());
                });
            },
        );
        let anchor: objc2::rc::Retained<KrateSignInAnchor> = {
            let this = KrateSignInAnchor::alloc(mtm).set_ivars(());
            unsafe { objc2::msg_send![super(this), init] }
        };
        // SAFETY: -[ASWebAuthenticationSession
        // initWithURL:callbackURLScheme:completionHandler:] on a fresh
        // allocation, its provider and Safari-sharing set, then -start, all
        // on the main thread. The block is copied by the session.
        unsafe {
            let made: objc2::rc::Allocated<objc2::runtime::AnyObject> =
                objc2::msg_send![class, alloc];
            let session: Option<objc2::rc::Retained<objc2::runtime::AnyObject>> = objc2::msg_send![
                made,
                initWithURL: &*link,
                callbackURLScheme: &*NSString::from_str("krate"),
                completionHandler: &*done
            ];
            let session = session.ok_or("the sign-in sheet would not open")?;
            let _: () = objc2::msg_send![&*session, setPresentationContextProvider: &*anchor];
            let _: () = objc2::msg_send![
                &*session,
                setPrefersEphemeralWebBrowserSession: objc2::runtime::Bool::NO
            ];
            let started: objc2::runtime::Bool = objc2::msg_send![&*session, start];
            if !started.as_bool() {
                return Err("the sign-in sheet would not open".to_string());
            }
            SIGNING_IN.with(|slot| *slot.borrow_mut() = Some((session, anchor)));
        }
        Ok(())
    }

    /// Sign in to Krate, inside Krate: krate.tech's sign-in page (GitHub,
    /// Google or email) in iOS's sign-in sheet over Krate. The page hands the
    /// account back to `krate://signed-in` with the nonce Krate sent, the
    /// sheet closes itself, and the person is signed in. Cancel says nothing.
    fn sign_in(data_root: &Path, private: &Path, art_dir: &Path) {
        let Some(nonce) = crate::account::new_nonce() else {
            notice(
                data_root,
                "Could not start signing in",
                "This phone did not give Krate a random number. Try again.",
            );
            return;
        };
        match web_sign_in(&crate::account::web_sign_in_url(&nonce)) {
            Ok(Some(back)) => match crate::account::keep_web_sign_in(&back, &nonce, private) {
                Ok(()) => {
                    krate_adapter_ios::uikit_native::show_loading("Krate", "Signing in");
                    refresh_account(private, art_dir);
                }
                Err(why) => notice(data_root, "Not signed in", &why),
            },
            Ok(None) => {}
            Err(why) => notice(
                data_root,
                "Could not start signing in",
                &format!("{why}. Try again."),
            ),
        }
    }

    /// Discover's rows from the catalogue as it stands, written whole then
    /// moved into place, so the screen never reads half a list.
    fn write_discover(ui: &Path, art_dir: &Path) {
        let rows: Vec<String> = crate::hub::catalogue(ui)
            .iter()
            .map(|app| {
                crate::home::discover_row(
                    &app.id,
                    &app.name,
                    &app.author,
                    &app.category,
                    &app.description,
                    crate::hub::cached_art(app, art_dir).as_deref(),
                    app.size,
                )
            })
            .collect();
        let part = ui.join("discover.txt.part");
        if std::fs::write(&part, rows.join("\n") + "\n").is_ok() {
            let _ = std::fs::rename(&part, ui.join("discover.txt"));
        }
    }

    /// Fetch the hub's catalogue off the guest thread, telling Discover how
    /// it stands (hub-state.txt): loading, then ready, or offline when there
    /// is nothing to show. The home screen never waits on the network.
    fn refresh_hub(ui: &Path, art_dir: &Path) {
        if crate::hub::catalogue(ui).is_empty() {
            let _ = std::fs::write(ui.join("hub-state.txt"), "loading\n");
        }
        let (ui, art_dir) = (ui.to_path_buf(), art_dir.to_path_buf());
        std::thread::spawn(move || {
            crate::hub::refresh(&ui, &art_dir);
            // discover.txt first: the screen reloads it when the state turns.
            write_discover(&ui, &art_dir);
            let now = if crate::hub::catalogue(&ui).is_empty() {
                "offline"
            } else {
                "ready"
            };
            let _ = std::fs::write(ui.join("hub-state.txt"), format!("{now}\n"));
        });
    }

    /// Krate's own screens: Library, Discover, You, Settings and each app's
    /// page (apps/krate-home). The player prepares what they show -- the
    /// library, the hub catalogue, decoded screenshots, the account -- and
    /// acts on the answer, the same answers as Android's. None when the
    /// person leaves, which only `home:closed` (or no answer at all) is.
    fn home(library: &Path, data_root: &Path) -> Option<App> {
        let ui = data_root.join("home-ui");
        let art_dir = ui.join("art");
        let _ = std::fs::create_dir_all(&art_dir);
        let private = private_root();
        let mut start: Option<String> = None;
        // The hub's catalogue as last fetched, refreshed in the background.
        // Nothing waits on the network: with no catalogue yet, Discover shows
        // its loading rows (hub-state.txt) and fills in when it arrives.
        static REFRESHED: std::sync::Once = std::sync::Once::new();
        REFRESHED.call_once(|| refresh_hub(&ui, &art_dir));
        if crate::account::token(&private).is_some() {
            static ACCOUNT_REFRESHED: std::sync::Once = std::sync::Once::new();
            ACCOUNT_REFRESHED.call_once(|| {
                let (private, art_dir) = (private.clone(), art_dir.clone());
                std::thread::spawn(move || refresh_account(&private, &art_dir));
            });
        }
        loop {
            let hub_apps = crate::hub::catalogue(&ui);
            let hub_art: std::collections::HashMap<String, String> = hub_apps
                .iter()
                .filter_map(|app| Some((app.id.clone(), crate::hub::cached_art(app, &art_dir)?)))
                .collect();

            // The Library, in the order the screen numbers it: each app,
            // its manifest, the SHA-256 the hub knows it by, and its row.
            let mut apps: Vec<App> = Vec::new();
            let mut manifests: Vec<Option<krate_manifest::Manifest>> = Vec::new();
            let mut shas: Vec<Option<String>> = Vec::new();
            let mut rows: Vec<String> = Vec::new();
            #[cfg(krate_native_image)]
            if let Ok(manifest) = krate_manifest::Manifest::parse(native::MANIFEST) {
                let preview = preview_name(&App::Carried);
                let art = art_dir.join(&preview).exists().then_some(preview.as_str());
                rows.push(crate::screen::carried_row(
                    &manifest.app.name,
                    art,
                    crate::screen::app_data_size(data_root, &manifest.app.id),
                    &manifest.app.version,
                ));
                apps.push(App::Carried);
                manifests.push(Some(manifest));
                shas.push(None);
            }
            // Where the received apps start in the screen's numbering.
            let first_received = apps.len();
            for (path, name, sha, size) in library_apps(library) {
                // The app as it really looked the last time it ran on this
                // phone; else, for an app from the hub, its hub picture (the
                // hub names apps by the SHA-256 of their bytes).
                let preview = preview_name(&App::Received(path.clone()));
                let art = if art_dir.join(&preview).exists() {
                    Some(preview.as_str())
                } else {
                    hub_art.get(&sha).map(String::as_str)
                };
                let manifest = manifest_of(&path);
                let data = manifest
                    .as_ref()
                    .map(|m| crate::screen::app_data_size(data_root, &m.app.id))
                    .unwrap_or(0);
                let from_hub = hub_apps.iter().any(|app| app.id == sha);
                rows.push(crate::home::library_row_full(
                    &name,
                    size,
                    art,
                    from_hub,
                    data,
                    manifest.as_ref().map(|m| m.app.version.as_str()),
                ));
                apps.push(App::Received(path));
                manifests.push(manifest);
                shas.push(Some(sha));
            }
            let _ = std::fs::write(ui.join("library.txt"), rows.join("\n") + "\n");
            // What each Library app can use, for its page.
            let perms = ui.join("perms");
            let _ = std::fs::remove_dir_all(&perms);
            let _ = std::fs::create_dir_all(&perms);
            for (n, manifest) in manifests.iter().enumerate() {
                let uses: Vec<(String, bool)> = manifest
                    .iter()
                    .flat_map(|m| m.capabilities.iter())
                    .filter_map(|request| Some((use_in_words(&request.cap)?, request.required)))
                    .collect();
                let _ = std::fs::write(
                    perms.join(format!("{n}.txt")),
                    crate::home::perms_file(&uses),
                );
            }
            // Sync across the person's devices: what sync.txt says, from
            // this iPhone's files; the hub asked on a thread of its own. The
            // same folder, name and (no) machine key a run's sync uses.
            let phone = crate::sync_home::Phone {
                data_root: data_root.to_path_buf(),
                device_dir: private.join("account-sync"),
                token: crate::account::token(&private),
                machine_key: Vec::new(),
                name: device_name(),
                hub: crate::run_config::SHARED_HUB.to_string(),
            };
            let sync_apps = crate::sync_home::lib_apps(&manifests);
            let (sync_view, asked) = crate::sync_home::view(&phone, &sync_apps);
            crate::sync_home::write(&ui, &sync_view);
            crate::sync_home::check_in_background(&phone, &sync_apps, &ui, false);
            let args = crate::sync_home::recovery_code(&phone)
                .map(|code| vec!["sync-code".to_string(), code])
                .unwrap_or_default();
            // Who is signed in, for the profile tab.
            let account = if crate::account::token(&private).is_some() {
                ACCOUNT.lock().expect("account lock").clone().or_else(|| {
                    Some(crate::home::Account {
                        login: "you".to_string(),
                        ..Default::default()
                    })
                })
            } else {
                None
            };
            let _ = std::fs::write(
                ui.join("account.txt"),
                crate::home::account_file(
                    account.as_ref(),
                    &device_name(),
                    crate::screen::storage_used(data_root),
                    env!("CARGO_PKG_VERSION"),
                ),
            );
            write_discover(&ui, &art_dir);
            match start.take() {
                Some(tab) => {
                    let _ = std::fs::write(ui.join("start.txt"), tab);
                }
                None => {
                    let _ = std::fs::remove_file(ui.join("start.txt"));
                }
            }

            // Only `home:closed`, or no answer at all (a crash, or the
            // screen closed for a file opened from elsewhere), leaves.
            let answer = run_trusted(HOME_WASM, HOME_MANIFEST, &ui, args)?;
            match crate::home::home_answer(&answer) {
                HomeAnswer::Open(n) => {
                    return (n < apps.len()).then(|| apps.swap_remove(n));
                }
                HomeAnswer::Get(id) => {
                    let name = crate::hub::catalogue(&ui)
                        .iter()
                        .find(|app| app.id == id)
                        .map(|app| app.name.clone())
                        .unwrap_or_else(|| "The app".to_string());
                    krate_adapter_ios::uikit_native::show_loading(&name, "Getting it from the hub");
                    match fetch_into(&id, data_root) {
                        Ok(file) => {
                            if let Some(app) = receive(&file, library, data_root) {
                                return Some(app);
                            }
                        }
                        Err(why) => notice(
                            data_root,
                            "Could not download this app",
                            &format!("{why}. Check the connection and try again."),
                        ),
                    }
                    start = Some("discover".to_string());
                }
                HomeAnswer::Files => {
                    if let Some(path) = pick_file() {
                        if let Some(app) = receive(&path, library, data_root) {
                            return Some(app);
                        }
                    }
                }
                HomeAnswer::SignIn => {
                    sign_in(data_root, &private, &art_dir);
                    start = Some("you".to_string());
                }
                HomeAnswer::SignOut => {
                    crate::account::sign_out(&private);
                    *ACCOUNT.lock().expect("account lock") = None;
                    let _ = std::fs::remove_file(art_dir.join("avatar.rgba"));
                    start = Some("you".to_string());
                }
                HomeAnswer::Remove(n) => match apps.get(n) {
                    Some(App::Received(path)) => {
                        if let Some(Some(manifest)) = manifests.get(n) {
                            crate::screen::clear_app_data(data_root, &manifest.app.id);
                        }
                        let _ = std::fs::remove_file(art_dir.join(preview_name(&apps[n])));
                        let _ = std::fs::remove_file(path);
                    }
                    #[cfg(krate_native_image)]
                    Some(App::Carried) => {
                        notice(
                            data_root,
                            "This app comes with Krate",
                            "It stays on this phone with Krate. You can clear what it keeps.",
                        );
                        start = Some(format!("app:{n}"));
                    }
                    None => {}
                },
                HomeAnswer::Clear(n) => {
                    if let Some(Some(manifest)) = manifests.get(n) {
                        crate::screen::clear_app_data(data_root, &manifest.app.id);
                    }
                    start = Some(format!("app:{n}"));
                }
                HomeAnswer::Share(n) => {
                    match (apps.get(n), manifests.get(n)) {
                        (Some(App::Received(path)), Some(Some(manifest))) => {
                            if let Ok(bytes) = std::fs::read(path) {
                                let name = format!(
                                    "{}.krate",
                                    manifest.app.name.replace(['/', '\\'], " ")
                                );
                                if let Err(why) = krate_adapter_ios::uikit_native::share(
                                    None,
                                    None,
                                    &[(name, bytes)],
                                ) {
                                    notice(data_root, "Could not share this app", &why);
                                }
                            }
                        }
                        #[cfg(krate_native_image)]
                        (Some(App::Carried), _) => notice(
                            data_root,
                            "This app comes with Krate",
                            "It is part of Krate itself, so there is no file to send.",
                        ),
                        _ => {}
                    }
                    start = Some(format!("app:{n}"));
                }
                HomeAnswer::Update(n) => {
                    // The hub lists each app's newest version under the same
                    // name: a different SHA-256 is a newer one.
                    // The catalogue as it is now: the one read when this
                    // screen opened may have been empty, still loading.
                    let newer = match (manifests.get(n), shas.get(n)) {
                        (Some(Some(manifest)), Some(Some(sha))) => crate::hub::catalogue(&ui)
                            .iter()
                            .find(|app| {
                                app.name.eq_ignore_ascii_case(&manifest.app.name) && app.id != *sha
                            })
                            .map(|app| app.id.clone()),
                        _ => None,
                    };
                    // The app's place after an update: a kept app is listed
                    // newest first, so the updated one moves to the top.
                    let mut lands = n;
                    match newer {
                        Some(id) => {
                            krate_adapter_ios::uikit_native::show_loading(
                                "Krate",
                                "Getting the newest version",
                            );
                            // A name is only a lead: the hub's copy replaces
                            // this one only when it is the same app (its own
                            // id), never a stranger's app sharing the name.
                            let wanted = match manifests.get(n) {
                                Some(Some(manifest)) => manifest.app.id.clone(),
                                _ => String::new(),
                            };
                            let same_app = |file: &Path| {
                                krate_bundle::open(file)
                                    .ok()
                                    .and_then(|open| {
                                        std::fs::read_to_string(open.manifest_path()).ok()
                                    })
                                    .and_then(|text| krate_manifest::Manifest::parse(&text).ok())
                                    .is_some_and(|m| !wanted.is_empty() && m.app.id == wanted)
                            };
                            match fetch_into(&id, data_root) {
                                Ok(file) if !same_app(&file) => {
                                    let _ = std::fs::remove_file(&file);
                                    notice(
                                        data_root,
                                        "Up to date",
                                        "This is the newest version on the hub.",
                                    );
                                }
                                Ok(file) => {
                                    if let Some(App::Received(path)) =
                                        receive(&file, library, data_root)
                                    {
                                        if let Some(at) = library_apps(library)
                                            .iter()
                                            .position(|(kept, ..)| *kept == path)
                                        {
                                            lands = first_received + at;
                                        }
                                        notice(
                                            data_root,
                                            "Updated",
                                            "This app is now the newest version on the hub.",
                                        );
                                    }
                                }
                                Err(why) => notice(
                                    data_root,
                                    "Could not update",
                                    &format!("{why}. Check the connection and try again."),
                                ),
                            }
                        }
                        None => notice(
                            data_root,
                            "Up to date",
                            "This is the newest version on the hub.",
                        ),
                    }
                    start = Some(format!("app:{lands}"));
                }
                HomeAnswer::Refresh => {
                    refresh_hub(&ui, &art_dir);
                    start = Some("discover".to_string());
                }
                HomeAnswer::Report => {
                    let link =
                        crate::screen::report_link(env!("CARGO_PKG_VERSION"), &device_name());
                    if let Err(why) = krate_adapter_ios::uikit_native::open_url(&link) {
                        notice(
                            data_root,
                            "Could not open Mail",
                            &format!("{why}. Write to yashraj@krate.tech and say what happened."),
                        );
                    }
                    start = Some("settings".to_string());
                }
                HomeAnswer::Sync(answer) => {
                    if matches!(
                        answer,
                        crate::home::SyncAnswer::Approve(_) | crate::home::SyncAnswer::Recover(_)
                    ) {
                        krate_adapter_ios::uikit_native::show_loading(
                            "Krate",
                            "Asking your account",
                        );
                    }
                    let acted = crate::sync_home::act(&phone, &sync_apps, &asked, &answer);
                    if let Some((title, words)) = &acted.notice {
                        notice(data_root, title, words);
                    }
                    if acted.recheck {
                        crate::sync_home::check_in_background(&phone, &sync_apps, &ui, true);
                    }
                    start = Some(acted.start);
                }
                // A newer screen's answer: show the home screen again.
                HomeAnswer::Stay => {
                    eprintln!("krate: the home screen answered something this Krate does not know; staying");
                }
                HomeAnswer::Closed => return None,
            }
        }
    }

    /// Download an app from the hub into Documents/Downloads, for `receive`
    /// to keep (it removes the download once kept).
    fn fetch_into(id: &str, data_root: &Path) -> Result<PathBuf, String> {
        let bytes = crate::hub::download(id, MAX_BUNDLE_BYTES)?;
        let downloads = data_root.join("Downloads");
        let _ = std::fs::create_dir_all(&downloads);
        let file = downloads.join(format!("{}.krate", crate::home::hub_id(id)));
        std::fs::write(&file, bytes).map_err(|e| format!("could not save it ({e})"))?;
        Ok(file)
    }

    /// The iOS Files picker, for a `.krate` saved anywhere Files can see.
    /// iOS copies the pick into the app (import mode); None when cancelled.
    fn pick_file() -> Option<PathBuf> {
        let (tx, rx) = std::sync::mpsc::channel::<Option<PathBuf>>();
        dispatch2::DispatchQueue::main().exec_async(move || present_picker(tx));
        rx.recv().ok().flatten()
    }

    pub struct PickerState {
        tx: std::cell::RefCell<Option<std::sync::mpsc::Sender<Option<PathBuf>>>>,
        window: std::cell::RefCell<Option<objc2::rc::Retained<UIWindow>>>,
    }

    define_class!(
        // SAFETY:
        // - NSObject has no subclassing requirements for a delegate.
        // - UIKit calls the picker's delegate on the main thread.
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = PickerState]
        struct KratePickerDelegate;

        unsafe impl NSObjectProtocol for KratePickerDelegate {}

        unsafe impl UIDocumentPickerDelegate for KratePickerDelegate {
            #[unsafe(method(documentPicker:didPickDocumentsAtURLs:))]
            fn picked(
                &self,
                _controller: &objc2_ui_kit::UIDocumentPickerViewController,
                urls: &objc2_foundation::NSArray<NSURL>,
            ) {
                let path = urls
                    .firstObject()
                    .and_then(|url| url.path())
                    .map(|p| PathBuf::from(p.to_string()));
                self.finish(path);
            }

            #[unsafe(method(documentPickerWasCancelled:))]
            fn cancelled(&self, _controller: &objc2_ui_kit::UIDocumentPickerViewController) {
                self.finish(None);
            }
        }
    );

    impl KratePickerDelegate {
        fn finish(&self, path: Option<PathBuf>) {
            if let Some(window) = self.ivars().window.borrow_mut().take() {
                window.setHidden(true);
            }
            if let Some(tx) = self.ivars().tx.borrow_mut().take() {
                let _ = tx.send(path);
            }
            PICKER.with(|slot| slot.borrow_mut().take());
        }
    }

    thread_local! {
        /// The live picker delegate: UIKit holds delegates weakly.
        static PICKER: std::cell::RefCell<Option<objc2::rc::Retained<KratePickerDelegate>>> =
            const { std::cell::RefCell::new(None) };
    }

    #[allow(deprecated)]
    fn present_picker(tx: std::sync::mpsc::Sender<Option<PathBuf>>) {
        let Some(mtm) = MainThreadMarker::new() else {
            let _ = tx.send(None);
            return;
        };
        let Some(window) = overlay_window(mtm) else {
            let _ = tx.send(None);
            return;
        };
        let types = objc2_foundation::NSArray::from_retained_slice(&[NSString::from_str(
            "dev.krate.bundle",
        )]);
        let picker = objc2_ui_kit::UIDocumentPickerViewController::initWithDocumentTypes_inMode(
            objc2_ui_kit::UIDocumentPickerViewController::alloc(mtm),
            &types,
            objc2_ui_kit::UIDocumentPickerMode::Import,
        );
        let delegate: objc2::rc::Retained<KratePickerDelegate> = {
            let this = KratePickerDelegate::alloc(mtm).set_ivars(PickerState {
                tx: std::cell::RefCell::new(Some(tx)),
                window: std::cell::RefCell::new(Some(window.clone())),
            });
            unsafe { objc2::msg_send![super(this), init] }
        };
        picker.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*delegate)));
        PICKER.with(|slot| *slot.borrow_mut() = Some(delegate));
        if let Some(root) = window.rootViewController() {
            root.presentViewController_animated_completion(&picker, true, None);
        }
    }

    /// A window above everything, for a system sheet (an alert, a picker).
    fn overlay_window(mtm: MainThreadMarker) -> Option<objc2::rc::Retained<UIWindow>> {
        let scene = UIApplication::sharedApplication(mtm)
            .connectedScenes()
            .allObjects()
            .into_iter()
            .find_map(|scene| scene.downcast::<UIWindowScene>().ok())?;
        let window = UIWindow::initWithWindowScene(UIWindow::alloc(mtm), &scene);
        let root = UIViewController::new(mtm);
        unsafe {
            window.setWindowLevel(objc2_ui_kit::UIWindowLevelAlert);
        }
        window.setRootViewController(Some(&root));
        window.makeKeyAndVisible();
        Some(window)
    }

    /// The file an app's preview is kept in, in the home screen's art folder:
    /// one per library file, so a newer copy of an app replaces its preview.
    /// (`screen-`, not the first builds' `preview-`: those could hold a
    /// picture of the Library itself and are left unread.)
    fn preview_name(app: &App) -> String {
        match app {
            #[cfg(krate_native_image)]
            App::Carried => "screen-carried.rgba".to_string(),
            App::Received(path) => format!(
                "screen-{}.rgba",
                sanitize(
                    &path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                )
            ),
        }
    }

    /// The app is gone: Krate's menu, its name and grants go with it.
    fn leave_app() {
        krate_adapter_ios::uikit_native::set_app_chrome(false);
        krate_adapter_ios::uikit_native::set_current_app(None);
        krate_adapter_ios::uikit_native::set_current_name(None);
        krate_adapter_ios::uikit_native::set_current_uses(None);
    }

    /// Ask, then run. Every open asks: the answer is never remembered.
    fn launch(app: App, data_root: &Path) {
        // The screen the person tapped from, blurred, for the permission
        // sheet to sit over (F-314): taken now, before anything covers it.
        let _ = std::fs::create_dir_all(data_root.join("wall"));
        krate_adapter_ios::uikit_native::capture_behind(
            &data_root.join("wall").join("behind.rgba"),
        );
        let preview_path = data_root
            .join("home-ui")
            .join("art")
            .join(preview_name(&app));
        // What runs, and its manifest.
        let mut _opened_bundle = None;
        let (manifest_text, wasm, assets_root) = match &app {
            #[cfg(krate_native_image)]
            App::Carried => (native::MANIFEST.to_string(), None, None),
            App::Received(path) => {
                let open = match krate_bundle::open(path) {
                    Ok(open) => open,
                    Err(err) => {
                        notice(data_root, "This app did not open", &format!("{err}"));
                        return;
                    }
                };
                let manifest = std::fs::read_to_string(open.manifest_path()).unwrap_or_default();
                let wasm = std::fs::read(open.component_path()).ok();
                let assets = open.assets_path().map(Path::to_path_buf);
                _opened_bundle = Some(open);
                (manifest, wasm, assets)
            }
        };
        let manifest = match krate_manifest::Manifest::parse(&manifest_text) {
            Ok(manifest) => manifest,
            Err(_) => {
                notice(
                    data_root,
                    "This app did not open",
                    "Its description could not be read.",
                );
                return;
            }
        };
        // Which app runs, for its reminders: their ids are namespaced by the
        // app's id, and a tapped one opens it again by this name.
        let opens = match &app {
            #[cfg(krate_native_image)]
            App::Carried => "carried".to_string(),
            App::Received(path) => path.to_string_lossy().into_owned(),
        };
        krate_adapter_ios::uikit_native::set_current_app(Some((manifest.app.id.clone(), opens)));
        // Krate's menu from the permission sheet on (IC-934): the sheet and
        // the opening cover are where a stuck app is first noticed, and Close
        // there means "don't open it".
        krate_adapter_ios::uikit_native::set_current_name(Some(manifest.app.name.clone()));
        krate_adapter_ios::uikit_native::set_current_uses(None);
        krate_adapter_ios::uikit_native::set_app_chrome(true);

        // The line under the app's name on the permission sheet.
        let detail = match &app {
            #[cfg(krate_native_image)]
            App::Carried => format!("Version {} \u{00b7} Built in", manifest.app.version),
            App::Received(path) => match std::fs::metadata(path) {
                Ok(meta) => format!(
                    "Version {} \u{00b7} {}",
                    manifest.app.version,
                    human_size(meta.len())
                ),
                Err(_) => format!("Version {}", manifest.app.version),
            },
        };
        let Some(granted) = ask_on_wall(data_root, &manifest, &detail) else {
            leave_app();
            return;
        };
        let declared = match manifest.declared_capabilities() {
            Ok(declared) => declared,
            Err(err) => {
                leave_app();
                notice(data_root, "This app did not open", &format!("{err:#}"));
                return;
            }
        };
        let kept: Vec<_> = manifest
            .capabilities
            .iter()
            .zip(declared)
            .filter(|(request, _)| request.required || granted.contains(&request.cap))
            .map(|(_, capability)| capability)
            .collect();
        // For Krate's menu (IC-934): the app's name, and what it may use this
        // run in the permission sheet's own words.
        let mut uses: Vec<String> = manifest
            .capabilities
            .iter()
            .filter(|request| request.required || granted.contains(&request.cap))
            .filter_map(|request| use_in_words(&request.cap))
            .collect();
        uses.dedup();
        krate_adapter_ios::uikit_native::set_current_uses(Some(uses));

        let records = kept
            .iter()
            .any(|cap| cap.module() == "audio" && cap.action() == "capture");
        record_session(records);
        let sandbox = data_root.join("apps").join(sanitize(&manifest.app.id));
        let _ = std::fs::create_dir_all(&sandbox);
        // The app's key-value store and database: the runtime opens them only
        // when told where, and the phone players never said, so every
        // store.kv and store.sql call was refused and a recipe book made in
        // Studio quit at start. Kept beside the sandbox, not in it, as on a
        // desktop: the app names keys, never files.
        let stores = data_root.join("stores");
        let _ = std::fs::create_dir_all(&stores);
        let store = stores.join(format!("{}.kv", sanitize(&manifest.app.id)));
        let files = crate::run_config::writes_files(&manifest).then(|| sandbox.clone());
        let mut config = krate_runtime::Config {
            session_policy: krate_policy::SessionPolicy::from_grants(kept),
            sandbox_root: sandbox,
            app_store_path: Some(store.clone()),
            app_database_path: Some(store.with_extension("sqlite")),
            bundle_assets_root: assets_root,
            phase3_ui_mode: krate_runtime::phase3_ui::Phase3HostUiMode::NativePrototype,
            ..Default::default()
        };
        // The same data on every device the person signs in on (IC-981):
        // signed in on this iPhone, the app's store.kv syncs by their account.
        // The device key rests beside the sign-in, in Application Support,
        // which no other app reads and the Files app never shows (no machine
        // key on the iPhone yet).
        let verified = _opened_bundle
            .as_ref()
            .and_then(crate::run_config::verified_publisher);
        let private = private_root();
        config.account_sync = crate::run_config::sync_plan(
            &private,
            verified.as_deref(),
            &store,
            crate::account::token(&private),
        )
        .map(
            |plan| krate_runtime::account_sync_host::AccountSyncContext {
                hub: plan.hub.to_string(),
                token: plan.token,
                device_dir: plan.device_dir,
                device_name: device_name(),
                machine_key: Vec::new(),
                app_id: manifest.app.id.clone(),
                publisher: plan.publisher,
                enabled: plan.enabled,
                background: true,
                // Its own folder: its files follow the person too (S3).
                files,
            },
        );
        if let Some(mib) = memory_budget_mib(&manifest) {
            config.memory_bytes = mib * 1024 * 1024;
        }

        // The carried native code runs when the app is the carried one --
        // tapped on the home screen, or arriving with the very same code.
        #[cfg(krate_native_image)]
        let native = match &wasm {
            None => true,
            Some(bytes) => krate_bundle::sha256_hex(bytes) == native::SHA256,
        };
        #[cfg(not(krate_native_image))]
        let native = false;

        // The cover while the app starts -- the screen turning, an engine
        // loading -- lifting on the app's first frame at the screen's real
        // size; Krate's close capsule over the app, the way back home.
        krate_adapter_ios::uikit_native::show_loading(&manifest.app.name, "Opening");
        // A picture of the app as it really looks, for its home-screen card,
        // taken a few seconds into its own run (once per run). Asked only
        // now, past the permission sheet: asked earlier, a slow answer on the
        // sheet or a Don't Allow took the picture of Krate's own Library.
        krate_adapter_ios::uikit_native::capture_preview(
            preview_path,
            std::time::Duration::from_millis(4000),
        );
        let outcome = if native {
            run_native(&config)
        } else {
            let Some(wasm) = wasm else {
                leave_app();
                return;
            };
            krate_runtime::Runtime::new(&config).and_then(|runtime| {
                runtime.run_bytes_for_world(&wasm, &config, krate_runtime::RuntimeWorld::Gui)
            })
        };
        krate_adapter_ios::uikit_native::cancel_preview();
        leave_app();
        if records {
            record_session(false);
        }
        // Back home: the cover until Krate's own screen has drawn.
        krate_adapter_ios::uikit_native::show_loading("Krate", "");
        match outcome {
            Ok(outcome) => eprintln!("app finished: {outcome:?}"),
            Err(err) => {
                eprintln!("app failed: {err:#}");
                notice(
                    data_root,
                    &format!("{} stopped", manifest.app.name),
                    &format!("{err:#}"),
                );
            }
        }
    }

    #[cfg(krate_native_image)]
    fn run_native(
        config: &krate_runtime::Config,
    ) -> krate_runtime::Result<krate_runtime::RunOutcome> {
        let runtime = krate_runtime::Runtime::new_for_native_image(config)?;
        let started = std::time::Instant::now();
        // SAFETY: the image was made by this Krate's precompile_native_image
        // and sits in this binary's signed, executable text for the life of
        // the process.
        let loaded = unsafe { runtime.load_native_image(&native::IMAGE.0) }?;
        eprintln!(
            "native image loaded in {:.1} ms",
            started.elapsed().as_secs_f64() * 1000.0
        );
        runtime.run_loaded_for_world(&loaded, config, krate_runtime::RuntimeWorld::Gui)
    }

    #[cfg(not(krate_native_image))]
    fn run_native(
        _config: &krate_runtime::Config,
    ) -> krate_runtime::Result<krate_runtime::RunOutcome> {
        unreachable!("no native app in this build")
    }

    /// Ask on Krate's own permission sheet (apps/krate-wall): the app's
    /// name, then each request's capability, its reason and whether the app
    /// needs it. Returns what the person allowed, or None when they did not
    /// open the app. An app that asks for nothing opens without asking.
    fn ask_on_wall(
        data_root: &Path,
        manifest: &krate_manifest::Manifest,
        detail: &str,
    ) -> Option<Vec<String>> {
        if manifest.capabilities.is_empty() {
            return Some(Vec::new());
        }
        // `wall:ask`, the name, then the line under it as its own argument,
        // so a name cannot forge that line.
        let mut args = vec![
            "wall:ask".to_string(),
            field(&manifest.app.name),
            field(detail),
        ];
        for request in &manifest.capabilities {
            args.push(field(&request.cap));
            args.push(field(&request.rationale));
            args.push(if request.required { "1" } else { "0" }.to_string());
        }
        let answer = run_wall(data_root, args)?;
        let caps = answer.trim().strip_prefix("wall:open:")?;
        Some(
            caps.split(',')
                .filter(|cap| !cap.is_empty())
                .map(str::to_string)
                .collect(),
        )
    }

    /// Run the wall guest -- Krate's own trusted screens -- and return what
    /// it answered.
    fn run_wall(data_root: &Path, args: Vec<String>) -> Option<String> {
        run_trusted(WALL_WASM, WALL_MANIFEST, &data_root.join("wall"), args)
    }

    /// Run one of Krate's own screens (the wall, the home screen) with
    /// everything its manifest declares, in `sandbox`, and return what it
    /// printed.
    fn run_trusted(
        wasm: &[u8],
        manifest: &str,
        sandbox: &Path,
        args: Vec<String>,
    ) -> Option<String> {
        let manifest = krate_manifest::Manifest::parse(manifest).ok()?;
        let policy = krate_policy::SessionPolicy::allow_all_declared(&manifest).ok()?;
        let _ = std::fs::create_dir_all(sandbox);
        let config = krate_runtime::Config {
            session_policy: policy,
            sandbox_root: sandbox.to_path_buf(),
            app_args: args,
            phase3_ui_mode: krate_runtime::phase3_ui::Phase3HostUiMode::NativePrototype,
            ..Default::default()
        };
        let runtime = krate_runtime::Runtime::new(&config).ok()?;
        let (_, stdout) = runtime
            .run_bytes_captured_for_world(wasm, &config, krate_runtime::RuntimeWorld::Gui)
            .map_err(|err| eprintln!("{} failed: {err:#}", manifest.app.name))
            .ok()?;
        Some(String::from_utf8_lossy(&stdout).into_owned())
    }

    /// Tell the person why something did not open, and wait for Close.
    fn notice(data_root: &Path, title: &str, message: &str) {
        let _ = run_wall(
            data_root,
            vec![
                "wall:notice".to_string(),
                title.replace(['\n', '\0'], " "),
                message.replace(['\n', '\0'], " "),
            ],
        );
    }

    /// `memory.budget:<MiB>`, if the app declared it. precompile_native_image
    /// reads it the same way: it is part of a native image's memory layout.
    fn memory_budget_mib(manifest: &krate_manifest::Manifest) -> Option<u64> {
        manifest
            .capabilities
            .iter()
            .find_map(|c| c.cap.strip_prefix("memory.budget:")?.parse::<u64>().ok())
    }
}

#[cfg(target_os = "ios")]
mod hub;

// Krate's home screen contract -- the Library and Discover rows, the account
// file, the answers -- and where an app's data lives: the Android player's
// own code, included rather than copied, so both phones read one contract.
#[cfg(any(target_os = "ios", test))]
#[allow(dead_code)]
#[path = "../../player-android/src/home.rs"]
mod home;
#[cfg(any(target_os = "ios", test))]
#[allow(dead_code)]
#[path = "../../player-android/src/run_config.rs"]
mod run_config;

// Signing in to Krate, and the account the profile tab shows (shared too).
#[cfg(target_os = "ios")]
#[path = "../../player-android/src/account.rs"]
mod account;

// Sync on Krate's home (IC-981 S2): the same file both phones read.
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
#[path = "../../player-android/src/sync_home.rs"]
mod sync_home;

// What the home screen's answers mean on an iPhone, and each app's data.
#[cfg(any(target_os = "ios", test))]
mod screen;

#[cfg(target_os = "ios")]
fn main() {
    player::main();
}

#[cfg(not(target_os = "ios"))]
fn main() {
    eprintln!("the Krate iOS player only runs on iOS");
}

//! Krate runtime: Phase 1 proof of concept.
//!
//! Phase 1 intentionally exposes only one temporary host interface:
//! `krate:phase1/host` with `print(string)` and `exit(s32)`.

use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs::File,
    io::SeekFrom,
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, UNIX_EPOCH},
};

use thiserror::Error;
use wasmtime::{
    component::{Component, HasSelf},
    Engine, ResourceLimiter, Store, Trap,
};

pub mod phase3_ui;
pub mod uapi;
pub mod uapi_dispatch;

pub mod embed;
#[cfg(feature = "phase2-bindings")]
pub mod phase2_bindings;
#[cfg(feature = "phase2-bindings")]
pub mod phase2_bridge;
#[cfg(feature = "phase2-bindings")]
pub mod phase2_host;
#[cfg(feature = "phase2-bindings")]
pub mod phase3_gui_bindings;
#[cfg(feature = "phase2-bindings")]
pub mod phase3_gui_host;

#[cfg(feature = "phase2-bindings")]
use krate_adapter_common::locale::{
    DateStyle as HostDateStyle, HostLocale, LocaleId as HostLocaleId,
    NumberStyle as HostNumberStyle,
};
#[cfg(feature = "phase2-bindings")]
use krate_adapter_common::net::{
    build_plain_http_request, normalize_resolved_socket_addrs, parse_plain_http_response,
    read_plain_http_response_limited, PlainHttpError, PlainHttpHeader, PlainHttpMethod,
    PlainHttpReadError, PlainHttpRequest, PlainHttpUrl,
};
#[cfg(feature = "phase2-bindings")]
use krate_adapter_common::path::{FsOperation, LogicalPath, PathError};
#[cfg(feature = "phase2-bindings")]
use krate_adapter_common::time::{HostClock, TimeError};
use krate_policy::SessionPolicy;
use uapi::UapiGuard;
use uapi_dispatch::{
    AdapterError, DateStyle, FileHandle, FileStat, FsAdapter, Header, HostAdapter, HttpRequest,
    HttpResponse, IoAdapter, LocaleAdapter, LocaleId, NetAdapter, OpenMode, TimeAdapter,
};

#[cfg(all(feature = "phase2-bindings", target_os = "linux"))]
use krate_adapter_linux as host_os_adapter;
#[cfg(all(feature = "phase2-bindings", target_os = "macos"))]
use krate_adapter_macos as host_os_adapter;
#[cfg(all(feature = "phase2-bindings", target_os = "windows"))]
use krate_adapter_windows as host_os_adapter;

pub const DEFAULT_MAX_HTTP_RESPONSE_BYTES: usize = 1024 * 1024;
pub const DEFAULT_HTTP_TIMEOUT_MILLIS: u32 = 5_000;
#[cfg(feature = "phase2-bindings")]
const MAX_PHASE2_READ_BYTES: usize = 8 * 1024 * 1024;
#[cfg(feature = "phase2-bindings")]
const MAX_PHASE2_WRITE_BYTES: usize = 8 * 1024 * 1024;
#[cfg(feature = "phase2-bindings")]
const MAX_PHASE2_LIST_ENTRIES: usize = 4096;
#[cfg(feature = "phase2-bindings")]
const MAX_PHASE2_ARG_COUNT: usize = 1024;
#[cfg(feature = "phase2-bindings")]
const MAX_PHASE2_ARGS_RAW_BYTES: usize = 64 * 1024;
#[cfg(feature = "phase2-bindings")]
const MAX_PHASE2_OPEN_RESOURCES: usize = 1024;

wasmtime::component::bindgen!({
    path: "../../wit/krate",
    world: "app",
});

/// Runtime configuration for a single `krate run` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Optional Wasmtime fuel budget. `None` means no fuel limit.
    pub fuel: Option<u64>,
    /// Linear-memory cap in bytes.
    pub memory_bytes: u64,
    /// Session-scoped UCap grants used by Phase 2 UAPI dispatch.
    pub session_policy: SessionPolicy,
    /// Optional fixed wall-clock time for deterministic test runs.
    pub test_time_millis: Option<u64>,
    /// Optional locale override for deterministic test runs.
    pub test_locale: Option<String>,
    /// Optional timezone override for deterministic test runs.
    pub test_timezone: Option<String>,
    /// Arguments exposed to Phase 2 apps through `krate:io/args`.
    pub app_args: Vec<String>,
    /// Maximum full HTTP response size accepted by the local Phase 2 adapter.
    pub max_http_response_bytes: usize,
    /// Default timeout for Phase 2 HTTP requests from helper UAPI calls.
    pub default_http_timeout_millis: Option<u32>,
    /// Root directory used to resolve relative Phase 2 filesystem paths.
    pub sandbox_root: PathBuf,
    /// Host UI backend mode for Phase 3 `gui` world runs.
    pub phase3_ui_mode: phase3_ui::Phase3HostUiMode,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            fuel: None,
            memory_bytes: 256 * 1024 * 1024,
            session_policy: SessionPolicy::default(),
            test_time_millis: None,
            test_locale: None,
            test_timezone: None,
            app_args: Vec::new(),
            max_http_response_bytes: DEFAULT_MAX_HTTP_RESPONSE_BYTES,
            default_http_timeout_millis: Some(DEFAULT_HTTP_TIMEOUT_MILLIS),
            sandbox_root: PathBuf::from("."),
            phase3_ui_mode: phase3_ui::Phase3HostUiMode::HeadlessDraft,
        }
    }
}

/// Result of a completed runtime invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    Exited(i32),
    LimitExceeded(String),
}

/// Errors surfaced by the Phase 1 runtime.
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("failed to create Wasmtime engine: {0}")]
    EngineInit(String),
    #[error("failed to read wasm input: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid wasm component: {0}")]
    InvalidComponent(String),
    #[error("failed to instantiate component: {0}")]
    Instantiate(String),
    #[error("component does not export a callable `run` function")]
    MissingRunExport,
    #[error("component trapped while running: {0}")]
    Trap(String),
}

pub type Result<T> = std::result::Result<T, RuntimeError>;

/// Reusable runtime handle.
pub struct Runtime {
    engine: Engine,
}

impl Runtime {
    pub fn new(config: &Config) -> Result<Self> {
        let mut wt_config = wasmtime::Config::new();
        wt_config.wasm_component_model(true);

        if config.fuel.is_some() {
            wt_config.consume_fuel(true);
        }

        let engine =
            Engine::new(&wt_config).map_err(|err| RuntimeError::EngineInit(err.to_string()))?;

        Ok(Self { engine })
    }

    pub fn run_file(&self, path: impl AsRef<Path>, config: &Config) -> Result<RunOutcome> {
        let bytes = read_path_on_host(path.as_ref())?;
        self.run_bytes(&bytes, config)
    }

    pub fn run_bytes(&self, bytes: &[u8], config: &Config) -> Result<RunOutcome> {
        self.run_bytes_with_output(bytes, config, OutputMode::Stdout)
    }

    pub fn run_bytes_silent(&self, bytes: &[u8], config: &Config) -> Result<RunOutcome> {
        self.run_bytes_with_output(bytes, config, OutputMode::Sink)
    }

    /// Run a component while capturing everything it writes to stdout.
    ///
    /// This is the embedding entry point: no terminal is touched, and the
    /// caller receives the app's stdout bytes next to the run outcome.
    pub fn run_bytes_captured(
        &self,
        bytes: &[u8],
        config: &Config,
    ) -> Result<(RunOutcome, Vec<u8>)> {
        let buffer = Rc::new(RefCell::new(Vec::new()));
        let outcome =
            self.run_bytes_with_output(bytes, config, OutputMode::Capture(buffer.clone()))?;
        let captured = buffer.borrow().clone();
        Ok((outcome, captured))
    }

    /// Run a component file while capturing its stdout. See [`Self::run_bytes_captured`].
    pub fn run_file_captured(
        &self,
        path: impl AsRef<Path>,
        config: &Config,
    ) -> Result<(RunOutcome, Vec<u8>)> {
        let bytes = read_path_on_host(path.as_ref())?;
        self.run_bytes_captured(&bytes, config)
    }

    pub fn load_component(&self, bytes: &[u8]) -> Result<LoadedComponent> {
        let component = Component::from_binary(&self.engine, bytes)
            .map_err(|err| RuntimeError::InvalidComponent(err.to_string()))?;

        Ok(LoadedComponent { component })
    }

    pub fn run_loaded_silent(
        &self,
        component: &LoadedComponent,
        config: &Config,
    ) -> Result<RunOutcome> {
        self.run_component_with_output(component, config, OutputMode::Sink)
    }

    fn run_bytes_with_output(
        &self,
        bytes: &[u8],
        config: &Config,
        output: OutputMode,
    ) -> Result<RunOutcome> {
        let component = self.load_component(bytes)?;
        self.run_component_with_output(&component, config, output)
    }

    fn run_component_with_output(
        &self,
        component: &LoadedComponent,
        config: &Config,
        output: OutputMode,
    ) -> Result<RunOutcome> {
        match self.run_phase1_component(component, config, output.clone()) {
            Ok(outcome) => Ok(outcome),
            Err(err) => {
                #[cfg(feature = "phase2-bindings")]
                if matches!(err, RuntimeError::Instantiate(_)) {
                    return match self.run_phase2_component(component, config, output.clone()) {
                        Ok(outcome) => Ok(outcome),
                        // A Phase 3 GUI component imports krate:ui, which the
                        // CLI world linker cannot satisfy — fall through to the
                        // gui world path before giving up.
                        Err(RuntimeError::Instantiate(_)) => {
                            self.run_phase3_gui_component(component, config, output)
                        }
                        Err(err) => Err(err),
                    };
                }

                Err(err)
            }
        }
    }

    fn new_store(&self, config: &Config, output: OutputMode) -> Result<Store<HostState>> {
        let mut store = Store::new(&self.engine, HostState::new(config, output)?);
        store.limiter(|state| &mut state.limits);

        if let Some(fuel) = config.fuel {
            store
                .set_fuel(fuel)
                .map_err(|err| RuntimeError::EngineInit(err.to_string()))?;
        }

        Ok(store)
    }

    fn run_phase1_component(
        &self,
        component: &LoadedComponent,
        config: &Config,
        output: OutputMode,
    ) -> Result<RunOutcome> {
        let mut store = self.new_store(config, output)?;
        let mut linker = wasmtime::component::Linker::new(&self.engine);
        App::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)
            .map_err(|err| RuntimeError::Instantiate(err.to_string()))?;

        let bindings = match App::instantiate(&mut store, &component.component, &linker) {
            Ok(bindings) => bindings,
            Err(err) => {
                if let Some(message) = classify_limit_error(&err) {
                    return Ok(RunOutcome::LimitExceeded(message));
                }

                return Err(RuntimeError::Instantiate(err.to_string()));
            }
        };

        if let Err(err) = bindings.call_run(&mut store) {
            if let Some(message) = classify_limit_error(&err) {
                return Ok(RunOutcome::LimitExceeded(message));
            }

            return Err(RuntimeError::Trap(err.to_string()));
        }

        Ok(RunOutcome::Exited(store.data().exit_code.unwrap_or(0)))
    }

    #[cfg(feature = "phase2-bindings")]
    fn run_phase2_component(
        &self,
        component: &LoadedComponent,
        config: &Config,
        output: OutputMode,
    ) -> Result<RunOutcome> {
        let mut store = self.new_store(config, output)?;
        let mut linker = wasmtime::component::Linker::new(&self.engine);
        phase2_bindings::Cli::add_to_linker::<_, HasSelf<_>>(
            &mut linker,
            |state: &mut HostState| state.phase2(),
        )
        .map_err(|err| RuntimeError::Instantiate(err.to_string()))?;

        let bindings = phase2_bindings::Cli::instantiate(&mut store, &component.component, &linker)
            .map_err(|err| RuntimeError::Instantiate(err.to_string()))?;

        let code = match bindings.call_run(&mut store) {
            Ok(code) => code,
            Err(err) => {
                if let Some(message) = classify_limit_error(&err) {
                    return Ok(RunOutcome::LimitExceeded(message));
                }

                return Err(RuntimeError::Trap(err.to_string()));
            }
        };

        Ok(RunOutcome::Exited(code))
    }

    /// Run a Phase 3 `gui` world component.
    ///
    /// The Phase 2 import surface is served by the same `Phase2Host` the CLI
    /// world uses; the new `ui`/`gfx`/`audio` imports are served by
    /// [`phase3_gui_host::Phase3GuiHost`] over the UCap-gated Phase 3 UI
    /// dispatcher. The host UI mode comes from [`Config::phase3_ui_mode`]:
    /// headless draft by default, the opt-in native prototype on request.
    #[cfg(feature = "phase2-bindings")]
    fn run_phase3_gui_component(
        &self,
        component: &LoadedComponent,
        config: &Config,
        output: OutputMode,
    ) -> Result<RunOutcome> {
        use phase2_bindings::krate::{fs, io, locale, net, time};
        use phase3_gui_bindings::krate::{audio, gfx, ui};

        let mut store = self.new_store(config, output)?;
        let gui_host = phase3_gui_host::Phase3GuiHost::new(
            UapiGuard::new(config.session_policy.clone()),
            config.phase3_ui_mode,
        )
        .map_err(|err| RuntimeError::Instantiate(err.to_string()))?;
        store.data_mut().phase3_gui = Some(gui_host);

        let mut linker = wasmtime::component::Linker::new(&self.engine);

        macro_rules! link_phase2 {
            ($module:path) => {{
                use $module as interface;
                interface::add_to_linker::<_, HasSelf<_>>(&mut linker, |state: &mut HostState| {
                    state.phase2()
                })
                .map_err(|err| RuntimeError::Instantiate(err.to_string()))?;
            }};
        }
        macro_rules! link_gui {
            ($module:path) => {{
                use $module as interface;
                interface::add_to_linker::<_, HasSelf<_>>(&mut linker, |state: &mut HostState| {
                    state.phase3_gui()
                })
                .map_err(|err| RuntimeError::Instantiate(err.to_string()))?;
            }};
        }

        link_phase2!(io::types);
        link_phase2!(io::streams);
        link_phase2!(io::stdio);
        link_phase2!(io::args);
        link_phase2!(io::log);
        link_phase2!(fs::types);
        link_phase2!(fs::files);
        link_phase2!(net::types);
        link_phase2!(net::http_client);
        link_phase2!(time::clock);
        link_phase2!(time::sleep);
        link_phase2!(locale::info);
        link_phase2!(locale::format);

        link_gui!(ui::types);
        link_gui!(ui::window);
        link_gui!(ui::tree);
        link_gui!(ui::events);
        link_gui!(ui::dialog);
        link_gui!(ui::clipboard);
        link_gui!(ui::menu);
        link_gui!(gfx::types);
        link_gui!(gfx::canvas2d);
        link_gui!(gfx::gpu3d);
        link_gui!(audio::types);
        link_gui!(audio::playback);
        link_gui!(audio::capture);

        let bindings =
            phase3_gui_bindings::Gui::instantiate(&mut store, &component.component, &linker)
                .map_err(|err| RuntimeError::Instantiate(err.to_string()))?;

        let code = match bindings.call_run(&mut store) {
            Ok(code) => code,
            Err(err) => {
                if let Some(message) = classify_limit_error(&err) {
                    return Ok(RunOutcome::LimitExceeded(message));
                }

                return Err(RuntimeError::Trap(err.to_string()));
            }
        };

        Ok(RunOutcome::Exited(code))
    }
}

pub struct LoadedComponent {
    component: Component,
}

struct HostState {
    exit_code: Option<i32>,
    limits: Phase1Limits,
    output: Rc<RefCell<OutputMode>>,
    _uapi: UapiGuard,
    #[cfg(feature = "phase2-bindings")]
    phase2: phase2_host::Phase2Host<'static>,
    #[cfg(feature = "phase2-bindings")]
    phase3_gui: Option<phase3_gui_host::Phase3GuiHost>,
}

impl HostState {
    fn new(config: &Config, output: OutputMode) -> Result<Self> {
        let memory_bytes = usize::try_from(config.memory_bytes)
            .map_err(|_| RuntimeError::EngineInit("memory limit is too large".to_string()))?;
        let output = Rc::new(RefCell::new(output));
        create_dir_all_on_host(&config.sandbox_root)?;

        Ok(Self {
            exit_code: None,
            limits: Phase1Limits { memory_bytes },
            output: output.clone(),
            _uapi: UapiGuard::new(config.session_policy.clone()),
            #[cfg(feature = "phase2-bindings")]
            phase2: phase2_host::Phase2Host::new_with_http_timeout(
                UapiGuard::new(config.session_policy.clone()),
                Box::new(LocalPhase2Adapter::new(
                    output,
                    config.test_time_millis,
                    config.test_locale.clone(),
                    config.test_timezone.clone(),
                    config.app_args.clone(),
                    config.max_http_response_bytes,
                    config.sandbox_root.clone(),
                )),
                config.default_http_timeout_millis,
            ),
            #[cfg(feature = "phase2-bindings")]
            phase3_gui: None,
        })
    }

    #[cfg(feature = "phase2-bindings")]
    fn phase2(&mut self) -> &mut phase2_host::Phase2Host<'static> {
        &mut self.phase2
    }

    #[cfg(feature = "phase2-bindings")]
    fn phase3_gui(&mut self) -> &mut phase3_gui_host::Phase3GuiHost {
        self.phase3_gui
            .as_mut()
            .expect("phase3 gui host is only used by the gui world run path")
    }

    #[cfg(test)]
    fn uapi(&self) -> &UapiGuard {
        &self._uapi
    }
}

#[derive(Clone)]
enum OutputMode {
    Stdout,
    Sink,
    /// Collect app stdout into a shared buffer for embedding callers.
    Capture(Rc<RefCell<Vec<u8>>>),
}

impl OutputMode {
    fn print_line(&mut self, msg: &str) {
        match self {
            Self::Stdout => print_stdout_line_on_host(msg),
            Self::Sink => {}
            Self::Capture(buffer) => {
                let mut buffer = buffer.borrow_mut();
                buffer.extend_from_slice(msg.as_bytes());
                buffer.push(b'\n');
            }
        }
    }

    fn write_bytes(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Stdout => write_stdout_on_host(bytes),
            Self::Sink => Ok(()),
            Self::Capture(buffer) => {
                buffer.borrow_mut().extend_from_slice(bytes);
                Ok(())
            }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Stdout => flush_stdout_on_host(),
            Self::Sink => Ok(()),
            Self::Capture(_) => Ok(()),
        }
    }
}

struct Phase1Limits {
    memory_bytes: usize,
}

impl ResourceLimiter for Phase1Limits {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > self.memory_bytes {
            wasmtime::Result::Err(wasmtime::Error::msg(format!(
                "memory limit exceeded: requested {desired} bytes, limit is {} bytes",
                self.memory_bytes
            )))
        } else {
            Ok(true)
        }
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(maximum.is_none_or(|max| desired <= max))
    }
}

impl krate::phase1::host::Host for HostState {
    fn print(&mut self, msg: String) {
        self.output.borrow_mut().print_line(&msg);
    }

    fn exit(&mut self, code: i32) {
        self.exit_code = Some(code);
    }
}

#[cfg(feature = "phase2-bindings")]
struct LocalPhase2Adapter {
    output: Rc<RefCell<OutputMode>>,
    state: RefCell<LocalPhase2AdapterState>,
    clock: HostClock,
    locale: HostLocale,
    app_args: Vec<String>,
    max_http_response_bytes: usize,
    sandbox_root: PathBuf,
}

#[cfg(feature = "phase2-bindings")]
impl LocalPhase2Adapter {
    fn new(
        output: Rc<RefCell<OutputMode>>,
        test_time_millis: Option<u64>,
        test_locale: Option<String>,
        test_timezone: Option<String>,
        app_args: Vec<String>,
        max_http_response_bytes: usize,
        sandbox_root: PathBuf,
    ) -> Self {
        Self {
            output,
            state: RefCell::new(LocalPhase2AdapterState::default()),
            clock: discover_host_clock(test_time_millis),
            locale: discover_host_locale(test_locale.as_deref(), test_timezone.as_deref()),
            app_args,
            max_http_response_bytes,
            sandbox_root,
        }
    }

    fn insert_resource(
        &self,
        resource: LocalResource,
    ) -> std::result::Result<FileHandle, AdapterError> {
        let mut state = self.state.borrow_mut();
        if state.resources.len() >= MAX_PHASE2_OPEN_RESOURCES {
            return Err(AdapterError::Io(format!(
                "resource table exceeds limit ({MAX_PHASE2_OPEN_RESOURCES} handles)"
            )));
        }
        let id = match state.free_ids.pop() {
            Some(id) => id,
            None => {
                let id = state.next_id;
                state.next_id = state
                    .next_id
                    .checked_add(1)
                    .ok_or_else(|| AdapterError::Io("resource id overflow".to_string()))?;
                id
            }
        };
        state.resources.insert(id, resource);
        Ok(FileHandle::resource(id))
    }

    fn close_resource(&self, handle: &FileHandle) -> std::result::Result<(), AdapterError> {
        let mut state = self.state.borrow_mut();
        match state.resources.remove(&handle.id) {
            Some(_) => {
                state.free_ids.push(handle.id);
                Ok(())
            }
            None => Err(AdapterError::NotFound),
        }
    }

    fn resolve_fs_path(
        &self,
        path: &str,
        operation: FsOperation,
    ) -> std::result::Result<PathBuf, AdapterError> {
        let logical = normalize_fs_path(path)?;
        operation
            .validate_target(&logical)
            .map_err(map_path_error)?;
        let missing_leaf = missing_leaf_for_operation(operation);
        self.resolve_sandboxed_fs_path(&logical, missing_leaf)
    }

    fn resolve_sandboxed_fs_path(
        &self,
        path: &LogicalPath,
        missing_leaf: MissingLeaf,
    ) -> std::result::Result<PathBuf, AdapterError> {
        let root = canonicalize_path_on_host(self.sandbox_root.as_path()).map_err(map_io_error)?;
        let sandbox_relative = logical_path_to_sandbox_relative(path);
        let host_path = root.join(sandbox_relative);
        ensure_no_symlink_segments(&root, &host_path, missing_leaf)?;

        match canonicalize_path_on_host(host_path.as_path()) {
            Ok(real_path) => {
                ensure_path_in_sandbox(&root, &real_path)?;
                Ok(host_path)
            }
            Err(err)
                if err.kind() == std::io::ErrorKind::NotFound
                    && missing_leaf == MissingLeaf::Allow =>
            {
                if let Some(err) = map_existing_broken_leaf_error(&host_path, err)? {
                    return Err(err);
                }
                let parent = host_path.parent().ok_or(AdapterError::InvalidPath)?;
                let real_parent = canonicalize_path_on_host(parent).map_err(map_io_error)?;
                ensure_path_in_sandbox(&root, &real_parent)?;
                Ok(host_path)
            }
            Err(err) => Err(map_io_error(err)),
        }
    }
}

#[cfg(feature = "phase2-bindings")]
fn discover_host_locale(
    locale_override: Option<&str>,
    timezone_override: Option<&str>,
) -> HostLocale {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::discover_locale(locale_override, timezone_override)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        HostLocale::from_env_with_overrides(locale_override, timezone_override)
    }
}

#[cfg(feature = "phase2-bindings")]
fn discover_host_clock(test_time_millis: Option<u64>) -> HostClock {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::discover_clock(test_time_millis)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        HostClock::new(test_time_millis)
    }
}

#[cfg(feature = "phase2-bindings")]
fn sleep_on_host(millis: u32) {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::sleep_millis(millis);
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        HostClock::sleep_millis(millis);
    }
}

#[cfg(feature = "phase2-bindings")]
fn host_current_locale(locale: &HostLocale) -> HostLocaleId {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::current_locale(locale)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        locale.current()
    }
}

#[cfg(feature = "phase2-bindings")]
fn timezone_from_host_locale(locale: &HostLocale) -> String {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::timezone(locale)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        locale.timezone()
    }
}

#[cfg(feature = "phase2-bindings")]
fn format_date_on_host(
    millis: u64,
    timezone: &str,
    style: HostDateStyle,
    locale: &HostLocaleId,
) -> String {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::format_date(millis, timezone, style, locale)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        HostLocale::format_date(millis, timezone, style, locale)
    }
}

#[cfg(feature = "phase2-bindings")]
fn format_number_on_host(value: f64, style: HostNumberStyle, locale: &HostLocaleId) -> String {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::format_number(value, style, locale)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        HostLocale::format_number(value, style, locale)
    }
}

#[cfg(feature = "phase2-bindings")]
fn logical_path_to_sandbox_relative(path: &LogicalPath) -> PathBuf {
    let normalized = path.as_str();
    let relative = normalized.strip_prefix('/').unwrap_or(normalized);

    if relative.is_empty() || relative == "." {
        PathBuf::from(".")
    } else {
        relative.split('/').collect()
    }
}

#[cfg(feature = "phase2-bindings")]
fn bounded_read_len(n: u32) -> std::result::Result<usize, AdapterError> {
    let n = n as usize;
    if n > MAX_PHASE2_READ_BYTES {
        return Err(AdapterError::Io(format!(
            "read request exceeds limit ({MAX_PHASE2_READ_BYTES} bytes)"
        )));
    }
    Ok(n)
}

#[cfg(feature = "phase2-bindings")]
fn bounded_write_len(n: usize) -> std::result::Result<u32, AdapterError> {
    if n > MAX_PHASE2_WRITE_BYTES {
        return Err(AdapterError::Io(format!(
            "write request exceeds limit ({MAX_PHASE2_WRITE_BYTES} bytes)"
        )));
    }
    u32::try_from(n).map_err(|_| AdapterError::Io("write length overflow".to_string()))
}

#[cfg(feature = "phase2-bindings")]
fn encode_args_raw(app_args: &[String]) -> std::result::Result<String, AdapterError> {
    if app_args.len() > MAX_PHASE2_ARG_COUNT {
        return Err(AdapterError::Io(format!(
            "app arguments exceed count limit ({MAX_PHASE2_ARG_COUNT} arguments)"
        )));
    }

    let mut raw = String::new();
    for arg in app_args {
        if arg.is_empty() {
            return Err(AdapterError::Io(
                "app arguments cannot contain empty entries in Phase 2 raw args".to_string(),
            ));
        }
        if arg.contains('\0') || arg.contains('\n') {
            return Err(AdapterError::Io(
                "app argument contains unsupported raw args delimiter characters".to_string(),
            ));
        }
        raw.push_str(arg);
        raw.push('\n');
        if raw.len() > MAX_PHASE2_ARGS_RAW_BYTES {
            return Err(AdapterError::Io(format!(
                "app arguments exceed raw args limit ({MAX_PHASE2_ARGS_RAW_BYTES} bytes)"
            )));
        }
    }
    Ok(raw)
}

#[cfg(feature = "phase2-bindings")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MissingLeaf {
    Deny,
    Allow,
}

#[cfg(feature = "phase2-bindings")]
fn missing_leaf_for_operation(operation: FsOperation) -> MissingLeaf {
    if operation.allows_missing_leaf() {
        MissingLeaf::Allow
    } else {
        MissingLeaf::Deny
    }
}

#[cfg(feature = "phase2-bindings")]
fn ensure_path_in_sandbox(root: &Path, path: &Path) -> std::result::Result<(), AdapterError> {
    if path.starts_with(root) {
        Ok(())
    } else {
        Err(AdapterError::PermissionDenied)
    }
}

#[cfg(feature = "phase2-bindings")]
fn ensure_no_symlink_segments(
    root: &Path,
    host_path: &Path,
    missing_leaf: MissingLeaf,
) -> std::result::Result<(), AdapterError> {
    let relative = host_path
        .strip_prefix(root)
        .map_err(|_| AdapterError::PermissionDenied)?;
    let segments: Vec<_> = relative.components().collect();
    if segments.is_empty() {
        return Ok(());
    }

    let mut current = root.to_path_buf();
    for (index, segment) in segments.iter().enumerate() {
        current.push(segment.as_os_str());
        match symlink_metadata_on_host(&current) {
            Ok(metadata) => {
                if metadata_has_blocked_link_semantics(&metadata) {
                    return Err(AdapterError::PermissionDenied);
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let is_leaf = index + 1 == segments.len();
                if is_leaf && missing_leaf == MissingLeaf::Allow {
                    return Ok(());
                }
                return Err(map_io_error(err));
            }
            Err(err) => return Err(map_io_error(err)),
        }
    }

    Ok(())
}

#[cfg(feature = "phase2-bindings")]
fn metadata_has_blocked_link_semantics(metadata: &std::fs::Metadata) -> bool {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::is_blocked_link_metadata(metadata)
    }

    #[cfg(all(
        not(target_os = "linux"),
        not(target_os = "macos"),
        not(target_os = "windows"),
        windows
    ))]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        (metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
    }

    #[cfg(all(
        not(target_os = "linux"),
        not(target_os = "macos"),
        not(target_os = "windows"),
        not(windows)
    ))]
    {
        metadata.file_type().is_symlink()
    }
}

#[cfg(feature = "phase2-bindings")]
fn map_existing_broken_leaf_error(
    path: &Path,
    original: std::io::Error,
) -> std::result::Result<Option<AdapterError>, AdapterError> {
    match symlink_metadata_on_host(path) {
        Ok(metadata) if metadata_has_blocked_link_semantics(&metadata) => {
            Ok(Some(AdapterError::PermissionDenied))
        }
        Ok(_) => Ok(Some(map_io_error(original))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(map_io_error(err)),
    }
}

#[cfg(feature = "phase2-bindings")]
#[derive(Default)]
struct LocalPhase2AdapterState {
    next_id: u64,
    free_ids: Vec<u64>,
    resources: BTreeMap<u64, LocalResource>,
}

#[cfg(feature = "phase2-bindings")]
enum LocalResource {
    File(File),
    Stdin,
    Stdout,
    Stderr,
}

#[cfg(feature = "phase2-bindings")]
impl HostAdapter for LocalPhase2Adapter {
    fn io(&self) -> &dyn IoAdapter {
        self
    }

    fn fs(&self) -> &dyn FsAdapter {
        self
    }

    fn net(&self) -> &dyn NetAdapter {
        self
    }

    fn time(&self) -> &dyn TimeAdapter {
        self
    }

    fn locale(&self) -> &dyn LocaleAdapter {
        self
    }
}

#[cfg(feature = "phase2-bindings")]
impl IoAdapter for LocalPhase2Adapter {
    fn stdin(&self) -> std::result::Result<FileHandle, AdapterError> {
        self.insert_resource(LocalResource::Stdin)
    }

    fn stdout(&self) -> std::result::Result<FileHandle, AdapterError> {
        self.insert_resource(LocalResource::Stdout)
    }

    fn stderr(&self) -> std::result::Result<FileHandle, AdapterError> {
        self.insert_resource(LocalResource::Stderr)
    }

    fn args_raw(&self) -> std::result::Result<String, AdapterError> {
        encode_args_raw(&self.app_args)
    }

    fn read_stream(
        &self,
        handle: &FileHandle,
        n: u32,
    ) -> std::result::Result<Vec<u8>, AdapterError> {
        let len = bounded_read_len(n)?;
        let mut state = self.state.borrow_mut();
        match state.resources.get_mut(&handle.id) {
            Some(LocalResource::Stdin) => {
                let mut buf = vec![0; len];
                let len = read_stdin_on_host(&mut buf).map_err(map_io_error)?;
                buf.truncate(len);
                Ok(buf)
            }
            Some(_) => Err(AdapterError::Unsupported),
            None => Err(AdapterError::NotFound),
        }
    }

    fn read_stream_to_string(
        &self,
        handle: &FileHandle,
    ) -> std::result::Result<String, AdapterError> {
        let bytes = self.read_stream(handle, 1024 * 1024)?;
        String::from_utf8(bytes).map_err(|err| AdapterError::Io(err.to_string()))
    }

    fn write_stream(
        &self,
        handle: &FileHandle,
        bytes: &[u8],
    ) -> std::result::Result<u32, AdapterError> {
        let len = bounded_write_len(bytes.len())?;
        self.write_all_stream(handle, bytes)?;
        Ok(len)
    }

    fn write_all_stream(
        &self,
        handle: &FileHandle,
        bytes: &[u8],
    ) -> std::result::Result<(), AdapterError> {
        let mut state = self.state.borrow_mut();
        match state.resources.get_mut(&handle.id) {
            Some(LocalResource::Stdout) => self.output.borrow_mut().write_bytes(bytes),
            Some(LocalResource::Stderr) => write_stderr_on_host(bytes),
            Some(_) => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "stream is not writable",
            )),
            None => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "unknown stream",
            )),
        }
        .map_err(map_io_error)
    }

    fn flush_stream(&self, handle: &FileHandle) -> std::result::Result<(), AdapterError> {
        let mut state = self.state.borrow_mut();
        match state.resources.get_mut(&handle.id) {
            Some(LocalResource::Stdout) => self.output.borrow_mut().flush(),
            Some(LocalResource::Stderr) => flush_stderr_on_host(),
            Some(_) => Ok(()),
            None => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "unknown stream",
            )),
        }
        .map_err(map_io_error)
    }

    fn close_stream(&self, handle: &FileHandle) -> std::result::Result<(), AdapterError> {
        self.close_resource(handle)
    }

    fn log(&self, level: &str, message: &str) -> std::result::Result<(), AdapterError> {
        tracing::event!(tracing::Level::INFO, level, "{message}");
        Ok(())
    }
}

#[cfg(feature = "phase2-bindings")]
impl FsAdapter for LocalPhase2Adapter {
    fn open(&self, path: &str, mode: OpenMode) -> std::result::Result<FileHandle, AdapterError> {
        let missing_leaf = match mode {
            OpenMode::Read => FsOperation::Existing,
            OpenMode::Write | OpenMode::ReadWrite | OpenMode::Append => FsOperation::CreateLeaf,
        };
        let path = self.resolve_fs_path(path, missing_leaf)?;
        let mut opts = std::fs::OpenOptions::new();
        apply_no_follow_final_symlink_on_host(&mut opts);
        match mode {
            OpenMode::Read => {
                opts.read(true);
            }
            OpenMode::Write => {
                opts.write(true).create(true).truncate(true);
            }
            OpenMode::ReadWrite => {
                opts.read(true).write(true).create(true);
            }
            OpenMode::Append => {
                opts.append(true).create(true);
            }
        }
        let file = open_path_on_host(path.as_path(), &mut opts).map_err(map_io_error)?;
        self.insert_resource(LocalResource::File(file))
    }

    fn read(&self, handle: &FileHandle, n: u32) -> std::result::Result<Vec<u8>, AdapterError> {
        let len = bounded_read_len(n)?;
        let mut state = self.state.borrow_mut();
        let Some(LocalResource::File(file)) = state.resources.get_mut(&handle.id) else {
            return Err(AdapterError::NotFound);
        };
        let mut buf = vec![0; len];
        let len = read_file_on_host(file, &mut buf).map_err(map_io_error)?;
        buf.truncate(len);
        Ok(buf)
    }

    fn write(&self, handle: &FileHandle, bytes: &[u8]) -> std::result::Result<u32, AdapterError> {
        bounded_write_len(bytes.len())?;
        let mut state = self.state.borrow_mut();
        let Some(LocalResource::File(file)) = state.resources.get_mut(&handle.id) else {
            return Err(AdapterError::NotFound);
        };
        let len = write_file_on_host(file, bytes).map_err(map_io_error)?;
        u32::try_from(len).map_err(|_| AdapterError::Io("write length overflow".to_string()))
    }

    fn seek_set(&self, handle: &FileHandle, pos: u64) -> std::result::Result<u64, AdapterError> {
        let mut state = self.state.borrow_mut();
        let Some(LocalResource::File(file)) = state.resources.get_mut(&handle.id) else {
            return Err(AdapterError::NotFound);
        };
        seek_file_on_host(file, SeekFrom::Start(pos)).map_err(map_io_error)
    }

    fn seek_end(&self, handle: &FileHandle) -> std::result::Result<u64, AdapterError> {
        let mut state = self.state.borrow_mut();
        let Some(LocalResource::File(file)) = state.resources.get_mut(&handle.id) else {
            return Err(AdapterError::NotFound);
        };
        seek_file_on_host(file, SeekFrom::End(0)).map_err(map_io_error)
    }

    fn stat_handle(&self, handle: &FileHandle) -> std::result::Result<FileStat, AdapterError> {
        let state = self.state.borrow();
        let Some(LocalResource::File(file)) = state.resources.get(&handle.id) else {
            return Err(AdapterError::NotFound);
        };
        file_metadata_on_host(file)
            .map(file_stat_from_metadata)
            .map_err(map_io_error)
    }

    fn stat(&self, path: &str) -> std::result::Result<FileStat, AdapterError> {
        let path = self.resolve_fs_path(path, FsOperation::Existing)?;
        stat_path_on_host(path.as_path())
            .map(file_stat_from_metadata)
            .map_err(map_io_error)
    }

    fn list(&self, path: &str) -> std::result::Result<Vec<String>, AdapterError> {
        let path = self.resolve_fs_path(path, FsOperation::Existing)?;
        let mut entries = Vec::new();
        for entry in read_dir_on_host(path.as_path()).map_err(map_io_error)? {
            let entry = entry.map_err(map_io_error)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| AdapterError::InvalidPath)?;
            entries.push(name);
            if entries.len() > MAX_PHASE2_LIST_ENTRIES {
                return Err(AdapterError::Io(format!(
                    "directory listing exceeds limit ({MAX_PHASE2_LIST_ENTRIES} entries)"
                )));
            }
        }
        entries.sort();
        Ok(entries)
    }

    fn remove_file(&self, path: &str) -> std::result::Result<(), AdapterError> {
        let path = self.resolve_fs_path(path, FsOperation::RemoveLeaf)?;
        remove_file_on_host(path.as_path()).map_err(map_io_error)
    }

    fn remove_dir(&self, path: &str) -> std::result::Result<(), AdapterError> {
        let path = self.resolve_fs_path(path, FsOperation::RemoveLeaf)?;
        remove_dir_on_host(path.as_path()).map_err(map_io_error)
    }

    fn mkdir(&self, path: &str) -> std::result::Result<(), AdapterError> {
        let path = self.resolve_fs_path(path, FsOperation::CreateLeaf)?;
        create_dir_on_host(path.as_path()).map_err(map_io_error)
    }

    fn rename(&self, from: &str, to: &str) -> std::result::Result<(), AdapterError> {
        let from = self.resolve_fs_path(from, FsOperation::RenameSource)?;
        let to = self.resolve_fs_path(to, FsOperation::RenameDestination)?;
        rename_path_on_host(from.as_path(), to.as_path()).map_err(map_io_error)
    }

    fn close_file(&self, handle: &FileHandle) -> std::result::Result<(), AdapterError> {
        self.close_resource(handle)
    }
}

#[cfg(feature = "phase2-bindings")]
fn apply_no_follow_final_symlink_on_host(opts: &mut std::fs::OpenOptions) {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::apply_no_follow_final_symlink(opts);
    }

    #[cfg(all(
        not(target_os = "linux"),
        not(target_os = "macos"),
        not(target_os = "windows"),
        unix
    ))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }

    #[cfg(all(
        not(target_os = "linux"),
        not(target_os = "macos"),
        not(target_os = "windows"),
        not(unix)
    ))]
    {
        let _ = opts;
    }
}

#[cfg(feature = "phase2-bindings")]
fn stat_path_on_host(path: &Path) -> std::io::Result<std::fs::Metadata> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::stat_path(path)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        std::fs::metadata(path)
    }
}

#[cfg(feature = "phase2-bindings")]
fn read_dir_on_host(path: &Path) -> std::io::Result<std::fs::ReadDir> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::read_dir(path)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        std::fs::read_dir(path)
    }
}

#[cfg(feature = "phase2-bindings")]
fn remove_file_on_host(path: &Path) -> std::io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::remove_file(path)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        std::fs::remove_file(path)
    }
}

#[cfg(feature = "phase2-bindings")]
fn remove_dir_on_host(path: &Path) -> std::io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::remove_dir(path)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        std::fs::remove_dir(path)
    }
}

#[cfg(feature = "phase2-bindings")]
fn create_dir_on_host(path: &Path) -> std::io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::create_dir(path)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        std::fs::create_dir(path)
    }
}

#[cfg(feature = "phase2-bindings")]
fn rename_path_on_host(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::rename_path(from, to)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        std::fs::rename(from, to)
    }
}

#[cfg(feature = "phase2-bindings")]
fn symlink_metadata_on_host(path: &Path) -> std::io::Result<std::fs::Metadata> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::symlink_metadata(path)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        std::fs::symlink_metadata(path)
    }
}

#[cfg(feature = "phase2-bindings")]
fn read_stdin_on_host(buf: &mut [u8]) -> std::io::Result<usize> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::read_stdin(buf)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let mut stdin = std::io::stdin();
        std::io::Read::read(&mut stdin, buf)
    }
}

#[cfg(feature = "phase2-bindings")]
fn write_stderr_on_host(bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::write_stderr(bytes)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let mut stderr = std::io::stderr();
        std::io::Write::write_all(&mut stderr, bytes)
    }
}

#[cfg(feature = "phase2-bindings")]
fn flush_stderr_on_host() -> std::io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::flush_stderr()
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let mut stderr = std::io::stderr();
        std::io::Write::flush(&mut stderr)
    }
}

fn print_stdout_line_on_host(msg: &str) {
    #[cfg(all(
        feature = "phase2-bindings",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    {
        host_os_adapter::print_stdout_line(msg);
    }

    #[cfg(not(all(
        feature = "phase2-bindings",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    )))]
    {
        println!("{msg}");
    }
}

fn write_stdout_on_host(bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(all(
        feature = "phase2-bindings",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    {
        host_os_adapter::write_stdout(bytes)
    }

    #[cfg(not(all(
        feature = "phase2-bindings",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    )))]
    {
        let mut stdout = std::io::stdout();
        std::io::Write::write_all(&mut stdout, bytes)
    }
}

fn flush_stdout_on_host() -> std::io::Result<()> {
    #[cfg(all(
        feature = "phase2-bindings",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    {
        host_os_adapter::flush_stdout()
    }

    #[cfg(not(all(
        feature = "phase2-bindings",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    )))]
    {
        let mut stdout = std::io::stdout();
        std::io::Write::flush(&mut stdout)
    }
}

fn read_path_on_host(path: &Path) -> std::io::Result<Vec<u8>> {
    #[cfg(all(
        feature = "phase2-bindings",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    {
        host_os_adapter::read_path(path)
    }

    #[cfg(not(all(
        feature = "phase2-bindings",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    )))]
    {
        std::fs::read(path)
    }
}

fn create_dir_all_on_host(path: &Path) -> std::io::Result<()> {
    #[cfg(all(
        feature = "phase2-bindings",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    {
        host_os_adapter::create_dir_all(path)
    }

    #[cfg(not(all(
        feature = "phase2-bindings",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    )))]
    {
        std::fs::create_dir_all(path)
    }
}

#[cfg(feature = "phase2-bindings")]
fn read_file_on_host(file: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::read_file(file, buf)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        std::io::Read::read(file, buf)
    }
}

#[cfg(feature = "phase2-bindings")]
fn write_file_on_host(file: &mut std::fs::File, bytes: &[u8]) -> std::io::Result<usize> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::write_file(file, bytes)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        std::io::Write::write(file, bytes)
    }
}

#[cfg(feature = "phase2-bindings")]
fn seek_file_on_host(file: &mut std::fs::File, pos: SeekFrom) -> std::io::Result<u64> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::seek_file(file, pos)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        std::io::Seek::seek(file, pos)
    }
}

#[cfg(feature = "phase2-bindings")]
fn file_metadata_on_host(file: &std::fs::File) -> std::io::Result<std::fs::Metadata> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::file_metadata(file)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        file.metadata()
    }
}

#[cfg(feature = "phase2-bindings")]
fn canonicalize_path_on_host(path: &Path) -> std::io::Result<PathBuf> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::canonicalize_path(path)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        path.canonicalize()
    }
}

#[cfg(feature = "phase2-bindings")]
fn open_path_on_host(
    path: &Path,
    opts: &mut std::fs::OpenOptions,
) -> std::io::Result<std::fs::File> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::open_path(path, opts)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        opts.open(path)
    }
}

#[cfg(feature = "phase2-bindings")]
impl NetAdapter for LocalPhase2Adapter {
    fn fetch(&self, req: HttpRequest) -> std::result::Result<HttpResponse, AdapterError> {
        let url = PlainHttpUrl::parse(&req.url).map_err(map_plain_http_error)?;
        let plain_req = plain_http_request_from_dispatch(&req);
        let request = build_plain_http_request(&plain_req, &url).map_err(map_plain_http_error)?;

        let mut stream = connect_plain_http_stream(&url, req.timeout_millis)?;
        if let Some(millis) = req.timeout_millis {
            let timeout = Duration::from_millis(millis.into());
            apply_tcp_timeouts_on_host(&stream, timeout).map_err(map_io_error)?;
        }

        write_all_tcp_on_host(&mut stream, &request).map_err(map_net_io_error)?;
        let response =
            read_plain_http_response_limited_on_host(&mut stream, self.max_http_response_bytes)
                .map_err(map_plain_http_read_error)?;
        let response = parse_plain_http_response(&response).map_err(map_plain_http_error)?;
        Ok(HttpResponse {
            status: response.status,
            headers: response
                .headers
                .into_iter()
                .map(|header| Header {
                    name: header.name,
                    value: header.value,
                })
                .collect(),
            body: response.body,
        })
    }
}

#[cfg(feature = "phase2-bindings")]
fn connect_plain_http_stream(
    url: &PlainHttpUrl,
    timeout_millis: Option<u32>,
) -> std::result::Result<TcpStream, AdapterError> {
    let addrs = resolve_plain_http_socket_addrs(url)?;

    if let Some(millis) = timeout_millis {
        if millis == 0 {
            return Err(AdapterError::Timeout);
        }
        let timeout = Duration::from_millis(u64::from(millis));
        let deadline = std::time::Instant::now() + timeout;
        let mut last_err = None;
        for addr in addrs {
            let remaining = remaining_connect_timeout(deadline)?;
            match connect_tcp_on_host(addr, Some(remaining)) {
                Ok(stream) => return Ok(stream),
                Err(err) => last_err = Some(err),
            }
        }
        if let Some(err) = last_err {
            return Err(map_net_io_error(err));
        }
        return Err(AdapterError::NotFound);
    }

    let mut last_err = None;
    for addr in addrs {
        match connect_tcp_on_host(addr, None) {
            Ok(stream) => return Ok(stream),
            Err(err) => last_err = Some(err),
        }
    }
    if let Some(err) = last_err {
        return Err(map_net_io_error(err));
    }
    Err(AdapterError::NotFound)
}

#[cfg(feature = "phase2-bindings")]
fn remaining_connect_timeout(
    deadline: std::time::Instant,
) -> std::result::Result<Duration, AdapterError> {
    remaining_connect_timeout_for(std::time::Instant::now(), deadline)
}

#[cfg(feature = "phase2-bindings")]
fn remaining_connect_timeout_for(
    now: std::time::Instant,
    deadline: std::time::Instant,
) -> std::result::Result<Duration, AdapterError> {
    if now >= deadline {
        return Err(AdapterError::Timeout);
    }
    Ok(deadline.duration_since(now))
}

#[cfg(feature = "phase2-bindings")]
fn resolve_plain_http_socket_addrs(
    url: &PlainHttpUrl,
) -> std::result::Result<Vec<SocketAddr>, AdapterError> {
    let addrs =
        resolve_socket_addrs_on_host(url.host.as_str(), url.port).map_err(map_net_resolve_error)?;
    normalize_resolved_socket_addrs_or_not_found(addrs)
}

#[cfg(feature = "phase2-bindings")]
fn normalize_resolved_socket_addrs_or_not_found(
    addrs: Vec<SocketAddr>,
) -> std::result::Result<Vec<SocketAddr>, AdapterError> {
    let addrs = normalize_resolved_socket_addrs(addrs);
    if addrs.is_empty() {
        return Err(AdapterError::NotFound);
    }
    Ok(addrs)
}

#[cfg(feature = "phase2-bindings")]
fn resolve_socket_addrs_on_host(host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::resolve_socket_addrs(host, port)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        use std::net::ToSocketAddrs;
        (host, port).to_socket_addrs().map(Iterator::collect)
    }
}

#[cfg(feature = "phase2-bindings")]
fn connect_tcp_on_host(addr: SocketAddr, timeout: Option<Duration>) -> std::io::Result<TcpStream> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::connect_tcp(addr, timeout)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        match timeout {
            Some(timeout) => TcpStream::connect_timeout(&addr, timeout),
            None => TcpStream::connect(addr),
        }
    }
}

#[cfg(feature = "phase2-bindings")]
fn apply_tcp_timeouts_on_host(stream: &TcpStream, timeout: Duration) -> std::io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::apply_tcp_timeouts(stream, timeout)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))
    }
}

#[cfg(feature = "phase2-bindings")]
fn write_all_tcp_on_host(stream: &mut TcpStream, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::write_all_tcp(stream, bytes)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        std::io::Write::write_all(stream, bytes)
    }
}

#[cfg(feature = "phase2-bindings")]
fn read_tcp_on_host(stream: &mut TcpStream, buf: &mut [u8]) -> std::io::Result<usize> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        host_os_adapter::read_tcp(stream, buf)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        std::io::Read::read(stream, buf)
    }
}

#[cfg(feature = "phase2-bindings")]
struct HostTcpReader<'a> {
    stream: &'a mut TcpStream,
}

#[cfg(feature = "phase2-bindings")]
impl std::io::Read for HostTcpReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        read_tcp_on_host(self.stream, buf)
    }
}

#[cfg(feature = "phase2-bindings")]
fn read_plain_http_response_limited_on_host(
    stream: &mut TcpStream,
    max_bytes: usize,
) -> std::result::Result<Vec<u8>, PlainHttpReadError> {
    let mut reader = HostTcpReader { stream };
    read_plain_http_response_limited(&mut reader, max_bytes)
}

#[cfg(feature = "phase2-bindings")]
fn plain_http_request_from_dispatch(req: &HttpRequest) -> PlainHttpRequest {
    PlainHttpRequest {
        method: plain_http_method_from_dispatch(req.method),
        headers: req
            .headers
            .iter()
            .map(|header| PlainHttpHeader {
                name: header.name.clone(),
                value: header.value.clone(),
            })
            .collect(),
        body: req.body.clone(),
    }
}

#[cfg(feature = "phase2-bindings")]
fn plain_http_method_from_dispatch(method: uapi_dispatch::HttpMethod) -> PlainHttpMethod {
    match method {
        uapi_dispatch::HttpMethod::Get => PlainHttpMethod::Get,
        uapi_dispatch::HttpMethod::Post => PlainHttpMethod::Post,
        uapi_dispatch::HttpMethod::Put => PlainHttpMethod::Put,
        uapi_dispatch::HttpMethod::Delete => PlainHttpMethod::Delete,
        uapi_dispatch::HttpMethod::Patch => PlainHttpMethod::Patch,
        uapi_dispatch::HttpMethod::Head => PlainHttpMethod::Head,
        uapi_dispatch::HttpMethod::Options => PlainHttpMethod::Options,
    }
}

#[cfg(feature = "phase2-bindings")]
fn map_plain_http_error(err: PlainHttpError) -> AdapterError {
    match err {
        PlainHttpError::UnsupportedScheme => AdapterError::Unsupported,
        PlainHttpError::InvalidUrl => AdapterError::InvalidPath,
        PlainHttpError::InvalidHeader => AdapterError::Protocol("invalid HTTP header".to_string()),
        PlainHttpError::BodyTooLarge => AdapterError::BodyTooLarge,
        PlainHttpError::InvalidResponse => {
            AdapterError::Protocol("invalid HTTP response".to_string())
        }
        PlainHttpError::HostControlledHeader => {
            AdapterError::Protocol("host-controlled HTTP header".to_string())
        }
    }
}

#[cfg(feature = "phase2-bindings")]
fn map_plain_http_read_error(err: PlainHttpReadError) -> AdapterError {
    match err {
        PlainHttpReadError::Timeout => AdapterError::Timeout,
        PlainHttpReadError::BodyTooLarge => AdapterError::BodyTooLarge,
        PlainHttpReadError::Io(err) => map_io_error(err),
    }
}

#[cfg(feature = "phase2-bindings")]
fn normalize_fs_path(path: &str) -> std::result::Result<LogicalPath, AdapterError> {
    LogicalPath::parse(path).map_err(map_path_error)
}

#[cfg(feature = "phase2-bindings")]
fn map_path_error(err: PathError) -> AdapterError {
    match err {
        PathError::Empty
        | PathError::ControlCharacter
        | PathError::ParentTraversal
        | PathError::SegmentTooLong
        | PathError::PathTooLong
        | PathError::AmbiguousWindowsSuffix
        | PathError::ReservedName
        | PathError::UnsupportedPrefix
        | PathError::UnsafeRootOperation => AdapterError::InvalidPath,
    }
}

#[cfg(feature = "phase2-bindings")]
fn map_time_error(err: TimeError) -> AdapterError {
    AdapterError::Io(err.to_string())
}

#[cfg(feature = "phase2-bindings")]
impl TimeAdapter for LocalPhase2Adapter {
    fn now_millis(&self) -> std::result::Result<u64, AdapterError> {
        self.clock.now_millis().map_err(map_time_error)
    }

    fn monotonic_nanos(&self) -> std::result::Result<u64, AdapterError> {
        Ok(self.clock.monotonic_nanos())
    }

    fn sleep_millis(&self, millis: u32) -> std::result::Result<(), AdapterError> {
        sleep_on_host(millis);
        Ok(())
    }
}

#[cfg(feature = "phase2-bindings")]
impl LocaleAdapter for LocalPhase2Adapter {
    fn current(&self) -> std::result::Result<LocaleId, AdapterError> {
        Ok(locale_from_host(host_current_locale(&self.locale)))
    }

    fn timezone(&self) -> std::result::Result<String, AdapterError> {
        Ok(timezone_from_host_locale(&self.locale))
    }

    fn format_date(
        &self,
        millis: u64,
        tz: &str,
        style: DateStyle,
        loc: &LocaleId,
    ) -> std::result::Result<String, AdapterError> {
        Ok(format_date_on_host(
            millis,
            tz,
            date_style_to_host(style),
            &locale_to_host(loc),
        ))
    }

    fn format_number(
        &self,
        value: f64,
        style: uapi_dispatch::NumberStyle,
        loc: &LocaleId,
    ) -> std::result::Result<String, AdapterError> {
        Ok(format_number_on_host(
            value,
            number_style_to_host(style),
            &locale_to_host(loc),
        ))
    }
}

#[cfg(feature = "phase2-bindings")]
fn locale_from_host(locale: HostLocaleId) -> LocaleId {
    LocaleId {
        bcp47: locale.bcp47,
    }
}

#[cfg(feature = "phase2-bindings")]
fn locale_to_host(locale: &LocaleId) -> HostLocaleId {
    HostLocaleId {
        bcp47: locale.bcp47.clone(),
    }
}

#[cfg(feature = "phase2-bindings")]
fn date_style_to_host(style: DateStyle) -> HostDateStyle {
    match style {
        DateStyle::Short => HostDateStyle::Short,
        DateStyle::Medium => HostDateStyle::Medium,
        DateStyle::Long => HostDateStyle::Long,
        DateStyle::Full => HostDateStyle::Full,
    }
}

#[cfg(feature = "phase2-bindings")]
fn number_style_to_host(style: uapi_dispatch::NumberStyle) -> HostNumberStyle {
    match style {
        uapi_dispatch::NumberStyle::Decimal => HostNumberStyle::Decimal,
        uapi_dispatch::NumberStyle::Percent => HostNumberStyle::Percent,
        uapi_dispatch::NumberStyle::Currency => HostNumberStyle::Currency,
    }
}

#[cfg(feature = "phase2-bindings")]
fn file_stat_from_metadata(metadata: std::fs::Metadata) -> FileStat {
    let modified_millis = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default();

    FileStat {
        size: metadata.len(),
        modified_millis,
        is_dir: metadata.is_dir(),
    }
}

#[cfg(feature = "phase2-bindings")]
fn map_io_error(err: std::io::Error) -> AdapterError {
    #[cfg(unix)]
    if err.raw_os_error() == Some(libc::ELOOP) {
        return AdapterError::PermissionDenied;
    }

    match err.kind() {
        std::io::ErrorKind::NotFound => AdapterError::NotFound,
        std::io::ErrorKind::PermissionDenied => AdapterError::PermissionDenied,
        std::io::ErrorKind::AlreadyExists => AdapterError::Io("already exists".to_string()),
        std::io::ErrorKind::InvalidInput => AdapterError::InvalidPath,
        _ => AdapterError::Io(err.to_string()),
    }
}

#[cfg(feature = "phase2-bindings")]
fn map_net_io_error(err: std::io::Error) -> AdapterError {
    match err.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => AdapterError::Timeout,
        std::io::ErrorKind::ConnectionRefused
        | std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::ConnectionAborted
        | std::io::ErrorKind::NotConnected
        | std::io::ErrorKind::AddrInUse
        | std::io::ErrorKind::AddrNotAvailable
        | std::io::ErrorKind::BrokenPipe
        | std::io::ErrorKind::UnexpectedEof => AdapterError::Network(err.to_string()),
        _ => map_io_error(err),
    }
}

#[cfg(feature = "phase2-bindings")]
fn map_net_resolve_error(err: std::io::Error) -> AdapterError {
    match err.kind() {
        std::io::ErrorKind::NotFound => AdapterError::NotFound,
        _ => AdapterError::Network(err.to_string()),
    }
}

fn classify_limit_error(err: &wasmtime::Error) -> Option<String> {
    if err.downcast_ref::<Trap>() == Some(&Trap::OutOfFuel) {
        return Some("fuel exhausted".to_string());
    }

    let message = err.to_string();
    if message.contains("memory limit exceeded") {
        Some(message)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn default_config_sets_phase_1_memory_cap() {
        assert_eq!(Config::default().memory_bytes, 256 * 1024 * 1024);
        assert_eq!(
            Config::default().max_http_response_bytes,
            DEFAULT_MAX_HTTP_RESPONSE_BYTES
        );
        assert_eq!(
            Config::default().default_http_timeout_millis,
            Some(DEFAULT_HTTP_TIMEOUT_MILLIS)
        );
    }

    #[test]
    fn host_state_receives_session_policy() {
        let config = Config {
            session_policy: SessionPolicy::from_cli_grants(&["fs.read:./notes/**".to_string()])
                .expect("policy"),
            ..Config::default()
        };
        let state = HostState::new(&config, OutputMode::Sink).expect("host state");

        assert!(state
            .uapi()
            .check(&uapi::UapiCall::Fs(uapi::FsCall::Read {
                path: "./notes/today.txt".to_string(),
            }))
            .is_ok());
    }

    #[test]
    fn invalid_component_is_reported() {
        let config = Config::default();
        let runtime = Runtime::new(&config).expect("runtime should initialize");
        let err = runtime
            .run_bytes(b"not wasm", &config)
            .expect_err("invalid bytes must fail");

        assert!(matches!(err, RuntimeError::InvalidComponent(_)));
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn plain_http_url_parser_normalizes_query_only_paths() {
        let parsed =
            PlainHttpUrl::parse("http://127.0.0.1:8080?name=krate#local").expect("parse HTTP URL");

        assert_eq!(parsed.host, "127.0.0.1");
        assert_eq!(parsed.port, 8080);
        assert_eq!(parsed.path_and_query, "/?name=krate");
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn fs_path_normalizer_rejects_parent_traversal_before_host_io() {
        let err = normalize_fs_path("fixtures/../secret.txt").expect_err("path should be rejected");

        assert_eq!(err, AdapterError::InvalidPath);
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn local_phase2_adapter_applies_test_locale_and_timezone_overrides() {
        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            Some(1_234_567_890),
            Some("en_GB.UTF-8".to_string()),
            Some("UTC".to_string()),
            Vec::new(),
            1024,
            PathBuf::from("."),
        );

        let locale = adapter.current().expect("read locale");
        let timezone = adapter.timezone().expect("read timezone");
        let date = adapter
            .format_date(1_234_567_890, &timezone, DateStyle::Medium, &locale)
            .expect("format date");

        assert_eq!(locale.bcp47, "en-GB");
        assert_eq!(timezone, "UTC");
        assert_eq!(date, "1970-01-15 06:56");
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn local_phase2_adapter_normalizes_timezone_offset_override() {
        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            Some("en_US.UTF-8".to_string()),
            Some("UTC+5:30".to_string()),
            Vec::new(),
            1024,
            PathBuf::from("."),
        );

        let locale = adapter.current().expect("read locale");
        let timezone = adapter.timezone().expect("read timezone");
        let date = adapter
            .format_date(1_234_567_890, &timezone, DateStyle::Long, &locale)
            .expect("format date");

        assert_eq!(timezone, "UTC+05:30");
        assert_eq!(date, "1970-01-15 12:26:07 UTC+05:30");
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn local_fs_adapter_normalizes_portable_separators() {
        let temp =
            std::env::temp_dir().join(format!("krate-path-normalize-{}", std::process::id()));
        let nested = temp.join("fixtures").join("public");
        std::fs::create_dir_all(&nested).expect("create fixture directory");
        let file = nested.join("note.txt");
        std::fs::write(&file, b"hello").expect("write fixture file");

        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            None,
            None,
            Vec::new(),
            1024,
            temp.clone(),
        );
        let path = "./fixtures\\public//note.txt";
        let handle = adapter
            .open(path, OpenMode::Read)
            .expect("normalized path should open");
        let bytes = adapter.read(&handle, 5).expect("read file");

        assert_eq!(bytes, b"hello");

        drop(handle);
        drop(adapter);
        std::fs::remove_file(file).expect("remove fixture file");
        std::fs::remove_dir_all(temp).expect("remove fixture directory");
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn local_fs_adapter_treats_absolute_logical_paths_as_sandbox_relative() {
        let unique = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        let temp = std::env::temp_dir().join(format!(
            "krate-absolute-logical-path-{}-{unique}",
            std::process::id(),
        ));
        let nested = temp.join("fixtures").join("public");
        std::fs::create_dir_all(&nested).expect("create fixture directory");
        let file = nested.join("note.txt");
        std::fs::write(&file, b"inside").expect("write fixture file");

        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            None,
            None,
            Vec::new(),
            1024,
            temp.clone(),
        );

        let handle = adapter
            .open("/fixtures/public/note.txt", OpenMode::Read)
            .expect("absolute logical path should resolve inside sandbox");
        let bytes = adapter.read(&handle, 6).expect("read file");

        assert_eq!(bytes, b"inside");

        drop(handle);
        drop(adapter);
        std::fs::remove_file(file).expect("remove fixture file");
        std::fs::remove_dir_all(temp).expect("remove fixture directory");
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn absolute_logical_path_becomes_host_relative_segments() {
        let logical = normalize_fs_path("/fixtures/public/note.txt").expect("valid path");
        let relative = logical_path_to_sandbox_relative(&logical);

        assert!(!relative.is_absolute());
        assert_eq!(
            relative,
            PathBuf::from("fixtures").join("public").join("note.txt")
        );
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn local_io_adapter_formats_args_raw_with_newline_separator() {
        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            None,
            None,
            vec!["clock".to_string(), "--utc".to_string()],
            1024,
            PathBuf::from("."),
        );

        let raw = adapter.args_raw().expect("encode raw args");
        assert_eq!(raw, "clock\n--utc\n");
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn local_io_adapter_rejects_invalid_raw_argument_shapes() {
        let empty_arg_adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            None,
            None,
            vec!["ok".to_string(), "".to_string()],
            1024,
            PathBuf::from("."),
        );
        let empty_err = empty_arg_adapter
            .args_raw()
            .expect_err("empty argument should be rejected");
        assert!(
            matches!(empty_err, AdapterError::Io(ref message) if message.contains("empty entries")),
            "unexpected empty-arg error: {empty_err:?}"
        );

        let newline_arg_adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            None,
            None,
            vec!["bad\narg".to_string()],
            1024,
            PathBuf::from("."),
        );
        let newline_err = newline_arg_adapter
            .args_raw()
            .expect_err("newline argument should be rejected");
        assert!(
            matches!(newline_err, AdapterError::Io(ref message) if message.contains("delimiter characters")),
            "unexpected newline-arg error: {newline_err:?}"
        );
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn local_io_adapter_rejects_oversized_raw_args() {
        let oversized = "x".repeat(MAX_PHASE2_ARGS_RAW_BYTES + 1);
        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            None,
            None,
            vec![oversized],
            1024,
            PathBuf::from("."),
        );

        let err = adapter
            .args_raw()
            .expect_err("oversized raw args should be rejected");
        assert!(
            matches!(err, AdapterError::Io(ref message) if message.contains("raw args limit")),
            "unexpected oversized-args error: {err:?}"
        );
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn local_io_adapter_rejects_too_many_raw_args() {
        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            None,
            None,
            vec!["x".to_string(); MAX_PHASE2_ARG_COUNT + 1],
            1024,
            PathBuf::from("."),
        );

        let err = adapter
            .args_raw()
            .expect_err("too many raw args should be rejected");
        assert!(
            matches!(err, AdapterError::Io(ref message) if message.contains("count limit")),
            "unexpected too-many-args error: {err:?}"
        );
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn local_fs_adapter_rejects_oversized_read_request() {
        let unique = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        let temp = std::env::temp_dir().join(format!(
            "krate-oversized-read-{}-{unique}",
            std::process::id(),
        ));
        std::fs::create_dir_all(&temp).expect("create sandbox");
        let file = temp.join("note.txt");
        std::fs::write(&file, b"hello").expect("write fixture file");

        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            None,
            None,
            Vec::new(),
            1024,
            temp.clone(),
        );

        let handle = adapter
            .open("note.txt", OpenMode::Read)
            .expect("open fixture file");
        let err = adapter
            .read(&handle, (MAX_PHASE2_READ_BYTES + 1) as u32)
            .expect_err("oversized read should be rejected");

        match err {
            AdapterError::Io(message) => assert!(
                message.contains("read request exceeds limit"),
                "unexpected message: {message}"
            ),
            other => panic!("unexpected error variant: {other:?}"),
        }

        drop(handle);
        drop(adapter);
        std::fs::remove_file(file).expect("remove fixture file");
        std::fs::remove_dir_all(temp).expect("remove fixture directory");
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn local_io_adapter_rejects_oversized_stream_write_request() {
        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            None,
            None,
            Vec::new(),
            1024,
            PathBuf::from("."),
        );

        let handle = adapter.stdout().expect("open stdout");
        let err = adapter
            .write_stream(&handle, &vec![b'x'; MAX_PHASE2_WRITE_BYTES + 1])
            .expect_err("oversized stream write should be rejected");

        match err {
            AdapterError::Io(message) => assert!(
                message.contains("write request exceeds limit"),
                "unexpected message: {message}"
            ),
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn local_io_adapter_rejects_resource_table_overflow() {
        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            None,
            None,
            Vec::new(),
            1024,
            PathBuf::from("."),
        );

        for _ in 0..MAX_PHASE2_OPEN_RESOURCES {
            adapter.stdout().expect("open stream handle within limit");
        }

        let err = adapter
            .stdout()
            .expect_err("resource-table overflow should be rejected");
        assert!(
            matches!(err, AdapterError::Io(ref message) if message.contains("resource table exceeds limit")),
            "unexpected resource-table overflow error: {err:?}"
        );
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn local_io_adapter_releases_slot_on_close() {
        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            None,
            None,
            Vec::new(),
            1024,
            PathBuf::from("."),
        );

        let mut handles = Vec::new();
        for _ in 0..MAX_PHASE2_OPEN_RESOURCES {
            handles.push(adapter.stdout().expect("open stream handle within limit"));
        }

        let err = adapter
            .stdout()
            .expect_err("resource-table overflow should be rejected before close");
        assert!(
            matches!(err, AdapterError::Io(ref message) if message.contains("resource table exceeds limit")),
            "unexpected pre-close overflow error: {err:?}"
        );

        let released = handles.pop().expect("last handle");
        adapter
            .close_stream(&released)
            .expect("close stream should release one resource slot");

        adapter
            .stdout()
            .expect("opening stream after close should succeed");
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn local_io_adapter_reuses_released_resource_id() {
        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            None,
            None,
            Vec::new(),
            1024,
            PathBuf::from("."),
        );

        let first = adapter.stdout().expect("open first stream");
        let second = adapter.stdout().expect("open second stream");
        assert!(
            second.id > first.id,
            "resource ids should increase while allocating fresh handles"
        );

        adapter
            .close_stream(&second)
            .expect("close should release the handle id");

        let reopened = adapter
            .stdout()
            .expect("opening after close should reuse released id");
        assert_eq!(
            reopened.id, second.id,
            "adapter should reuse released ids before allocating new ones"
        );
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn local_io_adapter_allocates_from_free_list_before_id_overflow() {
        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            None,
            None,
            Vec::new(),
            1024,
            PathBuf::from("."),
        );

        {
            let mut state = adapter.state.borrow_mut();
            state.next_id = u64::MAX;
            state.free_ids.push(42);
        }

        let handle = adapter
            .stdout()
            .expect("free-list id should be used before fresh id allocation");
        assert_eq!(handle.id, 42);
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn local_fs_adapter_rejects_oversized_write_request() {
        let unique = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        let temp = std::env::temp_dir().join(format!(
            "krate-oversized-write-{}-{unique}",
            std::process::id(),
        ));
        std::fs::create_dir_all(&temp).expect("create sandbox");

        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            None,
            None,
            Vec::new(),
            1024,
            temp.clone(),
        );

        let handle = adapter
            .open("note.txt", OpenMode::Write)
            .expect("open fixture file");
        let err = adapter
            .write(&handle, &vec![b'x'; MAX_PHASE2_WRITE_BYTES + 1])
            .expect_err("oversized file write should be rejected");

        match err {
            AdapterError::Io(message) => assert!(
                message.contains("write request exceeds limit"),
                "unexpected message: {message}"
            ),
            other => panic!("unexpected error variant: {other:?}"),
        }

        drop(handle);
        drop(adapter);
        std::fs::remove_dir_all(temp).expect("remove fixture directory");
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn local_fs_adapter_rejects_oversized_directory_listing() {
        let unique = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        let temp = std::env::temp_dir().join(format!(
            "krate-oversized-list-{}-{unique}",
            std::process::id(),
        ));
        let list_dir = temp.join("many");
        std::fs::create_dir_all(&list_dir).expect("create fixture directory");
        for index in 0..=MAX_PHASE2_LIST_ENTRIES {
            let path = list_dir.join(format!("f{index:04}.txt"));
            std::fs::write(path, b"x").expect("write fixture entry");
        }

        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            None,
            None,
            Vec::new(),
            1024,
            temp.clone(),
        );

        let err = adapter
            .list("many")
            .expect_err("oversized directory listing should be rejected");
        match err {
            AdapterError::Io(message) => assert!(
                message.contains("directory listing exceeds limit"),
                "unexpected message: {message}"
            ),
            other => panic!("unexpected error variant: {other:?}"),
        }

        drop(adapter);
        std::fs::remove_dir_all(temp).expect("remove fixture directory");
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn local_fs_adapter_rejects_destructive_root_targets() {
        let unique = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        let temp =
            std::env::temp_dir().join(format!("krate-root-target-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&temp).expect("create sandbox");
        std::fs::write(temp.join("source.txt"), b"source").expect("write source file");

        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            None,
            None,
            Vec::new(),
            1024,
            temp.clone(),
        );

        let remove_err = adapter
            .remove_dir(".")
            .expect_err("remove_dir must not target sandbox root");
        let rename_err = adapter
            .rename(".", "renamed-root")
            .expect_err("rename source must not target sandbox root");
        let rename_to_err = adapter
            .rename("source.txt", ".")
            .expect_err("rename destination must not target sandbox root");

        assert_eq!(remove_err, AdapterError::InvalidPath);
        assert_eq!(rename_err, AdapterError::InvalidPath);
        assert_eq!(rename_to_err, AdapterError::InvalidPath);
        assert!(temp.exists(), "sandbox root should still exist");

        drop(adapter);
        std::fs::remove_dir_all(temp).expect("remove fixture directory");
    }

    #[cfg(all(feature = "phase2-bindings", unix))]
    #[test]
    fn local_fs_adapter_rejects_relative_symlink_escape_from_sandbox_root() {
        use std::os::unix::fs::symlink;

        let unique = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        let temp = std::env::temp_dir().join(format!(
            "krate-sandbox-symlink-{}-{unique}",
            std::process::id()
        ));
        let sandbox = temp.join("sandbox");
        let outside = temp.join("outside");
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        std::fs::create_dir_all(&outside).expect("create outside dir");

        let outside_file = outside.join("secret.txt");
        std::fs::write(&outside_file, b"secret").expect("write outside file");
        let inside_file = sandbox.join("inside.txt");
        std::fs::write(&inside_file, b"inside").expect("write inside file");
        symlink(&outside_file, sandbox.join("secret-link.txt")).expect("create file symlink");
        symlink(&inside_file, sandbox.join("inside-link.txt")).expect("create inside file symlink");
        symlink(&outside, sandbox.join("outside-dir")).expect("create directory symlink");
        symlink(outside.join("missing.txt"), sandbox.join("broken-link.txt"))
            .expect("create broken file symlink");

        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            None,
            None,
            Vec::new(),
            1024,
            sandbox,
        );

        let read_err = adapter
            .open("secret-link.txt", OpenMode::Read)
            .expect_err("symlinked read target outside sandbox should be denied");
        let write_err = adapter
            .open("outside-dir/new.txt", OpenMode::Write)
            .expect_err("new file below symlinked outside parent should be denied");
        let broken_write_err = adapter
            .open("broken-link.txt", OpenMode::Write)
            .expect_err("write through broken symlink should be denied");
        let inside_link_err = adapter
            .open("inside-link.txt", OpenMode::Read)
            .expect_err("final symlink should not be followed during open");

        assert_eq!(read_err, AdapterError::PermissionDenied);
        assert_eq!(write_err, AdapterError::PermissionDenied);
        assert_eq!(broken_write_err, AdapterError::PermissionDenied);
        assert_eq!(inside_link_err, AdapterError::PermissionDenied);

        drop(adapter);
        std::fs::remove_dir_all(temp).expect("remove fixture directory");
    }

    #[cfg(all(feature = "phase2-bindings", unix))]
    #[test]
    fn local_fs_adapter_rejects_symlinked_segments_inside_sandbox() {
        use std::os::unix::fs::symlink;

        let unique = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        let temp = std::env::temp_dir().join(format!(
            "krate-sandbox-inner-symlink-{}-{unique}",
            std::process::id()
        ));
        let sandbox = temp.join("sandbox");
        let real = sandbox.join("real");
        std::fs::create_dir_all(&real).expect("create real directory");
        std::fs::write(real.join("inside.txt"), b"inside").expect("write fixture");
        symlink(&real, sandbox.join("alias")).expect("create in-sandbox symlink");

        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            None,
            None,
            Vec::new(),
            1024,
            sandbox,
        );

        let open_err = adapter
            .open("alias/inside.txt", OpenMode::Read)
            .expect_err("symlinked path segment should be denied");
        let create_err = adapter
            .open("alias/new.txt", OpenMode::Write)
            .expect_err("write through symlinked segment should be denied");

        assert_eq!(open_err, AdapterError::PermissionDenied);
        assert_eq!(create_err, AdapterError::PermissionDenied);

        drop(adapter);
        std::fs::remove_dir_all(temp).expect("remove fixture directory");
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn plain_http_request_builder_forwards_method_headers_and_body() {
        let url =
            PlainHttpUrl::parse("http://127.0.0.1:8080/submit?name=krate").expect("parse HTTP URL");
        let req = HttpRequest {
            method: uapi_dispatch::HttpMethod::Post,
            url: "http://127.0.0.1:8080/submit?name=krate".to_string(),
            headers: vec![Header {
                name: "X-Krate".to_string(),
                value: "yes".to_string(),
            }],
            body: b"payload".to_vec(),
            timeout_millis: Some(1000),
        };

        let request = build_plain_http_request(&plain_http_request_from_dispatch(&req), &url)
            .expect("build HTTP request");
        let request = String::from_utf8(request).expect("request is UTF-8");

        assert!(request.starts_with("POST /submit?name=krate HTTP/1.1\r\n"));
        assert!(request.contains("Host: 127.0.0.1\r\n"));
        assert!(request.contains("Connection: close\r\n"));
        assert!(request.contains("X-Krate: yes\r\n"));
        assert!(request.contains("Content-Length: 7\r\n"));
        assert!(request.ends_with("\r\n\r\npayload"));
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn plain_http_request_builder_rejects_host_controlled_headers() {
        let url = PlainHttpUrl::parse("http://127.0.0.1:8080/").expect("parse HTTP URL");
        let req = HttpRequest {
            method: uapi_dispatch::HttpMethod::Get,
            url: "http://127.0.0.1:8080/".to_string(),
            headers: vec![Header {
                name: "Content-Length".to_string(),
                value: "999".to_string(),
            }],
            body: Vec::new(),
            timeout_millis: None,
        };

        let err = build_plain_http_request(&plain_http_request_from_dispatch(&req), &url)
            .map_err(map_plain_http_error)
            .expect_err("host-controlled headers should be rejected");

        assert!(
            matches!(err, AdapterError::Protocol(message) if message == "host-controlled HTTP header")
        );
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn local_plain_http_adapter_zero_timeout_fails_as_timeout() {
        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            None,
            None,
            Vec::new(),
            1024,
            PathBuf::from("."),
        );
        let req = HttpRequest {
            method: uapi_dispatch::HttpMethod::Get,
            url: "http://127.0.0.1:1/".to_string(),
            headers: Vec::new(),
            body: Vec::new(),
            timeout_millis: Some(0),
        };

        let err = adapter.fetch(req).expect_err("zero timeout should fail");
        assert_eq!(err, AdapterError::Timeout);
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn map_net_io_error_classifies_connection_refused_as_network() {
        let err = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
        let mapped = map_net_io_error(err);
        assert!(matches!(mapped, AdapterError::Network(_)));
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn map_net_io_error_keeps_timeout_as_timeout() {
        let err = std::io::Error::from(std::io::ErrorKind::TimedOut);
        let mapped = map_net_io_error(err);
        assert_eq!(mapped, AdapterError::Timeout);
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn map_net_resolve_error_maps_not_found_to_adapter_not_found() {
        let err = std::io::Error::from(std::io::ErrorKind::NotFound);
        let mapped = map_net_resolve_error(err);
        assert_eq!(mapped, AdapterError::NotFound);
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn connect_plain_http_stream_without_timeout_maps_connection_refused_as_network() {
        let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("skipping local connect-refused test: bind is not permitted");
                return;
            }
            Err(err) => panic!("bind local connect-refused fixture: {err}"),
        };
        let port = listener.local_addr().expect("fixture address").port();
        drop(listener);
        let url = PlainHttpUrl {
            host: "127.0.0.1".to_string(),
            port,
            path_and_query: "/".to_string(),
        };
        let err = connect_plain_http_stream(&url, None).expect_err("connection should fail");
        assert!(matches!(err, AdapterError::Network(_)));
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn normalize_resolved_socket_addrs_or_not_found_rejects_fully_filtered_sets() {
        let unusable = vec![
            SocketAddr::from(([0, 0, 0, 0], 8080)),
            SocketAddr::from(([127, 0, 0, 1], 0)),
            SocketAddr::from(([239, 1, 2, 3], 8080)),
        ];
        let err = normalize_resolved_socket_addrs_or_not_found(unusable)
            .expect_err("fully filtered address sets should be treated as not found");
        assert_eq!(err, AdapterError::NotFound);
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn normalize_resolved_socket_addrs_or_not_found_keeps_usable_targets() {
        let addrs = vec![
            SocketAddr::from(([0, 0, 0, 0], 8080)),
            SocketAddr::from(([127, 0, 0, 1], 8080)),
        ];
        let normalized = normalize_resolved_socket_addrs_or_not_found(addrs)
            .expect("usable addresses should remain after normalization");
        assert_eq!(normalized, vec![SocketAddr::from(([127, 0, 0, 1], 8080))]);
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn remaining_connect_timeout_for_returns_timeout_at_deadline() {
        let now = std::time::Instant::now();
        let err = remaining_connect_timeout_for(now, now)
            .expect_err("deadline at current instant should time out");
        assert_eq!(err, AdapterError::Timeout);
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn remaining_connect_timeout_for_returns_remaining_budget_before_deadline() {
        let now = std::time::Instant::now();
        let deadline = now + Duration::from_millis(25);
        let remaining = remaining_connect_timeout_for(now, deadline)
            .expect("remaining timeout should be available before deadline");
        assert_eq!(remaining, Duration::from_millis(25));
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn local_plain_http_adapter_sends_post_body_and_parses_response() {
        let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("skipping local HTTP adapter socket test: bind is not permitted");
                return;
            }
            Err(err) => panic!("bind HTTP fixture: {err}"),
        };
        let addr = listener.local_addr().expect("fixture address");
        let (tx, rx) = std::sync::mpsc::channel();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept HTTP request");
            let mut request = Vec::new();
            let mut chunk = [0; 512];

            loop {
                let read = stream.read(&mut chunk).expect("read HTTP request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);

                if request.windows(4).any(|window| window == b"\r\n\r\n")
                    && request.ends_with(b"payload")
                {
                    break;
                }
            }

            tx.send(String::from_utf8(request).expect("request is UTF-8"))
                .expect("send captured request");
            stream
                .write_all(b"HTTP/1.1 201 Created\r\nContent-Type: text/plain\r\n\r\naccepted")
                .expect("write HTTP response");
        });

        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
            None,
            None,
            None,
            Vec::new(),
            1024,
            PathBuf::from("."),
        );
        let response = adapter
            .fetch(HttpRequest {
                method: uapi_dispatch::HttpMethod::Post,
                url: format!("http://{addr}/submit"),
                headers: vec![Header {
                    name: "X-Krate".to_string(),
                    value: "yes".to_string(),
                }],
                body: b"payload".to_vec(),
                timeout_millis: Some(1000),
            })
            .expect("POST should succeed");

        server.join().expect("server joins");
        let request = rx.recv().expect("captured request");

        assert!(request.starts_with("POST /submit HTTP/1.1\r\n"));
        assert!(request.contains("X-Krate: yes\r\n"));
        assert!(request.contains("Content-Length: 7\r\n"));
        assert!(request.ends_with("\r\n\r\npayload"));
        assert_eq!(response.status, 201);
        assert_eq!(response.body, b"accepted");
    }

    #[test]
    fn plain_http_response_parser_splits_headers_and_body() {
        let response = parse_plain_http_response(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nhello\n",
        )
        .expect("parse HTTP response");

        assert_eq!(response.status, 200);
        assert_eq!(
            response.headers,
            vec![PlainHttpHeader {
                name: "Content-Type".to_string(),
                value: "text/plain".to_string()
            }]
        );
        assert_eq!(response.body, b"hello\n");
    }

    #[test]
    fn plain_http_response_parser_reports_protocol_errors() {
        let err = parse_plain_http_response(b"not http")
            .expect_err("malformed response should be rejected");

        assert_eq!(err, PlainHttpError::InvalidResponse);
    }

    #[test]
    fn plain_http_response_reader_enforces_size_limit() {
        let mut response = std::io::Cursor::new(vec![b'x'; 5]);
        let err = read_plain_http_response_limited(&mut response, 4)
            .expect_err("oversized response should be rejected");
        let err = map_plain_http_read_error(err);

        assert_eq!(err, AdapterError::BodyTooLarge);
    }

    #[test]
    fn plain_http_response_reader_allows_exact_size_limit() {
        let mut response = std::io::Cursor::new(vec![b'x'; 4]);
        let bytes = read_plain_http_response_limited(&mut response, 4)
            .expect("exact limit should be accepted");

        assert_eq!(bytes.len(), 4);
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn phase2_cli_linker_installs() {
        let config = Config::default();
        let runtime = Runtime::new(&config).expect("runtime should initialize");
        let mut store = runtime
            .new_store(&config, OutputMode::Sink)
            .expect("store should initialize");
        let mut linker = wasmtime::component::Linker::new(&runtime.engine);

        phase2_bindings::Cli::add_to_linker::<_, HasSelf<_>>(
            &mut linker,
            |state: &mut HostState| state.phase2(),
        )
        .expect("Phase 2 UAPI imports should link");

        store.limiter(|state| &mut state.limits);
    }
}

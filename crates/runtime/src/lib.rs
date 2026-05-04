//! Layer36 runtime: Phase 1 proof of concept.
//!
//! Phase 1 intentionally exposes only one temporary host interface:
//! `layer36:phase1/host` with `print(string)` and `exit(s32)`.

use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    rc::Rc,
    time::UNIX_EPOCH,
};

use thiserror::Error;
use wasmtime::{
    component::{Component, HasSelf},
    Engine, ResourceLimiter, Store, Trap,
};

pub mod uapi;
pub mod uapi_dispatch;

#[cfg(feature = "phase2-bindings")]
pub mod phase2_bindings;
#[cfg(feature = "phase2-bindings")]
pub mod phase2_bridge;
#[cfg(feature = "phase2-bindings")]
pub mod phase2_host;

#[cfg(feature = "phase2-bindings")]
use layer36_adapter_common::locale::{
    DateStyle as HostDateStyle, HostLocale, LocaleId as HostLocaleId,
    NumberStyle as HostNumberStyle,
};
#[cfg(feature = "phase2-bindings")]
use layer36_adapter_common::net::{
    build_plain_http_request, PlainHttpError, PlainHttpHeader, PlainHttpMethod, PlainHttpRequest,
    PlainHttpUrl,
};
#[cfg(feature = "phase2-bindings")]
use layer36_adapter_common::path::{LogicalPath, PathError};
#[cfg(feature = "phase2-bindings")]
use layer36_adapter_common::time::{HostClock, TimeError};
use layer36_policy::SessionPolicy;
use uapi::UapiGuard;
use uapi_dispatch::{
    AdapterError, DateStyle, FileHandle, FileStat, FsAdapter, Header, HostAdapter, HttpRequest,
    HttpResponse, IoAdapter, LocaleAdapter, LocaleId, NetAdapter, OpenMode, TimeAdapter,
};

pub const DEFAULT_MAX_HTTP_RESPONSE_BYTES: usize = 1024 * 1024;

wasmtime::component::bindgen!({
    path: "../../wit/layer36",
    world: "app",
});

/// Runtime configuration for a single `layer36 run` invocation.
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
    /// Arguments exposed to Phase 2 apps through `layer36:io/args`.
    pub app_args: Vec<String>,
    /// Maximum full HTTP response size accepted by the local Phase 2 adapter.
    pub max_http_response_bytes: usize,
    /// Root directory used to resolve relative Phase 2 filesystem paths.
    pub sandbox_root: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            fuel: None,
            memory_bytes: 256 * 1024 * 1024,
            session_policy: SessionPolicy::default(),
            test_time_millis: None,
            app_args: Vec::new(),
            max_http_response_bytes: DEFAULT_MAX_HTTP_RESPONSE_BYTES,
            sandbox_root: PathBuf::from("."),
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
        let bytes = std::fs::read(path)?;
        self.run_bytes(&bytes, config)
    }

    pub fn run_bytes(&self, bytes: &[u8], config: &Config) -> Result<RunOutcome> {
        self.run_bytes_with_output(bytes, config, OutputMode::Stdout)
    }

    pub fn run_bytes_silent(&self, bytes: &[u8], config: &Config) -> Result<RunOutcome> {
        self.run_bytes_with_output(bytes, config, OutputMode::Sink)
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
                    return self.run_phase2_component(component, config, output);
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
}

impl HostState {
    fn new(config: &Config, output: OutputMode) -> Result<Self> {
        let memory_bytes = usize::try_from(config.memory_bytes)
            .map_err(|_| RuntimeError::EngineInit("memory limit is too large".to_string()))?;
        let output = Rc::new(RefCell::new(output));
        std::fs::create_dir_all(&config.sandbox_root)?;

        Ok(Self {
            exit_code: None,
            limits: Phase1Limits { memory_bytes },
            output: output.clone(),
            _uapi: UapiGuard::new(config.session_policy.clone()),
            #[cfg(feature = "phase2-bindings")]
            phase2: phase2_host::Phase2Host::new(
                UapiGuard::new(config.session_policy.clone()),
                Box::new(LocalPhase2Adapter::new(
                    output,
                    config.test_time_millis,
                    config.app_args.clone(),
                    config.max_http_response_bytes,
                    config.sandbox_root.clone(),
                )),
            ),
        })
    }

    #[cfg(feature = "phase2-bindings")]
    fn phase2(&mut self) -> &mut phase2_host::Phase2Host<'static> {
        &mut self.phase2
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
}

impl OutputMode {
    fn print_line(&mut self, msg: &str) {
        match self {
            Self::Stdout => println!("{msg}"),
            Self::Sink => {}
        }
    }

    fn write_bytes(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Stdout => std::io::stdout().write_all(bytes),
            Self::Sink => Ok(()),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Stdout => std::io::stdout().flush(),
            Self::Sink => Ok(()),
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

impl layer36::phase1::host::Host for HostState {
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
        app_args: Vec<String>,
        max_http_response_bytes: usize,
        sandbox_root: PathBuf,
    ) -> Self {
        Self {
            output,
            state: RefCell::new(LocalPhase2AdapterState::default()),
            clock: HostClock::new(test_time_millis),
            locale: HostLocale::from_env(),
            app_args,
            max_http_response_bytes,
            sandbox_root,
        }
    }

    fn insert_resource(&self, resource: LocalResource) -> FileHandle {
        let mut state = self.state.borrow_mut();
        let id = state.next_id;
        state.next_id += 1;
        state.resources.insert(id, resource);
        FileHandle::resource(id)
    }

    fn resolve_fs_path(
        &self,
        path: &str,
        missing_leaf: MissingLeaf,
    ) -> std::result::Result<PathBuf, AdapterError> {
        let logical = normalize_fs_path(path)?;
        let path = logical.to_path_buf();
        if path.is_absolute() {
            Ok(path)
        } else {
            self.resolve_sandboxed_fs_path(path, missing_leaf)
        }
    }

    fn resolve_sandboxed_fs_path(
        &self,
        path: PathBuf,
        missing_leaf: MissingLeaf,
    ) -> std::result::Result<PathBuf, AdapterError> {
        let root = self.sandbox_root.canonicalize().map_err(map_io_error)?;
        let host_path = root.join(path);

        match host_path.canonicalize() {
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
                let real_parent = parent.canonicalize().map_err(map_io_error)?;
                ensure_path_in_sandbox(&root, &real_parent)?;
                Ok(host_path)
            }
            Err(err) => Err(map_io_error(err)),
        }
    }
}

#[cfg(feature = "phase2-bindings")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MissingLeaf {
    Deny,
    Allow,
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
fn map_existing_broken_leaf_error(
    path: &Path,
    original: std::io::Error,
) -> std::result::Result<Option<AdapterError>, AdapterError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
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
        Ok(self.insert_resource(LocalResource::Stdin))
    }

    fn stdout(&self) -> std::result::Result<FileHandle, AdapterError> {
        Ok(self.insert_resource(LocalResource::Stdout))
    }

    fn stderr(&self) -> std::result::Result<FileHandle, AdapterError> {
        Ok(self.insert_resource(LocalResource::Stderr))
    }

    fn args_raw(&self) -> std::result::Result<String, AdapterError> {
        Ok(self.app_args.join("\n"))
    }

    fn read_stream(
        &self,
        handle: &FileHandle,
        n: u32,
    ) -> std::result::Result<Vec<u8>, AdapterError> {
        let mut state = self.state.borrow_mut();
        match state.resources.get_mut(&handle.id) {
            Some(LocalResource::Stdin) => {
                let mut buf = vec![0; n as usize];
                let len = std::io::stdin().read(&mut buf).map_err(map_io_error)?;
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
        self.write_all_stream(handle, bytes)?;
        Ok(bytes.len() as u32)
    }

    fn write_all_stream(
        &self,
        handle: &FileHandle,
        bytes: &[u8],
    ) -> std::result::Result<(), AdapterError> {
        let mut state = self.state.borrow_mut();
        match state.resources.get_mut(&handle.id) {
            Some(LocalResource::Stdout) => self.output.borrow_mut().write_bytes(bytes),
            Some(LocalResource::Stderr) => std::io::stderr().write_all(bytes),
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
            Some(LocalResource::Stderr) => std::io::stderr().flush(),
            Some(_) => Ok(()),
            None => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "unknown stream",
            )),
        }
        .map_err(map_io_error)
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
            OpenMode::Read => MissingLeaf::Deny,
            OpenMode::Write | OpenMode::ReadWrite | OpenMode::Append => MissingLeaf::Allow,
        };
        let path = self.resolve_fs_path(path, missing_leaf)?;
        let mut opts = std::fs::OpenOptions::new();
        apply_no_follow_final_symlink(&mut opts);
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
        let file = opts.open(path).map_err(map_io_error)?;
        Ok(self.insert_resource(LocalResource::File(file)))
    }

    fn read(&self, handle: &FileHandle, n: u32) -> std::result::Result<Vec<u8>, AdapterError> {
        let mut state = self.state.borrow_mut();
        let Some(LocalResource::File(file)) = state.resources.get_mut(&handle.id) else {
            return Err(AdapterError::NotFound);
        };
        let mut buf = vec![0; n as usize];
        let len = file.read(&mut buf).map_err(map_io_error)?;
        buf.truncate(len);
        Ok(buf)
    }

    fn write(&self, handle: &FileHandle, bytes: &[u8]) -> std::result::Result<u32, AdapterError> {
        let mut state = self.state.borrow_mut();
        let Some(LocalResource::File(file)) = state.resources.get_mut(&handle.id) else {
            return Err(AdapterError::NotFound);
        };
        let len = file.write(bytes).map_err(map_io_error)?;
        Ok(len as u32)
    }

    fn seek_set(&self, handle: &FileHandle, pos: u64) -> std::result::Result<u64, AdapterError> {
        let mut state = self.state.borrow_mut();
        let Some(LocalResource::File(file)) = state.resources.get_mut(&handle.id) else {
            return Err(AdapterError::NotFound);
        };
        file.seek(SeekFrom::Start(pos)).map_err(map_io_error)
    }

    fn seek_end(&self, handle: &FileHandle) -> std::result::Result<u64, AdapterError> {
        let mut state = self.state.borrow_mut();
        let Some(LocalResource::File(file)) = state.resources.get_mut(&handle.id) else {
            return Err(AdapterError::NotFound);
        };
        file.seek(SeekFrom::End(0)).map_err(map_io_error)
    }

    fn stat_handle(&self, handle: &FileHandle) -> std::result::Result<FileStat, AdapterError> {
        let state = self.state.borrow();
        let Some(LocalResource::File(file)) = state.resources.get(&handle.id) else {
            return Err(AdapterError::NotFound);
        };
        file.metadata()
            .map(file_stat_from_metadata)
            .map_err(map_io_error)
    }

    fn stat(&self, path: &str) -> std::result::Result<FileStat, AdapterError> {
        let path = self.resolve_fs_path(path, MissingLeaf::Deny)?;
        std::fs::metadata(path)
            .map(file_stat_from_metadata)
            .map_err(map_io_error)
    }

    fn list(&self, path: &str) -> std::result::Result<Vec<String>, AdapterError> {
        let path = self.resolve_fs_path(path, MissingLeaf::Deny)?;
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(path).map_err(map_io_error)? {
            let entry = entry.map_err(map_io_error)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| AdapterError::InvalidPath)?;
            entries.push(name);
        }
        entries.sort();
        Ok(entries)
    }

    fn remove_file(&self, path: &str) -> std::result::Result<(), AdapterError> {
        let path = self.resolve_fs_path(path, MissingLeaf::Deny)?;
        std::fs::remove_file(path).map_err(map_io_error)
    }

    fn remove_dir(&self, path: &str) -> std::result::Result<(), AdapterError> {
        let path = self.resolve_fs_path(path, MissingLeaf::Deny)?;
        std::fs::remove_dir(path).map_err(map_io_error)
    }

    fn mkdir(&self, path: &str) -> std::result::Result<(), AdapterError> {
        let path = self.resolve_fs_path(path, MissingLeaf::Allow)?;
        std::fs::create_dir(path).map_err(map_io_error)
    }

    fn rename(&self, from: &str, to: &str) -> std::result::Result<(), AdapterError> {
        let from = self.resolve_fs_path(from, MissingLeaf::Deny)?;
        let to = self.resolve_fs_path(to, MissingLeaf::Allow)?;
        std::fs::rename(from, to).map_err(map_io_error)
    }
}

#[cfg(all(feature = "phase2-bindings", unix))]
fn apply_no_follow_final_symlink(opts: &mut std::fs::OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    opts.custom_flags(libc::O_NOFOLLOW);
}

#[cfg(all(feature = "phase2-bindings", not(unix)))]
fn apply_no_follow_final_symlink(_opts: &mut std::fs::OpenOptions) {}

#[cfg(feature = "phase2-bindings")]
impl NetAdapter for LocalPhase2Adapter {
    fn fetch(&self, req: HttpRequest) -> std::result::Result<HttpResponse, AdapterError> {
        let url = PlainHttpUrl::parse(&req.url).map_err(map_plain_http_error)?;
        let plain_req = plain_http_request_from_dispatch(&req);
        let request = build_plain_http_request(&plain_req, &url).map_err(map_plain_http_error)?;

        let mut stream = TcpStream::connect((url.host.as_str(), url.port))
            .map_err(|err| AdapterError::Network(err.to_string()))?;
        if let Some(millis) = req.timeout_millis {
            let timeout = std::time::Duration::from_millis(millis.into());
            stream
                .set_read_timeout(Some(timeout))
                .map_err(map_io_error)?;
            stream
                .set_write_timeout(Some(timeout))
                .map_err(map_io_error)?;
        }

        stream.write_all(&request).map_err(map_net_io_error)?;
        let response = read_http_response_limited(&mut stream, self.max_http_response_bytes)?;
        parse_http_response(&response)
    }
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
        PlainHttpError::HostControlledHeader => {
            AdapterError::Protocol("host-controlled HTTP header".to_string())
        }
    }
}

#[cfg(feature = "phase2-bindings")]
fn normalize_fs_path(path: &str) -> std::result::Result<LogicalPath, AdapterError> {
    LogicalPath::parse(path).map_err(map_path_error)
}

#[cfg(feature = "phase2-bindings")]
fn map_path_error(err: PathError) -> AdapterError {
    match err {
        PathError::Empty | PathError::ControlCharacter | PathError::ParentTraversal => {
            AdapterError::InvalidPath
        }
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
        HostClock::sleep_millis(millis);
        Ok(())
    }
}

#[cfg(feature = "phase2-bindings")]
impl LocaleAdapter for LocalPhase2Adapter {
    fn current(&self) -> std::result::Result<LocaleId, AdapterError> {
        Ok(locale_from_host(self.locale.current()))
    }

    fn timezone(&self) -> std::result::Result<String, AdapterError> {
        Ok(self.locale.timezone())
    }

    fn format_date(
        &self,
        millis: u64,
        tz: &str,
        style: DateStyle,
        loc: &LocaleId,
    ) -> std::result::Result<String, AdapterError> {
        Ok(HostLocale::format_date(
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
        Ok(HostLocale::format_number(
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
        _ => map_io_error(err),
    }
}

fn read_http_response_limited(
    reader: &mut impl Read,
    max_bytes: usize,
) -> std::result::Result<Vec<u8>, AdapterError> {
    let mut response = Vec::new();
    let mut chunk = [0; 8192];

    loop {
        let read = reader.read(&mut chunk).map_err(map_net_io_error)?;
        if read == 0 {
            return Ok(response);
        }

        if response.len() + read > max_bytes {
            return Err(AdapterError::BodyTooLarge);
        }

        response.extend_from_slice(&chunk[..read]);
    }
}

fn parse_http_response(bytes: &[u8]) -> std::result::Result<HttpResponse, AdapterError> {
    let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
        return Err(AdapterError::Protocol("invalid HTTP response".to_string()));
    };
    let header_bytes = &bytes[..header_end];
    let body = bytes[header_end + 4..].to_vec();
    let headers_text =
        std::str::from_utf8(header_bytes).map_err(|err| AdapterError::Protocol(err.to_string()))?;
    let mut lines = headers_text.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| AdapterError::Protocol("missing HTTP status".to_string()))?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| AdapterError::Protocol("missing HTTP status code".to_string()))?
        .parse()
        .map_err(|_| AdapterError::Protocol("invalid HTTP status code".to_string()))?;

    let mut headers = Vec::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        headers.push(Header {
            name: name.trim().to_string(),
            value: value.trim().to_string(),
        });
    }

    Ok(HttpResponse {
        status,
        headers,
        body,
    })
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

    #[test]
    fn default_config_sets_phase_1_memory_cap() {
        assert_eq!(Config::default().memory_bytes, 256 * 1024 * 1024);
        assert_eq!(
            Config::default().max_http_response_bytes,
            DEFAULT_MAX_HTTP_RESPONSE_BYTES
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
        let parsed = PlainHttpUrl::parse("http://127.0.0.1:8080?name=layer36#local")
            .expect("parse HTTP URL");

        assert_eq!(parsed.host, "127.0.0.1");
        assert_eq!(parsed.port, 8080);
        assert_eq!(parsed.path_and_query, "/?name=layer36");
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn fs_path_normalizer_rejects_parent_traversal_before_host_io() {
        let err = normalize_fs_path("fixtures/../secret.txt").expect_err("path should be rejected");

        assert_eq!(err, AdapterError::InvalidPath);
    }

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn local_fs_adapter_normalizes_portable_separators() {
        let temp =
            std::env::temp_dir().join(format!("layer36-path-normalize-{}", std::process::id()));
        let nested = temp.join("fixtures").join("public");
        std::fs::create_dir_all(&nested).expect("create fixture directory");
        let file = nested.join("note.txt");
        std::fs::write(&file, b"hello").expect("write fixture file");

        let adapter = LocalPhase2Adapter::new(
            Rc::new(RefCell::new(OutputMode::Sink)),
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

    #[cfg(all(feature = "phase2-bindings", unix))]
    #[test]
    fn local_fs_adapter_rejects_relative_symlink_escape_from_sandbox_root() {
        use std::os::unix::fs::symlink;

        let unique = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        let temp = std::env::temp_dir().join(format!(
            "layer36-sandbox-symlink-{}-{unique}",
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

    #[cfg(feature = "phase2-bindings")]
    #[test]
    fn plain_http_request_builder_forwards_method_headers_and_body() {
        let url = PlainHttpUrl::parse("http://127.0.0.1:8080/submit?name=layer36")
            .expect("parse HTTP URL");
        let req = HttpRequest {
            method: uapi_dispatch::HttpMethod::Post,
            url: "http://127.0.0.1:8080/submit?name=layer36".to_string(),
            headers: vec![Header {
                name: "X-Layer36".to_string(),
                value: "yes".to_string(),
            }],
            body: b"payload".to_vec(),
            timeout_millis: Some(1000),
        };

        let request = build_plain_http_request(&plain_http_request_from_dispatch(&req), &url)
            .expect("build HTTP request");
        let request = String::from_utf8(request).expect("request is UTF-8");

        assert!(request.starts_with("POST /submit?name=layer36 HTTP/1.1\r\n"));
        assert!(request.contains("Host: 127.0.0.1\r\n"));
        assert!(request.contains("Connection: close\r\n"));
        assert!(request.contains("X-Layer36: yes\r\n"));
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
            Vec::new(),
            1024,
            PathBuf::from("."),
        );
        let response = adapter
            .fetch(HttpRequest {
                method: uapi_dispatch::HttpMethod::Post,
                url: format!("http://{addr}/submit"),
                headers: vec![Header {
                    name: "X-Layer36".to_string(),
                    value: "yes".to_string(),
                }],
                body: b"payload".to_vec(),
                timeout_millis: Some(1000),
            })
            .expect("POST should succeed");

        server.join().expect("server joins");
        let request = rx.recv().expect("captured request");

        assert!(request.starts_with("POST /submit HTTP/1.1\r\n"));
        assert!(request.contains("X-Layer36: yes\r\n"));
        assert!(request.contains("Content-Length: 7\r\n"));
        assert!(request.ends_with("\r\n\r\npayload"));
        assert_eq!(response.status, 201);
        assert_eq!(response.body, b"accepted");
    }

    #[test]
    fn plain_http_response_parser_splits_headers_and_body() {
        let response =
            parse_http_response(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nhello\n")
                .expect("parse HTTP response");

        assert_eq!(response.status, 200);
        assert_eq!(
            response.headers,
            vec![Header {
                name: "Content-Type".to_string(),
                value: "text/plain".to_string()
            }]
        );
        assert_eq!(response.body, b"hello\n");
    }

    #[test]
    fn plain_http_response_parser_reports_protocol_errors() {
        let err =
            parse_http_response(b"not http").expect_err("malformed response should be rejected");

        assert!(
            matches!(err, AdapterError::Protocol(message) if message == "invalid HTTP response")
        );
    }

    #[test]
    fn plain_http_response_reader_enforces_size_limit() {
        let mut response = std::io::Cursor::new(vec![b'x'; 5]);
        let err = read_http_response_limited(&mut response, 4)
            .expect_err("oversized response should be rejected");

        assert_eq!(err, AdapterError::BodyTooLarge);
    }

    #[test]
    fn plain_http_response_reader_allows_exact_size_limit() {
        let mut response = std::io::Cursor::new(vec![b'x'; 4]);
        let bytes =
            read_http_response_limited(&mut response, 4).expect("exact limit should be accepted");

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

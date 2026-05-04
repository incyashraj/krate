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
    path::Path,
    rc::Rc,
    time::{Instant, SystemTime, UNIX_EPOCH},
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

use layer36_policy::SessionPolicy;
use uapi::UapiGuard;
use uapi_dispatch::{
    AdapterError, DateStyle, FileHandle, FileStat, FsAdapter, Header, HostAdapter, HttpRequest,
    HttpResponse, IoAdapter, LocaleAdapter, LocaleId, NetAdapter, OpenMode, TimeAdapter,
};

const MAX_HTTP_RESPONSE_BYTES: usize = 1024 * 1024;

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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            fuel: None,
            memory_bytes: 256 * 1024 * 1024,
            session_policy: SessionPolicy::default(),
            test_time_millis: None,
            app_args: Vec::new(),
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
    started: Instant,
    test_time_millis: Option<u64>,
    app_args: Vec<String>,
}

#[cfg(feature = "phase2-bindings")]
impl LocalPhase2Adapter {
    fn new(
        output: Rc<RefCell<OutputMode>>,
        test_time_millis: Option<u64>,
        app_args: Vec<String>,
    ) -> Self {
        Self {
            output,
            state: RefCell::new(LocalPhase2AdapterState::default()),
            started: Instant::now(),
            test_time_millis,
            app_args,
        }
    }

    fn insert_resource(&self, resource: LocalResource) -> FileHandle {
        let mut state = self.state.borrow_mut();
        let id = state.next_id;
        state.next_id += 1;
        state.resources.insert(id, resource);
        FileHandle::resource(id)
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
        let mut opts = std::fs::OpenOptions::new();
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
        std::fs::metadata(path)
            .map(file_stat_from_metadata)
            .map_err(map_io_error)
    }

    fn list(&self, path: &str) -> std::result::Result<Vec<String>, AdapterError> {
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
        std::fs::remove_file(path).map_err(map_io_error)
    }

    fn remove_dir(&self, path: &str) -> std::result::Result<(), AdapterError> {
        std::fs::remove_dir(path).map_err(map_io_error)
    }

    fn mkdir(&self, path: &str) -> std::result::Result<(), AdapterError> {
        std::fs::create_dir(path).map_err(map_io_error)
    }

    fn rename(&self, from: &str, to: &str) -> std::result::Result<(), AdapterError> {
        std::fs::rename(from, to).map_err(map_io_error)
    }
}

#[cfg(feature = "phase2-bindings")]
impl NetAdapter for LocalPhase2Adapter {
    fn fetch(&self, req: HttpRequest) -> std::result::Result<HttpResponse, AdapterError> {
        let url = ParsedHttpUrl::parse(&req.url)?;
        if !matches!(req.method, uapi_dispatch::HttpMethod::Get) {
            return Err(AdapterError::Unsupported);
        }

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

        let mut request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
            url.path_and_query, url.host
        )
        .into_bytes();
        for header in req.headers {
            if header.name.contains(['\r', '\n']) || header.value.contains(['\r', '\n']) {
                return Err(AdapterError::Network("invalid HTTP header".to_string()));
            }
            request.extend_from_slice(header.name.as_bytes());
            request.extend_from_slice(b": ");
            request.extend_from_slice(header.value.as_bytes());
            request.extend_from_slice(b"\r\n");
        }
        request.extend_from_slice(b"\r\n");

        stream.write_all(&request).map_err(map_io_error)?;
        let response = read_http_response_limited(&mut stream)?;
        parse_http_response(&response)
    }
}

#[cfg(feature = "phase2-bindings")]
impl TimeAdapter for LocalPhase2Adapter {
    fn now_millis(&self) -> std::result::Result<u64, AdapterError> {
        if let Some(millis) = self.test_time_millis {
            return Ok(millis);
        }

        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| AdapterError::Io(err.to_string()))?
            .as_millis();
        Ok(millis as u64)
    }

    fn monotonic_nanos(&self) -> std::result::Result<u64, AdapterError> {
        Ok(self.started.elapsed().as_nanos() as u64)
    }

    fn sleep_millis(&self, millis: u32) -> std::result::Result<(), AdapterError> {
        std::thread::sleep(std::time::Duration::from_millis(millis.into()));
        Ok(())
    }
}

#[cfg(feature = "phase2-bindings")]
impl LocaleAdapter for LocalPhase2Adapter {
    fn current(&self) -> std::result::Result<LocaleId, AdapterError> {
        let bcp47 = std::env::var("LC_ALL")
            .or_else(|_| std::env::var("LANG"))
            .unwrap_or_else(|_| "en-US".to_string())
            .split('.')
            .next()
            .unwrap_or("en-US")
            .replace('_', "-");
        Ok(LocaleId { bcp47 })
    }

    fn timezone(&self) -> std::result::Result<String, AdapterError> {
        Ok(std::env::var("TZ").unwrap_or_else(|_| "UTC".to_string()))
    }

    fn format_date(
        &self,
        millis: u64,
        tz: &str,
        style: DateStyle,
        loc: &LocaleId,
    ) -> std::result::Result<String, AdapterError> {
        Ok(format!("{millis}:{tz}:{style:?}:{}", loc.bcp47))
    }

    fn format_number(
        &self,
        value: f64,
        style: uapi_dispatch::NumberStyle,
        loc: &LocaleId,
    ) -> std::result::Result<String, AdapterError> {
        Ok(format!("{value}:{style:?}:{}", loc.bcp47))
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
    match err.kind() {
        std::io::ErrorKind::NotFound => AdapterError::NotFound,
        std::io::ErrorKind::PermissionDenied => AdapterError::PermissionDenied,
        std::io::ErrorKind::AlreadyExists => AdapterError::Io("already exists".to_string()),
        std::io::ErrorKind::InvalidInput => AdapterError::InvalidPath,
        _ => AdapterError::Io(err.to_string()),
    }
}

struct ParsedHttpUrl {
    host: String,
    port: u16,
    path_and_query: String,
}

impl ParsedHttpUrl {
    fn parse(input: &str) -> std::result::Result<Self, AdapterError> {
        let Some(rest) = input.strip_prefix("http://") else {
            return Err(AdapterError::Unsupported);
        };
        let rest = rest.split_once('#').map_or(rest, |(before, _)| before);
        let (authority, path) = match rest.find(['/', '?']) {
            Some(index) => rest.split_at(index),
            None => (rest, "/"),
        };
        if authority.is_empty() || authority.contains('@') {
            return Err(AdapterError::InvalidPath);
        }

        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !host.is_empty() => {
                let port = port.parse().map_err(|_| AdapterError::InvalidPath)?;
                (host, port)
            }
            _ => (authority, 80),
        };

        let path_and_query = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };

        Ok(Self {
            host: host.to_string(),
            port,
            path_and_query,
        })
    }
}

fn read_http_response_limited(
    reader: &mut impl Read,
) -> std::result::Result<Vec<u8>, AdapterError> {
    let mut response = Vec::new();
    let mut chunk = [0; 8192];

    loop {
        let read = reader.read(&mut chunk).map_err(map_io_error)?;
        if read == 0 {
            return Ok(response);
        }

        if response.len() + read > MAX_HTTP_RESPONSE_BYTES {
            return Err(AdapterError::Network(format!(
                "HTTP response exceeds {MAX_HTTP_RESPONSE_BYTES} byte limit"
            )));
        }

        response.extend_from_slice(&chunk[..read]);
    }
}

fn parse_http_response(bytes: &[u8]) -> std::result::Result<HttpResponse, AdapterError> {
    let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
        return Err(AdapterError::Network("invalid HTTP response".to_string()));
    };
    let header_bytes = &bytes[..header_end];
    let body = bytes[header_end + 4..].to_vec();
    let headers_text =
        std::str::from_utf8(header_bytes).map_err(|err| AdapterError::Network(err.to_string()))?;
    let mut lines = headers_text.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| AdapterError::Network("missing HTTP status".to_string()))?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| AdapterError::Network("missing HTTP status code".to_string()))?
        .parse()
        .map_err(|_| AdapterError::Network("invalid HTTP status code".to_string()))?;

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

    #[test]
    fn plain_http_url_parser_normalizes_query_only_paths() {
        let parsed = ParsedHttpUrl::parse("http://127.0.0.1:8080?name=layer36#local")
            .expect("parse HTTP URL");

        assert_eq!(parsed.host, "127.0.0.1");
        assert_eq!(parsed.port, 8080);
        assert_eq!(parsed.path_and_query, "/?name=layer36");
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
    fn plain_http_response_reader_enforces_size_limit() {
        let mut response = std::io::Cursor::new(vec![b'x'; MAX_HTTP_RESPONSE_BYTES + 1]);
        let err = read_http_response_limited(&mut response)
            .expect_err("oversized response should be rejected");

        assert_eq!(
            err,
            AdapterError::Network(format!(
                "HTTP response exceeds {MAX_HTTP_RESPONSE_BYTES} byte limit"
            ))
        );
    }

    #[test]
    fn plain_http_response_reader_allows_exact_size_limit() {
        let mut response = std::io::Cursor::new(vec![b'x'; MAX_HTTP_RESPONSE_BYTES]);
        let bytes =
            read_http_response_limited(&mut response).expect("exact limit should be accepted");

        assert_eq!(bytes.len(), MAX_HTTP_RESPONSE_BYTES);
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

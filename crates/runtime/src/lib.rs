//! Layer36 runtime: Phase 1 proof of concept.
//!
//! Phase 1 intentionally exposes only one temporary host interface:
//! `layer36:phase1/host` with `print(string)` and `exit(s32)`.

use std::path::Path;

use thiserror::Error;
use wasmtime::{
    component::{Component, HasSelf},
    Engine, ResourceLimiter, Store, Trap,
};

pub mod uapi;

use layer36_policy::SessionPolicy;
use uapi::UapiGuard;

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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            fuel: None,
            memory_bytes: 256 * 1024 * 1024,
            session_policy: SessionPolicy::default(),
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
        let mut store = Store::new(&self.engine, HostState::new(config, output)?);
        store.limiter(|state| &mut state.limits);

        if let Some(fuel) = config.fuel {
            store
                .set_fuel(fuel)
                .map_err(|err| RuntimeError::EngineInit(err.to_string()))?;
        }

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
}

pub struct LoadedComponent {
    component: Component,
}

struct HostState {
    exit_code: Option<i32>,
    limits: Phase1Limits,
    output: OutputMode,
    _uapi: UapiGuard,
}

impl HostState {
    fn new(config: &Config, output: OutputMode) -> Result<Self> {
        let memory_bytes = usize::try_from(config.memory_bytes)
            .map_err(|_| RuntimeError::EngineInit("memory limit is too large".to_string()))?;

        Ok(Self {
            exit_code: None,
            limits: Phase1Limits { memory_bytes },
            output,
            _uapi: UapiGuard::new(config.session_policy.clone()),
        })
    }

    #[cfg(test)]
    fn uapi(&self) -> &UapiGuard {
        &self._uapi
    }
}

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
        self.output.print_line(&msg);
    }

    fn exit(&mut self, code: i32) {
        self.exit_code = Some(code);
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
}

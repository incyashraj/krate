//! Embedding API for running Layer36 components from other programs.
//!
//! This is the entry point for AI-agent frameworks and other hosts that want
//! to execute a portable component inside Layer36's capability sandbox
//! without a terminal: grants are supplied programmatically through
//! [`SessionPolicy`](layer36_policy::SessionPolicy), no prompt is ever shown,
//! and the app's stdout comes back captured next to a classified exit.
//!
//! ```no_run
//! use layer36_policy::SessionPolicy;
//! use layer36_runtime::{embed, Config};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let component = std::fs::read("app.wasm")?;
//!     let mut config = Config::default();
//!     config.session_policy = SessionPolicy::from_cli_grants(&[
//!         "fs.read:./data/**".to_string(),
//!     ])?;
//!
//!     let outcome = embed::run_component(&component, &config)?;
//!     println!("class: {}", outcome.exit_class().as_str());
//!     println!("stdout: {}", outcome.stdout_lossy());
//!     Ok(())
//! }
//! ```

use std::time::{Duration, Instant};

use crate::{Config, Result, RunOutcome, Runtime};

/// Exit code Layer36 CLI apps use for a capability denial.
pub const APP_EXIT_PERMISSION_DENIED: i32 = 5;

/// Classified result of an embedded component run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedExitClass {
    /// The app ran and returned exit code zero.
    Success,
    /// The app stopped itself after a capability denial (exit code 5 by
    /// Layer36 app convention).
    PermissionDenied,
    /// The app ran and returned a non-zero, app-defined exit code.
    AppError,
    /// The runtime stopped the app at a fuel or memory limit.
    LimitExceeded,
}

impl EmbedExitClass {
    /// Stable string form used by `layer36 run --json` and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::PermissionDenied => "permission-denied",
            Self::AppError => "app-error",
            Self::LimitExceeded => "limit-exceeded",
        }
    }
}

/// Outcome of one embedded component run.
#[derive(Debug, Clone)]
pub struct EmbedOutcome {
    outcome: RunOutcome,
    stdout: Vec<u8>,
    duration: Duration,
}

impl EmbedOutcome {
    /// Return the raw runtime outcome.
    pub fn outcome(&self) -> &RunOutcome {
        &self.outcome
    }

    /// Return the app exit code, if the app exited on its own.
    pub fn exit_code(&self) -> Option<i32> {
        match &self.outcome {
            RunOutcome::Exited(code) => Some(*code),
            RunOutcome::LimitExceeded(_) => None,
        }
    }

    /// Classify the run for policy-aware callers.
    pub fn exit_class(&self) -> EmbedExitClass {
        match &self.outcome {
            RunOutcome::Exited(0) => EmbedExitClass::Success,
            RunOutcome::Exited(code) if *code == APP_EXIT_PERMISSION_DENIED => {
                EmbedExitClass::PermissionDenied
            }
            RunOutcome::Exited(_) => EmbedExitClass::AppError,
            RunOutcome::LimitExceeded(_) => EmbedExitClass::LimitExceeded,
        }
    }

    /// Return the captured app stdout bytes.
    pub fn stdout(&self) -> &[u8] {
        &self.stdout
    }

    /// Return the captured app stdout as lossy UTF-8 text.
    pub fn stdout_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// Return how long the run took.
    pub fn duration(&self) -> Duration {
        self.duration
    }
}

/// Run component bytes inside the capability sandbox, capturing stdout.
///
/// Grants come from [`Config::session_policy`]; nothing is prompted. Errors
/// are runtime-level failures (invalid component, trap, engine setup) — a
/// capability denial inside the app is reported through
/// [`EmbedOutcome::exit_class`], not as an error.
pub fn run_component(component: &[u8], config: &Config) -> Result<EmbedOutcome> {
    let runtime = Runtime::new(config)?;
    let started = Instant::now();
    let (outcome, stdout) = runtime.run_bytes_captured(component, config)?;

    Ok(EmbedOutcome {
        outcome,
        stdout,
        duration: started.elapsed(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_classes_map_the_layer36_app_conventions() {
        let case = |outcome: RunOutcome| EmbedOutcome {
            outcome,
            stdout: Vec::new(),
            duration: Duration::from_millis(1),
        };

        assert_eq!(
            case(RunOutcome::Exited(0)).exit_class(),
            EmbedExitClass::Success
        );
        assert_eq!(
            case(RunOutcome::Exited(APP_EXIT_PERMISSION_DENIED)).exit_class(),
            EmbedExitClass::PermissionDenied
        );
        assert_eq!(
            case(RunOutcome::Exited(20)).exit_class(),
            EmbedExitClass::AppError
        );
        assert_eq!(
            case(RunOutcome::LimitExceeded("fuel".to_string())).exit_class(),
            EmbedExitClass::LimitExceeded
        );
        assert_eq!(case(RunOutcome::Exited(20)).exit_code(), Some(20));
    }

    #[test]
    fn embedded_run_captures_stdout_and_classifies_invalid_components() {
        let config = Config::default();
        let result = run_component(b"not a component", &config);
        assert!(matches!(
            result,
            Err(crate::RuntimeError::InvalidComponent(_))
        ));
    }
}

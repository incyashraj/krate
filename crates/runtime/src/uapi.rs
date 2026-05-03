//! Phase 2 UAPI capability checks.
//!
//! This module is the policy gate that future host adapters must pass through.
//! It maps each UAPI operation to the capability string required before native
//! host work begins.

use std::{fmt, str::FromStr};

use layer36_manifest::{Capability, ManifestError};
use layer36_policy::{PolicyError, SessionPolicy};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UapiCall {
    Io(IoCall),
    Fs(FsCall),
    Net(NetCall),
    Time(TimeCall),
    Locale(LocaleCall),
}

impl UapiCall {
    pub fn required_capability(&self) -> Result<Capability> {
        Capability::from_str(&self.to_capability_string()).map_err(Into::into)
    }

    fn to_capability_string(&self) -> String {
        match self {
            Self::Io(call) => format!("io.{call}"),
            Self::Fs(call) => call.to_capability_string(),
            Self::Net(NetCall::Connect { host, port }) => format!("net.connect:{host}:{port}"),
            Self::Time(call) => format!("time.{call}"),
            Self::Locale(call) => format!("locale.{call}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoCall {
    Stdin,
    Stdout,
    Stderr,
    Log,
}

impl fmt::Display for IoCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Stdin => "stdin",
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::Log => "log",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsCall {
    Read { path: String },
    Write { path: String },
    List { path: String },
    Remove { path: String },
    Mkdir { path: String },
}

impl FsCall {
    fn to_capability_string(&self) -> String {
        match self {
            Self::Read { path } => format!("fs.read:{path}"),
            Self::Write { path } => format!("fs.write:{path}"),
            Self::List { path } => format!("fs.list:{path}"),
            Self::Remove { path } => format!("fs.remove:{path}"),
            Self::Mkdir { path } => format!("fs.mkdir:{path}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetCall {
    Connect { host: String, port: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeCall {
    Clock,
    Monotonic,
    Sleep,
}

impl fmt::Display for TimeCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Clock => "clock",
            Self::Monotonic => "monotonic",
            Self::Sleep => "sleep",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocaleCall {
    Info,
    Format,
}

impl fmt::Display for LocaleCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Info => "info",
            Self::Format => "format",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UapiGuard {
    policy: SessionPolicy,
}

impl UapiGuard {
    pub fn new(policy: SessionPolicy) -> Self {
        Self { policy }
    }

    pub fn policy(&self) -> &SessionPolicy {
        &self.policy
    }

    pub fn check(&self, call: &UapiCall) -> Result<Capability> {
        let required = call.required_capability()?;
        self.policy.check(&required)?;
        Ok(required)
    }
}

impl Default for UapiGuard {
    fn default() -> Self {
        Self::new(SessionPolicy::default())
    }
}

#[derive(Debug, Error)]
pub enum UapiError {
    #[error(transparent)]
    Capability(#[from] ManifestError),
    #[error(transparent)]
    Policy(#[from] PolicyError),
}

pub type Result<T> = std::result::Result<T, UapiError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_grants_allow_low_risk_calls() {
        let guard = UapiGuard::default();
        let cap = guard
            .check(&UapiCall::Io(IoCall::Stdout))
            .expect("stdout should be default-granted");

        assert_eq!(cap.to_string(), "io.stdout");
    }

    #[test]
    fn fs_calls_are_denied_without_matching_grant() {
        let guard = UapiGuard::default();
        let err = guard
            .check(&UapiCall::Fs(FsCall::Read {
                path: "./notes/today.txt".to_string(),
            }))
            .expect_err("fs read should require a grant");

        assert!(matches!(err, UapiError::Policy(PolicyError::Denied { .. })));
    }

    #[test]
    fn fs_calls_pass_with_matching_grant() {
        let policy =
            SessionPolicy::from_cli_grants(&["fs.read:./notes/**".to_string()]).expect("policy");
        let guard = UapiGuard::new(policy);
        let cap = guard
            .check(&UapiCall::Fs(FsCall::Read {
                path: "./notes/today.txt".to_string(),
            }))
            .expect("fs read should pass");

        assert_eq!(cap.to_string(), "fs.read:./notes/today.txt");
    }

    #[test]
    fn net_connect_uses_host_and_port_scope() {
        let policy =
            SessionPolicy::from_cli_grants(&["net.connect:api.example.com:443".to_string()])
                .expect("policy");
        let guard = UapiGuard::new(policy);

        assert!(guard
            .check(&UapiCall::Net(NetCall::Connect {
                host: "api.example.com".to_string(),
                port: 443,
            }))
            .is_ok());
        assert!(guard
            .check(&UapiCall::Net(NetCall::Connect {
                host: "api.example.com".to_string(),
                port: 80,
            }))
            .is_err());
    }
}

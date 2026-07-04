//! Bridge between generated Phase 2 WIT types and runtime dispatch types.
//!
//! The generated bindings are the public component boundary. The dispatcher
//! types are the runtime boundary. Keeping the conversion code here makes the
//! later Wasmtime import wiring small and easier to audit.

use crate::{
    phase2_bindings::krate::{fs, io, locale, net},
    uapi_dispatch as dispatch,
};

pub fn open_mode_from_wit(mode: fs::types::OpenMode) -> dispatch::OpenMode {
    match mode {
        fs::types::OpenMode::Read => dispatch::OpenMode::Read,
        fs::types::OpenMode::Write => dispatch::OpenMode::Write,
        fs::types::OpenMode::ReadWrite => dispatch::OpenMode::ReadWrite,
        fs::types::OpenMode::Append => dispatch::OpenMode::Append,
    }
}

pub fn file_stat_to_wit(stat: dispatch::FileStat) -> fs::types::FileStat {
    fs::types::FileStat {
        size: stat.size,
        modified_millis: stat.modified_millis,
        is_dir: stat.is_dir,
    }
}

pub fn fs_error_to_wit(err: dispatch::FsDispatchError) -> fs::types::FsError {
    match err {
        dispatch::FsDispatchError::PermissionDenied => fs::types::FsError::PermissionDenied,
        dispatch::FsDispatchError::Policy(message) => fs::types::FsError::Io(message),
        dispatch::FsDispatchError::Adapter(err) => fs_adapter_error_to_wit(err),
    }
}

pub fn http_method_from_wit(method: net::types::HttpMethod) -> dispatch::HttpMethod {
    match method {
        net::types::HttpMethod::Get => dispatch::HttpMethod::Get,
        net::types::HttpMethod::Post => dispatch::HttpMethod::Post,
        net::types::HttpMethod::Put => dispatch::HttpMethod::Put,
        net::types::HttpMethod::Delete => dispatch::HttpMethod::Delete,
        net::types::HttpMethod::Patch => dispatch::HttpMethod::Patch,
        net::types::HttpMethod::Head => dispatch::HttpMethod::Head,
        net::types::HttpMethod::Options => dispatch::HttpMethod::Options,
    }
}

pub fn header_from_wit(header: net::types::Header) -> dispatch::Header {
    dispatch::Header {
        name: header.name,
        value: header.value,
    }
}

pub fn header_to_wit(header: dispatch::Header) -> net::types::Header {
    net::types::Header {
        name: header.name,
        value: header.value,
    }
}

pub fn request_from_wit(req: net::types::Request) -> dispatch::HttpRequest {
    dispatch::HttpRequest {
        method: http_method_from_wit(req.method),
        url: req.url,
        headers: req.headers.into_iter().map(header_from_wit).collect(),
        body: req.body,
        timeout_millis: req.timeout_millis,
    }
}

pub fn response_to_wit(response: dispatch::HttpResponse) -> net::types::Response {
    net::types::Response {
        status: response.status,
        headers: response.headers.into_iter().map(header_to_wit).collect(),
        body: response.body,
    }
}

pub fn net_error_to_wit(err: dispatch::NetDispatchError) -> net::types::NetError {
    match err {
        dispatch::NetDispatchError::InvalidUrl => net::types::NetError::InvalidUrl,
        dispatch::NetDispatchError::PermissionDenied => net::types::NetError::PermissionDenied,
        dispatch::NetDispatchError::Policy(message) => net::types::NetError::Other(message),
        dispatch::NetDispatchError::Adapter(err) => net_adapter_error_to_wit(err),
    }
}

pub fn log_level_to_str(level: io::types::LogLevel) -> &'static str {
    match level {
        io::types::LogLevel::Trace => "trace",
        io::types::LogLevel::Debug => "debug",
        io::types::LogLevel::Info => "info",
        io::types::LogLevel::Warn => "warn",
        io::types::LogLevel::Error => "error",
    }
}

pub fn io_error_to_wit(err: dispatch::DispatchError) -> io::types::IoError {
    match err {
        dispatch::DispatchError::PermissionDenied => {
            io::types::IoError::Other("permission denied".to_string())
        }
        dispatch::DispatchError::Policy(message) => io::types::IoError::Other(message),
        dispatch::DispatchError::Adapter(err) => io_adapter_error_to_wit(err),
    }
}

pub fn locale_from_wit(loc: locale::types::LocaleId) -> dispatch::LocaleId {
    dispatch::LocaleId { bcp47: loc.bcp47 }
}

pub fn locale_to_wit(loc: dispatch::LocaleId) -> locale::types::LocaleId {
    locale::types::LocaleId { bcp47: loc.bcp47 }
}

pub fn date_style_from_wit(style: locale::types::DateStyle) -> dispatch::DateStyle {
    match style {
        locale::types::DateStyle::Short => dispatch::DateStyle::Short,
        locale::types::DateStyle::Medium => dispatch::DateStyle::Medium,
        locale::types::DateStyle::Long => dispatch::DateStyle::Long,
        locale::types::DateStyle::Full => dispatch::DateStyle::Full,
    }
}

pub fn number_style_from_wit(style: locale::types::NumberStyle) -> dispatch::NumberStyle {
    match style {
        locale::types::NumberStyle::Decimal => dispatch::NumberStyle::Decimal,
        locale::types::NumberStyle::Percent => dispatch::NumberStyle::Percent,
        locale::types::NumberStyle::Currency => dispatch::NumberStyle::Currency,
    }
}

pub fn dispatch_error_to_trap(err: dispatch::DispatchError) -> wasmtime::Error {
    wasmtime::Error::msg(err.to_string())
}

fn fs_adapter_error_to_wit(err: dispatch::AdapterError) -> fs::types::FsError {
    match err {
        dispatch::AdapterError::InvalidPath => fs::types::FsError::InvalidPath,
        dispatch::AdapterError::NotFound => fs::types::FsError::NotFound,
        dispatch::AdapterError::PermissionDenied => fs::types::FsError::PermissionDenied,
        dispatch::AdapterError::Io(message)
        | dispatch::AdapterError::Network(message)
        | dispatch::AdapterError::Protocol(message) => fs::types::FsError::Io(message),
        dispatch::AdapterError::Timeout => fs::types::FsError::Io("timed out".to_string()),
        dispatch::AdapterError::BodyTooLarge => {
            fs::types::FsError::Io("HTTP response body is too large".to_string())
        }
        dispatch::AdapterError::Unsupported => fs::types::FsError::Io(
            "operation is not supported by this host adapter yet".to_string(),
        ),
    }
}

fn net_adapter_error_to_wit(err: dispatch::AdapterError) -> net::types::NetError {
    match err {
        dispatch::AdapterError::InvalidPath => net::types::NetError::InvalidUrl,
        dispatch::AdapterError::NotFound => {
            net::types::NetError::DnsFailure("not found".to_string())
        }
        dispatch::AdapterError::PermissionDenied => net::types::NetError::PermissionDenied,
        dispatch::AdapterError::Network(message) => net::types::NetError::ConnectFailure(message),
        dispatch::AdapterError::Timeout => net::types::NetError::Timeout,
        dispatch::AdapterError::Protocol(message) => net::types::NetError::Protocol(message),
        dispatch::AdapterError::Io(message) => net::types::NetError::Other(message),
        dispatch::AdapterError::BodyTooLarge => net::types::NetError::BodyTooLarge,
        dispatch::AdapterError::Unsupported => net::types::NetError::Other(
            "operation is not supported by this host adapter yet".to_string(),
        ),
    }
}

fn io_adapter_error_to_wit(err: dispatch::AdapterError) -> io::types::IoError {
    match err {
        dispatch::AdapterError::NotFound => io::types::IoError::Closed,
        dispatch::AdapterError::PermissionDenied => {
            io::types::IoError::Other("permission denied".to_string())
        }
        dispatch::AdapterError::InvalidPath
        | dispatch::AdapterError::Unsupported
        | dispatch::AdapterError::Network(_)
        | dispatch::AdapterError::Timeout
        | dispatch::AdapterError::Protocol(_)
        | dispatch::AdapterError::BodyTooLarge => io::types::IoError::Other(err.to_string()),
        dispatch::AdapterError::Io(message) => io::types::IoError::Other(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_filesystem_shapes() {
        assert_eq!(
            open_mode_from_wit(fs::types::OpenMode::ReadWrite),
            dispatch::OpenMode::ReadWrite
        );

        let stat = file_stat_to_wit(dispatch::FileStat {
            size: 42,
            modified_millis: 1000,
            is_dir: false,
        });

        assert_eq!(stat.size, 42);
        assert_eq!(stat.modified_millis, 1000);
        assert!(!stat.is_dir);
        assert!(matches!(
            fs_error_to_wit(dispatch::FsDispatchError::PermissionDenied),
            fs::types::FsError::PermissionDenied
        ));
        assert!(matches!(
            fs_error_to_wit(dispatch::FsDispatchError::Adapter(
                dispatch::AdapterError::InvalidPath
            )),
            fs::types::FsError::InvalidPath
        ));
    }

    #[test]
    fn maps_network_shapes() {
        let req = request_from_wit(net::types::Request {
            method: net::types::HttpMethod::Post,
            url: "https://example.com/data".to_string(),
            headers: vec![net::types::Header {
                name: "accept".to_string(),
                value: "application/json".to_string(),
            }],
            body: b"{}".to_vec(),
            timeout_millis: Some(5000),
        });

        assert_eq!(req.method, dispatch::HttpMethod::Post);
        assert_eq!(req.headers[0].name, "accept");
        assert_eq!(req.body, b"{}".to_vec());
        assert_eq!(req.timeout_millis, Some(5000));

        let response = response_to_wit(dispatch::HttpResponse {
            status: 200,
            headers: vec![dispatch::Header {
                name: "content-type".to_string(),
                value: "text/plain".to_string(),
            }],
            body: b"ok".to_vec(),
        });

        assert_eq!(response.status, 200);
        assert_eq!(response.headers[0].value, "text/plain");
        assert_eq!(response.body, b"ok".to_vec());
        assert!(matches!(
            net_error_to_wit(dispatch::NetDispatchError::InvalidUrl),
            net::types::NetError::InvalidUrl
        ));
        assert!(matches!(
            net_error_to_wit(dispatch::NetDispatchError::PermissionDenied),
            net::types::NetError::PermissionDenied
        ));
        assert!(matches!(
            net_error_to_wit(dispatch::NetDispatchError::Adapter(
                dispatch::AdapterError::BodyTooLarge
            )),
            net::types::NetError::BodyTooLarge
        ));
        assert!(matches!(
            net_error_to_wit(dispatch::NetDispatchError::Adapter(
                dispatch::AdapterError::Timeout
            )),
            net::types::NetError::Timeout
        ));
        assert!(matches!(
            net_error_to_wit(dispatch::NetDispatchError::Adapter(
                dispatch::AdapterError::Protocol("bad status".to_string())
            )),
            net::types::NetError::Protocol(message) if message == "bad status"
        ));
        assert!(matches!(
            net_error_to_wit(dispatch::NetDispatchError::Adapter(
                dispatch::AdapterError::NotFound
            )),
            net::types::NetError::DnsFailure(message) if message == "not found"
        ));
        assert!(matches!(
            net_error_to_wit(dispatch::NetDispatchError::Adapter(
                dispatch::AdapterError::Network("connection refused".to_string())
            )),
            net::types::NetError::ConnectFailure(message) if message == "connection refused"
        ));
    }

    #[test]
    fn maps_locale_and_log_shapes() {
        assert_eq!(log_level_to_str(io::types::LogLevel::Warn), "warn");
        assert!(matches!(
            io_error_to_wit(dispatch::DispatchError::Adapter(
                dispatch::AdapterError::NotFound
            )),
            io::types::IoError::Closed
        ));
        assert_eq!(
            date_style_from_wit(locale::types::DateStyle::Full),
            dispatch::DateStyle::Full
        );
        assert_eq!(
            number_style_from_wit(locale::types::NumberStyle::Percent),
            dispatch::NumberStyle::Percent
        );

        let loc = locale_from_wit(locale::types::LocaleId {
            bcp47: "en-US".to_string(),
        });
        assert_eq!(loc.bcp47, "en-US");
        assert_eq!(locale_to_wit(loc).bcp47, "en-US");
    }

    #[test]
    fn maps_trappable_dispatch_errors() {
        let err = dispatch_error_to_trap(dispatch::DispatchError::PermissionDenied);
        assert!(err.to_string().contains("permission denied"));
    }
}

//! Shared network helpers for host adapters.

use std::{collections::HashSet, io::Read, net::SocketAddr};

/// A parsed plain HTTP URL for the current Phase 2 adapter slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainHttpUrl {
    pub host: String,
    pub port: u16,
    pub path_and_query: String,
}

impl PlainHttpUrl {
    /// Parse an `http://` URL into host, port, and request target.
    ///
    /// HTTPS, auth info, and fragments are intentionally outside this early
    /// helper. HTTPS lands after we choose the shared TLS stack.
    pub fn parse(input: &str) -> Result<Self, PlainHttpError> {
        if contains_http_unsafe_ascii(input) {
            return Err(PlainHttpError::InvalidUrl);
        }

        let Some(rest) = strip_ascii_case_prefix(input, "http://") else {
            return Err(PlainHttpError::UnsupportedScheme);
        };
        let rest = rest.split_once('#').map_or(rest, |(before, _)| before);
        let endpoint =
            parse_url_endpoint_with_default(rest, 80).map_err(|_| PlainHttpError::InvalidUrl)?;
        let (authority, path) = match rest.find(['/', '?']) {
            Some(index) => rest.split_at(index),
            None => (rest, "/"),
        };
        if authority.is_empty() {
            return Err(PlainHttpError::InvalidUrl);
        }

        let path_and_query = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        if path_and_query.len() > MAX_HTTP_TARGET_BYTES {
            return Err(PlainHttpError::InvalidUrl);
        }

        Ok(Self {
            host: endpoint.host,
            port: endpoint.port,
            path_and_query,
        })
    }
}

const MAX_HTTP_HEADERS: usize = 64;
const MAX_HTTP_HEADER_NAME_BYTES: usize = 128;
const MAX_HTTP_HEADER_VALUE_BYTES: usize = 4 * 1024;
const MAX_HTTP_HEADER_BLOCK_BYTES: usize = 16 * 1024;
const MAX_HTTP_AUTHORITY_BYTES: usize = 255;
const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;
const MAX_HTTP_TARGET_BYTES: usize = 4096;
const MAX_HTTP_REQUEST_BYTES: usize = MAX_HTTP_BODY_BYTES + MAX_HTTP_HEADER_BLOCK_BYTES + 4096;
const MAX_RESOLVED_SOCKET_ADDRS: usize = 32;

/// A parsed network endpoint used for capability checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlEndpoint {
    pub host: String,
    pub port: u16,
}

/// Parse a URL into a network endpoint for policy checks.
///
/// This helper currently supports `http://` and `https://` endpoint extraction,
/// with early rejection for auth-info, unsafe ASCII, and unsupported authority
/// shapes used by the Phase 2 plain-network path.
pub fn parse_url_endpoint(input: &str) -> Result<UrlEndpoint, UrlEndpointError> {
    if contains_http_unsafe_ascii(input) {
        return Err(UrlEndpointError::InvalidUrl);
    }

    let (scheme, rest) = input
        .split_once("://")
        .ok_or(UrlEndpointError::InvalidUrl)?;
    let default_port = if scheme.eq_ignore_ascii_case("http") {
        80
    } else if scheme.eq_ignore_ascii_case("https") {
        443
    } else {
        return Err(UrlEndpointError::UnsupportedScheme);
    };

    parse_url_endpoint_with_default(rest, default_port)
}

/// Normalize resolved socket addresses for deterministic, bounded connect loops.
///
/// This helper keeps first-seen order, removes duplicates, and caps the result
/// size for this Phase 2 plain-network slice.
pub fn normalize_resolved_socket_addrs(addrs: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();

    for addr in addrs {
        if seen.insert(addr) {
            normalized.push(addr);
            if normalized.len() >= MAX_RESOLVED_SOCKET_ADDRS {
                break;
            }
        }
    }

    normalized
}

fn parse_url_endpoint_with_default(
    rest: &str,
    default_port: u16,
) -> Result<UrlEndpoint, UrlEndpointError> {
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.len() > MAX_HTTP_AUTHORITY_BYTES {
        return Err(UrlEndpointError::InvalidUrl);
    }
    if authority.is_empty() || authority.contains('@') {
        return Err(UrlEndpointError::InvalidUrl);
    }
    if authority.starts_with('[') || authority.contains("]:") {
        return Err(UrlEndpointError::InvalidUrl);
    }
    if authority.matches(':').count() > 1 {
        return Err(UrlEndpointError::InvalidUrl);
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => {
            let port: u16 = port.parse().map_err(|_| UrlEndpointError::InvalidUrl)?;
            if port == 0 {
                return Err(UrlEndpointError::InvalidUrl);
            }
            (host, port)
        }
        _ => (authority, default_port),
    };

    if host.is_empty() {
        return Err(UrlEndpointError::InvalidUrl);
    }
    if !is_valid_plain_http_host(host) {
        return Err(UrlEndpointError::InvalidUrl);
    }

    Ok(UrlEndpoint {
        host: host.to_ascii_lowercase(),
        port,
    })
}

fn strip_ascii_case_prefix<'a>(input: &'a str, prefix: &str) -> Option<&'a str> {
    let (candidate, rest) = input.split_at_checked(prefix.len())?;
    if candidate.eq_ignore_ascii_case(prefix) {
        Some(rest)
    } else {
        None
    }
}

/// HTTP methods supported by the Phase 2 request-framing helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlainHttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl PlainHttpMethod {
    fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
            Self::Patch => "PATCH",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
        }
    }
}

/// An app-provided HTTP header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainHttpHeader {
    pub name: String,
    pub value: String,
}

/// A plain HTTP request ready for shared host-adapter framing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainHttpRequest {
    pub method: PlainHttpMethod,
    pub headers: Vec<PlainHttpHeader>,
    pub body: Vec<u8>,
}

/// A parsed plain HTTP response from the current Phase 2 adapter slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainHttpResponse {
    pub status: u16,
    pub headers: Vec<PlainHttpHeader>,
    pub body: Vec<u8>,
}

/// Build an HTTP/1.1 request for the current plain HTTP adapter.
///
/// The host always owns `Host`, `Connection`, and `Content-Length` because
/// those headers describe the transport framing, not app intent.
pub fn build_plain_http_request(
    req: &PlainHttpRequest,
    url: &PlainHttpUrl,
) -> Result<Vec<u8>, PlainHttpError> {
    if req.headers.len() > MAX_HTTP_HEADERS {
        return Err(PlainHttpError::InvalidHeader);
    }
    if req.body.len() > MAX_HTTP_BODY_BYTES {
        return Err(PlainHttpError::BodyTooLarge);
    }

    let method = req.method.as_str();
    let mut request = format!(
        "{method} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
        url.path_and_query, url.host
    )
    .into_bytes();

    for header in &req.headers {
        if header.name.len() > MAX_HTTP_HEADER_NAME_BYTES
            || header.value.len() > MAX_HTTP_HEADER_VALUE_BYTES
        {
            return Err(PlainHttpError::InvalidHeader);
        }
        if !is_valid_plain_http_header_name(&header.name)
            || !is_safe_plain_http_header_value(&header.value)
        {
            return Err(PlainHttpError::InvalidHeader);
        }
        if is_host_controlled_http_header(&header.name) {
            return Err(PlainHttpError::HostControlledHeader);
        }
        request.extend_from_slice(header.name.as_bytes());
        request.extend_from_slice(b": ");
        request.extend_from_slice(header.value.as_bytes());
        request.extend_from_slice(b"\r\n");
    }
    if !req.body.is_empty() {
        request.extend_from_slice(format!("Content-Length: {}\r\n", req.body.len()).as_bytes());
    }
    request.extend_from_slice(b"\r\n");
    request.extend_from_slice(&req.body);
    if request.len() > MAX_HTTP_REQUEST_BYTES {
        return Err(PlainHttpError::BodyTooLarge);
    }

    Ok(request)
}

/// Parse a plain HTTP response for the current Phase 2 adapter slice.
pub fn parse_plain_http_response(bytes: &[u8]) -> Result<PlainHttpResponse, PlainHttpError> {
    let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
        return Err(PlainHttpError::InvalidResponse);
    };
    if header_end > MAX_HTTP_HEADER_BLOCK_BYTES {
        return Err(PlainHttpError::InvalidResponse);
    }

    let header_bytes = &bytes[..header_end];
    let body = bytes[header_end + 4..].to_vec();
    let headers_text =
        std::str::from_utf8(header_bytes).map_err(|_| PlainHttpError::InvalidResponse)?;
    let mut lines = headers_text.split("\r\n");
    let status_line = lines.next().ok_or(PlainHttpError::InvalidResponse)?;

    let mut status_parts = status_line.split_whitespace();
    let version = status_parts.next().ok_or(PlainHttpError::InvalidResponse)?;
    let code = status_parts.next().ok_or(PlainHttpError::InvalidResponse)?;
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        return Err(PlainHttpError::InvalidResponse);
    }
    if status_parts.next().is_none() {
        return Err(PlainHttpError::InvalidResponse);
    }
    let status = code
        .parse::<u16>()
        .map_err(|_| PlainHttpError::InvalidResponse)?;
    if !(100..=599).contains(&status) {
        return Err(PlainHttpError::InvalidResponse);
    }

    let mut headers = Vec::new();
    let mut content_length: Option<usize> = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return Err(PlainHttpError::InvalidResponse);
        };
        let name = name.trim();
        let value = value.trim();
        if name.len() > MAX_HTTP_HEADER_NAME_BYTES
            || value.len() > MAX_HTTP_HEADER_VALUE_BYTES
            || !is_valid_plain_http_header_name(name)
            || !is_safe_plain_http_header_value(value)
        {
            return Err(PlainHttpError::InvalidResponse);
        }
        if name.eq_ignore_ascii_case("transfer-encoding") {
            // The early Phase 2 plain adapter reads a full buffered response and
            // does not implement chunked decoding yet.
            return Err(PlainHttpError::InvalidResponse);
        }
        if name.eq_ignore_ascii_case("content-length") {
            let parsed = value
                .parse::<usize>()
                .map_err(|_| PlainHttpError::InvalidResponse)?;
            if let Some(existing) = content_length {
                if existing != parsed {
                    return Err(PlainHttpError::InvalidResponse);
                }
            } else {
                content_length = Some(parsed);
            }
        }
        headers.push(PlainHttpHeader {
            name: name.to_string(),
            value: value.to_string(),
        });
        if headers.len() > MAX_HTTP_HEADERS {
            return Err(PlainHttpError::InvalidResponse);
        }
    }
    if let Some(content_length) = content_length {
        if body.len() > content_length {
            return Err(PlainHttpError::InvalidResponse);
        }
        if body.len() < content_length {
            return Err(PlainHttpError::InvalidResponse);
        }
    }

    Ok(PlainHttpResponse {
        status,
        headers,
        body,
    })
}

/// Read a full plain HTTP response with a strict byte cap.
pub fn read_plain_http_response_limited(
    reader: &mut impl Read,
    max_bytes: usize,
) -> Result<Vec<u8>, PlainHttpReadError> {
    let mut response = Vec::new();
    let mut chunk = [0; 8192];

    loop {
        let read = match reader.read(&mut chunk) {
            Ok(read) => read,
            Err(err) => {
                return Err(match err.kind() {
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
                        PlainHttpReadError::Timeout
                    }
                    _ => PlainHttpReadError::Io(err),
                });
            }
        };

        if read == 0 {
            return Ok(response);
        }

        if response.len() + read > max_bytes {
            return Err(PlainHttpReadError::BodyTooLarge);
        }

        response.extend_from_slice(&chunk[..read]);
    }
}

fn is_valid_plain_http_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            matches!(
                byte,
                b'!' | b'#'
                    | b'$'
                    | b'%'
                    | b'&'
                    | b'\''
                    | b'*'
                    | b'+'
                    | b'-'
                    | b'.'
                    | b'^'
                    | b'_'
                    | b'`'
                    | b'|'
                    | b'~'
                    | b'0'..=b'9'
                    | b'a'..=b'z'
                    | b'A'..=b'Z'
            )
        })
}

fn is_host_controlled_http_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("host")
        || name.eq_ignore_ascii_case("connection")
        || name.eq_ignore_ascii_case("content-length")
        || name.eq_ignore_ascii_case("transfer-encoding")
}

fn is_valid_plain_http_host(host: &str) -> bool {
    if host.is_empty() || host.starts_with('.') || host.ends_with('.') || host.contains("..") {
        return false;
    }

    let is_numeric = host.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'.'));
    if is_numeric {
        return is_valid_ipv4_host(host);
    }

    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 {
            return false;
        }
        if label.starts_with('-') || label.ends_with('-') {
            return false;
        }
        if !label
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'-'))
        {
            return false;
        }
    }

    true
}

fn is_valid_ipv4_host(host: &str) -> bool {
    let mut parts = host.split('.');
    for _ in 0..4 {
        let Some(part) = parts.next() else {
            return false;
        };
        if part.is_empty() {
            return false;
        }
        if part.parse::<u8>().is_err() {
            return false;
        }
    }
    parts.next().is_none()
}

fn contains_http_unsafe_ascii(input: &str) -> bool {
    input
        .bytes()
        .any(|byte| !byte.is_ascii() || byte.is_ascii_control() || byte.is_ascii_whitespace())
}

fn is_safe_plain_http_header_value(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii() && !byte.is_ascii_control())
}

/// Errors returned by shared plain HTTP helpers.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlainHttpError {
    #[error("only http:// URLs are supported by the Phase 2 plain HTTP adapter")]
    UnsupportedScheme,
    #[error("invalid plain HTTP URL")]
    InvalidUrl,
    #[error("invalid HTTP header")]
    InvalidHeader,
    #[error("HTTP body exceeded byte limit")]
    BodyTooLarge,
    #[error("invalid HTTP response")]
    InvalidResponse,
    #[error("host-controlled HTTP header")]
    HostControlledHeader,
}

/// Errors returned by URL endpoint parsing for policy checks.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UrlEndpointError {
    #[error("unsupported URL scheme")]
    UnsupportedScheme,
    #[error("invalid URL")]
    InvalidUrl,
}

/// Errors returned while reading a full plain HTTP response body.
#[derive(Debug, thiserror::Error)]
pub enum PlainHttpReadError {
    #[error("HTTP read timed out")]
    Timeout,
    #[error("HTTP response exceeded byte limit")]
    BodyTooLarge,
    #[error("HTTP read failed: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_parser_normalizes_query_only_paths() {
        let parsed = PlainHttpUrl::parse("http://127.0.0.1:8080?name=layer36#local")
            .expect("parse HTTP URL");

        assert_eq!(parsed.host, "127.0.0.1");
        assert_eq!(parsed.port, 8080);
        assert_eq!(parsed.path_and_query, "/?name=layer36");
    }

    #[test]
    fn url_parser_normalizes_mixed_case_host() {
        let parsed = PlainHttpUrl::parse("http://ExAmPle.Com:8080/path").expect("parse HTTP URL");

        assert_eq!(parsed.host, "example.com");
        assert_eq!(parsed.port, 8080);
        assert_eq!(parsed.path_and_query, "/path");
    }

    #[test]
    fn url_parser_accepts_mixed_case_http_scheme() {
        let parsed = PlainHttpUrl::parse("HTTP://example.com/path").expect("parse HTTP URL");

        assert_eq!(parsed.host, "example.com");
        assert_eq!(parsed.port, 80);
        assert_eq!(parsed.path_and_query, "/path");
    }

    #[test]
    fn url_parser_rejects_request_line_injection_characters() {
        assert_eq!(
            PlainHttpUrl::parse("http://127.0.0.1:8080/path\r\nX-Bad: yes").unwrap_err(),
            PlainHttpError::InvalidUrl
        );
        assert_eq!(
            PlainHttpUrl::parse("http://127.0.0.1:8080/path with spaces").unwrap_err(),
            PlainHttpError::InvalidUrl
        );
        assert_eq!(
            PlainHttpUrl::parse("http://127.0.0.1:8080/caf\u{e9}").unwrap_err(),
            PlainHttpError::InvalidUrl
        );
    }

    #[test]
    fn url_parser_rejects_oversized_request_target() {
        let oversized = format!(
            "http://127.0.0.1:8080/{}",
            "a".repeat(MAX_HTTP_TARGET_BYTES)
        );

        assert_eq!(
            PlainHttpUrl::parse(&oversized).unwrap_err(),
            PlainHttpError::InvalidUrl
        );
    }

    #[test]
    fn url_parser_rejects_empty_or_zero_ports() {
        assert_eq!(
            PlainHttpUrl::parse("http://127.0.0.1:/").unwrap_err(),
            PlainHttpError::InvalidUrl
        );
        assert_eq!(
            PlainHttpUrl::parse("http://127.0.0.1:0/").unwrap_err(),
            PlainHttpError::InvalidUrl
        );
    }

    #[test]
    fn url_parser_rejects_unsupported_authority_forms() {
        assert_eq!(
            PlainHttpUrl::parse("http://127.0.0.1:8080:99/").unwrap_err(),
            PlainHttpError::InvalidUrl
        );
        assert_eq!(
            PlainHttpUrl::parse("http://[::1]:8080/").unwrap_err(),
            PlainHttpError::InvalidUrl
        );
    }

    #[test]
    fn request_builder_forwards_method_headers_and_body() {
        let url = PlainHttpUrl::parse("http://127.0.0.1:8080/submit?name=layer36")
            .expect("parse HTTP URL");
        let req = PlainHttpRequest {
            method: PlainHttpMethod::Post,
            headers: vec![PlainHttpHeader {
                name: "X-Layer36".to_string(),
                value: "yes".to_string(),
            }],
            body: b"payload".to_vec(),
        };

        let request = build_plain_http_request(&req, &url).expect("build HTTP request");
        let request = String::from_utf8(request).expect("request is UTF-8");

        assert!(request.starts_with("POST /submit?name=layer36 HTTP/1.1\r\n"));
        assert!(request.contains("Host: 127.0.0.1\r\n"));
        assert!(request.contains("Connection: close\r\n"));
        assert!(request.contains("X-Layer36: yes\r\n"));
        assert!(request.contains("Content-Length: 7\r\n"));
        assert!(request.ends_with("\r\n\r\npayload"));
    }

    #[test]
    fn request_builder_rejects_host_controlled_headers() {
        let url = PlainHttpUrl::parse("http://127.0.0.1:8080/").expect("parse HTTP URL");
        let req = PlainHttpRequest {
            method: PlainHttpMethod::Get,
            headers: vec![PlainHttpHeader {
                name: "Content-Length".to_string(),
                value: "999".to_string(),
            }],
            body: Vec::new(),
        };

        let err = build_plain_http_request(&req, &url)
            .expect_err("host-controlled headers should be rejected");

        assert_eq!(err, PlainHttpError::HostControlledHeader);
    }

    #[test]
    fn request_builder_rejects_control_characters_in_header_values() {
        let url = PlainHttpUrl::parse("http://127.0.0.1:8080/").expect("parse HTTP URL");
        let req = PlainHttpRequest {
            method: PlainHttpMethod::Get,
            headers: vec![PlainHttpHeader {
                name: "X-Layer36".to_string(),
                value: "safe\tno".to_string(),
            }],
            body: Vec::new(),
        };

        let err = build_plain_http_request(&req, &url)
            .expect_err("control characters should be rejected in header values");

        assert_eq!(err, PlainHttpError::InvalidHeader);
    }

    #[test]
    fn request_builder_rejects_header_limits() {
        let url = PlainHttpUrl::parse("http://127.0.0.1:8080/").expect("parse HTTP URL");
        let too_many = PlainHttpRequest {
            method: PlainHttpMethod::Get,
            headers: (0..(MAX_HTTP_HEADERS + 1))
                .map(|index| PlainHttpHeader {
                    name: format!("X-{index}"),
                    value: "ok".to_string(),
                })
                .collect(),
            body: Vec::new(),
        };
        assert_eq!(
            build_plain_http_request(&too_many, &url).unwrap_err(),
            PlainHttpError::InvalidHeader
        );

        let long_name = PlainHttpRequest {
            method: PlainHttpMethod::Get,
            headers: vec![PlainHttpHeader {
                name: "X".repeat(MAX_HTTP_HEADER_NAME_BYTES + 1),
                value: "ok".to_string(),
            }],
            body: Vec::new(),
        };
        assert_eq!(
            build_plain_http_request(&long_name, &url).unwrap_err(),
            PlainHttpError::InvalidHeader
        );

        let long_value = PlainHttpRequest {
            method: PlainHttpMethod::Get,
            headers: vec![PlainHttpHeader {
                name: "X-Layer36".to_string(),
                value: "v".repeat(MAX_HTTP_HEADER_VALUE_BYTES + 1),
            }],
            body: Vec::new(),
        };
        assert_eq!(
            build_plain_http_request(&long_value, &url).unwrap_err(),
            PlainHttpError::InvalidHeader
        );
    }

    #[test]
    fn request_builder_rejects_body_limit() {
        let url = PlainHttpUrl::parse("http://127.0.0.1:8080/").expect("parse HTTP URL");
        let req = PlainHttpRequest {
            method: PlainHttpMethod::Post,
            headers: Vec::new(),
            body: vec![b'x'; MAX_HTTP_BODY_BYTES + 1],
        };

        assert_eq!(
            build_plain_http_request(&req, &url).unwrap_err(),
            PlainHttpError::BodyTooLarge
        );
    }

    #[test]
    fn request_builder_rejects_oversized_total_request_frame() {
        let url = PlainHttpUrl::parse("http://127.0.0.1:8080/").expect("parse HTTP URL");
        let headers: Vec<PlainHttpHeader> = (0..16)
            .map(|index| PlainHttpHeader {
                name: format!("X-Big-{index}"),
                value: "v".repeat(MAX_HTTP_HEADER_VALUE_BYTES),
            })
            .collect();
        let req = PlainHttpRequest {
            method: PlainHttpMethod::Post,
            headers,
            body: vec![b'x'; MAX_HTTP_BODY_BYTES],
        };

        assert_eq!(
            build_plain_http_request(&req, &url).unwrap_err(),
            PlainHttpError::BodyTooLarge
        );
    }

    #[test]
    fn endpoint_parser_supports_http_and_https_default_ports() {
        let http = parse_url_endpoint("http://example.com/path").expect("HTTP endpoint");
        let https = parse_url_endpoint("https://example.com/path").expect("HTTPS endpoint");
        let mixed = parse_url_endpoint("https://ExAmPle.Com/path").expect("mixed-case endpoint");
        let mixed_scheme =
            parse_url_endpoint("HTTPS://example.com/path").expect("mixed-case scheme endpoint");

        assert_eq!(http.host, "example.com");
        assert_eq!(http.port, 80);
        assert_eq!(https.host, "example.com");
        assert_eq!(https.port, 443);
        assert_eq!(mixed.host, "example.com");
        assert_eq!(mixed.port, 443);
        assert_eq!(mixed_scheme.host, "example.com");
        assert_eq!(mixed_scheme.port, 443);
    }

    #[test]
    fn endpoint_parser_rejects_invalid_or_unsupported_urls() {
        assert_eq!(
            parse_url_endpoint("ftp://example.com/file").unwrap_err(),
            UrlEndpointError::UnsupportedScheme
        );
        assert_eq!(
            parse_url_endpoint("https://example.com:0/path").unwrap_err(),
            UrlEndpointError::InvalidUrl
        );
        assert_eq!(
            parse_url_endpoint("https://[::1]/path").unwrap_err(),
            UrlEndpointError::InvalidUrl
        );
        assert_eq!(
            parse_url_endpoint("https://exa_mple.com/path").unwrap_err(),
            UrlEndpointError::InvalidUrl
        );
        assert_eq!(
            parse_url_endpoint("https://example..com/path").unwrap_err(),
            UrlEndpointError::InvalidUrl
        );
        assert_eq!(
            parse_url_endpoint("https://-example.com/path").unwrap_err(),
            UrlEndpointError::InvalidUrl
        );
        assert_eq!(
            parse_url_endpoint("https://example-.com/path").unwrap_err(),
            UrlEndpointError::InvalidUrl
        );
        assert_eq!(
            parse_url_endpoint("https://999.0.0.1/path").unwrap_err(),
            UrlEndpointError::InvalidUrl
        );
        assert_eq!(
            parse_url_endpoint("https://example.com/caf\u{e9}").unwrap_err(),
            UrlEndpointError::InvalidUrl
        );
    }

    #[test]
    fn normalize_resolved_socket_addrs_deduplicates_and_caps() {
        let mut input: Vec<SocketAddr> = (0..40)
            .map(|offset| SocketAddr::from(([127, 0, 0, 1], 12000 + offset)))
            .collect();
        input.push(SocketAddr::from(([127, 0, 0, 1], 12005)));
        input.push(SocketAddr::from(([127, 0, 0, 1], 12010)));

        let normalized = normalize_resolved_socket_addrs(input);

        assert_eq!(normalized.len(), MAX_RESOLVED_SOCKET_ADDRS);
        assert_eq!(
            normalized.first().copied(),
            Some(SocketAddr::from(([127, 0, 0, 1], 12000)))
        );
        assert_eq!(
            normalized.last().copied(),
            Some(SocketAddr::from(([127, 0, 0, 1], 12031)))
        );
    }

    #[test]
    fn normalize_resolved_socket_addrs_keeps_first_seen_order() {
        let first = SocketAddr::from(([127, 0, 0, 1], 18080));
        let second = SocketAddr::from(([127, 0, 0, 1], 18081));
        let third = SocketAddr::from(([127, 0, 0, 1], 18082));
        let input = vec![second, first, second, third, first];

        let normalized = normalize_resolved_socket_addrs(input);

        assert_eq!(normalized, vec![second, first, third]);
    }

    #[test]
    fn response_parser_splits_headers_and_body() {
        let response =
            parse_plain_http_response(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nhi")
                .expect("parse response");

        assert_eq!(response.status, 200);
        assert_eq!(
            response.headers,
            vec![PlainHttpHeader {
                name: "Content-Type".to_string(),
                value: "text/plain".to_string(),
            }]
        );
        assert_eq!(response.body, b"hi");
    }

    #[test]
    fn response_parser_rejects_malformed_responses() {
        assert_eq!(
            parse_plain_http_response(b"HTTP/2 200 OK\r\n\r\nbody").unwrap_err(),
            PlainHttpError::InvalidResponse
        );
        assert_eq!(
            parse_plain_http_response(b"HTTP/1.1 700 Weird\r\n\r\nbody").unwrap_err(),
            PlainHttpError::InvalidResponse
        );
        assert_eq!(
            parse_plain_http_response(b"HTTP/1.1 200\r\n\r\nbody").unwrap_err(),
            PlainHttpError::InvalidResponse
        );
        assert_eq!(
            parse_plain_http_response(b"HTTP/1.1 200 OK\r\nBad Header\r\n\r\nbody").unwrap_err(),
            PlainHttpError::InvalidResponse
        );
        assert_eq!(
            parse_plain_http_response(
                format!(
                    "HTTP/1.1 200 OK\r\nLong: {}\r\n\r\nbody",
                    "v".repeat(MAX_HTTP_HEADER_VALUE_BYTES + 1)
                )
                .as_bytes()
            )
            .unwrap_err(),
            PlainHttpError::InvalidResponse
        );
        assert_eq!(
            parse_plain_http_response(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nbody\r\n0\r\n\r\n"
            )
            .unwrap_err(),
            PlainHttpError::InvalidResponse
        );
        assert_eq!(
            parse_plain_http_response(b"HTTP/1.1 200 OK\r\nContent-Length: nope\r\n\r\nbody")
                .unwrap_err(),
            PlainHttpError::InvalidResponse
        );
        assert_eq!(
            parse_plain_http_response(
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 4\r\n\r\nbody"
            )
            .unwrap_err(),
            PlainHttpError::InvalidResponse
        );
        assert_eq!(
            parse_plain_http_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nbody")
                .unwrap_err(),
            PlainHttpError::InvalidResponse
        );
        assert_eq!(
            parse_plain_http_response(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nbody")
                .unwrap_err(),
            PlainHttpError::InvalidResponse
        );
        assert_eq!(
            parse_plain_http_response(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\n").unwrap_err(),
            PlainHttpError::InvalidResponse
        );
    }

    #[test]
    fn response_parser_rejects_oversized_header_block() {
        let mut response = String::from("HTTP/1.1 200 OK\r\n");
        while response.len() <= MAX_HTTP_HEADER_BLOCK_BYTES {
            response.push_str("X-Layer36: ");
            response.push_str(&"a".repeat(200));
            response.push_str("\r\n");
        }
        response.push_str("\r\nok");

        assert_eq!(
            parse_plain_http_response(response.as_bytes()).unwrap_err(),
            PlainHttpError::InvalidResponse
        );
    }

    #[test]
    fn response_reader_enforces_size_limit() {
        let mut response = std::io::Cursor::new(vec![b'x'; 5]);
        let err = read_plain_http_response_limited(&mut response, 4)
            .expect_err("oversized response should be rejected");
        assert!(matches!(err, PlainHttpReadError::BodyTooLarge));
    }

    #[test]
    fn response_reader_allows_exact_size_limit() {
        let mut response = std::io::Cursor::new(vec![b'x'; 4]);
        let bytes = read_plain_http_response_limited(&mut response, 4)
            .expect("exact limit should be accepted");
        assert_eq!(bytes.len(), 4);
    }

    #[test]
    fn response_reader_maps_timeouts() {
        struct TimeoutReader;

        impl Read for TimeoutReader {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "timeout",
                ))
            }
        }

        let err = read_plain_http_response_limited(&mut TimeoutReader, 10)
            .expect_err("would-block should map to timeout");
        assert!(matches!(err, PlainHttpReadError::Timeout));
    }
}

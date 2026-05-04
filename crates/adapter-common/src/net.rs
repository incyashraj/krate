//! Shared network helpers for host adapters.

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

        let Some(rest) = input.strip_prefix("http://") else {
            return Err(PlainHttpError::UnsupportedScheme);
        };
        let rest = rest.split_once('#').map_or(rest, |(before, _)| before);
        let (authority, path) = match rest.find(['/', '?']) {
            Some(index) => rest.split_at(index),
            None => (rest, "/"),
        };
        if authority.is_empty() || authority.contains('@') {
            return Err(PlainHttpError::InvalidUrl);
        }

        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !host.is_empty() => {
                let port = port.parse().map_err(|_| PlainHttpError::InvalidUrl)?;
                if port == 0 {
                    return Err(PlainHttpError::InvalidUrl);
                }
                (host, port)
            }
            _ => (authority, 80),
        };
        if host.is_empty() {
            return Err(PlainHttpError::InvalidUrl);
        }

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

/// Build an HTTP/1.1 request for the current plain HTTP adapter.
///
/// The host always owns `Host`, `Connection`, and `Content-Length` because
/// those headers describe the transport framing, not app intent.
pub fn build_plain_http_request(
    req: &PlainHttpRequest,
    url: &PlainHttpUrl,
) -> Result<Vec<u8>, PlainHttpError> {
    let method = req.method.as_str();
    let mut request = format!(
        "{method} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
        url.path_and_query, url.host
    )
    .into_bytes();

    for header in &req.headers {
        if !is_valid_plain_http_header_name(&header.name) || header.value.contains(['\r', '\n']) {
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

    Ok(request)
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
}

fn contains_http_unsafe_ascii(input: &str) -> bool {
    input
        .bytes()
        .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
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
    #[error("host-controlled HTTP header")]
    HostControlledHeader,
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
    fn url_parser_rejects_request_line_injection_characters() {
        assert_eq!(
            PlainHttpUrl::parse("http://127.0.0.1:8080/path\r\nX-Bad: yes").unwrap_err(),
            PlainHttpError::InvalidUrl
        );
        assert_eq!(
            PlainHttpUrl::parse("http://127.0.0.1:8080/path with spaces").unwrap_err(),
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
}

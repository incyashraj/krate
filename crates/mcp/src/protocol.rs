//! The JSON-RPC 2.0 / MCP wire layer, with no knowledge of Krate.
//!
//! Split from the tools deliberately: the protocol is fiddly and worth testing
//! on its own, and the tools are slow (they compile Rust). Everything here is
//! pure -- a message in, a message out -- so the whole handshake can be tested
//! without touching a filesystem or spawning a process.
//!
//! Wire format, from the MCP specification (revision 2025-11-25):
//!
//!   - JSON-RPC 2.0, UTF-8, over stdin/stdout.
//!   - One message per line. A message MUST NOT contain an embedded newline,
//!     which `serde_json::to_string` guarantees since it escapes them.
//!   - The server MUST NOT write anything to stdout that is not an MCP
//!     message. Logging goes to stderr, which the client may ignore.
//!   - A notification (no `id`) gets no response, ever.

use serde_json::{json, Value};

/// The protocol revision this server implements.
///
/// `2025-11-25` is the current stable specification. `2026-07-28` exists as a
/// release candidate; we do not claim it, because we do not implement its
/// additions (Tasks, Extensions), and claiming a version you do not implement
/// is how a client ends up calling something that is not there.
pub const PROTOCOL_VERSION: &str = "2025-11-25";

/// Revisions we can actually speak. Older clients pin older strings, and the
/// wire shape this server uses is unchanged across all of them: initialize,
/// tools/list, tools/call. When a client asks for one of these we echo it back
/// rather than forcing our own, which is what the spec asks for.
pub const SUPPORTED_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// JSON-RPC error codes. The first four are from the JSON-RPC 2.0 standard.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;

/// What a server exposes to the protocol layer. Implemented by the Krate tool
/// set; a fake implementation makes the protocol testable in isolation.
pub trait ToolSet {
    /// The name reported in `serverInfo`.
    fn server_name(&self) -> &str;

    /// The version reported in `serverInfo`.
    fn server_version(&self) -> &str;

    /// Optional guidance handed to the model at initialize time. This is real
    /// leverage: it is the one chance to tell a model how the tools fit
    /// together before it starts guessing.
    fn instructions(&self) -> Option<String> {
        None
    }

    /// Every tool, as MCP tool definitions.
    fn tools(&self) -> Vec<Value>;

    /// Run one tool.
    ///
    /// `Ok` is a successful call. `Err` is a *tool execution* error: the call
    /// reached the tool and the tool has something to say about why it did not
    /// work. Per the spec these come back as a result with `isError: true`,
    /// not as a JSON-RPC error, precisely so the model reads the text and
    /// corrects itself. Only an unknown tool name is a protocol error.
    fn call(&self, name: &str, arguments: &Value) -> Result<Value, String>;
}

/// Handle one incoming line. `None` means "write nothing", which is the correct
/// and required response to a notification.
pub fn handle_line(tools: &dyn ToolSet, raw: &str) -> Option<Value> {
    let message: Value = match serde_json::from_str(raw) {
        Ok(message) => message,
        // A parse error has no id to answer against, so the spec's null-id
        // error response is the only honest reply.
        Err(err) => {
            return Some(error_response(
                Value::Null,
                PARSE_ERROR,
                &format!("parse error: {err}"),
                None,
            ))
        }
    };

    // A batch is an array. Batching was removed in the 2025-06-18 revision and
    // this server does not accept it -- but say so, rather than dropping the
    // message and leaving the client waiting for a reply that never comes.
    if message.is_array() {
        return Some(error_response(
            Value::Null,
            INVALID_REQUEST,
            "this server does not accept JSON-RPC batches; send one message per line",
            None,
        ));
    }

    let id = message.get("id").cloned();

    let Some(method) = message.get("method").and_then(Value::as_str) else {
        // No method and no id is not a message we can do anything with. No
        // method but an id is a malformed request we can answer.
        return id.map(|id| {
            error_response(
                id,
                INVALID_REQUEST,
                "message has no `method` field",
                None,
            )
        });
    };
    let method = method.to_string();
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    // No id means a notification: do the work, answer nothing.
    let Some(id) = id else {
        return None;
    };

    let result = match method.as_str() {
        "initialize" => match initialize(tools, &params) {
            Ok(result) => result,
            Err(err) => {
                return Some(error_response(
                    id,
                    INVALID_PARAMS,
                    &err.message,
                    err.data,
                ))
            }
        },
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools.tools() }),
        "tools/call" => match tools_call(tools, &params) {
            Ok(result) => result,
            Err(err) => {
                return Some(error_response(id, err.code, &err.message, err.data));
            }
        },
        // Everything else, including features we do not advertise
        // (resources/*, prompts/*), is honestly reported as absent rather than
        // faked with an empty list.
        other => {
            return Some(error_response(
                id,
                METHOD_NOT_FOUND,
                &format!("this server does not implement `{other}`"),
                None,
            ))
        }
    };

    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// A protocol-level failure: a code, a message, and optional structured data.
struct RpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

/// The initialize handshake, including version negotiation.
///
/// The rule from the spec: if the server supports the version the client asked
/// for, it MUST answer with that same version; otherwise it MUST answer with
/// one it does support. Answering with our own version unconditionally is the
/// common bug -- it makes an older client believe it negotiated something it
/// cannot speak.
fn initialize(tools: &dyn ToolSet, params: &Value) -> Result<Value, RpcError> {
    let requested = params.get("protocolVersion").and_then(Value::as_str);

    let agreed = match requested {
        Some(version) if SUPPORTED_VERSIONS.contains(&version) => version,
        // An unknown version is not fatal: offer ours and let the client decide
        // whether it can speak it (the spec says it SHOULD disconnect if not).
        _ => PROTOCOL_VERSION,
    };

    let mut result = json!({
        "protocolVersion": agreed,
        // Only what we actually serve. `listChanged: false` is the truth: the
        // tool list is fixed for the life of the process, so a client that
        // subscribed to changes would wait forever.
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": tools.server_name(),
            "version": tools.server_version(),
        },
    });
    if let Some(instructions) = tools.instructions() {
        result["instructions"] = Value::String(instructions);
    }
    Ok(result)
}

/// Dispatch a `tools/call`.
fn tools_call(tools: &dyn ToolSet, params: &Value) -> Result<Value, RpcError> {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Err(RpcError {
            code: INVALID_PARAMS,
            message: "tools/call needs a `name`".to_string(),
            data: None,
        });
    };

    // Absent arguments are an empty object, not an error: a tool that takes no
    // parameters is legitimately called with nothing.
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => json!({}),
        Some(value) if value.is_object() => value.clone(),
        Some(_) => {
            return Err(RpcError {
                code: INVALID_PARAMS,
                message: "`arguments` must be an object".to_string(),
                data: None,
            })
        }
    };

    let known: Vec<String> = tools
        .tools()
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    if !known.iter().any(|candidate| candidate == name) {
        // An unknown tool is a protocol error: the model cannot fix it by
        // adjusting arguments. Naming the real tools is what makes it
        // recoverable anyway.
        return Err(RpcError {
            code: METHOD_NOT_FOUND,
            message: format!(
                "unknown tool `{name}`; this server offers: {}",
                known.join(", ")
            ),
            data: None,
        });
    }

    match tools.call(name, &arguments) {
        Ok(value) => Ok(tool_result(&value, false)),
        // A tool execution error. Per the spec this is a *successful* JSON-RPC
        // response carrying isError, so the client hands the text to the model
        // and the model can correct itself. Returning -32603 here instead would
        // hide the actionable message behind a protocol failure.
        Err(message) => Ok(tool_result(&Value::String(message), true)),
    }
}

/// Build a CallToolResult.
///
/// A structured value goes in `structuredContent` *and*, per the spec's
/// backwards-compatibility note, serialized into a text block -- because many
/// clients still only read `content`, and a model that gets an empty content
/// array learns nothing.
fn tool_result(value: &Value, is_error: bool) -> Value {
    let text = match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    };

    let mut result = json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
    });
    if !is_error && value.is_object() {
        result["structuredContent"] = value.clone();
    }
    result
}

/// Build a JSON-RPC error response.
pub fn error_response(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tool set with no moving parts, so these tests exercise the protocol
    /// and nothing else.
    struct Fake;

    impl ToolSet for Fake {
        fn server_name(&self) -> &str {
            "fake"
        }
        fn server_version(&self) -> &str {
            "9.9.9"
        }
        fn instructions(&self) -> Option<String> {
            Some("read this first".to_string())
        }
        fn tools(&self) -> Vec<Value> {
            vec![json!({
                "name": "echo",
                "description": "echo back",
                "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } } },
            })]
        }
        fn call(&self, name: &str, arguments: &Value) -> Result<Value, String> {
            match name {
                "echo" => match arguments.get("text").and_then(Value::as_str) {
                    Some(text) => Ok(json!({ "echoed": text })),
                    None => Err("echo needs `text`, a string".to_string()),
                },
                other => Err(format!("unhandled {other}")),
            }
        }
    }

    fn call(raw: &str) -> Value {
        handle_line(&Fake, raw).expect("a response was expected")
    }

    #[test]
    fn initialize_agrees_on_the_version_the_client_asked_for() {
        let response = call(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
        );
        // The spec: if the server supports the requested version it MUST reply
        // with that same one. Replying with our own newest would strand a
        // client that cannot speak it.
        assert_eq!(response["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(response["result"]["serverInfo"]["name"], "fake");
        assert_eq!(response["result"]["serverInfo"]["version"], "9.9.9");
        assert_eq!(response["result"]["instructions"], "read this first");
        assert!(response["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn an_unknown_version_gets_ours_back_rather_than_a_failure() {
        let response = call(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        );
        assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);
    }

    #[test]
    fn initialize_without_a_version_still_works() {
        let response = call(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#);
        assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);
    }

    #[test]
    fn tools_list_carries_a_valid_input_schema() {
        let response = call(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let tools = response["result"]["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "echo");
        // The spec requires inputSchema to be a JSON Schema object, never null.
        assert_eq!(tools[0]["inputSchema"]["type"], "object");
    }

    #[test]
    fn a_good_call_returns_text_and_structured_content() {
        let response = call(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"text":"hi"}}}"#,
        );
        assert_eq!(response["result"]["isError"], false);
        assert_eq!(response["result"]["structuredContent"]["echoed"], "hi");
        // Backwards compatibility: the same data must also be in a text block,
        // because plenty of clients read only `content`.
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .expect("text block");
        let parsed: Value = serde_json::from_str(text).expect("text block parses as json");
        assert_eq!(parsed["echoed"], "hi");
    }

    #[test]
    fn a_bad_argument_is_a_tool_error_not_a_protocol_error() {
        let response = call(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"echo","arguments":{}}}"#,
        );
        // This is the distinction that decides whether a model can recover:
        // a result with isError reaches the model, a JSON-RPC error usually
        // does not.
        assert!(response.get("error").is_none());
        assert_eq!(response["result"]["isError"], true);
        assert!(response["result"]["content"][0]["text"]
            .as_str()
            .expect("text")
            .contains("needs `text`"));
    }

    #[test]
    fn an_unknown_tool_is_a_protocol_error_that_names_the_real_ones() {
        let response = call(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nope"}}"#,
        );
        assert_eq!(response["error"]["code"], METHOD_NOT_FOUND);
        let message = response["error"]["message"].as_str().expect("message");
        assert!(message.contains("nope"));
        // Naming what does exist is what turns a dead end into a retry.
        assert!(message.contains("echo"));
    }

    #[test]
    fn a_call_without_a_name_is_invalid_params() {
        let response = call(r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{}}"#);
        assert_eq!(response["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn non_object_arguments_are_refused() {
        let response = call(
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"echo","arguments":"hi"}}"#,
        );
        assert_eq!(response["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn a_tool_with_no_arguments_key_is_called_with_an_empty_object() {
        // "arguments" absent must not be an error: a no-parameter tool is
        // called exactly this way by real clients.
        let response = call(
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"echo"}}"#,
        );
        assert!(response.get("error").is_none());
        assert_eq!(response["result"]["isError"], true);
    }

    #[test]
    fn notifications_get_no_response_at_all() {
        assert!(handle_line(&Fake, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        assert!(handle_line(&Fake, r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#).is_none());
    }

    #[test]
    fn ping_is_answered_with_an_empty_result() {
        let response = call(r#"{"jsonrpc":"2.0","id":9,"method":"ping"}"#);
        assert_eq!(response["result"], json!({}));
    }

    #[test]
    fn unknown_methods_say_so_instead_of_faking_a_result() {
        let response = call(r#"{"jsonrpc":"2.0","id":10,"method":"resources/list"}"#);
        assert_eq!(response["error"]["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn malformed_json_gets_a_parse_error_with_a_null_id() {
        let response = handle_line(&Fake, "{not json").expect("response");
        assert_eq!(response["error"]["code"], PARSE_ERROR);
        assert_eq!(response["id"], Value::Null);
    }

    #[test]
    fn batches_are_refused_rather_than_silently_dropped() {
        // Batching left the spec in 2025-06-18. A client that sends one must
        // hear back, or it hangs waiting on a reply we were never going to send.
        let response = handle_line(&Fake, r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#)
            .expect("response");
        assert_eq!(response["error"]["code"], INVALID_REQUEST);
    }

    #[test]
    fn every_response_is_one_line_with_no_embedded_newline() {
        // The stdio transport requires it: messages are newline delimited and
        // MUST NOT contain embedded newlines. Our multi-line text content is
        // only safe because serde escapes it, so prove that it does.
        struct Multiline;
        impl ToolSet for Multiline {
            fn server_name(&self) -> &str {
                "m"
            }
            fn server_version(&self) -> &str {
                "1"
            }
            fn tools(&self) -> Vec<Value> {
                vec![json!({"name":"t","description":"d","inputSchema":{"type":"object"}})]
            }
            fn call(&self, _name: &str, _arguments: &Value) -> Result<Value, String> {
                Err("line one\nline two\nline three".to_string())
            }
        }
        let response =
            handle_line(&Multiline, r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"t"}}"#)
                .expect("response");
        let encoded = serde_json::to_string(&response).expect("encode");
        assert!(!encoded.contains('\n'), "encoded message must be one line");
    }
}

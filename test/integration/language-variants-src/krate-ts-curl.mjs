import { raw } from "layer36:io/args@0.1.0";
import { stderr, stdout } from "layer36:io/stdio@0.1.0";
import { get } from "layer36:net/http-client@0.1.0";

const encoder = new TextEncoder();

function writeLine(stream, value) {
  stream.writeAll(encoder.encode(`${value}\n`));
}

function parseTaggedError(err) {
  if (!err || typeof err !== "object") {
    return { tag: null, payload: null };
  }

  const record = err;
  const directTag = typeof record.tag === "string" ? record.tag : null;
  const directPayload =
    typeof record.payload === "string"
      ? record.payload
      : typeof record.val === "string"
        ? record.val
        : null;
  if (directTag || directPayload) {
    return { tag: directTag, payload: directPayload };
  }

  const nested =
    record.payload && typeof record.payload === "object"
      ? record.payload
      : record.val && typeof record.val === "object"
        ? record.val
        : null;
  if (!nested) {
    return { tag: null, payload: null };
  }

  return {
    tag: typeof nested.tag === "string" ? nested.tag : null,
    payload:
      typeof nested.payload === "string"
        ? nested.payload
        : typeof nested.val === "string"
          ? nested.val
          : null,
  };
}

function classifyError(err) {
  const { tag, payload } = parseTaggedError(err);

  if (tag === "permission-denied") {
    return { message: "layer36-ts-curl: permission denied", code: 5 };
  }
  if (tag === "invalid-url") {
    return { message: "layer36-ts-curl: invalid url", code: 20 };
  }
  if (tag === "body-too-large") {
    return { message: "layer36-ts-curl: response too large", code: 21 };
  }
  if (tag === "timeout") {
    return { message: "layer36-ts-curl: request timed out", code: 21 };
  }
  if (tag === "protocol") {
    return { message: "layer36-ts-curl: protocol error", code: 21 };
  }
  if (tag === "tls-failure") {
    return { message: "layer36-ts-curl: tls handshake failed", code: 21 };
  }
  if (tag === "dns-failure") {
    return { message: "layer36-ts-curl: dns lookup failed", code: 21 };
  }
  if (tag === "connect-failure") {
    return { message: "layer36-ts-curl: connection failed", code: 21 };
  }
  if (tag === "other") {
    return { message: "layer36-ts-curl: fetch failed", code: 21 };
  }

  if (typeof err === "string") {
    return { message: `layer36-ts-curl: ${err}`, code: 21 };
  }
  if (payload) {
    return { message: `layer36-ts-curl: ${payload}`, code: 21 };
  }

  try {
    const asJson = JSON.stringify(err);
    if (asJson && asJson !== "{}") {
      return { message: `layer36-ts-curl: ${asJson}`, code: 21 };
    }
  } catch {
    // Fall back to default conversion below.
  }

  return { message: `layer36-ts-curl: ${String(err)}`, code: 21 };
}

export function run() {
  const url = raw()
    .split("\n")
    .find((value) => value.length > 0);
  if (!url) {
    writeLine(stderr(), "usage: layer36-ts-curl <url>");
    return 2;
  }

  try {
    const out = stdout();
    out.writeAll(get(url));
    out.flush();
    return 0;
  } catch (err) {
    const classified = classifyError(err);
    writeLine(stderr(), classified.message);
    return classified.code;
  }
}

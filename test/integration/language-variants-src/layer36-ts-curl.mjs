import { raw } from "layer36:io/args@0.1.0";
import { stderr, stdout } from "layer36:io/stdio@0.1.0";
import { get } from "layer36:net/http-client@0.1.0";

const encoder = new TextEncoder();

function writeLine(stream, value) {
  stream.writeAll(encoder.encode(`${value}\n`));
}

function describeError(err) {
  if (typeof err === "string") {
    return err;
  }

  if (err && typeof err === "object") {
    const record = err;
    const tag = typeof record.tag === "string" ? record.tag : null;
    const payload =
      typeof record.payload === "string"
        ? record.payload
        : typeof record.val === "string"
          ? record.val
          : null;
    if (tag && payload) {
      return `${tag}: ${payload}`;
    }
    if (tag) {
      return tag;
    }
    if (payload) {
      return payload;
    }
    try {
      const asJson = JSON.stringify(record);
      if (asJson && asJson !== "{}") {
        return asJson;
      }
    } catch {
      // Fall back to default string conversion below.
    }
  }

  return String(err);
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
    writeLine(stderr(), `layer36-ts-curl: ${describeError(err)}`);
    writeLine(stderr(), "layer36-ts-curl: fetch failed");
    return 21;
  }
}

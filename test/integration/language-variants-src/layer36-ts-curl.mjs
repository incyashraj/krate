import { raw } from "layer36:io/args@0.1.0";
import { stderr, stdout } from "layer36:io/stdio@0.1.0";
import { get } from "layer36:net/http-client@0.1.0";

const encoder = new TextEncoder();

function writeLine(stream, value) {
  stream.writeAll(encoder.encode(`${value}\n`));
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
    writeLine(stderr(), `layer36-ts-curl: ${String(err)}`);
    writeLine(stderr(), "layer36-ts-curl: fetch failed");
    return 21;
  }
}

import { raw } from "layer36:io/args@0.1.0";
import { stderr, stdout } from "layer36:io/stdio@0.1.0";
import { open } from "layer36:fs/files@0.1.0";

const encoder = new TextEncoder();

function writeLine(stream, value) {
  stream.writeAll(encoder.encode(`${value}\n`));
}

export function run() {
  const args = raw()
    .split("\n")
    .filter((value) => value.length > 0);
  if (args.length === 0) {
    writeLine(stderr(), "usage: layer36-ts-cat <path> [path...]");
    return 2;
  }

  try {
    const out = stdout();
    for (const path of args) {
      const file = open(path, { tag: "read" });
      try {
        while (true) {
          const bytes = file.read(8192);
          if (bytes.length === 0) {
            break;
          }
          out.writeAll(bytes);
        }
      } finally {
        file[Symbol.dispose]();
      }
    }
    out.flush();
    return 0;
  } catch (err) {
    writeLine(stderr(), `layer36-ts-cat: ${String(err)}`);
    writeLine(stderr(), "layer36-ts-cat: read failed");
    return 21;
  }
}

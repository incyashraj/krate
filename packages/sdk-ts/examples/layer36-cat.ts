import { fs, io } from "@layer36/sdk";

const paths = io.args();

if (paths.length === 0) {
  io.eprintln("usage: layer36-ts-cat <path> [path...]");
  throw new Error("missing path");
}

for (const file of paths) {
  io.print(fs.readText(file));
}

import { fs, io } from "@krate/sdk";

const paths = io.args();

if (paths.length === 0) {
  io.eprintln("usage: krate-ts-cat <path> [path...]");
  throw new Error("missing path");
}

for (const file of paths) {
  io.print(fs.readText(file));
}

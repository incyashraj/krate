import { readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);

const fail = (message) => {
  console.error(`Go SDK shape check failed: ${message}`);
  process.exitCode = 1;
};

const readText = (relativePath) =>
  readFile(path.join(packageRoot, relativePath), "utf8");

const requireFile = (relativePath) => {
  if (!existsSync(path.join(packageRoot, relativePath))) {
    fail(`missing ${relativePath}`);
  }
};

for (const relativePath of [
  "go.mod",
  "README.md",
  "layer36/internal_missing.go",
  "layer36/io/io.go",
  "layer36/fs/fs.go",
  "layer36/net/net.go",
  "layer36/time/time.go",
  "layer36/locale/locale.go",
  "examples/layer36-cat/main.go",
  "examples/layer36-clock/main.go",
  "examples/layer36-curl/main.go",
]) {
  requireFile(relativePath);
}

const moduleFile = await readText("go.mod");
if (!moduleFile.includes("module github.com/incyashraj/layer6x6/packages/sdk-go")) {
  fail("go.mod has the wrong module path");
}

for (const [relativePath, tokens] of Object.entries({
  "layer36/io/io.go": ["func Args()", "func Print(", "func Eprintln("],
  "layer36/fs/fs.go": ["type OpenMode string", "func ReadText(", "func WriteText("],
  "layer36/net/net.go": ["type Request struct", "func GetText(", "func Fetch("],
  "layer36/time/time.go": ["func NowMillis()", "func SleepMillis("],
  "layer36/locale/locale.go": ["type LocaleID struct", "func FormatDate(", "func FormatNumber("],
})) {
  const source = await readText(relativePath);
  for (const token of tokens) {
    if (!source.includes(token)) {
      fail(`${relativePath} is missing ${token}`);
    }
  }
  if (source.includes("wasi:")) {
    fail(`${relativePath} must not depend on direct wasi:* imports`);
  }
}

if (process.exitCode) {
  process.exit();
}

for (const [relativePath, tokens] of Object.entries({
  "examples/layer36-cat/main.go": [
    "usage: layer36-go-cat <path> [path...]",
    "l36fs.ReadText(file)",
    "l36io.Print(body)",
  ],
  "examples/layer36-clock/main.go": [
    "app=layer36-go-clock",
    "locale=",
    "timezone=",
    "date=",
  ],
  "examples/layer36-curl/main.go": [
    "usage: layer36-go-curl <url>",
    "l36net.GetText(args[0])",
    "l36io.Print(body)",
  ],
})) {
  const source = await readText(relativePath);
  for (const token of tokens) {
    if (!source.includes(token)) {
      fail(`${relativePath} is missing ${token}`);
    }
  }
}

if (process.exitCode) {
  process.exit();
}

console.log("Go SDK shape check passed");

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
  "krate/internal_missing.go",
  "krate/io/io.go",
  "krate/fs/fs.go",
  "krate/net/net.go",
  "krate/time/time.go",
  "krate/locale/locale.go",
  "examples/krate-cat/main.go",
  "examples/krate-clock/main.go",
  "examples/krate-curl/main.go",
]) {
  requireFile(relativePath);
}

const moduleFile = await readText("go.mod");
if (!moduleFile.includes("module github.com/incyashraj/krate/packages/sdk-go")) {
  fail("go.mod has the wrong module path");
}

for (const [relativePath, tokens] of Object.entries({
  "krate/io/io.go": ["func Args()", "func Print(", "func Eprintln("],
  "krate/fs/fs.go": ["type OpenMode string", "func ReadText(", "func WriteText("],
  "krate/net/net.go": ["type Request struct", "func GetText(", "func Fetch("],
  "krate/time/time.go": ["func NowMillis()", "func SleepMillis("],
  "krate/locale/locale.go": ["type LocaleID struct", "func FormatDate(", "func FormatNumber("],
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
  "examples/krate-cat/main.go": [
    "usage: krate-go-cat <path> [path...]",
    "l36fs.ReadText(file)",
    "l36io.Print(body)",
  ],
  "examples/krate-clock/main.go": [
    "app=krate-go-clock",
    "locale=",
    "timezone=",
    "date=",
  ],
  "examples/krate-curl/main.go": [
    "usage: krate-go-curl <url>",
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

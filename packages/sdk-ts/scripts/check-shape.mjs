import { readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);

const fail = (message) => {
  console.error(`TypeScript SDK shape check failed: ${message}`);
  process.exitCode = 1;
};

const readText = (relativePath) =>
  readFile(path.join(packageRoot, relativePath), "utf8");

const requireFile = (relativePath) => {
  if (!existsSync(path.join(packageRoot, relativePath))) {
    fail(`missing ${relativePath}`);
  }
};

const pkg = JSON.parse(await readText("package.json"));

if (pkg.name !== "@layer36/sdk") {
  fail(`package name is ${pkg.name}, expected @layer36/sdk`);
}

if (pkg.type !== "module") {
  fail("package must stay ESM-only with type=module");
}

if (pkg.private !== true) {
  fail("package should remain private until the jco runtime proof lands");
}

for (const relativePath of [
  "README.md",
  "tsconfig.json",
  "src/index.ts",
  "src/imports.d.ts",
  "src/io.ts",
  "src/fs.ts",
  "src/net.ts",
  "src/time.ts",
  "src/locale.ts",
  "examples/layer36-cat.ts",
  "examples/layer36-clock.ts",
  "examples/layer36-curl.ts",
]) {
  requireFile(relativePath);
}

const index = await readText("src/index.ts");
for (const moduleName of ["fs", "io", "locale", "net", "time"]) {
  if (!index.includes(`export * as ${moduleName} from "./${moduleName}.js";`)) {
    fail(`src/index.ts does not export ${moduleName}`);
  }
}

const imports = await readText("src/imports.d.ts");
for (const moduleName of [
  "layer36:io/streams",
  "layer36:io/stdio",
  "layer36:io/args",
  "layer36:io/log",
  "layer36:fs/files",
  "layer36:net/http-client",
  "layer36:time/clock",
  "layer36:time/sleep",
  "layer36:locale/info",
  "layer36:locale/format",
]) {
  if (!imports.includes(`declare module "${moduleName}"`)) {
    fail(`src/imports.d.ts is missing ${moduleName}`);
  }
}

if (imports.includes("wasi:")) {
  fail("SDK declarations must not depend on direct wasi:* imports");
}

if (process.exitCode) {
  process.exit();
}

for (const [relativePath, tokens] of Object.entries({
  "examples/layer36-cat.ts": [
    "usage: layer36-ts-cat <path> [path...]",
    "io.print(fs.readText(file));",
  ],
  "examples/layer36-clock.ts": [
    "app=layer36-ts-clock",
    "locale=",
    "timezone=",
    "date=",
  ],
  "examples/layer36-curl.ts": [
    "usage: layer36-ts-curl <url>",
    "io.print(net.getText(url));",
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

console.log("TypeScript SDK shape check passed");

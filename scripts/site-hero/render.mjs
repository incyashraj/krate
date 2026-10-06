// The homepage's six desktops (docs/landing/os-{mac,win,linux}[-dark].webp):
// a real Krate app, built and shot by the engine, framed on each system.
// usage: node scripts/site-hero/render.mjs [app dir]   (default apps/krate-editor)
// Needs: a release `krate` in target/, cwebp, and Playwright
// (KRATE_PLAYWRIGHT_MODULE points at a module exporting `chromium`, as the site gate does).
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const root = path.resolve(path.dirname(new URL(import.meta.url).pathname), "../..");
const appDir = path.resolve(root, process.argv[2] || "apps/krate-editor");
const krate = path.join(root, "target/release/krate");
const work = fs.mkdtempSync(path.join(process.env.TMPDIR || "/tmp", "site-hero-"));
const manifest = fs.readFileSync(path.join(appDir, "manifest.toml"), "utf8");
const name = (manifest.match(/^name\s*=\s*"([^"]+)"/m) || [, "App"])[1];
const entry = (manifest.match(/^entry\s*=\s*"([^"]+)"/m) || [])[1];

// Build and check it, pack it the way Studio does (source included), and
// read the size people would see on the file.
execFileSync(krate, ["check-app", appDir], { stdio: "inherit" });
const bundle = path.join(work, `${name}.krate`);
execFileSync(krate, ["pack", "--manifest", path.join(appDir, "manifest.toml"), "-o", bundle, path.join(appDir, entry)], { stdio: "inherit" });
const kb = Math.round(fs.statSync(bundle).size / 1024);
// The app's own frame, five seconds in (the Editor types on the clock).
const shot = path.join(work, "app.png");
execFileSync(krate, ["run", bundle, "--shoot", shot], { env: { ...process.env, KRATE_SHOOT_AFTER_MS: process.env.SHOOT_AFTER_MS || "5000" }, stdio: "inherit" });

// The frame alone too, for the page's small windows (the opening scene).
execFileSync("cwebp", ["-quiet", "-q", "86", "-resize", "1180", "0", shot, "-o", path.join(root, "docs/landing/app-frame.webp")]);
console.log("wrote docs/landing/app-frame.webp");

const pw = await import(process.env.KRATE_PLAYWRIGHT_MODULE || "playwright");
const chromium = pw.chromium || pw.default.chromium;
const b = await chromium.launch();
const page = await (await b.newContext({ viewport: { width: 1200, height: 760 }, deviceScaleFactor: 2 })).newPage();
const html = pathToFileURL(path.join(root, "scripts/site-hero/desktop.html")).href;
for (const os of ["mac", "win", "linux"]) {
  for (const theme of ["light", "dark"]) {
    const qs = new URLSearchParams({ os, theme, name, shot: pathToFileURL(shot).href, file: `${name}.krate`, size: `${kb} KB` });
    await page.goto(`${html}?${qs}`);
    await page.waitForLoadState("networkidle"); await page.evaluate(() => document.fonts.ready);
    const png = path.join(work, `os-${os}${theme === "dark" ? "-dark" : ""}.png`);
    await page.screenshot({ path: png });
    const out = path.join(root, "docs/landing", path.basename(png, ".png") + ".webp");
    execFileSync("cwebp", ["-quiet", "-q", "86", png, "-o", out]);
    console.log(`wrote ${path.relative(root, out)}`);
  }
}
await b.close();
console.log(`${name}.krate is ${kb} KB (${fs.statSync(bundle).size} bytes); use that number on the page.`);

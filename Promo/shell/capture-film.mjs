// Capture the 20-second film shell (film20.html) to PNG frames.
//   node capture-film.mjs test <ms> [ms...]  -> ../out-film/test/test_t<ms>.png
//   node capture-film.mjs all [fps]          -> ../out-film/frames/f#####.png
// Same SEEK/TOTAL/PARTS contract as every Promo shell.
import puppeteer from "puppeteer";
import { mkdirSync } from "fs";
import { fileURLToPath } from "url";
import { dirname, join } from "path";

const here = dirname(fileURLToPath(import.meta.url));
const page_url = "file://" + join(here, "film20.html");
const [mode, arg1] = process.argv.slice(2);

const browser = await puppeteer.launch({
  headless: "new",
  args: ["--force-color-profile=srgb", "--disable-lcd-text", "--hide-scrollbars"],
});
const page = await browser.newPage();
await page.setViewport({ width: 1920, height: 1080, deviceScaleFactor: 1 });
await page.goto(page_url, { waitUntil: "networkidle0" });
await page.evaluate(() => document.fonts.ready);
await new Promise(r => setTimeout(r, 200));

async function frame(t, path) {
  await page.evaluate(ms => window.SEEK(ms), t);
  await new Promise(r => setTimeout(r, 20));
  await page.screenshot({ path, clip: { x: 0, y: 0, width: 1920, height: 1080 } });
}

if (mode === "test") {
  const dir = join(here, "..", "out-film", "test");
  mkdirSync(dir, { recursive: true });
  for (const t of process.argv.slice(3).map(Number)) {
    await frame(t, join(dir, `test_t${t}.png`));
    console.log("wrote test_t" + t);
  }
} else {
  const fps = Number(arg1) || 30;
  const total = await page.evaluate(() => window.TOTAL);
  const dir = join(here, "..", "out-film", "frames");
  mkdirSync(dir, { recursive: true });
  const step = 1000 / fps;
  let n = 0;
  for (let t = 0; t < total; t += step) {
    await frame(Math.round(t), join(dir, `f${String(n).padStart(5, "0")}.png`));
    n++;
    if (n % 60 === 0) console.log(`${n} frames`);
  }
  console.log(`done: ${n} frames at ${fps}fps`);
}
await browser.close();

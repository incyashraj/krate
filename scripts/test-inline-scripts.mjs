#!/usr/bin/env node
// Every inline <script> on every site page must parse. A syntax error does not
// break the build or the page's look: the page draws and its script simply
// never runs. The admin support board shipped that way for seven weeks (K-966).
//
//   node scripts/test-inline-scripts.mjs
//
// Classic scripts are compiled with vm.Script; module scripts are checked with
// `node --check` on a temporary .mjs file. JSON-LD blocks are parsed as JSON.
import { readFileSync, readdirSync, statSync, writeFileSync, mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { spawnSync } from 'node:child_process';
import vm from 'node:vm';

const ROOTS = ['docs/landing', 'docs/open', 'docs/cloud', 'docs/support', 'docs/pages', 'docs/answers', 'studio/ui'];
const pages = [];
const walk = d => { for (const n of readdirSync(d)) { const p = join(d, n); if (statSync(p).isDirectory()) walk(p); else if (n.endsWith('.html')) pages.push(p); } };
ROOTS.forEach(walk);

const tmp = mkdtempSync(join(tmpdir(), 'inline-scripts-'));
let checked = 0, bad = 0;
for (const page of pages) {
  const html = readFileSync(page, 'utf8');
  const re = /<script\b([^>]*)>([\s\S]*?)<\/script>/gi;
  let m, n = 0;
  while ((m = re.exec(html))) {
    const attrs = m[1], code = m[2];
    n++;
    if (/\bsrc=/.test(attrs) || !code.trim()) continue;
    const line = html.slice(0, m.index).split('\n').length;
    checked++;
    try {
      if (/application\/ld\+json/.test(attrs)) JSON.parse(code);
      else if (/type=["']?module/.test(attrs)) {
        const f = join(tmp, `s${checked}.mjs`); writeFileSync(f, code);
        const r = spawnSync(process.execPath, ['--check', f], { encoding: 'utf8' });
        if (r.status !== 0) throw new Error((r.stderr || '').split('\n').find(l => /Error/.test(l)) || 'does not parse');
      } else new vm.Script(code, { filename: `${page}:${line}` });
    } catch (e) {
      bad++;
      console.log(`FAIL ${page}:${line} script ${n}: ${String(e.message).split('\n')[0]}`);
    }
  }
}
rmSync(tmp, { recursive: true, force: true });
if (checked < 20) { console.log(`only ${checked} inline scripts found; the finder is broken`); process.exit(1); }
console.log(bad ? `${bad} inline script(s) do not parse` : `ok  ${checked} inline scripts on ${pages.length} pages parse`);
process.exit(bad ? 1 : 0);

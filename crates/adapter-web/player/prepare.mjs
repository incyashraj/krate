// Build the bundles the Player proofs open (IC-956), into build/apps:
//
//   curl, hello-gui, bigscroll, convert, checklist   the apps, packed from their
//                                                   built components (build.sh)
//   camera       hello-gui declaring camera.capture as required: a capability a
//                tab cannot give
//   orgprobe     apps/krate-orgstore-probe with store.org made optional, published
//                as an ordinary app: it runs, and has no team data (not-an-org-app)
//   tampered     the organisation fixture signed by acme's root, with one custom
//                section added to its component after signing: still a valid
//                component, no longer what was signed
//   twin         hello-gui with a different manifest comment: only its hash is
//                used, as a link whose bytes the store swapped
//
//   node prepare.mjs
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { inflateRawSync, deflateRawSync, crc32 } from 'node:zlib';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';

const here = fileURLToPath(new URL('.', import.meta.url));
const repo = join(here, '..', '..', '..');
const out = join(here, 'build', 'apps');
mkdirSync(out, { recursive: true });
const pack = (...args) => execFileSync(process.execPath, [join(here, 'pack.mjs'), ...args], { stdio: 'inherit' });
const component = (app) => join(repo, 'apps', `krate-${app}`, 'target', 'wasm32-wasip1', 'release', `krate_${app.replace(/-/g, '_')}.wasm`);
const manifest = (app) => join(repo, 'apps', `krate-${app}`, 'manifest.toml');

for (const app of ['curl', 'hello-gui', 'bigscroll', 'convert', 'checklist']) {
  pack(manifest(app), component(app), join(out, `${app}.krate`));
}
pack(manifest('hello-gui'), component('hello-gui'), join(out, 'camera.krate'), 'camera.capture');
// orgstore-probe with store.org optional and no organisation's mark: it runs in
// a tab and has no team data, as on a computer (team-data-proof.mjs opens it
// as an organisation's release).
pack(manifest('orgstore-probe'), component('orgstore-probe'), join(out, 'orgprobe.krate'), 'optional:store.org');

// ---- zip read/write, enough for these fixtures
const u16 = (b, i) => b[i] | (b[i + 1] << 8);
const u32 = (b, i) => (b[i] | (b[i + 1] << 8) | (b[i + 2] << 16) | (b[i + 3] << 24)) >>> 0;
function entries(zip) {
  let eocd = zip.length - 22;
  while (u32(zip, eocd) !== 0x06054b50) eocd -= 1;
  const count = u16(zip, eocd + 10);
  let at = u32(zip, eocd + 16);
  const list = [];
  for (let n = 0; n < count; n += 1) {
    const method = u16(zip, at + 10);
    const csize = u32(zip, at + 20);
    const nlen = u16(zip, at + 28);
    const elen = u16(zip, at + 30);
    const clen = u16(zip, at + 32);
    const local = u32(zip, at + 42);
    const name = new TextDecoder().decode(zip.subarray(at + 46, at + 46 + nlen));
    const start = local + 30 + u16(zip, local + 26) + u16(zip, local + 28);
    const raw = zip.subarray(start, start + csize);
    list.push({ name, data: method === 8 ? new Uint8Array(inflateRawSync(raw)) : new Uint8Array(raw) });
    at += 46 + nlen + elen + clen;
  }
  return list;
}
function zip(list) {
  const le16 = (n) => [n & 0xff, (n >> 8) & 0xff];
  const le32 = (n) => [n & 0xff, (n >> 8) & 0xff, (n >> 16) & 0xff, (n >>> 24) & 0xff];
  const parts = [];
  const central = [];
  let offset = 0;
  for (const e of list) {
    const name = new TextEncoder().encode(e.name);
    const stored = new Uint8Array(deflateRawSync(e.data));
    const crc = crc32(e.data) >>> 0;
    const head = [...le32(crc), ...le32(stored.length), ...le32(e.data.length), ...le16(name.length), ...le16(0)];
    const local = new Uint8Array([...le32(0x04034b50), ...le16(20), ...le16(0), ...le16(8), ...le16(0), ...le16(0x21), ...head, ...name]);
    central.push(new Uint8Array([...le32(0x02014b50), ...le16(20), ...le16(20), ...le16(0), ...le16(8), ...le16(0), ...le16(0x21), ...head, ...le16(0), ...le16(0), ...le16(0), ...le32(0), ...le32(offset), ...name]));
    parts.push(local, stored);
    offset += local.length + stored.length;
  }
  const size = central.reduce((n, c) => n + c.length, 0);
  return Buffer.concat([...parts, ...central, new Uint8Array([...le32(0x06054b50), ...le16(0), ...le16(0), ...le16(list.length), ...le16(list.length), ...le32(size), ...le32(offset), ...le16(0)])]);
}

// tampered: a custom section appended to the signed component.
const signed = new Uint8Array(readFileSync(join(repo, 'cloud', 'worker', 'test', 'fixtures', 'org', 'root-signed.krate')));
const tampered = entries(signed).map((e) => {
  if (e.name !== 'code.wasm') return e;
  const name = new TextEncoder().encode('krate-tamper');
  const body = [name.length, ...name, ...new TextEncoder().encode('changed after signing')];
  return { name: e.name, data: new Uint8Array([...e.data, 0x00, body.length, ...body]) };
});
writeFileSync(join(out, 'tampered.krate'), zip(tampered));

// twin: hello-gui under another hash.
const twin = entries(new Uint8Array(readFileSync(join(out, 'hello-gui.krate')))).map((e) =>
  e.name === 'manifest.toml' ? { name: e.name, data: new TextEncoder().encode(new TextDecoder().decode(e.data) + '\n# twin\n') } : e);
writeFileSync(join(out, 'twin.krate'), zip(twin));
console.log('prepared', out);

// A .krate for the proofs: an app's manifest (its entry pointed at the
// bundle's code.wasm) and its built component, zipped the way the hub's
// door reads an archive. `krate pack` makes the same two entries; this
// avoids building the whole CLI for a proof run.
//
//   node pack.mjs <manifest.toml> <component.wasm> <out.krate> [capability | optional:<capability>]...
//
// A proof may also import `packKrate(manifestText, componentBytes, extra)`
// and publish the bytes it returns without writing a file (devices-proof).
import { readFileSync, writeFileSync } from 'node:fs';
import { deflateRawSync, crc32 } from 'node:zlib';
import { pathToFileURL } from 'node:url';

/// The manifest with its entry at code.wasm and each of `extra` applied: a
/// capability added as required, `optional:<capability>` made optional, or
/// `without:<capability>` taken out.
export function proofManifest(text, extra = []) {
  let manifest = text.replace(/^entry\s*=.*$/m, 'entry = "code.wasm"');
  for (const cap of extra) {
    const [how, name] = cap.includes(':') && /^(optional|without):/.test(cap) ? [cap.slice(0, cap.indexOf(':')), cap.slice(cap.indexOf(':') + 1)] : ['add', cap];
    if (how === 'add') {
      manifest += `\n[[capabilities]]\ncap = "${cap}"\nrationale = "Proof: a capability a tab cannot give"\nrequired = true\n`;
      continue;
    }
    manifest = manifest
      .split('[[capabilities]]')
      .map((block) => {
        if (!block.includes(`cap = "${name}"`)) return block;
        // A declared capability made optional: the app runs without it.
        if (how === 'optional') return block.replace('required = true', 'required = false');
        return null;
      })
      .filter((block) => block !== null)
      .join('[[capabilities]]');
  }
  return manifest;
}

/// The .krate's bytes.
export function packKrate(manifestText, component, extra = []) {
  const entries = [
    { name: 'manifest.toml', data: new TextEncoder().encode(proofManifest(manifestText, extra)) },
    { name: 'code.wasm', data: new Uint8Array(component) },
  ];
  const le16 = (n) => [n & 0xff, (n >> 8) & 0xff];
  const le32 = (n) => [n & 0xff, (n >> 8) & 0xff, (n >> 16) & 0xff, (n >>> 24) & 0xff];
  const parts = [];
  const central = [];
  let offset = 0;
  for (const e of entries) {
    const name = new TextEncoder().encode(e.name);
    const stored = new Uint8Array(deflateRawSync(e.data, { level: 9 }));
    const crc = crc32(e.data) >>> 0;
    const local = new Uint8Array([
      ...le32(0x04034b50), ...le16(20), ...le16(0), ...le16(8), ...le16(0), ...le16(0x21),
      ...le32(crc), ...le32(stored.length), ...le32(e.data.length), ...le16(name.length), ...le16(0), ...name,
    ]);
    central.push(new Uint8Array([
      ...le32(0x02014b50), ...le16(20), ...le16(20), ...le16(0), ...le16(8), ...le16(0), ...le16(0x21),
      ...le32(crc), ...le32(stored.length), ...le32(e.data.length), ...le16(name.length), ...le16(0), ...le16(0),
      ...le16(0), ...le16(0), ...le32(0), ...le32(offset), ...name,
    ]));
    parts.push(local, stored);
    offset += local.length + stored.length;
  }
  const cdSize = central.reduce((n, c) => n + c.length, 0);
  const eocd = new Uint8Array([
    ...le32(0x06054b50), ...le16(0), ...le16(0), ...le16(entries.length), ...le16(entries.length),
    ...le32(cdSize), ...le32(offset), ...le16(0),
  ]);
  return Buffer.concat([...parts, ...central, eocd]);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [manifestPath, componentPath, outPath, ...extra] = process.argv.slice(2);
  const all = packKrate(readFileSync(manifestPath, 'utf8'), readFileSync(componentPath), extra);
  writeFileSync(outPath, all);
  console.log(`${outPath} ${all.length} bytes`);
}

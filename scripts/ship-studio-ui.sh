#!/usr/bin/env bash
#
# Ship Studio's interface to every installed Studio, without a release (IC-998).
#
#   scripts/ship-studio-ui.sh
#
# Packs studio/ui as it is in the committed tree into one bundle, signs it
# with Krate's interface key (~/.krate-studio-ui/ui-signing-key.pem, this
# machine only), and hands it to the hub. Each Studio checks the signature
# against the key compiled into it (studio/src/live_ui.rs) and uses the
# bundle only when it is newer than its own interface and calls no command
# it lacks. The web Studio is the site: scripts/ship-site.sh ships that.
#
# Running it is the decision to publish.

set -euo pipefail
cd "$(dirname "$0")/.."

KEY="${KRATE_UI_KEY:-$HOME/.krate-studio-ui/ui-signing-key.pem}"
HUB="${KRATE_HUB_URL:-https://hub.krate.tech}"
[ -f "$KEY" ] || { echo "ship-studio-ui: no signing key at $KEY -- this is the one machine that ships Studio's interface" >&2; exit 1; }

# A commit, not loose files: what ships is what the history says shipped.
if ! git diff --quiet HEAD -- studio/ui studio/src/main.rs || [ -n "$(git ls-files --others --exclude-standard -- studio/ui)" ]; then
  echo "ship-studio-ui: studio/ui has changes that are not committed. Commit them, then ship." >&2
  exit 1
fi

TOKEN=$(python3 -c "import json,os;print(json.load(open(os.path.expanduser('~/.krate/github.json')))['token'])" 2>/dev/null || true)
[ -n "$TOKEN" ] || { echo "ship-studio-ui: sign in first (krate account login): the hub takes a bundle from an admin only" >&2; exit 1; }

SERIAL=$(git log -1 --format=%ct -- studio/ui)
COMMIT=$(git rev-parse --short HEAD)
OUT=$(mktemp -d)
trap 'rm -rf "$OUT"' EXIT

node - "$KEY" "$SERIAL" "$COMMIT" "$OUT" <<'JS'
const fs = require("fs"), path = require("path"), crypto = require("crypto");
const [key, serial, commit, out] = process.argv.slice(2);
const root = "studio/ui";
const files = {};
(function walk(dir) {
  for (const name of fs.readdirSync(dir).sort()) {
    const full = path.join(dir, name);
    if (fs.statSync(full).isDirectory()) { walk(full); continue; }
    if (name.endsWith(".bak") || name === ".DS_Store") continue;
    files[path.relative(root, full)] = fs.readFileSync(full).toString("base64");
  }
})(root);
// The commands this interface needs from the desktop: what it calls that
// the desktop Studio of this commit answers. A call only the browser bridge
// answers (the web Studio's own) is already guarded in the interface.
const called = new Set();
for (const [name, b64] of Object.entries(files)) {
  if (!name.endsWith(".js")) continue;
  for (const m of Buffer.from(b64, "base64").toString().matchAll(/invoke\(\s*["']([a-z0-9_]+)["']/g)) called.add(m[1]);
}
const main = fs.readFileSync("studio/src/main.rs", "utf8");
const start = main.indexOf("tauri::generate_handler![") + "tauri::generate_handler![".length;
const handlers = new Set(main.slice(start, main.indexOf("])", start)).split(",")
  .map((n) => n.split("]").pop().trim().split("::").pop()).filter(Boolean));
const commands = [...called].filter((c) => handlers.has(c)).sort();
const bundle = JSON.stringify({ format: 1, serial: Number(serial), commit, commands, files });
const sig = crypto.sign(null, Buffer.from(bundle), crypto.createPrivateKey(fs.readFileSync(key))).toString("hex");
fs.writeFileSync(path.join(out, "body.json"), JSON.stringify({ bundle, sig }));
console.log(`packed ${Object.keys(files).length} files, ${(bundle.length / 1024 / 1024).toFixed(1)} MB, ${commands.length} commands, serial ${serial}`);
JS

answer=$(curl -sS -X PUT "$HUB/admin/studio-ui" -H "authorization: Bearer $TOKEN" -H "content-type: application/json" --data-binary "@$OUT/body.json" -w '\n%{http_code}')
code=$(printf '%s' "$answer" | tail -1)
if [ "$code" != "200" ]; then
  echo "ship-studio-ui: the hub said $code: $(printf '%s' "$answer" | sed '$d')" >&2
  exit 1
fi
echo "Shipped Studio's interface from $COMMIT. Each Studio takes it within seconds of starting, and offers Reload."

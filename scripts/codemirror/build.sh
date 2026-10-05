#!/bin/sh
# Rebuild studio/ui/vendor/codemirror.js, the editor in Studio's IDE.
# The versions are pinned in package.json and package-lock.json beside this
# script; the bundle is one IIFE that sets window.KrateCM (see entry.js).
# CodeMirror 6 and Lezer are MIT-licensed (Marijn Haverbeke and others).
set -e
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
cp "$here/package.json" "$here/package-lock.json" "$here/entry.js" "$work/"
cd "$work"
npm ci --no-audit --no-fund
out="$here/../../studio/ui/vendor/codemirror.js"
{
  printf '%s\n' "/* CodeMirror 6 (codemirror.net) and Lezer, MIT License, (C) Marijn Haverbeke and others."
  printf '%s\n' "   Bundled for Krate Studio's IDE by scripts/codemirror/build.sh; versions in scripts/codemirror/package.json. */"
  npx esbuild entry.js --bundle --minify --format=iife --legal-comments=eof
} > "$out"
echo "wrote $out ($(wc -c < "$out") bytes)"

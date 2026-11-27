#!/usr/bin/env bash
# Sign Krate for Claude Desktop's .mcpb files (IC-1005).
#
#   bash scripts/sign-mcpb.sh [--require] FILE.mcpb...
#
# Claude Desktop shows a .mcpb as signed only when its certificate chains to
# a root the computer trusts (`mcpb verify` asks the OS: `security
# verify-cert -p codeSign` on a Mac, X509Chain with the code-signing policy
# on Windows), and reports a self-signed certificate as unsigned.
#
# The certificate, first found wins:
#   MCPB_SIGN_CERT, MCPB_SIGN_KEY, MCPB_SIGN_INTERMEDIATE   PEM, any public CA
#   CERT_P12, CERT_PASSWORD   the release's Developer ID Application p12
#                             (base64), with Apple's Developer ID intermediate;
#                             it chains to Apple Root CA
#
# Apple's p12 exports use RC2-40, which OpenSSL 3 reads only with -legacy:
# without it the 0.6.2 release could not read the key, said nothing, and
# shipped both files unsigned (K-1387). Every OpenSSL is tried, and a p12
# that none reads is named.
#
# A file whose signature does not verify here goes back to unsigned, never
# out with a signature that reads as broken. --require makes that, or no
# certificate at all, a failure (for re-signing a published release).
set -euo pipefail

require=0
if [ "${1:-}" = "--require" ]; then
  require=1
  shift
fi
[ $# -gt 0 ] || { echo "usage: sign-mcpb.sh [--require] FILE.mcpb..." >&2; exit 2; }

MCPB=(npx --yes @anthropic-ai/mcpb@2.1.2)
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# `openssl pkcs12` with whichever OpenSSL reads an Apple p12: the first on
# PATH, the same with -legacy (OpenSSL 3), then macOS's own LibreSSL.
p12() {
  openssl pkcs12 "$@" 2>/dev/null && return 0
  openssl pkcs12 -legacy "$@" 2>/dev/null && return 0
  [ -x /usr/bin/openssl ] && /usr/bin/openssl pkcs12 "$@" 2>/dev/null && return 0
  return 1
}

cert="" key="" chain=""
if [ -n "${MCPB_SIGN_CERT:-}" ] && [ -n "${MCPB_SIGN_KEY:-}" ]; then
  printf '%s\n' "$MCPB_SIGN_CERT" > "$work/cert.pem"
  printf '%s\n' "$MCPB_SIGN_KEY" > "$work/key.pem"
  cert="$work/cert.pem" key="$work/key.pem"
  if [ -n "${MCPB_SIGN_INTERMEDIATE:-}" ]; then
    printf '%s\n' "$MCPB_SIGN_INTERMEDIATE" > "$work/chain.pem"
    chain="$work/chain.pem"
  fi
  echo "sign-mcpb: with MCPB_SIGN_CERT"
elif [ -n "${CERT_P12:-}" ]; then
  printf '%s' "$CERT_P12" | base64 --decode > "$work/id.p12"
  : "${CERT_PASSWORD:=}"
  export CERT_PASSWORD
  # The p12 can hold several identities (the release's held an Apple
  # Development one first, which no computer trusts for this): take the
  # Developer ID Application certificate and the key whose localKeyID it
  # shares.
  if p12 -in "$work/id.p12" -passin env:CERT_PASSWORD -nodes -out "$work/all.pem"; then
    if python3 - "$work/all.pem" "$work/cert.pem" "$work/key.pem" <<'PY'
import re, subprocess, sys
# `openssl pkcs12 -nodes` prints each bag as "Bag Attributes" lines (its
# localKeyID), maybe subject=/issuer= lines, then the PEM block.
certs, keys = [], []
kid, pem, kind = None, None, None
for line in open(sys.argv[1]):
    if line.startswith("Bag Attributes"):
        kid = None
    m = re.match(r"\s*localKeyID: ([0-9A-Fa-f ]+)", line)
    if m:
        kid = m.group(1).strip().upper()
    m = re.match(r"-----BEGIN ([A-Z ]+)-----", line)
    if m:
        kind, pem = m.group(1), ""
    if pem is not None:
        pem += line
    if kind and line.startswith(f"-----END {kind}-----"):
        if kind == "CERTIFICATE":
            subject = subprocess.run(["openssl", "x509", "-noout", "-subject"],
                                     input=pem, capture_output=True, text=True).stdout
            certs.append((kid, subject, pem))
        elif "PRIVATE KEY" in kind:
            keys.append((kid, pem))
        kind, pem = None, None
pick = [c for c in certs if "Developer ID Application" in c[1] and c[0]]
if not pick:
    print("sign-mcpb: no Developer ID Application certificate in the p12; it holds: "
          + "; ".join(c[1].strip() for c in certs), file=sys.stderr)
    sys.exit(1)
kid, _, pem = pick[0]
key = [k for k in keys if k[0] == kid]
if not key:
    print("sign-mcpb: the Developer ID Application certificate has no key in the p12", file=sys.stderr)
    sys.exit(1)
open(sys.argv[2], "w").write(pem)
open(sys.argv[3], "w").write(key[0][1])
PY
    then
      cert="$work/cert.pem" key="$work/key.pem"
    fi
    rm -f "$work/all.pem"
  else
    echo "::warning::sign-mcpb: no OpenSSL here could read the Developer ID p12 (CERT_P12 / CERT_PASSWORD)"
  fi
  rm -f "$work/id.p12"
  if [ -n "$cert" ]; then
    issuer="$(openssl x509 -in "$cert" -noout -issuer 2>/dev/null || true)"
    for ca in DeveloperIDG2CA DeveloperIDCA; do
      if curl -fsSL --retry 3 "https://www.apple.com/certificateauthority/${ca}.cer" -o "$work/$ca.cer" \
        && openssl x509 -inform der -in "$work/$ca.cer" -out "$work/$ca.pem" 2>/dev/null; then
        subject="$(openssl x509 -in "$work/$ca.pem" -noout -subject | sed 's/^subject=//')"
        case "$issuer" in *"${subject#*CN=}"*) chain="$work/$ca.pem" ;; esac
      fi
    done
    echo "sign-mcpb: with $(openssl x509 -in "$cert" -noout -subject | sed 's/.*CN *= *\([^,]*\).*/\1/')${chain:+, chained by $(basename "$chain" .pem)}"
  fi
fi

if [ -z "$cert" ] || [ -z "$key" ]; then
  echo "::warning::sign-mcpb: no certificate to sign with (MCPB_SIGN_CERT / MCPB_SIGN_KEY, or the Developer ID p12); Claude Desktop installs the files and shows them as unsigned."
  [ "$require" = 1 ] && exit 1
  exit 0
fi

chmod 600 "$key"
extra=()
[ -n "$chain" ] && extra=(-i "$chain")
failed=0
for f in "$@"; do
  cp "$f" "$work/unsigned.mcpb"
  "${MCPB[@]}" sign "$f" -c "$cert" -k "$key" ${extra[@]+"${extra[@]}"}
  if "${MCPB[@]}" verify "$f" 2>&1 | tee "$work/verify.txt" | grep -qx "Signature is valid"; then
    echo "signed: $(basename "$f")"
  else
    cat "$work/verify.txt"
    echo "::warning::$(basename "$f"): the signature does not verify against this computer's trusted roots, so it ships unsigned."
    cp "$work/unsigned.mcpb" "$f"
    failed=1
  fi
done
rm -f "$key"
[ "$require" = 1 ] && [ "$failed" = 1 ] && exit 1
exit 0

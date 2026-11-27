#!/usr/bin/env python3
"""Check a .mcpb file's signature the way `mcpb verify` means to (IC-1005).

    python3 scripts/verify-mcpb.py FILE.mcpb [--allow-untrusted]

`mcpb verify` (2.1.2, the newest) cannot say yes to any file: it calls
node-forge's pkcs7 verify(), which throws "PKCS#7 signature verification
not yet implemented", and turns that into "unsigned" (K-1387; upstream
mcpb PR #212, closed unmerged). So signing could never be checked.

This does both halves with tools that work:
  1. the signature: the block mcpb sign appends (MCPB_SIG_V1, a 4-byte
     little-endian length, a detached PKCS#7 over every byte before it,
     MCPB_SIG_END) is cut out and `openssl cms -verify` checks it against
     those bytes, digest and all;
  2. the certificate: the chain the signature carries is checked by the
     computer's own trust, as mcpb does -- `security verify-cert -p codeSign`
     on a Mac, X509Chain with the code-signing policy and online revocation
     on Windows, `openssl verify -purpose codesigning` elsewhere.

Prints "Signature is valid" and the publisher, and exits 0, only when both
hold. --allow-untrusted checks only the signature (for tests with a
self-signed certificate).
"""

import os
import subprocess
import sys
import tempfile

HEADER = b"MCPB_SIG_V1"
FOOTER = b"MCPB_SIG_END"


def split(data):
    """(signed bytes, PKCS#7 DER) or None when there is no signature block."""
    end = data.rfind(FOOTER)
    if end == -1:
        return None
    start = data.rfind(HEADER, 0, end)
    if start == -1:
        return None
    at = start + len(HEADER)
    length = int.from_bytes(data[at:at + 4], "little")
    sig = data[at + 4:at + 4 + length]
    if len(sig) != length or at + 4 + length != end:
        return None
    return data[:start], sig


def run(args, **kw):
    return subprocess.run(args, capture_output=True, text=True, **kw)


def chain_trusted(chain_pem):
    if sys.platform == "darwin":
        return run(["security", "verify-cert", "-c", chain_pem, "-p", "codeSign"]).returncode == 0
    if sys.platform == "win32":
        # The signer as DER and the rest as PKCS#7: forms every .NET reads
        # whole (a PEM file of several certificates is not one of them).
        tmp = os.path.dirname(chain_pem)
        leaf, extra = os.path.join(tmp, "leaf.cer"), os.path.join(tmp, "extra.p7b")
        first = open(chain_pem).read()
        first = first[:first.index("-----END CERTIFICATE-----") + len("-----END CERTIFICATE-----")] + "\n"
        run(["openssl", "x509", "-outform", "DER", "-out", leaf], input=first)
        run(["openssl", "crl2pkcs7", "-nocrl", "-certfile", chain_pem, "-outform", "DER", "-out", extra])
        ps = f"""
$ErrorActionPreference = 'Stop'
$leaf = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2('{leaf}')
$c = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2Collection
$c.Import('{extra}')
$chain = New-Object System.Security.Cryptography.X509Certificates.X509Chain
$chain.ChainPolicy.RevocationMode = 'Online'
$chain.ChainPolicy.RevocationFlag = 'EntireChain'
$chain.ChainPolicy.UrlRetrievalTimeout = New-TimeSpan -Seconds 30
[void]$chain.ChainPolicy.ApplicationPolicy.Add((New-Object System.Security.Cryptography.Oid '1.3.6.1.5.5.7.3.3'))
foreach ($x in $c) {{ [void]$chain.ChainPolicy.ExtraStore.Add($x) }}
if ($chain.Build($leaf)) {{ 'Valid' }} else {{ $chain.ChainStatus | ForEach-Object {{ Write-Output "$($_.Status): $($_.StatusInformation)" }}; exit 1 }}
"""
        r = run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", ps])
        if r.returncode != 0:
            print(r.stdout.strip() or r.stderr.strip(), file=sys.stderr)
        return r.returncode == 0 and "Valid" in r.stdout
    return run(["openssl", "verify", "-purpose", "codesigning", "-CApath", "/etc/ssl/certs",
                "-untrusted", chain_pem, chain_pem]).returncode == 0


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    allow_untrusted = "--allow-untrusted" in sys.argv
    if len(args) != 1:
        print(__doc__.strip().splitlines()[2].strip(), file=sys.stderr)
        return 2
    data = open(args[0], "rb").read()
    parts = split(data)
    if parts is None:
        print("ERROR: Extension is not signed")
        return 1
    signed, sig = parts
    with tempfile.TemporaryDirectory() as tmp:
        content = os.path.join(tmp, "content.bin")
        der = os.path.join(tmp, "sig.der")
        certs = os.path.join(tmp, "certs.pem")
        open(content, "wb").write(signed)
        open(der, "wb").write(sig)
        # The signature over these exact bytes; the chain is judged below.
        r = run(["openssl", "cms", "-verify", "-binary", "-inform", "DER", "-in", der,
                 "-content", content, "-noverify", "-out", os.devnull])
        if r.returncode != 0:
            print("ERROR: the signature does not match the file: " + r.stderr.strip().splitlines()[0]
                  if r.stderr.strip() else "ERROR: the signature does not match the file")
            return 1
        r = run(["openssl", "pkcs7", "-inform", "DER", "-in", der, "-print_certs", "-out", certs])
        if r.returncode != 0 or "BEGIN CERTIFICATE" not in open(certs).read():
            print("ERROR: the signature carries no certificate")
            return 1
        # PEM blocks only, signer first, as mcpb writes them.
        text = open(certs).read()
        blocks = []
        while "-----BEGIN CERTIFICATE-----" in text:
            a = text.index("-----BEGIN CERTIFICATE-----")
            b = text.index("-----END CERTIFICATE-----", a) + len("-----END CERTIFICATE-----")
            blocks.append(text[a:b] + "\n")
            text = text[b:]
        open(certs, "w").write("".join(blocks))
        subject = run(["openssl", "x509", "-noout", "-subject"], input=blocks[0]).stdout.strip()
        if not allow_untrusted and not chain_trusted(certs):
            print(f"ERROR: the certificate is not trusted by this computer ({subject})")
            return 1
    print("Signature is valid")
    print(f"Publisher: {subject.split('CN = ')[-1].split(',')[0] if 'CN = ' in subject else subject}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

# Hub admission fixtures written by something other than our own packer

`duplicate-source-path.krate` names `source/lib0.rs` twice. The `zip` crate's
writer refuses to emit a duplicate name, so this cannot be built with our own
tools -- it was assembled with Python's `zipfile` (two distinct names, one
byte-patched to equal the other in both the local header and the central
directory), which is exactly what a hostile publisher does. Four local
headers, four central records, an EOCD honestly declaring four; the opener
catches it because a parser folds the two same-named entries into three while
the file declares four (see K-278 / K-713).

Regenerate with:

    python3 - <<'PY'
    import zipfile, io
    MAN = (b'[app]\nid = "com.example.hub"\nname = "Hub"\n'
           b'version = "1.0.0"\nentry = "code.wasm"\n'
           b'world = "krate:app/cli@0.1.0"\n')
    WASM = bytes([0,0x61,0x73,0x6d,0x0d,0,0x01,0])
    buf = io.BytesIO(); z = zipfile.ZipFile(buf, 'w')
    z.writestr('manifest.toml', MAN); z.writestr('code.wasm', WASM)
    z.writestr('source/lib0.rs', b'the reviewed copy')
    z.writestr('source/lib1.rs', b'the attacker copy')
    z.close()
    data = bytearray(buf.getvalue()).replace(b'source/lib1.rs', b'source/lib0.rs')
    open('duplicate-source-path.krate', 'wb').write(bytes(data))
    PY

## forged-size-bomb.krate

A compression bomb whose source entries DECLARE ~1 byte each and each contain
32 MiB of zeros -- 384 MiB expanded, ~393 KB on the wire. It declares small,
so the declared-size preflight lets it through; it is refused by the guard
that counts bytes actually WRITTEN during extraction (K-255). An honest bomb
(one that declares its true size) is a different fixture, caught earlier at
the preflight -- this one exercises the written-bytes path specifically.

Regenerate with the script recorded in K-255's board entry, then forge every
`source/` entry's uncompressed-size field to 1 in both the local header
(offset 22) and the central-directory record (offset 24).

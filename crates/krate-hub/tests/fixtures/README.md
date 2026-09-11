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

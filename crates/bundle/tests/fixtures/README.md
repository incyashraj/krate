# Fixtures written by something other than our own packer

`duplicate-from-another-writer.krate` was produced by Python's `zipfile`,
which is not the library Krate packs with. It matters because the Rust `zip`
crate REFUSES to write the same entry name twice, so our own writer cannot
build the archive an attacker (or a careless tool) can. Python writes it
without complaint.

It is a well-formed archive in every other way -- four local headers, four
central-directory records, and an EOCD that honestly declares four. So it is
caught by the duplicate-name check rather than by the record-count check,
which is a different path through the opener than the hand-assembled fixture
in `lib.rs` exercises.

Regenerate with:

    python3 - <<'PY'
    import zipfile
    MAN = (b'[app]\nid = "com.example.dup"\nname = "Dup"\n'
           b'version = "1.0.0"\nentry = "code.wasm"\n'
           b'world = "krate:app/cli@0.1.0"\n')
    z = zipfile.ZipFile('duplicate-from-another-writer.krate', 'w', zipfile.ZIP_DEFLATED)
    z.writestr('manifest.toml', MAN)
    z.writestr('code.wasm', b'\0asm\x01\0\0\0')
    z.writestr('source/lib.rs', b'// the reviewed copy')
    z.writestr('source/lib.rs', b'// the attacker copy')
    z.close()
    PY

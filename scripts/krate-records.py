#!/usr/bin/env python3
"""An independent reader of a .krate's record set (IC-714, test 1477).

The runtime, the signer, the hub's scanner and Studio all resolve a bundle
through one Rust opener (crates/bundle). Test 1477 asks that an INDEPENDENT
reader -- other code, other language, other zip library -- resolves exactly
the same canonical record set, or refuses for the same reason. This is that
reader: Python's `zipfile` and the container profile's rules written out a
second time, from the profile's description rather than from the Rust.
`crates/cli/tests/cli.rs` runs it beside `krate inspect --json` on every
committed fixture and on freshly packed bundles and compares the two.

    krate-records.py APP.krate            # the record set as JSON, exit 0
                                          # or {"refused": why}, exit 1
    krate-records.py --self-test

The rules, in the order the opener applies them (profile 1 and 2):
  - the file is at most 256 MiB; at most 8192 file records
  - every name is ASCII, has no backslash, is at most 180 bytes and at most
    16 deep; two names that fold to the same lower-case string are one
    record twice, and the bundle is refused
  - `krate-profile` is read first: absent means 1, a non-number is damage,
    a number above what this reader knows (2) is a newer format
  - every record has a class: profile, manifest, component, signature,
    derived-from, extensions, asset (assets/), source (source/), sdk
    (sdk/), extension (ext/), or unknown; under profile 2 an unknown
    record is refused, under profile 1 it is ignored
  - a prefixed record's path is relative and clean: no empty, `.` or `..`
    segment, no drive letter
  - `ext/<owner>/<name>/<file>` groups are checked against
    `extensions.json`: every declared group present with the declared size
    and digest; an undeclared group is refused when the bundle declares
    extensions at all, or is profile 2
  - `manifest.toml` and `code.wasm` are present
"""
import hashlib
import io
import json
import re
import sys
import warnings
import zipfile

SCHEMA = "krate.records.v1"
READS_PROFILE = 2
MAX_BUNDLE_BYTES = 256 * 1024 * 1024
MAX_ENTRY_COUNT = 8192
MAX_PATH_DEPTH = 16
MAX_PATH_BYTES = 180
MAX_ENTRY_BYTES = 512 * 1024 * 1024
MAX_ASSET_BYTES = 96 * 1024 * 1024
MAX_TOTAL_ASSET_BYTES = 512 * 1024 * 1024
MAX_ASSET_COUNT = 4096
MAX_TOTAL_SOURCE_BYTES = 256 * 1024 * 1024
EXTENSIONS_SCHEMA = "krate.bundle.extensions.v1"
EXTENSION_DIGEST_SCHEMA = "krate.bundle.extension.v1"
LABEL = re.compile(r"^[a-z0-9][a-z0-9._-]{0,63}$")

FIXED = {
    "krate-profile": "profile",
    "manifest.toml": "manifest",
    "code.wasm": "component",
    "signature.json": "signature",
    "derived-from.json": "derived-from",
    "extensions.json": "extensions",
}
PREFIXED = [("assets/", "asset"), ("source/", "source"), ("sdk/", "sdk"), ("ext/", "extension")]


class Refused(Exception):
    pass


def record_class(name):
    if name in FIXED:
        return FIXED[name]
    for prefix, cls in PREFIXED:
        if name.startswith(prefix):
            return cls
    return "unknown"


def clean_relative(name, prefix):
    rest = name[len(prefix):]
    segments = rest.split("/")
    for seg in segments:
        if seg in ("", ".", ".."):
            raise Refused(f"{name}: an empty or dot segment cannot be a path inside {prefix}")
        if ":" in seg:
            raise Refused(f"{name}: a drive letter cannot be a path inside {prefix}")
    return rest


def extension_group(name):
    rest = name[len("ext/"):]
    parts = rest.split("/", 2)
    if len(parts) < 3 or not LABEL.match(parts[0]) or not LABEL.match(parts[1]) or not parts[2]:
        raise Refused(f"{name}: an extension entry is ext/<owner>/<name>/<file>")
    return parts[0] + "/" + parts[1]


def extension_digest(entries):
    h = hashlib.sha256()
    h.update(EXTENSION_DIGEST_SCHEMA.encode())
    h.update(b"\0")
    for name in sorted(entries):
        inner = hashlib.sha256(entries[name]).hexdigest()
        h.update(len(name.encode()).to_bytes(8, "little"))
        h.update(name.encode())
        h.update(inner.encode())
    return h.hexdigest()


def resolve(data):
    if len(data) > MAX_BUNDLE_BYTES:
        raise Refused(f"the file is {len(data)} bytes, more than {MAX_BUNDLE_BYTES}")
    try:
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            zf = zipfile.ZipFile(io.BytesIO(data))
            infos = zf.infolist()
    except zipfile.BadZipFile as err:
        raise Refused(f"not a zip archive: {err}") from None
    seen = set()
    files = []
    for info in infos:
        name = info.filename
        if name.endswith("/"):
            continue
        files.append(info)
        if len(files) > MAX_ENTRY_COUNT:
            raise Refused(f"more than {MAX_ENTRY_COUNT} records")
        if any(ord(c) > 127 for c in name):
            raise Refused(f"{name!r}: not an ASCII name")
        if "\\" in name:
            raise Refused(f"{name}: a backslash in a name")
        if len(name.encode()) > MAX_PATH_BYTES:
            raise Refused(f"{name[:40]}...: a name longer than {MAX_PATH_BYTES} bytes")
        if name.count("/") > MAX_PATH_DEPTH:
            raise Refused(f"{name}: deeper than {MAX_PATH_DEPTH}")
        folded = name.lower()
        if folded in seen:
            raise Refused(f"{name}: two records fold to one path")
        seen.add(folded)

    def read(info):
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            return zf.read(info)

    def measure(info, ceiling):
        """Bytes that actually come out of the decompressor, read to the
        ceiling plus one: the header's size is the archive's claim."""
        total = 0
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            try:
                with zf.open(info) as f:
                    while total <= ceiling:
                        chunk = f.read(1 << 20)
                        if not chunk:
                            break
                        total += len(chunk)
            except (zipfile.BadZipFile, NotImplementedError, RuntimeError) as err:
                raise Refused(f"{info.filename}: cannot be read: {err}") from None
        if total > ceiling:
            raise Refused(f"{info.filename}: expands past its ceiling of {ceiling} bytes")
        return total

    by_name = {info.filename: info for info in files}
    profile = 1
    if "krate-profile" in by_name:
        first = read(by_name["krate-profile"]).decode("utf-8", "replace").split("\n")[0].strip()
        if not first.isdigit():
            raise Refused(f"the profile line is damaged: {first!r}")
        profile = int(first)
        if profile > READS_PROFILE:
            raise Refused(f"a newer format (profile {profile}) than this reader knows ({READS_PROFILE})")

    records = []
    for info in files:
        cls = record_class(info.filename)
        records.append({"name": info.filename, "class": cls, "bytes": info.file_size})
    records.sort(key=lambda r: r["name"])
    if profile >= 2:
        for r in records:
            if r["class"] == "unknown":
                raise Refused(f"{r['name']}: a record profile {profile} does not name")
    declared_source = 0
    asset_count = 0
    for r in records:
        for prefix, cls in PREFIXED:
            if r["class"] == cls:
                clean_relative(r["name"], prefix)
        if r["class"] in ("source", "sdk", "extension"):
            declared_source += r["bytes"]
            if declared_source > MAX_TOTAL_SOURCE_BYTES:
                raise Refused(f"source, SDK and extensions declare more than {MAX_TOTAL_SOURCE_BYTES} bytes")
        if r["class"] == "asset":
            asset_count += 1
            if asset_count > MAX_ASSET_COUNT:
                raise Refused(f"more than {MAX_ASSET_COUNT} assets")
    # What actually comes out, under the same ceilings the opener applies.
    asset_bytes = source_bytes = 0
    for r in records:
        info = by_name[r["name"]]
        if r["class"] == "asset":
            asset_bytes += measure(info, MAX_ASSET_BYTES)
            if asset_bytes > MAX_TOTAL_ASSET_BYTES:
                raise Refused(f"assets expand to more than {MAX_TOTAL_ASSET_BYTES} bytes")
        elif r["class"] in ("source", "sdk", "extension"):
            source_bytes += measure(info, MAX_ENTRY_BYTES)
            if source_bytes > MAX_TOTAL_SOURCE_BYTES:
                raise Refused(f"source, SDK and extensions expand to more than {MAX_TOTAL_SOURCE_BYTES} bytes")
        elif r["class"] != "unknown":
            measure(info, MAX_ENTRY_BYTES)

    groups = {}
    for r in records:
        if r["class"] == "extension":
            key = extension_group(r["name"])
            groups.setdefault(key, {})[r["name"]] = read(by_name[r["name"]])
    declared = None
    if "extensions.json" in by_name:
        try:
            declared = json.loads(read(by_name["extensions.json"]))
        except ValueError as err:
            raise Refused(f"extensions.json is damaged: {err}") from None
        if not isinstance(declared, dict) or declared.get("schema") != EXTENSIONS_SCHEMA:
            raise Refused("extensions.json uses a format this reader does not know")
    extensions = []
    if declared is None:
        if profile >= 2 and groups:
            raise Refused(f"{sorted(groups)[0]}: an extension nobody declared")
    else:
        seen_groups = set()
        for record in declared.get("extensions", []):
            key = f"{record.get('owner')}/{record.get('name')}"
            if not LABEL.match(str(record.get("owner"))) or not LABEL.match(str(record.get("name"))):
                raise Refused(f"extensions.json: {key!r} is not a valid owner/name pair")
            if key in seen_groups:
                raise Refused(f"extensions.json: {key} declared twice")
            seen_groups.add(key)
            if key not in groups:
                raise Refused(f"{key}: declared but carries no entries")
            size = sum(len(b) for b in groups[key].values())
            if size != record.get("size"):
                raise Refused(f"{key}: declares {record.get('size')} bytes and carries {size}")
            if extension_digest(groups[key]) != record.get("digest"):
                raise Refused(f"{key}: contents do not match the declared digest")
            extensions.append({k: record[k] for k in ("owner", "name", "version", "size", "digest")})
        for key in sorted(groups):
            if key not in seen_groups:
                raise Refused(f"{key}: an extension nobody declared")

    for required in ("manifest.toml", "code.wasm"):
        if required not in by_name:
            raise Refused(f"missing its `{required}` entry")
    return {"schema": SCHEMA, "profile": profile, "records": records, "extensions": extensions}


# ----------------------------------------------------------------- self-test
def _zip(entries, dupe=None):
    buf = io.BytesIO()
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as zf:
            for name, body in entries:
                zf.writestr(name, body)
            if dupe:
                zf.writestr(dupe[0], dupe[1])
    return buf.getvalue()


MANIFEST = b'[app]\nid = "com.example.demo"\nname = "Demo"\nversion = "0.1.0"\nentry = "code.wasm"\n'
CORE = [("krate-profile", b"1"), ("manifest.toml", MANIFEST), ("code.wasm", b"\0asm\x01\0\0\0")]


def self_test():
    failures = []

    def check(name, cond, detail=""):
        if not cond:
            failures.append(f"{name}: {detail}" if detail else name)

    def refused(entries, **kw):
        try:
            resolve(_zip(entries, **kw))
            return None
        except Refused as err:
            return str(err)

    out = resolve(_zip(CORE + [("assets/a.png", b"x"), ("source/lib.rs", b"y"), ("notes.txt", b"z")]))
    check("profile 1 lists every record with its class",
          [(r["name"], r["class"]) for r in out["records"]] ==
          [("assets/a.png", "asset"), ("code.wasm", "component"), ("krate-profile", "profile"),
           ("manifest.toml", "manifest"), ("notes.txt", "unknown"), ("source/lib.rs", "source")], str(out))
    check("records carry the declared size", [r["bytes"] for r in out["records"]][0] == 1, str(out))
    check("no profile line is generation 1", resolve(_zip(CORE[1:]))["profile"] == 1)
    check("profile 2 refuses an unknown record",
          "notes.txt" in (refused([("krate-profile", b"2")] + CORE[1:] + [("notes.txt", b"z")]) or ""))
    check("profile 2 without strays opens", resolve(_zip([("krate-profile", b"2")] + CORE[1:]))["profile"] == 2)
    check("a newer profile is refused", "newer" in (refused([("krate-profile", b"3")] + CORE[1:]) or ""))
    check("a damaged profile is refused", "damaged" in (refused([("krate-profile", b"banana")] + CORE[1:]) or ""))
    check("a duplicate name is refused", "fold" in (refused(CORE, dupe=("manifest.toml", b"other")) or ""))
    check("a case-folded duplicate is refused", "fold" in (refused(CORE + [("Manifest.toml", b"o")]) or ""))
    check("a backslash is refused", "backslash" in (refused(CORE + [("assets\\a.png", b"x")]) or ""))
    check("a traversal is refused", "dot segment" in (refused(CORE + [("assets/../x", b"x")]) or ""))
    check("a missing component is refused", "code.wasm" in (refused(CORE[:2]) or ""))
    group = {"ext/acme/plugins/a.toml": b"[a]\n", "ext/acme/plugins/nested/b.bin": bytes(range(256))}
    decl = {"schema": EXTENSIONS_SCHEMA, "extensions": [{"owner": "acme", "name": "plugins", "version": "3",
            "size": 260, "digest": extension_digest(group)}]}
    ext_entries = CORE + [("extensions.json", json.dumps(decl).encode())] + sorted(group.items())
    out = resolve(_zip(ext_entries))
    check("a declared extension is listed", out["extensions"] == decl["extensions"], str(out))
    check("its entries are class extension", all(r["class"] == "extension" for r in out["records"] if r["name"].startswith("ext/")))
    tampered = [(n, (b"[b]\n" if n == "ext/acme/plugins/a.toml" else b)) for n, b in ext_entries]
    check("a tampered extension is refused", "digest" in (refused(tampered) or ""))
    bigger = [(n, (b"[aa]\n" if n == "ext/acme/plugins/a.toml" else b)) for n, b in ext_entries]
    check("a size change is refused", "declares 260 bytes and carries 261" in (refused(bigger) or ""))
    check("an undeclared group beside a declaration is refused",
          "nobody declared" in (refused(ext_entries + [("ext/evil/thing/x", b"s")]) or ""))
    check("an undeclared group under profile 1 with no declaration is ignored",
          resolve(_zip(CORE + [("ext/evil/thing/x", b"s")]))["extensions"] == [])
    check("an undeclared group under profile 2 is refused",
          "nobody declared" in (refused([("krate-profile", b"2")] + CORE[1:] + [("ext/evil/thing/x", b"s")]) or ""))
    check("a declared group with no entries is refused",
          "no entries" in (refused(CORE + [("extensions.json", json.dumps(decl).encode())]) or ""))
    check("a malformed extension path is refused",
          "ext/<owner>" in (refused(ext_entries + [("ext/acme/plugins", b"x")]) or ""))
    check("a damaged declaration is refused", "damaged" in (refused(CORE + [("extensions.json", b"{")]) or ""))
    check("the digest is the documented shape",
          extension_digest({"a": b""}) == hashlib.sha256(
              EXTENSION_DIGEST_SCHEMA.encode() + b"\0" + (1).to_bytes(8, "little") + b"a"
              + hashlib.sha256(b"").hexdigest().encode()).hexdigest())
    if failures:
        print("krate-records self-test FAILED:\n")
        for f in failures:
            print(f"  - {f}")
        return 1
    print("krate-records self-test OK -- profile 1 and 2, every class, folded duplicates, backslashes, "
          "traversal, declared/undeclared/tampered/missing extensions, damaged declarations, the digest shape")
    return 0


def main(argv):
    if "--self-test" in argv:
        return self_test()
    if len(argv) != 1:
        print(__doc__)
        return 2
    with open(argv[0], "rb") as f:
        data = f.read()
    try:
        print(json.dumps(resolve(data), indent=1))
        return 0
    except Refused as err:
        print(json.dumps({"schema": SCHEMA, "refused": str(err)}, indent=1))
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

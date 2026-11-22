#!/usr/bin/env python3
"""An isolated Obsidian profile that opens one vault on one note, with no
dialog, and the checks that it stayed isolated (IC-999, PROTOCOL.md 4).

  obsidian-profile.py prepare --user-data DIR --vault DIR --note REL
                              [--width 1280 --height 800]
      Registers the vault in DIR/obsidian.json (open: true), writes the
      per-vault window file, and, when the vault has no layout yet, writes
      <vault>/.obsidian/workspace.json: Obsidian 1.15.1's own first-launch
      layout (seen 2026-11-22) with the main pane's empty tab replaced by
      the target note. A warm profile keeps the layout Obsidian saved.

  obsidian-profile.py own-stamp
      The modification time of the person's own Obsidian folder, or
      "absent" -- read with stat only, never opened.

  obsidian-profile.py check --user-data DIR --own-stamp STAMP
      JSON: is the profile in use (obsidian.log and id written), did the
      updater drop a newer obsidian-*.asar into it, and is the person's own
      folder untouched. Exit 1 when any is wrong.

  obsidian-profile.py --self-test
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import tempfile
import time
from pathlib import Path

OWN = Path.home() / "Library" / "Application Support" / "obsidian"


def leaf(i: str, typ: str, state: dict, icon: str, title: str) -> dict:
    return {"id": i, "type": "leaf", "state": {"type": typ, "state": state, "icon": icon, "title": title}}


def default_layout(note: str) -> dict:
    """Obsidian 1.15.1's first-launch workspace.json (left: files, search,
    bookmarks; right, collapsed: backlinks, outgoing links, tags, all properties,
    outline),
    with the main leaf opening `note`. Ids are fixed so the file is the same
    on every run."""
    name = Path(note).stem
    main_leaf = "b0000000000000a1"
    return {
        "main": {
            "id": "b000000000000001",
            "type": "split",
            "children": [{
                "id": "b000000000000002",
                "type": "tabs",
                "children": [leaf(main_leaf, "markdown", {"file": note, "mode": "source", "source": False},
                                  "lucide-file", name)],
            }],
            "direction": "vertical",
        },
        "left": {
            "id": "b000000000000003",
            "type": "split",
            "children": [{
                "id": "b000000000000004",
                "type": "tabs",
                "children": [
                    leaf("b0000000000000a2", "file-explorer", {"sortOrder": "alphabetical", "autoReveal": False,
                                                               "showSearch": False, "searchQuery": ""},
                         "lucide-folder-closed", "Files"),
                    leaf("b0000000000000a3", "search", {"query": "", "matchingCase": False, "explainSearch": False,
                                                        "collapseAll": False, "extraContext": False,
                                                        "sortOrder": "alphabetical"}, "lucide-search", "Search"),
                    leaf("b0000000000000a4", "bookmarks", {"showSearch": False, "searchQuery": ""},
                         "lucide-bookmark", "Bookmarks"),
                ],
            }],
            "direction": "horizontal",
            "width": 300,
        },
        "right": {
            "id": "b000000000000005",
            "type": "split",
            "children": [{
                "id": "b000000000000006",
                "type": "tabs",
                "children": [
                    leaf("b0000000000000a5", "backlink", {"collapseAll": False, "extraContext": False,
                                                          "sortOrder": "alphabetical", "showSearch": False,
                                                          "searchQuery": "", "backlinkCollapsed": False,
                                                          "unlinkedCollapsed": True}, "links-coming-in", "Backlinks"),
                    leaf("b0000000000000a6", "outgoing-link", {"linksCollapsed": False, "unlinkedCollapsed": True},
                         "links-going-out", "Outgoing links"),
                    leaf("b0000000000000a7", "tag", {"sortOrder": "frequency", "useHierarchy": True,
                                                     "showSearch": False, "searchQuery": ""}, "lucide-tags", "Tags"),
                    leaf("b0000000000000a8", "all-properties", {"sortOrder": "frequency", "showSearch": False,
                                                                "searchQuery": ""}, "lucide-archive", "All properties"),
                    leaf("b0000000000000a9", "outline", {"followCursor": False, "showSearch": False,
                                                         "searchQuery": ""}, "lucide-list", "Outline"),
                ],
            }],
            "direction": "horizontal",
            "width": 300,
            "collapsed": True,
        },
        "left-ribbon": {"hiddenItems": {
            "switcher:Open quick switcher": False,
            "graph:Open graph view": False,
            "canvas:Create new canvas": False,
            "daily-notes:Open today's daily note": False,
            "templates:Insert template": False,
            "command-palette:Open command palette": False,
            "bases:Create new base": False,
        }},
        "active": main_leaf,
        "lastOpenFiles": [note],
    }


def vault_id(vault: Path) -> str:
    return hashlib.sha256(str(vault).encode()).hexdigest()[:16]


def prepare(user_data: Path, vault: Path, note: str, width: int, height: int) -> dict:
    vault = vault.resolve()
    if not (vault / note).is_file():
        raise SystemExit(f"target note missing: {vault / note}")
    user_data.mkdir(parents=True, exist_ok=True)
    vid = vault_id(vault)
    reg = user_data / "obsidian.json"
    data = json.loads(reg.read_text()) if reg.exists() else {}
    vaults = data.setdefault("vaults", {})
    # One vault only, open at launch; keep any setting Obsidian added.
    for k in list(vaults):
        if k != vid:
            del vaults[k]
    entry = vaults.setdefault(vid, {"path": str(vault), "ts": int(time.time() * 1000)})
    entry["path"] = str(vault)
    entry["open"] = True
    reg.write_text(json.dumps(data))
    win = user_data / f"{vid}.json"
    state = json.loads(win.read_text()) if win.exists() else {}
    state.update({"width": width, "height": height, "isMaximized": False, "devTools": False})
    for k in ("x", "y"):
        state.pop(k, None)
    win.write_text(json.dumps(state))
    layout = vault / ".obsidian" / "workspace.json"
    wrote_layout = False
    if not layout.exists():
        layout.parent.mkdir(parents=True, exist_ok=True)
        layout.write_text(json.dumps(default_layout(note), indent=2))
        wrote_layout = True
    return {"vault_id": vid, "vault": str(vault), "note": note, "wrote_layout": wrote_layout,
            "window": {"width": width, "height": height}}


def own_stamp() -> str:
    try:
        st = OWN.stat()
    except FileNotFoundError:
        return "absent"
    return f"{st.st_mtime_ns}"


def check(user_data: Path, stamp: str) -> dict:
    problems = []
    for f in ("obsidian.log", "id"):
        if not (user_data / f).exists():
            problems.append(f"profile_unused:{f}_missing")
    updates = sorted(p.name for p in user_data.glob("obsidian-*.asar"))
    if updates:
        problems.append("obsidian_downloaded_update:" + ",".join(updates))
    now = own_stamp()
    if now != stamp:
        problems.append("own_obsidian_folder_changed")
    return {"schema": "krate.benchmark.obsidian-isolation.v1", "user_data": str(user_data),
            "problems": problems, "own_stamp_before": stamp, "own_stamp_after": now, "updates": updates}


def self_test() -> int:
    fails = []
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        vault = root / "vault"
        (vault / "Projects").mkdir(parents=True)
        (vault / "Projects" / "Note 00001.md").write_text("# Note\n")
        ud = root / "ud"
        r = prepare(ud, vault, "Projects/Note 00001.md", 1280, 800)
        reg = json.loads((ud / "obsidian.json").read_text())
        e = reg["vaults"][r["vault_id"]]
        if e["path"] != str(vault.resolve()) or e["open"] is not True or len(r["vault_id"]) != 16:
            fails.append(f"registry: {reg}")
        w = json.loads((ud / f"{r['vault_id']}.json").read_text())
        if w.get("width") != 1280 or w.get("height") != 800 or "x" in w:
            fails.append(f"window file: {w}")
        lay = json.loads((vault / ".obsidian" / "workspace.json").read_text())
        main_leaf = lay["main"]["children"][0]["children"][0]
        if main_leaf["state"]["state"]["file"] != "Projects/Note 00001.md" or lay["active"] != main_leaf["id"]:
            fails.append("the main leaf opens the target note and is active")
        if lay["lastOpenFiles"] != ["Projects/Note 00001.md"] or not lay["right"].get("collapsed"):
            fails.append("lastOpenFiles names the note; right sidebar collapsed as shipped")
        # A warm profile keeps Obsidian's own saved layout.
        (vault / ".obsidian" / "workspace.json").write_text('{"saved": "by obsidian"}')
        if prepare(ud, vault, "Projects/Note 00001.md", 1280, 800)["wrote_layout"]:
            fails.append("an existing layout is left alone")
        c = check(ud, own_stamp())
        if not any(p.startswith("profile_unused") for p in c["problems"]):
            fails.append("a profile Obsidian never wrote to is reported unused")
        (ud / "obsidian.log").write_text("log")
        (ud / "id").write_text("x" * 32)
        if check(ud, own_stamp())["problems"]:
            fails.append(f"a used, clean profile passes: {check(ud, own_stamp())}")
        (ud / "obsidian-1.15.2.asar").write_bytes(b"x")
        if not any("obsidian_downloaded_update" in p for p in check(ud, own_stamp())["problems"]):
            fails.append("a downloaded update is caught")
        if not check(ud, "1")["problems"]:
            fails.append("a changed own-folder stamp is caught")
    if fails:
        for f in fails:
            print("FAIL", f)
        return 1
    print("obsidian-profile self-test OK -- one vault open at launch, window 1280x800, the note in the main pane, a saved layout kept, an unused profile, a downloaded update and a touched own folder each caught")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("action", nargs="?", choices=["prepare", "own-stamp", "check"])
    ap.add_argument("--user-data", type=Path)
    ap.add_argument("--vault", type=Path)
    ap.add_argument("--note")
    ap.add_argument("--width", type=int, default=1280)
    ap.add_argument("--height", type=int, default=800)
    ap.add_argument("--own-stamp")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    if a.action == "prepare":
        print(json.dumps(prepare(a.user_data, a.vault, a.note, a.width, a.height), sort_keys=True))
        return 0
    if a.action == "own-stamp":
        print(own_stamp())
        return 0
    if a.action == "check":
        r = check(a.user_data, a.own_stamp)
        print(json.dumps(r, sort_keys=True))
        return 1 if r["problems"] else 0
    ap.error("name an action")
    return 2


if __name__ == "__main__":
    sys.exit(main())

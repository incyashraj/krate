#!/usr/bin/env python3
"""Write the organisation-policy templates an IT team downloads, from one table.

Krate reads one machine policy (version 1) from the first of these that exists:
the operating system's managed settings (a macOS configuration profile for the
domain tech.krate.policy, or the Windows registry under
HKLM\\SOFTWARE\\Policies\\Krate), else a policy.toml file. Every source carries
the same keys. This script is the one place those keys are listed, and it
writes every template from that list, so the Windows template cannot offer a
key the macOS profile lacks, or name it differently:

    docs/landing/deploy/policy.toml                   sample file, every key
    docs/landing/deploy/tech.krate.policy.plist       the same settings as a plist
                                                      (Jamf, Intune "preference file")
    docs/landing/deploy/krate-policy.mobileconfig     a configuration profile
    docs/landing/deploy/windows/krate.admx            Group Policy / Intune template
    docs/landing/deploy/windows/en-US/krate.adml      its English text
    docs/landing/deploy/windows/krate-policy-example.reg
                                                      the sample as registry values

    python3 scripts/build-policy-templates.py           write them
    python3 scripts/build-policy-templates.py --check   fail if any differs from this table
    python3 scripts/build-policy-templates.py --self-test
    python3 scripts/build-policy-templates.py --org-pack DIR --name Acme --slug acme --root Acme=<64 hex>
                                                        one organisation's IT pack, as krate.tech makes it

How each kind of value is carried (P1 of the fleet contract):
    TOML table         plist dictionary     registry subkey
    list of strings    plist array          REG_MULTI_SZ
    boolean            <true/> / <false/>   REG_DWORD 1 / 0
    integer            <integer>            REG_DWORD
    string             <string>             REG_SZ
    trust.roots        array of {name,key}  subkey trust\\roots, one REG_SZ per
                                            root: value name = the root's name,
                                            data = its key

The sample root key is a placeholder that is NOT 64 hex characters, on
purpose: a sample deployed without replacing it refuses to open apps (a
malformed key fails closed) instead of trusting a key nobody chose.
"""

import argparse
import json
import pathlib
import plistlib
import sys
import tempfile
import uuid
import xml.etree.ElementTree as ET

ROOT = pathlib.Path(__file__).resolve().parents[1]
OUT = ROOT / "docs/landing/deploy"
DOMAIN = "tech.krate.policy"
REG = r"Software\Policies\Krate"
PLACEHOLDER_KEY = "REPLACE-WITH-YOUR-ROOT-PUBLIC-KEY-64-HEX-CHARACTERS"
PROVIDERS = ["claude", "codex", "gemini", "copilot", "grok"]

# Every key of policy version 1, in the order the contract (P2) lists them.
# kind: string | bool | list | enum | roots | int
# sample: the value every template shows. None: shown commented out in the
# TOML and left out of the other templates (the per-OS default applies).
KEYS = [
    dict(table="", key="version", kind="int", sample=1,
         title="Policy version",
         help="Required. Always 1 for this version of the policy. An unknown version refuses to open apps."),
    dict(table="", key="name", kind="string", sample="Acme Labs",
         title="Organisation name",
         help="Your organisation's name. Every message Krate shows about the policy uses it, for example \"Acme Labs does not allow this app\". Optional."),
    dict(table="org", key="slug", kind="string", sample="acme-labs",
         title="Organisation on Krate",
         help="The organisation on Krate's hub that these computers belong to: its short name, as in krate.tech/org/acme-labs. Krate shows that organisation's apps to the people signed in to it. Optional."),
    dict(table="trust", key="roots", kind="roots",
         sample=[{"name": "Acme Labs", "key": PLACEHOLDER_KEY}],
         title="Trusted signing roots",
         help="The signing roots this computer trusts: each a name and the public half of an Ed25519 root key, as 64 hex characters. An app passes when its signature verifies and its root (the delegation's root, or the signing key itself) is listed. Empty or absent: no limit on roots."),
    dict(table="trust", key="allow_unsigned", kind="bool", sample=False,
         title="Allow unsigned apps",
         help="Whether an app with no signature may open. Default: allowed when there is no [trust] section, refused when roots are listed. A signature that is present but damaged never passes, whatever this says."),
    dict(table="trust", key="allow_other_signed", kind="bool", sample=False,
         title="Allow apps signed by other roots",
         help="Whether an app signed by a root that is not listed may open. Default: allowed when no roots are listed, refused when they are."),
    dict(table="capabilities", key="deny", kind="list",
         sample=["camera.capture", "audio.capture", "location.read"],
         title="Blocked capabilities",
         help="Capabilities no app may be given on this computer, spelled as an app's manifest spells them (camera.capture, audio.capture, location.read, ui.clipboard, fs.write ...). Matched on the name before ':', so fs.write blocks every fs.write grant. A blocked capability is shown as blocked by your organisation and cannot be granted."),
    dict(table="network", key="allow", kind="list",
         sample=["*.acme.com:443", "api.github.com:443"],
         title="Network ceiling",
         help="The only hosts any app may connect to, whatever its manifest asks for. Same patterns as a manifest: host or *.host, then :port or :*. Absent: no ceiling."),
    dict(table="network", key="proxy", kind="string", sample=None, example="http://proxy.acme.com:3128",
         title="Proxy",
         help="The proxy every connection Krate makes goes through, for http and https alike, as http://host:port (user:password@ allowed), or \"direct\" to use none. When set, it replaces the proxy variables (HTTPS_PROXY, HTTP_PROXY, ALL_PROXY, NO_PROXY) and the system's own proxy settings, so a person's shell cannot route around it; absent, the variables apply when set and the system's static proxy otherwise. This computer's own services (localhost) are always reached directly. Krate does not run a PAC script or WPAD; if your computers rely on one, set the proxy here."),
    dict(table="network", key="no_proxy", kind="string", sample=None, example="internal.acme.com,git.acme.com",
         title="Hosts reached without the proxy",
         help="Used with proxy: hosts reached directly, separated by commas, as NO_PROXY is written: an entry covers the host and every name under it (acme.com covers git.acme.com), * covers every host. Address ranges such as 10.0.0.0/8 are not read; name the hosts."),
    dict(table="usage", key="counting", kind="bool", sample=False,
         title="Send usage counts",
         help="Whether Krate sends its anonymous usage counts (install, make, open, publish) to Krate's hub. false: none are sent and the first-run question about them is not asked."),
    dict(table="hub", key="url", kind="string", sample="https://hub.krate.tech",
         title="Hub",
         help="The only Krate hub this computer talks to for private apps, publishing and sign-in. Any other hub, from a flag or an environment variable, is refused."),
    dict(table="studio", key="ai", kind="bool", sample=True,
         title="Allow AI builds",
         help="false: Krate Studio and krate create refuse to start an AI build, before any AI provider is contacted."),
    dict(table="studio", key="providers", kind="list", sample=["claude", "codex"],
         title="Allowed AI providers",
         help="When present, only these AI providers may be used for a build. Provider ids: " + ", ".join(PROVIDERS) + "."),
    dict(table="updates", key="mode", kind="enum", options=["auto", "notify", "off"], sample="notify",
         title="Updates",
         help="auto (the default): updates are offered as usual. notify: Krate checks for a new version and says so, and never installs one. off: Krate never checks."),
    dict(table="audit", key="path", kind="string", sample=None,
         title="Open record location",
         help="Where Krate appends its record of every app opened or refused on this computer, one JSON line each. Default: /var/log/krate/opens.jsonl on macOS and Linux, %ProgramData%\\Krate\\logs\\opens.jsonl on Windows.",
         example="/var/log/krate/opens.jsonl"),
    dict(table="runtime", key="interpreter", kind="bool", sample=False,
         title="Run apps on the interpreter",
         help="true: every app runs on Krate's interpreter (Pulley) instead of compiled machine code, so Krate never makes executable memory. For endpoint protection that blocks code generated at run time, and Windows Arbitrary Code Guard. Apps may run slower this way."),
]

TABLES = ["org", "trust", "capabilities", "network", "usage", "hub", "studio", "updates", "audit", "runtime"]

# Device-side data rules (G17): [apps."*"] for every organisation's private
# app on the computer, [apps."<app id>"] for one app. Each key narrows when
# set to `narrows`; the other value is Krate's default. Shown commented out
# in the TOML sample (the sample policy sets none of them), offered in the
# Windows template for every organisation app (apps\*), and read the same
# way from a profile or the registry (apps\<id> subkeys).
APP_RULES = [
    dict(key="clipboard_read", narrows=False, example=True,
         title="Organisation apps: no reading the clipboard",
         help="false: the app cannot read the clipboard, and a paste the person makes into it is refused too."),
    dict(key="clipboard_write", narrows=False, example=False,
         title="Organisation apps: no copying out",
         help="false: nothing leaves the app by the clipboard."),
    dict(key="printing", narrows=False, example=False,
         title="Organisation apps: no printing",
         help="false: the app cannot print, or save as PDF from the print dialog."),
    dict(key="screen_capture", narrows=False, example=False,
         title="Organisation apps: kept out of screenshots",
         help="false: the app's windows are kept out of screenshots, screen recordings and screen sharing where the system allows it (macOS and Windows; not Linux), and Krate takes no picture of the app itself."),
    dict(key="wipe_when_offline_ends", narrows=True, example=True,
         title="Organisation apps: delete team data when offline access ends",
         help="true: when the app's offline access runs out, the app's team data on this computer is deleted at the next open."),
]
APP_RULES_EXAMPLE_ID = "dev.acme.payroll"

# The keys an organisation's own IT pack sets (IC-958 B4). krate.tech makes
# the same three files in the browser with docs/landing/kit/policy.js, and
# docs/landing/studio/org-policy-test.mjs proves the two write the same bytes.
ORG_PACK = {("", "version"), ("", "name"), ("org", "slug"), ("trust", "roots"), ("hub", "url")}


def clean(text):
    """A name as it may appear in a comment or a plist: no control characters."""
    return "".join(" " if ord(c) < 0x20 or ord(c) == 0x7F else c for c in text)


def org_keys(name, slug, hub, roots):
    """KEYS for one organisation: its name, short name, hub and roots set,
    every other key left to Krate's default (shown commented out in the TOML,
    absent from the other files)."""
    values = {("", "version"): 1, ("", "name"): clean(name), ("org", "slug"): slug,
              ("trust", "roots"): [{"name": clean(r["name"]), "key": r["key"]} for r in roots], ("hub", "url"): hub}
    out = []
    for k in KEYS:
        k = dict(k)
        at = (k["table"], k["key"])
        if at in ORG_PACK:
            k["sample"] = values[at]
        else:
            k.setdefault("example", k["sample"])
            k["sample"] = None
        out.append(k)
    return out


def sample_dict(keys=None):
    """The sample as nested dictionaries, the shape of a plist and of TOML."""
    d = {}
    for k in keys or KEYS:
        if k["sample"] is None:
            continue
        target = d if not k["table"] else d.setdefault(k["table"], {})
        target[k["key"]] = k["sample"]
    return d


# ---------------------------------------------------------------- TOML

def toml_value(v):
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, int):
        return str(v)
    if isinstance(v, str):
        return '"' + v.replace("\\", "\\\\").replace('"', '\\"') + '"'
    if isinstance(v, list):
        return "[" + ", ".join(toml_value(x) for x in v) + "]"
    raise TypeError(v)


def wrap(text, width=76, prefix="# "):
    words, lines, line = text.split(), [], ""
    for w in words:
        if line and len(line) + 1 + len(w) > width - len(prefix):
            lines.append(prefix + line)
            line = w
        else:
            line = f"{line} {w}" if line else w
    if line:
        lines.append(prefix + line)
    return "\n".join(lines)


def build_toml(keys=None, org=None):
    keys = keys or KEYS
    out = [
        f"# Krate organisation policy, version 1, for {clean(org)}." if org else "# Krate organisation policy, version 1. A sample with every key.",
        "#",
        "# Place it at:",
        "#   Linux    /etc/krate/policy.toml",
        "#   macOS    /Library/Application Support/Krate/policy.toml",
        "#   Windows  %ProgramData%\\Krate\\policy.toml",
        "# A configuration profile (macOS) or Group Policy (Windows) carrying the",
        "# same keys takes precedence over the file. `krate policy` shows which",
        "# one is in force.",
        "#",
        "# A policy only narrows what Krate allows. A key Krate does not know, or",
        "# a value it cannot read, refuses to open apps until it is fixed.",
        "# Generated by scripts/build-policy-templates.py; https://krate.tech/docs/running-in-an-organisation.html",
        "",
    ]
    current = None
    for k in keys:
        if k["table"] != current and k["table"]:
            current = k["table"]
            # A table whose every key is left to its default is shown
            # commented out, so the sample stays the same policy everywhere.
            shown = any(x["sample"] is not None for x in keys if x["table"] == current)
            out.append(f"[{current}]" if shown else f"# [{current}]")
        out.append(wrap(k["help"]))
        if k["kind"] == "roots":
            out.append("roots = [")
            for r in k["sample"]:
                out.append(f'  {{ name = {toml_value(r["name"])}, key = {toml_value(r["key"])} }},')
            out.append("]")
        elif k["sample"] is None:
            out.append(f'# {k["key"]} = {toml_value(k["example"])}')
        else:
            out.append(f'{k["key"]} = {toml_value(k["sample"])}')
        out.append("")
    if org is None:
        out += app_rules_toml()
    return "\n".join(out).rstrip() + "\n"


def app_rules_toml():
    """The [apps] section, commented out: what every organisation app, or one
    app by its id, may not do on these computers (G17)."""
    out = [wrap("Device-side data rules for your organisation's own apps: [apps.\"*\"] for every one of them, or [apps.\"<app id>\"] for one app. Each only narrows; a key left out keeps Krate's default. Uncomment to use."),
           '# [apps."*"]']
    for r in APP_RULES:
        out.append(wrap(r["help"]))
        out.append(f'# {r["key"]} = {toml_value(r["narrows"])}')
    out += [f'# [apps."{APP_RULES_EXAMPLE_ID}"]', "# clipboard_write = false", ""]
    return out


# ---------------------------------------------------------------- plist / profile

def build_plist():
    return plistlib.dumps(sample_dict(), sort_keys=False).decode()


# Fixed identifiers, so the generated file does not change on every run. An
# MDM re-signs and re-identifies a profile when it is uploaded.
PROFILE_UUID = str(uuid.uuid5(uuid.NAMESPACE_URL, "https://krate.tech/deploy/krate-policy.mobileconfig"))
PAYLOAD_UUID = str(uuid.uuid5(uuid.NAMESPACE_URL, "https://krate.tech/deploy/krate-policy.mobileconfig#tech.krate.policy"))


def build_mobileconfig(keys=None, org=None, slug=None):
    # com.apple.ManagedClient.preferences with Forced settings: macOS writes
    # exactly mcx_preference_settings into
    # /Library/Managed Preferences/tech.krate.policy.plist, nothing else.
    # One organisation's profile has its own fixed identifiers, from its
    # short name, so a second download is the same profile, not a new one.
    at = f"https://krate.tech/org/{slug}/krate-policy.mobileconfig" if slug else "https://krate.tech/deploy/krate-policy.mobileconfig"
    profile = {
        "PayloadContent": [{
            "PayloadContent": {DOMAIN: {"Forced": [{"mcx_preference_settings": sample_dict(keys)}]}},
            "PayloadDisplayName": "Krate organisation policy",
            "PayloadIdentifier": f"{DOMAIN}.settings",
            "PayloadType": "com.apple.ManagedClient.preferences",
            "PayloadUUID": str(uuid.uuid5(uuid.NAMESPACE_URL, at + "#" + DOMAIN)) if slug else PAYLOAD_UUID,
            "PayloadVersion": 1,
        }],
        "PayloadDescription": f"Sets the Krate organisation policy for {clean(org)} (tech.krate.policy, version 1)." if org
        else "Sets the Krate organisation policy (tech.krate.policy, version 1). Replace the sample root key before deploying.",
        "PayloadDisplayName": f"Krate policy for {clean(org)}" if org else "Krate organisation policy",
        "PayloadIdentifier": f"{DOMAIN}.profile",
        "PayloadOrganization": clean(org) if org else "Krate",
        "PayloadScope": "System",
        "PayloadType": "Configuration",
        "PayloadUUID": str(uuid.uuid5(uuid.NAMESPACE_URL, at)) if slug else PROFILE_UUID,
        "PayloadVersion": 1,
    }
    return plistlib.dumps(profile, sort_keys=False).decode()


# ---------------------------------------------------------------- ADMX / ADML

NS = "http://schemas.microsoft.com/GroupPolicy/2006/07/PolicyDefinitions"
CATEGORY = {"": "Krate", "org": "Krate_Krate", "trust": "Krate_Trust", "capabilities": "Krate_Apps", "network": "Krate_Apps",
            "runtime": "Krate_Apps", "usage": "Krate_Krate", "hub": "Krate_Krate", "updates": "Krate_Krate",
            "audit": "Krate_Krate", "studio": "Krate_Studio", "org": "Krate_Studio"}
CATEGORY_TEXT = {"Krate": "Krate", "Krate_Trust": "Which apps may open",
                 "Krate_Apps": "What apps may do", "Krate_Krate": "Krate itself",
                 "Krate_Data": "Organisation apps' data on this computer",
                 "Krate_Studio": "Krate Studio and AI builds"}


def policy_id(k):
    return "".join(p.capitalize() for p in (k["table"] + "_" + k["key"]).strip("_").split("_"))


def reg_key(k):
    return REG if not k["table"] else REG + "\\" + k["table"]


def xml_escape(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace('"', "&quot;")


def version_item():
    return f'<item key="{REG}" valueName="version"><value><decimal value="1" /></value></item>'


def build_admx():
    L = ['<?xml version="1.0" encoding="utf-8"?>',
         "<!-- Krate organisation policy, version 1. Generated by scripts/build-policy-templates.py. -->",
         f'<policyDefinitions xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" revision="1.0" schemaVersion="1.0" xmlns="{NS}">',
         "  <policyNamespaces>",
         '    <target prefix="krate" namespace="Krate.Policies.Krate" />',
         "  </policyNamespaces>",
         '  <resources minRequiredRevision="1.0" />',
         "  <supportedOn>",
         "    <definitions>",
         '      <definition name="SUPPORTED_KratePolicy1" displayName="$(string.SUPPORTED_KratePolicy1)" />',
         "    </definitions>",
         "  </supportedOn>",
         "  <categories>",
         '    <category name="Krate" displayName="$(string.Krate)" />']
    for cat in ["Krate_Trust", "Krate_Apps", "Krate_Data", "Krate_Krate", "Krate_Studio"]:
        L.append(f'    <category name="{cat}" displayName="$(string.{cat})"><parentCategory ref="Krate" /></category>')
    L += ["  </categories>", "  <policies>"]
    for k in KEYS:
        if k["key"] == "version":
            continue  # written by every policy below, never on its own
        pid = policy_id(k)
        attrs = f'name="{pid}" class="Machine" displayName="$(string.{pid})" explainText="$(string.{pid}_Help)" key="{reg_key(k)}"'
        body = [f'      <parentCategory ref="{CATEGORY[k["table"]]}" />',
                '      <supportedOn ref="SUPPORTED_KratePolicy1" />']
        # Every policy also writes version = 1, so a computer that is given
        # any one setting has a complete, readable policy. Krate refuses a
        # policy without a version, and Group Policy has no way to make one
        # setting depend on another.
        if k["kind"] == "bool":
            attrs += f' valueName="{k["key"]}"'
            body += ['      <enabledValue><decimal value="1" /></enabledValue>',
                     '      <disabledValue><decimal value="0" /></disabledValue>',
                     f"      <enabledList>{version_item()}</enabledList>",
                     f"      <disabledList>{version_item()}</disabledList>"]
        else:
            attrs += f' presentation="$(presentation.{pid})"'
            body.append(f"      <enabledList>{version_item()}</enabledList>")
            body.append("      <elements>")
            if k["kind"] == "string":
                body.append(f'        <text id="{k["key"]}" valueName="{k["key"]}" required="true" maxLength="1023" />')
            elif k["kind"] == "list":
                body.append(f'        <multiText id="{k["key"]}" valueName="{k["key"]}" required="true" maxLength="1023" />')
            elif k["kind"] == "roots":
                body.append(f'        <list id="roots" key="{REG}\\trust\\roots" explicitValue="true" />')
            elif k["kind"] == "enum":
                body.append(f'        <enum id="{k["key"]}" valueName="{k["key"]}" required="true">')
                for opt in k["options"]:
                    body.append(f'          <item displayName="$(string.{pid}_{opt})"><value><string>{opt}</string></value></item>')
                body.append("        </enum>")
            body.append("      </elements>")
        L.append(f"    <policy {attrs}>")
        L += body
        L.append("    </policy>")
    for r in APP_RULES:
        pid = app_rule_id(r)
        enabled, disabled = (1, 0) if r["narrows"] else (0, 1)
        L.append(f'    <policy name="{pid}" class="Machine" displayName="$(string.{pid})" explainText="$(string.{pid}_Help)" key="{APP_RULES_KEY}" valueName="{r["key"]}">')
        L += ['      <parentCategory ref="Krate_Data" />',
              '      <supportedOn ref="SUPPORTED_KratePolicy1" />',
              f'      <enabledValue><decimal value="{enabled}" /></enabledValue>',
              f'      <disabledValue><decimal value="{disabled}" /></disabledValue>',
              f"      <enabledList>{version_item()}</enabledList>",
              f"      <disabledList>{version_item()}</disabledList>",
              "    </policy>"]
    L += ["  </policies>", "</policyDefinitions>"]
    return "\n".join(L) + "\n"


APP_RULES_KEY = REG + "\\apps\\*"


def app_rule_id(r):
    return "AppsAll" + "".join(p.capitalize() for p in r["key"].split("_"))


def admx_help(k):
    extra = {
        "roots": " Enter each root on its own row: the name in the first column (Value name) and the 64 hex characters in the second (Value).",
        "deny": " One capability per line.",
        "allow": " One host pattern per line.",
        "providers": " One provider id per line.",
    }.get(k["key"], "")
    if k["kind"] == "bool":
        extra += " Enabled sets it to true, Disabled to false, Not configured leaves Krate's default."
    where = "HKLM\\" + reg_key(k).replace("Software", "SOFTWARE", 1)
    if k["kind"] == "roots":
        return k["help"] + extra + f" Registry: one value per root in {where}\\roots."
    return k["help"] + extra + f" Registry: the value {k['key']} in {where}."


def build_adml():
    S = [("Krate", "Krate"), ("SUPPORTED_KratePolicy1", "Krate with organisation policy version 1")]
    S += [(c, t) for c, t in CATEGORY_TEXT.items() if c != "Krate"]
    P = []
    for k in KEYS:
        if k["key"] == "version":
            continue
        pid = policy_id(k)
        S.append((pid, k["title"]))
        S.append((pid + "_Help", admx_help(k)))
        if k["kind"] == "enum":
            S += [(f"{pid}_{o}", o) for o in k["options"]]
        if k["kind"] == "string":
            P.append(f'      <presentation id="{pid}"><textBox refId="{k["key"]}"><label>{xml_escape(k["title"])}</label></textBox></presentation>')
        elif k["kind"] == "list":
            P.append(f'      <presentation id="{pid}"><text>{xml_escape(k["title"])}, one per line:</text><multiTextBox refId="{k["key"]}" defaultHeight="6" /></presentation>')
        elif k["kind"] == "roots":
            P.append(f'      <presentation id="{pid}"><listBox refId="roots">{xml_escape(k["title"])}</listBox></presentation>')
        elif k["kind"] == "enum":
            P.append(f'      <presentation id="{pid}"><dropdownList refId="{k["key"]}" noSort="true" defaultItem="0">{xml_escape(k["title"])}</dropdownList></presentation>')
    for r in APP_RULES:
        pid = app_rule_id(r)
        S.append((pid, r["title"]))
        state = "Enabled sets it, Disabled or Not configured leaves Krate's default."
        S.append((pid + "_Help", r["help"].replace("false: ", "").replace("true: ", "")
                  + f" For every organisation app on this computer. {state} One app only: the same value in HKLM\\SOFTWARE\\Policies\\Krate\\apps\\<app id>, or [apps.\"<app id>\"] in policy.toml. Registry: the value {r['key']} in HKLM\\SOFTWARE\\Policies\\Krate\\apps\\*."))
    L = ['<?xml version="1.0" encoding="utf-8"?>',
         "<!-- Krate organisation policy, version 1. Generated by scripts/build-policy-templates.py. -->",
         f'<policyDefinitionResources xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" revision="1.0" schemaVersion="1.0" xmlns="{NS}">',
         "  <displayName>Krate</displayName>",
         "  <description>Krate organisation policy, version 1.</description>",
         "  <resources>",
         "    <stringTable>"]
    L += [f'      <string id="{i}">{xml_escape(t)}</string>' for i, t in S]
    L += ["    </stringTable>", "    <presentationTable>"] + P + ["    </presentationTable>", "  </resources>",
                                                                "</policyDefinitionResources>"]
    return "\n".join(L) + "\n"


# ---------------------------------------------------------------- .reg

def reg_multi_sz(values):
    data = "".join(v + "\0" for v in values) + "\0"
    raw = data.encode("utf-16-le")
    return "hex(7):" + ",".join(f"{b:02x}" for b in raw)


def reg_string(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def build_reg(policy_keys=None, org=None):
    keys = {}
    for k in policy_keys or KEYS:
        if k["sample"] is None:
            continue
        key = "HKEY_LOCAL_MACHINE\\" + reg_key(k).replace("Software", "SOFTWARE", 1)
        v = k["sample"]
        if k["kind"] == "roots":
            sub = keys.setdefault(key + "\\roots", [])
            for r in v:
                sub.append(f'{reg_string(r["name"])}={reg_string(r["key"])}')
            keys.setdefault(key, [])
            continue
        line = reg_string(k["key"]) + "="
        if k["kind"] in ("bool", "int"):
            line += f"dword:{int(v):08x}"
        elif k["kind"] == "list":
            line += reg_multi_sz(v)
        else:
            line += reg_string(v)
        keys.setdefault(key, []).append(line)
    out = ["Windows Registry Editor Version 5.00", "",
           f"; Krate organisation policy, version 1, for {clean(org)}, as registry values." if org
           else "; Krate organisation policy, version 1: the sample policy as registry values.",
           "; For a test computer. Fleets use the ADMX template (krate.admx) instead.",
           "; Generated by scripts/build-policy-templates.py." if org
           else "; Replace the sample root key before importing. Generated by scripts/build-policy-templates.py.", ""]
    for key in sorted(keys, key=lambda x: (x.count("\\"), x)):
        out.append(f"[{key}]")
        out += keys[key]
        out.append("")
    if org is None:
        # The device-side data rules (G17), commented out as in policy.toml.
        out.append("; Device-side data rules for every organisation app (remove the ; to use):")
        out.append(";[HKEY_LOCAL_MACHINE\\SOFTWARE\\Policies\\Krate\\apps\\*]")
        for r in APP_RULES:
            out.append(f';"{r["key"]}"=dword:{int(r["narrows"]):08x}')
        out.append("")
    # regedit reads UTF-16 with a byte-order mark; a UTF-8 file imports wrongly.
    return "\r\n".join(out)


def build_keys_json():
    """The table itself, for docs/landing/kit/policy.js: krate.tech builds an
    organisation's pack in the browser from these same keys and words."""
    return json.dumps({"keys": KEYS, "org_pack": sorted([t, k] for t, k in ORG_PACK),
                       "app_rules": APP_RULES, "app_rules_example_id": APP_RULES_EXAMPLE_ID},
                      indent=1, ensure_ascii=False) + "\n"


FILES = {
    "policy-keys.json": build_keys_json,
    "policy.toml": build_toml,
    "tech.krate.policy.plist": build_plist,
    "krate-policy.mobileconfig": build_mobileconfig,
    "windows/krate.admx": build_admx,
    "windows/en-US/krate.adml": build_adml,
    "windows/krate-policy-example.reg": build_reg,
}


def org_pack(name, slug, hub, roots):
    """One organisation's IT pack: {file name: bytes}, the same three files
    krate.tech's dashboard makes in the browser (docs/landing/kit/policy.js)."""
    keys = org_keys(name, slug, hub, roots)
    return {
        "policy.toml": encode("policy.toml", build_toml(keys, name)),
        f"krate-policy-{slug}.mobileconfig": encode(".mobileconfig", build_mobileconfig(keys, name, slug)),
        f"krate-policy-{slug}.reg": encode(".reg", build_reg(keys, name)),
    }


def encode(name, text):
    if name.endswith(".reg"):
        return "﻿".encode("utf-16-le") + text.encode("utf-16-le")
    return text.encode("utf-8")


def write(out=OUT):
    for name, fn in FILES.items():
        p = out / name
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(encode(name, fn()))
        print(f"wrote {p.relative_to(ROOT) if ROOT in p.parents else p}")
    return 0


def check(out=OUT):
    bad = 0
    for name, fn in FILES.items():
        p = out / name
        if not p.is_file() or p.read_bytes() != encode(name, fn()):
            print(f"{p}: differs from scripts/build-policy-templates.py; run it to rewrite", file=sys.stderr)
            bad += 1
    print(f"policy templates: {len(FILES)} file(s), {bad} out of date")
    return 1 if bad else 0


def structure_problems(admx, adml):
    """What an ADMX/ADML pair would be refused for: references that do not resolve."""
    a = ET.fromstring(admx)
    b = ET.fromstring(adml)
    q = lambda t: f"{{{NS}}}{t}"
    strings = {s.get("id") for s in b.iter(q("string"))}
    presentations = {p.get("id"): p for p in b.iter(q("presentation"))}
    categories = {c.get("name") for c in a.iter(q("category"))}
    supported = {d.get("name") for d in a.iter(q("definition"))}
    out = []
    for el in a.iter():
        for attr in ("displayName", "explainText"):
            v = el.get(attr)
            if v and v.startswith("$(string.") and v[9:-1] not in strings:
                out.append(f"{el.tag}: {attr} {v} has no string")
    for pol in a.iter(q("policy")):
        name = pol.get("name")
        for ref in pol.iter(q("parentCategory")):
            if ref.get("ref") not in categories:
                out.append(f"{name}: category {ref.get('ref')} missing")
        for ref in pol.iter(q("supportedOn")):
            if ref.get("ref") not in supported:
                out.append(f"{name}: supportedOn {ref.get('ref')} missing")
        elements = pol.find(q("elements"))
        pres = pol.get("presentation")
        if elements is not None:
            if not pres:
                out.append(f"{name}: has elements and no presentation")
                continue
            p = presentations.get(pres[len("$(presentation."):-1])
            if p is None:
                out.append(f"{name}: presentation {pres} missing")
                continue
            refs = {c.get("refId") for c in p}
            for e in elements:
                if e.get("id") not in refs:
                    out.append(f"{name}: element {e.get('id')} is not presented")
    names = [p.get("name") for p in a.iter(q("policy"))]
    if len(names) != len(set(names)):
        out.append("duplicate policy names")
    return out


def self_test():
    # Every P2 key appears in every template, with the same name.
    want = {(k["table"], k["key"]) for k in KEYS}
    assert ("runtime", "interpreter") in want and ("trust", "roots") in want
    # IC-953: the organisation whose apps come first, in every template.
    assert ("org", "slug") in want
    assert {t for t, _ in want if t} == set(TABLES), "every table is listed"
    toml = build_toml()
    try:
        import tomllib
    except ModuleNotFoundError:  # Python < 3.11
        tomllib = None
    if tomllib:
        parsed = tomllib.loads(toml)
        expect = sample_dict()
        assert parsed == expect, (parsed, expect)
        # The commented-out key is shown, so every key has an example.
        assert "# path = " in toml
    plist = plistlib.loads(build_plist().encode())
    assert plist == sample_dict()
    prof = plistlib.loads(build_mobileconfig().encode())
    settings = prof["PayloadContent"][0]["PayloadContent"][DOMAIN]["Forced"][0]["mcx_preference_settings"]
    assert settings == sample_dict()
    assert isinstance(settings["version"], int) and not isinstance(settings["version"], bool)
    assert settings["trust"]["allow_unsigned"] is False
    admx, adml = build_admx(), build_adml()
    assert structure_problems(admx, adml) == [], structure_problems(admx, adml)
    # Every key but version is a policy, and each writes version = 1.
    a = ET.fromstring(admx)
    pols = list(a.iter(f"{{{NS}}}policy"))
    assert len(pols) == len(KEYS) - 1 + len(APP_RULES)
    # G17: every device-side rule is a policy on apps\*, by its key name.
    for r in APP_RULES:
        assert f'valueName="{r["key"]}"' in admx and APP_RULES_KEY in admx, r["key"]
        assert f'# {r["key"]} = ' in toml, r["key"]
    for p in pols:
        assert 'valueName="version"' in ET.tostring(p, encoding="unicode"), p.get("name")
    # Registry value names are exactly the TOML key names.
    reg = build_reg()
    for k in KEYS:
        if k["sample"] is not None and k["kind"] != "roots":
            assert f'"{k["key"]}"=' in reg, k["key"]
    assert "[HKEY_LOCAL_MACHINE\\SOFTWARE\\Policies\\Krate\\trust\\roots]" in reg
    assert reg_multi_sz(["a"]) == "hex(7):61,00,00,00,00,00"
    # The placeholder fails closed: it is not 64 hex characters.
    assert not all(c in "0123456789abcdef" for c in PLACEHOLDER_KEY.lower()) and len(PLACEHOLDER_KEY) != 64
    # A broken reference is caught.
    assert structure_problems(admx.replace("Krate_Trust\" />", "Krate_Nope\" />", 1), adml)
    assert structure_problems(admx, adml.replace('<presentation id="HubUrl">', '<presentation id="HubUrlX">'))
    # One organisation's pack sets exactly its name, short name, hub and
    # roots, and parses back to that; nothing of the sample comes along.
    pack = org_pack("Acme & Co", "acme", "https://hub.krate.tech", [{"name": "Acme & Co", "key": "ab" * 32}])
    want_pack = {"version": 1, "name": "Acme & Co", "org": {"slug": "acme"},
                 "trust": {"roots": [{"name": "Acme & Co", "key": "ab" * 32}]}, "hub": {"url": "https://hub.krate.tech"}}
    if tomllib:
        assert tomllib.loads(pack["policy.toml"].decode()) == want_pack
    prof = plistlib.loads(pack["krate-policy-acme.mobileconfig"])
    assert prof["PayloadContent"][0]["PayloadContent"][DOMAIN]["Forced"][0]["mcx_preference_settings"] == want_pack
    assert prof["PayloadUUID"] != PROFILE_UUID
    reg = pack["krate-policy-acme.reg"].decode("utf-16")
    assert '[HKEY_LOCAL_MACHINE\\SOFTWARE\\Policies\\Krate\\org]\r\n"slug"="acme"' in reg
    assert "REPLACE-WITH" not in reg and "camera" not in reg
    # --check notices drift.
    with tempfile.TemporaryDirectory() as tmp:
        t = pathlib.Path(tmp)
        write(t)
        assert check(t) == 0
        (t / "policy.toml").write_text("version = 2\n")
        assert check(t) == 1
    print("ok  build-policy-templates self-test")
    return 0


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--org-pack", metavar="DIR", help="write one organisation's IT pack into DIR")
    ap.add_argument("--name", help="with --org-pack: the organisation's name")
    ap.add_argument("--slug", help="with --org-pack: its short name")
    ap.add_argument("--hub", default="https://hub.krate.tech", help="with --org-pack: its hub")
    ap.add_argument("--root", action="append", default=[], metavar="NAME=KEY", help="with --org-pack: a trusted root, repeatable")
    args = ap.parse_args(argv)
    if args.self_test:
        return self_test()
    if args.org_pack:
        roots = [{"name": r.split("=", 1)[0], "key": r.split("=", 1)[1]} for r in args.root]
        out = pathlib.Path(args.org_pack)
        out.mkdir(parents=True, exist_ok=True)
        for name, data in org_pack(args.name or args.slug, args.slug, args.hub, roots).items():
            (out / name).write_bytes(data)
        return 0
    if args.check:
        return check()
    return write()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

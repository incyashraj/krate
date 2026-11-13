# Running Krate in an organisation

This chapter is for the people who look after a company's computers. It
covers installing Krate without anyone clicking, setting a policy that says
which apps may open and what they may do, reading the record of what opened,
working behind a company proxy, and controlling which release of an
organisation's app people get.

Two things to know first:

- **A policy only narrows.** Nothing in it can make Krate allow something it
  would refuse without one.
- **A policy that cannot be read refuses to open apps**, and says why. A typo
  in a security setting never reads as "allowed".

The policy, the open record, corporate network support and release control
arrive in the Krate release after 0.5.5. `krate policy` on a computer says
whether its Krate has them.

For what Krate itself enforces on every computer, policy or not, see the
[security model](https://krate.tech/security/).

## Install Krate silently

Every release on
[GitHub](https://github.com/incyashraj/krate/releases) carries installers
for fleets. Below, `$V` is the version, for example `0.5.6`.

| System | File | Installs |
|---|---|---|
| macOS, Apple silicon | `krate-$V-aarch64-apple-darwin.pkg` | `krate` and its build tool in `/usr/local/bin`, Krate Player in `/Applications` |
| macOS, Intel | `krate-$V-x86_64-apple-darwin.pkg` | the same |
| Debian, Ubuntu | `krate-$V-x86_64-unknown-linux-gnu.deb` (or `aarch64-...`) | `/usr/lib/krate/krate`, linked from `/usr/bin/krate`, and the `.krate` file type |
| Fedora, RHEL | `krate-$V-x86_64-unknown-linux-gnu.rpm` (or `aarch64-...`) | the same |
| Windows | `krate-$V-x86_64-pc-windows-msvc.zip` (or `aarch64-...`) | the files you copy |

None of the packages runs an install script, and none writes a policy.

### macOS

```sh
sudo installer -pkg krate-$V-aarch64-apple-darwin.pkg -target /
```

A package that is signed with Krate's Developer ID Installer certificate is
named as above. A release built without that certificate names its package
`krate-$V-<target>-unsigned.pkg`, so the release page shows which it is.

- **Jamf Pro**: upload the package under Settings, Computer management,
  Packages, and add it to a policy scoped to your computers.
- **Intune**: Apps, macOS, Add, **macOS app (PKG)**. This app type takes
  signed and unsigned packages alike.
- **Kandji**: Library, Add new, Custom App, upload the package, installation
  type "Package".

The Apple silicon package refuses to install on an Intel Mac. The Intel
package also runs on Apple silicon, through Rosetta.

### Linux

```sh
sudo apt install ./krate-$V-x86_64-unknown-linux-gnu.deb
sudo dnf install ./krate-$V-x86_64-unknown-linux-gnu.rpm
```

With Ansible:

```yaml
- name: Install Krate
  ansible.builtin.apt:
    deb: "https://github.com/incyashraj/krate/releases/download/v{{ krate_version }}/krate-{{ krate_version }}-x86_64-unknown-linux-gnu.deb"

- name: Krate organisation policy
  ansible.builtin.copy:
    src: policy.toml
    dest: /etc/krate/policy.toml
    owner: root
    group: root
    mode: "0644"
```

On Fedora or RHEL, `ansible.builtin.dnf` with `name:` set to the `.rpm` URL
does the same.

### Windows

A signed Windows installer (`msiexec` and `winget`) comes once Krate has a
Windows code-signing certificate. Until then, place the files from the zip
with your deployment tool, for example as an Intune Win32 app, running this
PowerShell as the system:

```powershell
$V = "0.5.6"
$name = "krate-$V-x86_64-pc-windows-msvc"
$dest = Join-Path $env:ProgramFiles "Krate"
Expand-Archive -Path ".\$name.zip" -DestinationPath $env:TEMP -Force
New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item "$env:TEMP\$name\*" $dest -Recurse -Force
$path = [Environment]::GetEnvironmentVariable("Path", "Machine")
if (($path -split ";") -notcontains $dest) {
  [Environment]::SetEnvironmentVariable("Path", "$path;$dest", "Machine")
}
```

### Check a download before you deploy it

Every release file is listed in the release's `SHA256SUMS`, and each one has
a build provenance record, signed by GitHub, saying which workflow at which
commit built those exact bytes:

```sh
shasum -a 256 -c SHA256SUMS --ignore-missing
gh attestation verify krate-$V-aarch64-apple-darwin.pkg --repo incyashraj/krate
```

The release also carries a bill of materials (CycloneDX JSON) for each
platform: `krate-$V-<target>.cdx.json` lists every crate the `krate` binary
links, `krate-studio-$V-<target>.cdx.json` the same for Krate Studio, and
`krate-studio-ui-$V.cdx.json` the JavaScript inside Studio's window.

## The organisation policy

### Where Krate reads it

Krate reads the first of these that exists on the computer, and ignores the
rest. `krate policy` names the one in force and any it ignored.

| System | 1. Managed settings | 2. File |
|---|---|---|
| macOS | the configuration profile domain `tech.krate.policy` (`/Library/Managed Preferences/tech.krate.policy.plist`) | `/Library/Application Support/Krate/policy.toml` |
| Windows | the registry, `HKLM\SOFTWARE\Policies\Krate` (Group Policy or Intune) | `%ProgramData%\Krate\policy.toml` |
| Linux | none | `/etc/krate/policy.toml` |

There is no per-person policy, on purpose: a person cannot write a policy
for themselves.

### Every key

This is the sample policy, with every key and what it does. Download it from
[krate.tech/deploy/policy.toml](https://krate.tech/deploy/policy.toml).

```toml
{{#include ../../landing/deploy/policy.toml}}
```

The sample's root key is a placeholder on purpose. Deployed unchanged, it
refuses to open apps rather than trusting a key nobody chose. Put the public
half of your organisation's root key there: the same key you registered
with `krate org root-key`, as 64 hex characters.

What makes a policy refuse to open any app, until it is fixed:

- no `version`, or a version this Krate does not know;
- a key Krate does not know, at the top or inside a table;
- a value it cannot read: a `key` that is not 64 hex characters, a host
  pattern that does not parse, an `updates.mode` other than `auto`,
  `notify` or `off`.

### Which apps may open

A root is the public half of an Ed25519 key, 64 hex characters. Two ways
to get one: for your own organisation, Settings on its page on krate.tech
shows the full root key; for any app, `krate policy check app.krate` on a
computer whose policy does not yet trust its publisher prints that app's
root and the exact line to add. The sample `policy.toml` carries a
placeholder key, and a policy with a placeholder refuses to open any app
until you put a real key in its place.

An app passes `[trust]` when one of these holds:

1. Its signature verifies over its current contents (and any delegation in it
   holds and has not been withdrawn or expired), and its root is in `roots`.
   The root is the delegation's root, or the signing key itself when a root
   signed it directly.
2. `roots` is empty, `allow_unsigned = false` is set, and the app is signed by
   anyone.
3. It is unsigned and `allow_unsigned = true`.
4. It is signed by a root that is not listed and `allow_other_signed = true`.

A signature that is present but damaged, or does not verify, never passes,
whatever the flags say. Trust is checked before anything in the app runs.

The policy is checked at every way an app opens: `krate run`, a
double-click, Krate Studio's Open, `krate launch`, and AI tools through
`krate mcp`. A refusal says that your organisation decided it, what about the
app did not fit, and who to ask, in a window when a window opened the app.

`krate policy check app.krate` answers in advance: it prints the exact words
a person would see, and exits 0 if the app would open and 1 if not.

### Rules for your organisation's own apps

`[apps."*"]` sets what every one of your organisation's private apps may do
on these computers, and `[apps."<app id>"]` sets it for one app (its id as
its manifest spells it, such as `dev.acme.payroll`). Both apply together; a
key left out keeps Krate's default, and each rule only narrows.

```toml
[apps."*"]
clipboard_write = false          # nothing leaves by the clipboard
wipe_when_offline_ends = true    # team data goes when offline access runs out

[apps."dev.acme.payroll"]
clipboard_read = false           # no reading the clipboard, pasting included
printing = false                 # no printing or saving as PDF
screen_capture = false           # kept out of screenshots and recordings
```

- `clipboard_read = false` and `clipboard_write = false` refuse the app's
  clipboard calls. A paste the person makes with Cmd+V or Ctrl+V into a
  field the app draws is refused too.
- `printing = false` refuses the app's print calls: the print dialog does
  not open, so there is no Save as PDF either.
- `screen_capture = false` keeps the app's windows out of screenshots,
  screen recordings and screen sharing on macOS and Windows, which honour
  the request; Linux has no such setting. Krate also takes no picture of
  the app itself (`krate run --shoot`, check pictures). A camera pointed at
  the screen is outside what any software can stop.
- `wipe_when_offline_ends = true` deletes the app's team data on this
  computer when its offline access has run out, at the next open, and writes
  a `team-data-wiped` line to the open record. Writes not yet sent are lost
  with it, which is what the rule is for.

A rule the app needs (a required `ui.clipboard:write`, say) stops the app
opening, with words saying your organisation blocks it. `krate policy`
lists the rules in force; in the Windows template they are the policies
under "Organisation apps' data on this computer", which set them for every
organisation app (`HKLM\SOFTWARE\Policies\Krate\apps\*`); one app's rules go
under `apps\<app id>` beside it.

### Send the policy with your MDM

The same settings come ready for each tool, each a download from krate.tech:

| File | For |
|---|---|
| [`krate-policy.mobileconfig`](https://krate.tech/deploy/krate-policy.mobileconfig) | a macOS configuration profile, ready to upload |
| [`tech.krate.policy.plist`](https://krate.tech/deploy/tech.krate.policy.plist) | the same settings as a plain plist, for MDMs that take a domain and a file |
| [`windows/krate.admx`](https://krate.tech/deploy/windows/krate.admx) and [`windows/en-US/krate.adml`](https://krate.tech/deploy/windows/en-US/krate.adml) | the Group Policy template, for Group Policy and Intune |
| [`windows/krate-policy-example.reg`](https://krate.tech/deploy/windows/krate-policy-example.reg) | the sample as registry values, for one test computer |

All of them carry the sample's placeholder root key. Edit it first.

**macOS, Jamf Pro.** Computers, Configuration Profiles, New, Application &
Custom Settings, Upload: preference domain `tech.krate.policy`, and upload
`tech.krate.policy.plist` after editing it. Or upload the edited
`.mobileconfig` itself. Scope it to computers, not users.

**macOS, Intune.** Devices, macOS, Configuration, Create, Templates,
**Preference file**: preference domain name `tech.krate.policy`, and upload
`tech.krate.policy.plist`. Or Templates, **Custom**, with the
`.mobileconfig` and the device channel.

**macOS, Kandji.** Library, Add new, Custom Profile, and upload the
`.mobileconfig`.

**Windows, Group Policy.** Copy `krate.admx` into your central store
(`\\<domain>\SYSVOL\<domain>\Policies\PolicyDefinitions`) or
`C:\Windows\PolicyDefinitions`, and `en-US\krate.adml` into its `en-US`
folder. The settings are under Computer Configuration, Administrative
Templates, Krate.

**Windows, Intune.** Devices, Windows, Configuration, **Import ADMX**: upload
`krate.admx` and `krate.adml` together. Then create a profile for Windows 10
and later, Templates, **Imported Administrative templates**, and choose the
Krate settings.

Each Group Policy setting writes `version = 1` beside its own value, so a
computer given any one setting has a complete policy.

**How values are stored.** A TOML table is a plist dictionary and a registry
subkey; a list is a plist array and a `REG_MULTI_SZ`; `true`/`false` are
`<true/>`/`<false/>` and a `REG_DWORD` of 1 or 0; `version` is a
`REG_DWORD`. The signing roots are the subkey `trust\roots` with one string
value per root: the value's name is the root's name and its data the
64 hex characters.

**Linux.** Copy `policy.toml` to `/etc/krate/policy.toml`, owned by root,
mode 0644 (the Ansible task above).

## The open record

Krate writes one line of JSON for every app opened, refused, or whose
permissions changed during a run. Nothing is sent anywhere: it is a file on
the computer, for your log collector.

| | Where |
|---|---|
| With a policy, macOS and Linux | `/var/log/krate/opens.jsonl`, or `[audit] path` |
| With a policy, Windows | `%ProgramData%\Krate\logs\opens.jsonl`, or `[audit] path` |
| No policy | `~/.krate/logs/opens.jsonl`, the person's own history |

If the person running Krate cannot write the policy's location, Krate writes
to `~/.krate/logs/opens.jsonl` instead and `krate policy` says so.

Each line:

```json
{"v":1,"at":"2026-10-14T09:12:03Z","event":"opened","app":"com.acme.expenses","name":"Expenses","version":"1.3.0","digest":"sha256:...","signer":"<hex or null>","root":"<hex or null>","org":"acme","door":"window","granted":["net.connect:api.acme.com:443"],"denied":["camera.capture"],"user":"dana","krate":"0.5.6"}
```

- `event` is `opened` or `refused`. (`granted` and `revoked` are kept for a
  permission that changes while an app runs; Krate does not change one
  mid-run today, so they do not appear yet.)
- A `refused` line adds `reason`, a stable code (`policy-unsigned`,
  `policy-other-signer`, `policy-tampered`, `policy-capability`,
  `policy-network`, `policy-unreadable`, `policy-hub`, and the codes for an
  organisation's private apps, such as `offline-expired`), and `words`,
  what the person saw.
- `door` is how it was opened: `window`, `terminal`, `mcp` or `player`.
- It never holds the paths of a person's documents, app data or prompts.
- Past 10 MiB the file is renamed `opens.1.jsonl` and a new one started. One
  old file is kept.

Point your log collector at the file (Splunk Universal Forwarder, Elastic
Agent, Fluent Bit, or any agent that follows a file). One JSON object per
line, UTF-8, so no parsing rules are needed.

## Corporate networks

Every connection Krate makes, for itself and for apps, trusts the
**operating system's certificate store**. A company root certificate that
your MDM installed is trusted with no Krate setting, and a network that
inspects TLS works as it does for a browser.

Krate finds the proxy in this order: the policy's `[network] proxy`; the
proxy variables, if any is set; then the proxy the operating system is set
to use (System Settings on a Mac, Internet Options on Windows, which a
configuration profile or Group Policy usually sets). It does not run an
automatic configuration script (PAC) or auto-discovery (WPAD). If your
computers rely on one, name the proxy in the policy, so it reaches every
computer with the rest of your settings:

```toml
[network]
proxy = "http://proxy.acme.com:3128"     # or "direct"
no_proxy = "internal.acme.com,git.acme.com"
```

With `proxy` set, the variables and the system settings are ignored, so a
person's shell cannot route around it. Without it, the usual variables
apply when set: `HTTPS_PROXY` for https, `HTTP_PROXY` for http,
`ALL_PROXY` for either, and `NO_PROXY`, in capitals or lower case. A
`NO_PROXY` entry covers the host and every name under it; address ranges
such as `10.0.0.0/8` are not read. This computer's own services
(`localhost`, `127.0.0.1`) are always reached directly. Websockets go
through the proxy too, as an HTTP `CONNECT` tunnel.

When a network intercepts connections and its root is not in the store,
Krate says that the network is intercepting connections and to ask IT,
instead of a certificate error.

An app still reaches only the hosts its permissions name, and, with a
policy, only those inside `[network] allow`.

## Release control and groups

For an [organisation](organisations.md)'s own apps, owners and admins
decide which approved release people get, and who gets it.

| Action | owner | admin | developer | reviewer | recipient |
|---|---|---|---|---|---|
| Withdraw a release | yes | yes | | yes | |
| Set the current release | yes | yes | | | |
| Manage groups | yes | yes | | | |
| Choose a release's audience | yes | yes | | | |

**Withdraw** an approved release when it must stop being used:

```sh
krate org release withdraw acme <digest> --reason "wrong tax rates"
```

The reason is required and is shown to people. Downloads and offline-access
renewals for that release stop at once, and a computer refuses it the next
time it checks in. Offline access already given lasts until it runs out, and
the dashboard says exactly how long before you confirm. Withdrawing the
current release makes the newest remaining approved release current. A
withdrawal cannot be undone: to bring the same app back, publish it again as
a new upload.

**Current release**: which approved release people get when they open the
app. Setting an older one is how you roll back:

```sh
krate org release current acme com.acme.expenses <digest>
```

**Groups** are named sets of members, such as `finance`:

```sh
krate org group create acme finance
krate org group add acme finance dana@acme.example
krate org group list acme
```

A release goes to everyone in the organisation unless you give it an
**audience** of groups:

```sh
krate org release audience acme <digest> --groups finance,ops
krate org release audience acme <digest> --everyone
```

Someone outside the audience is told the app is only for those groups and to
ask an admin. Owners, admins and reviewers always pass, so they can review
it. Every one of these changes is in the organisation's audit
(`krate org audit acme`).

The organisation dashboard at [krate.tech/org/](https://krate.tech/org/) does
the same: a Releases view for each app with its current release, history,
withdraw and set current; a Groups card under People; and an audience picker
on each release.

## Your organisation's apps on a new computer

Put your organisation's short name in the policy:

```toml
[org]
slug = "acme"
```

Krate Studio and `krate org apps` then list that organisation's apps first.
On a new computer where nobody has signed in yet, Studio's Home asks the
person to sign in with your company sign-in, and once they have, shows the
apps released to them. It allows nothing extra: what may open is still
decided by the rest of the policy and by each release's audience.

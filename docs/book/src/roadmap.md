# Where Krate is

Updated 7 November 2026. Krate 0.5.4 is the current release, and 0.5.5 is
in pre-release (rc9, 4 November 2026). What changed in each version is in
the [changelog](https://github.com/incyashraj/krate/blob/main/CHANGELOG.md)
and on the [releases page](https://github.com/incyashraj/krate/releases).

## What ships today

**Make an app.** [Krate Studio](https://krate.tech/studio/) runs on Mac,
Windows and Linux, and in the browser at
[krate.tech/app](https://krate.tech/app/): describe the app, watch it get
built, change it, and publish it. Studio's IDE (Beta) opens, edits and
builds a Krate project with your AI, in the 0.5.5 pre-release.

**Send it as a link.** A link opens a Krate app in any browser tab, phones
included, with nothing to install. The app keeps its own data, opens and
saves files, and asks before it uses the camera, microphone or location.

**Run it natively.** The same `.krate` file runs on macOS (Apple silicon
and Intel), Windows (x64 and ARM64) and Linux (x86_64 and arm64). At Krate
0.5.4, 10 ported apps pass the same checks on all three systems in CI
([the records](https://krate.tech/progress/)).

**Publish it.** The hub at [krate.tech/cloud](https://krate.tech/cloud/)
is the gallery: every app gets a page, every developer gets their own page
at `name.krate.tech`, and publishing an update reaches the links people
already have.

**Know who made it.** Publishers sign their apps, delegate release keys and
revoke them; an app changed after signing does not run. Every app declares
what it may touch, and the person opening it sees that list first. The
[security model](https://krate.tech/security/) has the details.

**Use it as a team.** [Organisations](organisations.md) give a company
private apps with approvals, releases and groups, team data encrypted end
to end, company connections the hub cannot read, and company sign-in
(SAML, OpenID Connect and SCIM). The console is live at
[krate.tech/orgs](https://krate.tech/orgs/), and the command line ships in
the 0.5.5 pre-release.

**Bring an app you have (Beta).** `krate port` reads a project and shows
the plan, and Studio's Port door rewrites it as one `.krate`
([porting](porting.md)). The door is in web Studio today and in desktop
Studio in the 0.5.5 pre-release.

**Bring bigger programs.** C and C++ programs run as Krate apps, apps reach
the GPU through `krate:gpu` (wgpu and Bevy draw in a Krate window), and
`krate:process` lets an app run programs on the computer it opens on. Doom
ships as one `.krate`. All of this is in the 0.5.5 pre-release.

## Next

- **Krate's own Players for iPhone and Android, in a release.** Phones
  open Krate apps from a link in the browser today.
- **Web apps inside a `.krate`.** Plain HTML and exported web apps run in
  the same file and behind the same permission card as every other app.
- **The same data on every device.** A signed-in person's apps keep the
  same data on each of their devices, with a switch to turn it off and a
  choice whenever two devices changed the same thing.

## Earlier pages

[Vision](vision.md), [Architecture](architecture.md),
[Build Log](build-log.md) and [Krate for Everyone](progress-for-everyone.md)
were written in July and August 2026, while Krate was being built phase by
phase, and describe the plan as it stood then.

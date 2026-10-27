# Organisations

An organisation is a company's own place on Krate. Its people make apps,
approve them and open them privately. Only the organisation's members and
the people it sends to can open its apps; a copied link alone opens
nothing. The apps' team data and the credentials for the company's APIs are
encrypted on members' computers, so Krate's hub stores them without being
able to read them.

Everything here is done in a browser, in the organisation console at
[krate.tech/orgs](https://krate.tech/orgs/). Nobody types a command or
handles a signing key. Most steps also have a `krate` command for people
who prefer a terminal or want to script it (see [From the command
line](#from-the-command-line)).

Installing Krate on the company's computers, the policy that says which
apps may open there, the record of what opened, company networks, and
release control are in
[Running Krate in an organisation](running-in-an-organisation.md).

## Starting one

Sign in at [krate.tech/orgs](https://krate.tech/orgs/), pick a name, and
you are the organisation's first owner. Three steps follow:

1. **Your first app.** Pick a ready app (Team Notes, Expenses or Stock
   List, each sharing its data across the team, encrypted) or upload a
   `.krate` you have. The console makes the organisation's signing key in
   your browser, signs the app with it, and releases it.
2. **Your team.** Paste work email addresses and choose a role. Krate
   emails each person a link to join, or gives you the link to send another
   way.
3. **Getting started.** A checklist in the console for what comes next:
   company sign-in, the IT pack for the company's computers, Krate Studio,
   and building apps with AI.

The console lives at `krate.tech/org/?o=<name>`. What each person sees
there depends on their role.

## Who can do what

People are known by a GitHub account or a work email. A work email is
proven by signing in with an email link, with Google, or with the company's
own sign-in. An email account is never mistaken for a GitHub account with a
similar name: `alice` on GitHub and `alice@corp.example` are two different
people to an organisation.

Each person has one role:

| Role | What it is for |
|---|---|
| owner | Everything: approving releases, company sign-in, provisioning, billing, and making other owners. |
| admin | Invites and removes developers, reviewers and members, sets up groups and company connections, withdraws releases, reads the activity. Does not approve releases. |
| developer | Makes apps and sends them for approval. |
| reviewer | Approves what developers send. |
| recipient | Opens the organisation's apps, and nothing else. The console calls this role Member. |

Two rules hold whatever your role:

- **Nobody approves their own upload once someone else can.** While the
  owner is the only person who may approve, they release their own apps,
  and each of those releases is marked "released by its sender (no second
  approver yet)". As soon as a second person who may approve joins (another
  owner, or a reviewer), every release needs a second person. An owner can
  turn that rule on from the start in Settings.
- **An organisation always has an owner.** Removing or demoting the last
  owner is refused, even when two people try at the same moment.

## Signing, with no keys to handle

Apps sent through an organisation are signed, so everyone can tell the app
came from the organisation and was not changed on the way. Krate does the
signing itself:

- The organisation's signing key is made on its first owner's computer, or
  in their browser. It is sealed to the computers of the owners and admins
  and to a recovery key, so Krate's hub keeps only sealed copies it cannot
  open.
- A developer's Krate makes its own release key the first time it sends an
  app. An owner's or admin's computer that is online signs the permission
  for it in the background, once the hub says the developer may publish.
  An owner can choose to approve new publishers by hand instead, with one
  click in Settings.
- When someone is removed, or loses the right to publish, their release
  keys are withdrawn: the hub refuses them at once, and the next owner's or
  admin's computer online signs the withdrawal.

Every one of these steps is in the organisation's activity.

## Sending and approving an app

A developer sends an app from Krate Studio's Send menu, "Send to Acme for
approval", or with `krate publish --org`. Krate marks it as the organisation's
private app and signs it. The mark is a small file inside the app, covered
by the signature, so it cannot be removed or changed without breaking the
signature. A marked app cannot be published publicly, and an app without
the mark never contacts the hub when it opens.

What arrives waits for approval and opens for nobody yet. The approver sees
what the app can do, in plain words, and what changed since the release it
replaces: what was added, what was removed and what stayed the same. An
update that asks for something new, such as the network, files or the
camera, is approved only once the approver confirms each new thing.

The approval is for those exact bytes, identified by their digest (a
fingerprint of the file). If the app changes after it was reviewed, the new
upload has a different digest and waits for its own approval. Approving the
old digest never releases the new bytes.

Once approved, the app is released to the organisation, and the hub checks
the person's sign-in against the organisation every time the app is
fetched.

### Approving with the code in view

Every app sent to an organisation carries its source. On the organisation's
page, each waiting version shows what changed in its code since the version
that opens now: every file added, removed or changed, and a line-by-line
diff of each one. Above the files are the things worth a second look,
worked out from the files themselves with no AI: a new network host or
permission, a new or changed dependency in `Cargo.toml` or `Cargo.lock`,
new `unsafe` Rust, a new binary file, how much the compiled app grew, and
whether it was made with AI and which model. Only the people who approve,
and whoever sent the version, can read its code. A very large change is cut
at 400 KB, and the page says where.

The email that says a version is waiting links straight to that version,
with its code and what it can do, and Approve and Decline beside them. It
works on a phone. If someone else has already decided, the page says who
and when.

### Approval rules

An owner sets the organisation's approval rules under Settings, Approval
rules:

- how many people approve each version: one or two, never its sender;
- a stricter count, one or two, for a version that asks for the network,
  files or the camera, or that adds any permission;
- a group one approver must be in, for every app or for one app;
- a group one approver must be in when a version connects to a network
  host the version before it did not.

The hub checks the rules when someone approves. An approval that does not
meet them yet is recorded and the version keeps waiting; the page says what
is still needed, for example "Needs one more approval from security". The
approval that meets them releases the version. An approval that could not
count, from someone outside the group the rules still need, is refused and
says whose approval is needed. Each approval is in the audit.

With the rules left as they are (one approval, no groups), an owner who is
the only person who can approve still releases their own apps, marked as
released by their sender. Any other rules need someone besides the sender,
so they cannot be met by one person alone.

## Opening it offline

The first time a member opens a private app, Krate shows the terms with the
exact date: "Works offline until <date>, then needs a connection. If you are
removed, it stops when it next connects." Each open while online checks in
with the hub and renews that offline access when it is half used. Owners
choose how long an app works offline: 7 days unless they say otherwise, 30
at most, or not at all, in which case it opens only with a connection.

Offline, the app opens only on that stored access, for that app and that
sign-in, until it runs out. When the computer connects again and the
check-in is refused, because the person was removed, the access is deleted
and the app does not open. The person's own data in it is kept.

This works for every way an app is opened: `krate run`, `krate launch`, a
double-click, Krate Studio's Open, and an AI tool through `krate mcp`. Each
says in words why a private app did not open, and how to sign in; on a
double-click on a Mac or Windows, in a window. A marked app whose signature
does not hold is refused too.

The limit to know: this is enforced by Krate on the computer. Someone who
strips the mark and the signature has an unsigned copy of the code, not the
organisation's app, and Krate cannot stop a file from being copied.

## Team data

An organisation's app keeps its shared data in the organisation itself,
with no server to run. The app asks for it in its manifest:

```toml
[[capabilities]]
cap = "store.org"
rationale = "Keep the notes with your team"
```

and reads and writes records with `krate::org` (`put`, `get`, `delete`,
`records`, `sync`, and `who` for the signed-in person and their role). Every
call works offline first, on the person's computer, and Krate syncs in the
background while the app is open. When two people change different fields
of one record at the same time, both changes stand.

**Encrypted end to end.** Records are encrypted on members' computers with
the organisation's key, which only those computers hold. Krate's hub stores
and passes on the encrypted records and cannot read them. What it does see
is which app synced, how many records and how big, when, and who synced.

Only a member who may open the app's release can sync its data. When a
person is removed, the organisation's team data is deleted from their
computer.

## Your company's apps on every computer

Every member sees the apps their organisations released to them, on any
computer with Krate: in Krate Studio's Home under Company apps, refreshed
when it opens and every 15 minutes while it is open, and with
`krate org apps`. Each app shows its version, its size and what it can do
on your computer, in the words the reviewer saw when they approved it. Only
apps for you are listed: an app limited to groups you are not in is not
there, and a withdrawn release is gone.

Opening one opens the release the organisation chose to open now, through
the same checks as any private app: after a rollback it is the older one.
Apps that may be used offline are kept ready on your computer, with their
offline access, so they open at once, and with no connection until that
access runs out.

## Every app's record

Every app `krate create` or Krate Studio makes carries a record of how it
was made: the tool, the AI and model when Krate knows it, when, and a
fingerprint of the request. The request itself goes with it only when the
builder chooses to keep it (`krate create --keep-prompt`, or Studio's
"Keep the request with the app"). The record is inside the app and covered
by its signature, so it cannot be changed afterwards without the signature
failing. `krate inspect app.krate` shows it; an app made by hand has none.

The console's **Apps** page lists every app the organisation has, for
owners, admins and reviewers: its owner, who last sent it, how it was made,
what it can do, the company connections it uses, who it is for, how many
people opened it in the last 30 days (owners and admins also see who), and
whether it is in use, waiting or withdrawn. The list downloads as a
spreadsheet.

Each app has an owner, by default whoever first sent it: the person IT asks
about the app. Owning an app gives no extra permission. When the owner is
removed from the organisation, the app shows "Needs an owner" until an
owner or admin chooses one; both are in the activity.

What the hub counts is only that a person opened a release on a given day,
seen when Krate checks the app in; nothing from inside the app. It keeps
those daily counts for 90 days. The console's Insights page adds them up
for each role, and shows people's names only to owners and admins.

## Company connections

A connection lets the organisation's apps call one of its APIs without any
app holding the API's credential. An owner or admin sets it up once in the
console: a name, the API's address, the credential (one header, or a user
name and password), which methods and paths are allowed (only reading
unless widened), which groups may use it, and which apps.

The credential is never shown again: the console says only which header it
is, who set it and when, and how often it was used. Replacing it gives
every app the new one the next time it opens. Moving a connection to
another address needs its credential again.

**Krate cannot read the credential.** It is encrypted on the computer it is
typed on, in the console or by the CLI, to the organisation's key, and only
the encrypted copy is sent. Krate's hub keeps that copy and hands it to a
member's computer when an allowed app opens; only the computers of the
organisation's members hold the key that opens it. When someone is
removed, the next member's computer online encrypts every credential again
to a new key that the removed person never had.

The exact terms: HPKE (RFC 9180) base mode with X25519, HKDF-SHA256 and
ChaCha20-Poly1305, bound to the organisation, the connection's name, its
header and its address, so a credential moved to another address does not
open. The limit to know: the hub hands the console the organisation's
public key, and decides who is a member, so a hub that lied could ask for a
credential to be encrypted to a key of its own; a computer that already
holds the organisation's key checks the one it is given against its own.

An app asks for it by name in its manifest:

```toml
[[capabilities]]
cap = "net.connection:finance-api"
rationale = "Read invoices"
```

and makes ordinary requests to the connection's address, with no
credential of its own. When the organisation's signed app opens, Krate asks
the hub for the credential for this person and this release, and adds it to
the app's requests to that address, after they leave the app. It is given
only to a member in the connection's groups, for an app it allows, in a
version that asked for it, and not for a withdrawn release. A request with a
method or path the connection does not allow, or offline, is refused in
words and never sent. A redirect is never followed with it, and no request
anywhere else carries it. Each use is counted per day in the activity.

The request goes from the person's computer, so an API on the company
network works. The limit to know: a server that echoes request headers back
would show the credential to the app.

### Telling the API who is calling

One credential serves the whole organisation, so on its own your API sees
only "the app". With **Tell the API who is calling** on (the default, in
the connection's form in Settings, or `--no-identity` on
`krate org connection create` to turn it off), every request an app sends
through the connection also carries a `Krate-Identity` header: a short
token, signed by Krate, that names the person. Your API can log who did
what, with no sign-in work of its own. The credential still decides who
gets in; the token tells you who it was.

The token is a JWT (RFC 7519) signed with EdDSA over Ed25519 (RFC 8037).
Its claims:

| Claim | What it is |
|-------|------------|
| `sub` | The person: `github:<login>` or `email:<address>` |
| `org` | The organisation's short name |
| `app` | The app's id from its manifest, such as `com.acme.ledger` |
| `release` | The SHA-256 of the exact release the person opened |
| `connection` | The connection's name, such as `finance-api` |
| `aud` | The connection's address, exactly as shown in Settings |
| `iss` | `https://hub.krate.tech` |
| `iat`, `nbf`, `exp` | Issued, valid from, valid until: five minutes |
| `jti` | A random id for this token |

To check one: take the key whose `kid` matches the token's header from
`https://hub.krate.tech/.well-known/jwks.json`, and accept the token only if
the signature holds with `alg` `EdDSA`, `aud` is your connection's address,
`iss` is `https://hub.krate.tech`, and `exp` has not passed (allow a minute
for clocks). Krate refreshes the token about a minute before it runs out,
and only for people who still pass every check the credential does: someone
removed from the organisation stops being named within five minutes. If a
fresh token cannot be had, the request still goes, without the header, so
treat a missing header as "unknown person", not as a refusal. Krate drops
any `Krate-Identity` an app sets itself, and never sends one outside the
connection's address.

In Node 18 or later, with the `jose` package:

```js
import { createRemoteJWKSet, jwtVerify } from "jose";

const keys = createRemoteJWKSet(new URL("https://hub.krate.tech/.well-known/jwks.json"));

export async function whoIsCalling(request) {
  const token = request.headers["krate-identity"];
  if (!token) return null; // sent without one: an unknown person
  const { payload } = await jwtVerify(token, keys, {
    algorithms: ["EdDSA"], issuer: "https://hub.krate.tech",
    audience: "https://api.acme.com/finance", clockTolerance: 60,
  });
  return payload; // payload.sub, payload.app, payload.release ...
}
```

## Company sign-in

An owner connects the company's own sign-in in Settings, so people sign in
with their work account. Krate supports OpenID Connect (Google Workspace,
Okta, Microsoft Entra, or any OpenID Connect provider) and SAML 2.0.

First the organisation proves it owns its email domain: Settings gives a
TXT record to add at `_krate-verify.<your domain>`, and Krate reads it.
Nothing about company sign-in works until the domain is proved, and a
person is signed in as an address only when the provider says the address
is verified and it is at that domain.

- **OpenID Connect.** Give the provider's issuer URL, a client id and
  secret. Sign-in uses the authorization code flow with PKCE, and the
  provider's answer is checked against its published keys.
- **SAML 2.0.** Give the provider's entity id, its sign-in URL and its
  signing certificate. Your provider needs Krate's side:

  | | |
  |---|---|
  | Entity id and metadata | `https://hub.krate.tech/saml/<organisation>` |
  | Reply (ACS) URL | `https://hub.krate.tech/login/saml/acs/<organisation>` |

  Krate accepts a response only when its assertion is signed by that
  certificate, it is for this organisation, it is in time, it answers a
  sign-in Krate started, and it has not been used before.

Two choices go with either:

- **Who joins on their own.** People at the domain can join when they first
  sign in, as a developer, reviewer or member. Never as an owner or admin:
  those are always given by a person.
- **Required.** Company sign-in can be the only way in for addresses at the
  domain. Krate keeps at least one owner who signs in another way, so a
  broken company sign-in cannot lock every owner out.

## Provisioning

Provisioning by SCIM 2.0 keeps the organisation's people and groups in step
with the company's directory (Okta, Microsoft Entra, or any SCIM 2.0
client). Set it up in Settings, under Provisioning (SCIM):

| | |
|---|---|
| SCIM base URL | `https://hub.krate.tech/scim/v2/<organisation>/` |
| Authentication | Bearer token, made in Settings |

The token is shown once, when it is made; Krate keeps only its fingerprint.
Revoking it in Settings stops it at the next request.

What the directory's records become:

- **A user** is a member, known by the email address in `userName`. They
  join at once, with no invitation, in the role the owner chose for
  provisioning: member unless set, and never owner or admin.
- **Deactivating or deleting a user** removes them exactly as an owner
  would: their next check-in, download and offline access are refused, and
  they leave every group. The directory brings back only people it took
  away; someone an owner removed in Krate stays removed. The last owner is
  never removed.
- **A group** becomes one of the organisation's groups, so an app released
  to the group `finance` follows the directory's Finance group.

Krate answers Users and Groups (create, read, list, replace, update and
delete), and `filter` with `eq` on `userName`, `externalId`,
`displayName` and `id`. Every change the directory makes is in the
activity, by `scim`.

## Removing someone

An owner or admin removes a person in the console, or the company's
directory does it through provisioning. Their access ends at once: the next
time they open the app, they are told they were removed and nothing runs.
Their own account is untouched. The organisation's releases stay, and so
does the record of who sent and who approved each one.

Anyone can leave an organisation themselves.

## The activity, and sending it to your own logs

Owners and admins see every change and every refused attempt: who did it,
when, and to what. Each event is chained to the one before it, and the hub
checks the whole chain each time. If any event was changed after it was
written, the console and `krate org audit` say so. The activity never holds
app files, source, team data, keys or tokens.

An owner can send the activity to the company's own logs (a SIEM such as
Splunk, Datadog or Sentinel, or anything that takes an HTTP request). In
Settings, give a webhook URL. From then on every event is sent there as
JSON, one per request, in order, with the hash that chains it, so the
receiver can check the chain itself. Each request is signed:

```
X-Krate-Signature: t=<unix seconds>,v1=<hex HMAC-SHA256 of "<t>.<body>">
X-Krate-Event-Id: <organisation>:<sequence>
```

The signing secret is shown once, when it is made. An event the receiver
does not accept is sent again with growing gaps, and given up after 24
hours and counted. "Send a test event" in Settings checks the receiver.

## The IT pack

For the company's computers, the console gives an IT pack once the first
app is released: the organisation's own policy as a configuration profile
for Macs (Jamf, Intune), a registry file for Windows (with the Group Policy
template for Intune or Group Policy), and a `policy.toml` for any computer,
including Linux. Computers with it open the organisation's apps, signed by
the organisation, and nothing signed by anyone else; Krate on them uses the
organisation's hub and lists its apps first. Krate itself installs from
[krate.tech/download](https://krate.tech/download/). Every setting, and how
to deploy it, is in
[Running Krate in an organisation](running-in-an-organisation.md).

## Closing and exporting

An owner can take the organisation's standard export from Settings at any
time: one zip with its people and roles, its activity and its released
apps, and a manifest that lists what is in it and what is left out on
purpose (signing keys, sign-in tokens, secrets and invitation codes).

An owner can close the organisation in Settings. A sole owner confirms by
typing its name; with two or more owners, another owner approves. It closes
on the date the console shows, not at once: until then its apps open,
nothing new can be sent or approved, and any owner can cancel. Former
owners can still take the export for a while after it closes. Billing never
closes an organisation and never stops an owner taking the export.

## When something is refused

A refusal says why and what to do next, for example:

- "you uploaded this candidate, so someone else with review authority has
  to approve it"
- "you were removed from acme -- ask one of its owners if that is a
  mistake"
- "this release asks for more than the one it replaces: confirm each new
  capability to approve it"
- "that would leave acme with no owner -- make someone else an owner first"

## From the command line

Everything people do day to day has a `krate` command. Company sign-in,
provisioning, the activity export, billing, offline settings, and closing
or exporting are in the console only.

Create an organisation and invite people, by GitHub username or by work
email. An invitation works once, only for the person it names, for 7 days:

```sh
krate org create acme --name "Acme"
krate org invite acme bob --role reviewer
krate org invite acme --email dan@acme.example --role recipient
krate org join acme <code>
```

`krate org list` shows your organisations and your role in each;
`krate org show acme` lists who is in it; `krate org role` and
`krate org remove` change and remove people; `krate org cancel-invite`
takes back an invitation nobody has used.

Send an app for approval and approve it:

```sh
krate publish app.krate --org acme
krate org candidates acme
krate org approve acme <digest>
```

`krate publish --org` marks and signs the app with this computer's release
key, making one and asking for its permission the first time; an unsigned
or unmarked app is turned back before anything is uploaded. Approving an
update that asks for something new names each new capability with
`--accept`. `krate org releases acme` lists every release, with who sent
it and who approved it.

Open a private app from its link, or the organisation's apps from the list:

```sh
krate run https://hub.krate.tech/a/<digest>
krate org apps acme --open com.acme.expenses
```

`krate run` sends the person's sign-in with the link, and only to a Krate
hub, never to whatever site a link points at.

The rest: `krate org connection create|replace|remove|list` for company
connections (the credential is read from a file or typed at a prompt that
does not show it), `krate org release` and `krate org group` for release
control and groups, `krate org keys` for the organisation's key, and
`krate org audit acme` for the activity. Keys can still be handled by hand
with `krate org root-key`, `krate delegate` and `krate revoke`.

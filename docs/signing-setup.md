# Signing and notarizing macOS builds

The release workflow signs and notarizes `Krate.app` and the `krate` binary
when five secrets are present, and skips with a warning when they are not. This
is what to create, once.

Until this is done, a downloaded build is ad-hoc signed with no team identity.
Gatekeeper only misses it because `curl` sets no quarantine flag — which means
the install path avoids the guarantee Krate exists to make. A reviewer noticed
in about a minute.

## 1. The right certificate

Check what you already have:

```bash
security find-identity -v -p codesigning
```

**"Apple Development" is not enough.** That is for running your own builds on
your own machines. Distribution needs **Developer ID Application**, which is a
separate certificate from the same paid membership.

Create one at
[developer.apple.com/account/resources/certificates](https://developer.apple.com/account/resources/certificates)
→ **+** → **Developer ID Application**. Download it, double-click to add it to
your keychain, then confirm:

```bash
security find-identity -v -p codesigning | grep "Developer ID Application"
```

## 2. Export it for CI

Keychain Access → **My Certificates** → right-click the Developer ID
Application entry → **Export** → `.p12`, with a password you will use below.

Then base64 it, because a GitHub secret holds text:

```bash
base64 -i ~/Desktop/krate-signing.p12 | pbcopy
```

## 3. An app-specific password

Notarization uses one, never your Apple ID password. Make it at
[appleid.apple.com](https://appleid.apple.com) → **Sign-In and Security** →
**App-Specific Passwords**.

## 4. Your team id

```bash
security find-identity -v -p codesigning | grep "Developer ID Application"
```

The ten-character code in parentheses is the team id.

## 5. Add the five secrets

`github.com/incyashraj/krate/settings/secrets/actions`:

| Secret | Value |
|---|---|
| `APPLE_CERT_P12` | the base64 from step 2 |
| `APPLE_CERT_PASSWORD` | the `.p12` password |
| `APPLE_ID` | your Apple ID email |
| `APPLE_TEAM_ID` | the ten-character team id |
| `APPLE_APP_PASSWORD` | the app-specific password |

## 6. Check it worked

Cut a release, download the zip, and run what a sceptical reviewer would:

```bash
spctl -a -vv /Applications/Krate.app
```

You want `accepted` and `source=Notarized Developer ID`. Anything else means it
is not done, whatever the build log says.

Notarization adds roughly two to five minutes to a macOS release. The ticket is
stapled to the bundle, so the app also opens on a machine that is offline.

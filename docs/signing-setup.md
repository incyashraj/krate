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

```bash
security export -k ~/Library/Keychains/login.keychain-db \
  -t identities -f pkcs12 -o /tmp/krate.p12 -P "your-password"
base64 -i /tmp/krate.p12 | pbcopy
```

That exports every identity with a private key, so the file will also hold an
"Apple Development" certificate if you have one. That is fine: the workflow
picks the Developer ID entry by name. Check it is in there before uploading:

```bash
openssl pkcs12 -in /tmp/krate.p12 -passin pass:your-password \
  -nokeys -legacy | grep -c "BEGIN CERTIFICATE"
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

## If notarization returns "Invalid"

The credentials are fine; the binary is not. Get the reason:

```bash
xcrun notarytool log <submission-id> --apple-id <email> \
  --team-id <team> --password <app-specific>
```

The one that caught us: `Krate.app/Contents/MacOS/` holds **two**
executables — the `Krate` launcher script and the `krate-cli` binary it
execs. Signing only the second left the first ad-hoc, and the notary service
checks every executable in that folder, not just the one named in the plist.
It reports this as "The binary is not signed with a valid Developer ID
certificate", which sounds like a certificate problem and is not.

Check for a stray ad-hoc signature before uploading anything:

```bash
for exe in Krate.app/Contents/MacOS/*; do
  codesign -dvvv "$exe" 2>&1 | grep -q adhoc && echo "unsigned: $exe"
done
```

## If notarization returns 401 and your credentials are right

This is the confusing one, and it is usually not the credentials.

After you create a Developer ID certificate, Apple frequently posts an updated
Developer Program License Agreement. Until it is accepted, the **notary service
answers 401 "Invalid credentials"** — even though the Apple ID, team id and
app-specific password are all correct, and even though `codesign` works fine
with the same certificate.

Go to [developer.apple.com/account](https://developer.apple.com/account). If
there is an agreement banner, accept it, then re-run the release. Nothing else
needs changing.

Only after that is clear is it worth re-checking:

- `APPLE_ID` is the Apple ID **email**, not a username.
- `APPLE_APP_PASSWORD` is app-specific (`xxxx-xxxx-xxxx-xxxx`) from
  [appleid.apple.com](https://appleid.apple.com), not the account password.
- `APPLE_TEAM_ID` matches what the certificate says:
  `security find-identity -v -p codesigning`

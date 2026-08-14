# Why Krate Studio ships Entitlements.plist

The studio was prompting people for their Documents folder, their music
library, and other volumes. None of that is in its code, and the prompts were
alarming: an app that writes one file was asking about Apple Music.

## What was actually happening

Measured with `vmmap` on the running process, before any picker was opened:

    MediaLibrary.framework
    Photos.framework
    PhotoLibraryServices.framework
    CloudPhotoLibrary.framework
    AVFoundation.framework
    CoreMediaIO.framework

None of these are linked at build time -- `otool -L` on the binary shows no
media frameworks at all. **WebKit loads them at startup**, because a webview
has to be ready for video, audio and camera elements. macOS sees an app
holding the photo and media stack, with nothing declared about what it intends
to do with it, and asks the person on demand.

## The fix

Two parts, and both are needed:

1. **Entitlements.plist** declares camera, microphone, photo library, music,
   movies, pictures, location, contacts and calendars as `false`. That is a
   promise the app will never use them, so the system has no reason to ask.

2. **The default output folder moved out of ~/Documents.** macOS guards
   Documents, Desktop and Downloads with TCC, so saving the first app there
   triggered a Documents prompt on its own -- unrelated to WebKit. The default
   is now `~/Krate Apps`, in the unguarded home folder. Documents is still
   available as a setting: if someone picks it, macOS asks once, in response
   to their own choice.

## Two traps when signing

**`--deep` drops the entitlements.** It re-signs the main executable last
without them, silently. Sign inner code first, then the app itself with
`--entitlements`, and verify with:

    codesign -d --entitlements - "/Applications/Krate Studio.app"

An empty result means they are not there, whatever the signing command
printed.

**The signer's XML parser is stricter than `plutil`.** This file had a long
explanatory comment and `codesign` failed with `AMFIUnserializeXML: syntax
error near line 16` while `plutil -lint` said OK. That is why the plist itself
carries no comments and this document exists instead.

## Adding a feature that needs one of these

Change the entry to `true` deliberately, and add the matching
`NS...UsageDescription` to the Info.plist so the prompt explains itself. The
list is a promise; breaking it silently is how apps end up feeling untrustworthy.

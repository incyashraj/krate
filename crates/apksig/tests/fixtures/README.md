# APK signing fixtures

Made with Android's own tools, so `tests/apksigner.rs` proves the verifier
reads what real signing writes. Inputs come from
`cargo test -p krate-apksig -- --ignored write_fixture_inputs` (an unsigned
APK naming `com.example.owned`, and the challenge for `app-rsa.apk`).

Tools: apksigner from Android SDK build-tools 36.1.0, keytool from the JBR
in Android Studio. Keys were throwaway PKCS#12 stores (not committed):
RSA 2048 `rsa`, `old`, `new`; EC P-256 `ec`.

    apksigner sign --ks rsa.p12 --min-sdk-version 24 --in app-unsigned.apk --out app-rsa.apk
    apksigner sign --ks ec.p12  --min-sdk-version 24 --in app-unsigned.apk --out app-ec.apk
    apksigner rotate --out lineage.bin --old-signer --ks old.p12 --new-signer --ks new.p12
    apksigner sign --ks old.p12 --next-signer --ks new.p12 --lineage lineage.bin \
        --min-sdk-version 24 --in app-unsigned.apk --out app-rotated.apk          # v3 old key, v3.1 new
    apksigner sign --ks old.p12 --next-signer --ks new.p12 --lineage lineage.bin \
        --rotation-min-sdk-version 24 --min-sdk-version 24 \
        --in app-unsigned.apk --out app-rotated-v3.apk                             # v3 new key, lineage
    apksigner sign --ks rsa.p12 --min-sdk-version 24 --in challenge-unsigned.apk --out challenge-rsa.apk
    apksigner sign --ks ec.p12  --min-sdk-version 24 --in challenge-unsigned.apk --out challenge-wrong-key.apk

The certificate digests the test expects are what
`apksigner verify --print-certs` printed for each file.

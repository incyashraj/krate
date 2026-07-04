# Core Concepts

These words show up everywhere in Krate. This page keeps them plain.

| Concept | Meaning |
|---------|---------|
| **WASM component** | The portable program file that Krate runs. |
| **Runtime** | The `krate` engine installed on a host machine. |
| **UAPI** | The common app API. Apps call this instead of calling each OS directly. |
| **UCap** | The permission system. It decides what an app is allowed to use. |
| **Host adapter** | The code that turns a Krate API call into a native OS call. |
| **`.l36app` bundle** | The future app package containing code, assets, manifest, and signature. |
| **Manifest** | The file that describes an app and the permissions it asks for. |
| **Marketplace** | The future distribution, update, and identity layer. |

## How They Fit

```mermaid
flowchart LR
    APP["WASM component"] --> UAPI["UAPI call"]
    UAPI --> UCAP["UCap check"]
    UCAP --> ADAPTER["Host adapter"]
    ADAPTER --> OS["Native OS"]
```

Example: a Krate app wants to read a file.

1. The app calls the Krate file API.
2. UCap checks whether that app has a grant for the file path.
3. The host adapter calls the native file API for the current OS.
4. The app gets a result that looks the same on every host.

Phase 1 proved the base runtime path. Phase 2 is the current working slice:
real UAPI calls for CLI-style apps, real capability checks, sample apps, and
evidence tracking. Later phases extend that model to GUI, mobile hosts,
bundles, signing, and distribution.

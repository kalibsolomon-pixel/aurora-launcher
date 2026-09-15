# Aurora Launcher

Aurora Launcher is a standalone launcher for the Aurora client mod for Minecraft: Java Edition. It is designed around isolated installations, transparent behavior, low background overhead, and user control.

## Development status

This repository contains the Phase 0 foundation (a Tauri 2 desktop shell, a Svelte/TypeScript frontend, a typed native status command, and platform-correct managed-data path resolution), the Phase 1 state model (a small versioned launcher configuration persisted as JSON under the managed-data root, a validated instance domain model with a read-only instance registry, deterministic managed-path derivation, and a validated Aurora release-manifest data model), the Phase 2 trusted artifact-acquisition pipeline: downloads are streamed into untrusted staging storage, verified against an expected size and SHA-256 digest, and promoted atomically into a content-addressed verified cache (`cache/artifacts/sha256/<digest>`), with deterministic tests served by local loopback test servers; the Phase 3 Minecraft metadata-resolution layer: an exact modern Minecraft version resolves from official Mojang metadata (the HTTPS discovery manifest plus a SHA-1-verified version document) into a deterministic, platform-aware normalized installation plan covering the Java requirement, client artifact, applicable libraries and native artifacts, the asset-index requirement, and unresolved launch arguments; and the Phase 4 Fabric metadata-resolution and composition layer: an exact Minecraft version plus Fabric Loader version resolves from the official Fabric Meta API into a normalized Fabric plan (loader libraries with their official digests where published, the intermediary when the Minecraft version needs one, and the Fabric client entry point) that composes with the vanilla plan into one deterministic game install plan.

Minecraft launching, Microsoft/Minecraft authentication, game installation (including all client/library/native/asset/runtime downloads and all Fabric artifact downloads), Fabric installation, Aurora downloads as a product feature, instance creation, profiles, and updates are **not implemented**.

## Prerequisites

- Git
- Node.js 20.19+ or 22.12+ with npm (Node.js 24 LTS is recommended)
- The stable Rust toolchain for the host platform
- Tauri 2 platform prerequisites:
  - Windows: Microsoft C++ Build Tools with the Desktop development with C++ workload and Microsoft Edge WebView2
  - macOS: Xcode Command Line Tools
  - Linux: the WebKitGTK and system packages listed in the [Tauri prerequisites guide](https://v2.tauri.app/start/prerequisites/)

## Install

```sh
npm install
```

## Run in development

```sh
npm run tauri dev
```

## Build

Build the frontend only:

```sh
npm run build
```

Build the desktop application and platform bundle:

```sh
npm run tauri build
```

## Test and check

```sh
npm run check
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo check --manifest-path src-tauri/Cargo.toml --all-targets
cargo test --manifest-path src-tauri/Cargo.toml
```

See [ARCHITECTURE.md](ARCHITECTURE.md) for implemented boundaries and future direction. Contributions are licensed under the [MIT License](LICENSE).

# Aurora Launcher

Aurora Launcher is a standalone launcher for the Aurora client mod for Minecraft: Java Edition. It is designed around isolated installations, transparent behavior, low background overhead, and user control.

## Development status

This repository contains the Phase 0 foundation (a Tauri 2 desktop shell, a Svelte/TypeScript frontend, a typed native status command, and platform-correct managed-data path resolution), the Phase 1 state model (a small versioned launcher configuration persisted as JSON under the managed-data root, a validated instance domain model, deterministic managed-path derivation, and a validated Aurora release-manifest data model), the Phase 2 trusted artifact-acquisition pipeline: downloads are streamed into untrusted staging storage, verified against an expected size and digest, and promoted atomically into content-addressed stores (SHA-256-addressed for artifacts with pre-known SHA-256 expectations, SHA-1-addressed for official Mojang artifacts, and a transport-observed store with recorded provenance for Fabric's digest-less artifacts), with deterministic tests served by local loopback test servers; the Phase 3 Minecraft metadata-resolution layer: an exact modern Minecraft version resolves from official Mojang metadata (the HTTPS discovery manifest plus a SHA-1-verified version document) into a deterministic, platform-aware normalized installation plan covering the Java requirement, client artifact, applicable libraries and native artifacts, the asset-index requirement, the official logging configuration, and unresolved launch arguments; the Phase 4 Fabric metadata-resolution and composition layer: an exact Minecraft version plus Fabric Loader version resolves from the official Fabric Meta API into a normalized Fabric plan that composes with the vanilla plan into one deterministic game install plan; the Phase 5 installation executor: a composed plan installs as a complete, isolated game under launcher-managed instance storage — verified acquisition per trust class, staged materialization with per-file trust validation, defensively safe native extraction, atomic commit of an Aurora-owned installed-state manifest, and deterministic re-validation from that manifest alone; and the Phase 6 persistent instance lifecycle: a writable atomic instance registry, generated opaque instance identifiers, instance creation that orchestrates release resolution → game installation → SHA-256-verified Aurora client-artifact installation → complete read-only validation before anything is reported ready, rename, selection with referential integrity, retry for interrupted creations, and a restrained production instance UI. Aurora releases currently come from a checked-in development fixture because no production release infrastructure exists.

Instances can be created, selected, renamed, retried, and completely validated. Minecraft launching, Microsoft/Minecraft authentication, Java runtime management, instance deletion, Aurora updates, and real Aurora release distribution are **not implemented**.

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

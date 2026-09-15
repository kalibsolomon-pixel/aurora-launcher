# Aurora Launcher

Aurora Launcher is a standalone launcher for the Aurora client mod for Minecraft: Java Edition. It is designed around isolated installations, transparent behavior, low background overhead, and user control.

## Development status

This repository contains the launcher foundation through Phase 7: Tauri/Svelte desktop boundaries and versioned state; verified SHA-256, official SHA-1, and transport-observed artifact stores; official Minecraft and Fabric planning; safe staged game installation; persistent isolated Aurora instances; and launcher-managed Java runtime provisioning. An exact Minecraft Java component and major version now resolves through Mojang's official per-platform runtime feed into a normalized plan, installs once in shared launcher storage through the existing SHA-1 cache, commits versioned state last, validates every managed entry, and runs a bounded `java -version` compatibility check. Aurora releases still come from a checked-in development fixture because no production release infrastructure exists.

Instances can be created, selected, renamed, retried, and completely validated. The selected content-ready instance can install, validate, and reuse its official managed Java runtime. Minecraft launching, Microsoft/Minecraft authentication, custom/system Java selection, instance deletion, Aurora updates, and real Aurora release distribution are **not implemented**.

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

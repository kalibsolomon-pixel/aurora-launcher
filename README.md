# Aurora Launcher

Aurora Launcher is a standalone launcher for the Aurora client mod for Minecraft: Java Edition. It is designed around isolated installations, transparent behavior, low background overhead, and user control.

## Development status

This repository contains the launcher foundation through Phase 9: Tauri/Svelte desktop boundaries and versioned state; verified SHA-256, official SHA-1, and transport-observed artifact stores; official Minecraft and Fabric planning; safe staged game installation; persistent isolated Aurora instances; launcher-managed Java runtime provisioning; Microsoft → Xbox → Minecraft authentication; and native launch assembly/process supervision. Signing in uses the system browser with public-client OAuth (authorization code + PKCE), verifies Minecraft ownership and profile, stores only the Microsoft refresh credential in the operating system's credential store (the real Windows Credential Manager; Linux/macOS persistence is not implemented yet and fails deliberately), and restores Rust-only sessions with refresh-token rotation.

**Authentication is live-verified.** The Aurora Client Microsoft application registration exists and has completed Microsoft's Minecraft Services AppID review/allowlisting, and the production OAuth flow has been verified end-to-end against the real services: system-browser authorization with a loopback callback (registered redirect `http://localhost`, ephemeral port, root path), PKCE/state validation, the Microsoft → Xbox Live → XSTS → Minecraft Services exchange chain, entitlement and profile retrieval, and secure persistence across an application restart — with no secret logged or persisted in plaintext. A build without the client ID (`AURORA_MICROSOFT_CLIENT_ID`) configured still reports `auth_configuration_missing` honestly. The launch pipeline is implemented and tested against current official metadata with synthetic sessions and harmless child processes; a real end-user launch now awaits the production Aurora release artifact rather than authentication.

Aurora releases still come from a checked-in development fixture because no production release infrastructure exists.

Instances can be created, selected, renamed, retried, and completely validated. The selected content-ready instance can install, validate, and reuse its official managed Java runtime. Rust owns Play readiness, exact classpath/native/logging/asset argument assembly, token redaction, structured process spawning, and exit supervision. Custom/system Java selection, instance deletion, Aurora updates, and real Aurora release distribution are **not implemented**.

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

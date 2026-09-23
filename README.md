# Aurora Launcher

Aurora Launcher is a standalone launcher for the Aurora client mod for Minecraft: Java Edition. It is designed around isolated installations, transparent behavior, low background overhead, and user control.

This repository is the canonical public repository for Aurora Launcher. It contains the launcher implementation (Tauri, Rust, and Svelte) and serves as the public release location. The Aurora client mod itself is developed as a separate project.

Aurora Launcher does **not** provide Minecraft accounts, bypass Minecraft ownership requirements, or distribute authentication credentials. Playing requires the user's own Microsoft account and a legitimate Minecraft: Java Edition entitlement.

## Development status

This repository contains the launcher foundation through Phase 9: Tauri/Svelte desktop boundaries and versioned state; verified SHA-256, official SHA-1, and transport-observed artifact stores; official Minecraft and Fabric planning; safe staged game installation; persistent isolated Aurora instances; launcher-managed Java runtime provisioning; Microsoft → Xbox → Minecraft authentication; and native launch assembly/process supervision. Signing in uses the system browser with public-client OAuth (authorization code + PKCE), verifies Minecraft ownership and profile, stores only the Microsoft refresh credential in the operating system's credential store (the real Windows Credential Manager; Linux/macOS persistence is not implemented yet and fails deliberately), and restores Rust-only sessions with refresh-token rotation.

**Authentication is live-verified.** The Aurora Client Microsoft application registration exists and has completed Microsoft's Minecraft Services AppID review/allowlisting, and the production OAuth flow has been verified end-to-end against the real services: system-browser authorization with a loopback callback (registered redirect `http://localhost`, ephemeral port, root path), PKCE/state validation, the Microsoft → Xbox Live → XSTS → Minecraft Services exchange chain, entitlement and profile retrieval, and secure persistence across an application restart — with no secret logged or persisted in plaintext. The application (client) ID is public application configuration — a desktop public client has no client secret — and is committed in the repository (`src-tauri/src/auth/flow.rs`), so normal development and official builds sign in without manual environment setup; the optional `AURORA_MICROSOFT_CLIENT_ID` build-environment variable overrides it for fork builds, which otherwise report `auth_configuration_missing` honestly. The launch pipeline is implemented and tested against current official metadata with synthetic sessions and harmless child processes; a real end-user launch now awaits the production Aurora release artifact rather than authentication.

Aurora releases still come from a checked-in development fixture because no production release infrastructure exists.

Instances are fully configurable: each one carries the desired Minecraft version (chosen from official Mojang metadata, snapshots behind an explicit filter), a Fabric Loader version policy (latest compatible or pinned), a memory allocation that produces the real JVM heap argument, additional JVM arguments with strict quoting and conflict rules, an optional custom window size, and a display name — saved atomically, with launch-only changes applying immediately and install-affecting changes applied through a deliberate, verified installation step. Older persisted instances migrate to the new registry schema automatically, preserving their identity and installed content. Instances can be selected, renamed, retried, and completely validated. The selected content-ready instance can install, validate, and reuse its official managed Java runtime. Rust owns Play readiness, exact classpath/native/logging/asset argument assembly, token redaction, structured process spawning, and exit supervision. Custom/system Java selection, instance deletion, Aurora updates, and real Aurora release distribution are **not implemented**.

The launcher's appearance is customizable from the Settings page: three built-in dark themes (the default Aurora Dark, the cooler Midnight, and true-black OLED) and an accent color chosen from a curated palette or a custom color, validated natively so text, focus, and status colors stay readable. The choice applies live and is remembered across restarts. Custom/system Java selection, instance deletion, Aurora updates, and real Aurora release distribution are **not implemented**.

Navigation separates global launcher destinations (Home, Instances, Accounts, Settings, About) from a contextual instance workspace: opening an instance from the sidebar shortcuts or the Instances page shows a stable header (breadcrumb, readiness badge, Open folder, Play) with Overview and Settings tabs. The Overview summarizes readiness, the active configuration, and the installed release; the Settings tab edits that instance's configuration with an explicit save step, while launcher-wide preferences stay in the sidebar's Settings destination. Home remains the fast launch surface, and both Play buttons use the same readiness-checked launch pipeline. Mod, resource-pack, shader, and log management are **not implemented** — the workspace is the shell they will later fit into.

Aurora's external application identity — the Windows executable, taskbar, title-bar, installer, and shortcut icon — is a rounded-square dark neutral gradient with the exact white Aurora mark, kept separate from the transparent internal mark used inside the UI, and generated deterministically from it by repository-owned tooling. On Windows, Settings also shows live desktop-integration state: an Aurora-owned desktop shortcut can be created and removed from there (only Aurora's own shortcut is ever touched), while the Start-menu shortcut that the installer creates is reported read-only. Shortcut status always reflects the operating system as it is right now. Linux/macOS desktop integration, taskbar pinning, file associations, and URI registration are **not implemented**.

## Authentication and privacy

Aurora Launcher signs players in through the Microsoft OAuth 2.0 authorization-code flow with PKCE in the user's system browser and never collects or stores Microsoft account passwords. After Microsoft sign-in it follows the standard Xbox Live, XSTS, and Minecraft Services flow required of third-party Minecraft: Java Edition launchers: it verifies Minecraft: Java Edition ownership, retrieves the authenticated player's Minecraft profile, and obtains the Minecraft access token required to launch the game. Authorization requests Xbox Live sign-in and offline access so a returning user can restore their session without repeating sign-in. The only persisted credential is the Microsoft refresh credential, held in operating-system secure credential storage, never in plaintext launcher configuration.

The launcher is designed around local operation and minimal data collection. It does not require an Aurora account and does not include unnecessary analytics or telemetry. Microsoft/Xbox/Minecraft credentials are used only for authentication and game-launch functionality. Downloaded game and client artifacts are integrity-checked before activation, and Minecraft instances are kept isolated from the user's default `.minecraft` installation.

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
npm run test
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo check --manifest-path src-tauri/Cargo.toml --all-targets
cargo test --manifest-path src-tauri/Cargo.toml
```

See [ARCHITECTURE.md](ARCHITECTURE.md) for implemented boundaries and future direction. Contributions are licensed under the [MIT License](LICENSE).

## Disclaimer

Aurora Launcher and Aurora are independent projects and are not affiliated with, endorsed by, or sponsored by Microsoft, Mojang Studios, or Fabric.

Minecraft is a trademark of Microsoft Corporation.

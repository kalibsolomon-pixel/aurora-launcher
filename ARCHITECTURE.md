# Aurora Launcher architecture

This document is the living source of truth for the launcher's boundaries. It distinguishes the foundation implemented now from future design so planned functionality is never mistaken for a working feature.

## Product philosophy

Aurora Launcher should make Aurora feel like a complete client product without becoming a second source of bloat. Favor fast startup, minimal background work, explicit user control, transparent and diagnosable behavior, isolated game installations, repairable managed artifacts, and cross-platform paths and processes. Do not duplicate settings that belong inside the Aurora mod.

## Implemented in Phase 0

The desktop application has three small layers:

1. `src/routes/+page.svelte` renders the shell and owns loading/success/error presentation state.
2. `src/lib/backend.ts` owns the TypeScript DTO and the single Tauri invocation. UI components do not perform native platform or filesystem work.
3. `src-tauri/src` owns native application status and managed-path resolution. `lib.rs` wires commands and plugins; `main.rs` is only the desktop entry point.

The `get_application_status` command returns a serialized `ApplicationStatus` with launcher version, OS, architecture, resolved managed-data root, and backend readiness. It returns a structured `CommandError` (`code` and user-readable `message`) rather than an arbitrary error string. The command queries Tauri's platform path resolver and does not create directories.

There is no persistent launcher configuration yet because Phase 0 has no preference that needs persistence. There is no logging dependency; concise startup/status diagnostics use standard error output in development-capable environments.

## Frontend/native boundary

Svelte is a presentation layer. Security-sensitive state and all future Minecraft/Aurora installation, authentication, download, integrity, Java/runtime, filesystem mutation, and process-launch logic stay behind native Rust commands or events. Commands should be narrow and use explicit request/response DTOs. Frontend code must not infer structured state by parsing strings.

SvelteKit is configured as a static, client-side SPA because Tauri has no Node server. Vite remains the development and production asset builder.

## Current native modules

- `application`: constructs the status DTO, maps internal failures to the command error contract, and exposes the Tauri command.
- `paths`: validates and represents the platform-resolved application-local data root without touching the filesystem.

Future code should add a module when its behavior is implemented. Likely domain boundaries are application state, instances, Minecraft metadata/installations, Fabric, Java/runtime management, downloads, integrity, Aurora distribution metadata, authentication, and launch/process supervision. Avoid a speculative service container, placeholder traits, or empty module trees.

## Managed filesystem model

The managed-data root comes from Tauri's application-local data directory for identifier `com.aurora.launcher`. It is never the user's normal `.minecraft` directory and is not located beside the executable. Phase 0 resolves this root but creates none of the future tree. The desktop webview runtime may create its own platform cache beneath this application directory on first boot (for example, WebView2 creates `EBWebView` on Windows); that runtime-owned cache is not an Aurora instance.

A proportional future layout is:

```text
<managed-data-root>/
├── launcher/       # versioned non-secret launcher configuration and state
├── runtimes/       # launcher-managed Java runtimes
├── cache/          # re-downloadable metadata and temporary artifacts
├── metadata/       # verified manifests and installation metadata
└── instances/
    └── <instance-id>/
        ├── game/   # isolated game directory and managed libraries/assets links
        ├── mods/   # Aurora plus optional instance-scoped mods
        ├── config/ # Minecraft, Fabric, Aurora, and mod configuration
        └── logs/   # instance-local launch/game logs
```

- Cache and incomplete temporary downloads should be safe to delete and reconstruct.
- Managed runtimes and verified launcher metadata should be repairable/re-downloadable, but deletion may be expensive and must remain scoped to proven managed paths.
- Instance saves, screenshots, servers, resource packs, shader packs, configuration, and user-added mods are user data. Back up or obtain explicit confirmation before destructive replacement or deletion.
- Secrets do not belong in ordinary launcher configuration. Use OS-backed secure credential storage where practical.
- User-selected external paths are not launcher-managed merely because the launcher can read them.

## Future instances

An instance is an isolated, identifiable game installation with its own game directory, compatible Aurora release, Minecraft version, Fabric Loader version, Java requirements, and user content. Profiles may later select or parameterize instances, but neither instances nor profiles exist today. Path validation must prevent traversal and destructive operations must prove that a target is beneath launcher-managed storage.

## Future distribution metadata

Aurora distribution should be manifest-driven rather than tied forever to one Minecraft version. A versioned release record should map:

```text
Aurora release and channel
  -> supported Minecraft version
  -> required Fabric Loader version
  -> Java/runtime requirements
  -> Aurora artifact URL
  -> expected cryptographic hash
```

The model must support stable, beta, and nightly channels. A static, versioned, signed manifest hosted later is likely sufficient initially; no backend service is justified yet. Manifest signature/key rotation, canonical encoding, and rollback policy remain decisions for the phase that implements distribution.

## Security model

### Authentication

- Never request or store a Microsoft password.
- Use the appropriate Microsoft OAuth flow and keep reusable tokens out of plaintext preferences.
- Prefer OS-backed secure credential storage and keep sensitive token handling native where practical.
- Frontend errors and logs must not expose tokens or secrets.

### Downloads and activation

- Treat every downloaded artifact as untrusted until it matches a known expected hash from trusted metadata.
- Download to temporary files, verify before activation, and use atomic or rollback-safe replacement where practical.
- Integrity mismatch is an explicit hard failure, never a warning that permits activation.

### Process execution

- Use structured executable-and-argument process APIs, never shell command strings.
- Separate launcher-generated trusted arguments from user-supplied JVM arguments and validate the latter at their trust boundary.
- Emit actionable, sanitized diagnostics without credentials or other secrets.

### Paths and deletion

- Reject uncontrolled traversal and normalize/validate containment before mutation.
- Track whether a path is launcher-managed or explicitly user-selected.
- Never recursively delete until the resolved target is proven to be inside the managed root and is the intended resource.

## Dependency decisions

- Tauri provides the cross-platform native shell, command boundary, platform path resolution, and build tooling.
- Serde derives the typed Rust-to-frontend DTO serialization.
- Svelte and TypeScript implement the typed presentation layer; SvelteKit's static adapter is retained from the official Tauri Svelte template to produce a serverless SPA.
- Vite supplies fast development and production asset builds; `svelte-check` provides compiler-aware type checking.
- No opener, HTTP, authentication, keyring, hashing, archive, Minecraft, updater, or logging package is included because Phase 0 has no behavior that needs one.

## Major Phase 0 decisions

- Bundle identifier and path compatibility key: `com.aurora.launcher`.
- Application-local data is preferred over roaming data because runtimes, caches, logs, and instances are machine-local and can be large.
- Path resolution is read-only at startup; directory creation waits for behavior that needs it.
- The first boundary proof uses real platform metadata, not a frontend mock.
- SvelteKit static SPA mode follows the current official Tauri/Svelte scaffold; no server runtime ships with the launcher.

## Explicitly deferred

Authentication and token storage; Minecraft, Fabric, Java, and Aurora acquisition; download and integrity pipelines; configuration persistence; instance/profile/mod/resource-pack/shader management; JVM argument construction; game launch and supervision; self-update; telemetry; social/news/cosmetic/cloud systems; and any custom backend service.

## Known limitations

The launcher only reports native status. It does not create or validate a managed directory tree, persist settings, install or repair artifacts, authenticate accounts, or launch a process. The TypeScript DTO mirrors the Rust DTO manually; if the boundary grows, evaluate generated bindings then rather than adding that dependency preemptively.

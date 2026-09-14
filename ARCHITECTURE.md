# Aurora Launcher architecture

This document is the living source of truth for the launcher's boundaries. It distinguishes the foundation implemented now from future design so planned functionality is never mistaken for a working feature.

## Product philosophy

Aurora Launcher should make Aurora feel like a complete client product without becoming a second source of bloat. Favor fast startup, minimal background work, explicit user control, transparent and diagnosable behavior, isolated game installations, repairable managed artifacts, and cross-platform paths and processes. Do not duplicate settings that belong inside the Aurora mod.

## Implemented in Phase 0

The desktop application has three small layers:

1. `src/routes/+page.svelte` renders the shell and owns loading/success/error presentation state.
2. `src/lib/backend.ts` owns the TypeScript DTOs and the Tauri invocations. UI components do not perform native platform or filesystem work.
3. `src-tauri/src` owns native application status, managed-path resolution, and the persisted-state domain. `lib.rs` wires commands and modules; `main.rs` is only the desktop entry point.

The `get_application_status` command returns a serialized `ApplicationStatus` with launcher version, OS, architecture, resolved managed-data root, and backend readiness. It returns a structured `CommandError` (`code` and user-readable `message`) rather than an arbitrary error string. The command queries Tauri's platform path resolver and does not create directories.

There is no logging dependency; concise startup/status diagnostics use standard error output in development-capable environments.

## Implemented in Phase 1

Phase 1 adds the local persistent-state model, the instance domain model, safe managed-path derivation, and the Aurora release-manifest data model. It implements no installation, downloading, authentication, or launching.

### Launcher configuration (`config`)

`LauncherConfig` is a deliberately small versioned document:

```json
{
  "schemaVersion": 1,
  "selectedInstanceId": null
}
```

- Stored as pretty, human-inspectable JSON (camelCase keys, matching the frontend DTO convention) at `<managed-data-root>/launcher/config.json`.
- Schema version `1` is the only supported version. A different version is a structured error (`config_unsupported_schema`), never an automatic migration; the version gate is the single point where future migration would attach.
- A missing file is materialized with safe defaults on the first state read. This is the only directory creation Phase 1 performs (`launcher/` plus the config file).
- Malformed data (invalid JSON, invalid UTF-8, wrong shape, invalid identifier) is a structured error (`config_malformed`) and the file on disk is never silently replaced or overwritten.
- Writes go through a sibling temporary file and a rename, so a partially written configuration can never be observed.
- Secrets do not belong in this file, and settings are added only when behavior requires them.

### Instance domain (`instances`)

- `InstanceId` is the only key to instance filesystem identity: 1–64 characters from `a–z`, `0–9`, `-`, `_`, beginning and ending with a letter or digit, and never a Windows-reserved device name (`con`, `prn`, `aux`, `nul`, `com0–9`, `lpt0–9`). Separators, traversal fragments, absolute paths, uppercase, and whitespace are rejected before any path can be constructed. Identifier generation is deferred; no dependency exists solely to generate IDs.
- `InstanceRecord` is `{ id, displayName, release }` where `release` is `{ channel, auroraVersion }` (`stable`/`beta`/`nightly`; a `null` version means "newest of the channel", resolved at a future install step). Display names are user-facing text with length/whitespace limits and never influence the filesystem.
- `InstanceRegistry` is the persisted list at `<managed-data-root>/launcher/instances.json` (schema version `1`). This phase only loads it: a missing file is an empty registry, while malformed data, unknown schema versions, or duplicate identifiers are deliberate errors (`instances_invalid`, `instances_unsupported_schema`). No code creates instances yet, so the registry is never written.
- The config's `selectedInstanceId` is validated as an identifier shape but is not yet cross-checked against the registry; referential validation arrives with the phase that manages instances.

### Managed-path derivation (`paths`)

`ManagedPaths` deterministically derives `launcher/`, `cache/`, `metadata/`, `runtimes/`, `instances/`, and per-instance `instances/<id>/{game,mods,config,logs}` locations. Derivation is pure — resolving locations never touches disk — and remains inside the managed root because instance paths accept only a validated `InstanceId` (display names and raw strings cannot reach path construction). Tests prove lexical containment and reject unsafe identifiers.

### Aurora release-manifest model (`distribution`)

A local, versioned manifest representation exists for future distribution work:

```text
ReleaseManifest (schemaVersion 1)
└── releases: AuroraRelease
    ├── auroraVersion
    ├── channel (stable | beta | nightly)
    ├── minecraftVersion
    ├── fabricLoaderVersion
    ├── java.majorVersion
    └── artifact { url, sha256, sizeBytes? }
```

- Each release maps independently to its Minecraft version, Fabric Loader version, and Java major version; nothing assumes a fixed pairing.
- Parsing validates deliberately: known channels only, non-empty whitespace-free version strings, HTTPS-only artifact URLs, exactly-64-hex-character SHA-256 fields, positive Java major versions, and positive artifact sizes when present. Invalid data fails with descriptive errors.
- This is representation only. Manifest fetching, signing, hash verification, downloading, and installation are not implemented; the URL/hash checks validate encoding, not trust.

## Frontend/native boundary

Svelte is a presentation layer. Security-sensitive state and all future Minecraft/Aurora installation, authentication, download, integrity, Java/runtime, filesystem mutation, and process-launch logic stay behind native Rust commands or events. Commands should be narrow and use explicit request/response DTOs. Frontend code must not infer structured state by parsing strings.

SvelteKit is configured as a static, client-side SPA because Tauri has no Node server. Vite remains the development and production asset builder.

## Current native modules

- `application`: constructs the status and launcher-state DTOs, maps internal failures to the command error contract, and exposes the Tauri commands (`get_application_status`, `get_launcher_state`).
- `paths`: validates and represents the platform-resolved application-local data root and derives managed locations without touching the filesystem.
- `config`: the versioned launcher-configuration model and its atomic JSON persistence.
- `instances`: validated instance identifiers, instance records, and the read-only instance-registry loader.
- `distribution`: the typed, validated Aurora release-manifest data model (local representation only).

Future code should add a module when its behavior is implemented. Likely domain boundaries are instance lifecycle, Minecraft metadata/installations, Fabric, Java/runtime management, downloads, integrity, distribution transport, authentication, and launch/process supervision. Avoid a speculative service container, placeholder traits, or empty module trees.

Structured error codes crossing the command boundary: `managed_path_unavailable`, `config_malformed`, `config_unsupported_schema`, `instances_invalid`, `instances_unsupported_schema`, and `storage_io_failure`. Codes are compatibility contracts; keep them stable and user messages readable.

## Managed filesystem model

The managed-data root comes from Tauri's application-local data directory for identifier `com.aurora.launcher`. It is never the user's normal `.minecraft` directory and is not located beside the executable. Phase 1 derives the full layout and materializes only `launcher/config.json` (with its parent directory) on first state read; everything else resolves without touching disk. The desktop webview runtime may create its own platform cache beneath this application directory on first boot (for example, WebView2 creates `EBWebView` on Windows); that runtime-owned cache is not an Aurora instance.

The implemented layout is:

```text
<managed-data-root>/
├── launcher/       # versioned non-secret launcher configuration and state (config.json materialized; instances.json read when present)
├── runtimes/       # launcher-managed Java runtimes (derived, not created yet)
├── cache/          # re-downloadable metadata and temporary artifacts (derived, not created yet)
├── metadata/       # verified manifests and installation metadata (derived, not created yet)
└── instances/      # one directory per validated instance id (derived, not created yet)
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

## Instances

An instance is an isolated, identifiable game installation with its own game directory, compatible Aurora release, Minecraft version, Fabric Loader version, Java requirements, and user content. The Phase 1 domain model (validated identifiers, records, registry loading, path derivation) is implemented; creating, installing, selecting in the UI, or deleting instances is not. Path validation prevents traversal, and destructive operations must prove that a target is beneath launcher-managed storage.

## Distribution metadata

Aurora distribution should be manifest-driven rather than tied forever to one Minecraft version. The Phase 1 local schema implements exactly this shape:

```text
Aurora release and channel
  -> supported Minecraft version
  -> required Fabric Loader version
  -> Java/runtime requirements
  -> Aurora artifact URL
  -> expected cryptographic hash
```

The model supports stable, beta, and nightly channels and keeps each mapping independent. What remains future: hosting and fetching the manifest, signature verification, key rotation, canonical encoding, and rollback policy — decisions for the phase that implements distribution.

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
- `serde_json` (added in Phase 1) implements the human-inspectable JSON persistence of the launcher configuration and instance registry, and the release-manifest model's parse/validate tests. It is the focused Serde-native JSON crate; no other persistence dependency is justified.
- Svelte and TypeScript implement the typed presentation layer; SvelteKit's static adapter is retained from the official Tauri Svelte template to produce a serverless SPA.
- Vite supplies fast development and production asset builds; `svelte-check` provides compiler-aware type checking.
- No opener, HTTP, authentication, keyring, hashing, archive, updater, UUID, URL-parsing, error-derivation, or logging package is included: Phase 1 needs none of them (identifier generation is deferred, URLs and hashes are only shape-validated, and errors are hand-rolled like Phase 0's).

## Major Phase 0 decisions

- Bundle identifier and path compatibility key: `com.aurora.launcher`.
- Application-local data is preferred over roaming data because runtimes, caches, logs, and instances are machine-local and can be large.
- Path resolution is read-only at startup; directory creation waits for behavior that needs it.
- The first boundary proof uses real platform metadata, not a frontend mock.
- SvelteKit static SPA mode follows the current official Tauri/Svelte scaffold; no server runtime ships with the launcher.

## Major Phase 1 decisions

- Persisted documents carry explicit schema versions, and loaders fail deliberately on unknown versions instead of migrating or guessing; migration infrastructure stays unbuilt until a second version exists.
- A malformed persisted file is never overwritten: the launcher reports it and stops, keeping user-repairable data intact.
- Configuration is materialized with defaults on first state read (creating `launcher/`); the instance registry stays read-only because nothing can create instances yet.
- camelCase JSON on disk mirrors the frontend DTO convention so the file, the Rust DTOs, and the TypeScript DTOs share one naming shape.
- Instance filesystem identity is a validated `InstanceId` enforced by the type system; display names can never reach path derivation.
- Artifact URLs are validated as HTTPS shape and hashes as hexadecimal encoding only; these are representation checks, not integrity or trust mechanisms.

## Explicitly deferred

Authentication and token storage; Minecraft, Fabric, Java, and Aurora acquisition; download and integrity pipelines; instance/profile/mod/resource-pack/shader creation and management (the Phase 1 registry and path model only represent them); JVM argument construction; game launch and supervision; self-update; telemetry; social/news/cosmetic/cloud systems; and any custom backend service.

## Known limitations

The launcher reports native status, persists a minimal configuration, and loads the instance registry read-only. It does not create or validate a managed directory tree beyond the launcher configuration, create instances, fetch or verify manifests, install or repair artifacts, authenticate accounts, or launch a process. A hand-edited `selectedInstanceId` is validated as an identifier shape but not checked against the registry until instance management exists. The TypeScript DTOs mirror the Rust DTOs manually; if the boundary grows further, evaluate generated bindings then rather than adding that dependency preemptively.

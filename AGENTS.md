# Agent operating guide

## Purpose and stack

Aurora Launcher is the standalone, cross-platform launcher for the separate Aurora Fabric client mod. The stack is Tauri 2, Rust, Svelte 5, TypeScript, SvelteKit in static SPA mode, and Vite. The launcher consumes Aurora as a future versioned artifact; never copy or couple to the mod's source tree.

## Commands

```sh
npm install
npm run tauri dev
npm run check
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo check --manifest-path src-tauri/Cargo.toml --all-targets
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri build
```

## Boundaries

- Svelte is presentation only. Installation, downloads, authentication, secure credentials, integrity checks, Java/runtime handling, filesystem mutation, and process launch belong in Rust.
- Cross the frontend/native boundary through narrow, typed commands/events. Keep DTO field naming and error shapes synchronized with `src/lib/backend.ts`.
- Use platform application-data APIs. Never default an instance to the user's normal `.minecraft`, and never write mutable data beside the executable.
- Grow Rust by launcher domain, not in a giant `main.rs` or catch-all service. Add modules only with implemented behavior.

## Persisted state

- Versioned persisted documents (launcher configuration, instance registry) fail deliberately on unknown schema versions and on malformed data; never migrate speculatively and never overwrite a damaged file.
- Instance filesystem identity is always a validated `InstanceId`. Never derive paths from display names, user text, or raw strings.
- Add persisted preferences only when implemented behavior needs them; secrets never enter ordinary configuration files.

## Discipline and security

- Add a dependency only with current functionality that needs it; prefer standard-library code and focused maintained packages. Commit lockfiles.
- Passwords never enter this application. Future auth uses Microsoft OAuth, native handling, and OS-backed credential storage where practical.
- Treat downloads as untrusted until an expected hash is verified; use temporary files and rollback-safe activation.
- Never trust a cache object because of its filename: verified cache artifacts are re-validated by hashing before reuse, and a corrupt object is replaced only through a full verified acquisition.
- Never install, extract, activate, or execute from an unverified staging file; only the verified store under `cache/artifacts/sha256/<digest>` contains trusted artifacts.
- Remote file names, `Content-Disposition`, and URLs never determine local paths; verified cache identity is the SHA-256 digest alone.
- Production artifact sources are HTTPS-only. Cleartext HTTP is acceptable exclusively for explicit loopback hosts (`127.0.0.1`, `::1`, `localhost`) as the documented test transport path; deterministic tests use local loopback servers, never public internet.
- Spawn processes with structured executable/argument APIs, never shell strings. Sanitize diagnostics and never log secrets or reusable tokens.
- Validate path containment before destructive operations. Distinguish managed paths from user-selected external paths.
- Do not casually change the Tauri identifier (`com.aurora.launcher`), managed-data semantics, command DTOs, error codes, or isolation model; they become compatibility contracts.

## Minecraft metadata and install planning

- Minecraft version resolution comes from first-party official Mojang metadata (the pinned piston-meta manifest endpoint and the URLs it provides), never third-party launcher APIs, scraped websites, or frontend-supplied URLs.
- Installers consume the normalized `MinecraftInstallPlan`; they never traverse or reparse raw Mojang JSON. External metadata DTOs stay inside `minecraft::metadata` and never become the launcher's domain model.
- Do not bypass the trusted artifact layer when acquiring product artifacts, and do not distort it for metadata: the version manifest is bootstrap discovery metadata (HTTPS plus validation, never a "verified artifact"), while version documents are verified against their manifest-provided SHA-1. HTTPS success alone is never treated as artifact verification.
- Official Mojang digests are SHA-1 and are represented as such; never relabel one as SHA-256, invent a digest, or discard an available official hash. The verified cache remains SHA-256-addressed.
- Historical metadata shapes (`inheritsFrom`, `minecraftArguments`, `natives`/`classifiers`/`extract`, `old_beta`/`old_alpha`) are rejected deliberately as unsupported; do not add speculative historical compatibility.
- Launch placeholders (`${auth_player_name}` and friends) stay semantically unresolved; never substitute fake account, session, or token values, and never shell-escape arguments.
- Rule evaluation stays pure, deterministic, and vocabulary-strict in `minecraft::rules`; do not scatter platform checks across modules or assume the development machine's platform.

## Fabric metadata and plan composition

- Fabric resolution consumes only the official Fabric Meta API from its pinned root (`https://meta.fabricmc.net/v2/`), never third-party launcher APIs, scraped web pages, or frontend-supplied URLs.
- Loader selection is exact: verify the requested Fabric Loader version exists, verify the Minecraft/Loader combination is supported, and reject unsupported combinations deliberately. Never silently substitute a newer loader, select "latest", or introduce automatic upgrades; Aurora release metadata, not Fabric Meta, is the intended policy source for required loader versions.
- Installers consume the normalized `FabricPlan` and composed `GameInstallPlan`; they never parse raw Fabric Meta JSON. External Fabric DTOs stay inside `fabric::metadata` and never become the launcher's domain model.
- Preserve the vanilla `MinecraftInstallPlan` as a meaningful, independently testable boundary; never mutate it into a Fabric-modified shape. Composition stays explicit so Mojang and Fabric requirements remain distinguishable.
- Never fabricate a digest for a Fabric artifact: record official Fabric SHA-256 digests where published, represent digest-less artifacts (the loader and intermediary) honestly as digest-less, and never treat an HTTPS-only Fabric artifact as cryptographically verified. Mojang SHA-1 and Aurora SHA-256 semantics remain unchanged.
- Do not introduce generic mod-loader abstractions, loader plugin systems, service containers, Maven frameworks, or dependency resolvers without a real implemented requirement; Aurora uses Fabric, so build the Fabric boundary directly.
- Do not solve deferred installation concerns (SHA-1 cache storage, natives extraction, log4j configuration, metadata persistence) inside Fabric work unless Fabric composition genuinely requires the domain adjustment.

## Game installation execution

- Installers consume normalized plans only (`GameInstallPlan` plus a validated `InstanceId`); they never parse raw Mojang or Fabric metadata, re-decide versions, or re-evaluate rules at install time. The asset index is acquired and verified like any artifact first, then parsed by the DTO boundary in `minecraft::metadata`.
- Never download directly into an instance location and never install directly from network staging: acquisition goes through the content-addressed stores (`sha256/`, `sha1/`, `transport-observed/`), and materialization copies only from verified store objects into instance staging.
- Installed state is committed last: the `installed-game.json` manifest is the completion marker, written at the end of the staged tree, made visible together with it by one directory rename. An interrupted installation must never look complete; a `game/` directory without a valid manifest is not an installation.
- Never call a transport-only Fabric artifact digest-verified: digest-less artifacts are `SecureTransportObserved` (HTTPS transport plus a locally observed SHA-256), their observed digests are consistency references with recorded provenance, and they never gain `ExpectedDigestVerified` status. A content drift under a pinned URL fails deliberately.
- Never extract an archive without path containment checks: relative forward-slash names only, traversal/absolute/drive-letter/backslash rejected, META-INF skipped per launcher semantics, duplicate names deduplicated only when byte-identical and a hard conflict otherwise, extraction scoped to the natives staging root.
- Never recursively delete an instance, or any path, without proving it is the fixed derived staging location or a manifest-proven managed game directory. `mods/`, `config/`, `logs/`, and future instance-root user content are never touched by installation; `.minecraft` remains off-limits.
- Never overwrite a malformed or unsupported-schema installed-state document; fail deliberately and leave repair to the user. Unknown existing `game/` trees are a hard conflict, not something to wipe.
- One installation per instance at a time; overlapping attempts fail with `installation_already_in_progress`. Do not add download parallelism, pause/resume, or repair without a phase that owns those decisions.

## Persistent instance lifecycle

- Instance display names never determine paths or identifiers; identifiers are generated opaque UUIDs, and rename changes metadata only — never move a filesystem directory.
- Registry writes are atomic (temporary sibling plus rename), schema-versioned, duplicate-checked, and never overwrite a malformed or unsupported file.
- Incomplete instances are never reported ready: creation persists an explicit `installing` record before installing and promotes it to `ready` only after complete validation passes; failures leave a retryable record rather than destructive rollback.
- Aurora artifacts require a pre-known expected SHA-256 from release metadata and verify through the SHA-256 store — never the transport-observed path; a release without a digest fails.
- Aurora-managed mods are deterministically named (`mods/aurora-<version>.jar`) and recorded in the instance's Aurora installed-state document so they stay distinguishable from user mods; never delete or enumerate user mods/config during install, update, or rollback, and never treat unrelated user files as validation damage.
- Instances pin concrete releases (channel + Aurora/Minecraft/Fabric Loader versions); never represent an instance as channel-only, never move instances between channels silently, and never auto-update.
- The release source today is the checked-in development fixture because no production Aurora release infrastructure exists; never invent production URLs, endpoints, or signing — wire the real source when it exists and keep the UI honest about the development source.
- Complete-instance validation is read-only and download-free: registry, game, Aurora artifact hash, and three-way version consistency. Never mutate or download during validation.
- A dangling selected instance is a deliberate state error; never silently select a random instance. Selection writes only identifiers present in the registry.
- Instance deletion is out of scope until a phase owns user-data retention policy; do not add delete buttons or recursive instance removal.
- `.minecraft` remains off-limits.

## Verification and Git

Run checks proportional to the change. Native-boundary or startup changes require type checking, Rust format/check/test, a production build, and a real application boot when the host permits it. Keep documentation aligned with implemented behavior. Preserve user changes, inspect `git status`, make focused commits, and never push unless explicitly requested.

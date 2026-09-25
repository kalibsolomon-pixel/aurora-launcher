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
- This is representation only. Manifest fetching, signing, and installation are not implemented. Phase 2 added the generic download/hash-verification pipeline, but nothing yet connects a fetched manifest to it; the URL/hash checks here validate encoding, not trust.

## Implemented in Phase 2

Phase 2 adds the first trusted artifact-acquisition pipeline: the launcher can take validated artifact metadata (HTTPS URL, expected SHA-256, optional expected byte size) and obtain a verified object inside Aurora-managed cache storage. It implements generic acquisition infrastructure only — no Minecraft, Fabric, Java, Aurora, or any other product artifact is fetched or installed.

### Trust boundary

> Download completion does not imply artifact trust.

Transfer success, HTTP status, and file presence never make an artifact trusted. A verified artifact must satisfy, in order: transport completed, the byte count matched the expected size when one exists, the exact-byte SHA-256 matched the manifest-provided digest, and the verified object was promoted into managed cache storage. Every failure is fail-closed and leaves no object in the verified store; failed staging files are removed.

### Acquisition pipeline (`downloads`, `integrity`, `cache`)

```text
validated artifact metadata (ArtifactSource)
        ↓ download (transport streams to staging, hashing while streaming)
untrusted staging file <managed-root>/cache/staging/download-<pid>-<ts>-<n>.part
        ↓ size check (early-fail when oversized; exact check at stream end)
        ↓ SHA-256 check against the canonical expected digest
        ↓ atomic promotion (rename into the store)
verified cache object <managed-root>/cache/artifacts/sha256/<digest>
```

- `downloads` is transport only: client construction, redirect policy, timeouts, status handling, and streamed transfer into a caller-provided staging file. It computes size and digest in the same single pass (never buffering whole artifacts) and refuses to return a staging path as "verified".
- `integrity` owns digest representation and verification: `ArtifactDigest` (canonical lowercase SHA-256, parsed from any casing; comparisons are byte comparisons of canonical data, never filename or metadata comparisons), `StreamingVerifier` (size + hash while streaming), and `verify_file` (re-validation of existing files).
- `cache` owns verified identity and promotion: content-addressed paths, staging naming, cache-hit validation, and the acquisition flow `ArtifactCache::acquire` implements.
- Staging names derive from process id, timestamp, and a process counter. Remote URLs, `Content-Disposition`, and any other response metadata never influence local paths, which are derived purely from the validated digest.

### Verified cache model

- Content-addressed by SHA-256, not by URL or filename: duplicate requests for the same digest naturally resolve to the same object. No sharding — one directory of uniform 64-hex names is clear and sufficient at launcher scale; no cache database, eviction, or size policy exists yet.
- Cache-hit policy: a file already at the verified path is never trusted because of its name. It is re-validated by hashing (and size when provided); only a passing validation is a cache hit. A corrupt object is replaced through a full verified re-acquisition (the replacement is promoted over it); an object that cannot even be read fails deliberately and is never destroyed on a guess.
- Promotion: the verified staging file is moved into the store with a rename. A partially written artifact can therefore never occupy a verified slot. If the rename fails (for example, a concurrent acquisition promoted the same digest first, or a reader briefly locked the file), the current destination is validated: a valid object wins the race and the acquisition becomes a cache hit; a proven-corrupt object is explicitly removed so the verified file can replace it; any unexpected validation error fails the acquisition without touching the destination.
- Concurrency: staging names are process-unique and promotion is a rename of a fully verified file, so simultaneous acquisitions of one digest can duplicate work but cannot corrupt the store. There is intentionally no lock file, queue, or coordination registry — the invariant is "never corrupt", not "never duplicate". Concurrent duplicate downloads are exercised by a deterministic test.

### Transport policy

- Production artifact URLs must use HTTPS. `ArtifactSource` re-validates every source at its own boundary (the release-manifest model separately validates HTTPS URL shape); cleartext HTTP is accepted only through the explicit `loopback_http_for_testing` constructor for `127.0.0.1`, `::1`, and `localhost` — the scoped test-transport path used by deterministic local tests. Embedded `user:password` URL credentials are rejected; artifact sizes must be positive when present.
- Redirects are followed up to a bounded limit (8) with a deliberate policy: an HTTPS request is never redirected to an insecure scheme, and a loopback-cleartext request is never redirected to a non-loopback insecure host (no loopback trampolines). Refusals surface as a structured `download_redirect_failure` with the specific reason.
- Connection setup is bounded (15 s) and reads are bounded by an idle timeout (30 s between body chunks), which suits large artifacts on slow links without imposing an overall deadline. There is no retry policy in this phase; each acquisition is one attempt.
- The HTTP client presents `aurora-launcher/<version>` as its user agent and honors system proxy configuration.

## Implemented in Phase 3

Phase 3 adds the Minecraft metadata-resolution layer: the launcher can resolve one exact Minecraft version from current official Mojang metadata into a deterministic, platform-aware `MinecraftInstallPlan`. It plans only — no client JAR, library, native, asset, runtime, Fabric, or Aurora artifact is downloaded, and nothing is installed, extracted, or executed.

### Resolution pipeline

```text
official version manifest (HTTPS discovery, no prior digest)
        ↓ exact version lookup (no fuzzy search, no aliases)
version document (manifest-provided URL, verified against manifest-provided SHA-1)
        ↓ parse + validate (external DTOs, `minecraft::metadata`)
platform-aware rule evaluation (`minecraft::rules`) + normalization (`minecraft::plan`)
        ↓
MinecraftInstallPlan (Aurora-owned domain types)
```

The composition lives in `minecraft::resolve_install_plan`; the plan is pure data with no filesystem mutation. Future installers consume the normalized plan and never traverse raw Mojang JSON.

### Metadata trust model

Official metadata splits into two classes, and the launcher never blurs them:

- **Bootstrap discovery metadata** — the version manifest (`https://piston-meta.mojang.com/mc/game/version_manifest_v2.json`). No prior digest exists before it is fetched; its trust is HTTPS transport plus deliberate parsing and validation. It is never called a verified artifact and never enters the content-addressed store.
- **Hash-addressable documents** — each manifest entry carries its version document's official SHA-1, so the version document is fetched from the manifest-provided URL and verified against the manifest-provided SHA-1 before parsing. A mismatch is a hard integrity failure.

This reuses Mojang's own integrity data instead of discarding it, without distorting the Phase 2 verified-artifact pipeline: metadata documents are small, buffered in memory under a hard 8 MiB cap, never staged, never promoted, and never persisted (fetch-on-demand only — no metadata cache, no SQLite, no background refresh). The metadata transport shares the artifact transport's client policy (user agent, timeouts, redirect rules, HTTPS-only production sources with loopback HTTP reserved for deterministic tests) but is a separate, narrower boundary because it must accept URLs without a prior SHA-256.

### External DTOs versus normalized plan

External Mojang shapes live in `minecraft::metadata` as DTOs covering only the fields resolution needs (manifest entries; the version document's identity, type, main class, Java requirement, asset index, client artifact, libraries, and modern `arguments`). Irrelevant fields (server jar, `logging`, timestamps, compliance levels, `minimumLauncherVersion`) are ignored. Legacy structures are parsed as detection sentinels and rejected deliberately: `inheritsFrom`, the `minecraftArguments` string, and library `natives`/`extract`/`classifiers` all fail as unsupported rather than being silently half-resolved.

The normalized plan in `minecraft::plan` is Aurora's own domain model:

```text
MinecraftInstallPlan
├── minecraft_version + version_type
├── java: JavaRequirement { component, major_version }
├── client: ArtifactRequirement { url, sha1, size_bytes }
├── asset_index: AssetIndexRequirement { id, artifact, total_size }
├── libraries: [PlannedLibrary { coordinate, path, kind, artifact }]
└── launch: LaunchMetadata { main_class, game_arguments, jvm_arguments }
```

A future installer can execute installation from this alone: every artifact carries its official URL, SHA-1, and exact size; every library carries a parsed Maven coordinate (`group:artifact:version[:classifier]`), its repository-relative layout path, and whether it is a platform library or a native artifact.

### Supported-version philosophy

Aurora targets modern Minecraft. The supported metadata is what current release and snapshot documents use: the structured `arguments` object, self-contained documents (no inheritance), and per-platform classifier libraries. Historical shapes — `old_beta`/`old_alpha` types, `minecraftArguments`, version inheritance, native-classifier downloads — are rejected deliberately with a dedicated unsupported error. Nothing is hardcoded to a version scheme: the launcher resolves whatever exact id the manifest lists (verified live against both `1.21.11`, a December 2025 release, and `26.2`, a June 2026 release under Mojang's newer year-based scheme). One documented reality of the official manifest: historical entries exist with spaces in their ids (for example `1.14.2 Pre-Release 4`), so listing-level validation checks presence and length while strict charset validation applies to the version the launcher requests.

### Artifact integrity representation

Mojang publishes SHA-1 values; Aurora's own distribution uses SHA-256. The `integrity` module gained `Sha1Digest` — canonical 40-hex parsing and a one-shot compute for bounded metadata documents — as a narrow extension next to the existing SHA-256 types. A SHA-1 value never masquerades as SHA-256 and never becomes a verified-cache identity: the store remains SHA-256-addressed for product artifacts. In Phase 3, the only cryptographically verified transfer is the version document (SHA-1 against the manifest value); the client/library/asset-index digests are *recorded* in the plan as official expectations for the phase that acquires them.

### Rule evaluation semantics

Rules are evaluated in `minecraft::rules`, pure and isolated from I/O and UI, unit-tested across Windows/Linux/macOS:

- Vocabulary: `os.name` ∈ {windows, linux, osx}, `os.arch` ∈ {x86, x86_64, arm64}, feature flags ∈ the six modern keys (`is_demo_user`, `has_custom_resolution`, `has_quick_plays_support`, `is_quick_play_singleplayer`, `is_quick_play_multiplayer`, `is_quick_play_realms`). Unknown vocabulary fails parsing deliberately.
- Semantics: last matching rule wins; an item with no rule list applies to everything; a rule list where nothing matches excludes the item.
- Planning evaluation resolves platform conditions now and keeps feature conditions unresolved (`IncludedIfFeatures`), so launch-time choices stay launch-time. A `disallow` rule carrying feature conditions cannot fire under the default no-features profile and does not decide planning.

Verified against live official metadata: library rules use `os.name` only; JVM argument rules additionally use `os.arch` (`x86`); game argument rules use features only.

### Libraries and natives

Modern metadata has no `natives`/`classifiers`/`extract` structures (verified for `1.21.11` and `26.2`): native artifacts are ordinary libraries whose coordinates carry a `natives-*` classifier and whose rules select the platform. The plan preserves document order (the deterministic classpath order), parses coordinates deliberately (3–4 segments, conservative charset — not a general Maven client), requires the declared repository path to equal the coordinate-derived Maven layout, and validates paths as safe relative forward-slash `.jar` paths before any future installer could place files under them. Platform filtering selects the applicable subset per platform; the native count is surfaced explicitly.

### Asset-index boundary

The plan records the asset-index requirement: index id, officially described document artifact (URL, SHA-1, size), and declared total size. The index itself is not fetched or enumerated in Phase 3 — acquiring the index document and its asset objects belongs to installation execution.

### Java requirement

The Phase 3 plan records the required runtime component name and major version (for example `java-runtime-epsilon` / 25 for `26.2`, `java-runtime-delta` / 21 for `1.21.11`). Phase 7 now consumes that boundary for managed provisioning; system-Java discovery and PATH modification remain absent.

### Unresolved launch arguments

Launch metadata keeps the main class and the ordered game/JVM argument groups for the planned platform with `${placeholder}` tokens verbatim (`${auth_player_name}`, `${auth_access_token}`, `${natives_directory}`, …). Placeholders are semantically unresolved: no fake tokens, no account/session substitution, no command-line construction, no shell escaping. Feature-conditioned arguments (demo mode, custom resolution, quick play) remain in the plan with their feature conditions attached instead of being dropped or forced.

### Error model

New stable command codes, mapped from structured internal errors and never exposing raw reqwest/Serde errors: `minecraft_version_invalid`, `minecraft_version_not_found`, `minecraft_manifest_invalid`, `minecraft_version_metadata_invalid`, `minecraft_metadata_network_failure`, `minecraft_metadata_integrity_failure`, `minecraft_version_unsupported`, `minecraft_library_invalid`, `minecraft_artifact_invalid`, and `minecraft_platform_unsupported`. Malformed external metadata and network failure are distinct categories.

### Development proof

The `plan_minecraft_install` command accepts one exact version string and returns a concise summary (version, Java requirement, library and native counts, asset-index and client resolution, main class, argument counts). The full plan stays native; the dev-only UI section (stripped from production like the Phase 2 proof) renders only the summary and never implies installation.

## Implemented in Phase 4

Phase 4 adds Fabric metadata resolution and install-plan composition: an exact Minecraft version + Fabric Loader version combination resolves from official Fabric Meta into a normalized `FabricPlan`, which composes with the vanilla `MinecraftInstallPlan` into a `GameInstallPlan`. It plans only — no Minecraft or Fabric library, client jar, intermediary, or loader artifact is downloaded, and nothing is installed, extracted, or launched.

### Resolution pipeline

```text
exact Minecraft version + exact Fabric Loader version
        ↓ loader version list (HTTPS discovery, no prior digest)
exact loader lookup (no "latest", no substitution)
        ↓ loader profile document (HTTPS, official Fabric Meta)
parse + validate (external DTOs, `fabric::metadata`)
normalization (`fabric::plan`)
        ↓
FabricPlan
        ↓ composition with MinecraftInstallPlan (`compose_game_plan`)
GameInstallPlan
```

The composition lives in `fabric::resolve_game_plan`, which chains the Phase 3 vanilla resolution, the Fabric resolution, and composition. The vanilla `MinecraftInstallPlan` is never mutated into a Fabric-modified shape: it remains an independently meaningful part of the composed plan, and composition is explicit containment (`GameInstallPlan` holds both plans plus derived views), so provenance survives for future diagnostics and repair.

### Official metadata and loader selection

Resolution consumes the current official Fabric Meta API (`https://meta.fabricmc.net/v2/`, verified live in September 2026): the loader version list (`/versions/loader`) and the per-combination profile document (`/versions/loader/:game/:loader`). Loader selection is exact by policy: the requested Loader version must exist in the official list (`fabric_loader_not_found` otherwise), and the Minecraft/Loader combination must be supported by official metadata — Fabric Meta answers HTTP 400 for an unsupported game version, which maps to `fabric_combination_unsupported`. Nothing substitutes a newer loader, selects "latest", or upgrades automatically; Aurora's future release manifest, not Fabric Meta, decides which Loader version Aurora requires.

The profile document embeds the loader's own launcher metadata (`launcherMeta`, published by the fabric-loader distribution on maven.fabricmc.net and served verbatim). External DTOs stay inside `fabric::metadata` and never become domain types; the pinned `launcherMeta` generation is 2, and a different generation is a deliberate unsupported error rather than a partial parse.

### Metadata trust model

Fabric Meta documents are bootstrap discovery metadata — the same trust class as Mojang's version manifest: no prior digest exists before the fetch, so trust is HTTPS transport plus deliberate parsing and validation, and the documents never enter the content-addressed store. The transport reuses the shared client policy (timeouts, bounded redirects, HTTPS-only production roots, loopback HTTP reserved for deterministic tests) through a separate narrow fetch boundary with the same 8 MiB buffering cap. Metadata is fetch-on-demand with no persistence, no cache, no background refresh.

### Normalized Fabric plan

```text
FabricPlan
├── minecraft_version + loader_version
├── main_class (the Fabric client entry point, KnotClient today)
├── libraries: [FabricLibrary { coordinate, repository, role, artifact }]
│   └── FabricArtifact { url, sha256?, size_bytes? }
└── min_java_major_version (the loader's declared floor)
```

Library order follows the official composition semantics, verified against fabric-meta's own profile builder and its `/profile/json` output: the loader's `common` libraries in document order, then the intermediary artifact (present only for obfuscated Minecraft versions — official metadata marks unobfuscated versions with the no-op placeholder `net.fabricmc:intermediary:0.0.0`, which contributes no library), then the Fabric Loader artifact itself, then the `client`-side group (empty in current metadata). Each entry records its role (`Common`/`Intermediary`/`Loader`/`ClientSide`), its parsed Maven coordinate, its validated repository base, and its derived artifact URL. The `development` and `server` library groups are deliberately ignored. The loader's `launcherMeta` contributes no launch arguments in the raw endpoint consumed here; the cosmetic `-DFabricMcEmu=…` JVM argument that the `/profile/json` endpoint synthesizes for the vanilla launcher is not composed into Aurora's plan.

### Maven artifact resolution (`fabric::maven`)

Fabric metadata names artifacts by Maven coordinate plus repository base rather than by fully described artifact URLs. The deterministic mapping is validated coordinate (`group:artifact:version[:classifier]`, conservative charset that includes the `+` official Fabric versions use, traversal-shaped segments rejected) plus validated repository base (HTTPS-only production, loopback HTTP for tests, clean directory URL) → repository-relative Maven layout path → artifact URL. This is a pure function over validated inputs — not a Maven client: no POM parsing, no dependency-graph traversal, no repository search.

### Artifact trust model

The trust difference between Mojang and Fabric artifacts is explicit. The loader's own libraries (the `common`/`client` groups) publish official digests — SHA-256, SHA-1, MD5, and SHA-512 with sizes — inside `launcherMeta`; Aurora records the SHA-256 (the strongest published digest, and the algorithm the verified cache is addressed by) and the size as pre-known expectations. The two artifacts Fabric Meta adds to every profile — the loader itself and the intermediary — carry no digest in the resolution metadata, and the plan represents them honestly as digest-less (`sha256: None`) rather than fabricating a value or borrowing another artifact's. A later installer must make a deliberate acquisition decision for digest-less Fabric artifacts (for example, pinning the SHA-256 computed on first verified acquisition); HTTPS success alone still never means verified. Mojang SHA-1 expectations and the SHA-256-addressed verified cache are unchanged, and no Fabric artifact is treated as a Phase 2 verified artifact by planning alone.

### Composed game plan

```text
GameInstallPlan
├── minecraft: MinecraftInstallPlan (unchanged vanilla requirements)
├── loader: FabricPlan (unchanged Fabric requirements)
├── libraries: [GameLibrary] (Mojang(…) | Fabric(…), deterministic order)
├── java: GameJavaRequirement
└── main_class: the final entry point
```

- **Main class**: the composed plan exposes the Fabric client entry point as the final main class while the vanilla plan retains Mojang's `net.minecraft.client.main.Main`; launch arguments, placeholders, and feature conditions remain exactly as Phase 3 resolved them (semantically unresolved).
- **Library order and collisions**: Mojang libraries in official document order, then Fabric libraries in official composition order. An exact duplicate coordinate collapses to one requirement (the earlier, Mojang-sourced entry wins). The same group, artifact, and classifier at different versions is a hard composition conflict (`fabric_plan_conflict`) — Aurora silently picks no winner. Different classifiers of one group and artifact coexist, which is how Mojang publishes platform natives.
- **Java**: Minecraft's runtime component and major version govern; the loader's declared floor is recorded, and the unobserved case of a loader floor above Mojang's requirement would normalize to the stricter major version with `raised_by_loader` set. No Java discovery, download, or selection exists.
- **Assets and client**: preserved unchanged through the vanilla half of the composed plan.

### Error model

New stable command codes, mapped from structured internal errors and never exposing raw reqwest/Serde errors: `fabric_loader_version_invalid`, `fabric_metadata_network_failure`, `fabric_metadata_invalid`, `fabric_metadata_unsupported`, `fabric_loader_not_found`, `fabric_combination_unsupported`, `fabric_library_invalid`, `fabric_repository_invalid`, and `fabric_plan_conflict`. Network failure, malformed external metadata, unsupported metadata semantics, unsupported combinations, invalid coordinates/repositories, and composition conflicts are distinct categories.

### Development proof

The `plan_fabric_install` command accepts one exact Minecraft version and one exact Fabric Loader version and returns a concise summary (both versions, vanilla/Fabric/final library counts, how many Fabric libraries carry official digests, the Java requirement and whether the loader raised it, and the final main class). The full composed plan stays native; the dev-only UI section renders only the summary and never implies installation.

## Implemented in Phase 5

Phase 5 adds the installation executor: a fully resolved `GameInstallPlan` plus a validated `InstanceId` materializes into a complete, isolated Minecraft + Fabric game under launcher-managed instance storage, committed with an Aurora-owned installed-state record and re-validatable from that record alone. It installs games — it never launches them, installs no Java runtime, installs no Aurora artifact, authenticates nothing, and never touches the user's `.minecraft`.

### Execution pipeline

```text
GameInstallPlan
      ↓ acquire per trust policy (three stores, below)
verified cache objects
      ↓ copy into instance staging + extract natives   (instances/<id>/.install-staging/game)
staged installation
      ↓ validate every staged file against its recorded trust
      ↓ write installed-state manifest LAST inside staging
      ↓ promote the staged tree onto game/ with one directory rename
InstalledGame (instances/<id>/game + installed-game.json)
```

The plan/execution boundary is preserved absolutely: the installer consumes only normalized Aurora plans, never raw Mojang/Fabric JSON, never decides versions, never re-evaluates rules. The single external document it must read beyond the plan — the asset index — is acquired and verified like any artifact first, then parsed by the DTO boundary in `minecraft::metadata` and normalized by `install::assets`; arbitrary asset-index JSON never flows through the executor.

### Atomicity and the staged/committed model

Nothing is ever written into the final `game/` directory directly. The complete tree, including the `installed-game.json` manifest that marks completion, is built under `.install-staging/` and becomes visible through one directory rename. Therefore:

- an interrupted installation always leaves the instance either without a game directory or with its previous complete one — a staging directory can never be mistaken for an installed game;
- stale staging from a failed attempt is removed by the next attempt, only after proving the path is exactly the derived `<instance-root>/.install-staging`;
- replacing an existing installation first moves the proven-managed previous `game/` into staging, then renames the new tree into place, then deletes the retired tree — if the process dies between the two renames the instance has *no* game directory (visibly incomplete, never a mixture) and the next attempt rebuilds from scratch;
- an existing `game/` **without** a valid manifest is a hard `installation_target_conflict`: Aurora neither wipes nor guesses at unrecognizable trees, and a malformed manifest is a deliberate `installation_state_invalid` error that is never overwritten.

### Instance filesystem layout

```text
instances/<id>/
├── game/                        # entirely launcher-managed, reconstructable
│   ├── installed-game.json      # the installed-state manifest (completion marker)
│   ├── versions/<mc>/client.jar
│   ├── versions/<mc>/<official logging file id>.xml
│   ├── libraries/<maven layout path>.jar      # Mojang + Fabric, classpath order preserved
│   ├── assets/indexes/<index id>.json
│   ├── assets/objects/<hh>/<sha1>
│   └── natives/<mc>/            # extracted native libraries
├── mods/                        # user-sensitive (never touched by installation)
├── config/                      # user-sensitive
├── logs/                        # user-sensitive
└── saves/, resourcepacks/, options.txt, …  # game-created user data
```

`game/` is the only launcher-managed, safe-to-rebuild subtree. Everything else under the instance root is user data: installation never reads, writes, or removes it, and the destructive paths of the executor (staging cleanup, retired-tree removal) operate only on the fixed derived staging path and manifest-proven game directories. Phase 9 implements the intended mapping: game/working directory = the instance root so vanilla-written `saves/`, `options.txt`, and resource packs land outside the managed tree, while classpath/assets/natives arguments point into `game/`.

### Installed-state manifest (`install::state`)

`installed-game.json` is a small versioned document (schema version 1, camelCase, human-inspectable) recording the Minecraft and Fabric Loader versions, an installation id and timestamp, the extracted natives directory, and every installed managed file with its logical role (`client`/`loggingConfig`/`library`/`nativeLibrary`/`assetIndex`/`assetObject`), its game-relative path, its size, and its trust record. Malformed documents and unknown schema versions fail deliberately and are never overwritten; validation is strict (safe relative paths, canonical digests per declared algorithm, positive sizes). The manifest exists so a future repair phase can validate and re-acquire every managed file from Aurora's own data without re-resolving external metadata. Repair is not implemented yet.

### Artifact trust model (three classes, never collapsed)

`integrity::ArtifactTrust` is the single honest representation, persisted per file in the manifest:

- **`ExpectedDigestVerified { sha256 }`** — a digest was known *before* acquisition from trusted metadata (Aurora's own distribution, Fabric's published loader-library digests) and the bytes verified against it.
- **`ExpectedDigestVerified { sha1 }`** — the bytes verified against an official Mojang SHA-1 (client jar, libraries, natives, asset index, asset objects, logging configuration).
- **`SecureTransportObserved { observed_sha256 }`** — no published digest existed (the Fabric loader and intermediary artifacts Fabric Meta adds to every profile); the artifact was acquired over the secure HTTPS transport and its SHA-256 was *computed locally* as a stable identity. This is transport trust plus observation, never expected-digest verification, and nothing ever reports it as such — not types, not diagnostics, not the manifest.

### Artifact stores (`cache`)

```text
cache/artifacts/
├── sha256/<digest>               # verified against a pre-known expected SHA-256
├── sha1/<digest>                 # verified against an official Mojang SHA-1
├── transport-observed/<digest>   # secure transport, locally observed SHA-256 identity
│   └── <digest>.json             # provenance sidecar: source URL → observed digest
└── ../staging/                   # untrusted in-flight downloads (all three pipelines)
```

- The SHA-256 store and its pipeline are unchanged from Phase 2 (cache-hit revalidation by re-hashing, corrupt-object replacement through full verified re-acquisition, rename promotion with race recovery, duplicate-friendly concurrency).
- The SHA-1 store mirrors that pipeline exactly with the digest comparison anchored in Mojang's SHA-1, so cache identity stays aligned with the externally expected digest. A SHA-1 value never masquerades as SHA-256 and never becomes a SHA-256-addressed identity.
- The transport-observed store addresses objects by their locally computed SHA-256. A provenance sidecar maps each source URL to the digest observed on first acquisition — an explicit TOFU-style local consistency reference: the *next* acquisition of the same URL compares its bytes against that observation, and content changing under a stable versioned URL fails deliberately (`fabric_artifact_unverified` after mapping). The sidecar is cache-internal reconstructable state (unparseable records grant no pin and no trust); documentation is explicit that the first acquisition was authenticated by HTTPS transport, not by an independent content digest, and that this is not equivalent security to an expected-digest verification. A full user-facing trust/pinning policy remains out of scope.
- Digest-less acquisitions are bounded (64 MiB transfer cap), streamed through staging like everything else, and never placed in a namespace whose semantics imply externally verified SHA-256.

### Assets

The plan's asset-index requirement (official URL, SHA-1, size) is acquired through the SHA-1 store; the verified document is parsed by the `AssetIndexObjectsDocument` DTO (flat `objects` map, every entry's SHA-1 and size validated — one malformed object invalidates the whole index) and normalized by `install::assets` into a deduplicated object set (identical hashes shared by several logical names collapse to one requirement; identity is the hash alone and the index's names never influence paths). Object URLs derive from the validated official root (`https://resources.download.minecraft.net/<hh>/<hash>`); objects are acquired through the SHA-1 store (naturally deduplicating across instances) and **copied** into `assets/objects/<hh>/<hash>` per instance. Copying (not symlinks or hard links) is the deliberate Phase 5 choice: symlinks need developer-mode/admin on Windows, hard links create shared-mutation coupling between the cache and instances, and correctness-before-I/O is the phase's rule; the disk cost is a documented, revisit-later tradeoff.

### Natives

Native artifacts are the planned `natives-*`-classifier libraries, verified against their official SHA-1 *before* extraction. `install::natives` extracts with the `zip` crate under strict rules: entry names must be relative, forward-slashed, traversal-free (`..`/`.`/empty/absolute/drive-letter/backslash rejected) and built from a conservative filename charset; targets are always joined onto the designated native staging root; `META-INF/**` entries are skipped (jar signature/metadata, matching the official launcher's long-standing extraction semantics, verified against real LWJGL native jars); duplicate file names — within an archive or across the planned native archives — are extracted once when identical and rejected as a hard conflict when they differ, so Aurora never guesses which duplicate wins (relevant to the multiple macOS `osx` native classifiers current metadata can select: exactly what the plan selected is materialized, nothing is arbitrarily deleted, and a genuine same-name conflict fails deliberately rather than guessing). Per-entry and per-archive uncompressed size bounds reject unreasonable content. Nothing extracted is executed or loaded.

### Logging configuration

Current version documents publish a client log4j2 XML configuration with an official SHA-1 (verified live: `26.2` ships `client-1.21.2.xml`). It is a genuinely required launch artifact, so the plan now records a narrow `LoggingRequirement` (official file id validated as a safe single-segment `.xml` name, URL, SHA-1, size; only the `log4j2-xml` type is supported and anything else is a deliberate unsupported error), and installation materializes it at `versions/<mc>/<file id>` with SHA-1 verification and records it in installed state. The launch-time JVM argument that references it is deliberately **not** constructed — that substitution stays a launch-phase concern.

### File-copy integrity and staged validation

Materialization copies only from verified store objects; each copy's byte count is checked immediately, and the staged-validation pass then re-hashes every staged file according to its recorded trust (SHA-1, SHA-256, or observed SHA-256) before the manifest is written. A successful copy API call is never itself treated as valid installed content. The commit-promotion path additionally checks the natives directory exists and is non-empty.

### Installation validation (`validate_installed_game`)

A deterministic, read-only, download-free validation of one instance's installed game against its own manifest: every recorded file must exist with its recorded size and re-verify per its trust type; the schema version must be supported; the completion record must be present. The outcome is `notInstalled`, `valid`, or `damaged` with a concrete problem list (missing file, size drift, digest drift, missing natives). Validation mutates nothing; it is the foundation for a future repair phase, which is not implemented.

### Concurrency

One installation may run per instance at a time (an in-process per-instance async mutex; a second attempt fails immediately with `installation_already_in_progress`). There is no global queue, no cross-process locking, and no download parallelism: installation is sequential by design — correctness first, bounded concurrency is a deliberate future decision. Artifact-store concurrency remains the Phase 2 "never corrupt, never coordinate" model.

### Progress

`InstallProgress` (phase, completed/total items, current logical item) is owned by Rust and reported through a callback; the dev-only command proof forwards it as `install-progress` events the frontend only displays. Progress never implies completion — completion is exclusively the committed installed-state manifest — and errors terminate the operation with structured codes.

### Error model

New stable command codes: `instance_id_invalid`, `installation_already_in_progress`, `installation_state_invalid`, `installation_target_conflict`, `minecraft_asset_index_invalid`, `minecraft_asset_invalid`, `native_archive_invalid`, `native_extraction_failure`, `artifact_materialization_failure`, `installation_validation_failure`, `installation_commit_failure`, and `fabric_artifact_unverified` (a digest-less Fabric artifact whose content drifted from its locally recorded observation). Acquisition failures keep their established transport/integrity/cache codes. Acquisition, integrity, malformed asset metadata, extraction, materialization, validation, and commit failures are distinct categories, each carrying developer context without exposing raw library errors.

## Implemented in Phase 6

Phase 6 turns the low-level installation capabilities into a persistent Aurora launcher instance: a writable instance registry, instance creation/rename/selection with referential integrity, an operational Aurora release model, SHA-256-verified Aurora client-artifact installation with its own installed-state record, complete read-only instance validation, and the first restrained production instance UI. An instance is complete and persistent — it is still not launchable: Java management, authentication, and process launching remain deferred, and instance deletion is deliberately unimplemented.

### Instance registry (writable, schema 2)

`launcher/instances.json` is now load-and-save. Schema 2 records carry the validated `InstanceId`, the display name (never a path), an explicit lifecycle `state` (`installing`/`ready`), and a concrete release pin (`channel`, `auroraVersion`, `minecraftVersion`, `fabricLoaderVersion`). Schema 1 — from before any code could write the registry — fails deliberately as unsupported; malformed registries are never overwritten. Saves are atomic (sibling temporary file plus rename), reject duplicate identifiers before touching disk, and registry read-modify-write windows are serialized by a process-wide mutex (long installations run outside it). "Channel-only" pins are deliberately unrepresentable: channels drift over time, installed state must identify the concrete release.

### Instance identity

Identifiers are generated, not user-chosen: opaque UUIDv4 in the canonical simple lowercase-hex form (32 characters, satisfying every `InstanceId` rule by construction), independent of display names and of timestamps alone. Generation re-checks uniqueness against the registry and the filesystem. The `uuid` crate (v4) is the one new dependency — a focused, universally maintained implementation rather than a custom identifier scheme.

### Creation, failure, and retry semantics (one coherent policy)

```text
resolve exact Aurora release
      ↓ allocate UUID, persist registry record with state = installing
resolve Minecraft + Fabric plans → Phase 5 game installer
      ↓ acquire + materialize Aurora artifact (SHA-256 verified)
complete read-only validation
      ↓ commit ready state (registry) → select if nothing is selected
ready instance
```

The `installing` record is persisted **before** the long installation runs, so a failure at any point — including a 15-minute game install dying midway — leaves an explicit non-ready record with its pinned release, never an ordinary-looking healthy instance and never a silent gap. Rollback never deletes anything (user data and managed trees alike are preserved for diagnosis); instead, `retry_instance_install` re-runs the installation for that same record, reusing Phase 5's staged-install recovery (a complete prior install is deliberately replaced; staging debris is recovered) and the verified caches, so already-valid artifacts are not re-downloaded. The record becomes `ready` only after complete validation passes, and nothing else moves it there. Progress is Rust-owned (`instance-progress` events) and composes the Phase 5 installer's item progress verbatim during the game-installation phase rather than duplicating it.

### Selection and referential integrity

The first successfully created instance is selected automatically when nothing is selected (explicit policy, tested); later creations never change an existing selection. Selection writes only identifiers that exist in the registry, and loading launcher state with a dangling stored selection is a deliberate error (`config_selected_instance_dangling`) — the launcher never silently selects a random instance and never repairs the selection behind the user's back.

### Aurora release model (operational) and release source

The Phase 1 release-manifest model is now operational: `ReleaseManifest::resolve_exact(version, channel)` selects exact releases — no "latest", no channel fallback, no substitution. **No production Aurora release infrastructure exists** (no manifest endpoint, no artifact repository), so the only operational source is the checked-in development fixture (`src-tauri/development/aurora-releases.json`, embedded via `include_str!`) whose artifact URLs pin a loopback development server the developer runs (`python -m http.server 8765` from that directory). The artifact-URL policy accepts HTTPS (production) and loopback cleartext (development/test) — the same policy every other transport boundary applies — and the UI labels the source as a development fixture; nothing implies production release discovery exists. Wiring a real HTTPS manifest source later is an additive change to this boundary. Manifest authenticity is stated honestly: an HTTPS-fetched manifest would be HTTPS-authenticated release metadata, not independently signature-verified — its SHA-256 values verify downloaded artifacts against it, and compromise of the manifest origin could replace both URL and digest together. No signing infrastructure exists, so none is invented.

The public Aurora Client `v2.1.1` release (ID 395572661) was checked on 2026-09-25: it is immutable and has no attached runtime JAR. GitHub's generated source archives are not runtime artifacts. Production publication therefore remains blocked. A future release can enter an intentional launcher manifest only after a public runtime JAR is anonymously downloadable, its embedded Fabric identity and Minecraft requirements match the release, and its byte size and SHA-256 are recorded. The manifest should be published at a stable HTTPS launcher/distribution location and point to the immutable Aurora-Client release asset; it must be updated last through a manual publication step. Commits, pushes, and CI build artifacts do not make a version launcher-visible. Existing development fixtures remain only development/test inputs. Installed instances continue to validate and launch from their recorded managed state without contacting a release source. The manifest model rejects duplicate Aurora versions so exact selection is unambiguous; production source discovery and update detection remain unimplemented.

### Aurora artifact installation and managed ownership

Aurora's client artifact always has a pre-known expected SHA-256 from release metadata, so acquisition uses the Phase 2 SHA-256 verified store unchanged — an Aurora artifact is always `ExpectedDigestVerified(sha256)`, never the transport-observed path reserved for digest-less Fabric artifacts; a release lacking a digest fails rather than weakening this. The artifact materializes at a deterministic launcher-managed path derived from validated release metadata (never a remote filename):

```text
instances/<id>/mods/aurora-<aurora version>.jar    # launcher-owned, repairable
```

The managed copy is size-checked and re-hashed against the expected digest before the Aurora installed-state record (`instances/<id>/aurora-installed.json`, schema 1, written atomically, committed only after the materialized artifact validates) records the concrete release facts and the artifact's path/size/SHA-256. Ownership is unambiguous: the launcher owns exactly the files its installed-state record names; every other file under `mods/` — and all user data — is user-owned, is never enumerated or removed by installation/update, and never counts as damage in validation. The record deliberately does not duplicate the game manifest; it is compared against the registry pin and the game manifest by complete validation.

### Complete instance validation (read-only)

`validate_instance` is the single deep, download-free, mutation-free check combining: registry presence; the Phase 5 game validation; the Aurora installed-state validation plus a SHA-256 re-hash of the materialized artifact; and cross-component consistency (registry pin ↔ Aurora installed state ↔ game manifest on Aurora/Minecraft/Fabric-Loader versions). The outcome is one of `ready`, `damaged` (with a concrete per-component problem list), `installing` (stored state; damage may exist underneath but readiness is not claimed), or `notInstalled`. The stored registry state is never treated as proof of health: `ready` is only reported when every component re-verifies. The stored `state` field is therefore not re-derived on every state load — deep validation hashes the whole installation and runs on demand.

### User-data boundary (unchanged, now enforced end to end)

`instances/<id>/game/` and the launcher-named Aurora artifact under `mods/` are launcher-managed and reconstructable. `mods/` (other files), `config/`, `logs/`, future `saves/`, screenshots, and resource packs are user data: creation, retry, rename, and validation never modify them, and rollback never deletes them.

### Production UI boundary

The production shell gains one restrained Instances panel: list (name, state, pinned versions), create (from the development release fixture, labeled as such), select, rename, retry (installing instances), and on-demand deep validation with per-problem output. Every displayed value is real Rust-owned state; progress comes only from native events. There is deliberately no launch control, no account/Java/mod-manager UI, and no instance deletion.

### Performance and storage posture (documented, deliberately unchanged)

Sequential game installation (~15 minutes cold for a modern version) and per-instance asset duplication remain as Phase 5 left them; instance creation reuses the verified caches so repeated creations of the same release are fast. Bounded download concurrency and hard-link asset sharing remain concrete future optimizations — they are not snuck into this phase.

## Implemented in Phase 7

Phase 7 provisions the reproducible Java runtime that Phase 9 launch consumes, without retroactively adding Java to the Phase 6 instance transaction. Minecraft/Fabric planning remains the authority for both `javaVersion.component` and the effective major version; the repeated Java major in Aurora's release fixture is now validated as a compatibility assertion and a mismatch fails deliberately.

### Official metadata and component resolution

Runtime discovery uses Mojang's current official product feed at `https://piston-meta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json`. The index is bootstrap metadata trusted through HTTPS plus strict parsing. Resolution selects exactly one entry for the exact Minecraft component and platform key; a missing, empty, or multi-entry component is an error, never a “latest” selection or component substitution. The selected index entry supplies the runtime build name/release date and the expected SHA-1/size/URL of its per-platform file manifest. That verified file manifest stays inside `runtime::metadata`; downstream code sees only `JavaRuntimePlan`.

Live research on 2026-09-15 confirmed `java-runtime-epsilon` for Windows x64 as Microsoft OpenJDK `25.0.1`, with a 152,664-byte manifest describing 82 directories and 411 raw files totaling 105,080,003 bytes. Mojang uses per-file delivery rather than an archive. Linux and macOS manifests additionally describe relative symbolic links; current Windows manifests contain none.

### Explicit platform mapping

The resolver accepts `RuntimePlatform { os, architecture }`; host detection occurs only at the application/lifecycle edge. Aurora maps normalized targets narrowly:

| Aurora target | Mojang key |
| --- | --- |
| Windows x86_64 / ARM64 / x86 | `windows-x64` / `windows-arm64` / `windows-x86` |
| Linux x86_64 / x86 | `linux` / `linux-i386` |
| macOS x86_64 / ARM64 | `mac-os` / `mac-os-arm64` |

Unmapped pairs (including Linux ARM64 and macOS x86 in the current feed vocabulary) fail before lookup. Availability is then checked independently, so a mapped platform with an empty component still fails. There is no architecture fallback.

### Runtime plan, integrity, and executable semantics

`JavaRuntimePlan` records the exact component, required major, platform and Mojang key, runtime build/release identity, manifest SHA-1, every normalized directory/file/link entry, and deterministic launch/diagnostic executable paths. Windows launching uses `bin/javaw.exe` and diagnostics use `bin/java.exe`; Linux uses `bin/java`; macOS uses `jre.bundle/Contents/Home/bin/java`. These paths must be executable file entries in the official manifest—no recursive filename search occurs.

Mojang publishes SHA-1 for the file manifest and every raw runtime file, plus exact sizes. Both flow through the existing SHA-1 acquisition/store and are `ExpectedDigestVerified(SHA-1)` facts; no SHA-256 is invented and HTTPS is not relabeled as content verification. LZMA alternatives in current metadata are deliberately not used: the raw entries already carry expected SHA-1 and avoid a new decompression dependency or trust path.

### Shared layout and transaction

One exact runtime is shared by every compatible instance:

```text
<managed-root>/runtimes/<component>/<mojang-platform>-<manifest-sha1>/
├── ... official runtime tree ...
└── runtime-installed.json
```

The installer validates any exact existing runtime before acquisition and reuses it with no runtime-file network request when valid. Otherwise it sequentially acquires files into the shared SHA-1 store (bounded concurrency of one), materializes only verified cache objects under the exact derived `.installing-<identity>/runtime` tree, preserves Unix executable bits, creates validated relative Unix links, validates the staged tree, writes `runtime-installed.json` last, and promotes by directory rename. Current Windows metadata has no links; encountering one on Windows fails rather than requiring developer mode/admin or guessing file-versus-directory semantics.

Stale exact staging is safely rebuilt. A valid state matching the exact final path proves a damaged runtime is launcher-owned and permits transactional replacement (old tree moved inside staging, restored if promotion fails). A final tree with no state, malformed state, an unknown schema, or mismatched identity is a hard conflict and is never overwritten. No garbage collection is implemented.

### Runtime state and validation

`runtime-installed.json` schema 1 records identity, component, required major, normalized OS/architecture and Mojang key, runtime build/release, manifest SHA-1, relative launch/diagnostic executables, and every managed directory/file/link with the minimum integrity metadata needed offline. Loading is strict; unknown schemas and malformed paths, digests, duplicates, or entry-kind combinations fail deliberately.

Validation is read-only and network-free once the plan is resolved. It proves the state describes the complete exact plan (an edited subset cannot pass), re-hashes every file by SHA-1 with size checks, verifies directories and link targets without following them, checks executable semantics, and optionally invokes the absolute managed `java` path with the single `-version` argument. The process receives a cleared/minimal environment, has a ten-second timeout with kill-on-drop, captures bounded sanitized output, requires successful exit, and confirms the reported modern/legacy-style major matches the plan. No shell or Minecraft process is involved.

### Instance relationship, UI, and concurrency

`ensure_instance_runtime(instance_id)` first requires Phase 6 content validation to be ready, resolves the instance's pinned release back through official Minecraft/Fabric planning, checks the release fixture's repeated Java major, resolves the exact runtime, reuses or installs it, validates and executes it, then returns a typed summary. Runtime absence/damage never mutates the instance registry and never makes otherwise valid instance content “damaged.” The production UI displays the selected instance as “content ready” with an independent Java `missing`/`damaged`/`ready` status and offers only Check/Install/Repair Java actions—no paths, JVM flags, memory settings, or system-Java picker.

One async mutex per runtime identity prevents concurrent mutation within this launcher process; a competing ensure fails immediately. Cross-process coordination remains deferred, matching the existing instance-install limitation.

## Implemented in Phase 8

Phase 8 adds Microsoft → Xbox → Minecraft authentication: public-client OAuth in the system browser with PKCE, the full Xbox/XSTS/Minecraft Services exchange chain, entitlement and profile validation, OS-backed storage for the one persisted secret, session restoration with refresh-token rotation, sign-out, and a restrained production account UI. It authenticates — it never launches Minecraft, collects no passwords, and touches neither instances nor Java runtimes.

### Public-client OAuth architecture

Aurora is a desktop **public client**: no client secret exists anywhere in the launcher, now or ever. Sign-in uses the Microsoft identity platform v2.0 **authorization-code + PKCE (S256)** flow on the consumer tenant (`https://login.microsoftonline.com/consumers/…` — Xbox authentication cannot consume work/school tokens), with the scope set `XboxLive.signin offline_access`. The authorization request is built in Rust from OS entropy (`getrandom`): a random 32-byte `state` and a 43-character verifier whose S256 challenge is sent with `code_challenge_method=S256`; `response_mode=query` and a fixed parameter set — no secrets, no prompt manipulation, no frontend-supplied endpoints.

The user authenticates in the **system browser** (opened through the official Tauri opener plugin; no embedded webview, no cookie persistence by Aurora, no page scraping or script injection). The redirect target is the documented native-app loopback pattern: a one-shot `TcpListener` bound to `127.0.0.1` on an ephemeral port, advertised as `http://localhost:<port>/` — the registered redirect's root path plus the dynamically selected port. Loopback redirect matching follows RFC 8252: the port is ignored when matching a registered `http://localhost` redirect, but the path is compared exactly, so the URI carries the root path and no path segment (a `/callback` suffix against the bare registration is rejected by Microsoft as `invalid_request`, a defect found and fixed during first live verification). The receiver accepts exactly that root path, answers with a fixed page (never reflecting request data), **ignores mismatched-state callbacks** (stale tabs must neither complete nor kill a fresh login) and non-root requests, and dies with its listener at the first decisive callback, on provider error, on cancellation, or on timeout — no listener ever survives its flow.

**The Aurora Client Microsoft application registration exists.** The application (client) ID is public application configuration — Aurora is a desktop public client and no client secret exists anywhere — so the approved ID is committed in `auth::flow` and every normal development or official build carries it without manual setup. The optional `AURORA_MICROSOFT_CLIENT_ID` build-environment variable overrides the committed value for fork/CI builds; when neither is present, `begin_microsoft_login` fails deliberately with `auth_configuration_missing`. Aurora will never borrow another launcher's client ID. Microsoft requires new application registrations to complete its Minecraft Services AppID review/allowlisting (the `aka.ms/mce-reviewappid` review) before `api.minecraftservices.com` accepts their tokens — an unapproved app receives HTTP 403 "Invalid app registration"; the Aurora Client review is complete. The registered platform redirect is `http://localhost`: the loopback receiver above supplies the ephemeral port and carries the registration's root path.

### Authentication chain

```text
system browser → loopback code+state
  ↓ POST token endpoint (form: grant, client_id, code_verifier, redirect_uri — never a secret)
Microsoft access + refresh tokens
  ↓ POST user.auth.xboxlive.com/user/authenticate (RPS ticket "d=<token>")
Xbox token + user hash
  ↓ POST xsts.auth.xboxlive.com/xsts/authorize (SandboxId RETAIL, RelyingParty rp://api.minecraftservices.com/)
XSTS token + user hash
  ↓ POST api.minecraftservices.com/authentication/login_with_xbox (identityToken "XBL3.0 x=<uhs>;<xsts>")
Minecraft access token (~24 h)
  ↓ GET /entitlements/mcstore (Bearer)
ownership confirmed (product_minecraft or game_minecraft)
  ↓ GET /minecraft/profile (Bearer)
profile UUID + name → Aurora account
```

Endpoint behavior was verified against current Microsoft identity-platform documentation and the community-documented launcher flow (wiki.vg's successor page on minecraft.wiki) in September 2026. All external DTOs live in `auth::metadata` behind a dedicated bounded POST/GET boundary (256 KiB response cap, no redirects, shared launcher user agent, strict parsing; parse failures never quote response bodies). Entitlement distinguishes *authenticated* from *entitled*: an account without a Java-ownership item is `auth_entitlement_missing`, and an account that owns Minecraft but has no profile yet is `auth_profile_missing`. XSTS denials carry the numeric `XErr` code; only evidence-backed codes map to user-understandable explanations (no Xbox profile, region unavailable, Korean adult verification, child/family restriction, ban), and unknown codes stay generic — account conditions are never guessed.

### Account model, secrets, and persistence

The account identifier is deliberately the Minecraft profile UUID in canonical undashed lowercase-hex form: stable for the account's lifetime, non-secret, and it makes re-signing a known account an update rather than a duplicate. Non-secret summaries (`launcher/accounts.json`, schema 1: account id, Minecraft name, selection) follow the established document conventions exactly — camelCase pretty JSON, atomic sibling-plus-rename writes, malformed/unsupported files never overwritten, dangling selections rejected (`accounts_selected_dangling`; the first successfully added account is selected automatically and later sign-ins never move the selection — both tested policies). Accounts and instances are separate launcher concepts; nothing binds them in this phase.

**The only persisted secret is the Microsoft refresh credential** (refresh token + client ID), stored through the `CredentialStore` boundary in the **real Windows Credential Manager** (generic credentials via `windows-sys`: `CredWriteW`/`CredReadW`/`CredDeleteW`, entries named `com.aurora.launcher/account/<uuid>`). Everything else — the Microsoft access token, Xbox token, XSTS token, and Minecraft token — is memory-only and regenerated from the refresh credential. macOS/Linux have no backed store implemented yet: the interface is portable but those platforms fail deliberately with `auth_credential_store_failure` rather than falling back to plaintext token files (the `keyring` crate was evaluated and rejected because a feature-less build silently substitutes an in-memory mock store). Credential-store entries and account records stay referentially consistent: a record without a credential reads as `reauthenticationRequired`, an orphaned credential is inert until the same account signs in again.

Secret hygiene is enforced by the `SecretString` wrapper (redacted `Debug`/`Display`, narrow `expose()`); tokens never appear in logs, events, error messages, DTOs, or ordinary JSON, and tests prove representative error/debug output and the accounts document contain no fixture token values.

### Sessions, refresh, sign-out

A `MinecraftSession` (account, profile, Minecraft token, expiry) lives in a process-wide Rust-owned cache — the token is exposed only to native authentication transport and Phase 9 launch assembly, never to the frontend. Expiry is honored with a five-minute skew margin. Restoration is on demand (`refresh_account_session`): a usable cached session short-circuits; otherwise the refresh credential is redeemed and — because the identity platform returns a replacement refresh token on every redemption — the rotated credential is persisted *immediately*, before the downstream chain runs. An `invalid_grant` rejection deletes the provably-dead credential and transitions the account to `auth_reauthentication_required`; a credential that resolves to a different Minecraft account is likewise removed. Sign-out removes the credential, the record, the cached session, and the selection when it pointed at the account; its wording is honest — removing the account from Aurora, not revoking the Microsoft session globally.

One login transaction is allowed per process (a second attempt fails with `auth_login_in_progress`); it is cancellable and time-bounded (default ten minutes), cancellation/timeout discard all transient state (PKCE, state, receiver), and late browser callbacks cannot resurrect a finished flow. Cross-process login coordination is deferred.

### Tauri boundary and UI

Commands: `get_accounts`, `begin_microsoft_login`, `cancel_microsoft_login`, `select_account`, `remove_account`, `refresh_account_session`. No command or event ever carries a token; progress events carry only a phase string. The production Accounts screen signs in through the system browser, lists accounts with derived status, selects, checks sessions, and removes accounts — no email display, no skin/avatar/cosmetic features, no password fields, and no raw profile UUIDs. Known authentication failures are translated into concise actionable language; internal structured error codes never appear as ordinary user-facing content there (they remain in logs and the development-only Developer screen). A build with no registration (a fork that blanks the committed client ID and sets no override) still surfaces the honest `auth_configuration_missing` condition instead of pretending to sign in.

### Live verification status

All authentication behavior is verified by the deterministic offline suite (synthetic chain servers, a scripted browser driving the real loopback receiver, an in-memory credential store, and a real Windows Credential Manager round-trip test). The production flow was then verified **live** against the real services (September 2026): Microsoft browser authorization, the loopback callback, PKCE/state validation, the full Microsoft → Xbox Live → XSTS → Minecraft Services exchange chain, entitlement/ownership confirmation, profile retrieval, the accounts UI integration, and secure persistence across an application restart all succeeded; credential storage and secret-redaction audits confirmed no secret was logged or persisted in plaintext. First live verification exposed the loopback redirect defect described above — a `/callback` suffix against the root-path registration — which was fixed and covered by new tests. Live coverage extends to the authentication chain; it does not yet include a real Minecraft game launch.

## Configurable instances

Instances are now genuinely configurable: each record carries the user's **desired configuration** alongside the concrete release pin that installed state must match, and launch-only settings reach the real Java process.

### Desired configuration versus installed state

Three conceptual categories, deliberately kept apart:

- **Desired user configuration** (`instances::settings::InstanceConfiguration`, stored on every registry record): the Minecraft version, the mod-loader kind and version policy, the memory allocation in MiB, additional JVM arguments as raw text, and an optional windowed resolution. This is what the user edits.
- **Resolved / installed state**: the concrete release pin (`PinnedRelease`: channel, Aurora/Minecraft/Fabric-Loader versions), the game installed-state manifest, the Aurora installed-state record, and the validated managed runtime. The pin is what installation actually produced; readiness compares desired configuration against it.
- **Transient runtime state**: the supervised child process, cached sessions, and progress events — never persisted configuration.

The desired configuration never edits installed facts and vice versa: memory, JVM arguments, window, and display name are launch-only and never invalidate content; the Minecraft version and a pinned loader version are install-affecting.

### Registry schema 3 and migration

`launcher/instances.json` schema 3 added the per-record `configuration`. Schema 2 files — the pre-configuration shape — migrate deterministically on load: identifiers, display names, lifecycle states, release pins, and the selected instance all survive; each migrated configuration is derived from its record's pin (same Minecraft version, the installed loader pinned, default memory/JVM arguments/window). Schema 1 still fails deliberately as unsupported, malformed files are never overwritten, and saving a migrated registry persists schema 3. Serialized old-schema fixtures are covered by tests.

### Loader model

`LoaderKind` is an explicit enum (`Fabric` today) so future kinds are additive, and `LoaderPolicy` is either `Automatic` (resolve the newest *stable* loader from official Fabric Meta's per-game list at install time — Fabric Meta publishes no "recommended" concept, only newest-first versions with `stable` markers) or `Pinned { version }`. Only loader kinds with a genuinely implemented pipeline are representable; **vanilla is deliberately deferred** because the composed plan, installer, and launch assembly are Fabric-direct (`GameInstallPlan` embeds a `FabricPlan`; installed state requires a loader version) — supporting vanilla means a real parallel composition path, not a flag. The automatic policy is an explicit user choice, not silent substitution: the resolved version is pinned into installed state and never drifts afterward; only a deliberate re-installation re-resolves it. A pinned version absent from the official list, or a Minecraft/Loader combination Fabric Meta rejects, is a clear error, never a fallback.

### Minecraft version selection and Aurora release compatibility

Version selection consumes the same pinned official Mojang manifest through the `list_minecraft_versions` command (releases by default, snapshots only behind an explicit filter; historical types stay excluded). Game configuration and Aurora release compatibility remain separate concepts: the lifecycle resolves the Aurora release whose Minecraft version matches the configured one (preferring the most stable channel when several match), and a configuration no release supports is rejected up front with the supported versions listed — the development fixture truthfully limits live combinations today. Changing the Minecraft version (or pinning a different loader) saves immediately but leaves the previous installation intact and **stale**: deep validation reports `stale`, Home shows an actionable blocker, and Play stays unavailable until `install_instance_configuration` deliberately installs the new configuration through the normal staged/verified/atomic pipeline. A failed configuration install mutates nothing if resolution fails, and leaves the usual retryable installing record once installation has begun.

### Memory and JVM argument policy

Memory is stored in whole MiB with deliberate bounds (default **2048**, minimum **512**, maximum **32768** — documented product decisions, not host-dependent probes). Aurora owns the heap: the configured memory generates exactly one authoritative `-Xmx<mib>m` JVM argument, emitted first in the argument vector. Custom JVM arguments may not carry heap flags (`-Xmx`, `-Xms`, `-Xmn`, their `-XX:`/long-option spellings) or classpath/executable flags (`-cp`, `-classpath`, `--class-path`, `-jar`) — a conflict is rejected with an explanation at configuration save and re-checked at launch assembly, so no duplicate or ambiguous heap configuration can exist. Aurora deliberately does not set `-Xms`: letting the JVM size the initial heap itself avoids overcommit; setting it "because launchers do" would be convention, not a requirement.

Additional JVM arguments persist as the user's raw text and parse into an argument vector at each trust boundary with one documented rule set (module documentation of `instances::settings`): whitespace separates tokens, double quotes group text, backslash runs before a quote follow Windows command-line rules, and an unterminated quote is malformed. Arguments flow into the structured process Command API as individual strings — no shell is ever involved, and executable, main class, classpath, natives, logging, and session arguments remain launcher/Mojang-owned. Final JVM order is deterministic: launcher heap argument, Mojang-planned JVM arguments (including logging), then the user's additional arguments last.

### Display options

An optional windowed resolution maps onto Minecraft's own `has_custom_resolution` launch-argument feature (the official `--width`/`--height` argument groups) with the values substituted at launch assembly; `null` keeps Minecraft's default behavior. **Fullscreen is deliberately deferred**: it lives in the instance's `options.txt`, which is user game data this launcher does not manage — implementing it honestly requires a settings-file subsystem that does not exist.

### Java and custom Java

The managed Mojang runtime model is unchanged and remains the only Java source: the required major derives from the resolved game plan, `ensure_instance_runtime` provisions the exact runtime, and the instance detail displays the required major and runtime state. Changing the Minecraft version naturally changes the required runtime once the new configuration is installed. **Custom Java executables are deliberately deferred**: launch assembly requires the validated executable inside managed runtimes storage, and admitting external executables would weaken that validation for a checkbox — system-Java selection remains an explicit future decision.

### Instance UI and save/apply semantics

Creation asks for the essentials only (name, Minecraft version, loader policy) and resolves everything else safely. The Instances page is a list plus a per-instance detail editor grouped General / Performance / Java / Display / Advanced; editing works on an explicit draft with one **Save changes** action that validates and persists the whole proposed configuration atomically, so related install-affecting changes are never applied half-edited. Install-affecting changes additionally surface an **Install new configuration** action. Home remains the Play surface and reflects stale/not-ready state immediately after any change; account state is independent.

## Launch pipeline

Phase 9 implements the native launch domain as three narrow parts: `launch::state` owns the non-secret Play-readiness decision, `launch::resolve` converts normalized verified state into a redacting `LaunchSpec`, and `launch::process` consumes only that spec to spawn and supervise the exact child. The process layer has no Mojang/Fabric/auth DTO knowledge and cannot accept an executable, path, or arbitrary JVM argument from Svelte.

### Preconditions and readiness authority

`get_play_readiness` returns one normalized DTO for a selected instance/account pair. Rust requires a present registry record in lifecycle `ready`, complete read-only instance validation (game, Aurora state/artifact, and version consistency), the exact resolved managed runtime with valid state and files, a known account with a usable cached session or refresh credential, and no `Starting`/`Running` child for that instance. The Svelte button displays this decision and stable blockers; it does not reconstruct security-critical readiness from unrelated UI fields. `play_instance` repeats the authoritative checks and additionally executes the bounded Java-version diagnostic before spawn, so stale frontend state cannot authorize launch.

Because Java requirements and the full normalized argument plan are not persisted today, readiness/Play re-resolve the exact pinned Minecraft and Fabric metadata through the existing official planners. Product artifacts are neither re-downloaded nor reinstalled, and installed files remain the execution source. A broad metadata-persistence/cache policy is deferred; consequently a cold Play currently needs official metadata services even when all product bytes are installed.

### LaunchSpec and arguments

`LaunchPlan` is a launch-owned view derived from the composed `GameInstallPlan`; it preserves the Fabric final main class, ordered classpath libraries, Mojang game/JVM argument groups and feature requirements, version/asset identity, and normalized logging argument. Demo and quick-play features stay false; the `has_custom_resolution` feature is enabled exactly when the instance's desired configuration carries a windowed resolution, substituting the official `--width`/`--height` arguments. Unknown, malformed, oversized, NUL-containing, or still-unresolved placeholders fail closed.

Supported current placeholders are player name, version name/type, isolated game directory, assets root/index, profile UUID, Minecraft access token, user type/properties, natives/library directories, launcher name/version, classpath/separator, logging path, the windowed-resolution values, and the current optional `clientid`/`auth_xuid`. Minecraft Services does not provide the latter two identifiers through Aurora's current identity model, so their required metadata values are honestly empty rather than fabricated. Arguments remain individual strings; spaces, Unicode, and quotes receive no shell escaping because no shell is involved. The instance's desired configuration supplies the launch-only process options through `LaunchOptions`: the authoritative heap argument (`-Xmx<mib>m`, first), Mojang's planned JVM arguments, and the parsed additional JVM arguments last — Fabric's cosmetic `-DFabricMcEmu` profile property stays absent.

The working/game directory is the instance root, not its managed `game/` subtree and never `.minecraft`. This lets Fabric discover `mods/aurora-<version>.jar` and user-added mods naturally and places saves, options, resource packs, configuration, and game output with that isolated instance. Launch never enumerates user mods and classpath construction never includes them.

### Classpath, assets, logging, and natives

Classpath order is the composed plan order (Mojang ordinary libraries, then Fabric libraries, exact duplicates already collapsed) followed by the Minecraft client. Mojang native classifier jars are extraction inputs and are excluded. Every path must have the matching installed-manifest role, exist, and remain canonically inside the validated game tree; no directory scan or Maven re-resolution occurs. The asset root and named installed asset index are similarly required.

Phase 3 logging normalization retains Mojang's validated one-`${path}` JVM argument template alongside the SHA-1-verified XML file. Launch substitutes the installed config path and never downloads or hardcodes a version-specific logging argument during spawn.

Current LWJGL 3 source resolves bundled natives beneath a common native root by OS and architecture itself (`Platform.mapLibraryPathBundled` adds its platform/architecture path; `Library` consults the configured paths). Current 26.2 metadata also supplies component-specific subdirectories such as `/java`, `/jna`, `/lwjgl`, and `/netty`. Aurora therefore passes the validated common extracted root to `${natives_directory}` and preserves Mojang's suffixes; it does not select an `x64` leaf or fall back between architectures. The native root comes only from `installed-game.json`, must be a real non-empty directory, and is canonical-containment checked beneath `game/`.

### Authentication and process supervision

The Minecraft token moves from the Rust-only `MinecraftSession` directly into one sensitive structured child argument. `LaunchSpec` is not serializable, its `Debug` output redacts sensitive arguments, production DTOs/events contain no argument arrays, and supervised stdout/stderr are bounded and scrubbed for exact secret values before an instance-local `logs/aurora-launch-*.log` is written. Passing the token in the OS-visible child argument vector is unavoidable Minecraft protocol exposure; it is not treated as storage and is never copied to ordinary launcher persistence or UI.

`tokio::process::Command` receives the absolute validated managed Java executable, individual argument strings, the instance-root working directory, null stdin, and piped stdout/stderr. The normal host environment is inherited for graphics/audio compatibility, but `CLASSPATH`, `JAVA_TOOL_OPTIONS`, `_JAVA_OPTIONS`, and `JDK_JAVA_OPTIONS` are removed so ambient Java injection cannot change the launch. No shell or reconstructed command line exists.

The process-local state machine is `Stopped → Starting → Running → Exited|Failed`; it records instance id, live PID, start time, exit code when available, and a sanitized message. A per-instance map refuses a second Starting/Running launch while allowing different instances. The exact returned child handle is awaited, never rediscovered by process name. Launcher lifetime remains active, Stop is deferred, normal/non-zero exit is reported without automatic repair or relaunch, and cross-process duplicate prevention is not implemented.

The production Play UI shows readiness, the selected Minecraft name, restrained native phases, Running/exit state, and actionable blockers. The authentication prerequisite for Play is cleared and live-verified (see Phase 8); the remaining v0.1 integration dependency for a real playable milestone is the production Aurora release artifact, which today comes from the checked-in development fixture. Synthetic sessions validate assembly and harmless test executables validate spawning, but neither is represented as a real sign-in or Minecraft launch.

## Launcher appearance customization

The launcher's visual language is a theme system: semantic design tokens (defined once as CSS custom properties in `src/app.css`) are the contract every component consumes, and themes/accent selections are attribute-scoped token sets applied to the document root — never per-page styles. The approved Aurora look is the default; customization changes palettes only, never layout or hierarchy.

### Ownership and persistence

Appearance is launcher-wide state owned by the launcher configuration (`config`, schema 2). It is never per-instance. The persisted shape:

```json
{
  "schemaVersion": 2,
  "selectedInstanceId": null,
  "appearance": {
    "theme": "aurora-dark",
    "accent": { "type": "preset", "id": "violet" }
  }
}
```

- Schema 1 (the pre-appearance shape) migrates deterministically on load: the selection survives and the appearance defaults; the file persists as schema 2 on its next save. Unknown schema versions still fail deliberately, and structurally malformed files are still `config_malformed` and never overwritten.
- Within a known schema, unknown theme ids and unusable accent colors normalize to the default look at load — appearance is cosmetic launcher-wide state and never blocks startup. A user *request* carrying an invalid value is rejected up front with `appearance_invalid` and nothing is written.
- Writes go through the same atomic sibling-plus-rename discipline as the rest of the configuration.

### Built-in themes

Three curated themes, all dark, differing only through a coherent surface palette: `aurora-dark` (the approved default: neutral near-black surfaces, restrained violet), `midnight` (cooler, deeper graphite/slate), and `oled` (true-black major surfaces). Theme ids are stable compatibility vocabulary (also the CSS `data-theme` values). There is deliberately no "follow system" mode: Aurora's design is fundamentally dark, no light theme exists, and a selector that always resolves to dark would be dishonest. Status colors (success/warning/error/working) are semantically stable across every theme — a green Ready never becomes violet because the accent is violet.

### Accent customization

The accent recolors only selection, keyboard focus, selected-state markers, and primary actions. The selection is either a curated preset (`violet`, `blue`, `cyan`, `green`, `amber`, `rose`, `neutral`) or a custom `#rrggbb` color; it never recolors status colors, ordinary text, or surfaces. Each accent resolves to the full accent-family token set (base, primary-button fill, hover, pressed, on-accent text, selected-background tint, outline tint):

- Presets carry hand-designed palettes, verified by deterministic tests: the base stays visible on true black (≥ 3:1), and the button fill keeps ≥ 4.5:1 contrast with its text color in resting, hover, and pressed states.
- Custom colors run a deterministic Rust derivation (`appearance::derive_custom_accent`): the base must parse and stay visible on dark surfaces (relative luminance ≥ 0.08 — darker requests are rejected with an explanation, never silently brightened); the button fill keeps hue/saturation with lightness clamped into a solid-fill band; text polarity is white or ink, whichever reaches ≥ 4.5:1, with step-darkening for mid-luminance fills; hover/pressed shift by fixed lightness deltas in the contrast-safe direction. There is no arbitrary user CSS, no style strings, and no frontend-side color math — the frontend applies exactly the token values Rust derived.

### Commands, application, and startup

`get_appearance` returns the current selection, the theme/accent catalogs, and the derived palette; `set_appearance` validates the whole proposed appearance first (rejecting unknown themes and unusable accents with `appearance_invalid`), then persists atomically. The frontend appearance store applies the returned values live (root `data-theme`/`data-accent` attributes for presets, derived inline custom properties for a custom accent) — no restart, no rerender of application state. At startup the appearance loads in the SvelteKit root layout `load` (awaited before any component mounts), and `app.html` carries an inline dark pre-paint guard, so the persisted look is in place for the first shell render; the pre-JS paint is dark under every theme. A failure loading appearance falls back to the default look without blocking the launcher (the Settings page surfaces the error).

### Accessibility constraints

Theme customization must not reduce accessibility: keyboard focus stays visible (the 2 px accent outline follows the accent under every theme), selection is never communicated by color alone (text markers, checked radios, and ✓ labels in Settings), status meaning is never color-only (dot + label badges), and `prefers-reduced-motion` disables non-essential animation exactly as before. Contrast for every built-in theme's text/surface pairs and every accent preset's button/text pairs is enforced by the deterministic test suite, not by visual judgment. Hairline borders are deliberately quiet (~1.2:1 against surfaces) per the design language — separation is redundant with the fill/shadow hierarchy and carries no meaning alone. A launcher-owned animation preference is deferred: the OS `prefers-reduced-motion` preference is respected unconditionally and could only be made more restrictive, so no setting ships yet.

## External application identity

Aurora has two deliberately separate canonical artwork roles. The **internal UI mark** is the transparent white Aurora mark (`static/aurora-icon.png`, the byte-for-byte developer artwork, SHA-256 `faba1af24b964cfd085505c63e9e43d7abedebd70d70794e841a3251d0fa5a19`): it renders directly on dark surfaces inside the launcher shell (the sidebar brand) and the Aurora client, and it is never regenerated, redrawn, or given a background. The **external application icon** is the OS-facing identity — Windows executable, taskbar, Alt+Tab, title bar, installer, shortcuts, Start menu — realized as the exact internal mark composited onto a rounded-square dark neutral vertical gradient: `static/aurora-app-icon.png`, a 1024×1024 canonical master.

### Deterministic generation

The external master and every platform derivative are produced by one repository-owned development tool, `tools/generate_external_icon.py` (Python + Pillow; the launcher never generates its icon at runtime). Given the same canonical transparent mark it is byte-for-byte reproducible. Exact parameters:

- canvas 1024×1024; corner radius 225 px (≈22%, a restrained modern rounded square);
- vertical linear sRGB gradient, top `rgb(38,39,42)` (dark charcoal) → bottom `rgb(5,5,6)` (near-black) — lightest at the top, monochrome-neutral, no tint, no dithering;
- the internal mark composited at 66% of canvas height, alpha-bbox centered, with its exact source pixels and alpha — no shadow, glow, outline, or recolor;
- fully transparent outside the rounded square (alpha 0 at the canvas corners);
- the rounded-square mask drawn at 4× supersampling and LANCZOS-reduced.

Every derivative frame is **one premultiplied-alpha LANCZOS resample taken directly from the 1024 master** — never a resize of an already-small frame (the afdc609 lesson: box-filtering thin strokes melts them into partial-alpha gray hairlines). Deterministic pixel-level invariants (dimensions, transparent corners, gradient direction and stops, neutrality, internal-mark hash, ICO frame inventory and order) are enforced by the test suite (`src-tauri/tests/icon_assets.rs`), not by visual judgment.

### Windows ICO frame-order requirement (compatibility contract)

tauri-build embeds `src-tauri/icons/icon.ico` into the executable as PE icon-group resource `32512` (the shell-facing EXE icon), and tauri-codegen builds the **runtime window HICON from ICO entry[0]** (tauri-codegen 2.6.3, `image.rs` — `icon_dir.entries()[0]`). An ascending ICO with 16×16 first made Windows upscale the tiny frame for the taskbar; the corrected ICO therefore puts **40×40 first**, with the full order 40, 16, 20, 24, 32, 48, 64, 128, 256 (all 32bpp, PNG-encoded entries). Generators that re-sort frames are a regression; the order is asserted by tests. The icon chain is: external master → generated ICO → tauri-build PE resource / tauri-codegen entry[0] HICON → title bar, taskbar, Alt+Tab, Explorer, shortcut shell icon (shortcuts leave the icon unset so the shell resolves the target EXE's embedded group).

The internal UI mark is untouched by all of this: the sidebar brand keeps rendering the transparent mark, and the SPA favicon (`static/favicon.png`) is the external identity at 128×128. The `src-tauri/icons/` derivative family (PNG sizes, ICNS with the modern PNG chunk set ic07–ic12, Windows Store square logos) is mechanically derived from the same master; the Linux/macOS derivatives exist for future cross-platform packaging and are not live-verified on this Windows-focused phase.

## Windows desktop integration

Aurora provides safe, user-controlled Windows shortcut integration (`shortcuts` module). One ownership rule governs everything: **the launcher only creates or removes a `.lnk` it can positively identify as its own** — the deterministic, product-named shortcut slot whose recorded target is the currently running Aurora executable, compared case-insensitively with normalized separators. A file at the slot that is unreadable, not a shell link, or targeting something else is a reported **conflict** (`shortcut_conflict`) and is never overwritten or deleted. Nothing is ever removed by name alone, and no path outside the exact slot is touched.

### Slots and ownership

- **Desktop** — `%Desktop%\Aurora Launcher.lnk`, owned by the launcher: Settings shows live status (present / absent / conflict / unknown) and offers Create/Remove exactly where valid. Management is offered only for **installed production builds**: release-profile executables not running out of a Cargo build-output directory (development/debug and `target/…` runs report that an installed build is required and never litter a permanent user shortcut). Removal is idempotent when absent.
- **Start menu** — owned by the installers, never by the launcher. Both Aurora installers create Start-menu shortcuts and remove them on uninstall (WiX: `Programs\Aurora Launcher\Aurora Launcher.lnk` with an AppUserModelID property; NSIS: `Programs\Aurora Launcher.lnk` by default, both removed only when they target the installed EXE). The launcher **reports** presence for these slots read-only (either layout counts, in the user's and the machine's Programs folders) and never creates, removes, or duplicates them — one coherent ownership model instead of competing shortcuts.

Status is **live filesystem state, queried on every Settings open and refresh** — never a persisted boolean — so a shortcut deleted outside Aurora disappears from Settings immediately.

### Installer behaviors (verified against the built packages)

The two installers are deliberately different in privilege model and shortcut placement, and nothing papers over it:

- **NSIS** (per-user by default): installs to `%LOCALAPPDATA%\Aurora Launcher`, writes `Programs\Aurora Launcher.lnk` (user) and a desktop shortcut (finish-page checkbox, default-checked interactively; always in silent installs), both targeting the installed EXE with no explicit icon (the shell resolves the EXE's embedded external icon). Silent install/uninstall verified live: both shortcuts and the uninstall entry are created and removed cleanly.
- **MSI/WiX** (per-machine): its silent install fails with MSI error 1925 without elevation by design. Database inspection of the built package confirms it authors a desktop shortcut, the `Programs\Aurora Launcher\Aurora Launcher.lnk` Start-menu shortcut (with `ProductIcon` and an AppUserModelID), and an in-install-dir "Uninstall Aurora Launcher" shortcut, all removed on uninstall — into the **common** Desktop/Programs folders, which is why the launcher's read-only reporting also consults the machine-wide Programs folder.

### Implementation boundaries

- Shortcuts are real `.lnk` shell links written through the Windows shell COM API (`IShellLinkW` + `IPersistFile` via the official `windows` crate, already resolved in the dependency tree) with structured calls — no shell-command strings, no PowerShell interpolation. A shortcut records the target executable, its working directory, and the product description; the icon stays unset so the shell shows the target's embedded external icon, exactly like the WiX desktop shortcut.
- Desktop and Programs directories resolve through `SHGetKnownFolderPath` (per-call COM apartment with correct balance; `RPC_E_CHANGED_MODE` coexists with the runtime's threading), so redirected and localized known folders work — no hard-coded English paths. Start-menu reporting additionally consults the machine-wide Programs folder because the MSI is per-machine; resolution failure there only narrows reporting.
- The pure classification/slot logic is separated from the COM layer and unit-tested everywhere; the real Windows shortcut flow (create → owned detection → foreign-conflict refusal → user-file conflict refusal → owned removal → installer-slot reporting) runs in a scratch directory under the system temp root in tests, never against the developer's real Desktop.
- Non-Windows builds report `supported: false` and fail create/remove deliberately with `shortcut_unsupported_platform`; the command/DTO shape is platform-neutral so future Linux/macOS integration extends rather than rewrites it.
- Taskbar pinning is intentionally absent — Windows deliberately restricts programmatic pinning and it stays a user choice.

New stable command codes: `shortcut_unsupported_platform`, `shortcut_management_unavailable`, `shortcut_conflict`, `shortcut_io_failure`, `shortcut_shell_failure`. Commands: `get_desktop_integration`, `create_desktop_shortcut`, `remove_desktop_shortcut` — each returns the refreshed live status.

## Instance workspace and navigation

The launcher's information architecture separates two navigation levels that never blur: **global launcher navigation** (the persistent sidebar) and **instance-local workspace navigation** (one open instance's tabs). The inspiration is structural — instance-oriented launchers such as Pandora demonstrate that a contextual instance workspace scales — but the visual language, hierarchy, spacing, and restraint are Aurora's own.

### Typed navigation model

Navigation is one typed frontend value (`src/lib/launcher/navigation.ts`, pure and deterministically unit-tested with the plain Node test runner through `npm test`):

```text
NavigationState
├── Global: page ∈ { home, instances, accounts, settings, about, developer(dev only) }
└── InstanceWorkspace: { instanceId, tab ∈ { overview, mods, settings } }
```

- The `developer` destination is stripped from production bundles together with its page, exactly as before.
- Instance-local tabs extend `InstanceTab`, never the global destinations: Mods is implemented; Resource Packs, Shaders, and Logs remain future *instance* tabs, and no global destination exists for them.
- SvelteKit routing stays a single static-SPA route on purpose: there is no browser history surface to keep sensible inside the desktop webview, and introducing router complexity without a need would contradict the minimal-shell principle.
- The workspace view resolves its instance id against the current registry state on every render; an id the registry no longer has produces an explicit "instance not found" state with a way back to Instances — never a silent redirect or a guessed substitute.
- Switching tabs or instances never changes the selected instance; only an explicit Select action (or the workspace header's Select & play, which composes select → readiness → launch) does.

### Sidebar

The sidebar keeps its compact width and order: brand, global destinations, then a restrained **Instances** section (up to four entries in registry order, plus an "All instances" link when the list is capped), the pinned account chip, and the version line. The registry records no last-played or last-opened timestamps, so the section is labeled "Instances" — an honest creation-order list, never a "Recent" claim the data cannot support. While an instance workspace is open, that instance's sidebar entry carries the page marker; the global destinations return to their normal state.

### Instance workspace

Opening an instance (sidebar entry, or Open on the Instances page) enters a stable contextual shell with three parts that persist across tab switches:

1. **Instance header** — a breadcrumb back to Instances, the instance name as the page title, the installed versions as a quiet subtitle, a compact readiness badge, and the contextual actions **Open folder** and **Play**.
2. **Instance-local tabs** — Overview, Mods, and Settings, as a compact horizontal `tablist` (keyboard-operable, arrow keys included) in the main content region. An unsaved Settings draft marks the Settings tab with a text "Unsaved" marker.
3. **The active tab's content.**

The workspace Play button is a *contextual* action, intentionally coexisting with Home's global launch surface. Both use the one authoritative pipeline: the same `get_play_readiness` decision, the same `play_instance` command, and the same frontend launch state — for a non-selected instance the button composes select → readiness refresh → launch-if-ready rather than creating a second launch path.

### Open folder

`open_instance_folder` is the one narrow command added for the workspace. It accepts only an instance id that exists in the registry, derives the instance root through `ManagedPaths` (validated identifier inside the managed instances directory — containment re-checked, never trusted by construction site alone), requires the folder to exist, and hands the absolute path to the OS opener as structured data. There is no arbitrary-path surface, no frontend filesystem path, and no shell string; failures map to the stable codes `instance_folder_missing` and `instance_folder_open_failure`.

### Home, Instances, and the workspace

Home remains the fast global launch surface (selected instance + Play + readiness) and is deliberately not the workspace. The Instances page is selection and creation: compact rows (identity, desired configuration, concise state) with Open/Select/Validate/Retry actions; the full configuration editor and the managed-Java lifecycle live in the workspace. Readiness presentation is shared, not duplicated: Home, the Instances list, and the workspace Overview all derive their status rows from one pure derivation module (`src/lib/launcher/instanceStatus.ts`, unit-tested), and Home and Overview render the same shared readiness-rows component over the same Rust-owned state.

### Instance Settings versus launcher Settings

The distinction the backend has always had is now visible: the workspace Settings tab owns the instance's desired configuration (Minecraft version, loader policy, memory, JVM arguments, window) with its explicit draft/Save model, while the sidebar's Settings destination owns launcher-wide preferences (appearance, Windows desktop integration). The workspace Settings tab states this in its footer. Drafts live per instance id in the frontend store, so switching tabs, opening another instance, or leaving the workspace never silently discards unsaved edits; dirty state is derived by comparing the draft with the saved configuration, so it can never disagree with what Save would change. Install-affecting changes keep their deliberate two-step shape — Save, then **Install new configuration** — and both Home and the workspace Overview agree about staleness because they share the same derivation.

## Local instance mod management

The Mods workspace tab is a local content-management boundary, not an online provider. Rust derives the authoritative `instances/<validated-id>/mods` directory from a registered instance id; the frontend supplies neither a path nor a filename. A refresh performs one shallow scan of direct children on a blocking worker, parses bounded metadata once, and returns a typed snapshot. Search, filters, and sorting operate only on that snapshot, so typing never rescans JARs. There is no watcher, polling loop, provider request, dependency download, or persistent content database.

### Ownership and file states

Ownership follows existing installed state rather than a second database. The exact artifact named by `aurora-installed.json` (and its externally renamed `.disabled` form) is `launcherManagedRequired`; ordinary direct-child `.jar` / `.jar.disabled` files are `userManaged`; unexpected regular files, directories, symbolic links, Windows reparse points, and unreadable entries are `unknown`. Required Aurora is visible as **Aurora Client** with **Required** and **Protected** state, and its details identify it as managed by Aurora; both the UI and backend reject disable/remove. Unknown entries remain visible for reconciliation but have no destructive actions. A malformed or unsupported Aurora installed-state document prevents inventory because ownership cannot be proven safely; it is never overwritten.

Enabled state is filesystem truth: `example.jar` is enabled and `example.jar.disabled` is disabled. Disable/enable is a same-directory rename, detects the target before mutation, never overwrites, and returns a fresh scan. Disabled JARs are still inspected as archives. Removal is permanent and restricted to a current, regular, user-managed JAR after explicit UI confirmation; there is no recursive deletion and no recycle/trash subsystem. Every mutation rescans from an opaque entry token, revalidates ownership/type/canonical containment, refuses links/reparse points, and then returns the authoritative resulting inventory. One mutation per instance runs at a time in-process; cross-process coordination remains deferred.

### Untrusted JAR inspection

`instance_mods` never executes or extracts a JAR. It opens only the exact root `fabric.mod.json`, rejects duplicate metadata entries, limits archives to 4,096 entries for inspection, skips metadata inspection for JARs above 512 MiB, and caps metadata at 256 KiB uncompressed/read. Individual corrupt ZIPs, malformed JSON, non-Fabric JARs, and oversized metadata become per-entry warnings rather than failing the page. Parsed fields are id, name, version, description (display-bounded), authors (first 16), environment, depends/recommends/suggests/conflicts/breaks, and whether an icon is declared. Icons use a generic launcher glyph in this phase: declared paths are not read or extracted, avoiding a binary-image/cache expansion of the command contract.

Display fallback is Fabric name → mod id → filename, with the manifest-proven required artifact labeled Aurora Client. Local diagnostics are deliberately conservative: enabled local mod ids support obvious missing-required-dependency and present-conflict/break warnings, plus duplicate-id warnings. `minecraft`, `fabricloader`, and `java` are treated as loader/game built-ins. Version-range satisfaction is not evaluated, so Aurora never labels a mod compatible/incompatible or turns these metadata warnings into launch blockers. Existing deep Rust readiness remains authoritative.

The UI keeps inventory viewable while Minecraft runs and explains that changes apply to the next launch; it never implies hot reload. Explicit Refresh reconciles manual filesystem changes without background watching. A page-level read failure offers Retry, while one bad entry leaves the rest usable. A later provider layer may augment a local artifact with optional provider identity and remote release/update data, but filesystem presence and enabled state remain authoritative and no Modrinth/CurseForge identity is required.

## Frontend/native boundary


Svelte is a presentation layer. Security-sensitive state and all Minecraft/Aurora installation, authentication, download, integrity, Java/runtime, filesystem mutation, and process-launch logic stay behind native Rust commands or events. Commands should be narrow and use explicit request/response DTOs. Frontend code must not infer structured state by parsing strings.

SvelteKit is configured as a static, client-side SPA because Tauri has no Node server. Vite remains the development and production asset builder.

### Screen organization

The shell is one persistent sidebar (Home, Instances, Accounts, Settings, About — plus Developer in development builds only, stripped from the production bundle together with its page — and a short Instances shortcut section beneath the destinations), one scrolling content region, and, when an instance is open, the instance workspace shell in place of a global page (see "Instance workspace and navigation"); the visual contract is `LAUNCHER_DESIGN_LANGUAGE.md`. Each screen presents only real backend state:

- **Home** — the selected instance, Rust's `PlayReadiness` decision verbatim, and Play as the dominant action; a purposeful empty state when no instance exists.
- **Instances** — creation from the essentials (name, Minecraft version from official metadata, loader policy) and a compact list of instances with concise readiness and Open/Select/Validate/Retry actions. Opening an instance enters its workspace; the page deliberately does not duplicate the Settings editor, and opaque instance ids and raw readiness dumps stay absent.
- **Instance workspace** — the contextual shell for one instance: header (breadcrumb, identity, readiness badge, Open folder, Play), Overview / Mods / Settings tabs, and the future extension point for further instance-content tabs. Overview shows readiness; Mods shows filesystem-authoritative local content with safe management; Settings is the desired-configuration editor with atomic Save changes and deliberate installation.
- **Accounts** — one obvious primary task when signed out (Sign in with Microsoft through the system browser), restrained selected/unselected account rows when signed in, and concise actionable wording for known failures (internal error codes stay off this screen).
- **Settings** — launcher-wide preferences: appearance (theme and accent, applied live and persisted through the launcher configuration; see "Launcher appearance customization") and Windows desktop integration (live desktop-shortcut status with create/remove where Aurora owns the slot, plus read-only installer-owned Start-menu reporting; see "Windows desktop integration"). The page stays sparse and purposeful rather than inventing settings.
- **About** — product information only: version, purpose, project/source/license, and legal disclaimers.
- **Developer** (development builds only) — launcher/platform diagnostics (version, platform, managed data root, persisted-state documents) and the Phase 2–5 pipeline proofs.

A Library destination does not exist and is not shown. Instance deletion remains deliberately unimplemented and appears nowhere in the UI.

## Current native modules

- `application`: constructs typed frontend DTOs, maps internal failures to the command error contract, and exposes the Tauri commands, including managed-runtime status and ensure operations and the derived instance-folder opening.
- `appearance`: the launcher-wide appearance domain — the built-in theme catalog, the curated accent presets, the deterministic custom-accent derivation with its WCAG-contrast validation, and the persisted appearance preferences consumed by the configuration (see "Launcher appearance customization").
- `shortcuts`: Windows desktop integration — the shortcut slot/ownership model (deterministic slots, target-proven ownership, conflicts never touched), known-folder resolution, shell COM `.lnk` creation/reading, and the live status the Settings page renders (see "Windows desktop integration").
- `paths`: validates and represents the platform-resolved application-local data root and derives managed locations without touching the filesystem.
- `config`: the versioned launcher-configuration model and its atomic JSON persistence, now schema 2 with the appearance preferences and a deterministic schema-1 migration.
- `instances`: validated instance identifiers (plus UUIDv4 generation), instance records with lifecycle state, concrete release pins, and the desired per-instance configuration (`settings`: the configuration model, memory bounds, the JVM-argument parser with its conflict policy, and the loader kind/policy vocabulary), the writable atomic-persisted schema-3 instance registry with deterministic schema-2 migration, and `lifecycle` (creation, retry, rename, selection, configuration updates and installs, and complete read-only instance validation including the stale gate).
- `distribution`: the typed Aurora release-manifest model, exact release resolution, and the checked-in development release fixture (no production release infrastructure exists yet).
- `aurora`: Aurora client-artifact installation — SHA-256-verified acquisition through the Phase 2 store, managed materialization under the instance's mods directory, and the versioned Aurora installed-state record.
- `downloads`: transport for trusted artifact acquisition (HTTPS enforcement, redirect policy, timeouts, streamed transfer with inline verification for SHA-256, official SHA-1, and digest-less observed sources); also builds the shared HTTP client the metadata transports reuse.
- `integrity`: canonical SHA-256 digests, official-metadata SHA-1 digests, streaming size/hash verification for both algorithms, and the `ArtifactTrust` representation of how bytes came to be trusted.
- `cache`: the content-addressed artifact stores — SHA-256-addressed, SHA-1-addressed, and transport-observed with provenance sidecars — plus untrusted staging, cache-hit validation, and promotion.
- `minecraft`: official-metadata resolution and install planning — `metadata` (discovery, external DTOs including the asset-index document and logging blocks, the SHA-1-verified fetch boundary), `rules` (pure platform/feature rule evaluation), `plan` (normalization into `MinecraftInstallPlan`), and the `resolve_install_plan` composition.
- `fabric`: Fabric Meta resolution and composition — `metadata` (loader discovery, external DTOs, the fetch boundary), `maven` (validated coordinates, repositories, and deterministic artifact-URL derivation), `plan` (the normalized `FabricPlan`, the composed `GameInstallPlan`, and the collision policy), and the `resolve_fabric_plan`/`resolve_game_plan` compositions.
- `install`: installation execution — `state` (the versioned installed-state manifest), `assets` (asset-object enumeration and official URL derivation), `natives` (defensive ZIP extraction), and the `install_game`/`validate_installed_game` executor with staging, validation, atomic commit, per-instance exclusion, and progress.
- `instance_mods`: shallow local inventory of the derived instance mods directory, bounded Fabric metadata inspection, manifest-backed ownership classification, conservative local warnings, opaque scan identities, reversible `.jar.disabled` renames, protected required artifacts, safe permanent single-file removal, and per-instance mutation exclusion.
- `runtime`: official Java provisioning — `metadata` (external Mojang DTOs and verified manifest resolution), `plan` (explicit platform mapping and normalized file/link/executable requirements), `install` (shared staged materialization, validation, diagnostic execution, reuse, and per-runtime exclusion), and `state` (schema-versioned offline validation facts).
- `auth`: Microsoft → Xbox → Minecraft authentication — `oauth` (pure PKCE/state/authorization-request/callback building blocks), `metadata` (pinned Microsoft/Xbox/Minecraft endpoints, external DTOs, and the bounded no-redirect exchange boundary), `credentials` (the `SecretString` wrapper and the OS-backed credential store: Windows Credential Manager now, other platforms deliberate failure), `accounts` (the versioned non-secret accounts document and selection), `session` (the in-memory Minecraft session cache used by launch assembly), `callback` (the one-shot loopback redirect receiver), and `flow` (sign-in, session restoration with refresh rotation, sign-out, and the process-local login transaction guard).
- `launch`: Rust-owned readiness, deterministic `LaunchSpec` assembly from normalized plans and validated installed state, strict placeholder/path resolution, and exact-child supervision with bounded redacted logs.

Future code should add a module when its behavior is implemented. The major remaining infrastructure boundary is production distribution transport. Avoid a speculative service container, placeholder traits, empty module trees, and generic mod-loader abstractions — Aurora uses Fabric, and its Fabric and launch boundaries are built directly.

Structured error codes crossing the command boundary: `managed_path_unavailable`, `config_malformed`, `config_unsupported_schema`, `config_selected_instance_dangling`, `instances_invalid`, `instances_unsupported_schema`, `instance_id_invalid`, `instance_not_found`, `instance_name_invalid`, `instance_registry_write_failure`, `instance_release_invalid`, `instance_not_ready`, `instance_consistency_failure`, `aurora_release_not_found`, `aurora_manifest_invalid`, `aurora_artifact_invalid`, `aurora_installation_invalid`, `aurora_materialization_failure`, `storage_io_failure`, `artifact_source_invalid`, `network_unavailable`, `download_http_failure`, `download_timeout`, `download_redirect_failure`, `artifact_size_mismatch`, `artifact_hash_mismatch`, `cache_io_failure`, `artifact_promotion_failure`, `minecraft_version_invalid`, `minecraft_version_not_found`, `minecraft_manifest_invalid`, `minecraft_version_metadata_invalid`, `minecraft_metadata_network_failure`, `minecraft_metadata_integrity_failure`, `minecraft_version_unsupported`, `minecraft_library_invalid`, `minecraft_artifact_invalid`, `minecraft_platform_unsupported`, `fabric_loader_version_invalid`, `fabric_metadata_network_failure`, `fabric_metadata_invalid`, `fabric_metadata_unsupported`, `fabric_loader_not_found`, `fabric_combination_unsupported`, `fabric_library_invalid`, `fabric_repository_invalid`, `fabric_plan_conflict`, `fabric_artifact_unverified`, `installation_already_in_progress`, `installation_state_invalid`, `installation_target_conflict`, `minecraft_asset_index_invalid`, `minecraft_asset_invalid`, `native_archive_invalid`, `native_extraction_failure`, `artifact_materialization_failure`, `installation_validation_failure`, `installation_commit_failure`, `auth_configuration_missing`, `auth_login_in_progress`, `auth_login_cancelled`, `auth_login_timeout`, `auth_entropy_failure`, `auth_callback_listener_failure`, `auth_browser_open_failure`, `auth_callback_invalid`, `auth_oauth_failure`, `auth_token_exchange_failure`, `auth_xbox_failure`, `auth_xsts_failure`, `auth_minecraft_services_failure`, `auth_entitlement_missing`, `auth_profile_missing`, `auth_profile_invalid`, `auth_reauthentication_required`, `auth_credential_store_failure`, `auth_account_not_found`, `auth_account_invalid`, `accounts_invalid`, `accounts_unsupported_schema`, `accounts_write_failure`, and `accounts_selected_dangling`. Phase 9 adds `launch_instance_not_ready`, `launch_instance_damaged`, `launch_runtime_not_ready`, `launch_authentication_required`, `launch_session_unavailable`, `launch_metadata_invalid`, `launch_placeholder_unresolved`, `launch_native_path_invalid`, `launch_classpath_invalid`, `launch_logging_invalid`, `launch_assets_invalid`, `launch_already_running`, `launch_spawn_failure`, and `launch_process_failure`. Configurable instances add `instance_configuration_invalid` and `launch_jvm_arguments_invalid`. Appearance customization adds `appearance_invalid`. Windows desktop integration adds `shortcut_unsupported_platform`, `shortcut_management_unavailable`, `shortcut_conflict`, `shortcut_io_failure`, and `shortcut_shell_failure`. The instance workspace adds `instance_folder_missing` and `instance_folder_open_failure`. Local Mods adds `mod_inventory_unavailable`, `mod_entry_unsafe`, `mod_entry_stale`, `mod_required_artifact`, `mod_target_conflict`, `mod_mutation_in_progress`, and `mod_mutation_failure`. Codes are compatibility contracts; keep them stable and user messages readable.

## Managed filesystem model

The managed-data root comes from Tauri's application-local data directory for identifier `com.aurora.launcher`. It is never the user's normal `.minecraft` directory and is not located beside the executable. Phase 1 derives the full layout and materializes only `launcher/config.json` (with its parent directory) on first state read; everything else resolves without touching disk. The desktop webview runtime may create its own platform cache beneath this application directory on first boot (for example, WebView2 creates `EBWebView` on Windows); that runtime-owned cache is not an Aurora instance.

The implemented layout is:

```text
<managed-data-root>/
├── launcher/       # versioned non-secret launcher configuration and state (config.json materialized; instances.json and accounts.json read when present)
├── runtimes/       # shared launcher-managed Java runtimes
│   └── <component>/<platform>-<manifest-sha1>/  # runtime tree + runtime-installed.json
├── cache/          # re-downloadable, safe-to-delete launcher data
│   ├── artifacts/
│   │   ├── sha256/<digest>              # verified against a pre-known expected SHA-256
│   │   ├── sha1/<digest>                # verified against an official Mojang SHA-1
│   │   └── transport-observed/<digest>  # secure-transport artifacts with locally observed SHA-256 identity (+ <digest>.json provenance sidecars)
│   └── staging/                         # untrusted in-flight download files (created by acquisition)
├── metadata/       # verified manifests and installation metadata (derived, not created yet)
└── instances/      # one directory per validated instance id
    └── <instance-id>/
        ├── game/               # launcher-managed, reconstructable game tree (versions/libraries/assets/natives + installed-game.json); installation stages in .install-staging/ beside it
        ├── mods/aurora-<version>.jar  # launcher-managed Aurora client artifact (other mods/ files are user data)
        ├── aurora-installed.json      # Aurora installed-state record (instance root)
        ├── mods/               # other files here are user data
        ├── config/             # Minecraft, Fabric, Aurora, and mod configuration (user data)
        └── logs/               # instance-local launch/game logs (user data)
```

- Cache and incomplete temporary downloads should be safe to delete and reconstruct.
- Managed runtimes and verified launcher metadata should be repairable/re-downloadable, but deletion may be expensive and must remain scoped to proven managed paths.
- Instance saves, screenshots, servers, resource packs, shader packs, configuration, and user-added mods are user data. Back up or obtain explicit confirmation before destructive replacement or deletion.
- Secrets do not belong in ordinary launcher configuration. Use OS-backed secure credential storage where practical.
- User-selected external paths are not launcher-managed merely because the launcher can read them.

## Instances

An instance is an isolated, identifiable game installation with its own game directory, compatible Aurora release, Minecraft version, Fabric Loader version, Java requirements, and user content — plus the user's desired configuration (see "Configurable instances"). The full lifecycle through ready is implemented: opaque UUID identifiers, records with lifecycle state, concrete release pins, and desired configuration, a writable atomic registry with deterministic schema migration, creation and retry orchestration over the Phase 5 installer plus Aurora artifact installation, configuration updates and deliberate reconfiguration installs, display-name rename, selection with referential integrity, and complete read-only validation (ready, damaged, installing, stale, not installed). Deleting instances remains deliberately unimplemented (user-data retention deserves its own design). Path validation prevents traversal, and destructive operations must prove that a target is beneath launcher-managed storage; within an instance, `game/`, the derived `.install-staging/`, and the launcher-named `mods/aurora-<version>.jar` are managed, while other `mods/` files, `config/`, `logs/`, and any future root-level user content are user data the lifecycle never touches.

## Distribution metadata

Aurora distribution should be manifest-driven rather than tied forever to one Minecraft version. The Phase 1 local schema implements exactly this shape, and Phase 6 made it operational locally:

```text
Aurora release and channel
  -> supported Minecraft version
  -> required Fabric Loader version
  -> Java/runtime requirements
  -> Aurora artifact URL
  -> expected cryptographic hash
```

The model supports stable, beta, and nightly channels and keeps each mapping independent; resolution is exact (`resolve_exact`), and instances pin concrete releases rather than channels. No production manifest endpoint or artifact repository exists: the operational source is the checked-in development fixture (`src-tauri/development/`), whose loopback artifact URLs a developer serves locally; a real HTTPS release source later is an additive boundary change. What remains future: production hosting and fetching of the manifest, signature verification, key rotation, canonical encoding, and rollback policy — decisions for the phase that implements real distribution.

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
- Never trust a cached file because of its name: verified cache objects are re-validated by hashing before reuse, and installation must never consume an unverified staging file.

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
- `reqwest` (added in Phase 2, `rustls-no-provider` + `system-proxy` features, default features off) implements the artifact transport: a mature, focused HTTP client that already exists in Tauri's ecosystem. `Response::chunk()` streams the body without pulling a futures-combinator dependency; no archive, retry, queue, or download-manager crates are included.
- `rustls` with the `ring` crypto provider (added in Phase 2) is installed as reqwest's process crypto provider. reqwest 0.13's default provider (aws-lc-rs) requires CMake/NASM tooling on some hosts; `ring` builds with a plain C compiler everywhere, which keeps launcher builds hermetic. Certificate verification uses the platform verifier reqwest selects by default (the OS certificate store).
- `sha2` (added in Phase 2) is the RustCrypto SHA-256 implementation matching the release manifest's artifact digest representation. It was already resolved in the dependency tree; no multi-hash abstraction exists.
- `sha1` (added in Phase 3, RustCrypto) implements the digest algorithm official Mojang metadata actually publishes. It exists next to `sha2` so official SHA-1 expectations are represented and verified accurately — never faked as SHA-256 and never used for verified-cache identity. No other hash algorithm or generic digest framework was introduced.
- `tokio` (added in Phase 2) is used for async staging-file I/O and narrow mutexes. Phase 7 enables its focused `process` and `time` features for bounded structured `java -version` execution; no process framework is introduced. The resulting `errno` and `signal-hook-registry` lockfile entries are Tokio's cross-platform transitive support, not direct launcher dependencies.
- `url` (added in Phase 2) parses artifact URLs so scheme and loopback-host enforcement is robust. It was already resolved in Tauri's dependency tree.
- `uuid` (added in Phase 6, `v4` feature) generates opaque instance identifiers — the simplest mature solution to collision-resistant, name-independent, timestamp-independent IDs; the canonical simple form fits the identifier rules by construction. A custom identifier scheme would have been invented complexity.
- `zip` (added in Phase 5, version 8.6, `deflate` feature only, default features off) implements the one archive format Minecraft native artifacts actually are — ZIP/JAR — for extraction only. It is the focused, actively maintained zip-rs implementation (stable 8.6.0, April 2026); its reader is wrapped by the launcher's own strict path-safety validation rather than trusted for it. No TAR support, no generic archive abstraction, no extra compression codecs.
- Svelte and TypeScript implement the typed presentation layer; SvelteKit's static adapter is retained from the official Tauri Svelte template to produce a serverless SPA.
- Vite supplies fast development and production asset builds; `svelte-check` provides compiler-aware type checking.
- Phase 4 added no dependency: Fabric Meta documents are JSON over the existing reqwest transport, and Maven artifact-path derivation is string construction over validated parts — no Maven client, XML/POM parser, archive crate, installer framework, database, generic mod-loader library, Java-management crate, or auth library was needed or added.
- Phase 8 added `getrandom`/`base64ct` for PKCE entropy and encoding, the official Tauri opener for the system browser, and narrow `windows-sys` Credential Manager bindings. Phase 9 adds no dependency: existing Rust/Tokio structured process APIs are sufficient, with no shell or process framework.

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

## Major Phase 2 decisions

- Trust is a property of the verified cache store only. Staging files are untrusted by definition; the store only ever receives objects that passed size and SHA-256 verification, and its existing objects are re-validated before reuse.
- Content addressing by SHA-256 (not URL, not filename) gives deterministic deduplication and makes the digest the single cache identity. No metadata database tracks the cache; correctness beats one extra local hash pass.
- The test transport path is explicit and narrow: cleartext HTTP exists only for loopback hosts, only through a named constructor, so deterministic tests need no public internet and no TLS endpoint while production sources stay HTTPS-only.
- Redirect handling is a pure, unit-testable decision function wired into reqwest's custom policy: bounded hops, no HTTPS downgrade, no loopback-cleartext escapes, with each refusal surfaced as a distinct structured error reason.
- `ring` was chosen over reqwest 0.13's default aws-lc-rs crypto provider so launcher builds never require CMake or NASM; the platform verifier keeps certificate trust anchored in the OS certificate store.
- Promotion is a single rename of a fully verified file (Windows `MoveFileExW` semantics replace the destination), with explicit recovery branches for the races that matter: concurrent same-digest completions and corrupt destination replacement. Unexpected errors never silently destroy a previously valid cache object.
- The development-facing `acquire_artifact` command exists to prove the pipeline through real IPC. It accepts typed artifact metadata only and is shown in the shell solely in dev builds; product download flows will wrap this pipeline in later phases.

## Major Phase 3 decisions

- Official metadata is resolved from the pinned Mojang manifest endpoint over HTTPS, never from third-party launcher APIs, and never from frontend-provided URLs: the only URLs the metadata transport follows are the pinned manifest URL and the version-document URLs the parsed manifest itself provides.
- The trust split is explicit: the manifest is bootstrap discovery metadata (HTTPS plus validation, never a "verified artifact"), while version documents are hash-addressable — fetched from the manifest URL and verified against the manifest SHA-1 before parsing. "HTTPS succeeded" is never equated with Phase 2 artifact verification.
- SHA-1 was added narrowly (a `Sha1Digest` type plus the `sha1` crate) because Mojang's integrity data is SHA-1; the verified cache stays SHA-256-addressed, so official digests and Aurora digests can never be confused.
- External Mojang DTOs stop at `minecraft::metadata`; everything downstream consumes the normalized plan. Legacy shapes (`inheritsFrom`, `minecraftArguments`, `natives`/`classifiers`/`extract`, historical version types) are detected and rejected deliberately rather than half-resolved.
- Rule evaluation is pure and vocabulary-strict: unknown os/arch/feature names fail parsing, defaults are documented (no rules = applicable; unmatched rule list = excluded; last match wins), and feature conditions stay unresolved in the plan.
- Library identity is structured (parsed Maven coordinate plus validated repository path that must agree with the coordinate layout), so no future phase parses file names to reconstruct identity or ordering.
- Metadata is fetch-on-demand with no persistence: no second cache system, no expiration logic, no background refresh — the smallest honest behavior for this phase.
- The dev-only `plan_minecraft_install` proof returns a summary DTO only; the full plan never crosses the IPC boundary and the UI never implies installation.

## Major Phase 4 decisions

- Fabric resolution consumes only the official Fabric Meta API from its pinned root; loader selection is exact (existence in the official list, then combination support), with no "latest", no substitution, and no automatic upgrades. Aurora's future release manifest remains the intended policy source for which loader version a release requires.
- The vanilla `MinecraftInstallPlan` is never mutated into a Fabric-modified shape. Composition is explicit containment: `GameInstallPlan` holds both unchanged plans plus derived views, so Mojang and Fabric requirements stay distinguishable for diagnostics and repair, and a future installer never needs to know Fabric Meta JSON semantics.
- The pinned `launcherMeta` generation is 2; a different generation fails deliberately instead of half-parsing. Library composition order mirrors fabric-meta's own profile builder (`common`, intermediary-when-not-placeholder, loader, then the client group), verified against the live `/profile/json` output rather than assumed.
- Fabric's coordinate charset deliberately includes `+` (official versions like `0.17.4+mixin.0.8.7` use it) in a fabric-owned `MavenCoordinate` type; the Mojang coordinate type in `minecraft::plan` is untouched, and the two never blur into a generic Maven framework — no POM parsing, no dependency resolution, no repository client.
- The trust difference is represented, not erased: Fabric's published SHA-256 digests are recorded where they exist; the loader and intermediary artifacts are represented as digest-less because official resolution metadata provides none for them. No digest is invented, HTTPS is never called verification, and the Phase 2 trust model is unchanged.
- Collisions are deliberate policy, never silent: exact duplicates collapse (Mojang first), same-identity-different-version is a hard `fabric_plan_conflict`, and classifier coexistence follows Mojang's own native-library practice.
- No new dependency was required: Fabric metadata is JSON over the existing transport, and Maven path derivation is string construction over validated parts.
- Metadata remains fetch-on-demand and memory-only; the dev-only `plan_fabric_install` proof returns a summary DTO, the full composed plan stays native, and the UI never implies installation.

## Major Phase 5 decisions

- Installers consume normalized plans only. The executor's inputs are `GameInstallPlan` and a validated `InstanceId`; the one document it reads beyond them (the asset index) is acquired and verified first and parsed by the metadata DTO boundary — never raw metadata re-resolution at install time.
- The cache remains the acquisition/trust boundary and the installer the materialization boundary, and that separation is visible in the modules: nothing is ever downloaded into an instance location, and nothing is ever installed from an unverified staging file — materialization copies only from verified store objects, then re-hashes the staged copy per its trust before the completion record exists.
- Completion is a property of the committed installed-state manifest. The manifest is written last inside the staged tree, and the whole tree becomes visible through one directory rename; every failure path leaves either no game directory or the previous complete one, proven by deterministic failure-injection tests (acquisition 404, injected materialization fault, malicious traversal archive, injected pre-commit fault).
- Cache identity stays aligned with the externally expected digest: Mojang artifacts are SHA-1-addressed in their own store (`cache/artifacts/sha1/`), never re-labeled as SHA-256, with all Phase 2 semantics (revalidation, corrupt replacement, rename promotion, race recovery) mirrored rather than generalized away.
- Digest-less Fabric artifacts are represented honestly as `SecureTransportObserved`: secure HTTPS transport plus a locally computed SHA-256 identity, persisted with provenance sidecars that give later acquisitions a TOFU-style local consistency check. The observed digest is never called an expected digest, never grants `verified` status, and a drift under a stable URL fails deliberately (`fabric_artifact_unverified`). A full user-facing pinning policy is deferred.
- Asset objects are copied into instances rather than symlinked (Windows developer-mode dependency) or hard-linked (shared-mutation coupling with the cache); cross-instance deduplication happens in the shared content-addressed store, and the per-instance disk cost is a documented tradeoff to revisit if it ever matters.
- Native extraction defends even against verified archives: strict relative-path/charset validation, META-INF exclusion matching official launcher semantics, duplicate names deduplicated-when-identical and a hard conflict when they differ (the macOS multi-`osx`-classifier case: materialize exactly what the plan selected, never delete arbitrarily, refuse to guess), and per-entry/per-archive size bounds.
- The official logging configuration is a planned, installed, recorded launch artifact (current documents publish one), while constructing the JVM argument that references it remains deliberately deferred to the launch phase.
- `game/` is the only launcher-managed subtree of an instance; `mods/`, `config/`, `logs/`, and future root-level user content are never read or written by installation, and destructive operations are restricted to the fixed derived staging path and manifest-proven game directories.
- Reinstallation over a manifest-proven installation is an explicit, deliberate replacement; an unrecognizable existing `game/` tree is a hard conflict, and a malformed installed-state document is never overwritten — the user repairs it, not the installer.
- Installation is sequential; one install per instance at a time (in-process async mutex, immediate `installation_already_in_progress` for overlaps). No download parallelism, no pause/resume, no cancellation, no repair yet — correctness first, each a deliberate future decision.

## Major Phase 6 decisions

- No production Aurora release infrastructure exists, so none was invented: the operational release source is the checked-in development fixture with loopback artifact URLs, labeled as development everywhere it surfaces, and the resolver consumes any parsed manifest so a real HTTPS source is additive later. Manifest authenticity is documented as HTTPS-authenticated (not signature-verified) — no signing infrastructure, no fake trust roots.
- Instance identity is generated (UUIDv4, simple form), never derived from user text; display names remain pure metadata whose change cannot move a single file.
- Registry writes are atomic and schema-versioned (schema 2 introduced lifecycle state and concrete release pins; schema 1 fails as unsupported rather than migrating). A malformed registry is never overwritten, and duplicate identifiers never reach disk.
- Creation persists an explicit `installing` record before the long installation and promotes it to `ready` only after complete validation; failures leave a retryable non-ready record rather than any form of destructive rollback. "Damaged" is computed by validation on demand, never stored.
- Aurora artifacts always require a pre-known expected SHA-256 and always verify through the Phase 2 SHA-256 store; the launcher-owned file is deterministically named from validated release metadata under `mods/`, with ownership recorded in a small versioned installed-state document that references — never duplicates — the game manifest.
- Complete validation is read-only and download-free: registry presence, deep game validation, Aurora artifact re-hash, and three-way version consistency (pin ↔ Aurora state ↔ game manifest). The stored `ready` flag is a floor, not proof; the UI exposes deep validation on demand instead of hashing whole installations on every state load.
- Selection is explicit: first successful instance auto-selected when nothing is selected, later creations never steal the selection, dangling selections are a deliberate state error, and nothing ever silently selects a random instance.
- Registry mutations are serialized by a process-wide mutex held only across read-modify-write windows; installations run outside it under Phase 5's per-instance exclusion.
- Sequential installation speed and per-instance asset duplication are deliberately untouched; they are documented, measured realities with concrete future optimization paths (bounded concurrency, hard links), not things to smuggle into a lifecycle phase.

## Major Phase 7 decisions

- Current official runtime metadata remains publicly usable and matches the component-based model, but delivery is a per-file tree rather than an archive. The implementation mirrors directories/files/links directly and keeps sequential acquisition as the simplest bounded strategy.
- Runtime identity is the official platform key plus the SHA-1-addressed file manifest beneath the validated component. Java major alone is never treated as interchangeable identity.
- The raw runtime entries use Mojang's exact SHA-1 and size through the existing verified store. Optional LZMA representations are not used, avoiding an unnecessary decompressor and second materialization path.
- Runtime state is committed last in staging and the directory is promoted atomically/rollback-safely. Valid exact state proves replaceable ownership; unknown or malformed trees remain untouched.
- Readiness is two-dimensional: Phase 6 instance content can remain ready while the exact managed runtime is missing or damaged. Java provisioning is an explicit post-creation operation and instances never contain absolute runtime paths.
- The release manifest's Java major is retained for backward compatibility only as an assertion against the official resolved game-plan major; Minecraft's component and effective Minecraft/Fabric major are authoritative.
- Diagnostic execution is deliberately only `java -version`, with an absolute managed executable, argument array, minimal cleared environment, captured bounded output, a ten-second timeout, and major-version confirmation. This is runtime validation, not game launching.
- Runtime exclusion is process-local and exact-identity-scoped. Cross-process locking, system Java, custom paths, runtime garbage collection, generalized repair, download concurrency, and launch configuration remain deferred.

## Major Phase 8 decisions

- Microsoft public-client authorization code + PKCE runs in the system browser with a one-shot loopback callback; passwords, embedded login webviews, client secrets, and borrowed registrations are prohibited.
- The application (client) ID is public configuration committed with the source (no client secret exists to protect), with the `AURORA_MICROSOFT_CLIENT_ID` build-environment variable as an explicit override channel for fork builds; a build with no registration at all fails honestly with `auth_configuration_missing`.
- Only the rotated Microsoft refresh credential is persisted, through the OS-backed store (Windows implemented); downstream tokens and Minecraft sessions are Rust-only and memory-only.
- Account records are non-secret, schema-versioned, atomically written, selected independently of instances, and keyed by validated Minecraft profile UUID.
- Entitlement and profile validation are mandatory and provider errors are sanitized. The production Aurora Client registration exists, completed the Minecraft Services AppID review, and the full chain was verified live (including secure persistence across restart).

## Major Phase 9 decisions

- Readiness is one Rust-owned decision over deep instance validation, exact managed runtime validation, a usable authenticated session path, and process-local duplicate exclusion. Svelte only displays that decision.
- `LaunchSpec` is deterministic, non-serializable, and redacting. It is assembled from the existing normalized Mojang/Fabric plans and installed manifest, never from scanned jars, user mods, frontend paths, or raw metadata DTOs.
- The isolated instance root is both Minecraft's game directory and process working directory. User mods/config/saves remain ordinary user-owned game data and are never repaired or removed after failure.
- The common extracted native root is passed to current LWJGL; Mojang's component suffixes are preserved. All classpath, asset, logging, native, and Java paths are role/existence/containment checked.
- Process execution uses exact structured arguments and the exact child handle. Output is bounded and secret-redacted; process state is runtime-only; same-instance duplicate launches fail while other instances remain independent.
- No Stop action, cross-process lock, automatic repair/relaunch, or metadata-persistence redesign is included. Authentication is live-verified; the production Aurora release artifact remains the pending v0.1 launch-integration dependency.

## Major configurable-instance decisions

- Desired configuration and installed state are separate persisted facts on every record; the release pin remains what installed content must match, and readiness compares the two. Launch-only settings (name, memory, JVM arguments, window) never invalidate content; install-affecting settings (Minecraft version, pinned loader version) make the instance honestly stale until a deliberate install.
- Registry schema 3 migrates schema 2 deterministically (identities, names, states, pins, and selection preserved; configurations derived from pins). Schema 1 stays unsupported, malformed files are never overwritten — the established document discipline, now with its first real migration.
- Aurora owns the heap exclusively: one generated `-Xmx` from the MiB configuration, no `-Xms`, and custom JVM arguments carrying heap/classpath/executable flags are rejected with an explanation at save and re-checked at launch. The documented quoting rules parse the raw text into an argument vector; no shell ever participates.
- Loader selection is a kind plus a policy: Fabric is the only implemented kind (vanilla deferred — the pipeline is deliberately Fabric-direct), and `Automatic` resolves the newest stable loader at install time as an explicit user choice whose result is then pinned in installed state; nothing ever silently substitutes or upgrades an installed loader.
- Minecraft version choice consumes the pinned official Mojang manifest (snapshots only behind an explicit filter) and is bounded by honest Aurora release compatibility: the lifecycle resolves a release matching the configured version, and unsupported combinations are refused up front rather than silently installed.
- Windowed resolution rides Mojang's own `has_custom_resolution` arguments; fullscreen (an `options.txt` concern) and custom Java executables (a managed-runtime trust concern) are deliberately deferred with documented reasons.

## Major external-identity and desktop-integration decisions

- The internal transparent mark and the external rounded-square application icon are two canonical roles, never one asset: the internal mark stays byte-identical (hash-pinned by tests), and the external icon derives from it deterministically — no image-model redraw, no reinterpretation, no background added to the internal source.
- The external master and every derivative come from one repository-owned generator (`tools/generate_external_icon.py`) with documented exact parameters, so regeneration is a mechanical transform rather than a judgment call; the launcher never generates its icon at runtime.
- The Windows ICO keeps 40×40 as entry[0] because tauri-codegen rasterizes the runtime window HICON from the first entry (verified in the pinned toolchain's source, not assumed); frame presence and order are enforced by deterministic tests.
- Shortcut management is ownership-proven, not name-based: only the exact product-named slot whose recorded target is this executable is created/refreshed/removed; anything else there is a reported conflict that survives untouched.
- The desktop shortcut is launcher-owned and Settings-managed; the Start menu belongs to the installers (both create and uninstall-clean their own), and Aurora only reports it — no duplicate or competing shortcuts, one coherent model across install, Settings, and uninstall.
- Creation/removal require an installed production build (release profile, outside Cargo build-output directories); development runs report the requirement instead of creating shortcuts into build trees.
- Shortcut status is always the live OS answer; no persisted preference stands in for filesystem truth.
- Runtime-created shortcuts deliberately set no icon location — the shell resolves the target EXE's embedded external icon, matching the WiX desktop shortcut's property set — and no AppUserModelID is written (the WiX desktop shortcut omits it too; matching the installer's own desktop-shortcut property set).
- Taskbar pinning stays out: Windows restricts programmatic pinning by design, and forcing pins through unsupported shell hacks is prohibited.

## Major instance-workspace decisions

- Navigation is one typed value with two levels (global page vs instance workspace + tab), owned by a pure tested module and a thin reactive store — never scattered booleans — and SvelteKit stays a single-route SPA because the desktop webview has no meaningful browser-history surface to maintain.
- The workspace is a stable shell: header + tab row persist while tabs switch, so switching never rebuilds the page, re-runs backend operations, or resets unrelated state.
- The workspace Play button composes the existing single pipeline (select when needed → Rust readiness → launch-if-ready); no second launch implementation, readiness reconstruction, or duplicated launch state exists, and Home stays the global launch surface.
- Open folder is a derived-path command, not a path-opener: registry existence, validated-identifier path derivation, containment re-check, existence check, then the OS opener with structured data. It cannot open anything but a managed instance root.
- Sidebar instance entries are registry-ordered under an honest "Instances" label because no recency data exists; inventing a "Recent" claim or a recency database for it was rejected.
- Readiness/status presentation is one shared, unit-tested derivation consumed by Home, the Instances list, and the workspace Overview — the consistency requirement is satisfied by construction, not by duplicated logic staying in sync.
- Configuration drafts are per instance id and dirty state is derived by comparison, so unsaved edits survive tab switches and navigation and can never silently disappear or disagree with Save.
- Global Settings stays launcher-wide (appearance, desktop integration) and the workspace Settings stays per-instance; the UI states the distinction and neither side absorbs the other's controls.
- Mods is the first implemented instance-content tab; Resource Packs, Shaders, and Logs remain documented `InstanceTab` extension points with no placeholders or global destinations.

## Major local-Mods decisions

- Filesystem truth is authoritative and bounded to direct children of the derived registered-instance mods directory; no frontend path, recursive traversal, watcher, database, or provider identity participates.
- Ownership reuses `aurora-installed.json`: the named Aurora artifact is required/protected, ordinary JARs are user-managed, and links/directories/unexpected files are unclassified and non-destructive.
- `.jar.disabled` is the reversible convention. Same-directory rename is atomic at the filesystem level, collision-refusing, post-rescanned, and protected in Rust rather than only by disabled controls.
- Entry mutation uses opaque scan-derived tokens. Rust resolves the token against a fresh inventory and re-proves a regular direct child beneath the canonical mods directory; stale identities, symlinks, junctions/reparse points, and path redirection fail closed.
- Removal is permanent single-file deletion after explicit confirmation. A recycle-bin abstraction would add platform-specific dependencies and ambiguous guarantees, so no recovery subsystem is implied; broad or recursive deletion is absent.
- Fabric metadata inspection reads only `fabric.mod.json` under documented entry/archive/byte bounds. Icons intentionally use the generic glyph; no archive asset is extracted or cached.
- Dependency/conflict diagnostics are local evidence only. Version ranges are preserved for details but not evaluated, and metadata warnings never override launch readiness.
- Mutations while a game is running remain available because they are ordinary safe local file operations, with explicit wording that they affect the next launch only.

## Explicitly deferred

Linux/macOS secure credential stores; system-Java discovery or custom Java selection; vanilla instances and any non-Fabric loader kind; fullscreen/window management beyond the official custom-resolution launch arguments; runtime garbage collection and cross-process installation/launch locks; a "follow system" theme mode and any light theme (Aurora is fundamentally dark; a fake System selector that always resolves to dark would be dishonest); a launcher-owned animation preference (the OS `prefers-reduced-motion` preference is respected unconditionally); Linux/macOS shortcut/desktop integration (the command and DTO shape is ready; no implementation ships until those platforms own it); taskbar pinning and any shell-hack pin forcing; file associations and URI/deep-link registration; production Aurora release infrastructure (manifest endpoint, artifact repository, signing) — today's source is the checked-in development fixture; instance deletion (user-data retention policy deserves its own design); online mod discovery/install/update, provider matching, automatic dependency resolution, and mod icon extraction; a full user-facing trust/pinning policy for transport-observed artifacts; generalized repair and automatic repair of damaged instances/installations (exact managed-runtime reinstall is the sole narrow exception); Aurora version updates and channel movement; persisted normalized launch metadata for network-independent cold Play; a Stop action; pause/resume and cancellation; download parallelism, retry, resume, bandwidth controls, and generalized queues; hard-link asset sharing and cache eviction/size policy; self-update; telemetry; social/news/cosmetic/cloud systems; and any custom backend service.

## Known limitations

The launcher reports native status, persists configuration/instances/accounts, acquires verified artifacts, plans and installs modern Minecraft plus Fabric and Aurora, provisions exact shared official Java, implements authenticated launch assembly and process supervision, makes instances configurable — desired Minecraft/loader/memory/JVM-argument/window settings with atomic saves, honest stale-state invalidation, deliberate reconfiguration installs, and a deterministic schema-3 registry migration — carries a launcher-wide appearance system (three built-in dark themes plus validated accent customization, persisted in the schema-2 launcher configuration, applied live and restored before the first shell render), now carries a separate external application identity (the deterministic rounded-square icon for the executable, taskbar, installer, and shortcuts) plus live Windows desktop-integration status with ownership-proven desktop-shortcut management in Settings, and organizes its UI around a typed two-level navigation model: global sidebar destinations plus a contextual instance workspace (Overview/Mods/Settings tabs, breadcrumb header with derived Open folder and the shared-pipeline Play). Mods provides filesystem-authoritative local inventory and safe single-file management; online discovery/install/update remains absent. The full production authentication chain is live-verified; the remaining v0.1 launch-integration dependency is the production Aurora release artifact, which Play reports as its outstanding blocker instead of using synthetic credentials. Aurora releases still come from the checked-in development fixture, which truthfully limits the Minecraft versions a live instance can be configured to install. Minecraft, Fabric, and runtime discovery metadata are re-fetched on resolution; installed products and runtime files are reused. Historical Minecraft shapes and unsupported runtime platform/component pairs fail deliberately. Vanilla instances, fullscreen management, and custom Java are deliberately deferred. Installation remains sequential, process-local exclusion has no cross-process lock, and there is no Stop, pause/resume, runtime garbage collection, instance deletion, or generalized repair. Damaged content is detected but not automatically repaired; a manifest-proven damaged managed runtime is the sole narrow reinstall exception. The TypeScript DTOs still mirror Rust manually.

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
- Spawn processes with structured executable/argument APIs, never shell strings. Sanitize diagnostics and never log secrets or reusable tokens.
- Validate path containment before destructive operations. Distinguish managed paths from user-selected external paths.
- Do not casually change the Tauri identifier (`com.aurora.launcher`), managed-data semantics, command DTOs, or isolation model; they become compatibility contracts.

## Verification and Git

Run checks proportional to the change. Native-boundary or startup changes require type checking, Rust format/check/test, a production build, and a real application boot when the host permits it. Keep documentation aligned with implemented behavior. Preserve user changes, inspect `git status`, make focused commits, and never push unless explicitly requested.

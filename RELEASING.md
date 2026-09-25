# Aurora Launcher release baseline

Aurora Launcher and Aurora Client are separate products. Launcher releases live in this repository; the launcher currently bundles a reviewed production entry for Aurora Client 2.1.2. Publishing a new Aurora Client artifact alone does not change launcher availability or any existing instance pin.

## Version and identity

`package.json` is the launcher version source. Tauri reads that file through `src-tauri/tauri.conf.json`; the native application status and About page display Tauri's resolved package version. `tools/release/verify-version.mjs` checks the npm lockfile, `Cargo.toml`, and `Cargo.lock` against it. `npm run build` and `npm run check` run this check, and the release workflow repeats it with the requested version. The executable and both installer metadata versions are checked after packaging.

The inherited `0.1.0` was a development version, not an intentional first public release number. The release validator rejects it. Before the first public release, the developer chooses a numeric `major.minor.patch` launcher version and updates `package.json`, `package-lock.json`, `src-tauri/Cargo.toml`, and `src-tauri/Cargo.lock` together. Run `npm run verify:version` and the full suite below. Do not use `npm version` with its default tag behavior. Launcher tags use `v<launcher-version>` in this launcher repository; Aurora Client 2.1.2 does not set the launcher version.

The Windows product name is **Aurora Launcher**, the executable is `aurora-launcher.exe`, and the stable Tauri identifier is `com.aurora.launcher`. Current installer `Manufacturer`/registry publisher text is `aurora`, derived from the identifier; no publisher certificate or explicit signing configuration exists. Preserve this identity and the checked-in icons. The installer uses the external Aurora icon, while the UI uses the transparent internal mark.

## Windows installers

`npm run tauri build` on Windows emits x64 NSIS (`Aurora Launcher_<version>_x64-setup.exe`) and x64 MSI (`Aurora Launcher_<version>_x64_en-US.msi`) because `bundle.targets` is `all`. The release workflow builds both and requires an explicit `installer_format` choice (`nsis`, `msi`, or `both`) for public assets. This is a distribution decision, not a ranking:

| Behavior | NSIS | MSI / WiX |
| --- | --- | --- |
| Default scope | Current user, `%LOCALAPPDATA%\Aurora Launcher` | Machine-wide, requires elevation |
| Start shortcut | Current user's Programs root | Common Programs `Aurora Launcher` folder |
| Desktop shortcut | Default checked on interactive finish page; created in silent mode | Common Desktop |
| Uninstall | Per-user uninstall entry and `uninstall.exe` | Windows Installer product registration |
| Existing same-name shortcut | Preinstall guard refuses a foreign target; uninstall checks the target before removal | WiX owns the authored common shortcuts as installer components; foreign-slot behavior needs isolated acceptance before choosing MSI for publication |

The NSIS preinstall hook checks the product-named user Start and desktop slots before Tauri's stock shortcut creation; a foreign or unreadable existing link stops installation before copying files. The stock NSIS uninstaller removes a shortcut only when it targets the installed executable. Settings can manage its own per-user desktop shortcut only when ownership is proven; it reports installer-owned Start shortcuts without changing them. The MSI uses WiX components for common shortcuts, so its foreign-slot behavior must be tested in an isolated environment before MSI is selected as a public format. The launcher-managed data root (`%LOCALAPPDATA%\com.aurora.launcher`: configuration, accounts reference, caches, runtimes, and isolated instances including user content) is outside the installation directory. **Default NSIS uninstall and MSI uninstall preserve it.** The stock interactive NSIS uninstaller also offers an unchecked **Delete app data** option; selecting it recursively deletes that data, including instances. Leave it unchecked during acceptance and make that consequence explicit in distribution guidance. Reinstall should reuse and validate preserved data. Do not delete `.minecraft` or instance content merely to make uninstall look clean.

Both formats currently build **unsigned**. Windows may show an unknown-publisher and SmartScreen/reputation warning; a successful hash check does not replace publisher authentication. Code signing is a separate developer decision before public distribution. Do not add a fabricated certificate or private key to the repository.

## Manual release workflow

`.github/workflows/launcher-release.yml` has only `workflow_dispatch`. Before invoking it:

1. Audit and normally push reviewed history to `main`; do not force-push or move a tag.
2. Choose and synchronize the first launcher version, then complete production installer acceptance on a disposable Windows user/profile or VM. Keep existing developer installation and launcher data intact.
3. Configure a GitHub Actions environment named `launcher-production` with required reviewers (preferably disallow self-review). GitHub does not make a newly named environment reviewer-gated automatically. Ensure Actions can create releases with its scoped `GITHUB_TOKEN`; no broad PAT is needed.
4. Dispatch from `main` with the exact 40-character current `main` commit SHA, the launcher version, and the chosen installer format.

The read-only resolve job requires the exact current `main` SHA, validates the version, and rejects an existing tag or release. The Windows build job runs frontend and Rust tests/checks, builds the frontend and both Tauri installers, checks executable/installer product metadata, and records exact sizes and SHA-256 values. It uploads only the selected installer(s) plus `release-assets.json` as a private workflow artifact. The reviewer-gated publish job downloads and re-hashes those same bytes, checks collisions again, creates a **draft**, uploads and verifies its asset list and sizes, then publishes. Finally it downloads the public installers without an authorization token and compares their sizes and SHA-256 values. A failure before publication leaves a draft for manual review; a failure after publication requires human investigation. The job does not rebuild installers.

GitHub releases are the public release location. The workflow summary and release notes report version, source SHA, filename, format, architecture, size, and SHA-256. Workflow publication is never triggered by a push, pull request, schedule, or another workflow. The workflow is prepared here only; this baseline task does not invoke it, create a tag, push, or publish a launcher release.

## Local verification and acceptance

```sh
npm ci
npm run verify:version
npm test
npm run check
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo check --manifest-path src-tauri/Cargo.toml --all-targets
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri build
```

For final acceptance, install the exact final installer in a clean, disposable Windows environment. Check the executable, installed version/icon, Start and desktop shortcuts, uninstall entry, and normal launch outside dev mode. With fresh launcher state, sign in through the system-browser flow, create a new stable Aurora Client 2.1.2 instance, confirm Minecraft 1.21.11 / Fabric Loader 0.19.5 / Fabric API 0.141.6+1.21.11 / managed Java 21, and verify both required mods are protected. Play, observe Aurora ready and launcher Running, confirm a duplicate launch is blocked, close the game, and observe clean exit. Restart the installed launcher and verify account/instance/selection/readiness persistence. Uninstall and reinstall the same installer; verify installed files and owned shortcuts are removed and restored without duplicate registration, while launcher data remains intact. Capture screenshots and logs outside Git, with secrets redacted. Do not treat a development executable or an older existing installation as final installer acceptance.

## Future launcher updates

No updater, polling, or automatic launcher installation exists now. Launcher versions are numeric `major.minor.patch`, compared independently from Aurora Client instance versions. A future **manual** “Check for launcher updates” action may query this repository's published GitHub Releases; there are no launcher channels planned. It should present the version and release notes and require user approval before download and installation. It must verify the downloaded bytes against authenticated release metadata and an appropriate publisher signature; a checksum fetched from the same unauthenticated channel is insufficient on its own. A Tauri self-updater, if later chosen, additionally requires real updater signing keys and signature verification. Failed installation must leave the previous launcher usable and preserve app data; Windows installer replacement would require a restart. These are design requirements, not implemented update behavior. Aurora Client releases remain bundled, reviewed instance-install inputs and never silently update pinned instances.

After the launcher baseline, the planned sequence is instance content-management foundations, then Modrinth, then CurseForge, then unified browsing and update UX. None of those provider behaviors belongs to this release.

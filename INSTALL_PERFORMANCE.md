# Installation performance investigation

This diagnostic records the 1.0.0 installer investigation on Windows, starting at commit `3a034de5d8bcfabd765fbb5db2867088a498e19f`. All benchmark roots were uniquely created under the OS temporary directory; the developer's launcher cache, accepted instances, and `.minecraft` were not cleared or modified. The controlled tests use an in-process loopback server. Live tests use the reviewed Aurora 2.1.2 production entry, Minecraft 1.21.11, Fabric Loader 0.19.5, Fabric API 0.141.6+1.21.11, and official Mojang/Fabric sources.

## Call flow and original bottlenecks

The UI invokes `create_instance`; Rust resolves the release and exact loader, writes an `installing` registry record, fetches Mojang and Fabric metadata, composes the normalized game plan, and executes the game installer. The installer acquires the asset index, client, logging configuration, composed libraries, and deduplicated asset objects through the trust-specific caches. It copies verified objects into instance staging, extracts verified natives, re-hashes the staged files, writes `installed-game.json` last, and promotes the tree. The lifecycle then acquires Aurora and Fabric API through the SHA-256 store, validates the complete instance, and changes its registry state to `ready`. Managed Java is a separate shared-runtime operation before Play; the runtime plan comes from official Mojang metadata, and its per-file SHA-1 acquisitions follow the same cache trust boundary.

The original installer awaited each of 4,591 asset objects, 83 libraries, and 402 Java runtime files one at a time. It constructed a new `reqwest::Client` for each HTTP request, preventing connection-pool reuse across requests. Artifacts were already streamed to staging with an expected hash and size check, then atomically promoted; no whole-artifact RAM buffering was found. Cache hits were re-hashed, staged copies were re-hashed, and final read-only validation re-hashed installed files. Those passes protect different trust transitions. The asset index must precede object planning; Mojang and Fabric metadata resolution is a short sequential waterfall (about 0.6 seconds in the live run), so overlapping it was not a priority. The pinned reqwest feature set has no HTTP/2 feature; pooled HTTP/1.1 connections can now be reused when the remote server permits keep-alive.

The per-instance and exact-runtime mutexes exclude overlapping mutation only for their own identity. Registry locking covers short read-modify-write windows, and the cache has no global acquisition lock. Cache races may duplicate a download but cannot promote unverified bytes. Progress is reported once per completed artifact and staged copy, then forwarded as Rust-owned Tauri events; the production UI displays aggregate counts. No aggregate directory scan drives progress.

## Measurements

The controlled game diagnostic serves 192 distinct small assets with 18 ms fixed server latency and runs the real installer core. A first baseline run **before any performance edit** was 5,400 ms cold, 376 ms warm reinstall, and 292 ms for a second instance. The final three-run medians are below. The loopback server closes each connection, so it measures bounded acquisition and filesystem work rather than keep-alive benefit.

| Controlled game scenario | Before | After median | Change | Artifact requests after | Max active requests after |
| --- | ---: | ---: | ---: | ---: | ---: |
| Cold | 5,400 ms | 1,505 ms | 72.1% faster | 198 | 16 |
| Warm reinstall | 376 ms | 228 ms | 39.4% faster | 0 | 0 |
| Second instance | 292 ms | 195 ms | 33.2% faster | 0 | 0 |

The controlled managed-Java diagnostic serves 130 files with the same latency. Cold installation changed from 3,251 ms with one active request to 1,001 ms with 16; warm validation reused the runtime with zero requests (9 ms before, 5 ms after). The game concurrency limit was compared at 8, 16, 24, and 32. Cold controlled results were 1,525, 1,578, 1,466, and 1,406 ms respectively in one sequence; the small gains beyond 16 did not justify more simultaneous sockets and file writes, so the default is 16. Staged file re-hashing uses eight workers, measured separately on the live warm path.

The live comparison below isolates the later staged-hash change: its first run already had pooled HTTP clients and 16-way acquisition. It is **not** a before-all-optimizations production benchmark. A full production baseline on the original commit was not captured. Both runs used fresh, separate temporary roots and the same production release, but Internet conditions differed.

| Live production scenario | After acquisition, before staged-hash change | Final | Observed change |
| --- | ---: | ---: | ---: |
| Cold | 71.396 s | 65.784 s | 7.9% faster |
| Warm reinstall | 26.988 s | 16.317 s | 39.5% faster |
| Second instance | 26.112 s | 15.629 s | 40.1% faster |

| Live phase | After acquisition, before staged-hash change | Final | Interpretation |
| --- | ---: | ---: | --- |
| Cold asset-object acquisition | 41.693 s | 48.541 s | Internet conditions varied; this does not isolate a code regression. |
| Cold library acquisition | 2.120 s | 2.563 s | Both runs used bounded acquisition. |
| Cold materialization | 1.984 s | 2.113 s | Verified-cache copies retained. |
| Cold staged verification | 18.792 s | 6.491 s | Eight blocking hash workers reduced this pass by 65.5%. |
| Warm staged verification | not recorded | 6.535 s | All staged files still checked. |
| Warm game artifact downloads | 0 | 0 | Both warm paths used verified cache. |
| Final cold game artifact bytes | not recorded | 556,703,015 | 4,677 files, including 4,591 asset objects. |

The live cold numbers are single runs under different Internet conditions. The final cold run downloaded 4,677 game files totaling 556,703,015 bytes; asset acquisition took 48.5 seconds, versus 41.7 seconds in the earlier run. That network variation masks the concurrency benefit seen under controlled latency. The most reliable live gain was staged verification of the same roughly 557 MB: 18.8 seconds before versus 6.5 seconds after, with the same SHA/size checks. The warm and second-instance final runs had 4,677 verified game-cache hits and **zero game artifact downloads** each. Aurora/Fabric API cache hits were also reused; metadata is still refreshed according to current policy. Local copies and final validation remain substantial warm costs. The real Java 21 runtime resolved in 3.744 seconds, installed in 9.496 seconds (203 downloaded files, 99,033,638 bytes; 199 shared-cache hits), and reused with zero requests in 0.626 seconds.

The final live test observed a process snapshot near 35 MB working set, 453 handles, and 29 threads during warm installation. No 429 response was observed. These are observations, not a resource-load benchmark. The controlled server proved actual concurrency above one and never above the configured bound. Hash-operation counts were not instrumented in the lower-level verifier; expected read passes are streaming download or cache-hit verification, staged-copy verification, and final instance validation. Corrupt-cache replacement and promotion races can add extra checks.

## Decisions and verification

HTTP clients are shared by exact transport options **and Tokio runtime identity**. The runtime key prevents a client whose dispatch tasks died with one test runtime from being reused in another. Library, asset, and runtime acquisitions run in bounded batches of 16. Each batch settles before reporting an error, and manifest order remains deterministic. Staged verification runs in bounded batches of eight blocking workers, with results collected in original file order. Downloads still stream and hash against published expectations; no copy, digest check, or cache revalidation was removed. Hard links were not introduced because shared mutation would couple instance and cache bytes. No retry policy was added because transient-failure data did not justify it, and permanent hash/metadata errors must fail promptly.

Opt-in `AURORA_INSTALL_DIAGNOSTICS=1` prints aggregate monotonic phase timings and cache/download counts locally, without secrets, per-object log spam, or telemetry. The ignored tests `benchmark_controlled_game_installation`, `benchmark_controlled_runtime_installation`, `benchmark_live_production_instances`, `benchmark_existing_production_warm_install`, and `benchmark_live_production_managed_java` provide repeatable paths. A live benchmark root is retained for inspection; only the uniquely created controlled loopback roots are removed by their tests.

The remaining dominant cold cost is network transfer of approximately 557 MB of game content, especially 452 MB of assets. The remaining warm costs are cache revalidation, isolated instance copying, staged hashing, and final deep validation. A later phase may measure progress-event pressure or safe filesystem techniques, but neither was changed without measured benefit or a proof of the existing isolation contract.

The optimized release executable booted and displayed the existing accepted instance as ready, with managed Java 21 ready and the existing account signed in. The developer manually created a new production instance during UI inspection; it reached `ready` and remains unselected by this investigation. At the developer's request to skip the agent's UI install, no agent Play action or clean-exit supervision check was performed on that instance; live Play acceptance is therefore unverified in this phase. The live benchmark instances passed the Rust deep-validation path in isolated temporary roots.

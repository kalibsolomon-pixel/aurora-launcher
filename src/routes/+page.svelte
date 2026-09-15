<script lang="ts">
  import { onMount } from "svelte";
  import {
    acquireArtifact,
    getApplicationStatus,
    getLauncherState,
    planMinecraftInstall,
    LauncherBackendError,
    type AcquiredArtifact,
    type ApplicationStatus,
    type LauncherState,
    type MinecraftPlanSummary,
  } from "$lib/backend";

  let status = $state<ApplicationStatus | null>(null);
  let launcherState = $state<LauncherState | null>(null);
  let statusError = $state<LauncherBackendError | null>(null);
  let stateError = $state<LauncherBackendError | null>(null);

  // Development proof of the native acquisition pipeline; stripped from
  // production builds.
  const devPipelineProof = import.meta.env.DEV;
  let artifactUrl = $state("");
  let artifactSha256 = $state("");
  let artifactSize = $state("");
  let acquisitionBusy = $state(false);
  let acquisition = $state<AcquiredArtifact | null>(null);
  let acquisitionError = $state<LauncherBackendError | null>(null);

  // Development proof of the Minecraft metadata-resolution layer; stripped
  // from production builds.
  let minecraftVersion = $state("");
  let planningBusy = $state(false);
  let planSummary = $state<MinecraftPlanSummary | null>(null);
  let planningError = $state<LauncherBackendError | null>(null);

  onMount(async () => {
    try {
      status = await getApplicationStatus();
    } catch (cause: unknown) {
      statusError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "The launcher status could not be loaded.");
    }

    try {
      launcherState = await getLauncherState();
    } catch (cause: unknown) {
      stateError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "The launcher state could not be loaded.");
    }
  });

  async function runAcquisition(event: SubmitEvent) {
    event.preventDefault();
    acquisitionBusy = true;
    acquisition = null;
    acquisitionError = null;

    const parsedSize = artifactSize.trim() === "" ? null : Number(artifactSize);
    try {
      acquisition = await acquireArtifact({
        url: artifactUrl.trim(),
        sha256: artifactSha256.trim(),
        sizeBytes: parsedSize !== null && Number.isFinite(parsedSize) ? parsedSize : null,
      });
    } catch (cause: unknown) {
      acquisitionError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "The artifact acquisition failed.");
    } finally {
      acquisitionBusy = false;
    }
  }

  async function runPlanning(event: SubmitEvent) {
    event.preventDefault();
    planningBusy = true;
    planSummary = null;
    planningError = null;

    try {
      planSummary = await planMinecraftInstall({ version: minecraftVersion.trim() });
    } catch (cause: unknown) {
      planningError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "The installation plan failed.");
    } finally {
      planningBusy = false;
    }
  }
</script>

<svelte:head>
  <title>Aurora Launcher</title>
</svelte:head>

<main>
  <section class="hero" aria-labelledby="app-title">
    <p class="eyebrow">A lightweight home for Aurora</p>
    <h1 id="app-title">Aurora Launcher</h1>
    <p class="summary">
      A clean, isolated foundation for the Aurora client mod for Minecraft: Java Edition.
    </p>
  </section>

  <section class="status-card" aria-labelledby="status-title" aria-live="polite">
    <div class="status-heading">
      <div>
        <p class="eyebrow">Native connection</p>
        <h2 id="status-title">Launcher status</h2>
      </div>

      {#if status}
        <span class="badge ready"><span aria-hidden="true"></span>Ready</span>
      {:else if statusError}
        <span class="badge error"><span aria-hidden="true"></span>Unavailable</span>
      {:else}
        <span class="badge loading"><span aria-hidden="true"></span>Connecting</span>
      {/if}
    </div>

    {#if status}
      <dl>
        <div>
          <dt>Launcher version</dt>
          <dd>{status.launcherVersion}</dd>
        </div>
        <div>
          <dt>Platform</dt>
          <dd>{status.platform.os} / {status.platform.architecture}</dd>
        </div>
        <div class="path-row">
          <dt>Managed data root</dt>
          <dd>{status.managedDataRoot}</dd>
        </div>
      </dl>
      <p class="footnote">No Minecraft installation or account data is accessed in this phase.</p>
    {:else if statusError}
      <div class="error-message" role="alert">
        <p>{statusError.message}</p>
        <code>{statusError.code}</code>
      </div>
    {:else}
      <div class="loading-message">
        <span class="spinner" aria-hidden="true"></span>
        <p>Requesting status from the native launcher core…</p>
      </div>
    {/if}
  </section>

  <section class="status-card" aria-labelledby="state-title" aria-live="polite">
    <div class="status-heading">
      <div>
        <p class="eyebrow">Persisted model</p>
        <h2 id="state-title">Launcher state</h2>
      </div>

      {#if launcherState}
        <span class="badge ready"><span aria-hidden="true"></span>Loaded</span>
      {:else if stateError}
        <span class="badge error"><span aria-hidden="true"></span>Unavailable</span>
      {:else}
        <span class="badge loading"><span aria-hidden="true"></span>Loading</span>
      {/if}
    </div>

    {#if launcherState}
      <dl>
        <div>
          <dt>Config schema version</dt>
          <dd>{launcherState.config.schemaVersion}</dd>
        </div>
        <div>
          <dt>Selected instance</dt>
          <dd>{launcherState.config.selectedInstanceId ?? "None"}</dd>
        </div>
        <div>
          <dt>Known instances</dt>
          <dd>{launcherState.instances.length}</dd>
        </div>
        {#each launcherState.instances as instance (instance.id)}
          <div>
            <dt>{instance.id}</dt>
            <dd>
              {instance.displayName} · {instance.channel}{#if instance.auroraVersion}
                · Aurora {instance.auroraVersion}
              {/if}
            </dd>
          </div>
        {/each}
      </dl>
      <p class="footnote">
        Instances cannot be created yet; instance management arrives in a later phase.
      </p>
    {:else if stateError}
      <div class="error-message" role="alert">
        <p>{stateError.message}</p>
        <code>{stateError.code}</code>
      </div>
    {:else}
      <div class="loading-message">
        <span class="spinner" aria-hidden="true"></span>
        <p>Loading persisted launcher state…</p>
      </div>
    {/if}
  </section>

  {#if devPipelineProof}
    <section class="status-card" aria-labelledby="acquire-title" aria-live="polite">
      <div class="status-heading">
        <div>
          <p class="eyebrow">Native pipeline proof</p>
          <h2 id="acquire-title">Artifact acquisition</h2>
        </div>

        {#if acquisitionBusy}
          <span class="badge loading"><span aria-hidden="true"></span>Acquiring</span>
        {:else if acquisition}
          <span class="badge ready"><span aria-hidden="true"></span>Verified</span>
        {:else if acquisitionError}
          <span class="badge error"><span aria-hidden="true"></span>Rejected</span>
        {/if}
      </div>

      <form class="acquire-form" onsubmit={runAcquisition}>
        <label>
          <span>Artifact URL (HTTPS)</span>
          <input type="url" bind:value={artifactUrl} placeholder="https://…" required />
        </label>
        <label>
          <span>Expected SHA-256</span>
          <input
            type="text"
            bind:value={artifactSha256}
            placeholder="64 hexadecimal characters"
            required
            spellcheck="false"
          />
        </label>
        <label>
          <span>Expected size in bytes (optional)</span>
          <input type="number" min="1" bind:value={artifactSize} placeholder="optional" />
        </label>
        <button type="submit" disabled={acquisitionBusy}>
          {acquisitionBusy ? "Acquiring…" : "Acquire into verified cache"}
        </button>
      </form>

      {#if acquisition}
        <dl>
          <div>
            <dt>Result</dt>
            <dd>
              {acquisition.origin === "cacheHit"
                ? "Cache hit — existing object revalidated"
                : "Downloaded and verified"}
            </dd>
          </div>
          <div>
            <dt>Verified bytes</dt>
            <dd>{acquisition.bytes}</dd>
          </div>
          <div>
            <dt>SHA-256</dt>
            <dd>{acquisition.sha256}</dd>
          </div>
          <div class="path-row">
            <dt>Verified object</dt>
            <dd>{acquisition.path}</dd>
          </div>
        </dl>
      {:else if acquisitionError}
        <div class="error-message" role="alert">
          <p>{acquisitionError.message}</p>
          <code>{acquisitionError.code}</code>
        </div>
      {/if}

      <p class="footnote">
        Development-only proof of the native download, verification, and promotion
        pipeline. Installation features are not implemented in this phase.
      </p>
    </section>

    <section class="status-card" aria-labelledby="plan-title" aria-live="polite">
      <div class="status-heading">
        <div>
          <p class="eyebrow">Native resolution proof</p>
          <h2 id="plan-title">Minecraft install planning</h2>
        </div>

        {#if planningBusy}
          <span class="badge loading"><span aria-hidden="true"></span>Resolving</span>
        {:else if planSummary}
          <span class="badge ready"><span aria-hidden="true"></span>Planned</span>
        {:else if planningError}
          <span class="badge error"><span aria-hidden="true"></span>Rejected</span>
        {/if}
      </div>

      <form class="acquire-form" onsubmit={runPlanning}>
        <label>
          <span>Exact Minecraft version</span>
          <input
            type="text"
            bind:value={minecraftVersion}
            placeholder="e.g. 1.21.11 or 26.2"
            required
            spellcheck="false"
          />
        </label>
        <button type="submit" disabled={planningBusy}>
          {planningBusy ? "Resolving…" : "Resolve installation plan"}
        </button>
      </form>

      {#if planSummary}
        <dl>
          <div>
            <dt>Minecraft</dt>
            <dd>{planSummary.minecraftVersion} ({planSummary.versionType})</dd>
          </div>
          <div>
            <dt>Java</dt>
            <dd>{planSummary.javaComponent} (major {planSummary.javaMajorVersion})</dd>
          </div>
          <div>
            <dt>Libraries</dt>
            <dd>
              {planSummary.libraryCount} applicable · {planSummary.nativeLibraryCount} native
              artifacts
            </dd>
          </div>
          <div>
            <dt>Asset index</dt>
            <dd>resolved ({planSummary.assetIndexId})</dd>
          </div>
          <div>
            <dt>Client</dt>
            <dd>resolved ({planSummary.clientSizeBytes.toLocaleString()} bytes)</dd>
          </div>
          <div>
            <dt>Main class</dt>
            <dd>{planSummary.mainClass}</dd>
          </div>
          <div>
            <dt>Launch arguments</dt>
            <dd>
              {planSummary.gameArgumentCount} game · {planSummary.jvmArgumentCount} JVM
            </dd>
          </div>
        </dl>
      {:else if planningError}
        <div class="error-message" role="alert">
          <p>{planningError.message}</p>
          <code>{planningError.code}</code>
        </div>
      {/if}

      <p class="footnote">
        Development-only proof of the native metadata-resolution layer: official
        discovery, a SHA-1-verified version document, and platform-aware planning.
        Nothing is installed and no game artifact is downloaded.
      </p>
    </section>
  {/if}
</main>

<style>
  :global(*) {
    box-sizing: border-box;
  }

  :global(html) {
    min-width: 320px;
    color: #eef4ff;
    background: #0b0e16;
    font-family:
      Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    font-synthesis: none;
    text-rendering: optimizeLegibility;
  }

  :global(body) {
    min-width: 320px;
    min-height: 100vh;
    margin: 0;
    background:
      radial-gradient(circle at 78% 8%, rgba(112, 93, 242, 0.16), transparent 30rem),
      linear-gradient(145deg, #0b0e16 0%, #111524 100%);
  }

  main {
    width: min(100% - 3rem, 860px);
    min-height: 100vh;
    margin: 0 auto;
    padding: clamp(3rem, 10vh, 6.5rem) 0 3rem;
  }

  .hero {
    max-width: 690px;
    margin-bottom: 2.25rem;
  }

  .eyebrow {
    margin: 0 0 0.45rem;
    color: #a99dff;
    font-size: 0.75rem;
    font-weight: 700;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  h1,
  h2,
  p {
    margin-top: 0;
  }

  h1 {
    margin-bottom: 0.7rem;
    font-size: clamp(2.6rem, 7vw, 4.6rem);
    line-height: 0.98;
    letter-spacing: -0.055em;
  }

  h2 {
    margin-bottom: 0;
    font-size: 1.25rem;
    letter-spacing: -0.02em;
  }

  .summary {
    margin-bottom: 0;
    color: #aab5ca;
    font-size: 1.05rem;
    line-height: 1.65;
  }

  .status-card {
    overflow: hidden;
    border: 1px solid #252b3f;
    border-radius: 16px;
    background: rgba(18, 22, 35, 0.84);
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.24);
  }

  .status-card + .status-card {
    margin-top: 1.5rem;
  }

  .status-heading {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 1.4rem 1.5rem;
    border-bottom: 1px solid #252b3f;
  }

  .status-heading .eyebrow {
    margin-bottom: 0.2rem;
  }

  .badge {
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.42rem 0.7rem;
    border-radius: 999px;
    font-size: 0.8rem;
    font-weight: 700;
  }

  .badge span {
    width: 0.46rem;
    height: 0.46rem;
    border-radius: 50%;
    background: currentColor;
  }

  .ready {
    color: #78e6ba;
    background: rgba(50, 172, 125, 0.12);
  }

  .error {
    color: #ff9a9a;
    background: rgba(201, 68, 68, 0.13);
  }

  .loading {
    color: #b8c0d3;
    background: rgba(132, 143, 168, 0.11);
  }

  dl {
    margin: 0;
  }

  dl div {
    display: grid;
    grid-template-columns: minmax(9rem, 0.55fr) minmax(0, 1fr);
    gap: 1.5rem;
    padding: 1rem 1.5rem;
    border-bottom: 1px solid #20263a;
  }

  dt {
    color: #818ca4;
    font-size: 0.86rem;
  }

  dd {
    min-width: 0;
    margin: 0;
    color: #e7ecf7;
    font-size: 0.9rem;
    font-weight: 600;
    overflow-wrap: anywhere;
  }

  .footnote,
  .loading-message,
  .error-message {
    margin: 0;
    padding: 1rem 1.5rem;
    color: #818ca4;
    font-size: 0.82rem;
  }

  .acquire-form {
    display: grid;
    gap: 0.9rem;
    padding: 1.25rem 1.5rem 0.5rem;
  }

  .acquire-form label {
    display: grid;
    gap: 0.35rem;
  }

  .acquire-form label span {
    color: #818ca4;
    font-size: 0.86rem;
  }

  .acquire-form input {
    width: 100%;
    padding: 0.6rem 0.75rem;
    border: 1px solid #2a3149;
    border-radius: 8px;
    background: #10141f;
    color: #e7ecf7;
    font: inherit;
    font-size: 0.9rem;
  }

  .acquire-form input:focus {
    border-color: #a99dff;
    outline: none;
  }

  .acquire-form button {
    justify-self: start;
    padding: 0.55rem 1.1rem;
    border: none;
    border-radius: 8px;
    background: #6f5df2;
    color: #ffffff;
    font: inherit;
    font-size: 0.88rem;
    font-weight: 700;
    cursor: pointer;
  }

  .acquire-form button:disabled {
    opacity: 0.6;
    cursor: progress;
  }

  .loading-message {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    min-height: 8rem;
  }

  .loading-message p,
  .error-message p {
    margin-bottom: 0;
  }

  .spinner {
    width: 1rem;
    height: 1rem;
    border: 2px solid #394158;
    border-top-color: #a99dff;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  .error-message code {
    display: inline-block;
    margin-top: 0.7rem;
    color: #ff9a9a;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (max-width: 620px) {
    main {
      width: min(100% - 1.5rem, 860px);
      padding-top: 2.25rem;
    }

    dl div {
      grid-template-columns: 1fr;
      gap: 0.3rem;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .spinner {
      animation: none;
    }
  }
</style>

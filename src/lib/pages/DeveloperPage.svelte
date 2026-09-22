<script lang="ts">
  import { launcher } from "$lib/launcher/store.svelte";
</script>

<!--
  Development-only proofs of the native pipelines; stripped from production
  builds exactly as before the shell/Home pilot.
-->
<div class="page">
  <header class="page-header">
    <div>
      <h2 class="page-title">Developer</h2>
      <p class="page-subtitle">
        Development-only proofs of the native pipeline boundaries.
      </p>
    </div>
  </header>

  <section class="status-card" aria-labelledby="acquire-title" aria-live="polite">
    <div class="status-heading">
      <div>
        <p class="eyebrow">Native pipeline proof</p>
        <h3 id="acquire-title">Artifact acquisition</h3>
      </div>

      {#if launcher.acquisitionBusy}
        <span class="badge loading"><span aria-hidden="true"></span>Acquiring</span>
      {:else if launcher.acquisition}
        <span class="badge ready"><span aria-hidden="true"></span>Verified</span>
      {:else if launcher.acquisitionError}
        <span class="badge error"><span aria-hidden="true"></span>Rejected</span>
      {/if}
    </div>

    <form class="acquire-form" onsubmit={(event) => launcher.runAcquisition(event)}>
      <label>
        <span>Artifact URL (HTTPS)</span>
        <input type="url" bind:value={launcher.artifactUrl} placeholder="https://…" required />
      </label>
      <label>
        <span>Expected SHA-256</span>
        <input
          type="text"
          bind:value={launcher.artifactSha256}
          placeholder="64 hexadecimal characters"
          required
          spellcheck="false"
        />
      </label>
      <label>
        <span>Expected size in bytes (optional)</span>
        <input
          type="number"
          min="1"
          bind:value={launcher.artifactSize}
          placeholder="optional"
        />
      </label>
      <button type="submit" disabled={launcher.acquisitionBusy}>
        {launcher.acquisitionBusy ? "Acquiring…" : "Acquire into verified cache"}
      </button>
    </form>

    {#if launcher.acquisition}
      <dl>
        <div>
          <dt>Result</dt>
          <dd>
            {launcher.acquisition.origin === "cacheHit"
              ? "Cache hit — existing object revalidated"
              : "Downloaded and verified"}
          </dd>
        </div>
        <div>
          <dt>Verified bytes</dt>
          <dd>{launcher.acquisition.bytes}</dd>
        </div>
        <div>
          <dt>SHA-256</dt>
          <dd>{launcher.acquisition.sha256}</dd>
        </div>
        <div class="path-row">
          <dt>Verified object</dt>
          <dd>{launcher.acquisition.path}</dd>
        </div>
      </dl>
    {:else if launcher.acquisitionError}
      <div class="error-message" role="alert">
        <p>{launcher.acquisitionError.message}</p>
        <code>{launcher.acquisitionError.code}</code>
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
        <h3 id="plan-title">Minecraft install planning</h3>
      </div>

      {#if launcher.planningBusy}
        <span class="badge loading"><span aria-hidden="true"></span>Resolving</span>
      {:else if launcher.planSummary}
        <span class="badge ready"><span aria-hidden="true"></span>Planned</span>
      {:else if launcher.planningError}
        <span class="badge error"><span aria-hidden="true"></span>Rejected</span>
      {/if}
    </div>

    <form class="acquire-form" onsubmit={(event) => launcher.runPlanning(event)}>
      <label>
        <span>Exact Minecraft version</span>
        <input
          type="text"
          bind:value={launcher.minecraftVersion}
          placeholder="e.g. 1.21.11 or 26.2"
          required
          spellcheck="false"
        />
      </label>
      <button type="submit" disabled={launcher.planningBusy}>
        {launcher.planningBusy ? "Resolving…" : "Resolve installation plan"}
      </button>
    </form>

    {#if launcher.planSummary}
      <dl>
        <div>
          <dt>Minecraft</dt>
          <dd>{launcher.planSummary.minecraftVersion} ({launcher.planSummary.versionType})</dd>
        </div>
        <div>
          <dt>Java</dt>
          <dd>{launcher.planSummary.javaComponent} (major {launcher.planSummary.javaMajorVersion})</dd>
        </div>
        <div>
          <dt>Libraries</dt>
          <dd>
            {launcher.planSummary.libraryCount} applicable · {launcher.planSummary.nativeLibraryCount} native
            artifacts
          </dd>
        </div>
        <div>
          <dt>Asset index</dt>
          <dd>resolved ({launcher.planSummary.assetIndexId})</dd>
        </div>
        <div>
          <dt>Client</dt>
          <dd>resolved ({launcher.planSummary.clientSizeBytes.toLocaleString()} bytes)</dd>
        </div>
        <div>
          <dt>Main class</dt>
          <dd>{launcher.planSummary.mainClass}</dd>
        </div>
        <div>
          <dt>Launch arguments</dt>
          <dd>
            {launcher.planSummary.gameArgumentCount} game · {launcher.planSummary.jvmArgumentCount} JVM
          </dd>
        </div>
      </dl>
    {:else if launcher.planningError}
      <div class="error-message" role="alert">
        <p>{launcher.planningError.message}</p>
        <code>{launcher.planningError.code}</code>
      </div>
    {/if}

    <p class="footnote">
      Development-only proof of the native metadata-resolution layer: official
      discovery, a SHA-1-verified version document, and platform-aware planning.
      Nothing is installed and no game artifact is downloaded.
    </p>
  </section>

  <section class="status-card" aria-labelledby="fabric-plan-title" aria-live="polite">
    <div class="status-heading">
      <div>
        <p class="eyebrow">Native composition proof</p>
        <h3 id="fabric-plan-title">Fabric install planning</h3>
      </div>

      {#if launcher.fabricPlanningBusy}
        <span class="badge loading"><span aria-hidden="true"></span>Composing</span>
      {:else if launcher.fabricPlanSummary}
        <span class="badge ready"><span aria-hidden="true"></span>Planned</span>
      {:else if launcher.fabricPlanningError}
        <span class="badge error"><span aria-hidden="true"></span>Rejected</span>
      {/if}
    </div>

    <form class="acquire-form" onsubmit={(event) => launcher.runFabricPlanning(event)}>
      <label>
        <span>Exact Minecraft version</span>
        <input
          type="text"
          bind:value={launcher.fabricMinecraftVersion}
          placeholder="e.g. 26.2 or 1.21.11"
          required
          spellcheck="false"
        />
      </label>
      <label>
        <span>Exact Fabric Loader version</span>
        <input
          type="text"
          bind:value={launcher.fabricLoaderVersion}
          placeholder="e.g. 0.19.5"
          required
          spellcheck="false"
        />
      </label>
      <button type="submit" disabled={launcher.fabricPlanningBusy}>
        {launcher.fabricPlanningBusy ? "Composing…" : "Compose game plan"}
      </button>
    </form>

    {#if launcher.fabricPlanSummary}
      <dl>
        <div>
          <dt>Minecraft</dt>
          <dd>{launcher.fabricPlanSummary.minecraftVersion}</dd>
        </div>
        <div>
          <dt>Fabric Loader</dt>
          <dd>{launcher.fabricPlanSummary.loaderVersion}</dd>
        </div>
        <div>
          <dt>Vanilla libraries</dt>
          <dd>{launcher.fabricPlanSummary.vanillaLibraryCount}</dd>
        </div>
        <div>
          <dt>Fabric libraries</dt>
          <dd>
            {launcher.fabricPlanSummary.fabricLibraryCount}
            ({launcher.fabricPlanSummary.fabricDigestedLibraryCount} with official digests)
          </dd>
        </div>
        <div>
          <dt>Final libraries</dt>
          <dd>{launcher.fabricPlanSummary.finalLibraryCount}</dd>
        </div>
        <div>
          <dt>Java</dt>
          <dd>
            {launcher.fabricPlanSummary.javaComponent} (major {launcher.fabricPlanSummary.javaMajorVersion}){#if launcher.fabricPlanSummary.javaRaisedByLoader}
              — raised by the loader{/if}
          </dd>
        </div>
        <div>
          <dt>Final main class</dt>
          <dd>{launcher.fabricPlanSummary.finalMainClass}</dd>
        </div>
      </dl>
    {:else if launcher.fabricPlanningError}
      <div class="error-message" role="alert">
        <p>{launcher.fabricPlanningError.message}</p>
        <code>{launcher.fabricPlanningError.code}</code>
      </div>
    {/if}

    <p class="footnote">
      Development-only proof of the native Fabric layer: official loader discovery,
      exact profile resolution, and composition with the vanilla plan. Nothing is
      installed and no Minecraft or Fabric artifact is downloaded.
    </p>
  </section>

  <section class="status-card" aria-labelledby="install-title" aria-live="polite">
    <div class="status-heading">
      <div>
        <p class="eyebrow">Native installation proof</p>
        <h3 id="install-title">Game installation</h3>
      </div>

      {#if launcher.installBusy}
        <span class="badge loading"><span aria-hidden="true"></span
          >{launcher.installProgress ? launcher.installProgress.phase : "Installing"}</span
        >
      {:else if launcher.installSummary}
        <span class="badge ready"><span aria-hidden="true"></span>Installed</span>
      {:else if launcher.installError}
        <span class="badge error"><span aria-hidden="true"></span>Failed</span>
      {/if}
    </div>

    <form class="acquire-form" onsubmit={(event) => launcher.runInstall(event)}>
      <label>
        <span>Instance id (managed storage)</span>
        <input
          type="text"
          bind:value={launcher.installInstanceId}
          placeholder="e.g. dev-install"
          required
          spellcheck="false"
        />
      </label>
      <label>
        <span>Exact Minecraft version</span>
        <input
          type="text"
          bind:value={launcher.installMinecraftVersion}
          placeholder="e.g. 26.2"
          required
          spellcheck="false"
        />
      </label>
      <label>
        <span>Exact Fabric Loader version</span>
        <input
          type="text"
          bind:value={launcher.installLoaderVersion}
          placeholder="e.g. 0.19.5"
          required
          spellcheck="false"
        />
      </label>
      <button type="submit" disabled={launcher.installBusy}>
        {launcher.installBusy ? "Installing…" : "Install isolated game"}
      </button>
    </form>

    {#if launcher.installBusy && launcher.installProgress}
      <dl>
        <div>
          <dt>Phase</dt>
          <dd>{launcher.installProgress.phase}</dd>
        </div>
        <div>
          <dt>Progress</dt>
          <dd>
            {launcher.installProgress.completedItems} / {launcher.installProgress.totalItems}
            {#if launcher.installProgress.currentItem}· {launcher.installProgress.currentItem}{/if}
          </dd>
        </div>
      </dl>
    {:else if launcher.installSummary}
      <dl>
        <div>
          <dt>Installed</dt>
          <dd>
            Minecraft {launcher.installSummary.minecraftVersion} + Fabric Loader
            {launcher.installSummary.loaderVersion}
          </dd>
        </div>
        <div>
          <dt>Files</dt>
          <dd>
            {launcher.installSummary.fileCount} managed files
            ({launcher.installSummary.totalBytes.toLocaleString()} bytes)
          </dd>
        </div>
        <div>
          <dt>Trust classes</dt>
          <dd>
            {launcher.installSummary.verifiedSha1Files} SHA-1 verified ·
            {launcher.installSummary.verifiedSha256Files} SHA-256 verified ·
            {launcher.installSummary.transportObservedFiles} secure-transport observed
          </dd>
        </div>
        <div class="path-row">
          <dt>Game directory</dt>
          <dd>{launcher.installSummary.gameDirectory}</dd>
        </div>
      </dl>
    {:else if launcher.installError}
      <div class="error-message" role="alert">
        <p>{launcher.installError.message}</p>
        <code>{launcher.installError.code}</code>
      </div>
    {/if}

    <form class="acquire-form" onsubmit={(event) => launcher.runValidation(event)}>
      <button
        type="submit"
        disabled={launcher.validationBusy || launcher.installInstanceId.trim() === ""}
      >
        {launcher.validationBusy ? "Validating…" : "Validate installed game"}
      </button>
    </form>

    {#if launcher.validation}
      <dl>
        <div>
          <dt>Status</dt>
          <dd>{launcher.validation.status}</dd>
        </div>
        {#if launcher.validation.installationId}
          <div>
            <dt>Installation</dt>
            <dd>
              {launcher.validation.minecraftVersion} + {launcher.validation.loaderVersion} ·
              {launcher.validation.installationId}
            </dd>
          </div>
        {/if}
        {#if launcher.validation.checkedFiles > 0}
          <div>
            <dt>Verified</dt>
            <dd>
              {launcher.validation.checkedFiles} files ({launcher.validation.verifiedBytes.toLocaleString()} bytes)
            </dd>
          </div>
        {/if}
        {#each launcher.validation.problems as problem (problem.path)}
          <div>
            <dt>{problem.path}</dt>
            <dd>{problem.reason}</dd>
          </div>
        {/each}
      </dl>
    {:else if launcher.validationError}
      <div class="error-message" role="alert">
        <p>{launcher.validationError.message}</p>
        <code>{launcher.validationError.code}</code>
      </div>
    {/if}

    <p class="footnote">
      Development-only proof of the native installation executor: verified acquisition,
      staged materialization, native extraction, validation, and atomic commit into
      launcher-managed instance storage. Nothing is launched; no Java runtime is
      installed; the user's .minecraft is never touched.
    </p>
  </section>
</div>

<style>
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

  .eyebrow {
    margin: 0 0 0.45rem;
    color: #a99dff;
    font-size: 0.75rem;
    font-weight: 700;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  h3 {
    margin: 0;
    font-size: 1.25rem;
    letter-spacing: -0.02em;
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

  .error-message code {
    display: inline-block;
    margin-top: 0.7rem;
    color: #ff9a9a;
  }

  @media (max-width: 620px) {
    dl div {
      grid-template-columns: 1fr;
      gap: 0.3rem;
    }
  }
</style>

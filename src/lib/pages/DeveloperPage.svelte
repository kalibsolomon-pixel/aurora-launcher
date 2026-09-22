<script lang="ts">
  import { launcher } from "$lib/launcher/store.svelte";
</script>

<!--
  Development-only surface: launcher/platform diagnostics plus the native
  pipeline proofs. This page is stripped from production builds together with
  its navigation entry; technical identifiers and raw diagnostics belong
  here, never on the product screens.
-->
<div class="page">
  <header class="page-header">
    <div>
      <h2 class="page-title">Developer</h2>
      <p class="page-subtitle">
        Launcher diagnostics and development-only proofs of the native pipeline boundaries.
      </p>
    </div>
  </header>

  <section class="group" aria-live="polite">
    <div class="group-heading">
      <div>
        <h3 class="group-title">Application</h3>
        <p class="group-subtitle">Native backend status and managed paths.</p>
      </div>
      {#if launcher.status}
        <span class="status-badge status-success">Ready</span>
      {:else if launcher.statusError}
        <span class="status-badge status-error">Unavailable</span>
      {:else}
        <span class="status-badge status-muted">Connecting</span>
      {/if}
    </div>

    {#if launcher.status}
      <div class="group-row">
        <span class="group-row-title">Launcher version</span>
        <span class="group-row-value value-mono">{launcher.status.launcherVersion}</span>
      </div>
      <div class="group-row">
        <span class="group-row-title">Platform</span>
        <span class="group-row-value">
          {launcher.status.platform.os} · {launcher.status.platform.architecture}
        </span>
      </div>
      <div class="group-row">
        <div class="group-row-main">
          <span class="group-row-title">Managed data root</span>
          <span class="group-row-detail value-mono">{launcher.status.managedDataRoot}</span>
        </div>
      </div>
    {:else if launcher.statusError}
      <p class="inline-message inline-message-error group-row" role="alert">
        {launcher.statusError.message}
        <code>{launcher.statusError.code}</code>
      </p>
    {:else}
      <div class="group-row group-row-loading">
        <span class="spinner" aria-hidden="true"></span>
        <span class="group-row-detail">Requesting status from the native launcher core…</span>
      </div>
    {/if}
  </section>

  <section class="group" aria-live="polite">
    <div class="group-heading">
      <div>
        <h3 class="group-title">Persisted launcher state</h3>
        <p class="group-subtitle">Versioned documents under the managed root.</p>
      </div>
      {#if launcher.launcherState}
        <span class="status-badge status-success">Loaded</span>
      {:else if launcher.stateError}
        <span class="status-badge status-error">Unavailable</span>
      {:else}
        <span class="status-badge status-muted">Loading</span>
      {/if}
    </div>

    {#if launcher.launcherState}
      <div class="group-row">
        <span class="group-row-title">Config schema version</span>
        <span class="group-row-value">{launcher.launcherState.config.schemaVersion}</span>
      </div>
      <div class="group-row">
        <span class="group-row-title">Selected instance</span>
        <span class="group-row-value value-mono">
          {launcher.launcherState.config.selectedInstanceId ?? "None"}
        </span>
      </div>
      <div class="group-row">
        <span class="group-row-title">Known instances</span>
        <span class="group-row-value">{launcher.launcherState.instances.length}</span>
      </div>
      {#if launcher.accountsState}
        <div class="group-row">
          <span class="group-row-title">Known accounts</span>
          <span class="group-row-value">
            {launcher.accountsState.accounts.length}
            {#if launcher.accountsState.selectedAccountId}
              · selected {launcher.accountsState.accounts.find((account) => account.accountId === launcher.accountsState?.selectedAccountId)?.minecraftName ?? "unknown"}
            {/if}
          </span>
        </div>
      {/if}
    {:else if launcher.stateError}
      <p class="inline-message inline-message-error group-row" role="alert">
        {launcher.stateError.message}
        <code>{launcher.stateError.code}</code>
      </p>
    {:else}
      <div class="group-row group-row-loading">
        <span class="spinner" aria-hidden="true"></span>
        <span class="group-row-detail">Loading persisted launcher state…</span>
      </div>
    {/if}
  </section>

  <section class="group" aria-live="polite">
    <div class="group-heading">
      <div>
        <h3 class="group-title">Artifact acquisition</h3>
        <p class="group-subtitle">Download, hash-verification, and promotion pipeline.</p>
      </div>
      {#if launcher.acquisitionBusy}
        <span class="status-badge status-working">Acquiring</span>
      {:else if launcher.acquisition}
        <span class="status-badge status-success">Verified</span>
      {:else if launcher.acquisitionError}
        <span class="status-badge status-error">Rejected</span>
      {/if}
    </div>

    <form
      class="group-form"
      onsubmit={(event) => {
        event.preventDefault();
        void launcher.runAcquisition();
      }}
    >
      <div class="field-grid">
        <label class="field">
          <span class="field-label">Artifact URL (HTTPS)</span>
          <input type="url" bind:value={launcher.artifactUrl} placeholder="https://…" required />
        </label>
        <label class="field">
          <span class="field-label">Expected SHA-256</span>
          <input
            type="text"
            bind:value={launcher.artifactSha256}
            placeholder="64 hexadecimal characters"
            required
            spellcheck="false"
          />
        </label>
        <label class="field">
          <span class="field-label">Expected size in bytes (optional)</span>
          <input type="number" min="1" bind:value={launcher.artifactSize} placeholder="optional" />
        </label>
      </div>
      <div class="form-actions">
        <button type="submit" class="btn" disabled={launcher.acquisitionBusy}>
          {launcher.acquisitionBusy ? "Acquiring…" : "Acquire into verified cache"}
        </button>
      </div>
    </form>

    {#if launcher.acquisition}
      <div class="group-row">
        <span class="group-row-title">Result</span>
        <span class="group-row-value">
          {launcher.acquisition.origin === "cacheHit"
            ? "Cache hit — existing object revalidated"
            : "Downloaded and verified"}
        </span>
      </div>
      <div class="group-row">
        <span class="group-row-title">Verified bytes</span>
        <span class="group-row-value">{launcher.acquisition.bytes.toLocaleString()}</span>
      </div>
      <div class="group-row">
        <span class="group-row-title">SHA-256</span>
        <span class="group-row-value value-mono">{launcher.acquisition.sha256}</span>
      </div>
      <div class="group-row">
        <div class="group-row-main">
          <span class="group-row-title">Verified object</span>
          <span class="group-row-detail value-mono">{launcher.acquisition.path}</span>
        </div>
      </div>
    {:else if launcher.acquisitionError}
      <p class="inline-message inline-message-error group-row" role="alert">
        {launcher.acquisitionError.message}
        <code>{launcher.acquisitionError.code}</code>
      </p>
    {/if}

    <p class="group-footer">
      Development-only proof of the native download, verification, and promotion pipeline.
    </p>
  </section>

  <section class="group" aria-live="polite">
    <div class="group-heading">
      <div>
        <h3 class="group-title">Minecraft install planning</h3>
        <p class="group-subtitle">Official metadata resolution into a platform-aware plan.</p>
      </div>
      {#if launcher.planningBusy}
        <span class="status-badge status-working">Resolving</span>
      {:else if launcher.planSummary}
        <span class="status-badge status-success">Planned</span>
      {:else if launcher.planningError}
        <span class="status-badge status-error">Rejected</span>
      {/if}
    </div>

    <form
      class="group-form"
      onsubmit={(event) => {
        event.preventDefault();
        void launcher.runPlanning();
      }}
    >
      <div class="field-grid">
        <label class="field">
          <span class="field-label">Exact Minecraft version</span>
          <input
            type="text"
            bind:value={launcher.minecraftVersion}
            placeholder="e.g. 1.21.11 or 26.2"
            required
            spellcheck="false"
          />
        </label>
      </div>
      <div class="form-actions">
        <button type="submit" class="btn" disabled={launcher.planningBusy}>
          {launcher.planningBusy ? "Resolving…" : "Resolve installation plan"}
        </button>
      </div>
    </form>

    {#if launcher.planSummary}
      <div class="group-row">
        <span class="group-row-title">Minecraft</span>
        <span class="group-row-value">
          {launcher.planSummary.minecraftVersion} ({launcher.planSummary.versionType})
        </span>
      </div>
      <div class="group-row">
        <span class="group-row-title">Java</span>
        <span class="group-row-value">
          {launcher.planSummary.javaComponent} (major {launcher.planSummary.javaMajorVersion})
        </span>
      </div>
      <div class="group-row">
        <span class="group-row-title">Libraries</span>
        <span class="group-row-value">
          {launcher.planSummary.libraryCount} applicable · {launcher.planSummary.nativeLibraryCount}
          native artifacts
        </span>
      </div>
      <div class="group-row">
        <span class="group-row-title">Asset index</span>
        <span class="group-row-value">resolved ({launcher.planSummary.assetIndexId})</span>
      </div>
      <div class="group-row">
        <span class="group-row-title">Client</span>
        <span class="group-row-value">
          resolved ({launcher.planSummary.clientSizeBytes.toLocaleString()} bytes)
        </span>
      </div>
      <div class="group-row">
        <span class="group-row-title">Main class</span>
        <span class="group-row-value value-mono">{launcher.planSummary.mainClass}</span>
      </div>
      <div class="group-row">
        <span class="group-row-title">Launch arguments</span>
        <span class="group-row-value">
          {launcher.planSummary.gameArgumentCount} game · {launcher.planSummary.jvmArgumentCount} JVM
        </span>
      </div>
    {:else if launcher.planningError}
      <p class="inline-message inline-message-error group-row" role="alert">
        {launcher.planningError.message}
        <code>{launcher.planningError.code}</code>
      </p>
    {/if}

    <p class="group-footer">
      Development-only proof of the native metadata-resolution layer: official discovery, a
      SHA-1-verified version document, and platform-aware planning. Nothing is installed and no
      game artifact is downloaded.
    </p>
  </section>

  <section class="group" aria-live="polite">
    <div class="group-heading">
      <div>
        <h3 class="group-title">Fabric install planning</h3>
        <p class="group-subtitle">Fabric Meta resolution and composition with the vanilla plan.</p>
      </div>
      {#if launcher.fabricPlanningBusy}
        <span class="status-badge status-working">Composing</span>
      {:else if launcher.fabricPlanSummary}
        <span class="status-badge status-success">Planned</span>
      {:else if launcher.fabricPlanningError}
        <span class="status-badge status-error">Rejected</span>
      {/if}
    </div>

    <form
      class="group-form"
      onsubmit={(event) => {
        event.preventDefault();
        void launcher.runFabricPlanning();
      }}
    >
      <div class="field-grid">
        <label class="field">
          <span class="field-label">Exact Minecraft version</span>
          <input
            type="text"
            bind:value={launcher.fabricMinecraftVersion}
            placeholder="e.g. 26.2 or 1.21.11"
            required
            spellcheck="false"
          />
        </label>
        <label class="field">
          <span class="field-label">Exact Fabric Loader version</span>
          <input
            type="text"
            bind:value={launcher.fabricLoaderVersion}
            placeholder="e.g. 0.19.5"
            required
            spellcheck="false"
          />
        </label>
      </div>
      <div class="form-actions">
        <button type="submit" class="btn" disabled={launcher.fabricPlanningBusy}>
          {launcher.fabricPlanningBusy ? "Composing…" : "Compose game plan"}
        </button>
      </div>
    </form>

    {#if launcher.fabricPlanSummary}
      <div class="group-row">
        <span class="group-row-title">Minecraft</span>
        <span class="group-row-value">{launcher.fabricPlanSummary.minecraftVersion}</span>
      </div>
      <div class="group-row">
        <span class="group-row-title">Fabric Loader</span>
        <span class="group-row-value">{launcher.fabricPlanSummary.loaderVersion}</span>
      </div>
      <div class="group-row">
        <span class="group-row-title">Vanilla libraries</span>
        <span class="group-row-value">{launcher.fabricPlanSummary.vanillaLibraryCount}</span>
      </div>
      <div class="group-row">
        <span class="group-row-title">Fabric libraries</span>
        <span class="group-row-value">
          {launcher.fabricPlanSummary.fabricLibraryCount}
          ({launcher.fabricPlanSummary.fabricDigestedLibraryCount} with official digests)
        </span>
      </div>
      <div class="group-row">
        <span class="group-row-title">Final libraries</span>
        <span class="group-row-value">{launcher.fabricPlanSummary.finalLibraryCount}</span>
      </div>
      <div class="group-row">
        <span class="group-row-title">Java</span>
        <span class="group-row-value">
          {launcher.fabricPlanSummary.javaComponent} (major
          {launcher.fabricPlanSummary.javaMajorVersion}){#if launcher.fabricPlanSummary.javaRaisedByLoader}
            — raised by the loader
          {/if}
        </span>
      </div>
      <div class="group-row">
        <span class="group-row-title">Final main class</span>
        <span class="group-row-value value-mono">{launcher.fabricPlanSummary.finalMainClass}</span>
      </div>
    {:else if launcher.fabricPlanningError}
      <p class="inline-message inline-message-error group-row" role="alert">
        {launcher.fabricPlanningError.message}
        <code>{launcher.fabricPlanningError.code}</code>
      </p>
    {/if}

    <p class="group-footer">
      Development-only proof of the native Fabric layer: official loader discovery, exact profile
      resolution, and composition with the vanilla plan. Nothing is installed and no Minecraft or
      Fabric artifact is downloaded.
    </p>
  </section>

  <section class="group" aria-live="polite">
    <div class="group-heading">
      <div>
        <h3 class="group-title">Game installation</h3>
        <p class="group-subtitle">
          Staged installation executor into launcher-managed instance storage.
        </p>
      </div>
      {#if launcher.installBusy}
        <span class="status-badge status-working">
          {launcher.installProgress ? launcher.installProgress.phase : "Installing"}
        </span>
      {:else if launcher.installSummary}
        <span class="status-badge status-success">Installed</span>
      {:else if launcher.installError}
        <span class="status-badge status-error">Failed</span>
      {/if}
    </div>

    <form
      class="group-form"
      onsubmit={(event) => {
        event.preventDefault();
        void launcher.runInstall();
      }}
    >
      <div class="field-grid">
        <label class="field">
          <span class="field-label">Instance id (managed storage)</span>
          <input
            type="text"
            bind:value={launcher.installInstanceId}
            placeholder="e.g. dev-install"
            required
            spellcheck="false"
          />
        </label>
        <label class="field">
          <span class="field-label">Exact Minecraft version</span>
          <input
            type="text"
            bind:value={launcher.installMinecraftVersion}
            placeholder="e.g. 26.2"
            required
            spellcheck="false"
          />
        </label>
        <label class="field">
          <span class="field-label">Exact Fabric Loader version</span>
          <input
            type="text"
            bind:value={launcher.installLoaderVersion}
            placeholder="e.g. 0.19.5"
            required
            spellcheck="false"
          />
        </label>
      </div>
      <div class="form-actions">
        <button type="submit" class="btn" disabled={launcher.installBusy}>
          {launcher.installBusy ? "Installing…" : "Install isolated game"}
        </button>
        <button
          type="button"
          class="btn"
          onclick={() => void launcher.runValidation()}
          disabled={launcher.validationBusy || launcher.installInstanceId.trim() === ""}
        >
          {launcher.validationBusy ? "Validating…" : "Validate installed game"}
        </button>
      </div>
    </form>

    {#if launcher.installBusy && launcher.installProgress}
      <div class="group-row">
        <span class="group-row-title">Progress</span>
        <span class="group-row-value">
          {launcher.installProgress.phase} · {launcher.installProgress.completedItems} /
          {launcher.installProgress.totalItems}
          {#if launcher.installProgress.currentItem}· {launcher.installProgress.currentItem}{/if}
        </span>
      </div>
    {:else if launcher.installSummary}
      <div class="group-row">
        <span class="group-row-title">Installed</span>
        <span class="group-row-value">
          Minecraft {launcher.installSummary.minecraftVersion} + Fabric Loader
          {launcher.installSummary.loaderVersion}
        </span>
      </div>
      <div class="group-row">
        <span class="group-row-title">Files</span>
        <span class="group-row-value">
          {launcher.installSummary.fileCount} managed files
          ({launcher.installSummary.totalBytes.toLocaleString()} bytes)
        </span>
      </div>
      <div class="group-row">
        <span class="group-row-title">Trust classes</span>
        <span class="group-row-value">
          {launcher.installSummary.verifiedSha1Files} SHA-1 verified ·
          {launcher.installSummary.verifiedSha256Files} SHA-256 verified ·
          {launcher.installSummary.transportObservedFiles} secure-transport observed
        </span>
      </div>
      <div class="group-row">
        <div class="group-row-main">
          <span class="group-row-title">Game directory</span>
          <span class="group-row-detail value-mono">{launcher.installSummary.gameDirectory}</span>
        </div>
      </div>
    {:else if launcher.installError}
      <p class="inline-message inline-message-error group-row" role="alert">
        {launcher.installError.message}
        <code>{launcher.installError.code}</code>
      </p>
    {/if}

    {#if launcher.validation}
      <div class="group-row">
        <span class="group-row-title">Validation</span>
        <span class="group-row-value">{launcher.validation.status}</span>
      </div>
      {#if launcher.validation.installationId}
        <div class="group-row">
          <span class="group-row-title">Installation</span>
          <span class="group-row-value value-mono">
            {launcher.validation.minecraftVersion} + {launcher.validation.loaderVersion} ·
            {launcher.validation.installationId}
          </span>
        </div>
      {/if}
      {#if launcher.validation.checkedFiles > 0}
        <div class="group-row">
          <span class="group-row-title">Verified</span>
          <span class="group-row-value">
            {launcher.validation.checkedFiles}
            files ({launcher.validation.verifiedBytes.toLocaleString()} bytes)
          </span>
        </div>
      {/if}
      {#each launcher.validation.problems as problem (problem.path)}
        <div class="group-row">
          <div class="group-row-main">
            <span class="group-row-title value-mono">{problem.path}</span>
            <span class="group-row-detail">{problem.reason}</span>
          </div>
        </div>
      {/each}
    {:else if launcher.validationError}
      <p class="inline-message inline-message-error group-row" role="alert">
        {launcher.validationError.message}
        <code>{launcher.validationError.code}</code>
      </p>
    {/if}

    <p class="group-footer">
      Development-only proof of the native installation executor: verified acquisition, staged
      materialization, native extraction, validation, and atomic commit into launcher-managed
      instance storage. Nothing is launched; no Java runtime is installed; the user's .minecraft is
      never touched.
    </p>
  </section>
</div>

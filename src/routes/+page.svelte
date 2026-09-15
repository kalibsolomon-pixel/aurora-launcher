<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import {
    acquireArtifact,
    beginMicrosoftLogin,
    cancelMicrosoftLogin,
    createInstance,
    ensureInstanceRuntime,
    getApplicationStatus,
    getAccounts,
    getInstanceRuntimeStatus,
    getLauncherState,
    installGame,
    listAuroraReleases,
    planFabricInstall,
    planMinecraftInstall,
    refreshAccountSession,
    removeAccount,
    renameInstance,
    retryInstanceInstall,
    selectAccount,
    selectInstance,
    validateInstance,
    validateInstalledGame,
    LauncherBackendError,
    type AcquiredArtifact,
    type AccountSession,
    type AccountSummary,
    type AccountsState,
    type ApplicationStatus,
    type AuroraReleaseSummary,
    type AuroraChannel,
    type AuthProgressEvent,
    type FabricPlanSummary,
    type InstalledGameSummary,
    type InstalledGameValidation,
    type InstanceProgressEvent,
    type InstanceSummary,
    type InstanceValidationDto,
    type InstallProgressEvent,
    type LauncherState,
    type MinecraftPlanSummary,
    type RuntimeProgressEvent,
    type RuntimeStatusDto,
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

  // Development proof of the Fabric resolution and composition layer;
  // stripped from production builds.
  let fabricMinecraftVersion = $state("");
  let fabricLoaderVersion = $state("");
  let fabricPlanningBusy = $state(false);
  let fabricPlanSummary = $state<FabricPlanSummary | null>(null);
  let fabricPlanningError = $state<LauncherBackendError | null>(null);

  // Development proof of the Phase 5 installation executor; stripped from
  // production builds. Progress comes from native events only — nothing is
  // faked or inferred.
  let installInstanceId = $state("");
  let installMinecraftVersion = $state("");
  let installLoaderVersion = $state("");
  let installBusy = $state(false);
  let installProgress = $state<InstallProgressEvent | null>(null);
  let installSummary = $state<InstalledGameSummary | null>(null);
  let installError = $state<LauncherBackendError | null>(null);
  let validationBusy = $state(false);
  let validation = $state<InstalledGameValidation | null>(null);
  let validationError = $state<LauncherBackendError | null>(null);

  // Production instance management (Phase 6). All state comes from Rust.
  let releases = $state<AuroraReleaseSummary[]>([]);
  let releasesError = $state<LauncherBackendError | null>(null);
  let createDisplayName = $state("");
  let createChannel = $state<AuroraChannel>("stable");
  let createVersion = $state("");
  let createBusy = $state(false);
  let createProgress = $state<InstanceProgressEvent | null>(null);
  let createError = $state<LauncherBackendError | null>(null);
  let renaming = $state<{ id: string; name: string } | null>(null);
  let instanceBusy = $state<string | null>(null);
  let instanceValidations = $state<Record<string, InstanceValidationDto>>({});
  let instanceError = $state<LauncherBackendError | null>(null);
  let runtimeStatus = $state<RuntimeStatusDto | null>(null);
  let runtimeBusy = $state(false);
  let runtimeProgress = $state<RuntimeProgressEvent | null>(null);
  let runtimeError = $state<LauncherBackendError | null>(null);

  // Production account surface (Phase 8). All state comes from Rust; the
  // UI never sees tokens.
  let accountsState = $state<AccountsState | null>(null);
  let accountsError = $state<LauncherBackendError | null>(null);
  let signInBusy = $state(false);
  let signInProgress = $state<AuthProgressEvent | null>(null);
  let signInError = $state<LauncherBackendError | null>(null);
  let accountBusy = $state<string | null>(null);
  let accountError = $state<LauncherBackendError | null>(null);
  let accountSessions = $state<Record<string, AccountSession>>({});

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
      const selected = launcherState.instances.find(
        (instance) => instance.id === launcherState?.config.selectedInstanceId,
      );
      if (selected?.state === "ready") void runRuntimeStatus(selected.id);
    } catch (cause: unknown) {
      stateError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "The launcher state could not be loaded.");
    }

    try {
      releases = await listAuroraReleases();
      const preferred = releases.find((release) => release.channel === "stable");
      if (preferred) {
        createChannel = preferred.channel;
        createVersion = preferred.auroraVersion;
      }
    } catch (cause: unknown) {
      releasesError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "The Aurora release list failed.");
    }

    try {
      accountsState = await getAccounts();
    } catch (cause: unknown) {
      accountsError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "The account list could not be loaded.");
    }
  });

  onMount(() => {
    // Native progress events drive the installation displays.
    let unsubscribeInstall: (() => void) | null = null;
    let unsubscribeInstance: (() => void) | null = null;
    let unsubscribeRuntime: (() => void) | null = null;
    let unsubscribeAuth: (() => void) | null = null;
    listen<InstallProgressEvent>("install-progress", (event) => {
      installProgress = event.payload;
    }).then((stop) => {
      unsubscribeInstall = stop;
    });
    listen<InstanceProgressEvent>("instance-progress", (event) => {
      createProgress = event.payload;
    }).then((stop) => {
      unsubscribeInstance = stop;
    });
    listen<RuntimeProgressEvent>("runtime-progress", (event) => {
      runtimeProgress = event.payload;
    }).then((stop) => {
      unsubscribeRuntime = stop;
    });
    listen<AuthProgressEvent>("auth-progress", (event) => {
      signInProgress = event.payload;
    }).then((stop) => {
      unsubscribeAuth = stop;
    });
    return () => {
      unsubscribeInstall?.();
      unsubscribeInstance?.();
      unsubscribeRuntime?.();
      unsubscribeAuth?.();
    };
  });

  async function refreshState() {
    try {
      launcherState = await getLauncherState();
    } catch {
      // State refresh is best-effort after mutations; load errors surface
      // through the dedicated state card.
    }
  }

  async function runCreateInstance(event: SubmitEvent) {
    event.preventDefault();
    createBusy = true;
    createProgress = null;
    createError = null;

    try {
      await createInstance({
        displayName: createDisplayName.trim(),
        channel: createChannel,
        auroraVersion: createVersion,
      });
      createDisplayName = "";
      await refreshState();
    } catch (cause: unknown) {
      createError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "The instance creation failed.");
      await refreshState();
    } finally {
      createBusy = false;
    }
  }

  async function runSelect(id: string) {
    instanceBusy = id;
    instanceError = null;
    try {
      await selectInstance(id);
      await refreshState();
      runtimeStatus = null;
      void runRuntimeStatus(id);
    } catch (cause: unknown) {
      instanceError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "The selection failed.");
    } finally {
      instanceBusy = null;
    }
  }

  async function runRename(event: SubmitEvent) {
    event.preventDefault();
    if (!renaming) return;
    instanceBusy = renaming.id;
    instanceError = null;
    try {
      await renameInstance(renaming.id, renaming.name.trim());
      renaming = null;
      await refreshState();
    } catch (cause: unknown) {
      instanceError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "The rename failed.");
    } finally {
      instanceBusy = null;
    }
  }

  async function runRetry(id: string) {
    instanceBusy = id;
    instanceError = null;
    createProgress = null;
    try {
      await retryInstanceInstall(id);
      await refreshState();
    } catch (cause: unknown) {
      instanceError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "The retry failed.");
      await refreshState();
    } finally {
      instanceBusy = null;
    }
  }

  async function runValidate(id: string) {
    instanceBusy = id;
    instanceError = null;
    try {
      instanceValidations[id] = await validateInstance(id);
    } catch (cause: unknown) {
      instanceError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "The validation failed.");
    } finally {
      instanceBusy = null;
    }
  }

  async function runRuntimeStatus(id: string) {
    runtimeBusy = true;
    runtimeError = null;
    runtimeProgress = null;
    try {
      runtimeStatus = await getInstanceRuntimeStatus(id);
    } catch (cause: unknown) {
      runtimeError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "The managed Java status failed.");
    } finally {
      runtimeBusy = false;
    }
  }

  async function runEnsureRuntime(id: string) {
    runtimeBusy = true;
    runtimeError = null;
    runtimeProgress = null;
    try {
      runtimeStatus = await ensureInstanceRuntime(id);
    } catch (cause: unknown) {
      runtimeError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "Managed Java installation failed.");
    } finally {
      runtimeBusy = false;
    }
  }

  async function refreshAccounts() {
    try {
      accountsState = await getAccounts();
    } catch {
      // Account refresh is best-effort after mutations; load errors surface
      // through the dedicated account card.
    }
  }

  async function runSignIn(event: SubmitEvent) {
    event.preventDefault();
    signInBusy = true;
    signInProgress = null;
    signInError = null;
    try {
      await beginMicrosoftLogin();
      await refreshAccounts();
    } catch (cause: unknown) {
      signInError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "Microsoft sign-in failed.");
      await refreshAccounts();
    } finally {
      signInBusy = false;
      signInProgress = null;
    }
  }

  async function runCancelSignIn(event: SubmitEvent) {
    event.preventDefault();
    try {
      await cancelMicrosoftLogin();
    } catch {
      // Cancellation is best-effort; the flow itself reports its outcome.
    }
  }

  async function runSelectAccount(id: string) {
    accountBusy = id;
    accountError = null;
    try {
      await selectAccount(id);
      await refreshAccounts();
    } catch (cause: unknown) {
      accountError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "The account selection failed.");
    } finally {
      accountBusy = null;
    }
  }

  async function runRemoveAccount(id: string) {
    accountBusy = id;
    accountError = null;
    try {
      await removeAccount(id);
      delete accountSessions[id];
      await refreshAccounts();
    } catch (cause: unknown) {
      accountError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "Removing the account failed.");
    } finally {
      accountBusy = null;
    }
  }

  async function runRefreshAccountSession(id: string) {
    accountBusy = id;
    accountError = null;
    try {
      accountSessions[id] = await refreshAccountSession(id);
      await refreshAccounts();
    } catch (cause: unknown) {
      accountError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "Restoring the account session failed.");
      await refreshAccounts();
    } finally {
      accountBusy = null;
    }
  }

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

  async function runFabricPlanning(event: SubmitEvent) {
    event.preventDefault();
    fabricPlanningBusy = true;
    fabricPlanSummary = null;
    fabricPlanningError = null;

    try {
      fabricPlanSummary = await planFabricInstall({
        minecraftVersion: fabricMinecraftVersion.trim(),
        loaderVersion: fabricLoaderVersion.trim(),
      });
    } catch (cause: unknown) {
      fabricPlanningError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "The composed plan failed.");
    } finally {
      fabricPlanningBusy = false;
    }
  }

  async function runInstall(event: SubmitEvent) {
    event.preventDefault();
    installBusy = true;
    installProgress = null;
    installSummary = null;
    installError = null;
    validation = null;
    validationError = null;

    try {
      installSummary = await installGame({
        instanceId: installInstanceId.trim(),
        minecraftVersion: installMinecraftVersion.trim(),
        loaderVersion: installLoaderVersion.trim(),
      });
    } catch (cause: unknown) {
      installError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "The installation failed.");
    } finally {
      installBusy = false;
    }
  }

  async function runValidation(event: SubmitEvent) {
    event.preventDefault();
    validationBusy = true;
    validation = null;
    validationError = null;

    try {
      validation = await validateInstalledGame({
        instanceId: installInstanceId.trim(),
      });
    } catch (cause: unknown) {
      validationError =
        cause instanceof LauncherBackendError
          ? cause
          : new LauncherBackendError("unknown_error", "The validation failed.");
    } finally {
      validationBusy = false;
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
      <p class="footnote">No Minecraft installation data is accessed in this phase.</p>
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
      </dl>
      <p class="footnote">
        Instance management lives in the panel below. Installation is supported;
        launching is not implemented yet. Managed Java is shown for the selected instance.
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

  <section class="status-card" aria-labelledby="instances-title" aria-live="polite">
    <div class="status-heading">
      <div>
        <p class="eyebrow">Persistent instances</p>
        <h2 id="instances-title">Instances</h2>
      </div>

      {#if createBusy}
        <span class="badge loading"><span aria-hidden="true"></span
          >{createProgress ? createProgress.phase : "Working"}</span
        >
      {:else}
        <span class="badge ready"><span aria-hidden="true"></span
          >{launcherState?.instances.length ?? 0}</span
        >
      {/if}
    </div>

    <form class="acquire-form" onsubmit={runCreateInstance}>
      <label>
        <span>Display name</span>
        <input
          type="text"
          bind:value={createDisplayName}
          placeholder="e.g. My Aurora Setup"
          required
          maxlength="80"
        />
      </label>
      <label>
        <span>Channel</span>
        <select bind:value={createChannel}>
          <option value="stable">stable</option>
          <option value="beta">beta</option>
          <option value="nightly">nightly</option>
        </select>
      </label>
      <label>
        <span>Aurora release</span>
        <select bind:value={createVersion} required>
          {#each releases.filter((release) => release.channel === createChannel) as release (release.auroraVersion)}
            <option value={release.auroraVersion}>
              Aurora {release.auroraVersion} · Minecraft {release.minecraftVersion} · Fabric
              {release.fabricLoaderVersion}
            </option>
          {/each}
        </select>
      </label>
      <button
        type="submit"
        disabled={createBusy || createDisplayName.trim() === "" || createVersion === ""}
      >
        {createBusy ? "Creating…" : "Create instance"}
      </button>
    </form>

    {#if createBusy && createProgress}
      <dl>
        <div>
          <dt>Progress</dt>
          <dd>
            {createProgress.phase}
            {#if createProgress.game}
              · {createProgress.game.completedItems}/{createProgress.game.totalItems}
            {/if}
          </dd>
        </div>
      </dl>
    {/if}

    {#if createError}
      <div class="error-message" role="alert">
        <p>{createError.message}</p>
        <code>{createError.code}</code>
      </div>
    {/if}

    {#if releasesError}
      <div class="error-message" role="alert">
        <p>{releasesError.message}</p>
        <code>{releasesError.code}</code>
      </div>
    {:else if releases.length > 0 && releases[0].source === "development-fixture"}
      <p class="footnote">
        Aurora releases currently come from the launcher's checked-in development fixture —
        no production release infrastructure exists yet. Serve
        <code>src-tauri/development</code> on 127.0.0.1:8765 for artifact downloads.
      </p>
    {/if}

    {#if instanceError}
      <div class="error-message" role="alert">
        <p>{instanceError.message}</p>
        <code>{instanceError.code}</code>
      </div>
    {/if}

    {#if launcherState && launcherState.instances.length === 0 && !createBusy}
      <p class="footnote">No instances yet — create the first one above.</p>
    {/if}

    {#if launcherState}
      {#each launcherState.instances as instance (instance.id)}
      <div class="instance-row">
        <div class="instance-main">
          <div class="instance-title">
            <strong>{instance.displayName}</strong>
            {#if launcherState.config.selectedInstanceId === instance.id}
              <span class="badge ready"><span aria-hidden="true"></span>Selected</span>
            {/if}
            {#if instance.state === "installing"}
              <span class="badge loading"><span aria-hidden="true"></span>Installing</span>
            {:else if instanceValidations[instance.id]?.status === "damaged"}
              <span class="badge error"><span aria-hidden="true"></span>Damaged</span>
            {:else if instanceValidations[instance.id]?.status === "ready"}
              <span class="badge ready"><span aria-hidden="true"></span>Verified</span>
            {:else if instance.state === "ready"}
              <span class="badge ready"><span aria-hidden="true"></span>Ready</span>
            {/if}
          </div>
          <div class="instance-meta">
            Aurora {instance.auroraVersion} ({instance.channel}) · Minecraft
            {instance.minecraftVersion} · Fabric {instance.fabricLoaderVersion}
          </div>
          <div class="instance-meta instance-id">id: {instance.id}</div>

          {#if instanceValidations[instance.id]}
            <div
              class="instance-meta validation-line"
              class:damaged={instanceValidations[instance.id].status === "damaged"}
            >
              Validation: {instanceValidations[instance.id].status}
              {#each instanceValidations[instance.id].problems as problem (problem.reason)}
                <div class="instance-meta">
                  {problem.component}: {problem.reason}
                </div>
              {/each}
            </div>
          {/if}

          {#if launcherState.config.selectedInstanceId === instance.id && instance.state === "ready"}
            <div class="instance-meta validation-line" class:damaged={runtimeStatus?.status === "damaged"}>
              Content: ready · Java:
              {#if runtimeBusy}
                {runtimeProgress
                  ? `${runtimeProgress.phase} ${runtimeProgress.completedItems}/${runtimeProgress.totalItems}`
                  : "resolving"}
              {:else if runtimeStatus?.instanceId === instance.id}
                {runtimeStatus.status} · {runtimeStatus.component} · required {runtimeStatus.requiredMajorVersion}{#if runtimeStatus.runtimeVersion}
                  · installed {runtimeStatus.runtimeVersion}
                {/if}
                {#if runtimeStatus.reused === true} · reused verified runtime{/if}
                {#each runtimeStatus.problems as problem}
                  <div class="instance-meta">Java: {problem}</div>
                {/each}
              {:else}
                not checked
              {/if}
            </div>
            {#if runtimeError}
              <div class="error-message" role="alert">
                <p>{runtimeError.message}</p>
                <code>{runtimeError.code}</code>
              </div>
            {/if}
          {/if}
        </div>

        <div class="instance-actions">
          {#if renaming?.id === instance.id}
            <form class="rename-form" onsubmit={runRename}>
              <input
                type="text"
                bind:value={renaming.name}
                required
                maxlength="80"
                placeholder="New display name"
              />
              <button type="submit" disabled={instanceBusy === instance.id}>Save</button>
              <button
                type="button"
                onclick={() => (renaming = null)}
                disabled={instanceBusy === instance.id}
              >
                Cancel
              </button>
            </form>
          {:else}
            {#if launcherState.config.selectedInstanceId !== instance.id}
              <button
                type="button"
                onclick={() => runSelect(instance.id)}
                disabled={instanceBusy === instance.id || createBusy}
              >
                Select
              </button>
            {/if}
            <button
              type="button"
              onclick={() => (renaming = { id: instance.id, name: instance.displayName })}
              disabled={instanceBusy === instance.id || createBusy}
            >
              Rename
            </button>
            {#if instance.state === "installing"}
              <button
                type="button"
                onclick={() => runRetry(instance.id)}
                disabled={instanceBusy === instance.id || createBusy}
              >
                Retry install
              </button>
            {/if}
            <button
              type="button"
              onclick={() => runValidate(instance.id)}
              disabled={instanceBusy === instance.id || createBusy}
            >
              Validate
            </button>
            {#if launcherState.config.selectedInstanceId === instance.id && instance.state === "ready"}
              <button
                type="button"
                onclick={() => runRuntimeStatus(instance.id)}
                disabled={runtimeBusy || instanceBusy === instance.id || createBusy}
              >
                Check Java
              </button>
              {#if runtimeStatus?.instanceId === instance.id && runtimeStatus.status !== "ready"}
                <button
                  type="button"
                  onclick={() => runEnsureRuntime(instance.id)}
                  disabled={runtimeBusy || instanceBusy === instance.id || createBusy}
                >
                  {runtimeStatus.status === "damaged" ? "Repair Java" : "Install Java"}
                </button>
              {/if}
            {/if}
          {/if}
        </div>
      </div>
      {/each}
    {/if}

    <p class="footnote">
      Instances are complete, isolated installations — game, Fabric, and the Aurora client
      artifact — validated before they are reported content-ready. The selected instance can
      acquire and validate its official shared Mojang Java runtime independently. Game
      launching and instance deletion are deliberately not implemented.
    </p>
  </section>

  <section class="status-card" aria-labelledby="accounts-title" aria-live="polite">
    <div class="status-heading">
      <div>
        <p class="eyebrow">Microsoft &amp; Minecraft authentication</p>
        <h2 id="accounts-title">Account</h2>
      </div>

      {#if signInBusy}
        <span class="badge loading"><span aria-hidden="true"></span
          >{signInProgress ? signInProgress.phase : "Waiting for Microsoft"}</span
        >
      {:else if accountsState && accountsState.accounts.length > 0}
        <span class="badge ready"><span aria-hidden="true"></span>Signed in</span>
      {:else if accountsError}
        <span class="badge error"><span aria-hidden="true"></span>Unavailable</span>
      {:else}
        <span class="badge loading"><span aria-hidden="true"></span>Not signed in</span>
      {/if}
    </div>

    {#if accountsError}
      <div class="error-message" role="alert">
        <p>{accountsError.message}</p>
        <code>{accountsError.code}</code>
      </div>
    {/if}

    {#if signInBusy}
      <dl>
        <div>
          <dt>Sign-in</dt>
          <dd>
            Complete the Microsoft sign-in in your browser, then return here.
            {#if signInProgress}Current step: {signInProgress.phase}{/if}
          </dd>
        </div>
      </dl>
      <form class="acquire-form" onsubmit={runCancelSignIn}>
        <button type="submit">Cancel sign-in</button>
      </form>
    {:else}
      <form class="acquire-form" onsubmit={runSignIn}>
        <button type="submit" disabled={accountsState === null}>
          {accountsState && accountsState.accounts.length > 0
            ? "Add another account"
            : "Sign in with Microsoft"}
        </button>
      </form>
    {/if}

    {#if signInError}
      <div class="error-message" role="alert">
        <p>{signInError.message}</p>
        <code>{signInError.code}</code>
      </div>
    {/if}

    {#if accountError}
      <div class="error-message" role="alert">
        <p>{accountError.message}</p>
        <code>{accountError.code}</code>
      </div>
    {/if}

    {#if accountsState && accountsState.accounts.length === 0 && !signInBusy}
      <p class="footnote">Not signed in.</p>
    {/if}

    {#if accountsState}
      {#each accountsState.accounts as account (account.accountId)}
      <div class="instance-row">
        <div class="instance-main">
          <div class="instance-title">
            <strong>{account.minecraftName}</strong>
            {#if accountsState.selectedAccountId === account.accountId}
              <span class="badge ready"><span aria-hidden="true"></span>Selected</span>
            {/if}
            {#if account.status === "reauthenticationRequired"}
              <span class="badge error"><span aria-hidden="true"></span>Sign-in required</span>
            {:else}
              <span class="badge ready"><span aria-hidden="true"></span>Signed in</span>
            {/if}
          </div>
          <div class="instance-meta instance-id">id: {account.accountId}</div>
          {#if accountSessions[account.accountId]}
            <div class="instance-meta validation-line">
              Session: ready
            </div>
          {/if}
        </div>

        <div class="instance-actions">
          {#if accountsState.selectedAccountId !== account.accountId}
            <button
              type="button"
              onclick={() => runSelectAccount(account.accountId)}
              disabled={accountBusy === account.accountId || signInBusy}
            >
              Select
            </button>
          {/if}
          <button
            type="button"
            onclick={() => runRefreshAccountSession(account.accountId)}
            disabled={accountBusy === account.accountId || signInBusy}
          >
            Check session
          </button>
          <button
            type="button"
            onclick={() => runRemoveAccount(account.accountId)}
            disabled={accountBusy === account.accountId || signInBusy}
          >
            Remove account
          </button>
        </div>
      </div>
      {/each}
    {/if}

    <p class="footnote">
      Sign-in uses your system browser, and only the Microsoft refresh credential is stored —
      in the operating system's credential store, never in plain files. Accounts are separate
      from instances; Minecraft launching is not implemented yet.
    </p>
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

    <section class="status-card" aria-labelledby="fabric-plan-title" aria-live="polite">
      <div class="status-heading">
        <div>
          <p class="eyebrow">Native composition proof</p>
          <h2 id="fabric-plan-title">Fabric install planning</h2>
        </div>

        {#if fabricPlanningBusy}
          <span class="badge loading"><span aria-hidden="true"></span>Composing</span>
        {:else if fabricPlanSummary}
          <span class="badge ready"><span aria-hidden="true"></span>Planned</span>
        {:else if fabricPlanningError}
          <span class="badge error"><span aria-hidden="true"></span>Rejected</span>
        {/if}
      </div>

      <form class="acquire-form" onsubmit={runFabricPlanning}>
        <label>
          <span>Exact Minecraft version</span>
          <input
            type="text"
            bind:value={fabricMinecraftVersion}
            placeholder="e.g. 26.2 or 1.21.11"
            required
            spellcheck="false"
          />
        </label>
        <label>
          <span>Exact Fabric Loader version</span>
          <input
            type="text"
            bind:value={fabricLoaderVersion}
            placeholder="e.g. 0.19.5"
            required
            spellcheck="false"
          />
        </label>
        <button type="submit" disabled={fabricPlanningBusy}>
          {fabricPlanningBusy ? "Composing…" : "Compose game plan"}
        </button>
      </form>

      {#if fabricPlanSummary}
        <dl>
          <div>
            <dt>Minecraft</dt>
            <dd>{fabricPlanSummary.minecraftVersion}</dd>
          </div>
          <div>
            <dt>Fabric Loader</dt>
            <dd>{fabricPlanSummary.loaderVersion}</dd>
          </div>
          <div>
            <dt>Vanilla libraries</dt>
            <dd>{fabricPlanSummary.vanillaLibraryCount}</dd>
          </div>
          <div>
            <dt>Fabric libraries</dt>
            <dd>
              {fabricPlanSummary.fabricLibraryCount}
              ({fabricPlanSummary.fabricDigestedLibraryCount} with official digests)
            </dd>
          </div>
          <div>
            <dt>Final libraries</dt>
            <dd>{fabricPlanSummary.finalLibraryCount}</dd>
          </div>
          <div>
            <dt>Java</dt>
            <dd>
              {fabricPlanSummary.javaComponent} (major {fabricPlanSummary.javaMajorVersion}){#if fabricPlanSummary.javaRaisedByLoader}
                — raised by the loader{/if}
            </dd>
          </div>
          <div>
            <dt>Final main class</dt>
            <dd>{fabricPlanSummary.finalMainClass}</dd>
          </div>
        </dl>
      {:else if fabricPlanningError}
        <div class="error-message" role="alert">
          <p>{fabricPlanningError.message}</p>
          <code>{fabricPlanningError.code}</code>
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
          <h2 id="install-title">Game installation</h2>
        </div>

        {#if installBusy}
          <span class="badge loading"><span aria-hidden="true"></span
            >{installProgress ? installProgress.phase : "Installing"}</span
          >
        {:else if installSummary}
          <span class="badge ready"><span aria-hidden="true"></span>Installed</span>
        {:else if installError}
          <span class="badge error"><span aria-hidden="true"></span>Failed</span>
        {/if}
      </div>

      <form class="acquire-form" onsubmit={runInstall}>
        <label>
          <span>Instance id (managed storage)</span>
          <input
            type="text"
            bind:value={installInstanceId}
            placeholder="e.g. dev-install"
            required
            spellcheck="false"
          />
        </label>
        <label>
          <span>Exact Minecraft version</span>
          <input
            type="text"
            bind:value={installMinecraftVersion}
            placeholder="e.g. 26.2"
            required
            spellcheck="false"
          />
        </label>
        <label>
          <span>Exact Fabric Loader version</span>
          <input
            type="text"
            bind:value={installLoaderVersion}
            placeholder="e.g. 0.19.5"
            required
            spellcheck="false"
          />
        </label>
        <button type="submit" disabled={installBusy}>
          {installBusy ? "Installing…" : "Install isolated game"}
        </button>
      </form>

      {#if installBusy && installProgress}
        <dl>
          <div>
            <dt>Phase</dt>
            <dd>{installProgress.phase}</dd>
          </div>
          <div>
            <dt>Progress</dt>
            <dd>
              {installProgress.completedItems} / {installProgress.totalItems}
              {#if installProgress.currentItem}· {installProgress.currentItem}{/if}
            </dd>
          </div>
        </dl>
      {:else if installSummary}
        <dl>
          <div>
            <dt>Installed</dt>
            <dd>
              Minecraft {installSummary.minecraftVersion} + Fabric Loader
              {installSummary.loaderVersion}
            </dd>
          </div>
          <div>
            <dt>Files</dt>
            <dd>
              {installSummary.fileCount} managed files
              ({installSummary.totalBytes.toLocaleString()} bytes)
            </dd>
          </div>
          <div>
            <dt>Trust classes</dt>
            <dd>
              {installSummary.verifiedSha1Files} SHA-1 verified ·
              {installSummary.verifiedSha256Files} SHA-256 verified ·
              {installSummary.transportObservedFiles} secure-transport observed
            </dd>
          </div>
          <div class="path-row">
            <dt>Game directory</dt>
            <dd>{installSummary.gameDirectory}</dd>
          </div>
        </dl>
      {:else if installError}
        <div class="error-message" role="alert">
          <p>{installError.message}</p>
          <code>{installError.code}</code>
        </div>
      {/if}

      <form class="acquire-form" onsubmit={runValidation}>
        <button type="submit" disabled={validationBusy || installInstanceId.trim() === ""}>
          {validationBusy ? "Validating…" : "Validate installed game"}
        </button>
      </form>

      {#if validation}
        <dl>
          <div>
            <dt>Status</dt>
            <dd>{validation.status}</dd>
          </div>
          {#if validation.installationId}
            <div>
              <dt>Installation</dt>
              <dd>
                {validation.minecraftVersion} + {validation.loaderVersion} ·
                {validation.installationId}
              </dd>
            </div>
          {/if}
          {#if validation.checkedFiles > 0}
            <div>
              <dt>Verified</dt>
              <dd>
                {validation.checkedFiles} files ({validation.verifiedBytes.toLocaleString()} bytes)
              </dd>
            </div>
          {/if}
          {#each validation.problems as problem (problem.path)}
            <div>
              <dt>{problem.path}</dt>
              <dd>{problem.reason}</dd>
            </div>
          {/each}
        </dl>
      {:else if validationError}
        <div class="error-message" role="alert">
          <p>{validationError.message}</p>
          <code>{validationError.code}</code>
        </div>
      {/if}

      <p class="footnote">
        Development-only proof of the native installation executor: verified acquisition,
        staged materialization, native extraction, validation, and atomic commit into
        launcher-managed instance storage. Nothing is launched; no Java runtime is
        installed; the user's .minecraft is never touched.
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

  .acquire-form select {
    width: 100%;
    padding: 0.6rem 0.75rem;
    border: 1px solid #2a3149;
    border-radius: 8px;
    background: #10141f;
    color: #e7ecf7;
    font: inherit;
    font-size: 0.9rem;
  }

  .instance-row {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    gap: 1rem;
    padding: 1rem 1.5rem;
    border-bottom: 1px solid #20263a;
  }

  .instance-main {
    min-width: 0;
    flex: 1 1 18rem;
  }

  .instance-title {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    flex-wrap: wrap;
  }

  .instance-meta {
    margin-top: 0.35rem;
    color: #818ca4;
    font-size: 0.84rem;
    overflow-wrap: anywhere;
  }

  .instance-meta.instance-id {
    font-size: 0.76rem;
  }

  .validation-line.damaged {
    color: #ff9a9a;
  }

  .instance-actions {
    display: flex;
    align-items: flex-start;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  .instance-actions button {
    padding: 0.4rem 0.85rem;
    border: 1px solid #2a3149;
    border-radius: 8px;
    background: #1a2032;
    color: #e7ecf7;
    font: inherit;
    font-size: 0.82rem;
    font-weight: 600;
    cursor: pointer;
  }

  .instance-actions button:hover:not(:disabled) {
    border-color: #6f5df2;
  }

  .instance-actions button:disabled {
    opacity: 0.55;
    cursor: progress;
  }

  .rename-form {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }

  .rename-form input {
    padding: 0.4rem 0.6rem;
    border: 1px solid #2a3149;
    border-radius: 8px;
    background: #10141f;
    color: #e7ecf7;
    font: inherit;
    font-size: 0.85rem;
  }

  .rename-form button {
    padding: 0.4rem 0.85rem;
    border: 1px solid #2a3149;
    border-radius: 8px;
    background: #1a2032;
    color: #e7ecf7;
    font: inherit;
    font-size: 0.82rem;
    font-weight: 600;
    cursor: pointer;
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

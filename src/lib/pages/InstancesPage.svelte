<script lang="ts">
  import { launcher } from "$lib/launcher/store.svelte";
  import { navigation } from "$lib/launcher/navigation.svelte";
  import {
    configurationLabel,
    instanceContentStatus,
  } from "$lib/launcher/instanceStatus";
  import type { InstanceSummary } from "$lib/backend";

  const instances = $derived(launcher.launcherState?.instances ?? []);
  const selectedId = $derived(launcher.launcherState?.config.selectedInstanceId ?? null);

  // The create form defaults its Minecraft version to the newest release
  // the Aurora release source supports, so a fresh instance is compatible
  // by construction.
  $effect(() => {
    if (launcher.createMinecraftVersion === "" && launcher.releases.length > 0) {
      launcher.createMinecraftVersion = launcher.releases[0].minecraftVersion;
    }
  });

  function badge(instance: InstanceSummary): { tone: string; label: string } | null {
    const decision = instanceContentStatus(
      instance,
      launcher.instanceValidations[instance.id],
      launcher.createBusy ? (launcher.createProgress?.phase ?? null) : null,
    );
    // Installing rows keep the badge; otherwise a not-installed summary is
    // only meaningful with the validation detail beside it.
    if (instance.state !== "installing" && decision.tone === "status-muted") {
      return null;
    }
    return { tone: decision.tone, label: decision.label };
  }

  function statusDetail(instance: InstanceSummary): string | null {
    const validation = launcher.instanceValidations[instance.id];
    if (instance.state === "installing") {
      return launcher.createBusy && launcher.createProgress
        ? launcher.createProgress.phase
        : "This instance did not finish installing — retry below.";
    }
    if (validation?.status === "damaged") {
      return validation.problems[0]
        ? `${validation.problems[0].component}: ${validation.problems[0].reason}`
        : "Deep validation found problems.";
    }
    if (validation?.status === "stale") {
      return "The saved configuration differs from the installed content — open the instance and install the new configuration.";
    }
    if (validation?.status === "ready") {
      return "Deep validation passed.";
    }
    return null;
  }
</script>

<!--
  Instance selection and creation. Compact rows: identity, concise state,
  and the actions that belong to the list (open, select, validate, retry).
  The full configuration editor and Java lifecycle live in each instance's
  workspace, so this page never duplicates the Settings editor.
-->
<div class="page">
  <header class="page-header">
    <div>
      <h2 class="page-title">Instances</h2>
      <p class="page-subtitle">Isolated Minecraft installations Aurora launches from.</p>
    </div>
  </header>

  <section class="group" aria-labelledby="create-title">
    <div class="group-heading">
      <div>
        <h3 class="group-title" id="create-title">Create instance</h3>
        <p class="group-subtitle">
          Choose a name, a Minecraft version, and a loader — everything else can be
          configured later.
        </p>
      </div>
      {#if launcher.createBusy}
        <span class="status-badge status-working">
          {launcher.createProgress ? launcher.createProgress.phase : "Working"}
        </span>
      {/if}
    </div>

    <form
      class="group-form"
      onsubmit={(event) => {
        event.preventDefault();
        void launcher.runCreateInstance();
      }}
    >
      <div class="field-grid">
        <label class="field">
          <span class="field-label">Name</span>
          <input
            type="text"
            bind:value={launcher.createDisplayName}
            placeholder="e.g. My Aurora Setup"
            required
            maxlength="80"
          />
        </label>
        <label class="field">
          <span class="field-label">Minecraft version</span>
          <select
            bind:value={launcher.createMinecraftVersion}
            onfocus={() => launcher.loadMinecraftVersions(launcher.createIncludeSnapshots)}
            onclick={() => launcher.loadMinecraftVersions(launcher.createIncludeSnapshots)}
            required
          >
            {#if launcher.minecraftVersions === null}
              <option value={launcher.createMinecraftVersion}>
                {launcher.createMinecraftVersion || "Loading versions…"}
              </option>
            {:else}
              {#each launcher.minecraftVersions as version (version.id)}
                <option value={version.id}>
                  {version.id}{version.versionType === "snapshot" ? " (snapshot)" : ""}
                </option>
              {/each}
            {/if}
          </select>
        </label>
        <label class="field">
          <span class="field-label">Loader</span>
          <select
            value={launcher.createLoaderPolicy.type === "automatic" ? "" : "pinned"}
            onchange={(event) => {
              const value = (event.currentTarget as HTMLSelectElement).value;
              launcher.createLoaderPolicy =
                value === ""
                  ? { type: "automatic" }
                  : {
                      type: "pinned",
                      version:
                        (launcher.loaderVersions?.find((loader) => loader.stable)?.version ??
                        ""),
                    };
              if (value === "pinned" && launcher.createMinecraftVersion) {
                void launcher.loadLoaderVersions(launcher.createMinecraftVersion);
              }
            }}
          >
            <option value="">Fabric — latest compatible</option>
            <option value="pinned">Fabric — choose version</option>
          </select>
          {#if launcher.createLoaderPolicy.type === "pinned"}
            <select
              class="field-nested"
              bind:value={launcher.createLoaderPolicy.version}
              required
            >
              {#if launcher.loaderVersions === null}
                <option value="">Loading loader versions…</option>
              {:else}
                {#each launcher.loaderVersions as loader (loader.version)}
                  <option value={loader.version}>
                    {loader.version}{loader.stable ? "" : " (unstable)"}
                  </option>
                {/each}
              {/if}
            </select>
          {/if}
        </label>
      </div>
      <label class="check-field">
        <input
          type="checkbox"
          bind:checked={launcher.createIncludeSnapshots}
          onchange={() => launcher.loadMinecraftVersions(launcher.createIncludeSnapshots)}
        />
        <span>Show snapshot versions</span>
      </label>
      <div class="form-actions">
        <button
          type="submit"
          class="btn btn-primary"
          disabled={launcher.createBusy ||
            launcher.createDisplayName.trim() === "" ||
            launcher.createMinecraftVersion === ""}
        >
          {launcher.createBusy ? "Creating…" : "Create instance"}
        </button>
        {#if launcher.createBusy && launcher.createProgress}
          <span class="group-row-detail">
            {launcher.createProgress.phase}
            {#if launcher.createProgress.game}
              · {launcher.createProgress.game.completedItems}/{launcher.createProgress.game.totalItems}
            {/if}
          </span>
        {/if}
      </div>
    </form>

    {#if launcher.createError}
      <p class="inline-message inline-message-error group-row" role="alert">
        {launcher.createError.message}
      </p>
    {/if}
    {#if launcher.minecraftVersionsError}
      <p class="inline-message inline-message-error group-row" role="alert">
        {launcher.minecraftVersionsError.message}
      </p>
    {/if}
    {#if launcher.loaderVersionsError}
      <p class="inline-message inline-message-error group-row" role="alert">
        {launcher.loaderVersionsError.message}
      </p>
    {/if}

    {#if launcher.releases.length > 0 && launcher.releases[0].source === "development-fixture"}
      <p class="group-footer">
        Development release source: Aurora releases currently come from the launcher's checked-in
        development fixture — no production release infrastructure exists yet. Serve
        <code>src-tauri/development</code> on 127.0.0.1:8765 for artifact downloads. Instances can
        only be created for Minecraft versions this source supports.
      </p>
    {/if}
  </section>

  <section class="group" aria-labelledby="list-title" aria-live="polite">
    <div class="group-heading">
      <div>
        <h3 class="group-title" id="list-title">Your instances</h3>
        <p class="group-subtitle">
          {instances.length === 0
            ? "Nothing here yet."
            : `${instances.length} ${instances.length === 1 ? "instance" : "instances"} — open one to manage it; the selected one launches from Home.`}
        </p>
      </div>
    </div>

    {#if launcher.stateError}
      <p class="inline-message inline-message-error group-row" role="alert">
        {launcher.stateError.message}
      </p>
    {:else if launcher.launcherState === null}
      <div class="group-row group-row-loading">
        <span class="spinner" aria-hidden="true"></span>
        <span class="group-row-detail">Loading instances…</span>
      </div>
    {:else if instances.length === 0}
      <p class="group-footer">No instances yet — create the first one above.</p>
    {:else}
      {#each instances as instance (instance.id)}
        {@const selected = selectedId === instance.id}
        {@const busy = launcher.instanceBusy === instance.id}
        {@const detail = statusDetail(instance)}
        {@const rowBadge = badge(instance)}
        {@const problemTone =
          launcher.instanceValidations[instance.id]?.status === "damaged"
            ? "problem-line-error"
            : "problem-line-warning"}
        <div class="group-row" class:group-row-selected={selected}>
          <div class="group-row-main">
            <span class="instance-name-line">
              <span class="group-row-title">{instance.displayName}</span>
              {#if selected}<span class="row-marker">Selected</span>{/if}
            </span>
            <span class="group-row-detail">{configurationLabel(instance)}</span>
            {#if detail}<span class="group-row-detail {problemTone}">{detail}</span>{/if}
          </div>

          <div class="group-row-actions">
            {#if rowBadge}<span class="status-badge {rowBadge.tone}">{rowBadge.label}</span>{/if}
            <button
              type="button"
              class="btn"
              onclick={() => navigation.openInstance(instance.id)}
              disabled={busy || launcher.createBusy}
            >
              Open
            </button>
            {#if !selected}
              <button
                type="button"
                class="btn"
                onclick={() => launcher.runSelect(instance.id)}
                disabled={busy || launcher.createBusy}
              >
                Select
              </button>
            {/if}
            {#if instance.state === "installing"}
              <button
                type="button"
                class="btn"
                onclick={() => launcher.runRetry(instance.id)}
                disabled={busy || launcher.createBusy}
              >
                Retry install
              </button>
            {/if}
            <button
              type="button"
              class="btn"
              onclick={() => launcher.runValidate(instance.id)}
              disabled={busy || launcher.createBusy}
            >
              {busy ? "Validating…" : "Validate"}
            </button>
          </div>
        </div>
      {/each}

      {#if launcher.instanceError}
        <p class="inline-message inline-message-error group-row" role="alert">
          {launcher.instanceError.message}
        </p>
      {/if}
    {/if}

    <p class="group-footer">
      Instances are complete, isolated installations — game, Fabric, and the Aurora client
      artifact — validated before they are reported ready. Open an instance to manage its
      configuration and Java runtime. Instance deletion remains deliberately unimplemented.
    </p>
  </section>
</div>

<style>
  .instance-name-line {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    min-width: 0;
    flex-wrap: wrap;
  }

  .field-nested {
    margin-top: var(--space-2);
  }

  .check-field {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    color: var(--color-text-secondary);
    font-size: var(--text-body);
    cursor: pointer;
  }

  .problem-line-warning {
    color: var(--color-warning);
  }

  .problem-line-error {
    color: var(--color-error);
  }

  .group-footer code {
    font-family: var(--font-mono);
    font-size: var(--text-metadata);
  }
</style>

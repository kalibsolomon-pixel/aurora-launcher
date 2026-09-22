<script lang="ts">
  import { launcher } from "$lib/launcher/store.svelte";

  const instances = $derived(launcher.launcherState?.instances ?? []);
  const selectedId = $derived(launcher.launcherState?.config.selectedInstanceId ?? null);
  const channelReleases = $derived(
    launcher.releases.filter((release) => release.channel === launcher.createChannel),
  );

  // Keep the release choice valid for the chosen channel instead of letting
  // it silently point at a release hidden from the list.
  let lastChannel = $state(launcher.createChannel);
  $effect(() => {
    if (launcher.createChannel === lastChannel) return;
    lastChannel = launcher.createChannel;
    const preferred = launcher.releases.find((release) => release.channel === launcher.createChannel);
    if (preferred) launcher.createVersion = preferred.auroraVersion;
  });

  function statusBadge(
    instance: (typeof instances)[number],
  ): { tone: string; label: string } | null {
    if (instance.state === "installing") {
      return { tone: "status-working", label: "Installing" };
    }
    const validation = launcher.instanceValidations[instance.id];
    if (validation?.status === "damaged") return { tone: "status-error", label: "Damaged" };
    if (validation?.status === "ready") return { tone: "status-success", label: "Verified" };
    if (instance.state === "ready") return { tone: "status-success", label: "Ready" };
    return null;
  }

  function javaBadge(): { tone: string; label: string } {
    if (launcher.runtimeBusy) {
      return {
        tone: "status-working",
        label: launcher.runtimeProgress
          ? `${launcher.runtimeProgress.completedItems}/${launcher.runtimeProgress.totalItems}`
          : "Checking…",
      };
    }
    if (launcher.runtimeError) return { tone: "status-error", label: "Status failed" };
    const status = launcher.runtimeStatus;
    if (!status) return { tone: "status-muted", label: "Not checked" };
    if (status.status === "ready") return { tone: "status-success", label: "Ready" };
    if (status.status === "damaged") return { tone: "status-error", label: "Damaged" };
    return { tone: "status-warning", label: "Not installed" };
  }

  function javaDetail(): string | null {
    const status = launcher.runtimeStatus;
    if (launcher.runtimeBusy) {
      return launcher.runtimeProgress ? launcher.runtimeProgress.phase : null;
    }
    if (launcher.runtimeError) return launcher.runtimeError.message;
    if (!status) return null;
    const base = `${status.component} · Java ${status.requiredMajorVersion}`;
    if (status.status === "ready") {
      return (
        base +
        (status.runtimeVersion ? ` · ${status.runtimeVersion}` : "") +
        (status.reused === true ? " · reused verified runtime" : "")
      );
    }
    if (status.status === "damaged") {
      return status.problems[0] ?? "The managed runtime failed validation.";
    }
    return `${base} — install it to make this instance playable.`;
  }
</script>

<!--
  Instance lifecycle surface: creation from the currently supported release
  source, then selection, rename, retry, and validation per instance. Java
  provisioning stays here because it is instance lifecycle; launching itself
  lives on Home. Full instance configuration is the next phase, not this one.
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
          A complete, isolated installation pinned to one exact Aurora release.
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
          <span class="field-label">Display name</span>
          <input
            type="text"
            bind:value={launcher.createDisplayName}
            placeholder="e.g. My Aurora Setup"
            required
            maxlength="80"
          />
        </label>
        <label class="field">
          <span class="field-label">Channel</span>
          <select bind:value={launcher.createChannel}>
            <option value="stable">stable</option>
            <option value="beta">beta</option>
            <option value="nightly">nightly</option>
          </select>
        </label>
        <label class="field field-span">
          <span class="field-label">Aurora release</span>
          <select bind:value={launcher.createVersion} required>
            {#each channelReleases as release (release.auroraVersion)}
              <option value={release.auroraVersion}>
                Aurora {release.auroraVersion} · Minecraft {release.minecraftVersion} · Fabric
                {release.fabricLoaderVersion}
              </option>
            {/each}
          </select>
        </label>
      </div>
      <div class="form-actions">
        <button
          type="submit"
          class="btn btn-primary"
          disabled={launcher.createBusy ||
            launcher.createDisplayName.trim() === "" ||
            launcher.createVersion === ""}
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
    {#if launcher.releasesError}
      <p class="inline-message inline-message-error group-row" role="alert">
        {launcher.releasesError.message}
      </p>
    {/if}

    {#if launcher.releases.length > 0 && launcher.releases[0].source === "development-fixture"}
      <p class="group-footer">
        Development release source: Aurora releases currently come from the launcher's checked-in
        development fixture — no production release infrastructure exists yet. Serve
        <code>src-tauri/development</code> on 127.0.0.1:8765 for artifact downloads.
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
            : `${instances.length} ${instances.length === 1 ? "instance" : "instances"} — the selected one launches from Home.`}
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
        {@const validation = launcher.instanceValidations[instance.id]}
        {@const badge = statusBadge(instance)}
        <div class="group-row" class:group-row-selected={selected}>
          <div class="group-row-main">
            <span class="instance-name-line">
              <span class="group-row-title">{instance.displayName}</span>
              {#if selected}<span class="row-marker">Selected</span>{/if}
            </span>
            <span class="group-row-detail">
              Aurora {instance.auroraVersion} ({instance.channel}) · Minecraft
              {instance.minecraftVersion} · Fabric {instance.fabricLoaderVersion}
            </span>
            {#if instance.state === "installing"}
              <span class="group-row-detail">
                {launcher.createBusy && launcher.createProgress
                  ? launcher.createProgress.phase
                  : "This instance did not finish installing — retry below."}
              </span>
            {:else if validation?.status === "damaged"}
              {#each validation.problems as problem (problem.reason)}
                <span class="group-row-detail problem-line">{problem.component}: {problem.reason}</span>
              {/each}
            {:else if validation?.status === "ready"}
              <span class="group-row-detail">Deep validation passed.</span>
            {/if}
          </div>

          <div class="group-row-actions">
            {#if badge}<span class="status-badge {badge.tone}">{badge.label}</span>{/if}
            {#if launcher.renaming?.id === instance.id}
              <form
                class="rename-form"
                onsubmit={(event) => {
                  event.preventDefault();
                  void launcher.runRename();
                }}
              >
                <label class="field">
                  <span class="field-label">New display name</span>
                  <input
                    type="text"
                    bind:value={launcher.renaming.name}
                    required
                    maxlength="80"
                  />
                </label>
                <button type="submit" class="btn" disabled={busy}>Save</button>
                <button
                  type="button"
                  class="btn btn-quiet"
                  onclick={() => (launcher.renaming = null)}
                  disabled={busy}
                >
                  Cancel
                </button>
              </form>
            {:else}
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
              <button
                type="button"
                class="btn"
                onclick={() => (launcher.renaming = { id: instance.id, name: instance.displayName })}
                disabled={busy || launcher.createBusy}
              >
                Rename
              </button>
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
            {/if}
          </div>
        </div>

        {#if selected && instance.state === "ready"}
          {@const java = javaBadge()}
          <div class="group-row group-row-sub">
            <div class="group-row-main">
              <span class="group-row-title">Java runtime</span>
              {#if javaDetail()}<span class="group-row-detail">{javaDetail()}</span>{/if}
            </div>
            <div class="group-row-actions">
              <span class="status-badge {java.tone}">{java.label}</span>
              <button
                type="button"
                class="btn"
                onclick={() => launcher.runRuntimeStatus(instance.id)}
                disabled={launcher.runtimeBusy || launcher.createBusy}
              >
                {launcher.runtimeBusy ? "Checking…" : "Check Java"}
              </button>
              {#if launcher.runtimeStatus?.instanceId === instance.id && launcher.runtimeStatus.status !== "ready" && !launcher.runtimeBusy}
                <button
                  type="button"
                  class="btn"
                  onclick={() => launcher.runEnsureRuntime(instance.id)}
                  disabled={launcher.createBusy}
                >
                  {launcher.runtimeStatus.status === "damaged" ? "Repair Java" : "Install Java"}
                </button>
              {/if}
            </div>
          </div>
        {/if}
      {/each}

      {#if launcher.runtimeError}
        <p class="inline-message inline-message-error group-row" role="alert">
          {launcher.runtimeError.message}
        </p>
      {/if}
      {#if launcher.instanceError}
        <p class="inline-message inline-message-error group-row" role="alert">
          {launcher.instanceError.message}
        </p>
      {/if}
    {/if}

    <p class="group-footer">
      Instances are complete, isolated installations — game, Fabric, and the Aurora client
      artifact — validated before they are reported ready. Instance deletion remains deliberately
      unimplemented.
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

  /* The Java row belongs to the selected instance above it; a quiet inset
     keeps the association visible without nesting surfaces. */
  .group-row-sub {
    padding-left: var(--space-6);
    background: var(--color-surface-sunken);
  }

  .problem-line {
    color: var(--color-error);
  }

  .rename-form {
    display: flex;
    align-items: end;
    gap: var(--space-2);
    flex-wrap: wrap;
  }

  .rename-form .field {
    flex: 1 1 12rem;
  }

  .group-footer code {
    font-family: var(--font-mono);
    font-size: var(--text-metadata);
  }
</style>

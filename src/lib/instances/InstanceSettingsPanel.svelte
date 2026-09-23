<script lang="ts">
  import { launcher } from "$lib/launcher/store.svelte";
  import { configurationRequiresInstall, draftIsDirty } from "$lib/launcher/instanceStatus";
  import type { InstanceConfiguration, InstanceSummary } from "$lib/backend";

  let { instance }: { instance: InstanceSummary } = $props();

  // The draft lives in the store keyed by instance id: seeding is idempotent
  // and unsaved edits survive tab switches, other instances, and leaving the
  // workspace. Dirty state is derived, never tracked.
  $effect(() => {
    launcher.openDetail(instance.id);
  });

  const draft = $derived(launcher.draftFor(instance.id));
  const dirty = $derived(
    draft ? draftIsDirty(draft, instance.configuration) : false,
  );
  const needsInstall = $derived(configurationRequiresInstall(instance));

  function onDraftChange(): void {
    if (draft && draft.minecraftVersion.trim() !== "") {
      void launcher.loadLoaderVersions(draft.minecraftVersion);
    }
  }

  function loaderVersionValue(configuration: InstanceConfiguration): string {
    return configuration.loader.policy.type === "pinned"
      ? (configuration.loader.policy.version ?? "")
      : "";
  }

  function setLoaderPolicy(event: Event): void {
    if (!draft) return;
    const value = (event.currentTarget as HTMLSelectElement).value;
    draft.loader.policy =
      value === "" ? { type: "automatic" } : { type: "pinned", version: value };
    onDraftChange();
  }

  function onLoaderVersionChange(event: Event): void {
    if (!draft) return;
    const value = (event.currentTarget as HTMLSelectElement).value;
    draft.loader.policy = { type: "pinned", version: value };
    onDraftChange();
  }

  function javaContext(): string | null {
    const status = launcher.runtimeStatus;
    if (launcher.runtimeBusy && status?.instanceId === instance.id) {
      return launcher.runtimeProgress
        ? `${launcher.runtimeProgress.phase} ${launcher.runtimeProgress.completedItems}/${launcher.runtimeProgress.totalItems}`
        : null;
    }
    if (!status || status.instanceId !== instance.id) return null;
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
    return `${base} — manage it from the Overview tab.`;
  }
</script>

<!--
  The workspace Settings tab: the instance's desired-configuration editor.
  Edits work on an explicit draft of the whole configuration; Save persists
  it atomically, and install-affecting changes are applied through a
  deliberate install step — never by a dropdown changing content. These
  settings belong to this instance only; launcher-wide preferences live in
  the sidebar's Settings destination.
-->
<section class="group" aria-labelledby="instance-settings-title">
  <div class="group-heading">
    <div>
      <h3 class="group-title" id="instance-settings-title">Settings</h3>
      <p class="group-subtitle">
        The saved configuration this instance launches with.
      </p>
    </div>
    <div class="group-row-actions">
      {#if dirty}<span class="status-badge status-warning">Unsaved changes</span>{/if}
      <button
        type="button"
        class="btn btn-quiet"
        onclick={() => {
          launcher.renaming = { id: instance.id, name: instance.displayName };
        }}
        disabled={launcher.detailBusy !== null}
      >
        Rename
      </button>
    </div>
  </div>

  {#if !draft}
    <div class="group-row group-row-loading">
      <span class="spinner" aria-hidden="true"></span>
      <span class="group-row-detail">Loading configuration…</span>
    </div>
  {:else}
    <form
      class="group-form"
      onsubmit={(event) => {
        event.preventDefault();
        void launcher.runSaveConfiguration(instance.id);
      }}
    >
      {#if launcher.renaming?.id === instance.id}
        <div class="field-grid">
          <label class="field">
            <span class="field-label">New name</span>
            <input
              type="text"
              bind:value={launcher.renaming.name}
              required
              maxlength="80"
            />
          </label>
          <div class="form-actions">
            <button
              type="button"
              class="btn"
              onclick={() => void launcher.runRename()}
              disabled={launcher.detailBusy !== null}
            >
              Save name
            </button>
            <button
              type="button"
              class="btn btn-quiet"
              onclick={() => (launcher.renaming = null)}
              disabled={launcher.detailBusy !== null}
            >
              Cancel
            </button>
          </div>
        </div>
      {/if}

      <h4 class="detail-section-title">General</h4>
      <div class="field-grid">
        <label class="field">
          <span class="field-label">Minecraft version</span>
          <select
            bind:value={draft.minecraftVersion}
            onchange={onDraftChange}
            onfocus={() => launcher.loadMinecraftVersions(false)}
            onclick={() => launcher.loadMinecraftVersions(false)}
          >
            {#if launcher.minecraftVersions === null}
              <option value={draft.minecraftVersion}>{draft.minecraftVersion}</option>
            {:else}
              {#each launcher.minecraftVersions as version (version.id)}
                <option value={version.id}>{version.id}</option>
              {/each}
            {/if}
          </select>
          <span class="field-hint">Changing this requires installing new content.</span>
        </label>
        <label class="field">
          <span class="field-label">Mod loader</span>
          <select value="fabric" disabled>
            <option value="fabric">Fabric</option>
          </select>
        </label>
        <label class="field">
          <span class="field-label">Fabric Loader version</span>
          {#if draft.loader.policy.type === "automatic"}
            <select value="" onchange={setLoaderPolicy}>
              <option value="">Latest compatible</option>
              {#each launcher.loaderVersions ?? [] as loader (loader.version)}
                <option value={loader.version}>
                  {loader.version}{loader.stable ? "" : " (unstable)"}
                </option>
              {/each}
            </select>
            <span class="field-hint">
              Automatically uses the newest stable loader for this Minecraft version.
            </span>
          {:else}
            <select value={loaderVersionValue(draft)} onchange={onLoaderVersionChange}>
              <option value="">Latest compatible</option>
              {#each launcher.loaderVersions ?? [] as loader (loader.version)}
                <option value={loader.version}>
                  {loader.version}{loader.stable ? "" : " (unstable)"}
                </option>
              {/each}
            </select>
          {/if}
        </label>
      </div>

      <h4 class="detail-section-title">Performance</h4>
      <div class="field-grid">
        <label class="field">
          <span class="field-label">Memory (MB)</span>
          <input
            type="number"
            min="512"
            max="32768"
            step="256"
            bind:value={draft.memoryMib}
            onchange={onDraftChange}
            required
          />
          <span class="field-hint">
            The memory allocated to the game, between 512 and 32768 MB.
          </span>
        </label>
      </div>

      <h4 class="detail-section-title">Java</h4>
      <div class="field-grid">
        <div class="field">
          <span class="field-label">Runtime</span>
          <span class="field-value">Automatic — managed by Aurora</span>
          <span class="field-hint">
            {javaContext() ??
              "The required Java version follows the selected Minecraft version."}
          </span>
        </div>
        <label class="field field-span">
          <span class="field-label">Additional JVM arguments</span>
          <input
            type="text"
            bind:value={draft.additionalJvmArguments}
            oninput={onDraftChange}
            placeholder='e.g. -Dexample=value "-Dlabel=two words"'
          />
          <span class="field-hint">
            Passed to the Java process as separate arguments. Double quotes group text;
            heap settings (-Xmx, -Xms) and classpath flags are owned by Aurora.
          </span>
        </label>
      </div>

      <h4 class="detail-section-title">Display</h4>
      <div class="field-grid">
        <div class="field">
          <span class="field-label">Window size</span>
          <div class="window-row">
            <label class="check-field">
              <input
                type="checkbox"
                checked={draft.window !== null}
                onchange={(event) => {
                  const checked = (event.currentTarget as HTMLInputElement).checked;
                  draft.window = checked ? { width: 854, height: 480 } : null;
                  onDraftChange();
                }}
              />
              <span>Custom size</span>
            </label>
            {#if draft.window}
              <label class="field field-narrow">
                <span class="field-label">Width</span>
                <input
                  type="number"
                  min="100"
                  max="7680"
                  bind:value={draft.window.width}
                  onchange={onDraftChange}
                  required
                />
              </label>
              <label class="field field-narrow">
                <span class="field-label">Height</span>
                <input
                  type="number"
                  min="100"
                  max="7680"
                  bind:value={draft.window.height}
                  onchange={onDraftChange}
                  required
                />
              </label>
            {/if}
          </div>
        </div>
      </div>

      {#if launcher.detailError}
        <p class="inline-message inline-message-error group-row" role="alert">
          {launcher.detailError.message}
        </p>
      {/if}

      <div class="form-actions">
        <button
          type="submit"
          class="btn btn-primary"
          disabled={launcher.detailBusy !== null || !dirty}
        >
          {launcher.detailBusy === instance.id ? "Saving…" : "Save changes"}
        </button>
        {#if dirty}
          <button
            type="button"
            class="btn btn-quiet"
            onclick={() => launcher.discardDraft(instance.id)}
            disabled={launcher.detailBusy !== null}
          >
            Discard
          </button>
        {/if}
        {#if needsInstall}
          <button
            type="button"
            class="btn"
            onclick={() => void launcher.runInstallConfiguration(instance.id)}
            disabled={launcher.detailInstallBusy !== null || dirty}
          >
            {launcher.detailInstallBusy === instance.id ? "Installing…" : "Install new configuration"}
          </button>
        {/if}
        {#if launcher.detailInstallBusy === instance.id && launcher.createProgress}
          <span class="group-row-detail">
            {launcher.createProgress.phase}
            {#if launcher.createProgress.game}
              · {launcher.createProgress.game.completedItems}/{launcher.createProgress.game.totalItems}
            {/if}
          </span>
        {/if}
      </div>
    </form>
  {/if}

  <p class="group-footer">
    These settings belong to this instance. Launcher-wide preferences — appearance and
    desktop integration — live in the sidebar's Settings destination.
  </p>
</section>

<style>
  .detail-section-title {
    font-size: 0.95rem;
    font-weight: 600;
    color: var(--color-text);
    margin: var(--space-5) 0 var(--space-3);
  }

  .group-form .detail-section-title:first-of-type {
    margin-top: var(--space-2);
  }

  .field-narrow {
    max-width: 7rem;
  }

  .field-hint {
    display: block;
    margin-top: var(--space-1);
    font-size: var(--text-metadata);
    color: var(--color-text-muted);
  }

  .field-value {
    display: block;
    padding: var(--space-2) 0;
    font-size: var(--text-body);
    color: var(--color-text);
  }

  .check-field {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    color: var(--color-text-secondary);
    font-size: var(--text-body);
    cursor: pointer;
  }

  .window-row {
    display: flex;
    align-items: end;
    gap: var(--space-4);
    flex-wrap: wrap;
  }
</style>

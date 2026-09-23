<script lang="ts">
  import { launcher } from "$lib/launcher/store.svelte";
  import type { InstanceConfiguration, InstanceSummary } from "$lib/backend";

  function needsInstall(instance: InstanceSummary): boolean {
    if (instance.state !== "ready") return false;
    return (
      instance.configuration.minecraftVersion !== instance.minecraftVersion ||
      (instance.configuration.loader.policy.type === "pinned" &&
        instance.configuration.loader.policy.version !== instance.fabricLoaderVersion)
    );
  }

  const instances = $derived(launcher.launcherState?.instances ?? []);
  const selectedId = $derived(launcher.launcherState?.config.selectedInstanceId ?? null);
  const detailInstance = $derived(
    launcher.detailId
      ? (instances.find((instance) => instance.id === launcher.detailId) ?? null)
      : null,
  );
  const draft = $derived(launcher.detailDraft);

  // The create form defaults its Minecraft version to the newest release
  // the Aurora release source supports, so a fresh instance is compatible
  // by construction.
  $effect(() => {
    if (launcher.createMinecraftVersion === "" && launcher.releases.length > 0) {
      launcher.createMinecraftVersion = launcher.releases[0].minecraftVersion;
    }
  });

  function statusBadge(
    instance: InstanceSummary,
  ): { tone: string; label: string } | null {
    if (instance.state === "installing") {
      return { tone: "status-working", label: "Installing" };
    }
    const validation = launcher.instanceValidations[instance.id];
    if (validation?.status === "damaged") return { tone: "status-error", label: "Damaged" };
    if (validation?.status === "stale") return { tone: "status-warning", label: "Needs install" };
    if (validation?.status === "ready") return { tone: "status-success", label: "Verified" };
    if (instance.state === "ready") {
      const configured =
        instance.configuration.minecraftVersion === instance.minecraftVersion;
      if (!configured) return { tone: "status-warning", label: "Needs install" };
      return { tone: "status-success", label: "Ready" };
    }
    return null;
  }

  function configurationLabel(instance: InstanceSummary): string {
    const loader =
      instance.configuration.loader.policy.type === "pinned"
        ? `Fabric ${instance.configuration.loader.policy.version}`
        : "Fabric (latest compatible)";
    return `Minecraft ${instance.configuration.minecraftVersion} · ${loader}`;
  }

  function onDraftChange(): void {
    launcher.detailDirty = true;
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
      value === ""
        ? { type: "automatic" }
        : { type: "pinned", version: value };
    onDraftChange();
  }

  function onLoaderVersionChange(event: Event): void {
    if (!draft) return;
    const value = (event.currentTarget as HTMLSelectElement).value;
    draft.loader.policy = { type: "pinned", version: value };
    onDraftChange();
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
  Instance lifecycle surface: creation from the essentials, a list of what
  exists, and a detail editor for one instance's full configuration. Java
  provisioning stays here because it is instance lifecycle; launching itself
  lives on Home. The detail editor edits a draft and saves the whole proposed
  configuration atomically; install-affecting changes are then applied
  through an explicit install step, never by a dropdown changing content.
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
        {@const open = launcher.detailId === instance.id}
        <div class="group-row" class:group-row-selected={selected}>
          <div class="group-row-main">
            <span class="instance-name-line">
              <span class="group-row-title">{instance.displayName}</span>
              {#if selected}<span class="row-marker">Selected</span>{/if}
            </span>
            <span class="group-row-detail">{configurationLabel(instance)}</span>
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
            {:else if validation?.status === "stale"}
              <span class="group-row-detail problem-line">
                The saved configuration differs from the installed content — open the instance
                and install the new configuration.
              </span>
            {:else if validation?.status === "ready"}
              <span class="group-row-detail">Deep validation passed.</span>
            {/if}
          </div>

          <div class="group-row-actions">
            {#if badge}<span class="status-badge {badge.tone}">{badge.label}</span>{/if}
            {#if !open}
              <button
                type="button"
                class="btn"
                onclick={() => launcher.openDetail(instance.id)}
                disabled={busy || launcher.createBusy}
              >
                Configure
              </button>
            {:else}
              <button
                type="button"
                class="btn"
                onclick={() => launcher.closeDetail()}
                disabled={launcher.detailBusy || launcher.detailInstallBusy}
              >
                Close
              </button>
            {/if}
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

        {#if open && detailInstance && draft}
          {@const java = javaBadge()}
          <div class="detail-panel">
            <form
              class="group-form"
              onsubmit={(event) => {
                event.preventDefault();
                void launcher.runSaveConfiguration();
              }}
            >
              <div class="detail-heading">
                <div>
                  <h4 class="group-title">{detailInstance.displayName}</h4>
                  <p class="group-subtitle">
                    Installed: Minecraft {detailInstance.minecraftVersion} · Fabric
                    {detailInstance.fabricLoaderVersion}
                  </p>
                </div>
                <button
                  type="button"
                  class="btn btn-quiet"
                  onclick={() => {
                    launcher.renaming = {
                      id: detailInstance.id,
                      name: detailInstance.displayName,
                    };
                  }}
                  disabled={launcher.detailBusy}
                >
                  Rename
                </button>
              </div>

              {#if launcher.renaming?.id === detailInstance.id}
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
                      disabled={launcher.detailBusy}
                    >
                      Save name
                    </button>
                    <button
                      type="button"
                      class="btn btn-quiet"
                      onclick={() => (launcher.renaming = null)}
                      disabled={launcher.detailBusy}
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
                    <select
                      value={loaderVersionValue(draft)}
                      onchange={onLoaderVersionChange}
                    >
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
                    {javaDetail() ??
                      "The required Java version follows the selected Minecraft version."}
                  </span>
                </div>
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
                          draft.window = checked
                            ? { width: 854, height: 480 }
                            : null;
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

              <h4 class="detail-section-title">Advanced</h4>
              <div class="field-grid">
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

              {#if launcher.detailError}
                <p class="inline-message inline-message-error group-row" role="alert">
                  {launcher.detailError.message}
                </p>
              {/if}

              <div class="form-actions">
                <button
                  type="submit"
                  class="btn btn-primary"
                  disabled={launcher.detailBusy || !launcher.detailDirty}
                >
                  {launcher.detailBusy ? "Saving…" : "Save changes"}
                </button>
                {#if launcher.detailDirty}
                  <button
                    type="button"
                    class="btn btn-quiet"
                    onclick={() => launcher.openDetail(detailInstance.id)}
                    disabled={launcher.detailBusy}
                  >
                    Discard
                  </button>
                {/if}
                {#if needsInstall(detailInstance)}
                  <button
                    type="button"
                    class="btn"
                    onclick={() => void launcher.runInstallConfiguration()}
                    disabled={launcher.detailInstallBusy || launcher.detailDirty}
                  >
                    {launcher.detailInstallBusy ? "Installing…" : "Install new configuration"}
                  </button>
                {/if}
                {#if launcher.detailInstallBusy && launcher.createProgress}
                  <span class="group-row-detail">
                    {launcher.createProgress.phase}
                    {#if launcher.createProgress.game}
                      · {launcher.createProgress.game.completedItems}/{launcher.createProgress.game.totalItems}
                    {/if}
                  </span>
                {/if}
              </div>
            </form>

            {#if selected && detailInstance.state === "ready"}
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
                    onclick={() => launcher.runRuntimeStatus(detailInstance.id)}
                    disabled={launcher.runtimeBusy || launcher.createBusy}
                  >
                    {launcher.runtimeBusy ? "Checking…" : "Check Java"}
                  </button>
                  {#if launcher.runtimeStatus?.instanceId === detailInstance.id && launcher.runtimeStatus.status !== "ready" && !launcher.runtimeBusy}
                    <button
                      type="button"
                      class="btn"
                      onclick={() => launcher.runEnsureRuntime(detailInstance.id)}
                      disabled={launcher.createBusy}
                    >
                      {launcher.runtimeStatus.status === "damaged" ? "Repair Java" : "Install Java"}
                    </button>
                  {/if}
                </div>
              </div>
            {/if}
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
      artifact — validated before they are reported ready. Name, memory, JVM arguments, and
      window changes apply immediately; Minecraft and loader changes are installed through the
      button in the instance's settings. Instance deletion remains deliberately unimplemented.
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

  /* The detail editor belongs to the row above it; a quiet inset keeps the
     association visible without nesting surfaces. */
  .detail-panel {
    padding: var(--space-4) var(--space-4) var(--space-4) var(--space-6);
    background: var(--color-surface-sunken);
    border-top: 1px solid var(--color-border);
  }

  .detail-heading {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--space-4);
    flex-wrap: wrap;
    margin-bottom: var(--space-4);
  }

  .detail-section-title {
    font-size: 0.95rem;
    font-weight: 600;
    color: var(--color-text);
    margin: var(--space-5) 0 var(--space-3);
  }

  .detail-panel .detail-section-title:first-of-type {
    margin-top: var(--space-2);
  }

  .group-row-sub {
    padding-left: var(--space-6);
    background: var(--color-surface);
  }

  .field-nested {
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

  .problem-line {
    color: var(--color-warning);
  }

  .detail-panel .problem-line {
    color: var(--color-error);
  }

  .group-footer code {
    font-family: var(--font-mono);
    font-size: var(--text-metadata);
  }
</style>

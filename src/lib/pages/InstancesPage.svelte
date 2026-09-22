<script lang="ts">
  import { launcher } from "$lib/launcher/store.svelte";
</script>

<!--
  Existing instance management surface, intentionally not redesigned in the
  shell/Home pilot: the internal content keeps its established presentation
  and every control until its own design phase.
-->
<div class="page">
  <header class="page-header">
    <div>
      <h2 class="page-title">Instances</h2>
      <p class="page-subtitle">Isolated installations Aurora launches from.</p>
    </div>
  </header>

  <section class="status-card" aria-labelledby="instances-title" aria-live="polite">
    <div class="status-heading">
      <div>
        <p class="eyebrow">Persistent instances</p>
        <h3 id="instances-title">Instances</h3>
      </div>

      {#if launcher.createBusy}
        <span class="badge loading"><span aria-hidden="true"></span
          >{launcher.createProgress ? launcher.createProgress.phase : "Working"}</span
        >
      {:else}
        <span class="badge ready"><span aria-hidden="true"></span
          >{launcher.launcherState?.instances.length ?? 0}</span
        >
      {/if}
    </div>

    <form class="acquire-form" onsubmit={(event) => launcher.runCreateInstance(event)}>
      <label>
        <span>Display name</span>
        <input
          type="text"
          bind:value={launcher.createDisplayName}
          placeholder="e.g. My Aurora Setup"
          required
          maxlength="80"
        />
      </label>
      <label>
        <span>Channel</span>
        <select bind:value={launcher.createChannel}>
          <option value="stable">stable</option>
          <option value="beta">beta</option>
          <option value="nightly">nightly</option>
        </select>
      </label>
      <label>
        <span>Aurora release</span>
        <select bind:value={launcher.createVersion} required>
          {#each launcher.releases.filter((release) => release.channel === launcher.createChannel) as release (release.auroraVersion)}
            <option value={release.auroraVersion}>
              Aurora {release.auroraVersion} · Minecraft {release.minecraftVersion} · Fabric
              {release.fabricLoaderVersion}
            </option>
          {/each}
        </select>
      </label>
      <button
        type="submit"
        disabled={launcher.createBusy ||
          launcher.createDisplayName.trim() === "" ||
          launcher.createVersion === ""}
      >
        {launcher.createBusy ? "Creating…" : "Create instance"}
      </button>
    </form>

    {#if launcher.createBusy && launcher.createProgress}
      <dl>
        <div>
          <dt>Progress</dt>
          <dd>
            {launcher.createProgress.phase}
            {#if launcher.createProgress.game}
              · {launcher.createProgress.game.completedItems}/{launcher.createProgress.game.totalItems}
            {/if}
          </dd>
        </div>
      </dl>
    {/if}

    {#if launcher.createError}
      <div class="error-message" role="alert">
        <p>{launcher.createError.message}</p>
        <code>{launcher.createError.code}</code>
      </div>
    {/if}

    {#if launcher.releasesError}
      <div class="error-message" role="alert">
        <p>{launcher.releasesError.message}</p>
        <code>{launcher.releasesError.code}</code>
      </div>
    {:else if launcher.releases.length > 0 && launcher.releases[0].source === "development-fixture"}
      <p class="footnote">
        Aurora releases currently come from the launcher's checked-in development fixture —
        no production release infrastructure exists yet. Serve
        <code>src-tauri/development</code> on 127.0.0.1:8765 for artifact downloads.
      </p>
    {/if}

    {#if launcher.instanceError}
      <div class="error-message" role="alert">
        <p>{launcher.instanceError.message}</p>
        <code>{launcher.instanceError.code}</code>
      </div>
    {/if}

    {#if launcher.launcherState && launcher.launcherState.instances.length === 0 && !launcher.createBusy}
      <p class="footnote">No instances yet — create the first one above.</p>
    {/if}

    {#if launcher.launcherState}
      {#each launcher.launcherState.instances as instance (instance.id)}
      <div class="instance-row">
        <div class="instance-main">
          <div class="instance-title">
            <strong>{instance.displayName}</strong>
            {#if launcher.launcherState.config.selectedInstanceId === instance.id}
              <span class="badge ready"><span aria-hidden="true"></span>Selected</span>
            {/if}
            {#if instance.state === "installing"}
              <span class="badge loading"><span aria-hidden="true"></span>Installing</span>
            {:else if launcher.instanceValidations[instance.id]?.status === "damaged"}
              <span class="badge error"><span aria-hidden="true"></span>Damaged</span>
            {:else if launcher.instanceValidations[instance.id]?.status === "ready"}
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

          {#if launcher.instanceValidations[instance.id]}
            <div
              class="instance-meta validation-line"
              class:damaged={launcher.instanceValidations[instance.id].status === "damaged"}
            >
              Validation: {launcher.instanceValidations[instance.id].status}
              {#each launcher.instanceValidations[instance.id].problems as problem (problem.reason)}
                <div class="instance-meta">
                  {problem.component}: {problem.reason}
                </div>
              {/each}
            </div>
          {/if}

          {#if launcher.launcherState.config.selectedInstanceId === instance.id && instance.state === "ready"}
            <div class="instance-meta validation-line" class:damaged={launcher.runtimeStatus?.status === "damaged"}>
              Content: ready · Java:
              {#if launcher.runtimeBusy}
                {launcher.runtimeProgress
                  ? `${launcher.runtimeProgress.phase} ${launcher.runtimeProgress.completedItems}/${launcher.runtimeProgress.totalItems}`
                  : "resolving"}
              {:else if launcher.runtimeStatus?.instanceId === instance.id}
                {launcher.runtimeStatus.status} · {launcher.runtimeStatus.component} · required {launcher.runtimeStatus.requiredMajorVersion}{#if launcher.runtimeStatus.runtimeVersion}
                  · installed {launcher.runtimeStatus.runtimeVersion}
                {/if}
                {#if launcher.runtimeStatus.reused === true} · reused verified runtime{/if}
                {#each launcher.runtimeStatus.problems as problem}
                  <div class="instance-meta">Java: {problem}</div>
                {/each}
              {:else}
                not checked
              {/if}
            </div>
            {#if launcher.runtimeError}
              <div class="error-message" role="alert">
                <p>{launcher.runtimeError.message}</p>
                <code>{launcher.runtimeError.code}</code>
              </div>
            {/if}
            <div class="instance-meta validation-line" class:damaged={launcher.playReadiness && !launcher.playReadiness.ready}>
              Play:
              {#if launcher.playReadinessBusy}
                checking prerequisites
              {:else if launcher.playProcess?.instanceId === instance.id && launcher.playProcess.status === "running"}
                running
              {:else if launcher.playProcess?.instanceId === instance.id && launcher.playProcess.status === "starting"}
                starting
              {:else if launcher.playReadiness?.instanceId === instance.id}
                {launcher.playReadiness.ready ? "ready" : "blocked"} · Account:
                {launcher.playReadiness.accountName ?? "none selected"}
                {#each launcher.playReadiness.blockers as blocker (blocker.code)}
                  <div class="instance-meta">{blocker.message}</div>
                {/each}
              {:else}
                not checked
              {/if}
            </div>
            {#if launcher.playProgress && launcher.playBusy}
              <div class="instance-meta">Launch: {launcher.playProgress.phase}</div>
            {/if}
            {#if launcher.playProcess?.instanceId === instance.id && launcher.playProcess.status === "failed"}
              <div class="error-message" role="alert">
                <p>{launcher.playProcess.message ?? "Minecraft exited unsuccessfully."}</p>
                {#if launcher.playProcess.exitCode !== null}<code>exit {launcher.playProcess.exitCode}</code>{/if}
              </div>
            {/if}
            {#if launcher.playError}
              <div class="error-message" role="alert">
                <p>{launcher.playError.message}</p>
                <code>{launcher.playError.code}</code>
              </div>
            {/if}
          {/if}
        </div>

        <div class="instance-actions">
          {#if launcher.renaming?.id === instance.id}
            <form class="rename-form" onsubmit={(event) => launcher.runRename(event)}>
              <input
                type="text"
                bind:value={launcher.renaming.name}
                required
                maxlength="80"
                placeholder="New display name"
              />
              <button type="submit" disabled={launcher.instanceBusy === instance.id}>Save</button>
              <button
                type="button"
                onclick={() => (launcher.renaming = null)}
                disabled={launcher.instanceBusy === instance.id}
              >
                Cancel
              </button>
            </form>
          {:else}
            {#if launcher.launcherState.config.selectedInstanceId !== instance.id}
              <button
                type="button"
                onclick={() => launcher.runSelect(instance.id)}
                disabled={launcher.instanceBusy === instance.id || launcher.createBusy}
              >
                Select
              </button>
            {/if}
            <button
              type="button"
              onclick={() => (launcher.renaming = { id: instance.id, name: instance.displayName })}
              disabled={launcher.instanceBusy === instance.id || launcher.createBusy}
            >
              Rename
            </button>
            {#if instance.state === "installing"}
              <button
                type="button"
                onclick={() => launcher.runRetry(instance.id)}
                disabled={launcher.instanceBusy === instance.id || launcher.createBusy}
              >
                Retry install
              </button>
            {/if}
            <button
              type="button"
              onclick={() => launcher.runValidate(instance.id)}
              disabled={launcher.instanceBusy === instance.id || launcher.createBusy}
            >
              Validate
            </button>
            {#if launcher.launcherState.config.selectedInstanceId === instance.id && instance.state === "ready"}
              <button
                class="primary-action"
                type="button"
                onclick={() => launcher.runPlay(instance.id)}
                disabled={launcher.playBusy || launcher.playReadinessBusy || !launcher.playReadiness?.ready}
              >
                {launcher.playProcess?.instanceId === instance.id && launcher.playProcess.status === "running"
                  ? "Running"
                  : launcher.playBusy
                    ? "Starting…"
                    : "Play"}
              </button>
              <button
                type="button"
                onclick={() => launcher.refreshPlayReadiness()}
                disabled={launcher.playBusy || launcher.playReadinessBusy}
              >
                {launcher.playReadinessBusy ? "Checking…" : "Check Play"}
              </button>
              <button
                type="button"
                onclick={() => launcher.runRuntimeStatus(instance.id)}
                disabled={launcher.runtimeBusy || launcher.instanceBusy === instance.id || launcher.createBusy}
              >
                Check Java
              </button>
              {#if launcher.runtimeStatus?.instanceId === instance.id && launcher.runtimeStatus.status !== "ready"}
                <button
                  type="button"
                  onclick={() => launcher.runEnsureRuntime(instance.id)}
                  disabled={launcher.runtimeBusy || launcher.instanceBusy === instance.id || launcher.createBusy}
                >
                  {launcher.runtimeStatus.status === "damaged" ? "Repair Java" : "Install Java"}
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
      acquire and validate its official shared Mojang Java runtime independently, then launch
      through the supervised Play pipeline. Instance deletion remains deliberately unimplemented.
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

  .instance-actions button.primary-action {
    border-color: #7b6cf5;
    background: #6f5df2;
    color: #ffffff;
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

  .error-message code {
    display: inline-block;
    margin-top: 0.7rem;
    color: #ff9a9a;
  }
</style>

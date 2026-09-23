<script lang="ts">
  import { launcher } from "$lib/launcher/store.svelte";
  import { navigation } from "$lib/launcher/navigation.svelte";
  import ReadinessRows from "$lib/instances/ReadinessRows.svelte";

  const instance = $derived(launcher.selectedInstance);
  const readiness = $derived(
    launcher.playReadiness && launcher.playReadiness.instanceId === instance?.id
      ? launcher.playReadiness
      : null,
  );
  const process = $derived(
    launcher.playProcess && launcher.playProcess.instanceId === instance?.id
      ? launcher.playProcess
      : null,
  );

  const playLabel = $derived(
    process?.status === "running"
      ? "Running"
      : launcher.playBusy
        ? "Starting…"
        : "Play",
  );
  const playDisabled = $derived(
    launcher.playBusy || launcher.playReadinessBusy || !readiness?.ready,
  );
</script>

<!--
  Aurora's fast launch surface: the selected instance and everything Play
  needs, with the readiness decision Rust owns rendered verbatim. The full
  per-instance workspace lives behind Instances — Home stays a launch
  surface, sharing its readiness rows with the workspace Overview.
-->
<div class="page">
  <header class="page-header">
    <div>
      <h2 class="page-title">Home</h2>
      <p class="page-subtitle">Your selected instance and everything Play needs.</p>
    </div>
  </header>

  {#if launcher.stateError}
    <section class="group" aria-live="polite">
      <div class="group-heading">
        <div>
          <h3 class="group-title">Launcher state</h3>
          <p class="group-subtitle">Persisted launcher state could not be loaded.</p>
        </div>
        <span class="status-badge status-error">Error</span>
      </div>
      <p class="inline-message inline-message-error group-row" role="alert">
        {launcher.stateError.message}
        <code>{launcher.stateError.code}</code>
      </p>
    </section>
  {:else if !launcher.launcherState}
    <section class="group" aria-live="polite">
      <div class="group-row group-row-loading">
        <span class="spinner" aria-hidden="true"></span>
        <p class="group-row-detail">Loading persisted launcher state…</p>
      </div>
    </section>
  {:else if !instance}
    {#if launcher.launcherState.instances.length === 0}
      <section class="empty-state" aria-live="polite">
        <h3 class="empty-title">No instances yet</h3>
        <p class="empty-detail">
          Create an isolated Minecraft installation to get started.
        </p>
        <button type="button" class="btn btn-primary" onclick={() => navigation.goTo("instances")}>
          Create instance
        </button>
      </section>
    {:else}
      <section class="empty-state" aria-live="polite">
        <h3 class="empty-title">No instance selected</h3>
        <p class="empty-detail">Choose an instance to launch from.</p>
        <button type="button" class="btn btn-primary" onclick={() => navigation.goTo("instances")}>
          Go to Instances
        </button>
      </section>
    {/if}
  {:else}
    <section class="group" aria-live="polite">
      <div class="group-heading">
        <div class="instance-heading">
          <h3 class="instance-name">{instance.displayName}</h3>
          <p class="instance-versions">
            Aurora {instance.auroraVersion} ({instance.channel}) · Minecraft
            {instance.minecraftVersion} · Fabric {instance.fabricLoaderVersion}
          </p>
        </div>
        <div class="play-actions">
          <button
            type="button"
            class="btn btn-primary"
            onclick={() => launcher.runPlay(instance.id)}
            disabled={playDisabled}
          >
            {playLabel}
          </button>
          <button
            type="button"
            class="btn btn-quiet"
            onclick={() => launcher.refreshPlayReadiness()}
            disabled={launcher.playBusy || launcher.playReadinessBusy}
          >
            {launcher.playReadinessBusy ? "Checking…" : "Check again"}
          </button>
        </div>
      </div>

      <ReadinessRows {instance} />

      <p class="group-footer">
        Readiness is decided by Aurora from validated content, the exact managed Java runtime,
        and a usable authenticated session — the button reflects that decision.
      </p>
    </section>
  {/if}
</div>

<style>
  .instance-heading {
    min-width: 0;
  }

  .instance-name {
    margin: 0;
    font-size: 1.05rem;
    font-weight: 600;
    letter-spacing: -0.01em;
    overflow-wrap: anywhere;
  }

  .instance-versions {
    margin: var(--space-1) 0 0;
    color: var(--color-text-secondary);
    font-size: var(--text-metadata);
  }

  .play-actions {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-wrap: wrap;
    justify-content: flex-end;
  }
</style>

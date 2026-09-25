<script lang="ts">
  import { launcher } from "$lib/launcher/store.svelte";
  import { navigation } from "$lib/launcher/navigation.svelte";
  import { configurationRequiresInstall } from "$lib/launcher/instanceStatus";
  import ReadinessRows from "$lib/instances/ReadinessRows.svelte";
  import type { InstanceSummary } from "$lib/backend";

  let { instance }: { instance: InstanceSummary } = $props();

  const isSelected = $derived(
    launcher.launcherState?.config.selectedInstanceId === instance.id,
  );
  const busy = $derived(launcher.instanceBusy === instance.id);

  const loaderPolicy = $derived(
    instance.configuration.loader.policy.type === "pinned"
      ? `Fabric ${instance.configuration.loader.policy.version}`
      : "Fabric (release version)",
  );

  function windowLabel(): string {
    const window = instance.configuration.window;
    return window ? `${window.width} × ${window.height}` : "Default";
  }
</script>

<!--
  The workspace Overview: what this instance is, whether it is ready, which
  configuration is active, and what to do next — compact grouped surfaces
  over data Aurora already owns. Deep validation and Java checks are
  explicit, on-demand actions; nothing polls while the tab is open.
-->
<section class="group" aria-labelledby="readiness-title">
  <div class="group-heading">
    <div>
      <h3 class="group-title" id="readiness-title">Readiness</h3>
    </div>
    <div class="group-row-actions">
      {#if instance.state === "installing"}
        <button
          type="button"
          class="btn"
          onclick={() => launcher.runRetry(instance.id)}
          disabled={busy || launcher.createBusy}
        >
          Retry install
        </button>
      {:else}
        <button
          type="button"
          class="btn btn-quiet"
          onclick={() => launcher.refreshPlayReadiness()}
          disabled={launcher.playBusy || launcher.playReadinessBusy || !isSelected}
          title={isSelected ? undefined : "Select this instance to refresh Play readiness"}
        >
          {launcher.playReadinessBusy ? "Checking…" : "Check again"}
        </button>
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

  <ReadinessRows {instance} />

  {#if !isSelected}
    <div class="group-row">
      <div class="group-row-main">
        <span class="group-row-title">Selected instance</span>
        <span class="group-row-detail">
          This instance is not the one Play launches from — selecting it makes it the
          launch target.
        </span>
      </div>
      <div class="group-row-actions">
        <button
          type="button"
          class="btn"
          onclick={() => launcher.runSelect(instance.id)}
          disabled={busy || launcher.createBusy}
        >
          Select
        </button>
      </div>
    </div>
  {/if}

  <p class="group-footer">
    Readiness is decided by Aurora from validated content, the exact managed Java runtime,
    and a usable authenticated session — the Play button reflects that decision.
  </p>
</section>

<section class="group" aria-labelledby="configuration-title">
  <div class="group-heading">
    <div>
      <h3 class="group-title" id="configuration-title">Active configuration</h3>
      <p class="group-subtitle">The saved desired configuration launch uses.</p>
    </div>
    <div class="group-row-actions">
      <button
        type="button"
        class="btn btn-quiet"
        onclick={() => navigation.setInstanceTab("settings")}
      >
        Change in Settings
      </button>
    </div>
  </div>

  <div class="group-row">
    <span class="group-row-title">Minecraft version</span>
    <span class="group-row-value" class:warning-value={configurationRequiresInstall(instance)}>
      {instance.configuration.minecraftVersion}
    </span>
  </div>
  <div class="group-row">
    <span class="group-row-title">Mod loader</span>
    <span class="group-row-value">{loaderPolicy}</span>
  </div>
  <div class="group-row">
    <span class="group-row-title">Memory</span>
    <span class="group-row-value">{instance.configuration.memoryMib} MB</span>
  </div>
  <div class="group-row">
    <span class="group-row-title">Additional JVM arguments</span>
    <span class="group-row-value">{instance.configuration.additionalJvmArguments || "None"}</span>
  </div>
  <div class="group-row">
    <span class="group-row-title">Window size</span>
    <span class="group-row-value">{windowLabel()}</span>
  </div>

  <p class="group-footer">
    Name, memory, JVM arguments, and window changes apply immediately after saving.
    Minecraft and loader changes take effect through a deliberate install of the new
    configuration.
  </p>
</section>

<section class="group" aria-labelledby="release-title">
  <div class="group-heading">
    <div>
      <h3 class="group-title" id="release-title">Installed release</h3>
      <p class="group-subtitle">
        The concrete release pin installed content must match.
      </p>
    </div>
  </div>

  <div class="group-row">
    <span class="group-row-title">Aurora</span>
    <span class="group-row-value">{instance.auroraVersion} ({instance.channel})</span>
  </div>
  <div class="group-row">
    <span class="group-row-title">Minecraft</span>
    <span class="group-row-value">{instance.minecraftVersion}</span>
  </div>
  <div class="group-row">
    <span class="group-row-title">Fabric Loader</span>
    <span class="group-row-value">{instance.fabricLoaderVersion}</span>
  </div>

  <p class="group-footer">
    Instances pin concrete releases and never move between channels on their own.
    Production releases are bundled with the launcher; debug builds also offer development fixtures.
  </p>
</section>

<style>
  .warning-value {
    color: var(--color-warning);
  }
</style>
